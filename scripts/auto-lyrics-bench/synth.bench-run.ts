import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "vitest";

import type { Prediction, SynthGt } from "./common";

import {
  algorithm,
  dataRoot,
  manifest,
  Measurement,
  percentile,
  readJson,
  requireValue,
  root,
  studioCache,
  synthSchema,
  writeReport,
} from "./common";

interface Stats {
  errors: number[];
  total: number;
  covered: number;
  mcFp: number;
  mcSections: number;
  falseRepeats: number;
  falseRepeatSections: number;
  falseRepeatSectionSamples: number[];
  samples: number;
}
function emptyStats(): Stats {
  return {
    errors: [],
    total: 0,
    covered: 0,
    mcFp: 0,
    mcSections: 0,
    falseRepeats: 0,
    falseRepeatSections: 0,
    falseRepeatSectionSamples: [],
    samples: 0,
  };
}

// Mirrors evaluateMethod in the prototype: greedy one-to-one occurrence matching,
// a five-second pairing window, and uncertain predictions excluded from false positives.
function evaluate(preds: Prediction[], gt: SynthGt, stats: Stats) {
  stats.samples++;
  if (gt.mc_region) stats.mcSections++;
  const gtByIdx = new Map<number, number[]>();
  for (const line of gt.lines || []) {
    if (line.status !== "normal" || line.true_start === null) continue;
    const group = gtByIdx.get(line.idx) || [];
    group.push(line.true_start);
    gtByIdx.set(line.idx, group);
  }
  const predByIdx = new Map<number, Prediction[]>();
  for (const pred of preds) {
    const group = predByIdx.get(pred.idx) || [];
    group.push(pred);
    predByIdx.set(pred.idx, group);
  }
  const falseGroups = new Set<string>();
  for (const idx of new Set([...gtByIdx.keys(), ...predByIdx.keys()])) {
    const truths = gtByIdx.get(idx) || [];
    const predictions = predByIdx.get(idx) || [];
    stats.total += truths.length;
    const pairs: { i: number; j: number; dist: number }[] = [];
    for (let i = 0; i < truths.length; i++) {
      for (let j = 0; j < predictions.length; j++) {
        const dist = Math.abs(truths[i] - predictions[j].liveStartSec);
        if (dist <= 5) pairs.push({ i, j, dist });
      }
    }
    pairs.sort((a, b) => a.dist - b.dist);
    const matchedG = new Set<number>();
    const matchedP = new Set<number>();
    for (const pair of pairs) {
      if (matchedG.has(pair.i) || matchedP.has(pair.j)) continue;
      matchedG.add(pair.i);
      matchedP.add(pair.j);
      stats.errors.push(pair.dist);
      if (!predictions[pair.j].uncertain && pair.dist <= 1) stats.covered++;
    }
    predictions.forEach((pred, j) => {
      if (matchedP.has(j) || pred.uncertain) return;
      if (gt.mc_region && pred.liveStartSec >= gt.mc_region[0] && pred.liveStartSec <= gt.mc_region[1]) stats.mcFp++;
      else {
        stats.falseRepeats++;
        falseGroups.add(pred.repeatGroup || `line:${pred.idx}@${pred.liveStartSec.toFixed(2)}`);
      }
    });
  }
  stats.falseRepeatSections += falseGroups.size;
  stats.falseRepeatSectionSamples.push(falseGroups.size);
}
function summary(stats: Stats) {
  return {
    samples: stats.samples,
    totalOccurrences: stats.total,
    matchedOccurrences: stats.errors.length,
    medianMs: percentile(stats.errors, 50) * 1000,
    p90Ms: percentile(stats.errors, 90) * 1000,
    coveragePct: (stats.covered / Math.max(1, stats.total)) * 100,
    mcFalsePositiveLines: stats.mcFp,
    mcFpPerSection: stats.mcFp / Math.max(1, stats.mcSections),
    falseRepeats: stats.falseRepeats,
    falseRepeatsPerSample: stats.falseRepeats / Math.max(1, stats.samples),
    falseRepeatSections: stats.falseRepeatSections,
    falseRepeatSectionsPerSample: stats.falseRepeatSections / Math.max(1, stats.samples),
    falseRepeatSectionsMaxPerSample: Math.max(0, ...stats.falseRepeatSectionSamples),
  };
}

