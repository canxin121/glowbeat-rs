//! Perceptual, layered music-to-light rendering.
//!
//! The director deliberately separates musical responsibilities:
//! - harmony selects a stable palette;
//! - beat and bar phase provide continuous motion;
//! - low/mid/high transients produce different visual materials;
//! - scene state controls density, scale, and contrast;
//! - the full-track path can provide an optional [`RenderContext`] containing
//!   anticipatory and narrative overrides.

use crate::config::{OutputMode, RenderConfig};
use crate::math::{gamma_encode, hash_unit, lerp, oklch_to_linear_srgb, smoothstep, wrap01};
use crate::types::{LightPoint, MusicFrame, Rgba, SceneKind, SpatialMode, NUM_CHROMA};

/// Optional high-level direction supplied by the full-track score player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderContext {
    /// Optional palette hue override in turns.
    pub palette_hue: Option<f32>,
    /// Additional cyclic hue shift in turns.
    pub palette_shift: f32,
    /// Multiplicative scene intensity.
    pub intensity: f32,
    /// Optional spatial pattern override.
    pub spatial_mode: Option<SpatialMode>,
    /// Stable seed used to keep a section's visual motif coherent.
    pub motif_seed: u32,
    /// Anticipatory buildup amount in `[0, 1]`.
    pub anticipation: f32,
    /// Artistic blackout amount in `[0, 1]`.
    pub blackout: f32,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            palette_hue: None,
            palette_shift: 0.0,
            intensity: 1.0,
            spatial_mode: None,
            motif_seed: 0,
            anticipation: 0.0,
            blackout: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TimedPulse {
    strength: f32,
    sample_index: u64,
}

impl TimedPulse {
    fn value_at(self, sample_index: u64, sample_rate: u32, decay_seconds: f32) -> f32 {
        if self.strength <= 0.0 {
            return 0.0;
        }
        if sample_index < self.sample_index {
            return 0.0;
        }
        let elapsed_samples = sample_index - self.sample_index;
        let elapsed = elapsed_samples as f64 / sample_rate.max(1) as f64;
        (f64::from(self.strength) * (-elapsed / f64::from(decay_seconds)).exp()) as f32
    }

    fn ingest(&mut self, sample_index: u64, strength: f32, sample_rate: u32, decay_seconds: f32) {
        let retained = self.value_at(sample_index, sample_rate, decay_seconds);
        self.strength = retained.max(strength.clamp(0.0, 1.5));
        self.sample_index = sample_index;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PulseValues {
    low: f32,
    mid: f32,
    high: f32,
    impact: f32,
    drop: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct PulseBank {
    low: TimedPulse,
    mid: TimedPulse,
    high: TimedPulse,
    impact: TimedPulse,
    drop: TimedPulse,
}

impl PulseBank {
    fn ingest(&mut self, frame: &MusicFrame, sample_rate: u32) {
        self.low
            .ingest(frame.sample_index, frame.onset_low, sample_rate, 0.215);
        self.mid
            .ingest(frame.sample_index, frame.onset_mid, sample_rate, 0.115);
        self.high
            .ingest(frame.sample_index, frame.onset_high, sample_rate, 0.070);
        let downbeat = if frame.downbeat_event {
            0.32 + 0.38 * frame.downbeat_confidence
        } else {
            0.0
        };
        self.impact.ingest(
            frame.sample_index,
            frame.impact.max(downbeat),
            sample_rate,
            0.145,
        );
        self.drop
            .ingest(frame.sample_index, frame.drop, sample_rate, 0.480);
    }

    fn values_at(&self, sample_index: u64, sample_rate: u32) -> PulseValues {
        PulseValues {
            low: self.low.value_at(sample_index, sample_rate, 0.215),
            mid: self.mid.value_at(sample_index, sample_rate, 0.115),
            high: self.high.value_at(sample_index, sample_rate, 0.070),
            impact: self.impact.value_at(sample_index, sample_rate, 0.145),
            drop: self.drop.value_at(sample_index, sample_rate, 0.480),
        }
    }
}

/// Stateful renderer shared by real-time and full-track playback.
///
/// The constructor preallocates per-light state. Calling [`Self::render`] with
/// no more than `light_capacity` lights performs no heap allocation.
#[derive(Clone, Debug)]
pub struct LightDirector {
    config: RenderConfig,
    sample_rate: u32,
    latest: MusicFrame,
    palette_hue: f32,
    palette_confidence: f32,
    pulses: PulseBank,
    previous_linear: Vec<[f32; 4]>,
}

impl LightDirector {
    /// Creates a director with reusable state for `light_capacity` outputs.
    pub fn new(
        config: RenderConfig,
        sample_rate: u32,
        light_capacity: usize,
    ) -> Result<Self, crate::config::ConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            sample_rate: sample_rate.max(1),
            latest: MusicFrame::default(),
            palette_hue: 0.72,
            palette_confidence: 0.0,
            pulses: PulseBank::default(),
            previous_linear: vec![[0.0; 4]; light_capacity],
        })
    }

