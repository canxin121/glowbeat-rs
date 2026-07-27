//! Multi-hypothesis tempo estimation with a continuously corrected beat oscillator.

use crate::math::{lerp, wrap01, wrap_signed_cycle};

/// Beat tracker output for one analysis hop.
#[derive(Clone, Copy, Debug)]
pub struct BeatFrame {
    /// Estimated BPM.
    pub bpm: f32,
    /// Beat phase in `[0, 1)`.
    pub phase: f32,
    /// Beat confidence.
    pub confidence: f32,
    /// Beat position within the bar.
    pub beat_in_bar: u8,
    /// Inferred meter.
    pub meter: u8,
    /// Bar phase in `[0, 1)`.
    pub bar_phase: f32,
    /// Meter/downbeat confidence.
    pub downbeat_confidence: f32,
    /// True for one hop when the oscillator wraps.
    pub beat_event: bool,
    /// True for one hop on an inferred downbeat.
    pub downbeat_event: bool,
    /// Monotonic beat index.
    pub beat_index: u64,
}

impl Default for BeatFrame {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            phase: 0.0,
            confidence: 0.0,
            beat_in_bar: 0,
            meter: 4,
            bar_phase: 0.0,
            downbeat_confidence: 0.0,
            beat_event: false,
            downbeat_event: false,
            beat_index: 0,
        }
    }
}

/// Tempo/phase tracker designed for causal streaming input.
#[derive(Clone, Debug)]
pub struct BeatTracker {
    dt: f32,
    min_bpm: f32,
    max_bpm: f32,
    history: Vec<f32>,
    history_write: usize,
    history_count: usize,
    scores: Vec<f32>,
    frames_until_update: usize,
    update_interval: usize,
    bpm: f32,
    confidence: f32,
    phase: f32,
    beat_index: u64,
    beat_accumulator: f32,
    accent_history: [(u64, f32); 24],
    accent_write: usize,
    accent_count: usize,
    meter: u8,
    downbeat_residue: u8,
    downbeat_confidence: f32,
}

impl BeatTracker {
    /// Creates a beat tracker.
    #[must_use]
    pub fn new(dt: f32, min_bpm: f32, max_bpm: f32, history_seconds: f32) -> Self {
        let capacity = (history_seconds / dt).ceil().max(128.0) as usize;
        let minimum_lag = (60.0 / max_bpm / dt).floor().max(1.0) as usize;
        let maximum_lag = (60.0 / min_bpm / dt).ceil().max(minimum_lag as f32) as usize;
        Self {
            dt,
            min_bpm,
            max_bpm,
            history: vec![0.0; capacity],
            history_write: 0,
            history_count: 0,
            scores: vec![0.0; maximum_lag - minimum_lag + 1],
            frames_until_update: 0,
            update_interval: (0.25 / dt).round().max(1.0) as usize,
            bpm: 120.0_f32.clamp(min_bpm, max_bpm),
            confidence: 0.0,
            phase: 0.0,
            beat_index: 0,
            beat_accumulator: 0.0,
            accent_history: [(0, 0.0); 24],
            accent_write: 0,
            accent_count: 0,
            meter: 4,
            downbeat_residue: 0,
            downbeat_confidence: 0.0,
        }
    }

