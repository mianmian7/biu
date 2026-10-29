import { describe, expect, it } from "vitest";

import type { SongCandidate } from "../../shared/auto-lyrics/identify";

import { checkVocalOnset, rankReferences } from "../../shared/auto-lyrics/reference";

const song: SongCandidate = { id: 1, name: "Test Song", artists: [], duration: 190 };

describe("studio reference selection", () => {
  it.each(["【MV】Test Song", "Test Song Project DIVA Arcade", "Test Song 官方MV"])(
    "ranks %s below studio audio",
    title => {
      const studio = {
        bvid: "z-studio",
        title: title.includes("官方") ? "Test Song 官方音源" : "Test Song CD音源",
        duration: 190,
      };
      const video = { bvid: "a-video", title, duration: 190 };
      const ranked = rankReferences([video, studio], song);
      expect(ranked.map(candidate => candidate.bvid)).toEqual(["z-studio", "a-video"]);
      expect(ranked[1].score).toBe(title.includes("官方") ? 55 : 30);
    },
  );

  it("ranks an AI cover below studio audio without matching AI inside words", () => {
    const candidates = [
      { bvid: "ai", title: "【AI高松灯】Test Song", duration: 190 },
      { bvid: "main", title: "Test Song MAIN THEME", duration: 190 },
    ];
    expect(rankReferences(candidates, song).map(c => [c.bvid, c.score])).toEqual([
      ["main", 60],
      ["ai", -20],
    ]);
  });

  it("enforces a two-second duration gate and rewards original audio evidence", () => {
    const candidates = [
      { bvid: "original", title: "Test Song 原曲 CD 歌词 罗马音", duration: 190 },
      { bvid: "neutral", title: "Test Song", duration: 189 },
      { bvid: "live", title: "Test Song Live 现场", duration: 189 },
      { bvid: "cover", title: "Test Song 翻唱 cover 教学 reaction 伴奏", duration: 189 },
      { bvid: "long", title: "Test Song 原曲官方MV", duration: 192 },
      { bvid: "border", title: "Test Song 扒谱 假名", duration: 191 },
    ];
    expect(rankReferences(candidates, { ...song, duration: 189 }).map(c => c.bvid)).toEqual([
      "original",
      "neutral",
      "border",
      "live",
      "cover",
    ]);
    expect(rankReferences(candidates, { ...song, duration: NaN })).toEqual([]);
  });

  it("requires the song title or an alias even when durations match", () => {
    const song: SongCandidate = {
      name: "空の箱 (井芹仁菜、河原木桃香)",
      artists: ["トゲナシトゲアリ"],
      aliases: ["空之箱"],
      duration: 200,
      id: 1,
    };
    const candidates = [
      { bvid: "wrong", title: "【4KHi-res】无声之鱼（声なき魚）《少女乐队的呐喊》第三集插入曲完整版", duration: 200 },
      { bvid: "original", title: "空の箱 / トゲナシトゲアリ 官方音源", duration: 200 },
      { bvid: "alias", title: "【中字】空之箱 CD", duration: 200 },
    ];
    expect(rankReferences(candidates, song).map(c => c.bvid)).toEqual(["alias", "original"]);
  });

  it("requires an audible energy rise within 1.5 seconds of the lyric", () => {
    const rate = 1000;
    const pcm = new Float32Array(rate * 8);
    for (let i = 3 * rate; i < pcm.length; i++) pcm[i] = Math.sin(i * 0.3) * 0.4;
    expect(checkVocalOnset(pcm, rate, 3)).toBe(true);
    expect(checkVocalOnset(pcm, rate, 1.6)).toBe(true);
    expect(checkVocalOnset(pcm, rate, 6)).toBe(false);
    expect(checkVocalOnset(new Float32Array(rate * 8), rate, 3)).toBe(false);
    expect(
      checkVocalOnset(
        pcm.map(v => v * 0.001),
        rate,
        3,
      ),
    ).toBe(false);
    expect(checkVocalOnset(pcm, 0, 3)).toBe(false);
    expect(checkVocalOnset(pcm, rate, 20)).toBe(false);
  });

  it("only requires an audible reference for a cold open", () => {
    const rate = 1000;
    // Quiet upload of 声なき魚: energy ≈ 0.002, below the onset deltas.
    const cold = Float32Array.from({ length: rate * 6 }, (_, i) => Math.sin(i * 0.3) * 0.06);
    expect(checkVocalOnset(cold, rate, 0.42)).toBe(true);
    expect(checkVocalOnset(new Float32Array(rate * 6), rate, 0.42)).toBe(false);
  });

  it("does not mistake constant energy or a gradual fade for an onset", () => {
    expect(checkVocalOnset(new Float32Array(5000).fill(0.5), 1000, 2)).toBe(false);
    const fade = Float32Array.from({ length: 5000 }, (_, i) => 0.1 + i / 100000);
    expect(checkVocalOnset(fade, 1000, 2)).toBe(false);
  });

  it("ignores a small flicker on a flat bed but keeps a half-second entrance", () => {
    const rate = 1000;
    const flat = new Float32Array(rate * 8);
    for (let i = 0; i < flat.length; i++) flat[i] = Math.sin(i * 0.7) * 0.08;
    for (let i = 3 * rate; i < 3.1 * rate; i++) flat[i] = Math.sin(i * 0.7) * 0.16;
    expect(checkVocalOnset(flat, rate, 3)).toBe(false);
    const entrance = new Float32Array(rate * 6);
    for (let i = 2 * rate; i < entrance.length; i++) {
      const gain = Math.min(1, (i - 2 * rate) / (rate * 0.5));
      entrance[i] = Math.sin(i * 0.5) * 0.2 * gain;
    }
    expect(checkVocalOnset(entrance, rate, 2)).toBe(true);
  });
});