test.skipIf(!dataRoot)("synthetic gold metrics", () => {
  const measurement = new Measurement();
  const cache = studioCache(algorithm, measurement);
  const aggregate = emptyStats();
  const byTransform = new Map<string, Stats>();
  const medleyErrors: number[] = [];
  const medleyStartErrors: number[] = [];
  const medleyEndErrors: number[] = [];
  let medleyTotal = 0;
  let medleyMissed = 0;
  const samples = [];
  for (const id of manifest()) {
    const dir = path.join(root(), "align/synth_data");
    const gt = synthSchema.parse(readJson(path.join(dir, `${id}.gt.json`)));
    const wav = measurement.run(`${id}:wav`, () => algorithm.readWav(readFileSync(path.join(dir, `${id}.wav`))));
    const liveFeat = measurement.run(`${id}:features`, () =>
      new algorithm.AudioFeatures({ sampleRate: wav.sampleRate }).extract(wav.samples),
    );
    if (gt.is_medley) {
      const songs = [...cache].map(([song, base]) => ({ id: song, studioFeat: base.feat }));
      const detected = measurement.run(`${id}:medley`, () => algorithm.findMedleySongs(songs, liveFeat));
      for (const expected of gt.medley_songs || []) {
        medleyTotal++;
        const found = detected.find(song => song.id === expected.id);
        if (!found) medleyMissed++;
        const startError = found ? Math.abs(found.startSec - expected.startSec) : 999;
        const endError = found ? Math.abs(found.endSec - expected.endSec) : 999;
        medleyErrors.push(startError, endError);
        medleyStartErrors.push(startError);
        medleyEndErrors.push(endError);
      }
      samples.push({ id, detected });
      continue;
    }
    const base = requireValue(cache.get(gt.base_song || ""), `base song for ${id}`);
    const coarse = measurement.run(`${id}:coarse`, () => algorithm.estimateGlobalOffset(base.feat, liveFeat));
    const preds = measurement.run(`${id}:align`, () => algorithm.alignLinesGapDTW(base.lines, base.feat, liveFeat));
    const transStats = byTransform.get(gt.transform) || emptyStats();
    evaluate(preds, gt, aggregate);
    evaluate(preds, gt, transStats);
    byTransform.set(gt.transform, transStats);
    samples.push({
      id,
      coarseOffsetSec: coarse.offsetSec,
      coarseErrorSec: gt.global_offset === undefined ? null : Math.abs(coarse.offsetSec - gt.global_offset),
    });
  }
  const overall = summary(aggregate);
  const transforms = Object.fromEntries([...byTransform].map(([name, stats]) => [name, summary(stats)]));
  const medley = {
    totalSongs: medleyTotal,
    missedSongs: medleyMissed,
    missRatePct: (medleyMissed / Math.max(1, medleyTotal)) * 100,
    medianErrorSec: percentile(medleyErrors, 50),
    p90ErrorSec: percentile(medleyErrors, 90),
    medianStartErrorSec: percentile(medleyStartErrors, 50),
    p90StartErrorSec: percentile(medleyStartErrors, 90),
    medianEndErrorSec: percentile(medleyEndErrors, 50),
    p90EndErrorSec: percentile(medleyEndErrors, 90),
  };
  writeReport("synth", { overall, byTransform: transforms, medley, samples }, measurement);
  console.table([
    { transform: "all non-medley", ...overall },
    ...Object.entries(transforms).map(([transform, stats]) => ({ transform, ...stats })),
  ]);
  console.table([medley]);
  expect(overall.medianMs).toBeLessThanOrEqual(300);
  expect(overall.p90Ms).toBeLessThanOrEqual(700);
  expect(overall.coveragePct).toBeGreaterThanOrEqual(90);
  expect(overall.mcFalsePositiveLines).toBeLessThanOrEqual(1);
  expect(medley.missedSongs).toBe(0);
});
