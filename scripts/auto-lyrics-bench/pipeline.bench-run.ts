/**
 * AutoLyricsService end to end with real Netease lyrics and real alignment. Bilibili download and ffmpeg are
 * replaced by local WAV files (Live: data/audio, references: data/studio). Netease search is geo-filtered outside
 * CN, so by default it replays the song catalog recorded on a CN machine (identify_eval_results-1.16.2.json);
 * AUTO_LYRICS_NETEASE=live uses the real search instead.
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, vi } from "vitest";
import { z } from "zod";

import type { SongCandidate } from "../../shared/auto-lyrics/identify";
import type { AutoLyricsResult } from "../../shared/auto-lyrics/service-types";

import { AutoLyricsService } from "../../electron/auto-lyrics/service";
import { sources } from "../../electron/auto-lyrics/sources";
import { generateLrc } from "../../shared/auto-lyrics";
import { normalize } from "../../shared/auto-lyrics/identify";
import { algorithm, dataRoot, Measurement, root, writeReport } from "./common";

vi.mock("../../electron/network/cookie", () => ({ getCookieString: async () => "" }));
vi.mock("../../electron/ipc/api/wbi", () => ({ encodeParamsWbi: async () => ({}) }));

const STUDIO: Record<string, string> = { 名もなき何もかも: "nanimo", 空の箱: "sora", 爆ぜて咲く: "haze" };

const recordedSchema = z.array(
  z.object({
    modes: z.record(
      z.string(),
      z.object({
        searches: z
          .array(
            z.object({
              query: z.string(),
              songs: z.array(
                z.object({
                  id: z.number(),
                  title: z.string(),
                  artists: z.array(z.string()),
                  duration: z.number(),
                  aliases: z.array(z.string()).default([]),
                }),
              ),
            }),
          )
          .default([]),
      }),
    ),
  }),
);

/**
 * A conservative search engine over recorded Netease metadata: the recorded result for the same query, else songs
 * whose name (without a bracket suffix) or artist equals the query. Fuzzy full-title hits are not simulated.
 */
function recordedSearch(file: string) {
  const catalog = new Map<number, SongCandidate>();
  const recorded = new Map<string, SongCandidate[]>();
  for (const entry of recordedSchema.parse(JSON.parse(readFileSync(file, "utf8"))))
    for (const mode of Object.values(entry.modes))
      for (const search of mode.searches) {
        const songs = search.songs.map(song => ({
          id: song.id,
          name: song.title,
          artists: song.artists,
          duration: song.duration / 1000,
          aliases: song.aliases,
        }));
        for (const song of songs) catalog.set(song.id, song);
        if (songs.length) recorded.set(normalize(search.query), songs);
      }
  return async (query: string): Promise<SongCandidate[]> => {
    const q = normalize(query);
    return (
      recorded.get(q) ??
      [...catalog.values()]
        .filter(
          song =>
            normalize(song.name.replace(/\s*[(（【].*$/, "")) === q ||
            song.artists.some(artist => normalize(artist) === q),
        )
        .slice(0, 20)
    );
  };
}

test.skipIf(!dataRoot)("pipeline: identify -> netease lyric -> reference -> coarse/fine LRC", async () => {
  const data = path.join(root(), "data");
  const cases = readFileSync(path.join(data, "lyrics/meta.txt"), "utf8")
    .trim()
    .split("\n")
    .map(line => {
      const [bvid, title, duration, ownerName] = line.split("|");
      return { bvid, cid: 1, title, duration: Number(duration), tid: 3, ownerName };
    });
  const wav = (file: string) => new Uint8Array(readFileSync(file));
  const extractor = () => new algorithm.AudioFeatures({ sampleRate: 22050 });
  const service = new AutoLyricsService({
    sources: {
      search:
        process.env.AUTO_LYRICS_NETEASE === "live"
          ? sources.search
          : recordedSearch(path.join(data, "identify_eval_results-1.16.2.json")),
      lyric: sources.lyric,
      async references(song) {
        const name = Object.entries(STUDIO).find(([title]) => song.name.startsWith(title))?.[1];
        if (!name) return [];
        const { duration } = algorithm.readWav(wav(path.join(data, `studio/${name}.wav`)));
        return [{ bvid: `studio:${name}`, title: `${song.name} 官方音源`, duration }];
      },
      async audio(bvid) {
        return bvid.startsWith("studio:")
          ? wav(path.join(data, `studio/${bvid.slice(7)}.wav`))
          : wav(path.join(data, `audio/${bvid}.wav`));
      },
    },
    decode: async bytes => algorithm.readWav(bytes).samples,
    async align({ studio, live, lines }, _signal, onCoarse) {
      const s = extractor().extract(studio);
      const l = extractor().extract(live);
      const coarse = algorithm.estimateGlobalOffset(s, l);
      onCoarse(coarse);
      return { coarse, lines: algorithm.alignLinesGapDTW(lines, s, l) };
    },
    cache: { get: () => undefined, set: () => {} },
    references: { get: () => undefined, set: () => {} },
  });
  const coarseAt = new Map<string, number>();
  service.onProgress(event => {
    if (event.phase === "coarse") coarseAt.set(event.key, event.timings.coarse ?? Number.NaN);
  });

  const measurement = new Measurement();
  const results: (AutoLyricsResult & { coarseEventMs?: number })[] = [];
  for (const params of cases) {
    const result = await service.start(params);
    results.push({ ...result, coarseEventMs: coarseAt.get(result.key) });
  }
  const summary = results.map(r => ({
    key: r.key,
    status: r.status,
    song: r.song ? `${r.song.name} / ${r.song.artists.join(",")}` : undefined,
    reference: r.reference?.bvid,
    offsetSec: r.coarse?.offsetSec,
    confidence: r.confidence,
    lrcLines: r.lyrics?.lyrics?.split("\n").filter(Boolean).length,
    coarseEventMs: r.coarseEventMs && Math.round(r.coarseEventMs),
    doneMs: Math.round(r.timings.done ?? 0),
    error: r.error,
  }));
  console.table(summary);
  for (const r of results.filter(r => r.lines)) {
    console.log(`== ${r.key}\n${generateLrc(r.lines!.slice(0, 6))}`);
  }
  writeReport("pipeline", { summary, lyrics: results.map(r => ({ key: r.key, lrc: r.lyrics?.lyrics })) }, measurement);
  expect(results.filter(r => r.status === "matched").length).toBeGreaterThanOrEqual(3);
});
