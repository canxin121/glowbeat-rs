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
            smoothing_alpha(self.dt, 0.75) * (onset_activity.clamp(0.0, 1.0) - self.onset_density);

        let band_novelty = 1.0 - cosine_similarity(bands, &self.reference_bands).max(0.0);
        let chroma_novelty = 1.0 - cosine_similarity(chroma, &self.reference_chroma).max(0.0);
        let energy_jump = (self.fast_energy - self.slow_energy).abs();
        let novelty =
            (0.58 * band_novelty + 0.17 * chroma_novelty + 0.65 * energy_jump).clamp(0.0, 1.0);
        let reference_alpha = smoothing_alpha(self.dt, if novelty > 0.35 { 3.0 } else { 1.2 });
        for (reference, current) in self.reference_bands.iter_mut().zip(bands) {
            *reference += reference_alpha * (*current - *reference);
        }
        for (reference, current) in self.reference_chroma.iter_mut().zip(chroma) {
            *reference += reference_alpha * (*current - *reference);
        }

        let trend_raw = (self.fast_energy - self.very_slow_energy) * 3.0;
        let energy_trend = trend_raw.clamp(-1.0, 1.0);
        let build_target = (0.55 * smoothstep(0.02, 0.28, self.fast_energy - self.slow_energy)
            + 0.25 * self.onset_density
            + 0.20 * smoothstep(0.25, 0.75, centroid))
        .clamp(0.0, 1.0);
        self.previous_buildup = self.buildup;
        let buildup_tau = if build_target > self.buildup {
            0.45
        } else {
            0.85
        };
        self.buildup += smoothing_alpha(self.dt, buildup_tau) * (build_target - self.buildup);

        let release = self.previous_buildup.max(self.buildup);
        let confirmed_drop = release > 0.38
            && impact > 0.32
            && (downbeat_event || impact > 0.72)
            && self.fast_energy > self.slow_energy + 0.035;
        if confirmed_drop {
            self.drop = (0.55 + 0.45 * impact).clamp(0.0, 1.0);
            self.buildup *= 0.25;
        } else {
            self.drop *= (-self.dt / 0.42).exp();
        }

        self.scene = if energy < 0.025 {
            SceneKind::Silence
        } else if self.drop > 0.20 {
            SceneKind::Drop
        } else if self.buildup > 0.48 {
            SceneKind::Build
        } else if novelty > 0.48 {
            SceneKind::Transition
        } else if energy > 0.66 || self.onset_density > 0.62 {
            SceneKind::Intense
        } else if energy < 0.24 && self.onset_density < 0.22 {
            SceneKind::Calm
        } else {
            SceneKind::Groove
        };

        SceneFrame {
            energy_trend,
            buildup: self.buildup.clamp(0.0, 1.0),
            drop: self.drop.clamp(0.0, 1.0),
            novelty,
            scene: self.scene,
        }
    }
}
