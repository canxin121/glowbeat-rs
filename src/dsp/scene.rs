//! Online multi-timescale scene, buildup, and drop inference.

use crate::math::{cosine_similarity, smoothing_alpha, smoothstep};
use crate::types::{SceneKind, NUM_BANDS, NUM_CHROMA};

/// Scene-level features for one hop.
#[derive(Clone, Copy, Debug)]
pub struct SceneFrame {
    /// Energy trend in `[-1, 1]`.
    pub energy_trend: f32,
    /// Buildup likelihood.
    pub buildup: f32,
    /// Confirmed drop pulse.
    pub drop: f32,
    /// Structural novelty.
    pub novelty: f32,
    /// Coarse scene kind.
    pub scene: SceneKind,
}

impl Default for SceneFrame {
    fn default() -> Self {
        Self {
            energy_trend: 0.0,
            buildup: 0.0,
            drop: 0.0,
            novelty: 0.0,
            scene: SceneKind::Silence,
        }
    }
}

/// Causal scene observer using energy, timbre, and event trends.
///
/// Scene labels are intentionally slower than beat/onset features. Candidate
/// hysteresis and minimum dwell times prevent a threshold from changing the
/// complete palette/spatial material dozens of times per second.
#[derive(Clone, Debug)]
pub struct SceneTracker {
    dt: f32,
    fast_energy: f32,
    slow_energy: f32,
    very_slow_energy: f32,
    onset_density: f32,
    reference_bands: [f32; NUM_BANDS],
    reference_chroma: [f32; NUM_CHROMA],
    buildup: f32,
    drop: f32,
    previous_buildup: f32,
    scene: SceneKind,
    candidate: SceneKind,
    candidate_frames: usize,
    scene_frames: usize,
    reference_ready: bool,
}

impl SceneTracker {
    /// Creates a scene observer.
    #[must_use]
    pub fn new(dt: f32) -> Self {
        Self {
            dt,
            fast_energy: 0.0,
            slow_energy: 0.0,
            very_slow_energy: 0.0,
            onset_density: 0.0,
            reference_bands: [0.0; NUM_BANDS],
            reference_chroma: [0.0; NUM_CHROMA],
            buildup: 0.0,
            drop: 0.0,
            previous_buildup: 0.0,
            scene: SceneKind::Silence,
            candidate: SceneKind::Silence,
            candidate_frames: 0,
            scene_frames: 0,
            reference_ready: false,
        }
    }

