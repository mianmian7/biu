/** WAV decoding for fixtures and evaluation. Runtime decoding supplies PCM directly. */
export function parseWav(bytes: Uint8Array): {
  sampleRate: number;
  channels: number;
  samples: Float32Array;
  duration: number;
} {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const tag = (offset: number) => String.fromCharCode(...bytes.subarray(offset, offset + 4));
  if (bytes.length < 12 || tag(0) !== "RIFF" || tag(8) !== "WAVE") {
    throw new Error("Invalid WAV header");
  }
  let format: { code: number; channels: number; sampleRate: number; bits: number } | undefined;
  let data: { offset: number; size: number } | undefined;
  for (let offset = 12; offset < bytes.length; ) {
    if (offset + 8 > bytes.length) throw new Error("Truncated WAV chunk header");
    const chunk = tag(offset);
    const size = view.getUint32(offset + 4, true);
    const start = offset + 8;
    if (start + size > bytes.length) throw new Error("Truncated WAV chunk");
    if (chunk === "fmt ") {
      if (size < 16) throw new Error("Invalid WAV fmt chunk");
      format = {
        code: view.getUint16(start, true),
        channels: view.getUint16(start + 2, true),
        sampleRate: view.getUint32(start + 4, true),
        bits: view.getUint16(start + 14, true),
      };
    } else if (chunk === "data") {
      data = { offset: start, size };
      break;
    }
    offset = start + size + (size % 2);
  }
  if (!format || !data) throw new Error("Missing fmt or data chunk in WAV");
  if (format.code !== 1) throw new Error(`Unsupported WAV format ${format.code}`);
  if (format.bits !== 16) throw new Error(`Unsupported bitsPerSample: ${format.bits}`);
  if (format.channels !== 1 && format.channels !== 2) {
    throw new Error(`Unsupported channel count: ${format.channels}`);
  }
  if (format.sampleRate === 0) throw new Error("Invalid WAV sample rate");
  const frameBytes = 2 * format.channels;
  if (data.size % frameBytes !== 0) throw new Error("Incomplete WAV sample frame");
  const samples = new Float32Array(data.size / frameBytes);
  for (let i = 0; i < samples.length; i++) {
    const offset = data.offset + i * frameBytes;
    const left = view.getInt16(offset, true);
    samples[i] = format.channels === 1 ? left / 32768 : (left + view.getInt16(offset + 2, true)) / (2 * 32768);
  }
  return { sampleRate: format.sampleRate, channels: 1, samples, duration: samples.length / format.sampleRate };
}
