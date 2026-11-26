//! Synthetic WAV generation for tests.

/// Encode 16-bit PCM WAV from a per-(frame, channel) sample function.
pub fn wav_from_fn(
    rate: u32,
    channels: u16,
    frames: usize,
    mut f: impl FnMut(usize, u16) -> i16,
) -> Vec<u8> {
    let data_len = frames * channels as usize * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for i in 0..frames {
        for c in 0..channels {
            v.extend_from_slice(&f(i, c).to_le_bytes());
        }
    }
    v
}

/// Sine at `freq` Hz, amplitude 0.5, identical on all channels.
pub fn sine_wav(rate: u32, channels: u16, frames: usize, freq: f64) -> Vec<u8> {
    wav_from_fn(rate, channels, frames, |i, _| {
        let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() * 0.5;
        (s * i16::MAX as f64) as i16
    })
}

/// Stereo ramp where each sample encodes its frame index (`frames` must be < 32768).
pub fn stereo_ramp_wav(rate: u32, frames: usize) -> Vec<u8> {
    assert!(frames < 32_768);
    wav_from_fn(rate, 2, frames, |i, _| i as i16)
}

/// Recover the frame index encoded by [`stereo_ramp_wav`].
pub fn ramp_frame_index(sample: f32) -> i64 {
    (sample as f64 * 32_768.0).round() as i64
}
