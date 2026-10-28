import { describe, expect, it } from "vitest";

import { alignLinesGapDTW, estimateGlobalOffset } from "../../shared/auto-lyrics/align";
import { AudioFeatures } from "../../shared/auto-lyrics/features";
import { FFT } from "../../shared/auto-lyrics/fft";

const sampleRate = 22050;
const hopSize = 512;

function melody() {
  const pitches = [48, 55, 62, 51, 58, 65, 53, 60, 67, 50, 57, 64, 52, 59, 66, 49, 56, 63];
  const samples = new Float32Array(sampleRate * 9);
  for (let i = 0; i < samples.length; i++) {
    const segment = Math.floor(i / (sampleRate / 2));
    const t = i / sampleRate;
    const midi = pitches[segment];
    const envelope = Math.min(1, (t % 0.5) * 40, (0.5 - (t % 0.5)) * 40);
    for (const interval of [0, 4, 7]) {
      const frequency = 440 * 2 ** ((midi + interval - 69) / 12);
      samples[i] += (0.25 / 3) * envelope * Math.sin(2 * Math.PI * frequency * t);
    }
  }
  return samples;
}

describe("FFT", () => {
  it("matches a direct complex DFT including phase", () => {
    const size = 16;
    const real = Float32Array.from({ length: size }, (_, i) => Math.sin(i * 0.7) + i / 19);
    const imaginary = Float32Array.from({ length: size }, (_, i) => Math.cos(i * 0.3) / 3);
    const sourceReal = [...real];
    const sourceImaginary = [...imaginary];
    new FFT(size).transform(real, imaginary);
    for (let k = 0; k < size; k++) {
      let expectedReal = 0;
      let expectedImaginary = 0;
      for (let n = 0; n < size; n++) {
        const phase = (-2 * Math.PI * k * n) / size;
        expectedReal += sourceReal[n] * Math.cos(phase) - sourceImaginary[n] * Math.sin(phase);
        expectedImaginary += sourceReal[n] * Math.sin(phase) + sourceImaginary[n] * Math.cos(phase);
      }
      expect(real[k]).toBeCloseTo(expectedReal, 5);
      expect(imaginary[k]).toBeCloseTo(expectedImaginary, 5);
    }
  });

  it("rejects a non-power-of-two transform size", () => {
    expect(() => new FFT(15)).toThrow("power of 2");
  });
});

describe("audio alignment", () => {
  const studioSamples = melody();
  const extractor = new AudioFeatures({ sampleRate, hopSize });
  const studio = extractor.extract(studioSamples);

  it("produces unit L2 CENS vectors and zero vectors for silence", () => {
    expect(studio.numFrames).toBe(Math.floor((studioSamples.length - 2048) / hopSize) + 1);
    expect(studio.fps).toBe(sampleRate / hopSize);
    for (const frame of studio.cens) {
      const norm = Math.sqrt(frame.reduce((sum, value) => sum + value * value, 0));
      expect(norm).toBeCloseTo(1, 5);
    }
    const silence = extractor.extract(new Float32Array(sampleRate));
    expect(silence.cens.every(frame => frame.every(value => value === 0))).toBe(true);
    expect(silence.onset.every(value => value === 0)).toBe(true);
    expect(studio.onset.some(value => value > 0)).toBe(true);
  });

  it("recovers a known leading offset within one feature frame", () => {
    const shift = 43 * hopSize;
    const liveSamples = new Float32Array(studioSamples.length + shift);
    liveSamples.set(studioSamples, shift);
    const live = extractor.extract(liveSamples);
    const result = estimateGlobalOffset(studio, live);
    expect(Math.abs(result.offsetSec - shift / sampleRate)).toBeLessThanOrEqual(1 / studio.fps);
    expect(result.confidence).toBeGreaterThan(0.9);
  });

  it("keeps line starts monotonic and near the inserted-silence timeline", () => {
    const leading = hopSize * 43;
    const cut = sampleRate * 4;
    const gap = sampleRate;
    const liveSamples = new Float32Array(studioSamples.length + leading + gap);
    liveSamples.set(studioSamples.subarray(0, cut), leading);
    liveSamples.set(studioSamples.subarray(cut), leading + cut + gap);
    const live = extractor.extract(liveSamples);
    const starts = [1, 2.5, 5, 6.5, 8];
    const lines = starts.map((studio_start, idx) => ({ idx, studio_start, text: `line ${idx}` }));
    const result = alignLinesGapDTW(lines, studio, live);
    expect(result).toHaveLength(lines.length);
    for (let i = 0; i < result.length; i++) {
      const line = result[i];
      const expected = starts[i] + leading / sampleRate + (starts[i] >= 4 ? 1 : 0);
      expect(line.liveStartSec).toBeGreaterThanOrEqual(i === 0 ? 0 : result[i - 1].liveStartSec);
      expect(line.liveStartSec).toBeLessThan(live.duration);
      expect(Math.abs(line.liveStartSec - expected)).toBeLessThan(0.5);
      expect(line.confidence).toBeGreaterThanOrEqual(0);
      expect(line.confidence).toBeLessThanOrEqual(1);
    }
  });
});
