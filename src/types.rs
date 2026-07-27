//! Public data types shared by the analysis and rendering pipelines.

use core::fmt;

/// Number of logarithmic frequency bands emitted by the analyzer.
pub const NUM_BANDS: usize = 24;
/// Number of pitch classes in the chroma vector.
pub const NUM_CHROMA: usize = 12;

/// A normalized four-channel light value.
///
/// In [`crate::config::OutputMode::Alpha`], `a` is opacity. In
/// [`crate::config::OutputMode::Amber`], `a` is a physical amber channel.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Rgba {
    /// Red channel in `[0, 1]`.
    pub r: f32,
    /// Green channel in `[0, 1]`.
    pub g: f32,
    /// Blue channel in `[0, 1]`.
    pub b: f32,
    /// Alpha or amber channel in `[0, 1]`, depending on output mode.
    pub a: f32,
}

impl Rgba {
    /// Constructs a four-channel value.
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Returns a value with every channel clamped to `[0, 1]`.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
            a: self.a.clamp(0.0, 1.0),
        }
    }

    /// Returns true when every channel is finite.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.r.is_finite() && self.g.is_finite() && self.b.is_finite() && self.a.is_finite()
    }
}

impl fmt::Debug for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Rgba")
            .field(&format_args!("{:.3}", self.r))
            .field(&format_args!("{:.3}", self.g))
            .field(&format_args!("{:.3}", self.b))
            .field(&format_args!("{:.3}", self.a))
            .finish()
    }
}

/// A light's normalized spatial coordinate and stable identifier.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightPoint {
    /// Stable device or pixel identifier, used for deterministic variation.
    pub id: u32,
    /// Horizontal coordinate, conventionally in `[-1, 1]`.
    pub x: f32,
    /// Vertical coordinate, conventionally in `[-1, 1]`.
    pub y: f32,
    /// Depth coordinate, conventionally in `[-1, 1]`.
    pub z: f32,
    /// Optional zone value in `[0, 1]`.
    pub zone: f32,
}

impl LightPoint {
    /// Creates a point with a stable identifier.
    #[must_use]
    pub const fn new(id: u32, x: f32, y: f32, z: f32, zone: f32) -> Self {
        Self { id, x, y, z, zone }
    }

    /// Generates a one-dimensional line layout centered around zero.
    #[must_use]
    pub fn line(count: usize) -> Vec<Self> {
        if count == 0 {
            return Vec::new();
        }
        if count == 1 {
            return vec![Self::new(0, 0.0, 0.0, 0.0, 0.5)];
        }
        (0..count)
            .map(|index| {
                let t = index as f32 / (count - 1) as f32;
                Self::new(index as u32, t * 2.0 - 1.0, 0.0, 0.0, t)
            })
            .collect()
    }

    /// Generates a ring layout in the XY plane.
    #[must_use]
    pub fn ring(count: usize) -> Vec<Self> {
        if count == 0 {
            return Vec::new();
        }
        let tau = core::f32::consts::TAU;
        (0..count)
            .map(|index| {
                let t = index as f32 / count as f32;
                let angle = tau * t;
                Self::new(index as u32, angle.cos(), angle.sin(), 0.0, t)
            })
            .collect()
    }
}

/// Coarse musical scene inferred by the streaming observer or full-track pass.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SceneKind {
    /// Input is effectively silent.
    Silence = 0,
    /// Sparse or low-energy material.
    Calm = 1,
    /// Stable medium-energy groove.
    #[default]
    Groove = 2,
    /// High-energy material.
    Intense = 3,
    /// Rising energy and density suggest a buildup.
    Build = 4,
    /// Confirmed release or drop event.
    Drop = 5,
    /// Transitional or structurally novel material.
    Transition = 6,
}

impl SceneKind {
    /// Converts a serialized byte into a scene kind.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Silence,
            1 => Self::Calm,
            3 => Self::Intense,
            4 => Self::Build,
            5 => Self::Drop,
            6 => Self::Transition,
            _ => Self::Groove,
        }
    }
}

/// Spatial organization used by the light renderer.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpatialMode {
    /// All lights share the same large-scale envelope.
    #[default]
    Uniform = 0,
    /// A smooth directional traveling wave.
    Wave = 1,
    /// A radial expansion or contraction.
    Radial = 2,
    /// A rotating spiral field.
    Spiral = 3,
    /// Stable zone-based call and response.
    Zones = 4,
    /// Mirror-symmetric motion around the center.
    Mirror = 5,
    /// Sparse deterministic highlights.
    Sparkle = 6,
}