    /// Updates scene state.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        loudness: f32,
        spectral_energy: f32,
        bands: &[f32; NUM_BANDS],
        chroma: &[f32; NUM_CHROMA],
        onset_activity: f32,
        impact: f32,
        downbeat_event: bool,
        centroid: f32,
    ) -> SceneFrame {
        let energy = (0.72 * loudness + 0.28 * spectral_energy).clamp(0.0, 1.0);
        self.fast_energy += smoothing_alpha(self.dt, 0.10) * (energy - self.fast_energy);
        self.slow_energy += smoothing_alpha(self.dt, 1.10) * (energy - self.slow_energy);
        self.very_slow_energy += smoothing_alpha(self.dt, 5.0) * (energy - self.very_slow_energy);
        self.onset_density +=
            smoothing_alpha(self.dt, 0.85) * (onset_activity.clamp(0.0, 1.0) - self.onset_density);

        // Initial zero references otherwise report novelty=1 and force a noisy
        // Transition/Sparkle scene as soon as the first FFT frame arrives.
        if !self.reference_ready && energy > 0.018 {
            self.reference_bands.copy_from_slice(bands);
            self.reference_chroma.copy_from_slice(chroma);
            self.reference_ready = true;
        }

        let current_band_energy = bands.iter().map(|value| value * value).sum::<f32>();
        let current_chroma_energy = chroma.iter().map(|value| value * value).sum::<f32>();
        let band_novelty = if self.reference_ready && current_band_energy > 1.0e-5 {
            1.0 - cosine_similarity(bands, &self.reference_bands).max(0.0)
        } else {
            0.0
        };
        let chroma_novelty = if self.reference_ready && current_chroma_energy > 1.0e-5 {
            1.0 - cosine_similarity(chroma, &self.reference_chroma).max(0.0)
        } else {
            0.0
        };
        let energy_jump = (self.fast_energy - self.slow_energy).abs();
        let novelty = if energy < 0.025 {
            0.0
        } else {
            (0.54 * band_novelty + 0.14 * chroma_novelty + 0.58 * energy_jump).clamp(0.0, 1.0)
        };

        if self.reference_ready {
            let reference_alpha = smoothing_alpha(self.dt, if novelty > 0.40 { 2.6 } else { 1.0 });
            for (reference, current) in self.reference_bands.iter_mut().zip(bands) {
                *reference += reference_alpha * (*current - *reference);
            }
            for (reference, current) in self.reference_chroma.iter_mut().zip(chroma) {
                *reference += reference_alpha * (*current - *reference);
            }
        }

        let trend_raw = (self.fast_energy - self.very_slow_energy) * 3.0;
        let energy_trend = trend_raw.clamp(-1.0, 1.0);
        let build_target = (0.55 * smoothstep(0.025, 0.28, self.fast_energy - self.slow_energy)
            + 0.25 * self.onset_density
            + 0.20 * smoothstep(0.28, 0.78, centroid))
        .clamp(0.0, 1.0);
        self.previous_buildup = self.buildup;
        let buildup_tau = if build_target > self.buildup {
            0.52
        } else {
            1.00
        };
        self.buildup += smoothing_alpha(self.dt, buildup_tau) * (build_target - self.buildup);

        let release = self.previous_buildup.max(self.buildup);
        let confirmed_drop = release > 0.40
            && impact > 0.36
            && (downbeat_event || impact > 0.78)
            && self.fast_energy > self.slow_energy + 0.040;
        if confirmed_drop {
            self.drop = (0.52 + 0.42 * impact).clamp(0.0, 1.0);
            self.buildup *= 0.25;
        } else {
            self.drop *= (-self.dt / 0.52).exp();
        }

        let proposed = self.classify_scene(energy, novelty);
        self.update_scene_state(proposed, confirmed_drop);

        SceneFrame {
            energy_trend,
            buildup: self.buildup.clamp(0.0, 1.0),
            drop: self.drop.clamp(0.0, 1.0),
            novelty,
            scene: self.scene,
        }
    }

    fn classify_scene(&self, energy: f32, novelty: f32) -> SceneKind {
        let silence_limit = if self.scene == SceneKind::Silence {
            0.036
        } else {
            0.020
        };
        let build_limit = if self.scene == SceneKind::Build {
            0.39
        } else {
            0.52
        };
        let novelty_limit = if self.scene == SceneKind::Transition {
            0.34
        } else {
            0.56
        };
        let intense_energy = if self.scene == SceneKind::Intense {
            0.58
        } else {
            0.69
        };
        let intense_density = if self.scene == SceneKind::Intense {
            0.52
        } else {
            0.66
        };
        let calm_energy = if self.scene == SceneKind::Calm {
            0.30
        } else {
            0.21
        };
        let calm_density = if self.scene == SceneKind::Calm {
            0.30
        } else {
            0.20
        };

        if energy < silence_limit {
            SceneKind::Silence
        } else if self.drop > 0.20 {
            SceneKind::Drop
        } else if self.buildup > build_limit {
            SceneKind::Build
        } else if novelty > novelty_limit {
            SceneKind::Transition
        } else if energy > intense_energy || self.onset_density > intense_density {
            SceneKind::Intense
        } else if energy < calm_energy && self.onset_density < calm_density {
            SceneKind::Calm
        } else {
            SceneKind::Groove
        }
    }

    fn update_scene_state(&mut self, proposed: SceneKind, confirmed_drop: bool) {
        self.scene_frames = self.scene_frames.saturating_add(1);

        // A confirmed drop is an observed event rather than a statistical scene
        // guess, so it may switch immediately. All other labels require dwell.
        if confirmed_drop || (proposed == SceneKind::Drop && self.drop > 0.42) {
            self.scene = SceneKind::Drop;
            self.candidate = SceneKind::Drop;
            self.candidate_frames = 0;
            self.scene_frames = 0;
            return;
        }

        if proposed == self.scene {
            self.candidate = proposed;
            self.candidate_frames = 0;
            return;
        }

        if proposed == self.candidate {
            self.candidate_frames = self.candidate_frames.saturating_add(1);
        } else {
            self.candidate = proposed;
            self.candidate_frames = 1;
        }

        let mut required_seconds: f32 = match proposed {
            SceneKind::Silence => 0.24,
            SceneKind::Transition => 0.18,
            SceneKind::Build => 0.26,
            SceneKind::Intense => 0.30,
            SceneKind::Calm | SceneKind::Groove => 0.34,
            SceneKind::Drop => 0.0,
        };
        // Music should become visibly alive quickly after silence, while the
        // longer dwell rules continue to protect steady-state scene changes.
        if self.scene == SceneKind::Silence && proposed != SceneKind::Silence {
            required_seconds = required_seconds.min(0.14);
        }
        let minimum_scene_seconds: f32 = match self.scene {
            SceneKind::Silence => 0.10,
            SceneKind::Drop => 0.42,
            SceneKind::Transition => 0.38,
            _ => 0.55,
        };
        let required_frames = (required_seconds / self.dt.max(1.0e-6)).ceil().max(1.0) as usize;
        let minimum_scene_frames = (minimum_scene_seconds / self.dt.max(1.0e-6))
            .ceil()
            .max(1.0) as usize;

        if self.candidate_frames >= required_frames && self.scene_frames >= minimum_scene_frames {
            self.scene = proposed;
            self.candidate_frames = 0;
            self.scene_frames = 0;
        }
    }
}