    /// Returns the renderer configuration.
    #[must_use]
    pub const fn config(&self) -> &RenderConfig {
        &self.config
    }

    /// Number of lights with preallocated temporal state.
    #[must_use]
    pub fn light_capacity(&self) -> usize {
        self.previous_linear.len()
    }

    /// Ensures temporal state exists for at least `capacity` lights.
    ///
    /// Existing light history is retained. New lights begin from black.
    pub fn reserve_lights(&mut self, capacity: usize) {
        if capacity > self.previous_linear.len() {
            self.previous_linear.resize(capacity, [0.0; 4]);
        }
    }

    /// Clears palette, pulse, and smoothing history while retaining capacity.
    pub fn reset(&mut self) {
        let capacity = self.previous_linear.len();
        *self = Self::new(self.config.clone(), self.sample_rate, capacity)
            .expect("existing renderer configuration remains valid");
    }

    /// Ingests an analysis frame and updates persistent palette/event state.
    pub fn ingest_frame(&mut self, frame: &MusicFrame) {
        self.pulses.ingest(frame, self.sample_rate);
        let (hue, concentration) = harmonic_hue(&frame.chroma);
        let confidence = (frame.tonal_confidence * concentration).clamp(0.0, 1.0);
        if confidence > 0.035 {
            let difference = signed_hue_difference(hue, self.palette_hue);
            // Harmony should move slowly enough to feel intentional, but become
            // more responsive at confident structural boundaries.
            let boundary_boost = 1.0 + 1.6 * frame.novelty;
            let alpha = (0.018 + 0.095 * confidence) * boundary_boost;
            self.palette_hue = wrap01(self.palette_hue + difference * alpha.min(0.35));
        }
        self.palette_confidence += 0.08 * (confidence - self.palette_confidence);
        self.latest = frame.clone();
    }

    /// Renders the latest music state into `output` at an absolute audio sample.
    pub fn render(
        &mut self,
        sample_index: u64,
        lights: &[LightPoint],
        output: &mut [Rgba],
    ) -> Result<(), RenderError> {
        self.render_with_context(sample_index, lights, output, &RenderContext::default())
    }

