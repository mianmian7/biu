import { app } from "electron";
import log from "electron-log";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { z } from "zod";

import type { Identification } from "../../shared/auto-lyrics/identify";
import type { AutoLyricsResult } from "../../shared/auto-lyrics/service-types";

import { autoLyricsService } from "./runtime";
import { sources } from "./sources";

const caseSchema = z.object({
  bvid: z.string().regex(/^BV[\da-zA-Z]{10}$/),
  page: z.number().int().positive().default(1),
});
const configSchema = z.object({
  cases: z.array(caseSchema).default([]),
  identifyOnly: z.array(caseSchema.extend({ expect: z.unknown().optional() })).default([]),
  outDir: z.string().min(1),
  repeat: z.number().int().min(1).default(1),
});

export const benchConfigPath = process.argv
  .find(arg => arg.startsWith("--auto-lyrics-bench="))
  ?.split("=")
  .slice(1)
  .join("=");

function memorySampler() {
  const peaks = new Map<
    number,
    { pid: number; type: string; name?: string; workingSetKB: number; privateKB: number }
  >();
  const samples: { atMs: number; mainKB: number; utilityKB: number }[] = [];
  const started = performance.now();
  const sample = () => {
    let mainKB = 0;
    let utilityKB = 0;
    for (const metric of app.getAppMetrics()) {
      if (metric.type !== "Browser" && metric.type !== "Utility") continue;
      const previous = peaks.get(metric.pid);
      const workingSetKB = metric.memory.workingSetSize;
      peaks.set(metric.pid, {
        pid: metric.pid,
        type: metric.type,
        name: metric.name,
        workingSetKB: Math.max(previous?.workingSetKB ?? 0, workingSetKB),
        privateKB: Math.max(previous?.privateKB ?? 0, metric.memory.privateBytes ?? 0),
      });
      if (metric.type === "Browser") mainKB += workingSetKB;
      else utilityKB += workingSetKB;
    }
    samples.push({ atMs: performance.now() - started, mainKB, utilityKB });
  };
  sample();
  const interval = setInterval(sample, 200);
  return () => {
    clearInterval(interval);
    sample();
    return { peaks: [...peaks.values()], samples, intervalMs: 200 };
  };
}

async function videoParams(item: z.infer<typeof caseSchema>, signal: AbortSignal) {
  const video = await sources.view(item.bvid, signal);
  const page = video.pages.find(page => page.page === item.page);
  if (!page) throw new Error(`Page ${item.page} not found for ${item.bvid}`);
  return {
    bvid: video.bvid,
    cid: page.cid,
    title: video.title,
    pageTitle: page.part,
    duration: page.duration,
    tid: video.tid,
    ownerName: video.ownerName,
  };
}

/** Explicit opt-in only. Network failures are results; malformed configuration is fatal. */
export async function runAutoLyricsBench(configPath: string): Promise<void> {
  const config = configSchema.parse(JSON.parse(await readFile(configPath, "utf8")));
  const outDir = path.resolve(path.dirname(configPath), config.outDir);
  await mkdir(outDir, { recursive: true });
  const results: unknown[] = [];
  let sequence = 0;
  for (const [mode, cases] of [
    ["pipeline", config.cases],
    ["identify", config.identifyOnly],
  ] as const) {
    for (const item of cases) {
      for (let iteration = 0; iteration < (mode === "pipeline" ? config.repeat : 1); iteration++) {
        const startedAt = new Date().toISOString();
        const started = performance.now();
        const finishMemory = memorySampler();
        const controller = new AbortController();
        const events: unknown[] = [];
        const unsubscribe = autoLyricsService.onProgress(event => events.push(event));
        let result: AutoLyricsResult | Identification | undefined;
        let error: string | undefined;
        let viewDoneMs: number | undefined;
        try {
          const params = await videoParams(item, controller.signal);
          viewDoneMs = performance.now() - started;
          result =
            mode === "identify"
              ? await autoLyricsService.identify(params, controller.signal)
              : await autoLyricsService.start({ ...params, bypassCache: true });
        } catch (cause) {
          error = cause instanceof Error ? cause.message : String(cause);
        } finally {
          unsubscribe();
        }
        const row = {
          mode,
          ...item,
          iteration,
          coldStart: mode === "pipeline" && sequence === 0,
          startedAt,
          viewDoneMs,
          totalMs: performance.now() - started,
          status: result?.status ?? "error",
          result,
          error: error ?? (result && "error" in result ? result.error : undefined),
          events,
          memory: finishMemory(),
          decode: "ffmpeg s16le mono 22050Hz",
        };
        results.push(row);
        await writeFile(
          path.join(outDir, `${String(sequence++).padStart(3, "0")}-${item.bvid}-p${item.page}-${mode}.json`),
          JSON.stringify(row, null, 2),
        );
        await writeFile(path.join(outDir, "results.json"), JSON.stringify(results, null, 2));
        log.info("[auto-lyrics-bench]", {
          bvid: item.bvid,
          page: item.page,
          mode,
          iteration,
          totalMs: row.totalMs,
          error,
        });
      }
    }
  }
}
