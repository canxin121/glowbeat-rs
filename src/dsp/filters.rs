//! Input conditioning for analysis-only PCM.

use crate::math::{smoothing_alpha, smoothstep};

/// Metrics accumulated over one short analysis hop.
#[derive(Clone, Copy, Debug, Default)]
pub struct HopMetrics {
    /// Compressed short-term level.
    pub loudness: f32,
    /// Maximum normalized absolute sample.
    pub peak: f32,
}

/// Causal high-pass, adaptive noise gate, and feature-only AGC.
#[derive(Clone, Debug)]
pub struct Preprocessor {
    sample_rate: f32,
    hp_r: f32,
    previous_input: f32,
    previous_output: f32,
    envelope: f32,
    noise_floor: f32,
    gain: f32,
    target: f32,
    max_gain: f32,
    sum_squares: f64,
    hop_peak: f32,
    hop_count: usize,
}

impl Preprocessor {
    /// Creates an input conditioner.
    #[must_use]
    pub fn new(sample_rate: u32, high_pass_hz: f32, target: f32, max_gain: f32) -> Self {
        let sample_rate_f = sample_rate as f32;
        let hp_r = (-core::f32::consts::TAU * high_pass_hz / sample_rate_f).exp();
        Self {
            sample_rate: sample_rate_f,
            hp_r,
            previous_input: 0.0,
            previous_output: 0.0,
            envelope: 0.0,
            noise_floor: 1.0e-4,
            gain: 1.0,
            target,
            max_gain,
            sum_squares: 0.0,
            hop_peak: 0.0,
            hop_count: 0,
        }
    }

    /// Processes one mono sample and returns an analysis-normalized sample.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let finite_input = if input.is_finite() { input } else { 0.0 };
        let high_passed = finite_input - self.previous_input + self.hp_r * self.previous_output;
        self.previous_input = finite_input;
        self.previous_output = high_passed;

        let dt = 1.0 / self.sample_rate;
        let absolute = high_passed.abs();
        let envelope_tau = if absolute > self.envelope {
            0.008
        } else {
            0.180
        };
        let envelope_alpha = smoothing_alpha(dt, envelope_tau);
        self.envelope += envelope_alpha * (absolute - self.envelope);

        let floor_tau = if absolute < self.noise_floor {
            0.080
        } else {
            8.0
        };
        let floor_alpha = smoothing_alpha(dt, floor_tau);
        self.noise_floor += floor_alpha * (absolute - self.noise_floor);
        self.noise_floor = self.noise_floor.clamp(1.0e-6, 0.2);

        let gate_open = self.noise_floor.mul_add(2.2, 2.0e-5);
        let gate_full = self.noise_floor.mul_add(5.0, 2.0e-4);
        let gate = smoothstep(gate_open, gate_full, self.envelope);

        let desired_gain = if self.envelope > gate_open {
            (self.target / self.envelope.max(1.0e-5)).clamp(0.18, self.max_gain)
        } else {
            1.0
        };
        let gain_tau = if desired_gain < self.gain {
            0.018
        } else {
            0.75
        };
        let gain_alpha = smoothing_alpha(dt, gain_tau);
        self.gain += gain_alpha * (desired_gain - self.gain);

        let normalized = (high_passed * self.gain * gate).tanh();
        self.sum_squares += f64::from(normalized) * f64::from(normalized);
        self.hop_peak = self.hop_peak.max(normalized.abs());
        self.hop_count += 1;
        normalized
    }

    /// Ends the current short hop and resets hop-local accumulators.
    pub fn finish_hop(&mut self) -> HopMetrics {
        let rms = if self.hop_count == 0 {
            0.0
        } else {
            (self.sum_squares / self.hop_count as f64).sqrt() as f32
        };
        let loudness = ((1.0 + 24.0 * rms).ln() / 25.0_f32.ln()).clamp(0.0, 1.0);
        let metrics = HopMetrics {
            loudness,
            peak: self.hop_peak.clamp(0.0, 1.0),
        };
        self.sum_squares = 0.0;
        self.hop_peak = 0.0;
        self.hop_count = 0;
        metrics
    }
}
