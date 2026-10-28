import { describe, expect, it } from "vitest";

import { generateLrc, parseLrc } from "../../shared/auto-lyrics/lrc";

describe("LRC", () => {
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
