//! Lightweight 64-bar spectrum analyzer (Hann-windowed radix-2 FFT, log-spaced bands).
//!
//! All work is gated by a shared `waveform_enabled` flag: when the full-screen player is closed
//! the analyzer does nothing at all.

use std::sync::atomic::{AtomicBool, Ordering};

pub const SPECTRUM_BARS: usize = 64;
pub const FFT_SIZE: usize = 2048;
const MIN_FREQ: f32 = 30.0;
const MAX_FREQ: f32 = 16_000.0;
const FLOOR_DB: f32 = -70.0;
/// Per-update multiplicative decay for falling bars (~30 updates/s).
const DECAY: f32 = 0.82;

pub struct SpectrumAnalyzer {
    sample_rate: u32,
    /// Latest mono samples, ring-indexed by `write`.
    history: Vec<f32>,
    write: usize,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// FFT bin ranges `[lo, hi)` for each bar.
    bands: [(usize, usize); SPECTRUM_BARS],
    bars: [f32; SPECTRUM_BARS],
    analyses: u64,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos()
            })
            .collect();
        let mut s = Self {
            sample_rate: sample_rate.max(1),
            history: vec![0.0; FFT_SIZE],
            write: 0,
            window,
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            bands: [(0, 1); SPECTRUM_BARS],
            bars: [0.0; SPECTRUM_BARS],
            analyses: 0,
        };
        s.compute_bands();
        s
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let sample_rate = sample_rate.max(1);
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.compute_bands();
        }
    }

    fn compute_bands(&mut self) {
        let nyquist = self.sample_rate as f32 / 2.0;
        let hz_per_bin = self.sample_rate as f32 / FFT_SIZE as f32;
        let max_f = MAX_FREQ.min(nyquist * 0.95);
        let ratio = (max_f / MIN_FREQ).ln();
        let max_bin = FFT_SIZE / 2;
        for (b, band) in self.bands.iter_mut().enumerate() {
            let f_lo = MIN_FREQ * (ratio * b as f32 / SPECTRUM_BARS as f32).exp();
            let f_hi = MIN_FREQ * (ratio * (b + 1) as f32 / SPECTRUM_BARS as f32).exp();
            let lo = ((f_lo / hz_per_bin).floor() as usize).clamp(1, max_bin - 1);
            let hi = ((f_hi / hz_per_bin).ceil() as usize).clamp(lo + 1, max_bin);
            *band = (lo, hi);
        }
    }

    /// Append interleaved stereo samples (downmixed to mono) into the analysis window.
    pub fn push_stereo(&mut self, stereo: &[f32]) {
        for f in stereo.as_chunks::<2>().0 {
            self.push_mono_sample((f[0] + f[1]) * 0.5);
        }
    }

    /// Append mono samples into the analysis window.
    pub fn push_mono(&mut self, mono: &[f32]) {
        for &s in mono {
            self.push_mono_sample(s);
        }
    }

    #[inline]
    fn push_mono_sample(&mut self, s: f32) {
        self.history[self.write] = s;
        self.write = (self.write + 1) % FFT_SIZE;
    }

    /// Feed stereo samples and recompute bars, but only if `enabled` is set.
    ///
    /// Returns `false` (and does no work at all) when disabled.
    pub fn process(&mut self, stereo: &[f32], enabled: &AtomicBool) -> bool {
        if !enabled.load(Ordering::Relaxed) {
            return false;
        }
        self.push_stereo(stereo);
        self.analyze();
        true
    }

    /// Run the FFT over the current window and update bars with exponential decay.
    pub fn analyze(&mut self) {
        self.analyses += 1;
        for i in 0..FFT_SIZE {
            let s = self.history[(self.write + i) % FFT_SIZE];
            self.re[i] = s * self.window[i];
            self.im[i] = 0.0;
        }
        fft_in_place(&mut self.re, &mut self.im);
        // Hann coherent gain 0.5 => a full-scale sine peaks at N/4.
        let norm = 4.0 / FFT_SIZE as f32;
        for (b, &(lo, hi)) in self.bands.iter().enumerate() {
            let mut peak = 0.0f32;
            for k in lo..hi {
                let m = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * norm;
                peak = peak.max(m);
            }
            let db = 20.0 * peak.max(1e-9).log10();
            let target = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            let decayed = self.bars[b] * DECAY;
            self.bars[b] = if target > decayed { target } else { decayed };
        }
    }

    /// Let bars fall toward zero without analyzing (e.g. while paused). Returns true if any bar
    /// is still non-zero.
    pub fn decay(&mut self) -> bool {
        let mut any = false;
        for b in &mut self.bars {
            *b *= DECAY;
            if *b < 0.002 {
                *b = 0.0;
            } else {
                any = true;
            }
        }
        any
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.bars = [0.0; SPECTRUM_BARS];
    }

    pub fn bars(&self) -> [f32; SPECTRUM_BARS] {
        self.bars
    }

    /// Number of FFT analyses performed (for diagnostics/tests).
    pub fn analyses(&self) -> u64 {
        self.analyses
    }

    /// Bar index that contains `freq` Hz.
    pub fn bar_for_freq(&self, freq: f32) -> Option<usize> {
        let k = (freq * FFT_SIZE as f32 / self.sample_rate as f32).round() as usize;
        self.bands.iter().position(|&(lo, hi)| k >= lo && k < hi)
    }
}

