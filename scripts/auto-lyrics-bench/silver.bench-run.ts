import path from "node:path";
import { expect, test } from "vitest";

import type { Lines, SilverEntry } from "./common";

import { lyricAnchors } from "../../shared/auto-lyrics/align";
import {
  algorithm,
  dataRoot,
  linesSchema,
  loadPair,
  Measurement,
  quantile,
  readJson,
  requireValue,
  root,
  silverGtSchema,
  silverIds,
  writeReport,
} from "./common";

function collapse(entries: SilverEntry[]): SilverEntry[] {
  const out: SilverEntry[] = [];
  for (const entry of [...entries].sort((a, b) => a.live_start - b.live_start)) {
    if (out.at(-1)?.idx === entry.idx) continue;
    out.push({ ...entry });
  }
  return out;
}
function norm(text: string) {
  return text.replace(/[\s\p{P}\p{S}]+/gu, "");
}
function stats(errors: number[], totalCount: number) {
  const round = (value: number | null, digits: number) => (value === null ? null : +value.toFixed(digits));
  return {
    count: errors.length,
    totalCount,
    median: round(quantile(errors, 0.5), 3),
    p90: round(quantile(errors, 0.9), 3),
    le500ms: errors.length ? +((errors.filter(error => error <= 0.5).length / errors.length) * 100).toFixed(1) : 0,
    le1s: errors.length ? +((errors.filter(error => error <= 1).length / errors.length) * 100).toFixed(1) : 0,
    coverage: +((errors.length / Math.max(1, totalCount)) * 100).toFixed(1),
  };
}

test.skipIf(!dataRoot)("silver metrics with collapsed ground truth and text-matched predictions", () => {
  const measurement = new Measurement();
  const results = [];
  const ids = silverIds();
  expect(ids).toHaveLength(5);
  for (const id of ids) {
    const dir = path.join(root(), "silver", id);
    const lines = linesSchema.parse(readJson(path.join(dir, "lines.json")));
    const gt = collapse(silverGtSchema.parse(readJson(path.join(dir, "gt.json"))).gt);
    const { studioFeat, liveFeat } = loadPair(
      path.join(dir, "studio.wav"),
      path.join(dir, "live.wav"),
      algorithm,
      measurement,
      id,
    );
    const coarse = measurement.run(`${id}:coarse`, () =>
      algorithm.estimateGlobalOffset(studioFeat, liveFeat, null, lyricAnchors(lines)),
    );
    const preds = measurement.run(`${id}:align`, () => algorithm.alignLinesGapDTW(lines, studioFeat, liveFeat));
    const lrcMap = new Map(lines.map(line => [line.idx, line.studio_start]));
    const differences = gt.flatMap(entry => {
      const studioStart = lrcMap.get(entry.idx);
      return studioStart === undefined ? [] : [entry.live_start - studioStart];
    });
    const medianOffset = quantile(differences, 0.5) ?? 0;
    const coarseError = +(coarse.offsetSec - medianOffset).toFixed(3);
    const linesByText = new Map<string, Lines>();
    const textByIdx = new Map(lines.map(line => [line.idx, norm(line.text)]));
    for (const line of lines) {
      const key = norm(line.text);
      const group = linesByText.get(key) || [];
      group.push(line);
      linesByText.set(key, group);
    }
    const predictionsByText = new Map<string, number[]>();
    for (const pred of preds) {
      const key = requireValue(textByIdx.get(pred.idx), `line text ${pred.idx}`);
      const group = predictionsByText.get(key) || [];
      group.push(pred.liveStartSec);
      predictionsByText.set(key, group);
    }
    const signedErrors: number[] = [];
    const all: number[] = [],
      single: number[] = [],
      repeated: number[] = [];
    let singleTotal = 0,
      repeatedTotal = 0;
    for (const entry of gt) {
      const key = requireValue(textByIdx.get(entry.idx), `line text ${entry.idx}`);
      const isRepeated = (linesByText.get(key)?.length || 0) > 1;
      if (isRepeated) repeatedTotal++;
      else singleTotal++;
      const times = predictionsByText.get(key);
      if (!times?.length) continue;
      const nearest = times.reduce((best, time) =>
        Math.abs(time - entry.live_start) < Math.abs(best - entry.live_start) ? time : best,
      );
      const signed = nearest - entry.live_start;
      signedErrors.push(signed);
      const error = Math.abs(signed);
      all.push(error);
      (isRepeated ? repeated : single).push(error);
    }
    const bias = quantile(signedErrors, 0.5) ?? 0;
    const residuals = signedErrors.map(signed => Math.abs(signed - bias));
    results.push({
      bvid: id,
      matchedGtCount: gt.length,
      bias: +bias.toFixed(3),
      residual: stats(residuals, gt.length),
      outliers: residuals.filter(residual => residual > 1).length,
      coarse: {
        predOffset: +coarse.offsetSec.toFixed(3),
        gtMedianOffset: +medianOffset.toFixed(3),
        coarseError,
        absCoarseError: +Math.abs(coarseError).toFixed(3),
        confidence: coarse.confidence,
      },
      lineStats: {
        all: stats(all, gt.length),
        single: stats(single, singleTotal),
        repeated: stats(repeated, repeatedTotal),
      },
    });
  }
  writeReport("silver", results, measurement);
  console.table(
    results.map(result => ({
      bvid: result.bvid,
      coarseErrorSec: result.coarse.absCoarseError,
      bias: result.bias,
      residualMedian: result.residual.median,
      residualP90: result.residual.p90,
      outliers: result.outliers,
      ...result.lineStats.all,
    })),
  );
  console.table(
    results.flatMap(result =>
      ["single", "repeated"].map(category => ({
        bvid: result.bvid,
        category,
        ...(category === "single" ? result.lineStats.single : result.lineStats.repeated),
      })),
    ),
  );
  expect(results.filter(result => result.coarse.absCoarseError <= 1).length).toBeGreaterThanOrEqual(4);
});
