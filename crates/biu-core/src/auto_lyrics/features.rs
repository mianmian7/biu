use std::f32::consts::TAU;

pub const SAMPLE_RATE: u32 = 16_000;
const FFT_SIZE: usize = 2048;
// Keep the original 43 frames per second when decoding at 16 kHz.
const HOP: usize = 372;
const SMOOTH: usize = 41;

pub struct Fft {
    size: usize,
    bits: Vec<usize>,
    roots: Vec<(f32, f32)>,
    window: Vec<f32>,
}

impl Fft {
    pub fn new(size: usize) -> Option<Self> {
        if size < 2 || !size.is_power_of_two() {
            return None;
        }
        let bits = (0..size)
            .map(|i| i.reverse_bits() >> (usize::BITS - size.ilog2()))
            .collect();
        let roots = (0..size / 2)
            .map(|i| {
                let angle = -TAU * i as f32 / size as f32;
                let (sin, cos) = angle.sin_cos();
                (cos, sin)
            })
            .collect();
        let window = (0..size)
            .map(|i| 0.5 * (1.0 - (TAU * i as f32 / (size - 1) as f32).cos()))
            .collect();
        Some(Self {
            size,
            bits,
            roots,
            window,
        })
    }

    /// In-place forward radix-2 FFT on two buffers of the configured length.
    pub fn transform(&self, real: &mut [f32], imag: &mut [f32]) {
        assert_eq!(real.len(), self.size);
        assert_eq!(imag.len(), self.size);
        for (i, &reversed) in self.bits.iter().enumerate() {
            if i < reversed {
                real.swap(i, reversed);
                imag.swap(i, reversed);
            }
        }
        for level in 1..=self.size.ilog2() {
            let width = 1 << level;
            let half = width / 2;
            let step = self.size / width;
            for (real, imag) in real
                .chunks_exact_mut(width)
                .zip(imag.chunks_exact_mut(width))
            {
                for i in 0..half {
                    let (cos, sin) = self.roots[i * step];
                    let j = i + half;
                    let re = real[j] * cos - imag[j] * sin;
                    let im = real[j] * sin + imag[j] * cos;
                    real[j] = real[i] - re;
                    imag[j] = imag[i] - im;
                    real[i] += re;
                    imag[i] += im;
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct AudioFeatures {
    pub fps: f64,
    pub duration_secs: f64,
    pub chroma: Vec<[u8; 12]>,
    pub cens: Vec<[f32; 12]>,
    pub onset: Vec<f32>,
    // Non-overlapping hop energies for the vocal-onset check.
    pub(super) energy: Vec<f32>,
}

pub fn extract_features(samples: &[f32]) -> AudioFeatures {
    let mut extractor = FeatureExtractor::new();
    for chunk in samples.chunks(8192) {
        extractor.push(chunk.iter().copied());
    }
    extractor.finish()
}

struct Bin {
    index: usize,
    pitch: usize,
    fraction: f32,
}

pub(super) struct FeatureExtractor {
    fft: Fft,
    bins: Vec<Bin>,
    real: Vec<f32>,
    imag: Vec<f32>,
    mag: Vec<f32>,
    previous: Vec<f32>,
    samples: Vec<f32>,
    sample_count: usize,
    chroma: Vec<[u8; 12]>,
    onset: Vec<f32>,
    energy: Vec<f32>,
}

impl FeatureExtractor {
    pub(super) fn new() -> Self {
        let bins = (1..FFT_SIZE / 2)
            .filter_map(|index| {
                let freq = index as f32 * SAMPLE_RATE as f32 / FFT_SIZE as f32;
                if !(65.0..=3500.0).contains(&freq) {
                    return None;
                }
                let pitch = (12.0 * (freq / 440.0).log2() + 69.0).rem_euclid(12.0);
                Some(Bin {
                    index,
                    pitch: pitch as usize % 12,
                    fraction: pitch.fract(),
                })
            })
            .collect();
        Self {
            fft: Fft::new(FFT_SIZE).unwrap(),
            bins,
            real: vec![0.0; FFT_SIZE],
            imag: vec![0.0; FFT_SIZE],
            mag: vec![0.0; FFT_SIZE / 2],
            previous: vec![0.0; FFT_SIZE / 2],
            samples: Vec::with_capacity(FFT_SIZE + 8192),
            sample_count: 0,
            chroma: Vec::new(),
            onset: Vec::new(),
            energy: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, samples: impl IntoIterator<Item = f32>) {
        let old_len = self.samples.len();
        self.samples.extend(samples);
        self.sample_count += self.samples.len() - old_len;
        let mut start = 0;
        while start + FFT_SIZE <= self.samples.len() {
            let frame = &self.samples[start..start + FFT_SIZE];
            for ((real, &sample), &gain) in self.real.iter_mut().zip(frame).zip(&self.fft.window) {
                *real = sample * gain;
            }
            self.imag.fill(0.0);
            self.fft.transform(&mut self.real, &mut self.imag);
            self.energy.push(
                frame[..HOP]
                    .iter()
                    .map(|sample| sample * sample)
                    .sum::<f32>()
                    / HOP as f32,
            );
            let mut flux = 0.0;
            for (k, magnitude) in self.mag.iter_mut().enumerate().skip(1) {
                *magnitude = self.real[k].hypot(self.imag[k]);
                flux += (*magnitude - self.previous[k]).max(0.0);
            }
            self.onset.push(flux);
            std::mem::swap(&mut self.mag, &mut self.previous);
            let mut energy = [0.0; 12];
            for bin in &self.bins {
                let value = self.previous[bin.index].powi(2);
                energy[bin.pitch] += value * (1.0 - bin.fraction);
                energy[(bin.pitch + 1) % 12] += value * bin.fraction;
            }
            let sum: f32 = energy.iter().sum();
            self.chroma.push(if sum > 1e-6 {
                energy.map(|v| {
                    [0.05, 0.1, 0.2, 0.4].partition_point(|threshold| v / sum >= *threshold) as u8
                })
            } else {
                [0; 12]
            });
            start += HOP;
        }
        self.samples.copy_within(start.., 0);
        self.samples.truncate(self.samples.len() - start);
    }

    pub(super) fn finish(self) -> AudioFeatures {
        let weights: [f32; SMOOTH] =
            std::array::from_fn(|i| 0.5 * (1.0 - (TAU * (i as f32 + 0.5) / SMOOTH as f32).cos()));
        let cens = (0..self.chroma.len())
            .map(|index| {
                let mut row = [0.0; 12];
                for (i, weight) in weights.iter().enumerate() {
                    let frame = (index + i)
                        .saturating_sub(SMOOTH / 2)
                        .min(self.chroma.len() - 1);
                    for (output, value) in row.iter_mut().zip(self.chroma[frame]) {
                        *output += f32::from(value) * weight;
                    }
                }
                let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 1e-6 {
                    row.iter_mut().for_each(|value| *value /= norm);
                }
                row
            })
            .collect();
        AudioFeatures {
            fps: f64::from(SAMPLE_RATE) / HOP as f64,
            duration_secs: self.sample_count as f64 / f64::from(SAMPLE_RATE),
            chroma: self.chroma,
            cens,
            onset: self.onset,
            energy: self.energy,
        }
    }
}

pub(super) fn has_vocal_onset(audio: &AudioFeatures, first_line: f64) -> bool {
    let energy = &audio.energy;
    let fps = audio.fps;
    if first_line < 1.5 {
        let end = (((first_line + 1.5) * fps).ceil() as usize).min(energy.len());
        return end > 0 && energy[..end].iter().sum::<f32>() / end as f32 >= 1e-4;
    }
    [(0.1, 0.01), (0.5, 0.002)]
        .into_iter()
        .any(|(seconds, delta)| {
            let width = (seconds * fps).round().max(1.0) as usize;
            let start = (((first_line - 1.5) * fps) as usize).max(width);
            let end = (((first_line + 1.5) * fps).ceil() as usize)
                .min(energy.len().saturating_sub(width));
            let mean = |i: usize| energy[i..i + width].iter().sum::<f32>() / width as f32;
            (start..=end).step_by((width / 2).max(1)).any(|i| {
                let before = mean(i - width);
                let after = mean(i);
                after >= before * 1.6 && after - before >= delta
            })
        })
}