/// Iterative in-place radix-2 Cooley-Tukey FFT. `re.len()` must be a power of two.
fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // Bit reversal.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (w_im, w_re) = ang.sin_cos();
        let (w_re, w_im) = (w_re as f32, w_im as f32);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * w_re - ci * w_im;
                ci = cr * w_im + ci * w_re;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frames: usize, freq: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.8;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn sine_440_activates_its_bar() {
        let enabled = AtomicBool::new(true);
        let mut a = SpectrumAnalyzer::new(48_000);
        assert!(a.process(&sine(48_000, FFT_SIZE * 2, 440.0), &enabled));
        let bars = a.bars();
        let peak_bar = bars
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap()
            .0;
        let expected = a.bar_for_freq(440.0).unwrap();
        assert!(
            (peak_bar as i64 - expected as i64).abs() <= 1,
            "peak {peak_bar} expected {expected}"
        );
        assert!(bars[peak_bar] > 0.9, "peak level {}", bars[peak_bar]);
        // Far-away bars stay quiet.
        assert!(bars[SPECTRUM_BARS - 1] < 0.3, "{:?}", bars);
        assert!(bars.iter().all(|b| (0.0..=1.0).contains(b)));
    }

    #[test]
    fn disabled_flag_skips_all_work_and_bars_decay() {
        let enabled = AtomicBool::new(false);
        let mut a = SpectrumAnalyzer::new(44_100);
        assert!(!a.process(&sine(44_100, 4096, 1000.0), &enabled));
        assert_eq!(a.analyses(), 0);
        assert_eq!(a.bars(), [0.0; SPECTRUM_BARS]);

        enabled.store(true, Ordering::Relaxed);
        assert!(a.process(&sine(44_100, 4096, 1000.0), &enabled));
        assert_eq!(a.analyses(), 1);
        let before = a.bars();
        // Silence: bars fall exponentially, not instantly.
        a.process(&vec![0.0; FFT_SIZE * 2], &enabled);
        let bar = a.bar_for_freq(1000.0).unwrap();
        assert!(a.bars()[bar] < before[bar] && a.bars()[bar] > 0.5 * before[bar]);
        while a.decay() {}
        assert_eq!(a.bars(), [0.0; SPECTRUM_BARS]);
    }

    #[test]
    fn fft_matches_dft_on_impulse() {
        let mut re = vec![0.0f32; 8];
        let mut im = vec![0.0f32; 8];
        re[1] = 1.0;
        fft_in_place(&mut re, &mut im);
        for k in 0..8 {
            let ang = -2.0 * std::f32::consts::PI * k as f32 / 8.0;
            assert!((re[k] - ang.cos()).abs() < 1e-5);
            assert!((im[k] - ang.sin()).abs() < 1e-5);
        }
    }
}
