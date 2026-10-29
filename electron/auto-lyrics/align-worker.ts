import { performance } from "node:perf_hooks";

import { alignLinesGapDTW, estimateGlobalOffset } from "../../shared/auto-lyrics/align";
import { AudioFeatures } from "../../shared/auto-lyrics/features";
import { alignRequestSchema } from "./worker-protocol";

process.parentPort.once("message", event => {
  const port = event.ports[0];
  if (!port) {
    process.parentPort.postMessage({ kind: "error", error: "Missing alignment MessagePort" });
    return;
  }
  port.once("message", message => {
    const started = performance.now();
    try {
      const request = alignRequestSchema.parse(message.data);
      const extractor = new AudioFeatures({ sampleRate: 22050 });
      const studio = extractor.extract(request.studio);
      const live = extractor.extract(request.live);
      const coarse = estimateGlobalOffset(studio, live);
      port.postMessage({
        kind: "coarse",
        coarse,
        elapsedMs: performance.now() - started,
        memoryPeakKB: process.resourceUsage().maxRSS,
      });
      const lines = alignLinesGapDTW(request.lines, studio, live);
      port.postMessage({
        kind: "fine",
        coarse,
        lines,
        elapsedMs: performance.now() - started,
        memoryPeakKB: process.resourceUsage().maxRSS,
      });
    } catch (error) {
      port.postMessage({ kind: "error", error: error instanceof Error ? error.message : String(error) });
    }
  });
  port.start();
});
