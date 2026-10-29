import { performance } from "node:perf_hooks";

import { alignLinesGapDTW, estimateGlobalOffset, lyricAnchors, selectReference } from "../../shared/auto-lyrics/align";
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
      const studios = request.studios.map(pcm => extractor.extract(pcm));
      const selection = selectReference(studios);
      const studio = studios[selection.referenceIndex];
      const live = extractor.extract(request.live);
      const coarse = estimateGlobalOffset(studio, live, null, lyricAnchors(request.lines));
      port.postMessage({
        kind: "coarse",
        ...selection,
        coarse,
        elapsedMs: performance.now() - started,
        memoryPeakKB: process.resourceUsage().maxRSS,
      });
      const lines = alignLinesGapDTW(request.lines, studio, live);
      port.postMessage({
        kind: "fine",
        ...selection,
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