    /// Renders with high-level full-track direction.
    pub fn render_with_context(
        &mut self,
        sample_index: u64,
        lights: &[LightPoint],
        output: &mut [Rgba],
        context: &RenderContext,
    ) -> Result<(), RenderError> {
        if lights.len() != output.len() {
            return Err(RenderError::LengthMismatch);
        }
        if lights.len() > self.previous_linear.len() {
            return Err(RenderError::CapacityExceeded {
                requested: lights.len(),
                capacity: self.previous_linear.len(),
            });
        }

        let pulses = self.pulses.values_at(sample_index, self.sample_rate);

        // `latest` is initialized to a valid silent frame, so the same path
        // safely handles rendering before the first analysis hop.
        let frame = &self.latest;
        let elapsed = (sample_index as f64 - frame.sample_index as f64) / self.sample_rate as f64;
        let elapsed = elapsed as f32;
        let beat_phase = wrap01(frame.beat_phase + elapsed * frame.bpm.max(1.0) / 60.0);
        let beats_elapsed = elapsed * frame.bpm.max(1.0) / 60.0;
        let meter = i64::from(frame.meter.max(1));
        let advanced_beats = beats_elapsed.floor() as i64;
        let beat_in_bar = (i64::from(frame.beat_in_bar) + advanced_beats).rem_euclid(meter) as u8;
        let bar_phase = wrap01(
            frame.bar_phase + elapsed * frame.bpm.max(1.0) / (60.0 * frame.meter.max(1) as f32),
        );

        let context_hue = context.palette_hue.unwrap_or(self.palette_hue);
        let hue = wrap01(context_hue + context.palette_shift + self.config.hue_offset);
        let scene = frame.scene;
        let mode = context
            .spatial_mode
            .unwrap_or_else(|| default_spatial_mode(scene));
        let scene_params = scene_parameters(scene, frame, context);

        let tonal = self.palette_confidence.clamp(0.0, 1.0);
        let saturation =
            (self.config.saturation * lerp(0.74, 1.06, tonal) * scene_params.saturation)
                .clamp(0.0, 1.35);
        let chroma = (0.08 + 0.17 * saturation).clamp(0.0, 0.27);
        let base_rgb = oklch_to_linear_srgb(scene_params.lightness, chroma, hue);
        let secondary_hue = wrap01(hue + scene_params.secondary_offset);
        let secondary_rgb = oklch_to_linear_srgb(
            (scene_params.lightness + 0.04).clamp(0.0, 0.94),
            chroma * 0.92,
            secondary_hue,
        );
        let accent_rgb = oklch_to_linear_srgb(
            (scene_params.lightness + 0.16).clamp(0.0, 0.96),
            (chroma * 1.08).min(0.30),
            wrap01(hue + scene_params.accent_offset),
        );
        let high_rgb = oklch_to_linear_srgb(
            (scene_params.lightness + 0.24).clamp(0.0, 0.98),
            0.025 + 0.055 * saturation,
            wrap01(hue + 0.065),
        );

        let beat_shape = beat_envelope(beat_phase, frame.beat_confidence);
        let offbeat = beat_envelope(wrap01(beat_phase + 0.5), frame.beat_confidence) * 0.42;
        let bar_accent = if beat_in_bar == 0 { 1.0 } else { 0.40 };
        let groove =
            (beat_shape * (0.60 + 0.40 * bar_accent) + offbeat) * self.config.beat_strength;
        let anticipation = context.anticipation.clamp(0.0, 1.0);
        let blackout = context.blackout.clamp(0.0, 1.0);
        let global_level = (scene_params.base_level
            + 0.34 * frame.loudness
            + 0.14 * groove
            + 0.09 * frame.energy_trend.max(0.0)
            + 0.10 * anticipation)
            * context.intensity.max(0.0)
            * (1.0 - blackout);

        // Use f64 for absolute sample-clock conversion so sparkle timing stays
        // stable on long recordings after f32 can no longer represent every
        // individual sample index.
        let elapsed_time = sample_index as f64 / self.sample_rate as f64;
        let sparkle_bucket = (elapsed_time * 18.0).floor() as u32;
        for (index, (point, destination)) in lights.iter().zip(output.iter_mut()).enumerate() {
            let field = spatial_field(
                mode,
                point,
                bar_phase,
                beat_phase,
                beat_in_bar,
                frame,
                context.motif_seed,
                sparkle_bucket,
            );
            let spatial_mix = lerp(1.0, field, self.config.spatial_strength.clamp(0.0, 1.0));

            let mut rgb = scale_rgb(base_rgb, global_level * (0.72 + 0.28 * spatial_mix));
            add_scaled_rgb(
                &mut rgb,
                secondary_rgb,
                (0.07 + 0.19 * groove) * (1.0 - 0.45 * anticipation) * spatial_mix,
            );

            let low_layer = pulses.low
                * self.config.accent_strength
                * (0.58 + 0.42 * low_spatial(point, beat_phase));
            let mid_layer = pulses.mid
                * self.config.accent_strength
                * (0.62 + 0.38 * mirror_spatial(point, bar_phase));
            let high_random = hash_unit(point.id ^ sparkle_bucket.wrapping_mul(0x9e37_79b9));
            let high_gate = smoothstep(0.62, 0.98, high_random + pulses.high * 0.65);
            let high_layer = pulses.high
                * self.config.accent_strength
                * high_gate
                * (0.42 + 0.58 * frame.spectral_centroid);
            let impact_layer =
                pulses.impact * self.config.accent_strength * (0.60 + 0.40 * spatial_mix);
            let drop_layer = pulses.drop
                * self.config.accent_strength
                * radial_release(point, pulses.drop, beat_phase);

            add_scaled_rgb(&mut rgb, accent_rgb, 0.34 * low_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.52 * mid_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.32 * high_layer);
            add_scaled_rgb(&mut rgb, accent_rgb, 0.45 * impact_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.72 * drop_layer);

            // A buildup becomes visually denser and more focused without
            // committing to an unconfirmed future event.
            let focus = anticipation.max(frame.buildup);
            let center = (1.0 - (point.x * point.x + point.y * point.y).sqrt()).clamp(0.0, 1.0);
            let focus_gain = lerp(1.0, 0.68 + 0.72 * center, focus * 0.55);
            for channel in &mut rgb {
                *channel *= focus_gain;
            }

            let max_channel = rgb[0].max(rgb[1]).max(rgb[2]);
            if max_channel > 1.0 {
                let compression = 1.0 / (0.72 + 0.28 * max_channel);
                for channel in &mut rgb {
                    *channel *= compression;
                }
            }
            for channel in &mut rgb {
                *channel = (*channel * self.config.max_brightness).clamp(0.0, 1.0);
            }

            let mut rgba_linear = match self.config.output_mode {
                OutputMode::Alpha => [rgb[0], rgb[1], rgb[2], 1.0],
                OutputMode::Amber => rgb_to_rgba_amber(rgb, self.config.amber_mix),
            };
            limit_luminance_step(
                &mut rgba_linear,
                &self.previous_linear[index],
                self.config.max_luma_step,
            );
            self.previous_linear[index] = rgba_linear;
            *destination = Rgba::new(
                gamma_encode(rgba_linear[0], self.config.output_gamma),
                gamma_encode(rgba_linear[1], self.config.output_gamma),
                gamma_encode(rgba_linear[2], self.config.output_gamma),
                match self.config.output_mode {
                    OutputMode::Alpha => rgba_linear[3],
                    OutputMode::Amber => gamma_encode(rgba_linear[3], self.config.output_gamma),
                },
            )
            .clamped();
        }
        Ok(())
    }
}

