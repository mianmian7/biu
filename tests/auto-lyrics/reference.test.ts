import { describe, expect, it } from "vitest";

import { checkVocalOnset, rankReferences } from "../../shared/auto-lyrics/reference";

describe("studio reference selection", () => {
  it("enforces a two-second duration gate and rewards original audio evidence", () => {
    const candidates = [
      { bvid: "original", title: "原曲 CD 歌词 罗马音", duration: 190 },
      { bvid: "neutral", title: "Title", duration: 189 },
      { bvid: "live", title: "Title Live 现场", duration: 189 },
      { bvid: "cover", title: "翻唱 cover 教学 reaction 伴奏", duration: 189 },
      { bvid: "long", title: "原曲官方MV", duration: 192 },
      { bvid: "border", title: "扒谱 假名", duration: 191 },
    ];
    expect(rankReferences(candidates, 189).map(c => c.bvid)).toEqual([
      "original",
      "neutral",
      "border",
      "live",
      "cover",
    ]);
    expect(rankReferences(candidates, NaN)).toEqual([]);
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
