//! Streaming stereo cubic (Catmull-Rom) resampler with variable playback rate.
//!
//! The effective step through the source is `src_rate / out_rate * rate`, so playback speed is
//! applied by resampling (tape-style: pitch follows speed). Changing the rate or output rate
//! mid-stream keeps the fractional read position, so there are no discontinuities.

pub const MIN_RATE: f32 = 0.5;
pub const MAX_RATE: f32 = 2.0;

/// Clamp a playback rate to the supported range (NaN -> 1.0).
pub fn clamp_rate(rate: f32) -> f32 {
    if rate.is_finite() {
        rate.clamp(MIN_RATE, MAX_RATE)
    } else {
        1.0
    }
}

#[derive(Debug, Clone)]
pub struct Resampler {
    src_rate: u32,
    out_rate: u32,
    rate: f32,
    step: f64,
    /// Pending interleaved stereo input frames (with one frame of history at index 0).
    buf: Vec<f32>,
    /// Fractional frame read position into `buf`.
    pos: f64,
    primed: bool,
}

impl Resampler {
    pub fn new(src_rate: u32, out_rate: u32) -> Self {
        let mut r = Self {
            src_rate: src_rate.max(1),
            out_rate: out_rate.max(1),
            rate: 1.0,
            step: 1.0,
            buf: Vec::with_capacity(8192),
            pos: 1.0,
            primed: false,
        };
        r.update_step();
        r
    }

    fn update_step(&mut self) {
        self.step = self.src_rate as f64 / self.out_rate as f64 * self.rate as f64;
    }

    /// Set playback speed (clamped to `0.5..=2.0`).
    pub fn set_rate(&mut self, rate: f32) {
        let rate = clamp_rate(rate);
        if rate != self.rate {
            self.rate = rate;
            self.update_step();
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn set_source_rate(&mut self, src_rate: u32) {
        let src_rate = src_rate.max(1);
        if src_rate != self.src_rate {
            self.src_rate = src_rate;
            self.update_step();
        }
    }

    pub fn set_output_rate(&mut self, out_rate: u32) {
        let out_rate = out_rate.max(1);
        if out_rate != self.out_rate {
            self.out_rate = out_rate;
            self.update_step();
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    /// Source frames consumed per output frame.
    pub fn step(&self) -> f64 {
        self.step
    }

    /// Drop all buffered state (after a seek).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.pos = 1.0;
        self.primed = false;
    }

    /// Feed interleaved stereo `input`, appending resampled stereo frames to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        debug_assert!(input.len().is_multiple_of(2));
        if !input.is_empty() {
            if !self.primed {
                // Duplicate the first frame as history so index `pos - 1` always exists.
                self.buf.extend_from_slice(&input[..2]);
                self.primed = true;
            }
            self.buf.extend_from_slice(input);
        }
        self.render(out);
    }

    /// Emit the tail at end of stream (pads with the last frame).
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if !self.primed || self.buf.len() < 2 {
            return;
        }
        let n = self.buf.len();
        let (l, r) = (self.buf[n - 2], self.buf[n - 1]);
        let frames_before = n / 2;
        self.buf.extend_from_slice(&[l, r, l, r]);
        // Only render up to the last real frame.
        let limit = (frames_before - 1) as f64;
        while self.pos <= limit {
            self.emit(out);
        }
        self.reset();
    }

    fn render(&mut self, out: &mut Vec<f32>) {
        let frames = self.buf.len() / 2;
        if frames < 4 {
            return;
        }
        // Need frames floor(pos)-1 ..= floor(pos)+2.
        let limit = (frames - 2) as f64;
        let estimate = ((limit - self.pos) / self.step).max(0.0) as usize + 1;
        out.reserve(estimate * 2);
        while self.pos < limit {
            self.emit(out);
        }
        // Discard consumed frames, keeping one frame of history.
        let drop = (self.pos.floor() as usize).saturating_sub(1);
        if drop > 0 {
            self.buf.drain(..drop * 2);
            self.pos -= drop as f64;
        }
    }

    #[inline]
    fn emit(&mut self, out: &mut Vec<f32>) {
        let i = self.pos.floor() as usize;
        let t = (self.pos - i as f64) as f32;
        let b = &self.buf;
        for c in 0..2 {
            let p0 = b[(i - 1) * 2 + c];
            let p1 = b[i * 2 + c];
            let p2 = b[(i + 1) * 2 + c];
            let p3 = b[(i + 2) * 2 + c];
            out.push(catmull_rom(p0, p1, p2, p3, t).clamp(-1.0, 1.0));
        }
        self.pos += self.step;
    }
}

#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo(rate: u32, frames: usize, freq: f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32
                    * 0.5;
                [s, s]
            })
            .collect()
    }

    fn zero_crossings(stereo: &[f32]) -> usize {
        let l: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        l.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    fn run_chunked(r: &mut Resampler, input: &[f32], chunk_frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for chunk in input.chunks(chunk_frames * 2) {
            r.process(chunk, &mut out);
        }
        r.flush(&mut out);
        out
    }

    #[test]
    fn upsample_44k1_to_48k_preserves_length_and_pitch() {
        let input = sine_stereo(44_100, 44_100, 1000.0);
        let mut r = Resampler::new(44_100, 48_000);
        let out = run_chunked(&mut r, &input, 1024);
        let frames = out.len() / 2;
        assert!((frames as i64 - 48_000).abs() <= 3, "frames {frames}");
        // 1 kHz for 1 s => ~2000 zero crossings in both domains.
        let zc = zero_crossings(&out) as i64;
        assert!((zc - 2000).abs() <= 4, "zero crossings {zc}");
        // Amplitude preserved.
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn rate_1_5x_shortens_output() {
        let input = sine_stereo(44_100, 44_100, 1000.0);
        let mut r = Resampler::new(44_100, 48_000);
        r.set_rate(1.5);
        let out = run_chunked(&mut r, &input, 700);
        let frames = out.len() / 2;
        assert!((frames as i64 - 32_000).abs() <= 3, "frames {frames}");
        // Pitch scales with speed: 1.5 kHz at 48 kHz for 2/3 s => ~2000 crossings still.
        let zc = zero_crossings(&out) as i64;
        assert!((zc - 2000).abs() <= 4, "zero crossings {zc}");
    }

    #[test]
    fn identity_is_exact_and_rate_is_clamped() {
        let input: Vec<f32> = (0..2000).map(|i| ((i % 200) as f32 / 200.0) - 0.5).collect();
        let mut r = Resampler::new(48_000, 48_000);
        let out = run_chunked(&mut r, &input, 128);
        assert_eq!(out.len(), input.len());
        for (a, b) in out.iter().zip(&input) {
            assert!((a - b).abs() < 1e-6);
        }
        r.set_rate(10.0);
        assert_eq!(r.rate(), MAX_RATE);
        r.set_rate(0.1);
        assert_eq!(r.rate(), MIN_RATE);
        r.set_rate(f32::NAN);
        assert_eq!(r.rate(), 1.0);
    }

    #[test]
    fn mid_stream_rate_change_is_continuous() {
        let input = sine_stereo(48_000, 48_000, 200.0);
        let mut r = Resampler::new(48_000, 48_000);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(512).enumerate() {
            r.set_rate(if n % 2 == 0 { 0.8 } else { 1.3 });
            r.process(chunk, &mut out);
        }
        // A 200 Hz sine sped up to at most 1.3x never jumps more than ~0.04 per sample.
        let max_jump = out
            .as_chunks::<2>().0.iter()
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_jump < 0.05, "discontinuity {max_jump}");
    }
}