/// Rendering error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderError {
    /// The point and output slices have different lengths.
    LengthMismatch,
    /// The director was created for fewer lights than requested.
    CapacityExceeded {
        /// Number of requested lights.
        requested: usize,
        /// Preallocated capacity.
        capacity: usize,
    },
}

impl core::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LengthMismatch => formatter.write_str("light and output lengths differ"),
            Self::CapacityExceeded {
                requested,
                capacity,
            } => write!(
                formatter,
                "requested {requested} lights but director capacity is {capacity}"
            ),
        }
    }
}

impl std::error::Error for RenderError {}

#[derive(Clone, Copy)]
struct SceneParameters {
    base_level: f32,
    lightness: f32,
    saturation: f32,
    secondary_offset: f32,
    accent_offset: f32,
}

fn scene_parameters(
    scene: SceneKind,
    frame: &MusicFrame,
    context: &RenderContext,
) -> SceneParameters {
    let anticipation = context.anticipation.max(frame.buildup).clamp(0.0, 1.0);
    match scene {
        SceneKind::Silence => SceneParameters {
            base_level: 0.012,
            lightness: 0.48,
            saturation: 0.58,
            secondary_offset: 0.075,
            accent_offset: 0.50,
        },
        SceneKind::Calm => SceneParameters {
            base_level: 0.055,
            lightness: 0.58,
            saturation: 0.78,
            secondary_offset: 0.095,
            accent_offset: 0.47,
        },
        SceneKind::Groove => SceneParameters {
            base_level: 0.105,
            lightness: 0.63,
            saturation: 0.93,
            secondary_offset: 0.105,
            accent_offset: 0.42,
        },
        SceneKind::Intense => SceneParameters {
            base_level: 0.17,
            lightness: 0.68,
            saturation: 1.04,
            secondary_offset: 0.085,
            accent_offset: 0.50,
        },
        SceneKind::Build => SceneParameters {
            base_level: lerp(0.095, 0.18, anticipation),
            lightness: lerp(0.61, 0.69, anticipation),
            saturation: 1.02,
            secondary_offset: lerp(0.11, 0.045, anticipation),
            accent_offset: 0.48,
        },
        SceneKind::Drop => SceneParameters {
            base_level: 0.235,
            lightness: 0.73,
            saturation: 1.08,
            secondary_offset: 0.055,
            accent_offset: 0.50,
        },
        SceneKind::Transition => SceneParameters {
            base_level: 0.085,
            lightness: 0.61,
            saturation: 0.88,
            secondary_offset: 0.16,
            accent_offset: 0.38,
        },
    }
}

