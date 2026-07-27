//! Adaptive multi-band spectral-flux onset detection.

use crate::math::smoothstep;
use crate::types::NUM_BANDS;

const HISTORY: usize = 256;
const LANES: usize = 4;

/// Onset strengths for one analysis hop.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnsetFrame {
    /// Low-frequency event pulse.
    pub low: f32,
    /// Mid-frequency event pulse.
    pub mid: f32,
    /// High-frequency event pulse.
    pub high: f32,
    /// Broadband event pulse.
    pub impact: f32,
    /// Continuous broadband positive flux.
    pub flux: f32,
}

/// Robust adaptive onset detector with separate frequency lanes.
#[derive(Clone, Debug)]
pub struct OnsetDetector {
    previous: [f32; NUM_BANDS],
    history: [[f32; LANES]; HISTORY],
    history_write: usize,
    history_count: usize,
    thresholds: [f32; LANES],
    spreads: [f32; LANES],
    above: [bool; LANES],
    refractory: [usize; LANES],
    refractory_frames: usize,
    update_counter: usize,
    scratch: [f32; HISTORY],
}

impl OnsetDetector {
    /// Creates a detector for a given hop duration.
    #[must_use]
    pub fn new(hop_seconds: f32) -> Self {
        Self {
            previous: [0.0; NUM_BANDS],
            history: [[0.0; LANES]; HISTORY],
            history_write: 0,
            history_count: 0,
            thresholds: [0.012; LANES],
            spreads: [0.008; LANES],
            above: [false; LANES],
            refractory: [usize::MAX / 2; LANES],
            refractory_frames: (0.075 / hop_seconds).ceil().max(1.0) as usize,
            update_counter: 0,
            scratch: [0.0; HISTORY],
        }
    }

    /// Updates onset strengths from normalized logarithmic bands.
    pub fn update(&mut self, bands: &[f32; NUM_BANDS]) -> OnsetFrame {
        let mut positive = [0.0_f32; NUM_BANDS];
        for index in 0..NUM_BANDS {
            positive[index] = (bands[index] - self.previous[index]).max(0.0);
            self.previous[index] = bands[index];
        }

        let low = weighted_mean(&positive[0..7]);
        let mid = weighted_mean(&positive[7..16]);
        let high = weighted_mean(&positive[16..24]);
        let impact = 0.45 * low + 0.35 * mid + 0.20 * high;
        let raw = [low, mid, high, impact];

        self.history[self.history_write] = raw;
        self.history_write = (self.history_write + 1) % HISTORY;
        self.history_count = (self.history_count + 1).min(HISTORY);
        self.update_counter += 1;
        if self.update_counter >= 4 && self.history_count >= 16 {
            self.update_counter = 0;
            for lane in 0..LANES {
                let (median, mad) = self.median_and_mad(lane);
                self.thresholds[lane] = (median + 2.8 * mad + 0.0025).max(0.004);
                self.spreads[lane] = (5.0 * mad + 0.006).max(0.008);
            }
        }

        let mut pulse = [0.0_f32; LANES];
        for lane in 0..LANES {
            self.refractory[lane] = self.refractory[lane].saturating_add(1);
            let is_above = raw[lane] > self.thresholds[lane];
            let strength = smoothstep(
                self.thresholds[lane],
                self.thresholds[lane] + self.spreads[lane],
                raw[lane],
            );
            if is_above && !self.above[lane] && self.refractory[lane] >= self.refractory_frames {
                pulse[lane] = strength;
                self.refractory[lane] = 0;
            }
            self.above[lane] = is_above;
        }

        if pulse[3] == 0.0 {
            pulse[3] = (0.65 * pulse[0] + 0.55 * pulse[1] + 0.35 * pulse[2]).clamp(0.0, 1.0);
        }

        OnsetFrame {
            low: pulse[0],
            mid: pulse[1],
            high: pulse[2],
            impact: pulse[3],
            flux: smoothstep(
                self.thresholds[3] * 0.55,
                self.thresholds[3] + self.spreads[3],
                raw[3],
            ),
        }
    }

    fn median_and_mad(&mut self, lane: usize) -> (f32, f32) {
        for chronological in 0..self.history_count {
            let index =
                (self.history_write + HISTORY - self.history_count + chronological) % HISTORY;
            self.scratch[chronological] = self.history[index][lane];
        }
        let values = &mut self.scratch[..self.history_count];
        values.sort_by(f32::total_cmp);
        let median = values[self.history_count / 2];
        for value in values.iter_mut() {
            *value = (*value - median).abs();
        }
        values.sort_by(f32::total_cmp);
        let mad = values[self.history_count / 2];
        (median, mad)
    }
}

fn weighted_mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut weight_sum = 0.0;
    for (index, &value) in values.iter().enumerate() {
        let weight = 0.85 + 0.30 * index as f32 / values.len() as f32;
        sum += value * weight;
        weight_sum += weight;
    }
    sum / weight_sum.max(1.0e-6)
}
