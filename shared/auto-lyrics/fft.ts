/**
 * Fast Radix-2 Cooley-Tukey FFT & Chroma / CENS Feature Extractor in pure JS.
 * Zero external dependencies.
 */

export class FFT {
  size: number;
  levels: number;
  cosTable: Float32Array;
  sinTable: Float32Array;
  bitRev: Uint32Array;
  hann: Float32Array;

  constructor(size: number) {
    if ((size & (size - 1)) !== 0) {
      throw new Error(`FFT size must be a power of 2, got ${size}`);
    }
    this.size = size;
    this.levels = Math.log2(size);
    this.cosTable = new Float32Array(size / 2);
    this.sinTable = new Float32Array(size / 2);
    this.bitRev = new Uint32Array(size);

    for (let i = 0; i < size / 2; i++) {
      const angle = (-2 * Math.PI * i) / size;
      this.cosTable[i] = Math.cos(angle);
      this.sinTable[i] = Math.sin(angle);
    }

    for (let i = 0; i < size; i++) {
      let rev = 0;
      let temp = i;
      for (let j = 0; j < this.levels; j++) {
        rev = (rev << 1) | (temp & 1);
        temp >>= 1;
      }
      this.bitRev[i] = rev;
    }

    // Precomputed Hann window
    this.hann = new Float32Array(size);
    for (let i = 0; i < size; i++) {
      this.hann[i] = 0.5 * (1 - Math.cos((2 * Math.PI * i) / (size - 1)));
    }
  }

  /**
   * In-place Radix-2 FFT on real and imag Float32Array buffers.
   */
  transform(real: Float32Array, imag: Float32Array): void {
    const n = this.size;
    const bitRev = this.bitRev;
    for (let i = 0; i < n; i++) {
      const j = bitRev[i];
      if (i < j) {
        const tr = real[i];
        real[i] = real[j];
        real[j] = tr;
        const ti = imag[i];
        imag[i] = imag[j];
        imag[j] = ti;
      }
    }

    const cosTable = this.cosTable;
    const sinTable = this.sinTable;

    for (let len = 2; len <= n; len <<= 1) {
      const halfLen = len >> 1;
      const step = n / len;
      for (let i = 0; i < n; i += len) {
        let k = 0;
        for (let j = 0; j < halfLen; j++) {
          const c = cosTable[k];
          const s = sinTable[k];
          k += step;

          const uR = real[i + j];
          const uI = imag[i + j];
          const vR = real[i + j + halfLen];
          const vI = imag[i + j + halfLen];

          const tR = vR * c - vI * s;
          const tI = vR * s + vI * c;

          real[i + j] = uR + tR;
          imag[i + j] = uI + tI;
          real[i + j + halfLen] = uR - tR;
          imag[i + j + halfLen] = uI - tI;
        }
      }
    }
  }
}
