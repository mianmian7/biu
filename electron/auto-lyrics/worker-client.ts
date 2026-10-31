import { MessageChannelMain, utilityProcess } from "electron";
import log from "electron-log";
import path from "node:path";
import { fileURLToPath } from "node:url";

import type { selectReference } from "../../shared/auto-lyrics/align";
import type { AlignResult, GlobalOffset } from "../../shared/auto-lyrics/types";
import type { AlignRequest } from "./worker-protocol";

import { alignResponseSchema } from "./worker-protocol";

export function alignAudio(
  request: AlignRequest,
  signal: AbortSignal,
  onCoarse: (coarse: GlobalOffset) => void,
): Promise<{ coarse: GlobalOffset; lines: AlignResult[]; memoryPeakKB: number } & ReturnType<typeof selectReference>> {
  signal.throwIfAborted();
  const workerPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "align-worker.cjs");
  return new Promise((resolve, reject) => {
    const worker = utilityProcess.fork(workerPath, [], { serviceName: "AutoLyricsAlignment", stdio: "pipe" });
    const { port1, port2 } = new MessageChannelMain();
    let settled = false;
    const cleanup = () => {
      signal.removeEventListener("abort", cancel);
      port1.close();
      worker.kill();
    };
    const fail = (error: unknown) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const cancel = () => fail(signal.reason ?? new DOMException("Cancelled", "AbortError"));
    signal.addEventListener("abort", cancel, { once: true });
    worker.once("exit", code => {
      if (!settled) fail(new Error(`Alignment worker exited ${code}`));
    });
    worker.stderr?.on("data", (data: Buffer) => log.error("[auto-lyrics] worker stderr", data.toString()));
    worker.on("message", (message: unknown) => {
      const response = alignResponseSchema.safeParse(message);
      if (response.success && response.data.kind === "error") fail(new Error(response.data.error));
    });
    port1.on("message", event => {
      if (settled) return;
      const response = alignResponseSchema.safeParse(event.data);
      if (!response.success) {
        fail(new Error(`Invalid alignment response: ${response.error.message}`));
        return;
      }
      const data = response.data;
      if (data.kind === "error") {
        fail(new Error(data.error));
        return;
      }
      log.info("[auto-lyrics] worker", {
        stage: data.kind,
        elapsedMs: data.elapsedMs,
        memoryPeakKB: data.memoryPeakKB,
      });
      if (data.kind === "coarse") {
        try {
          onCoarse(data.coarse);
        } catch (error) {
          fail(error);
        }
      } else {
        settled = true;
        cleanup();
        resolve({
          coarse: data.coarse,
          lines: data.lines,
          memoryPeakKB: data.memoryPeakKB,
          referenceIndex: data.referenceIndex,
          referenceConsensus: data.referenceConsensus,
          referenceLags: data.referenceLags,
        });
      }
    });
    port1.start();
    worker.once("spawn", () => {
      if (settled) return;
      worker.postMessage({ kind: "connect" }, [port2]);
      port1.postMessage(request);
    });
    if (signal.aborted) cancel();
  });
}
