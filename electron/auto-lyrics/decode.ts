import log from "electron-log";
import { spawn } from "node:child_process";
import { performance } from "node:perf_hooks";

import { clearResolvedFfmpegPath, resolveFfmpegPath } from "../utils";
import { DecodeUnavailableError, isDecodeUnavailable } from "./ffmpeg-resolve";

export const SAMPLE_RATE = 22050;

function decodeFailure(error: unknown): unknown {
  const code = typeof error === "object" && error !== null && "code" in error ? String(error.code) : "";
  if (isDecodeUnavailable(error)) return error;
  if (code === "ENOEXEC" || code === "ENOENT") {
    clearResolvedFfmpegPath();
    return new DecodeUnavailableError(code);
  }
  return error;
}

export function decodeAudio(data: Uint8Array, signal: AbortSignal): Promise<Float32Array> {
  signal.throwIfAborted();
  let ffmpegPath: string;
  try {
    ffmpegPath = resolveFfmpegPath();
  } catch (error) {
    const failure = isDecodeUnavailable(error)
      ? error
      : new DecodeUnavailableError(error instanceof Error ? error.message : String(error));
    return Promise.reject(failure);
  }
  const started = performance.now();
  return new Promise((resolve, reject) => {
    const child = spawn(
      ffmpegPath,
      [
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        "pipe:0",
        "-f",
        "s16le",
        "-ac",
        "1",
        "-ar",
        String(SAMPLE_RATE),
        "pipe:1",
      ],
      { stdio: ["pipe", "pipe", "pipe"], windowsHide: true },
    );
    const chunks: Buffer[] = [];
    let stderr = "";
    let settled = false;
    const finish = (error?: unknown, pcm?: Float32Array) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener("abort", cancel);
      log.info("[auto-lyrics] decode", {
        elapsedMs: performance.now() - started,
        samples: pcm?.length,
        error: error ? String(error) : undefined,
      });
      if (error) reject(error);
      else resolve(pcm ?? new Float32Array());
    };
    const cancel = () => {
      child.kill("SIGKILL");
      finish(signal.reason ?? new DOMException("Cancelled", "AbortError"));
    };
    signal.addEventListener("abort", cancel, { once: true });
    child.on("error", error => finish(decodeFailure(error)));
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => {
      stderr = (stderr + chunk.toString()).slice(-4096);
    });
    child.stdin.on("error", error => {
      if (!settled) {
        child.kill("SIGKILL");
        finish(error);
      }
    });
    child.on("close", code => {
      if (settled) return;
      if (code !== 0) {
        finish(new Error(`ffmpeg exited ${code}: ${stderr}`));
        return;
      }
      const bytes = Buffer.concat(chunks);
      if (bytes.length < 2 || bytes.length % 2) {
        finish(new Error("ffmpeg returned invalid PCM"));
        return;
      }
      const pcm = new Float32Array(bytes.length / 2);
      for (let i = 0; i < pcm.length; i++) pcm[i] = bytes.readInt16LE(i * 2) / 32768;
      finish(undefined, pcm);
    });
    if (signal.aborted) cancel();
    else child.stdin.end(data);
  });
}
