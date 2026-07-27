//! High-level causal music-to-light API for a single live RGB fixture.
//!
//! [`LiveEngine`] is the complete real-time path for sources whose future is
//! unknown, including microphone and Android playback-capture audio. It owns
//! the causal DSP, musical interpretation, style rendering, sensitivity, and
//! compact low/mid/high metrics used by a user interface. Platform code only
//! supplies PCM and transports the resulting color.

use crate::config::{ConfigError, EngineConfig};
use crate::streaming::StreamingEngine;
use crate::types::{LightPoint, Rgba};

/// Artistic presentation selected by the user for a live stream.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LiveStyle {
    /// GEM pink/violet material driven by GlowBeat's musical luminance.
    #[default]
    GemPulse = 0,
    /// GlowBeat's native harmonic and scene-directed palette.
    Spectrum = 1,
    /// A continuous aurora rotation retaining GlowBeat's musical luminance.
    Aurora = 2,
}

impl LiveStyle {
    /// Decodes a platform wire value, using the safe default for unknown values.
    #[must_use]
    pub const fn from_wire(value: i32) -> Self {
        match value {
            1 => Self::Spectrum,
            2 => Self::Aurora,
            _ => Self::GemPulse,
        }
    }
}

/// User-selected settings interpreted entirely by the Rust live renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveSettings {
    /// Artistic presentation.
    pub style: LiveStyle,
    /// Output intensity multiplier in `[0.5, 2.0]`.
    pub sensitivity: f32,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            style: LiveStyle::GemPulse,
            sensitivity: 1.0,
        }
    }
}

impl LiveSettings {
    /// Converts untrusted platform values into bounded renderer settings.
    #[must_use]
    pub fn from_wire(style: i32, sensitivity: f32) -> Self {
        Self {
            style: LiveStyle::from_wire(style),
            sensitivity: if sensitivity.is_finite() {
                sensitivity.clamp(0.5, 2.0)
            } else {
                Self::default().sensitivity
            },
        }
    }
}

/// Compact music state emitted alongside the final live color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveMetrics {
    /// Waveform peak in `[0, 1]`.
    pub peak: f32,
    /// Perceptual loudness proxy in `[0, 1]`.
    pub loudness: f32,
    /// Low-frequency energy derived from GlowBeat's logarithmic filterbank.
    pub low: f32,
    /// Mid-frequency energy derived from GlowBeat's logarithmic filterbank.
    pub mid: f32,
    /// High-frequency energy derived from GlowBeat's logarithmic filterbank.
    pub high: f32,
    /// Broadband positive spectral flux.
    pub spectral_flux: f32,
    /// Strongest transient impact observed in this input block.
    pub impact: f32,
    /// True when a beat occurred in this input block.
    pub beat_event: bool,
    /// Estimated tempo in beats per minute.
    pub bpm: f32,
    /// Confidence in the inferred beat grid.
    pub beat_confidence: f32,
    /// Buildup/drop event strength.
    pub musical_event: f32,
}

/// Final renderer-owned color and current causal musical state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveFrame {
    /// Final style- and sensitivity-adjusted RGBA color.
    pub color: Rgba,
    /// Compact causal music analysis metrics.
    pub metrics: LiveMetrics,
}

/// Allocation-stable one-fixture live music-to-light engine.
///
/// The engine accepts mono PCM because platform adapters should downmix before
/// crossing their FFI boundary. It owns no platform I/O and performs no heap
/// allocation after construction for normal input blocks.
#[derive(Clone, Debug)]
pub struct LiveEngine {
    engine: StreamingEngine,
    point: [LightPoint; 1],
    color: [Rgba; 1],
}

impl LiveEngine {
    /// Creates an Android-friendly live engine for normalized mono PCM.
    pub fn new_mono(sample_rate: u32) -> Result<Self, ConfigError> {
        let mut config = EngineConfig::default();
        config.analysis.sample_rate = sample_rate;
        config.analysis.channels = 1;
        config.analysis.max_frequency_hz = config
            .analysis
            .max_frequency_hz
            .min(sample_rate as f32 * 0.48);
        config.render.frames_per_second = 100.0;
        // A physical stick receives one shared RGB value, not a spatial pixel layout.
        config.render.spatial_strength = 0.0;
        Self::with_config(config)
    }

    /// Creates a live engine from a validated mono configuration.
    pub fn with_config(config: EngineConfig) -> Result<Self, ConfigError> {
        if config.analysis.channels != 1 {
            return Err(ConfigError("LiveEngine requires mono PCM"));
        }
        Ok(Self {
            engine: StreamingEngine::new(config, 1)?,
            point: [LightPoint::new(0, 0.0, 0.0, 0.0, 0.5)],
            color: [Rgba::default()],
        })
    }

