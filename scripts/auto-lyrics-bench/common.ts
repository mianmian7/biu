import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { z } from "zod";

import {
  alignLinesGapDTW,
  AudioFeatures,
  estimateGlobalOffset,
  findMedleySongs,
  parseWav,
} from "../../shared/auto-lyrics";

export const dataRoot = process.env.AUTO_LYRICS_BENCH_DATA;
const lineSchema = z.object({ idx: z.number(), studio_start: z.number(), text: z.string() });
export const linesSchema = z.array(lineSchema);
export const manifestSchema = z.array(z.string());
export const synthSchema = z.object({
  id: z.string(),
  transform: z.string(),
  base_song: z.string().optional(),
  is_medley: z.boolean().optional(),
  global_offset: z.number().optional(),
  mc_region: z.tuple([z.number(), z.number()]).optional(),
  lines: z.array(z.object({ idx: z.number(), true_start: z.number().nullable(), status: z.string() })).optional(),
  medley_songs: z.array(z.object({ id: z.string(), startSec: z.number(), endSec: z.number() })).optional(),
});
export const silverGtSchema = z.object({ gt: z.array(z.object({ idx: z.number(), live_start: z.number() })) });
export type SilverEntry = z.infer<typeof silverGtSchema>["gt"][number];
export type Lines = z.infer<typeof linesSchema>;
export type Prediction = ReturnType<typeof alignLinesGapDTW>[number] & { repeatGroup?: string };
export type SynthGt = z.infer<typeof synthSchema>;
export const algorithm = { AudioFeatures, estimateGlobalOffset, alignLinesGapDTW, findMedleySongs, readWav: parseWav };
export type Algorithm = typeof algorithm;

export function readJson(filename: string): unknown {
  return JSON.parse(readFileSync(filename, "utf8"));
}
export function root(): string {
  if (!dataRoot) throw new Error("AUTO_LYRICS_BENCH_DATA is required");
  return path.resolve(dataRoot);
}
export function silverIds(): string[] {
  return readdirSync(path.join(root(), "silver"), { withFileTypes: true })
    .filter(entry => entry.isDirectory() && entry.name.startsWith("BV"))
    .map(entry => entry.name)
    .sort();
}
export function manifest(): string[] {
  return manifestSchema.parse(readJson(path.join(root(), "align/synth_data/manifest.json")));
}
export async function prototype(): Promise<Algorithm> {
  const dir = path.join(root(), "align/src.v1");
  const [features, dtw, wav]: unknown[] = await Promise.all([
    import(/* @vite-ignore */ pathToFileURL(path.join(dir, "features.js")).href),
    import(/* @vite-ignore */ pathToFileURL(path.join(dir, "dtw.js")).href),
    import(/* @vite-ignore */ pathToFileURL(path.join(dir, "wav.js")).href),
  ]);
  const functionSchema = <T>() => z.custom<T>(value => typeof value === "function");
  const featureModule = z.object({ AudioFeatures: functionSchema<Algorithm["AudioFeatures"]>() }).parse(features);
  const dtwModule = z
    .object({
      estimateGlobalOffset: functionSchema<Algorithm["estimateGlobalOffset"]>(),
      alignLinesGapDTW: functionSchema<Algorithm["alignLinesGapDTW"]>(),
      findMedleySongs: functionSchema<Algorithm["findMedleySongs"]>(),
    })
    .parse(dtw);
  const wavModule = z.object({ readWav: functionSchema<Algorithm["readWav"]>() }).parse(wav);
  return { ...featureModule, ...dtwModule, ...wavModule };
}
export class Measurement {
  stages: { stage: string; ms: number; rssBytes: number }[] = [];
  run<T>(stage: string, fn: () => T): T {
    const start = performance.now();
    const result = fn();
    this.stages.push({ stage, ms: performance.now() - start, rssBytes: process.memoryUsage().rss });
    return result;
  }
  report() {
    return {
      stages: this.stages,
      rssPeakBytes: Math.max(process.memoryUsage().rss, ...this.stages.map(stage => stage.rssBytes)),
      processHighWaterRssBytes: process.resourceUsage().maxRSS * 1024,
      rssSampling:
        "process.memoryUsage().rss after each synchronous stage; processHighWaterRssBytes also records OS process lifetime high-water mark",
    };
  }
}
export function writeReport(name: string, result: unknown, measurement: Measurement) {
  const out = process.env.AUTO_LYRICS_BENCH_OUT || "/tmp/auto-lyrics-bench/";
  mkdirSync(out, { recursive: true });
  const report = { ...measurement.report(), result };
  writeFileSync(path.join(out, `${name}.json`), JSON.stringify(report, null, 2) + "\n");
  console.table([
    {
      report: name,
      rssPeakMiB: report.rssPeakBytes / 1024 ** 2,
      processHighWaterMiB: report.processHighWaterRssBytes / 1024 ** 2,
    },
  ]);
  const timings = new Map<string, number[]>();
  for (const stage of report.stages) {
    const kind = stage.stage.split(":").at(-1) || stage.stage;
    const values = timings.get(kind) || [];
    values.push(stage.ms);
    timings.set(kind, values);
  }
  console.table(
    [...timings].map(([stage, values]) => ({
      stage,
      count: values.length,
      totalMs: values.reduce((sum, value) => sum + value, 0),
      medianMs: quantile(values, 0.5),
      maxMs: Math.max(...values),
    })),
  );
  return report;
}
export function loadPair(
  studioFile: string,
  liveFile: string,
  impl: Algorithm,
  measurement: Measurement,
  label: string,
) {
  const [studio, live] = measurement.run(`${label}:wav`, () => [
    impl.readWav(readFileSync(studioFile)),
    impl.readWav(readFileSync(liveFile)),
  ]);
  const studioFeat = measurement.run(`${label}:studioFeatures`, () =>
    new impl.AudioFeatures({ sampleRate: studio.sampleRate }).extract(studio.samples),
  );
  const liveFeat = measurement.run(`${label}:liveFeatures`, () =>
    new impl.AudioFeatures({ sampleRate: live.sampleRate }).extract(live.samples),
  );
  return { studioFeat, liveFeat };
}
export function studioCache(impl: Algorithm, measurement: Measurement) {
  return new Map(
    ["nanimo", "sora", "haze"].map(song => {
      const wav = measurement.run(`${song}:wav`, () =>
        impl.readWav(readFileSync(path.join(root(), "data/studio", `${song}.wav`))),
      );
      const feat = measurement.run(`${song}:features`, () =>
        new impl.AudioFeatures({ sampleRate: wav.sampleRate }).extract(wav.samples),
      );
      const lines = linesSchema.parse(readJson(path.join(root(), "data/lyrics", `${song}.lines.json`)));
      return [song, { feat, lines }];
    }),
  );
}
export function requireValue<T>(value: T | undefined, label: string): T {
  if (value === undefined) throw new Error(`Missing ${label}`);
  return value;
}
export function percentile(values: number[], percent: number) {
  if (!values.length) return 0;
  return [...values].sort((a, b) => a - b)[Math.floor((percent / 100) * (values.length - 1))];
}
export function quantile(values: number[], q: number): number | null {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const pos = (sorted.length - 1) * q;
  const base = Math.floor(pos);
  return sorted[base + 1] === undefined
    ? sorted[base]
    : sorted[base] + (pos - base) * (sorted[base + 1] - sorted[base]);
}