    /// Advances the tracker with a non-negative onset drive.
    pub fn update(&mut self, onset_drive: f32) -> BeatFrame {
        let drive = onset_drive.clamp(0.0, 1.5);
        self.history[self.history_write] = drive;
        self.history_write = (self.history_write + 1) % self.history.len();
        self.history_count = (self.history_count + 1).min(self.history.len());
        self.beat_accumulator = self.beat_accumulator.max(drive);
        if drive < 0.012 {
            self.confidence *= (-self.dt / 3.0).exp();
            self.downbeat_confidence *= (-self.dt / 4.0).exp();
        }

        if self.frames_until_update == 0 {
            self.reestimate_tempo_and_phase();
            self.frames_until_update = self.update_interval;
        }
        self.frames_until_update = self.frames_until_update.saturating_sub(1);

        let previous_phase = self.phase;
        self.phase = wrap01(self.phase + self.dt * self.bpm / 60.0);
        let free_running_wrap = self.phase < previous_phase;

        if drive > 0.15 {
            let error = wrap_signed_cycle(self.phase);
            if self.confidence < 0.12 && drive > 0.45 {
                self.phase = 0.0;
            } else if error.abs() < 0.34 {
                let correction = (error * (0.10 + 0.32 * drive.min(1.0))).clamp(-0.075, 0.075);
                self.phase = wrap01(self.phase - correction);
                self.bpm =
                    (self.bpm * (1.0 - 0.010 * error * drive)).clamp(self.min_bpm, self.max_bpm);
            }
        }

        // A phase correction commonly moves the oscillator slightly backwards.
        // That is not a new beat. Only accept a correction-induced wrap when the
        // oscillator was already near the end of the cycle and lands near zero.
        let correction_wrap = !free_running_wrap && previous_phase > 0.72 && self.phase < 0.28;
        let oscillator_wrap = free_running_wrap || correction_wrap;
        if oscillator_wrap {
            self.record_completed_beat();
            self.beat_index = self.beat_index.wrapping_add(1);
            self.beat_accumulator = drive;
            self.update_meter();
        }

        let beat_in_bar = self.current_beat_in_bar();
        // The oscillator may free-run while the stream is silent or before a
        // tempo has locked. Keep its phase internally, but do not expose those
        // speculative wraps as lighting events.
        let beat_event = oscillator_wrap && (self.confidence > 0.22 || drive > 0.62);
        let downbeat_event = beat_event
            && beat_in_bar == 0
            && (self.downbeat_confidence > 0.15 || self.confidence > 0.60);
        let bar_phase = (beat_in_bar as f32 + self.phase) / self.meter.max(1) as f32;
        BeatFrame {
            bpm: self.bpm,
            phase: self.phase,
            confidence: self.confidence,
            beat_in_bar,
            meter: self.meter,
            bar_phase: wrap01(bar_phase),
            downbeat_confidence: self.downbeat_confidence,
            beat_event,
            downbeat_event,
            beat_index: self.beat_index,
        }
    }

    fn reestimate_tempo_and_phase(&mut self) {
        let minimum_required = (2.5 / self.dt) as usize;
        if self.history_count < minimum_required.min(self.history.len()) {
            self.confidence *= 0.98;
            return;
        }
        let minimum_lag = (60.0 / self.max_bpm / self.dt).floor().max(1.0) as usize;
        let maximum_lag = (60.0 / self.min_bpm / self.dt).ceil() as usize;
        let maximum_lag = maximum_lag.min(self.history_count.saturating_sub(2));
        if maximum_lag <= minimum_lag {
            return;
        }

        let mean = (0..self.history_count)
            .map(|index| self.history_at(index))
            .sum::<f32>()
            / self.history_count as f32;
        let mut best_score = f32::NEG_INFINITY;
        let mut second_score = f32::NEG_INFINITY;
        let mut best_lag = minimum_lag;
        let mut score_sum = 0.0;
        let mut score_count = 0usize;

        for lag in minimum_lag..=maximum_lag {
            let mut numerator = 0.0;
            let mut left_energy = 0.0;
            let mut right_energy = 0.0;
            for index in lag..self.history_count {
                let left = self.history_at(index) - mean;
                let right = self.history_at(index - lag) - mean;
                numerator = left.mul_add(right, numerator);
                left_energy = left.mul_add(left, left_energy);
                right_energy = right.mul_add(right, right_energy);
            }
            let correlation = if left_energy > 1.0e-9 && right_energy > 1.0e-9 {
                numerator / (left_energy * right_energy).sqrt()
            } else {
                0.0
            };
            let bpm = 60.0 / (lag as f32 * self.dt);
            let log_ratio = (bpm / self.bpm.max(1.0)).ln().abs();
            let continuity = (-0.5 * (log_ratio / 0.24).powi(2)).exp();
            let musical_prior = if (72.0..=176.0).contains(&bpm) {
                1.0
            } else {
                0.93
            };
            let score = correlation * (0.78 + 0.22 * continuity) * musical_prior;
            let score_index = lag - minimum_lag;
            if score_index < self.scores.len() {
                self.scores[score_index] = score;
            }
            score_sum += score.max(0.0);
            score_count += 1;
            if score > best_score {
                second_score = best_score;
                best_score = score;
                best_lag = lag;
            } else if score > second_score {
                second_score = score;
            }
        }

        if !best_score.is_finite() || best_score <= 0.0 {
            self.confidence *= 0.96;
            return;
        }
        let mean_score = score_sum / score_count.max(1) as f32;
        let separation = ((best_score - mean_score) / (best_score.abs() + 1.0e-6)).clamp(0.0, 1.0);
        // Adjacent lags belong to the same broad autocorrelation peak. Compare
        // against the best independent peak outside a small neighborhood so a
        // wide but unambiguous tempo peak is not assigned artificially low confidence.
        let exclusion = ((best_lag as f32 * 0.04).ceil() as usize).max(2);
        let mut independent_second = f32::NEG_INFINITY;
        for lag in minimum_lag..=maximum_lag {
            if lag.abs_diff(best_lag) <= exclusion {
                continue;
            }
            if let Some(&score) = self.scores.get(lag - minimum_lag) {
                independent_second = independent_second.max(score);
            }
        }
        let competing_score = if independent_second.is_finite() {
            independent_second
        } else {
            second_score
        };
        let peak_separation =
            ((best_score - competing_score.max(0.0)) / (best_score.abs() + 1.0e-6)).clamp(0.0, 1.0);
        let activity = (mean * 8.0).clamp(0.0, 1.0);
        let new_confidence = (0.70 * separation + 0.30 * peak_separation) * activity;
        self.confidence = lerp(self.confidence, new_confidence, 0.30).clamp(0.0, 1.0);

        let candidate_bpm = 60.0 / (best_lag as f32 * self.dt);
        let tempo_blend = (0.06 + 0.30 * self.confidence).clamp(0.04, 0.36);
        self.bpm = lerp(self.bpm, candidate_bpm, tempo_blend).clamp(self.min_bpm, self.max_bpm);

        let search_beats = (self.history_count / best_lag).min(16);
        if search_beats >= 3 {
            let mut best_offset = 0usize;
            let mut best_phase_score = f32::NEG_INFINITY;
            for offset in 0..best_lag {
                let mut score = 0.0;
                for beat in 0..search_beats {
                    let distance = offset + beat * best_lag;
                    if distance >= self.history_count {
                        break;
                    }
                    let index = self.history_count - 1 - distance;
                    let recency = 1.0 - 0.55 * beat as f32 / search_beats as f32;
                    score += self.history_at(index) * recency;
                }
                if score > best_phase_score {
                    best_phase_score = score;
                    best_offset = offset;
                }
            }
            let measured_phase = best_offset as f32 / best_lag as f32;
            let phase_error = wrap_signed_cycle(measured_phase - self.phase);
            let phase_correction =
                (phase_error * (0.08 + 0.35 * self.confidence)).clamp(-0.090, 0.090);
            self.phase = wrap01(self.phase + phase_correction);
        }
    }

