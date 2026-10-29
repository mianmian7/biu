/**
 * Wrong-song rejection: align every Live against every studio reference. Same-song pairs must score clearly above
 * different-song pairs, otherwise alignment cannot arbitrate between ambiguous identification candidates.
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "vitest";

import { verifyScore } from "../../shared/auto-lyrics/align";
import { algorithm, dataRoot, linesSchema, Measurement, readJson, root, writeReport } from "./common";

const LIVE: Record<string, string> = {
  BV19T421S7nv: "nanimo",
  BV15t421K7Le: "nanimo",
  BV1QhieYjEY2: "sora",
  BV1dy75z8Enu: "haze",
};

test.skipIf(!dataRoot)("cross: same-song alignment scores above different-song alignment", () => {
  const data = path.join(root(), "data");
  const wav = (file: string) => algorithm.readWav(readFileSync(file));
  const extract = ({ samples, sampleRate }: { samples: Float32Array; sampleRate: number }) =>
    new algorithm.AudioFeatures({ sampleRate }).extract(samples);
  const features = (file: string) => extract(wav(file));
  const measurement = new Measurement();
  const studios = Object.fromEntries(
    ["nanimo", "sora", "haze"].map(song => [
      song,
      {
        feat: measurement.run(`${song}:studio`, () => features(path.join(data, `studio/${song}.wav`))),
        lines: linesSchema.parse(readJson(path.join(data, `lyrics/${song}.lines.json`))),
      },
    ]),
  );
  // "tv": the Live cut to 45 % of the studio length from where the song starts, like a TV-size edit.
  const pairs = Object.entries(LIVE).flatMap(([bvid, truth]) => {
    const audio = wav(path.join(data, `audio/${bvid}.wav`));
    const full = measurement.run(`${bvid}:live`, () => extract(audio));
    const offset = Math.max(0, algorithm.estimateGlobalOffset(studios[truth].feat, full).offsetSec);
    const tvSec = studios[truth].feat.duration * 0.45;
    const begin = Math.round(offset * audio.sampleRate);
    const tv = measurement.run(`${bvid}:tv`, () =>
      extract({ ...audio, samples: audio.samples.slice(begin, begin + Math.round(tvSec * audio.sampleRate)) }),
    );
    return (
      [
        ["full", full],
        ["tv", tv],
      ] as const
    ).flatMap(([cut, live]) =>
      Object.entries(studios).map(([song, studio]) => {
        const coarse = algorithm.estimateGlobalOffset(studio.feat, live);
        const lines = algorithm.alignLinesGapDTW(studio.lines, studio.feat, live);
        const score = verifyScore(lines);
        return { bvid, cut, song, same: song === truth, coarse: coarse.confidence, score };
      }),
    );
  });
  writeReport("cross", pairs, measurement);
  console.table(pairs);
  // TV-size rows are reported only: GapDTW cannot skip the cut sections, so they do not separate.
  const worstSame = Math.min(...pairs.filter(p => p.cut === "full" && p.same).map(p => p.score));
  const bestWrong = Math.max(...pairs.filter(p => p.cut === "full" && !p.same).map(p => p.score));
  expect(worstSame).toBeGreaterThan(bestWrong);
});
