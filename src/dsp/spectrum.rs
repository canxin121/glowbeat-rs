//! Multi-band causal spectrum analysis.

use crate::math::{hann_window, Complex32, FftPlan};
use crate::types::NUM_BANDS;

#[derive(Clone, Copy, Debug)]
struct BinMap {
    left: u8,
    right: u8,
    left_weight: f32,
    right_weight: f32,
    frequency_hz: f32,
}

/// Output of one short-time spectrum frame.
#[derive(Clone, Debug)]
pub struct SpectrumFeatures {
    /// Normalized logarithmic bands.
    pub bands: [f32; NUM_BANDS],
    /// Normalized spectral centroid.
    pub centroid: f32,
    /// Compressed total spectral energy.
    pub total_energy: f32,
}

impl Default for SpectrumFeatures {
    fn default() -> Self {
        Self {
            bands: [0.0; NUM_BANDS],
            centroid: 0.0,
            total_energy: 0.0,
        }
    }
}

/// Reusable FFT and logarithmic filterbank.
#[derive(Clone, Debug)]
pub struct SpectrumAnalyzer {
    fft_size: usize,
    sample_rate: u32,
    fft: FftPlan,
    window: Vec<f32>,
    buffer: Vec<Complex32>,
    magnitudes: Vec<f32>,
    bin_map: Vec<Option<BinMap>>,
    band_peak: [f32; NUM_BANDS],
    smoothed_bands: [f32; NUM_BANDS],
}

impl SpectrumAnalyzer {
    /// Creates a spectrum analyzer and all reusable buffers.
    #[must_use]
    pub fn new(
        sample_rate: u32,
        fft_size: usize,
        min_frequency_hz: f32,
        max_frequency_hz: f32,
    ) -> Self {
        let fft = FftPlan::new(fft_size);
        let window = hann_window(fft_size);
        let buffer = vec![Complex32::default(); fft_size];
        let magnitudes = vec![0.0; fft_size / 2 + 1];
        let min_log = min_frequency_hz.ln();
        let max_log = max_frequency_hz.ln();
        let bin_map = (0..=fft_size / 2)
            .map(|bin| {
                let frequency_hz = bin as f32 * sample_rate as f32 / fft_size as f32;
                if frequency_hz < min_frequency_hz || frequency_hz > max_frequency_hz {
                    return None;
                }
                let position =
                    ((frequency_hz.ln() - min_log) / (max_log - min_log)) * (NUM_BANDS - 1) as f32;
                let left = position.floor().clamp(0.0, (NUM_BANDS - 1) as f32) as usize;
                let right = (left + 1).min(NUM_BANDS - 1);
                let right_weight = (position - left as f32).clamp(0.0, 1.0);
                Some(BinMap {
                    left: left as u8,
                    right: right as u8,
                    left_weight: 1.0 - right_weight,
                    right_weight,
                    frequency_hz,
                })
            })
            .collect();
        Self {
            fft_size,
            sample_rate,
            fft,
            window,
            buffer,
            magnitudes,
            bin_map,
            band_peak: [0.08; NUM_BANDS],
            smoothed_bands: [0.0; NUM_BANDS],
        }
    }

    /// FFT size.
    #[must_use]
    pub const fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Most recently computed magnitude spectrum.
    #[must_use]
    pub fn magnitudes(&self) -> &[f32] {
        &self.magnitudes
    }

    /// Analyzes the newest `fft_size` samples in a circular buffer.
    pub fn analyze_ring(&mut self, ring: &[f32], next_write: usize) -> SpectrumFeatures {
        debug_assert!(ring.len() >= self.fft_size);
        let ring_len = ring.len();
        let oldest = (next_write + ring_len - self.fft_size) % ring_len;
        for index in 0..self.fft_size {
            let sample = ring[(oldest + index) % ring_len] * self.window[index];
            self.buffer[index] = Complex32::new(sample, 0.0);
        }
        self.fft.process(&mut self.buffer);

        let amplitude_scale = 2.0 / self.fft_size as f32;
        for (bin, magnitude) in self.magnitudes.iter_mut().enumerate() {
            *magnitude = self.buffer[bin].norm_sqr().sqrt() * amplitude_scale;
        }

        let mut band_energy = [0.0_f32; NUM_BANDS];
        let mut centroid_numerator = 0.0;
        let mut centroid_denominator = 0.0;
        for (bin, mapping) in self.bin_map.iter().enumerate() {
            let Some(mapping) = mapping else {
                continue;
            };
            let magnitude = self.magnitudes[bin];
            let perceptual = magnitude.sqrt();
            let left = mapping.left as usize;
            let right = mapping.right as usize;
            band_energy[left] += perceptual * mapping.left_weight;
            band_energy[right] += perceptual * mapping.right_weight;
            centroid_numerator = mapping.frequency_hz.mul_add(perceptual, centroid_numerator);
            centroid_denominator += perceptual;
        }

        let hop_attack = 0.55;
        let hop_release = 0.14;
        for ((energy, peak), smoothed) in band_energy
            .iter()
            .zip(&mut self.band_peak)
            .zip(&mut self.smoothed_bands)
        {
            let compressed = ((1.0 + 18.0 * energy).ln() / 19.0_f32.ln()).clamp(0.0, 4.0);
            *peak = (*peak * 0.9992).max(compressed);
            let normalized = (compressed / (*peak + 0.04)).clamp(0.0, 1.35);
            let coefficient = if normalized > *smoothed {
                hop_attack
            } else {
                hop_release
            };
            *smoothed += coefficient * (normalized - *smoothed);
        }

        let centroid = if centroid_denominator > 1.0e-9 {
            let hz = centroid_numerator / centroid_denominator;
            let nyquist = self.sample_rate as f32 * 0.5;
            (hz / nyquist).sqrt().clamp(0.0, 1.0)
        } else {
            0.0
        };
        let total_linear = self.magnitudes.iter().skip(1).copied().sum::<f32>()
            / self.magnitudes.len().max(1) as f32;
        let total_energy = ((1.0 + 80.0 * total_linear).ln() / 81.0_f32.ln()).clamp(0.0, 1.0);

        SpectrumFeatures {
            bands: self.smoothed_bands,
            centroid,
            total_energy,
        }
    }
}
