import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "vitest";

import type { Lines } from "./common";

/** The prototype's per-line post-processing: onset snapping, no outlier smoothing. */
const ORIGINAL_LINE_OPTIONS = { onsetRefineSec: 0.25, outlierWindow: 0 };

import {
  algorithm,
  dataRoot,
  linesSchema,
  loadPair,
  manifest,
  Measurement,
  prototype,
  readJson,
  requireValue,
  root,
  silverIds,
  studioCache,
  synthSchema,
  writeReport,
} from "./common";

test.skipIf(!dataRoot)("prototype and TypeScript parity on every synth, silver and bench Live sample", async () => {
  const measurement = new Measurement();
  const original = await prototype();
  const tsCache = studioCache(algorithm, measurement);
  const jsCache = studioCache(original, measurement);
  const rows: { sample: string; lines: number; maxLineDifferenceMs: number; offsetDifferenceMs: number }[] = [];
  const medleys: { sample: string; maxDifferenceMs: number }[] = [];
  function compare(
    label: string,
    lines: Lines,
    tsStudio: ReturnType<typeof algorithm.AudioFeatures.prototype.extract>,
    tsLive: ReturnType<typeof algorithm.AudioFeatures.prototype.extract>,
    jsStudio: ReturnType<typeof algorithm.AudioFeatures.prototype.extract>,
    jsLive: ReturnType<typeof algorithm.AudioFeatures.prototype.extract>,
  ) {
    const tsOffset = measurement.run(`${label}:tsCoarse`, () => algorithm.estimateGlobalOffset(tsStudio, tsLive));
    const jsOffset = measurement.run(`${label}:jsCoarse`, () => original.estimateGlobalOffset(jsStudio, jsLive));
    const tsLines = measurement.run(`${label}:tsAlign`, () =>
      algorithm.alignLinesGapDTW(lines, tsStudio, tsLive, ORIGINAL_LINE_OPTIONS),
    );
    const jsLines = measurement.run(`${label}:jsAlign`, () => original.alignLinesGapDTW(lines, jsStudio, jsLive));
    expect(tsLines.map(line => line.idx)).toEqual(jsLines.map(line => line.idx));
    const maxLineDifferenceMs = Math.max(
      0,
      ...tsLines.map((line, index) => Math.abs(line.liveStartSec - jsLines[index].liveStartSec) * 1000),
    );
    rows.push({
      sample: label,
      lines: tsLines.length,
      maxLineDifferenceMs,
      offsetDifferenceMs: Math.abs(tsOffset.offsetSec - jsOffset.offsetSec) * 1000,
    });
  }
  for (const id of manifest()) {
    const dir = path.join(root(), "align/synth_data");
    const gt = synthSchema.parse(readJson(path.join(dir, `${id}.gt.json`)));
    const buffer = measurement.run(`${id}:load`, () => readFileSync(path.join(dir, `${id}.wav`)));
    const tsWav = measurement.run(`${id}:tsWav`, () => algorithm.readWav(buffer));
    const jsWav = measurement.run(`${id}:jsWav`, () => original.readWav(buffer));
    const tsLive = measurement.run(`${id}:tsFeatures`, () =>
      new algorithm.AudioFeatures({ sampleRate: tsWav.sampleRate }).extract(tsWav.samples),
    );
    const jsLive = measurement.run(`${id}:jsFeatures`, () =>
      new original.AudioFeatures({ sampleRate: jsWav.sampleRate }).extract(jsWav.samples),
    );
    if (gt.is_medley) {
      const tsFound = measurement.run(`${id}:tsMedley`, () =>
        algorithm.findMedleySongs(
          [...tsCache].map(([song, base]) => ({ id: song, studioFeat: base.feat })),
          tsLive,
        ),
      );
      const jsFound = measurement.run(`${id}:jsMedley`, () =>
        original.findMedleySongs(
          [...jsCache].map(([song, base]) => ({ id: song, studioFeat: base.feat })),
          jsLive,
        ),
      );
      expect(tsFound.map(song => song.id)).toEqual(jsFound.map(song => song.id));
      medleys.push({
        sample: id,
        maxDifferenceMs: Math.max(
          0,
          ...tsFound.flatMap((song, index) => [
            Math.abs(song.startSec - jsFound[index].startSec) * 1000,
            Math.abs(song.endSec - jsFound[index].endSec) * 1000,
          ]),
        ),
      });
      for (const song of gt.medley_songs || []) {
        const tsBase = requireValue(tsCache.get(song.id), song.id);
        const jsBase = requireValue(jsCache.get(song.id), song.id);
        compare(`${id}/${song.id}`, tsBase.lines, tsBase.feat, tsLive, jsBase.feat, jsLive);
      }
    } else {
      const tsBase = requireValue(tsCache.get(gt.base_song || ""), id);
      const jsBase = requireValue(jsCache.get(gt.base_song || ""), id);
      compare(id, tsBase.lines, tsBase.feat, tsLive, jsBase.feat, jsLive);
    }
  }
  const silver = silverIds();
  expect(silver).toHaveLength(5);
  for (const id of silver) {
    const dir = path.join(root(), "silver", id);
    const lines = linesSchema.parse(readJson(path.join(dir, "lines.json")));
    const ts = loadPair(path.join(dir, "studio.wav"), path.join(dir, "live.wav"), algorithm, measurement, `${id}:ts`);
    const js = loadPair(path.join(dir, "studio.wav"), path.join(dir, "live.wav"), original, measurement, `${id}:js`);
    compare(id, lines, ts.studioFeat, ts.liveFeat, js.studioFeat, js.liveFeat);
  }
  for (const [id, song] of [
    ["BV15t421K7Le", "nanimo"],
    ["BV19T421S7nv", "nanimo"],
    ["BV1QhieYjEY2", "sora"],
    ["BV1dy75z8Enu", "haze"],
  ]) {
    const tsBase = requireValue(tsCache.get(song), song);
    const jsBase = requireValue(jsCache.get(song), song);
    const file = path.join(root(), "data/audio", `${id}.wav`);
    const tsWav = measurement.run(`${id}:tsWav`, () => algorithm.readWav(readFileSync(file)));
    const jsWav = measurement.run(`${id}:jsWav`, () => original.readWav(readFileSync(file)));
    const tsFeat = measurement.run(`${id}:tsFeatures`, () =>
      new algorithm.AudioFeatures({ sampleRate: tsWav.sampleRate }).extract(tsWav.samples),
    );
    const jsFeat = measurement.run(`${id}:jsFeatures`, () =>
      new original.AudioFeatures({ sampleRate: jsWav.sampleRate }).extract(jsWav.samples),
    );
    compare(id, tsBase.lines, tsBase.feat, tsFeat, jsBase.feat, jsFeat);
  }
  const maxLineDifferenceMs = Math.max(0, ...rows.map(row => row.maxLineDifferenceMs));
  const maxOffsetDifferenceMs = Math.max(0, ...rows.map(row => row.offsetDifferenceMs));
  const maxMedleyDifferenceMs = Math.max(0, ...medleys.map(row => row.maxDifferenceMs));
  writeReport(
    "parity",
    {
      synthSamples: manifest().length,
      silverSamples: silver.length,
      benchLiveSamples: 4,
      maxLineDifferenceMs,
      maxOffsetDifferenceMs,
      maxMedleyDifferenceMs,
      rows,
      medleys,
    },
    measurement,
  );
  console.table(rows);
  console.table(medleys);
  console.table([{ maxLineDifferenceMs, maxOffsetDifferenceMs, maxMedleyDifferenceMs }]);
  expect(maxLineDifferenceMs).toBeLessThanOrEqual(10);
  expect(maxOffsetDifferenceMs).toBeLessThanOrEqual(10);
  expect(maxMedleyDifferenceMs).toBeLessThanOrEqual(10);
});
