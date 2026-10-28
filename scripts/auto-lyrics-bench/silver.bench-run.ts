import path from "node:path";
import { expect, test } from "vitest";

import type { Lines, SilverEntry } from "./common";

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
function disambiguate(entries: SilverEntry[], lines: Lines): SilverEntry[] {
  const norm = (text: string) => text.replace(/[\s\p{P}\p{S}]+/gu, "");
  const byText = new Map<string, Lines>();
  for (const line of lines) {
    const key = norm(line.text);
    const group = byText.get(key) || [];
    group.push(line);
    byText.set(key, group);
  }
  const studioOf = new Map(lines.map(line => [line.idx, line.studio_start]));
  const textOf = new Map(lines.map(line => [line.idx, norm(line.text)]));
  const candidates = (entry: SilverEntry) =>
    byText.get(requireValue(textOf.get(entry.idx), `line text ${entry.idx}`)) || [];
  const unique = entries.filter(entry => candidates(entry).length === 1);
  for (const entry of entries) {
    const cands = candidates(entry);
    if (cands.length < 2 || !unique.length) continue;
    const prev = [...unique].reverse().find(anchor => anchor.live_start <= entry.live_start);
    const next = unique.find(anchor => anchor.live_start >= entry.live_start);
    const anchors = [prev, next].filter(anchor => anchor !== undefined);
    const expected =
      anchors.reduce(
        (sum, anchor) =>
          sum + entry.live_start - anchor.live_start + requireValue(studioOf.get(anchor.idx), `anchor ${anchor.idx}`),
        0,
      ) / anchors.length;
    entry.idx = cands.reduce((best, candidate) =>
      Math.abs(candidate.studio_start - expected) < Math.abs(best.studio_start - expected) ? candidate : best,
    ).idx;
  }
  return collapse(entries);
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

test.skipIf(!dataRoot)("silver metrics with collapsed and disambiguated ground truth", () => {
  const measurement = new Measurement();
  const results = [];
  const ids = silverIds();
  expect(ids).toHaveLength(5);
  for (const id of ids) {
    const dir = path.join(root(), "silver", id);
    const lines = linesSchema.parse(readJson(path.join(dir, "lines.json")));
    const gt = disambiguate(collapse(silverGtSchema.parse(readJson(path.join(dir, "gt.json"))).gt), lines);
    const { studioFeat, liveFeat } = loadPair(
      path.join(dir, "studio.wav"),
      path.join(dir, "live.wav"),
      algorithm,
      measurement,
      id,
    );
    const coarse = measurement.run(`${id}:coarse`, () => algorithm.estimateGlobalOffset(studioFeat, liveFeat));
    const preds = measurement.run(`${id}:align`, () => algorithm.alignLinesGapDTW(lines, studioFeat, liveFeat));
    const lrcMap = new Map(lines.map(line => [line.idx, line.studio_start]));
    const differences = gt.flatMap(entry => {
      const studioStart = lrcMap.get(entry.idx);
      return studioStart === undefined ? [] : [entry.live_start - studioStart];
    });
    const medianOffset = quantile(differences, 0.5) ?? 0;
    const coarseError = +(coarse.offsetSec - medianOffset).toFixed(3);
    const predByIdx = new Map<number, number[]>();
    for (const pred of preds) {
      const group = predByIdx.get(pred.idx) || [];
      group.push(pred.liveStartSec);
      predByIdx.set(pred.idx, group);
    }
    const frequency = new Map<number, number>();
    for (const entry of gt) frequency.set(entry.idx, (frequency.get(entry.idx) || 0) + 1);
    const all: number[] = [],
      single: number[] = [],
      repeated: number[] = [];
    let singleTotal = 0,
      repeatedTotal = 0;
    for (const entry of gt) {
      const isRepeated = (frequency.get(entry.idx) || 0) > 1;
      if (isRepeated) repeatedTotal++;
      else singleTotal++;
      const times = predByIdx.get(entry.idx);
      if (!times?.length) continue;
      const error = Math.min(...times.map(time => Math.abs(time - entry.live_start)));
      all.push(error);
      (isRepeated ? repeated : single).push(error);
    }
    results.push({
      bvid: id,
      matchedGtCount: gt.length,
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
