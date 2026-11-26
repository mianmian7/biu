import { isJapaneseLyrics } from "@shared/lyrics/romaji";

import type { RomajiRequest, RomajiResponse, loadTokenizer } from "./romaji-worker";

import { ensureMediaBase, invokeTauri } from "./index";

const IDLE_RELEASE_MS = 8_000;
const CACHE_MAX_ENTRIES = 64;
const CACHE_MAX_BYTES = 200 * 1024;

type Result = (string | null)[];

const cache = new Map<string, { result: Result; bytes: number }>();
let cacheBytes = 0;
const inFlight = new Map<string, Promise<Result>>();
const pending = new Map<number, { resolve: (result: Result) => void; reject: (error: unknown) => void }>();
let requestId = 0;
let worker: Worker | undefined;
let inlineTokenizer: ReturnType<typeof loadTokenizer> | undefined;
let releaseTimer: ReturnType<typeof setTimeout> | undefined;

function getWorker() {
  if (worker) return worker;
  const instance = new Worker(new URL("./romaji-worker.ts", import.meta.url), { type: "module" });
  const fail = (error: Error) => {
    instance.terminate();
    if (worker !== instance) return;
    worker = undefined;
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  };
  instance.onerror = event => fail(new Error(event.message || "Romaji Web Worker error"));
  instance.onmessageerror = () => fail(new Error("Invalid Romaji Web Worker response"));
  instance.onmessage = (event: MessageEvent<RomajiResponse>) => {
    const response = event.data;
    const request = pending.get(response.id);
    if (!request) return;
    pending.delete(response.id);
    if ("error" in response) request.reject(new Error(response.error));
    else request.resolve(response.result);
  };
  worker = instance;
  return instance;
}

async function romanize(lines: string[]): Promise<Result> {
  await invokeTauri<void>("kuromoji_ensure_dict");
  const base = await ensureMediaBase();
  if (typeof Worker === "undefined") {
    const { loadTokenizer, romanizeWithTokenizer } = await import("./romaji-worker");
    inlineTokenizer ??= loadTokenizer(base).catch(error => {
      inlineTokenizer = undefined;
      throw error;
    });
    return romanizeWithTokenizer(await inlineTokenizer, lines);
  }

  const instance = getWorker();
  return new Promise((resolve, reject) => {
    const id = ++requestId;
    pending.set(id, { resolve, reject });
    try {
      instance.postMessage({ id, base, lines } satisfies RomajiRequest);
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

function cacheResult(key: string, result: Result) {
  const bytes = 2 * (key.length + result.reduce((length, line) => length + (line?.length ?? 0), 0));
  if (bytes > CACHE_MAX_BYTES) return;
  cache.set(key, { result, bytes });
  cacheBytes += bytes;
  for (const [oldKey, old] of cache) {
    if (cache.size <= CACHE_MAX_ENTRIES && cacheBytes <= CACHE_MAX_BYTES) break;
    cache.delete(oldKey);
    cacheBytes -= old.bytes;
  }
}

export async function romanizeLyricLines(lines: readonly string[]): Promise<Result> {
  if (!Array.isArray(lines)) return [];
  const normalized = lines.map(line => (typeof line === "string" ? line.trim() : ""));
  if (!isJapaneseLyrics(normalized)) return normalized.map(() => null);

  const key = JSON.stringify(normalized);
  const cached = cache.get(key);
  if (cached) {
    cache.delete(key);
    cache.set(key, cached);
    return cached.result.slice();
  }

  let task = inFlight.get(key);
  if (!task) {
    clearTimeout(releaseTimer);
    task = romanize(normalized)
      .then(result => {
        cacheResult(key, result);
        return result;
      })
      .catch(error => {
        void invokeTauri("frontend_log", {
          level: "warn",
          message: `[lyrics-romaji] romanize failed: ${error instanceof Error ? error.message : String(error)}`,
        }).catch(() => {});
        return normalized.map(() => null);
      })
      .finally(() => {
        inFlight.delete(key);
        if (inFlight.size === 0) {
          releaseTimer = setTimeout(() => {
            worker?.terminate();
            worker = undefined;
            inlineTokenizer = undefined;
          }, IDLE_RELEASE_MS);
        }
      });
    inFlight.set(key, task);
  }
  return (await task).slice();
}