fn default_spatial_mode(scene: SceneKind) -> SpatialMode {
    match scene {
        SceneKind::Silence => SpatialMode::Uniform,
        SceneKind::Calm => SpatialMode::Mirror,
        SceneKind::Groove => SpatialMode::Wave,
        SceneKind::Intense => SpatialMode::Zones,
        SceneKind::Build => SpatialMode::Spiral,
        SceneKind::Drop => SpatialMode::Radial,
        SceneKind::Transition => SpatialMode::Sparkle,
    }
}

fn harmonic_hue(chroma: &[f32; NUM_CHROMA]) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut total = 0.0;
    for (pitch_class, &weight) in chroma.iter().enumerate() {
        // Circle-of-fifths order makes harmonically related pitch classes
        // visually adjacent. C=0; multiplication by 7 walks fifths modulo 12.
        let fifth_position = (pitch_class * 7) % NUM_CHROMA;
        let angle = core::f32::consts::TAU * fifth_position as f32 / NUM_CHROMA as f32;
        x += weight * angle.cos();
        y += weight * angle.sin();
        total += weight;
    }
    if total <= 1.0e-9 {
        return (0.0, 0.0);
    }
    let hue = wrap01(y.atan2(x) / core::f32::consts::TAU);
    let concentration = (x.mul_add(x, y * y).sqrt() / total).clamp(0.0, 1.0);
    (hue, concentration)
}

fn signed_hue_difference(target: f32, current: f32) -> f32 {
    let difference = wrap01(target - current + 0.5) - 0.5;
    if difference == -0.5 {
        0.5
    } else {
        difference
    }
}

fn beat_envelope(phase: f32, confidence: f32) -> f32 {
    let attack = smoothstep(0.0, 0.035, phase);
    let decay = (-phase / lerp(0.24, 0.115, confidence.clamp(0.0, 1.0))).exp();
    let pulse = if phase < 0.035 {
        attack.max(decay)
    } else {
        decay
    };
    pulse * lerp(0.32, 1.0, confidence.clamp(0.0, 1.0))
}

#[allow(clippy::too_many_arguments)]
fn spatial_field(
    mode: SpatialMode,
    point: &LightPoint,
    bar_phase: f32,
    beat_phase: f32,
    beat_in_bar: u8,
    frame: &MusicFrame,
    seed: u32,
    sparkle_bucket: u32,
) -> f32 {
    let tau = core::f32::consts::TAU;
    match mode {
        SpatialMode::Uniform => 1.0,
        SpatialMode::Wave => {
            let phase = tau * (1.25 * point.x + 0.35 * point.y - 1.75 * bar_phase);
            0.30 + 0.70 * (0.5 + 0.5 * phase.sin()).powi(2)
        }
        SpatialMode::Radial => {
            let radius = (point.x * point.x + point.y * point.y + 0.25 * point.z * point.z).sqrt();
            let ring = wrap01(beat_phase * 1.15 - radius * 0.72);
            0.24 + 0.76 * (-((ring.min(1.0 - ring)) / 0.18).powi(2)).exp()
        }
        SpatialMode::Spiral => {
            let angle = point.y.atan2(point.x) / tau;
            let radius = (point.x * point.x + point.y * point.y).sqrt();
            let phase = tau * (angle + 0.65 * radius - 1.25 * bar_phase);
            0.28 + 0.72 * (0.5 + 0.5 * phase.cos())
        }
        SpatialMode::Zones => {
            let group = ((point.zone.clamp(0.0, 0.999) * 4.0).floor() as u8)
                .wrapping_add((seed & 3) as u8)
                % 4;
            let active = (beat_in_bar + group) % frame.meter.max(1);
            if active == 0 {
                1.0
            } else {
                0.38
            }
        }
        SpatialMode::Mirror => {
            let phase = tau * (point.x.abs() * 1.35 - bar_phase * 1.4);
            0.38 + 0.62 * (0.5 + 0.5 * phase.cos())
        }
        SpatialMode::Sparkle => {
            let random = hash_unit(point.id ^ seed ^ sparkle_bucket.wrapping_mul(0x85eb_ca6b));
            let gate = smoothstep(0.89, 0.995, random + frame.onset_high * 0.28);
            0.28 + 0.72 * gate
        }
    }
}

