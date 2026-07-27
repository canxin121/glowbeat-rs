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
use crate::math::{
    gamma_encode, hash_unit, lerp, oklch_to_linear_srgb, smoothing_alpha, smoothstep, wrap01,
};
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
            .ingest(frame.sample_index, frame.onset_low, sample_rate, 0.260);
        self.mid
            .ingest(frame.sample_index, frame.onset_mid, sample_rate, 0.155);
        self.high
            .ingest(frame.sample_index, frame.onset_high, sample_rate, 0.115);
        let downbeat_evidence = frame.beat_confidence * frame.downbeat_confidence;
        let downbeat = if frame.downbeat_event && downbeat_evidence > 0.18 {
            0.12 + 0.38 * downbeat_evidence
        } else {
            0.0
        };
        self.impact.ingest(
            frame.sample_index,
            frame.impact.max(downbeat),
            sample_rate,
            0.190,
        );
        self.drop
            .ingest(frame.sample_index, frame.drop, sample_rate, 0.560);
    }

    fn values_at(&self, sample_index: u64, sample_rate: u32) -> PulseValues {
        PulseValues {
            low: self.low.value_at(sample_index, sample_rate, 0.260),
            mid: self.mid.value_at(sample_index, sample_rate, 0.155),
            high: self.high.value_at(sample_index, sample_rate, 0.115),
            impact: self.impact.value_at(sample_index, sample_rate, 0.190),
            drop: self.drop.value_at(sample_index, sample_rate, 0.560),
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
    render_hue: f32,
    palette_confidence: f32,
    pulses: PulseBank,
    previous_output: Vec<[f32; 4]>,
    last_ingest_sample: Option<u64>,
    last_render_sample: Option<u64>,
}

impl LightDirector {
    /// Creates a director with reusable state for `light_capacity` outputs.
    pub fn new(
        config: RenderConfig,
        sample_rate: u32,
        light_capacity: usize,
    ) -> Result<Self, crate::config::ConfigError> {
        config.validate()?;
        let initial_output = initial_output_state(config.output_mode);
        Ok(Self {
            config,
            sample_rate: sample_rate.max(1),
            latest: MusicFrame::default(),
            palette_hue: 0.72,
            render_hue: 0.72,
            palette_confidence: 0.0,
            pulses: PulseBank::default(),
            previous_output: vec![initial_output; light_capacity],
            last_ingest_sample: None,
            last_render_sample: None,
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
        self.previous_output.len()
    }

    /// Ensures temporal state exists for at least `capacity` lights.
    ///
    /// Existing light history is retained. New lights begin from black.
    pub fn reserve_lights(&mut self, capacity: usize) {
        if capacity > self.previous_output.len() {
            self.previous_output
                .resize(capacity, initial_output_state(self.config.output_mode));
        }
    }

    /// Clears palette, pulse, and smoothing history while retaining capacity.
    pub fn reset(&mut self) {
        let capacity = self.previous_output.len();
        *self = Self::new(self.config.clone(), self.sample_rate, capacity)
            .expect("existing renderer configuration remains valid");
    }

    /// Ingests an analysis frame and updates persistent palette/event state.
    pub fn ingest_frame(&mut self, frame: &MusicFrame) {
        let first_frame = self.last_ingest_sample.is_none();
        let dt = self
            .last_ingest_sample
            .map(|previous| {
                frame.sample_index.saturating_sub(previous) as f32 / self.sample_rate as f32
            })
            .unwrap_or(1.0 / self.config.frames_per_second.max(1.0))
            .clamp(1.0e-5, 0.25);
        self.last_ingest_sample = Some(frame.sample_index);

        // Event pulses consume unsmoothed detector output so their latency stays
        // low. Continuous visual controls below use time-based smoothing.
        self.pulses.ingest(frame, self.sample_rate);
        let (hue, concentration) = harmonic_hue(&frame.chroma);
        let confidence = (frame.tonal_confidence * concentration).clamp(0.0, 1.0);
        if first_frame {
            if confidence > 0.12 {
                self.palette_hue = hue;
                self.render_hue = hue;
            }
            self.palette_confidence = confidence;
            self.latest = frame.clone();
            return;
        }

        if confidence > 0.08 {
            let difference = signed_hue_difference(hue, self.palette_hue);
            // Stable harmony normally moves over roughly a second. A real
            // structural boundary may accelerate the transition, but never turn
            // a noisy chroma frame into an instantaneous color jump.
            let boundary = smoothstep(0.30, 0.85, frame.novelty);
            let time_constant = lerp(1.20, 0.42, boundary);
            let alpha = smoothing_alpha(dt, time_constant) * smoothstep(0.08, 0.55, confidence);
            self.palette_hue = wrap01(self.palette_hue + difference * alpha);
        }
        self.palette_confidence +=
            smoothing_alpha(dt, 0.65) * (confidence - self.palette_confidence);

        let previous = &self.latest;
        let mut smoothed = frame.clone();
        smoothed.loudness = asymmetric_smooth(previous.loudness, frame.loudness, dt, 0.040, 0.220);
        smoothed.spectral_centroid = asymmetric_smooth(
            previous.spectral_centroid,
            frame.spectral_centroid,
            dt,
            0.090,
            0.260,
        );
        smoothed.energy_trend =
            asymmetric_smooth(previous.energy_trend, frame.energy_trend, dt, 0.120, 0.360);
        smoothed.buildup = asymmetric_smooth(previous.buildup, frame.buildup, dt, 0.180, 0.520);
        smoothed.novelty = asymmetric_smooth(previous.novelty, frame.novelty, dt, 0.080, 0.380);
        smoothed.flux = asymmetric_smooth(previous.flux, frame.flux, dt, 0.018, 0.100);
        smoothed.onset_low =
            asymmetric_smooth(previous.onset_low, frame.onset_low, dt, 0.010, 0.150);
        smoothed.onset_mid =
            asymmetric_smooth(previous.onset_mid, frame.onset_mid, dt, 0.010, 0.120);
        smoothed.onset_high =
            asymmetric_smooth(previous.onset_high, frame.onset_high, dt, 0.008, 0.095);
        smoothed.impact = asymmetric_smooth(previous.impact, frame.impact, dt, 0.008, 0.160);
        self.latest = smoothed;
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
        if lights.len() > self.previous_output.len() {
            return Err(RenderError::CapacityExceeded {
                requested: lights.len(),
                capacity: self.previous_output.len(),
            });
        }

        let nominal_dt = 1.0 / self.config.frames_per_second.max(1.0);
        let render_dt = self
            .last_render_sample
            .map(|previous| sample_index.saturating_sub(previous) as f32 / self.sample_rate as f32)
            .filter(|value| *value > 0.0)
            .unwrap_or(nominal_dt)
            .clamp(nominal_dt * 0.20, 0.25);
        self.last_render_sample = Some(sample_index);
        // Strength profiles are defined against a 120 Hz reference so the
        // perceived transition time remains similar at 60, 100, or 120 FPS.
        let reference_dt = 1.0 / 120.0;
        let frame_scale = (render_dt / reference_dt).clamp(0.05, 30.0);
        let output_step = if self.config.max_luma_step >= 1.0 {
            1.0
        } else {
            (self.config.max_luma_step * frame_scale).clamp(0.0, 0.20)
        };

        let pulses = self.pulses.values_at(sample_index, self.sample_rate);
        let target_hue = wrap01(
            context.palette_hue.unwrap_or(self.palette_hue)
                + context.palette_shift
                + self.config.hue_offset,
        );
        let hue_difference = signed_hue_difference(target_hue, self.render_hue);
        let hue_time_constant = if context.blackout > 0.35 { 0.24 } else { 0.48 };
        self.render_hue = wrap01(
            self.render_hue + hue_difference * smoothing_alpha(render_dt, hue_time_constant),
        );
        let hue = self.render_hue;

        // `latest` is initialized to a valid silent frame, so the same path
        // safely handles rendering before the first analysis hop.
        let frame = &self.latest;
        let elapsed =
            sample_index.saturating_sub(frame.sample_index) as f64 / self.sample_rate as f64;
        let elapsed = elapsed as f32;
        let beat_phase = wrap01(frame.beat_phase + elapsed * frame.bpm.max(1.0) / 60.0);
        let beats_elapsed = elapsed * frame.bpm.max(1.0) / 60.0;
        let meter = i64::from(frame.meter.max(1));
        let advanced_beats = beats_elapsed.floor() as i64;
        let beat_in_bar = (i64::from(frame.beat_in_bar) + advanced_beats).rem_euclid(meter) as u8;
        let bar_phase = wrap01(
            frame.bar_phase + elapsed * frame.bpm.max(1.0) / (60.0 * frame.meter.max(1) as f32),
        );

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

        let beat_confidence = frame.beat_confidence.clamp(0.0, 1.0);
        let beat_gate = smoothstep(0.18, 0.56, beat_confidence);
        let beat_shape = beat_envelope(beat_phase, beat_confidence) * beat_gate;
        // Above roughly 150 BPM, professional lighting commonly reads the groove
        // in half-time. Keep every detected beat for phase continuity, but make
        // alternating beats intentionally subordinate in even meters.
        let half_time = if frame.meter % 2 == 0 {
            smoothstep(148.0, 180.0, frame.bpm)
        } else {
            0.0
        };
        let beat_role = if beat_in_bar % 2 == 0 {
            1.0
        } else {
            lerp(1.0, 0.20, half_time)
        };
        // The old renderer emitted a strong half-beat pulse even before the
        // tempo had locked. Keep only a subtle offbeat texture at high confidence,
        // and remove it as the main tempo becomes visually dense.
        let offbeat = beat_envelope(wrap01(beat_phase + 0.5), beat_confidence)
            * 0.10
            * smoothstep(0.58, 0.90, beat_confidence)
            * (1.0 - 0.86 * half_time);
        let bar_accent = if beat_in_bar == 0 { 1.0 } else { 0.42 };
        let groove = (beat_shape * beat_role * (0.64 + 0.36 * bar_accent) + offbeat)
            * self.config.beat_strength;
        let anticipation = context.anticipation.clamp(0.0, 1.0);
        let blackout = context.blackout.clamp(0.0, 1.0);
        let global_level = (scene_params.base_level
            + 0.31 * frame.loudness
            + 0.10 * groove
            + 0.07 * frame.energy_trend.max(0.0)
            + 0.08 * anticipation)
            * context.intensity.max(0.0)
            * (1.0 - blackout);

        // Use f64 for absolute sample-clock conversion so sparkle timing stays
        // stable on long recordings after f32 can no longer represent every
        // individual sample index.
        let elapsed_time = sample_index as f64 / self.sample_rate as f64;
        let sparkle_position = elapsed_time * 5.0;
        let sparkle_bucket = sparkle_position.floor() as u32;
        let sparkle_phase = sparkle_position.fract() as f32;
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
                sparkle_phase,
            );
            let spatial_mix = lerp(1.0, field, self.config.spatial_strength.clamp(0.0, 1.0));

            let mut rgb = scale_rgb(base_rgb, global_level * (0.72 + 0.28 * spatial_mix));
            add_scaled_rgb(
                &mut rgb,
                secondary_rgb,
                (0.07 + 0.14 * groove) * (1.0 - 0.45 * anticipation) * spatial_mix,
            );

            let low_layer = pulses.low
                * self.config.accent_strength
                * (0.58 + 0.42 * low_spatial(point, beat_phase));
            let mid_layer = pulses.mid
                * self.config.accent_strength
                * (0.62 + 0.38 * mirror_spatial(point, bar_phase));
            let high_random = temporal_hash(
                point.id ^ 0x9e37_79b9,
                sparkle_bucket,
                sparkle_phase,
                0x7f4a_7c15,
            );
            let random_gate = smoothstep(0.66, 0.98, high_random + pulses.high * 0.42);
            // A single physical stick must not randomly disappear. Spatial
            // randomness is gradually introduced only when a layout asks for it.
            let high_gate = lerp(
                1.0,
                random_gate,
                self.config.spatial_strength.clamp(0.0, 1.0),
            );
            let high_layer = pulses.high
                * self.config.accent_strength
                * high_gate
                * (0.38 + 0.46 * frame.spectral_centroid);
            let impact_layer =
                pulses.impact * self.config.accent_strength * (0.60 + 0.40 * spatial_mix);
            let drop_layer = pulses.drop
                * self.config.accent_strength
                * radial_release(point, pulses.drop, beat_phase);

            add_scaled_rgb(&mut rgb, accent_rgb, 0.30 * low_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.36 * mid_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.18 * high_layer);
            add_scaled_rgb(&mut rgb, accent_rgb, 0.34 * impact_layer);
            add_scaled_rgb(&mut rgb, high_rgb, 0.56 * drop_layer);

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

            // Mix effects in linear light, then apply the output transfer once.
            // Temporal limiting intentionally happens in the transmitted/output
            // domain so a small near-black linear value cannot become a large
            // one-frame jump after gamma encoding.
            let mut rgba_output = match self.config.output_mode {
                OutputMode::Alpha => [
                    gamma_encode(rgb[0], self.config.output_gamma),
                    gamma_encode(rgb[1], self.config.output_gamma),
                    gamma_encode(rgb[2], self.config.output_gamma),
                    1.0,
                ],
                OutputMode::Amber => {
                    let rgba_linear = rgb_to_rgba_amber(rgb, self.config.amber_mix);
                    [
                        gamma_encode(rgba_linear[0], self.config.output_gamma),
                        gamma_encode(rgba_linear[1], self.config.output_gamma),
                        gamma_encode(rgba_linear[2], self.config.output_gamma),
                        gamma_encode(rgba_linear[3], self.config.output_gamma),
                    ]
                }
            };
            limit_output_step(
                &mut rgba_output,
                &self.previous_output[index],
                output_step,
                self.config.output_mode == OutputMode::Amber,
            );
            self.previous_output[index] = rgba_output;
            *destination = Rgba::new(
                rgba_output[0],
                rgba_output[1],
                rgba_output[2],
                rgba_output[3],
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
        SceneKind::Transition => SpatialMode::Mirror,
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
    let decay = (-phase / lerp(0.28, 0.17, confidence.clamp(0.0, 1.0))).exp();
    // A very short rounded attack avoids a mathematical discontinuity while
    // keeping the perceived beat close to the audio event.
    let rounded_attack = smoothstep(0.0, 0.018, phase);
    if phase < 0.018 {
        decay.max(rounded_attack)
    } else {
        decay
    }
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
    sparkle_phase: f32,
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
            let random = temporal_hash(point.id ^ seed, sparkle_bucket, sparkle_phase, 0x85eb_ca6b);
            let gate = smoothstep(0.84, 0.985, random + frame.onset_high * 0.18);
            0.52 + 0.48 * gate
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

fn initial_output_state(mode: OutputMode) -> [f32; 4] {
    match mode {
        OutputMode::Alpha => [0.0, 0.0, 0.0, 1.0],
        OutputMode::Amber => [0.0; 4],
    }
}

fn asymmetric_smooth(
    previous: f32,
    target: f32,
    dt: f32,
    attack_seconds: f32,
    release_seconds: f32,
) -> f32 {
    let time_constant = if target > previous {
        attack_seconds
    } else {
        release_seconds
    };
    previous + smoothing_alpha(dt, time_constant) * (target - previous)
}

fn temporal_hash(seed: u32, bucket: u32, phase: f32, multiplier: u32) -> f32 {
    let current = hash_unit(seed ^ bucket.wrapping_mul(multiplier));
    let next = hash_unit(seed ^ bucket.wrapping_add(1).wrapping_mul(multiplier));
    lerp(current, next, smoothstep(0.0, 1.0, phase))
}

fn limit_output_step(
    current: &mut [f32; 4],
    previous: &[f32; 4],
    max_step: f32,
    physical_fourth_channel: bool,
) {
    if max_step >= 1.0 {
        return;
    }

    let current_luma = 0.2126 * current[0] + 0.7152 * current[1] + 0.0722 * current[2];
    let previous_luma = 0.2126 * previous[0] + 0.7152 * previous[1] + 0.0722 * previous[2];
    let luma_difference = current_luma - previous_luma;
    let luma_limit = if luma_difference >= 0.0 {
        max_step * 1.55
    } else {
        max_step * 0.72
    };

    let mut blend: f32 = 1.0;
    if luma_difference.abs() > luma_limit.max(1.0e-8) {
        blend = blend.min(luma_limit / luma_difference.abs());
    }

    // Equal-luminance red/blue switches are still perceived as abrupt. Limit
    // the largest physical channel delta in addition to total luminance.
    let channels = if physical_fourth_channel { 4 } else { 3 };
    let mut largest_channel_delta = 0.0_f32;
    for channel in 0..channels {
        largest_channel_delta =
            largest_channel_delta.max((current[channel] - previous[channel]).abs());
    }
    let color_limit = (max_step * 1.35 + 0.0015).min(0.95);
    if largest_channel_delta > color_limit.max(1.0e-8) {
        blend = blend.min(color_limit / largest_channel_delta);
    }

    if blend < 1.0 {
        for channel in 0..channels {
            current[channel] = lerp(previous[channel], current[channel], blend);
        }
    }
    if !physical_fourth_channel {
        current[3] = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_finite_and_bounded() {
        let mut director = LightDirector::new(RenderConfig::default(), 48_000, 64).unwrap();
        let mut frame = MusicFrame {
            scene: SceneKind::Drop,
            loudness: 0.8,
            drop: 1.0,
            impact: 1.0,
            ..MusicFrame::default()
        };
        frame.chroma[0] = 0.7;
        frame.chroma[7] = 0.3;
        frame.tonal_confidence = 0.9;
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
    fn output_limiter_bounds_luminance_and_color_steps() {
        let previous = [0.10, 0.28, 0.42, 1.0];
        let mut current = [0.92, 0.02, 0.84, 1.0];
        let max_step = 0.055;
        limit_output_step(&mut current, &previous, max_step, false);

        let luma = |color: &[f32; 4]| 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
        let luma_limit = max_step * 1.55 + 1.0e-6;
        assert!((luma(&current) - luma(&previous)).abs() <= luma_limit);
        let color_limit = max_step * 1.35 + 0.0015 + 1.0e-6;
        assert!(current[..3]
            .iter()
            .zip(&previous[..3])
            .all(|(next, prior)| (*next - *prior).abs() <= color_limit));
        assert_eq!(current[3], 1.0);
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