impl SpatialMode {
    /// Converts a serialized byte into a spatial mode.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Wave,
            2 => Self::Radial,
            3 => Self::Spiral,
            4 => Self::Zones,
            5 => Self::Mirror,
            6 => Self::Sparkle,
            _ => Self::Uniform,
        }
    }
}

/// Kind of a transient lighting accent.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccentKind {
    /// Low-frequency attack, usually kick or bass.
    Low = 0,
    /// Mid-frequency attack, usually snare, clap, or vocal consonant.
    Mid = 1,
    /// High-frequency attack, usually hi-hat or cymbal.
    High = 2,
    /// Broadband impact.
    Impact = 3,
    /// Beat-grid event.
    Beat = 4,
    /// Bar-level downbeat event.
    Downbeat = 5,
    /// Confirmed drop/release event.
    Drop = 6,
}

impl AccentKind {
    /// Converts a serialized byte into an accent kind.
    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Low,
            1 => Self::Mid,
            2 => Self::High,
            3 => Self::Impact,
            4 => Self::Beat,
            5 => Self::Downbeat,
            6 => Self::Drop,
            _ => Self::Impact,
        }
    }
}

/// A time-stamped musical accent used in full-track scores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccentEvent {
    /// Absolute audio sample index.
    pub sample_index: u64,
    /// Event category.
    pub kind: AccentKind,
    /// Normalized strength.
    pub strength: f32,
}

/// A compact, continuously updated description of the music.
#[derive(Clone, Debug, PartialEq)]
pub struct MusicFrame {
    /// Audio sample index at the end of the analyzed hop.
    pub sample_index: u64,
    /// Seconds from the beginning of the stream.
    pub time_seconds: f64,
    /// Perceptual loudness proxy in `[0, 1]`.
    pub loudness: f32,
    /// Short-term waveform peak in `[0, 1]`.
    pub peak: f32,
    /// Twenty-four normalized logarithmic frequency bands.
    pub bands: [f32; NUM_BANDS],
    /// Twelve normalized pitch-class strengths, C through B.
    pub chroma: [f32; NUM_CHROMA],
    /// Confidence that a stable pitch-class distribution is present.
    pub tonal_confidence: f32,
    /// Spectral centroid normalized to `[0, 1]`.
    pub spectral_centroid: f32,
    /// Broadband positive spectral flux.
    pub flux: f32,
    /// Low-frequency onset strength.
    pub onset_low: f32,
    /// Mid-frequency onset strength.
    pub onset_mid: f32,
    /// High-frequency onset strength.
    pub onset_high: f32,
    /// Broadband impact strength.
    pub impact: f32,
    /// Estimated tempo in beats per minute.
    pub bpm: f32,
    /// Continuous phase of the current beat in `[0, 1)`.
    pub beat_phase: f32,
    /// Beat-grid confidence in `[0, 1]`.
    pub beat_confidence: f32,
    /// Current beat within the inferred bar, beginning at zero.
    pub beat_in_bar: u8,
    /// Inferred meter, normally three or four.
    pub meter: u8,
    /// Continuous phase through the bar in `[0, 1)`.
    pub bar_phase: f32,
    /// Downbeat/meter confidence in `[0, 1]`.
    pub downbeat_confidence: f32,
    /// Set for a single analysis hop when the beat oscillator wraps.
    pub beat_event: bool,
    /// Set for a single analysis hop when a downbeat is inferred.
    pub downbeat_event: bool,
    /// Short-versus-long energy trend in `[-1, 1]`.
    pub energy_trend: f32,
    /// Probability-like buildup score in `[0, 1]`.
    pub buildup: f32,
    /// Confirmed drop pulse in `[0, 1]`.
    pub drop: f32,
    /// Structural novelty score in `[0, 1]`.
    pub novelty: f32,
    /// Coarse scene classification.
    pub scene: SceneKind,
}

impl Default for MusicFrame {
    fn default() -> Self {
        Self {
            sample_index: 0,
            time_seconds: 0.0,
            loudness: 0.0,
            peak: 0.0,
            bands: [0.0; NUM_BANDS],
            chroma: [0.0; NUM_CHROMA],
            tonal_confidence: 0.0,
            spectral_centroid: 0.0,
            flux: 0.0,
            onset_low: 0.0,
            onset_mid: 0.0,
            onset_high: 0.0,
            impact: 0.0,
            bpm: 120.0,
            beat_phase: 0.0,
            beat_confidence: 0.0,
            beat_in_bar: 0,
            meter: 4,
            bar_phase: 0.0,
            downbeat_confidence: 0.0,
            beat_event: false,
            downbeat_event: false,
            energy_trend: 0.0,
            buildup: 0.0,
            drop: 0.0,
            novelty: 0.0,
            scene: SceneKind::Silence,
        }
    }
}