    fn history_at(&self, chronological_index: usize) -> f32 {
        let start =
            (self.history_write + self.history.len() - self.history_count) % self.history.len();
        self.history[(start + chronological_index) % self.history.len()]
    }

    fn record_completed_beat(&mut self) {
        self.accent_history[self.accent_write] = (self.beat_index, self.beat_accumulator);
        self.accent_write = (self.accent_write + 1) % self.accent_history.len();
        self.accent_count = (self.accent_count + 1).min(self.accent_history.len());
    }

    fn update_meter(&mut self) {
        if self.accent_count < 8 {
            return;
        }
        let mut overall_best = f32::NEG_INFINITY;
        let mut runner_up = f32::NEG_INFINITY;
        let mut chosen_meter = self.meter;
        let mut chosen_residue = self.downbeat_residue;
        for meter in [3_u8, 4_u8] {
            for residue in 0..meter {
                let mut downbeat_sum = 0.0;
                let mut other_sum = 0.0;
                let mut downbeat_count = 0.0;
                let mut other_count = 0.0;
                for chronological in 0..self.accent_count {
                    let index = (self.accent_write + self.accent_history.len() - self.accent_count
                        + chronological)
                        % self.accent_history.len();
                    let (beat_index, accent) = self.accent_history[index];
                    let recency = 0.55 + 0.45 * chronological as f32 / self.accent_count as f32;
                    if beat_index % u64::from(meter) == u64::from(residue) {
                        downbeat_sum += accent * recency;
                        downbeat_count += recency;
                    } else {
                        other_sum += accent * recency;
                        other_count += recency;
                    }
                }
                let downbeat_mean = downbeat_sum / downbeat_count.max(1.0e-6);
                let other_mean = other_sum / other_count.max(1.0e-6);
                let score = downbeat_mean - 0.55 * other_mean;
                if score > overall_best {
                    runner_up = overall_best;
                    overall_best = score;
                    chosen_meter = meter;
                    chosen_residue = residue;
                } else if score > runner_up {
                    runner_up = score;
                }
            }
        }
        if overall_best.is_finite() {
            self.meter = chosen_meter;
            self.downbeat_residue = chosen_residue;
            let confidence =
                ((overall_best - runner_up.max(-1.0)) / (overall_best.abs() + 0.2)).clamp(0.0, 1.0);
            self.downbeat_confidence = lerp(self.downbeat_confidence, confidence, 0.18);
        }
    }

    fn current_beat_in_bar(&self) -> u8 {
        let meter = u64::from(self.meter.max(1));
        ((self.beat_index + meter - u64::from(self.downbeat_residue)) % meter) as u8
    }
}