    /// Pushes one block of normalized mono PCM and returns the final live result.
    ///
    /// Short sub-blocks preserve beat events when a platform recorder provides
    /// more than one analysis hop per read without allocating in the hot path.
    pub fn process_mono(&mut self, pcm: &[f32], settings: LiveSettings) -> Option<LiveFrame> {
        if pcm.is_empty() {
            return None;
        }
        let mut beat_event = false;
        let mut strongest_impact = 0.0_f32;
        let mut sample_index = self.engine.sample_index();
        for chunk in pcm.chunks(128) {
            let report = self.engine.push_mono(chunk);
            sample_index = report.sample_index;
            if let Some(frame) = self.engine.latest_frame() {
                beat_event |= frame.beat_event;
                strongest_impact = strongest_impact.max(frame.impact);
            }
        }
        let frame = self.engine.latest_frame()?.clone();
        self.engine
            .render_at(sample_index, &self.point, &mut self.color)
            .ok()?;
        let mut color = self.color[0];
        apply_style(
            &mut color,
            settings.style,
            frame.time_seconds as f32,
            frame.loudness,
        );
        color = scale_color(color, settings.sensitivity);
        Some(LiveFrame {
            color,
            metrics: LiveMetrics {
                peak: frame.peak,
                loudness: frame.loudness,
                low: band_mean(&frame.bands[0..8]),
                mid: band_mean(&frame.bands[8..16]),
                high: band_mean(&frame.bands[16..24]),
                spectral_flux: frame.flux,
                impact: strongest_impact,
                beat_event,
                bpm: frame.bpm,
                beat_confidence: frame.beat_confidence,
                musical_event: frame.drop.max(frame.buildup * 0.25),
            },
        })
    }

    /// Clears causal analysis and renderer history after an input discontinuity.
    pub fn reset(&mut self) {
        self.engine.reset();
        self.color = [Rgba::default()];
    }
}

fn band_mean(values: &[f32]) -> f32 {
    (values.iter().sum::<f32>() / values.len() as f32).clamp(0.0, 1.0)
}

fn scale_color(mut color: Rgba, sensitivity: f32) -> Rgba {
    let gain = if sensitivity.is_finite() {
        sensitivity.clamp(0.5, 2.0)
    } else {
        LiveSettings::default().sensitivity
    };
    color.r = (color.r * gain).clamp(0.0, 1.0);
    color.g = (color.g * gain).clamp(0.0, 1.0);
    color.b = (color.b * gain).clamp(0.0, 1.0);
    color
}

fn apply_style(color: &mut Rgba, style: LiveStyle, time: f32, loudness: f32) {
    match style {
        LiveStyle::GemPulse => {
            let luminance = color.r.max(color.g).max(color.b);
            color.r = luminance;
            color.g = luminance * (0.08 + loudness * 0.12);
            color.b = luminance * (0.52 + loudness * 0.30);
        }
        LiveStyle::Spectrum => {}
        LiveStyle::Aurora => {
            let (hue, saturation, value) = rgb_to_hsv(color.r, color.g, color.b);
            let rotated = (hue + time * (0.025 + loudness * 0.08)).rem_euclid(1.0);
            let (r, g, b) = hsv_to_rgb(rotated, saturation.max(0.72), value);
            color.r = r;
            color.g = g;
            color.b = b;
        }
    }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (f32, f32, f32) {
    let section = (hue * 6.0).floor() as i32;
    let fraction = hue * 6.0 - section as f32;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - fraction * saturation);
    let t = value * (1.0 - (1.0 - fraction) * saturation);
    match section.rem_euclid(6) {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_live_styles_emit_bounded_color_and_rust_metrics() {
        let sample_rate = 48_000;
        let audio = (0..sample_rate)
            .map(|index| {
                0.45 * (core::f32::consts::TAU * 128.0 * index as f32 / sample_rate as f32).sin()
            })
            .collect::<Vec<_>>();
        for style in [LiveStyle::GemPulse, LiveStyle::Spectrum, LiveStyle::Aurora] {
            let mut engine = LiveEngine::new_mono(sample_rate).unwrap();
            let frame = engine
                .process_mono(
                    &audio,
                    LiveSettings {
                        style,
                        sensitivity: 1.4,
                    },
                )
                .unwrap();
            assert!(frame.color.is_finite());
            assert_eq!(frame.color, frame.color.clamped());
            assert!((0.0..=1.0).contains(&frame.metrics.low));
            assert!((0.0..=1.0).contains(&frame.metrics.mid));
            assert!((0.0..=1.0).contains(&frame.metrics.high));
            assert!(frame.metrics.bpm.is_finite());
        }
    }

    #[test]
    fn invalid_wire_sensitivity_uses_safe_default() {
        assert_eq!(
            LiveSettings::from_wire(99, f32::NAN),
            LiveSettings::default()
        );
    }
}
