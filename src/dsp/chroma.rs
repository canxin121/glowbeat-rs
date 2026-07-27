//! Long-window chroma and tonal confidence.

use crate::math::smoothing_alpha;
use crate::types::NUM_CHROMA;

/// Smoothed pitch-class profile.
#[derive(Clone, Copy, Debug)]
pub struct ChromaFrame {
    /// C through B pitch-class strengths, normalized to sum to one when tonal.
    pub values: [f32; NUM_CHROMA],
    /// Tonal confidence in `[0, 1]`.
    pub confidence: f32,
}

impl Default for ChromaFrame {
    fn default() -> Self {
        Self {
            values: [0.0; NUM_CHROMA],
            confidence: 0.0,
        }
    }
}

/// CQT-like pitch-class folding over an FFT magnitude spectrum.
#[derive(Clone, Debug)]
pub struct ChromaAnalyzer {
    smoothed: [f32; NUM_CHROMA],
    confidence: f32,
}

impl ChromaAnalyzer {
    /// Creates a chroma analyzer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            smoothed: [0.0; NUM_CHROMA],
            confidence: 0.0,
        }
    }

    /// Updates chroma from a magnitude spectrum.
    pub fn update(
        &mut self,
        magnitudes: &[f32],
        sample_rate: u32,
        fft_size: usize,
        dt: f32,
    ) -> ChromaFrame {
        let mut raw = [0.0_f32; NUM_CHROMA];
        for (bin, &magnitude) in magnitudes.iter().enumerate().skip(1) {
            let frequency = bin as f32 * sample_rate as f32 / fft_size as f32;
            if !(55.0..=5_500.0).contains(&frequency) {
                continue;
            }
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            let base = midi.floor();
            let fraction = midi - base;
            let pitch = base as i32;
            let left = pitch.rem_euclid(NUM_CHROMA as i32) as usize;
            let right = (left + 1) % NUM_CHROMA;
            let whitening = magnitude.max(0.0).powf(0.38);
            let low_suppression = (frequency / 90.0).clamp(0.25, 1.0);
            let value = whitening * low_suppression;
            raw[left] += value * (1.0 - fraction);
            raw[right] += value * fraction;
        }

        let sum = raw.iter().sum::<f32>();
        if sum > 1.0e-9 {
            for value in &mut raw {
                *value /= sum;
            }
        }
        let alpha = smoothing_alpha(dt, 0.48);
        for (smoothed, value) in self.smoothed.iter_mut().zip(raw) {
            *smoothed += alpha * (value - *smoothed);
        }
        let smooth_sum = self.smoothed.iter().sum::<f32>();
        if smooth_sum > 1.0e-9 {
            for value in &mut self.smoothed {
                *value /= smooth_sum;
            }
        }

        let mut entropy = 0.0;
        let mut maximum = 0.0_f32;
        for &value in &self.smoothed {
            if value > 1.0e-9 {
                entropy -= value * value.ln();
            }
            maximum = maximum.max(value);
        }
        let entropy_confidence = 1.0 - entropy / (NUM_CHROMA as f32).ln();
        let peak_confidence = ((maximum - 1.0 / NUM_CHROMA as f32) * 3.0).clamp(0.0, 1.0);
        let raw_confidence =
            (0.65 * entropy_confidence + 0.35 * peak_confidence) * (sum * 0.08).clamp(0.0, 1.0);
        self.confidence += smoothing_alpha(dt, 0.75) * (raw_confidence - self.confidence);

        ChromaFrame {
            values: self.smoothed,
            confidence: self.confidence.clamp(0.0, 1.0),
        }
    }
}

impl Default for ChromaAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
