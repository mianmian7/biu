import { describe, expect, it } from "vitest";

import { generateLrc, mapCompanionLrc, parseLrc } from "../../shared/auto-lyrics/lrc";

describe("LRC", () => {
  it("maps translation and romaji by studio timestamp, including repeats and skipped rows", () => {
    const aligned = [
      { idx: 0, text: "original", studioStart: 1.234, liveStartSec: 3, confidence: 1, uncertain: false },
      { idx: 0, text: "original", studioStart: 1.234, liveStartSec: 7, confidence: 1, uncertain: false },
      { idx: 2, text: "last", studioStart: 4, liveStartSec: 9, confidence: 1, uncertain: false },
    ];
    expect(mapCompanionLrc("[00:01.23]译文\n[00:02]已跳过\n[00:04]结尾\n[00:05]无对应", aligned)).toBe(
      "[00:03.00]译文\n[00:07.00]译文\n[00:09.00]结尾",
    );
    expect(mapCompanionLrc("[00:01.234]romaji\n[00:04]owari", aligned)).toBe(
      "[00:03.00]romaji\n[00:07.00]romaji\n[00:09.00]owari",
    );
    expect(mapCompanionLrc("", aligned)).toBe("");
  });

  it("expands multiple timestamps, sorts lyrics, and filters metadata and blank rows", () => {
    expect(parseLrc("[ar:Artist]\n[00:03.125][00:01.20] chorus \r\n[00:02.00] \n[01:02]end\n[00:61]bad")).toEqual([
      { idx: 0, studio_start: 1.2, text: "chorus" },
      { idx: 1, studio_start: 3.125, text: "chorus" },
      { idx: 2, studio_start: 62, text: "end" },
    ]);
  });

  it("round-trips aligned text and timestamps to centisecond precision", () => {
    const lines = parseLrc("[00:01.23]first [chorus]\n[00:59.999]second\n[02:00.10]third");
    const aligned = lines.map(line => ({
      idx: line.idx,
      text: line.text,
      studioStart: line.studio_start,
      liveStartSec: line.studio_start,
      confidence: 1,
      uncertain: false,
    }));
    const output = generateLrc([...aligned].reverse());
    expect(output).toBe("[00:01.23]first [chorus]\n[01:00.00]second\n[02:00.10]third");
    const parsed = parseLrc(output);
    for (let i = 0; i < lines.length; i++) {
      expect(parsed[i].text).toBe(lines[i].text);
      expect(Math.abs(parsed[i].studio_start - lines[i].studio_start)).toBeLessThanOrEqual(0.005);
    }
    expect(generateLrc(aligned)).toBe(output);
  });
});
