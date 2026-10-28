import { describe, expect, it } from "vitest";

import { parseWav } from "../../shared/auto-lyrics/pcm";

function wav(channels: number, values: number[], junk = false) {
  const extra = junk ? 10 : 0;
  const bytes = new Uint8Array(44 + extra + values.length * 2);
  const view = new DataView(bytes.buffer);
  const tag = (offset: number, value: string) => {
    for (let i = 0; i < value.length; i++) bytes[offset + i] = value.charCodeAt(i);
  };
  tag(0, "RIFF");
  view.setUint32(4, bytes.length - 8, true);
  tag(8, "WAVE");
  tag(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, channels, true);
  view.setUint32(24, 22050, true);
  view.setUint32(28, 22050 * channels * 2, true);
  view.setUint16(32, channels * 2, true);
  view.setUint16(34, 16, true);
  if (junk) {
    tag(36, "JUNK");
    view.setUint32(40, 1, true);
    bytes[44] = 123;
  }
  tag(36 + extra, "data");
  view.setUint32(40 + extra, values.length * 2, true);
  values.forEach((value, index) => view.setInt16(44 + extra + index * 2, value, true));
  return bytes;
}

describe("PCM WAV", () => {
  it("normalizes signed 16-bit samples and respects Uint8Array byte offsets", () => {
    const source = wav(1, [-32768, 0, 32767]);
    const padded = new Uint8Array(source.length + 12);
    padded.set(source, 7);
    const decoded = parseWav(padded.subarray(7, 7 + source.length));
    expect([...decoded.samples]).toEqual([-1, 0, 32767 / 32768]);
    expect(decoded.sampleRate).toBe(22050);
    expect(decoded.channels).toBe(1);
    expect(decoded.duration).toBe(3 / 22050);
  });

  it("downmixes stereo and skips padded unknown chunks", () => {
    expect([...parseWav(wav(2, [-32768, 32767, 16384, 0], true)).samples]).toEqual([-1 / 65536, 0.25]);
  });

  it("rejects malformed headers, unsupported encodings, and truncated data", () => {
    expect(() => parseWav(new Uint8Array(3))).toThrow("Invalid WAV header");
    const source = wav(1, [1, 2]);
    expect(() => parseWav(source.subarray(0, source.length - 1))).toThrow("Truncated WAV chunk");
    new DataView(source.buffer).setUint16(20, 3, true);
    expect(() => parseWav(source)).toThrow("Unsupported WAV format");
    expect(() => parseWav(wav(2, [1]))).toThrow("Incomplete WAV sample frame");
  });
});
