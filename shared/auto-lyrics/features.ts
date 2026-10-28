import type { AudioFeatureOptions, AudioFeatureSet } from "./types";

import { FFT } from "./fft";

/**
 * Audio feature extractor:
 * STFT -> Pitch-class profile -> Müller CENS (Chroma Energy Normalized Statistics)
 * plus Onset Spectral Flux envelope.
 */
export class AudioFeatures {
  sampleRate: number;
  nFft: number;
  hopSize: number;
  fft: FFT;
  minFreq: number;
  maxFreq: number;
  binWeights: { k: number; pc0: number; w0: number; pc1: number; w1: number }[];
  real: Float32Array;
  imag: Float32Array;

  constructor(options: AudioFeatureOptions = {}) {
    this.sampleRate = options.sampleRate || 22050;
    this.nFft = options.nFft || 2048;
    this.hopSize = options.hopSize || 512;
    this.fft = new FFT(this.nFft);

    // C2 (~65.4 Hz) to C7 (~2093 Hz)
    this.minFreq = options.minFreq || 65.0;
    this.maxFreq = options.maxFreq || 3500.0;

    // Pitch mapping with soft Gaussian spread over pitch classes
    const halfFft = this.nFft / 2;
    this.binWeights = [];
    const df = this.sampleRate / this.nFft;

    for (let k = 1; k < halfFft; k++) {
      const freq = k * df;
      if (freq >= this.minFreq && freq <= this.maxFreq) {
        const midi = 12 * Math.log2(freq / 440.0) + 69;
        const pcExact = ((midi % 12) + 12) % 12;
        const pc0 = Math.floor(pcExact);
        const pc1 = (pc0 + 1) % 12;
        const frac = pcExact - pc0;
        this.binWeights.push({
          k,
          pc0,
          w0: 1.0 - frac,
          pc1,
          w1: frac,
        });
      }
    }

    this.real = new Float32Array(this.nFft);
    this.imag = new Float32Array(this.nFft);
  }

  extract(samples: Float32Array): AudioFeatureSet {
    const nFft = this.nFft;
    const hopSize = this.hopSize;
    const numFrames = Math.max(0, Math.floor((samples.length - nFft) / hopSize) + 1);
    const halfFft = nFft / 2;

    const rawChroma = new Array<Float32Array>(numFrames);
    const prevMag = new Float32Array(halfFft);
    const currMag = new Float32Array(halfFft);
    const onset = new Float32Array(numFrames);

    const real = this.real;
    const imag = this.imag;
    const hann = this.fft.hann;
    const binWeights = this.binWeights;
    const numWeights = binWeights.length;

    for (let f = 0; f < numFrames; f++) {
      const start = f * hopSize;

      for (let i = 0; i < nFft; i++) {
        real[i] = samples[start + i] * hann[i];
        imag[i] = 0.0;
      }

      this.fft.transform(real, imag);

      const frameChroma = new Float32Array(12);
      let frameOnset = 0.0;

      for (let k = 1; k < halfFft; k++) {
        const r = real[k];
        const im = imag[k];
        const mag = Math.sqrt(r * r + im * im);
        currMag[k] = mag;

        const diff = mag - prevMag[k];
        if (diff > 0) frameOnset += diff;
      }

      for (let w = 0; w < numWeights; w++) {
        const bw = binWeights[w];
        const mag = currMag[bw.k];
        const energy = mag * mag;
        frameChroma[bw.pc0] += energy * bw.w0;
        frameChroma[bw.pc1] += energy * bw.w1;
      }

      onset[f] = frameOnset;
      prevMag.set(currMag);

      // Müller CENS step 1: L1 normalize
      let sum = 0;
      for (let c = 0; c < 12; c++) sum += frameChroma[c];

      // Müller CENS step 2: 4-tier quantization
      const q = new Float32Array(12);
      if (sum > 1e-6) {
        const invSum = 1.0 / sum;
        for (let c = 0; c < 12; c++) {
          const p = frameChroma[c] * invSum;
          if (p >= 0.4) q[c] = 4;
          else if (p >= 0.2) q[c] = 3;
          else if (p >= 0.1) q[c] = 2;
          else if (p >= 0.05) q[c] = 1;
          else q[c] = 0;
        }
      }
      rawChroma[f] = q;
    }

    // Müller CENS step 3: Temporal smoothing (Hann window of 41 frames ~ 0.95s)
    const smoothWin = 41;
    const halfWin = Math.floor(smoothWin / 2);
    const cens = new Array<Float32Array>(numFrames);

    const weights = new Float32Array(smoothWin);
    let weightSum = 0;
    for (let w = 0; w < smoothWin; w++) {
      const v = 0.5 * (1 - Math.cos((2 * Math.PI * (w + 0.5)) / smoothWin));
      weights[w] = v;
      weightSum += v;
    }
    for (let w = 0; w < smoothWin; w++) weights[w] /= weightSum;

    // Müller CENS step 4: L2 normalize smoothed vector
    for (let f = 0; f < numFrames; f++) {
      const smoothed = new Float32Array(12);
      for (let w = 0; w < smoothWin; w++) {
        const idx = Math.min(Math.max(0, f - halfWin + w), numFrames - 1);
        const cf = rawChroma[idx];
        const weight = weights[w];
        for (let c = 0; c < 12; c++) {
          smoothed[c] += cf[c] * weight;
        }
      }

      let normSq = 0;
      for (let c = 0; c < 12; c++) normSq += smoothed[c] * smoothed[c];
      const norm = Math.sqrt(normSq);
      if (norm > 1e-6) {
        const invNorm = 1.0 / norm;
        for (let c = 0; c < 12; c++) smoothed[c] *= invNorm;
      }
      cens[f] = smoothed;
    }

    return {
      chroma: rawChroma,
      cens,
      onset,
      fps: this.sampleRate / this.hopSize,
      numFrames,
      duration: samples.length / this.sampleRate,
    };
  }
}