fn low_spatial(point: &LightPoint, beat_phase: f32) -> f32 {
    let radius = (point.x * point.x + point.y * point.y).sqrt();
    let wave = (core::f32::consts::TAU * (radius * 0.65 - beat_phase)).cos();
    0.5 + 0.5 * wave
}

fn mirror_spatial(point: &LightPoint, bar_phase: f32) -> f32 {
    let wave = (core::f32::consts::TAU * (point.x.abs() - bar_phase)).cos();
    0.5 + 0.5 * wave
}

fn radial_release(point: &LightPoint, pulse: f32, phase: f32) -> f32 {
    let radius = (point.x * point.x + point.y * point.y).sqrt();
    let center = wrap01((1.0 - pulse) * 0.65 + phase * 0.25);
    let distance = (radius - center).abs();
    0.45 + 0.55 * (-(distance / 0.24).powi(2)).exp()
}

fn scale_rgb(rgb: [f32; 3], gain: f32) -> [f32; 3] {
    [rgb[0] * gain, rgb[1] * gain, rgb[2] * gain]
}

fn add_scaled_rgb(target: &mut [f32; 3], rgb: [f32; 3], gain: f32) {
    target[0] += rgb[0] * gain;
    target[1] += rgb[1] * gain;
    target[2] += rgb[2] * gain;
}

fn rgb_to_rgba_amber(rgb: [f32; 3], amber_mix: f32) -> [f32; 4] {
    // Generic decomposition for uncalibrated RGBA emitters. A production app
    // can post-process these values with a measured 3x4/XYZ calibration matrix.
    let warm_overlap = rgb[0].min(rgb[1]) * (1.0 - 0.65 * rgb[2]);
    let amber = warm_overlap * amber_mix.clamp(0.0, 1.0);
    [
        (rgb[0] - 0.56 * amber).max(0.0),
        (rgb[1] - 0.38 * amber).max(0.0),
        rgb[2],
        amber.clamp(0.0, 1.0),
    ]
}

fn limit_luminance_step(current: &mut [f32; 4], previous: &[f32; 4], max_step: f32) {
    if max_step >= 1.0 {
        return;
    }
    let current_luma = 0.2126 * current[0] + 0.7152 * current[1] + 0.0722 * current[2];
    let previous_luma = 0.2126 * previous[0] + 0.7152 * previous[1] + 0.0722 * previous[2];
    let difference = current_luma - previous_luma;
    if difference.abs() <= max_step || current_luma <= 1.0e-8 {
        return;
    }
    let target_luma = previous_luma + difference.signum() * max_step;
    let gain = (target_luma / current_luma).clamp(0.0, 1.0);
    current[0] *= gain;
    current[1] *= gain;
    current[2] *= gain;
    current[3] = lerp(previous[3], current[3], gain);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_finite_and_bounded() {
        let mut director = LightDirector::new(RenderConfig::default(), 48_000, 64).unwrap();
        let mut chroma = [0.0; NUM_CHROMA];
        chroma[0] = 0.7;
        chroma[7] = 0.3;
        let frame = MusicFrame {
            scene: SceneKind::Drop,
            loudness: 0.8,
            drop: 1.0,
            impact: 1.0,
            chroma,
            tonal_confidence: 0.9,
            ..MusicFrame::default()
        };
        director.ingest_frame(&frame);
        let points = LightPoint::ring(64);
        let mut output = vec![Rgba::default(); 64];
        director.render(1_024, &points, &mut output).unwrap();
        assert!(output.iter().all(|value| value.is_finite()));
        assert!(output.iter().all(|value| {
            [value.r, value.g, value.b, value.a]
                .iter()
                .all(|channel| (0.0..=1.0).contains(channel))
        }));
    }

    #[test]
    fn harmonic_hue_has_confidence() {
        let mut chroma = [0.0; NUM_CHROMA];
        chroma[0] = 0.75;
        chroma[7] = 0.25;
        let (_, confidence) = harmonic_hue(&chroma);
        assert!(confidence > 0.7);
    }
}
