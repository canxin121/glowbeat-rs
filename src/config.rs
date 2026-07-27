//! Engine configuration and validation.

use core::fmt;

/// Meaning of the fourth output channel.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputMode {
    /// The fourth channel is opacity. The renderer emits `a = 1` by default.
    #[default]
    Alpha = 0,
    /// The fourth channel is a physical amber LED channel.
    Amber = 1,
}

/// Music-analysis parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisConfig {
    /// Input sample rate in hertz.
    pub sample_rate: u32,
    /// Number of interleaved input channels.
    pub channels: usize,
    /// Causal short FFT size. Must be a power of two.
    pub short_fft_size: usize,
    /// Short-analysis hop in samples. Must be no larger than the FFT size.
    pub short_hop_size: usize,
    /// Long FFT size used for chroma and stable timbre. Must be a power of two.
    pub long_fft_size: usize,
    /// Long-analysis hop in samples.
    pub long_hop_size: usize,
    /// Lowest frequency included in the filterbank.
    pub min_frequency_hz: f32,
    /// Highest frequency included in the filterbank.
    pub max_frequency_hz: f32,
    /// Minimum tracked tempo.
    pub min_bpm: f32,
    /// Maximum tracked tempo.
    pub max_bpm: f32,
    /// Beat history length in seconds.
    pub beat_history_seconds: f32,
    /// High-pass cutoff used to suppress DC, handling noise, and rumble.
    pub high_pass_hz: f32,
    /// Target analysis level for the feature-only automatic gain control.
    pub agc_target: f32,
    /// Maximum feature-analysis gain.
    pub agc_max_gain: f32,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            short_fft_size: 1_024,
            short_hop_size: 256,
            long_fft_size: 4_096,
            long_hop_size: 1_024,
            min_frequency_hz: 45.0,
            max_frequency_hz: 16_000.0,
            min_bpm: 55.0,
            max_bpm: 200.0,
            beat_history_seconds: 8.0,
            high_pass_hz: 35.0,
            agc_target: 0.18,
            agc_max_gain: 12.0,
        }
    }
}

impl AnalysisConfig {
    /// Validates all invariants required by the real-time analyzer.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if ![
            self.min_frequency_hz,
            self.max_frequency_hz,
            self.min_bpm,
            self.max_bpm,
            self.beat_history_seconds,
            self.high_pass_hz,
            self.agc_target,
            self.agc_max_gain,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            return Err(ConfigError("analysis settings must be finite"));
        }
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(ConfigError("sample_rate must be between 8 kHz and 384 kHz"));
        }
        if self.channels == 0 || self.channels > 32 {
            return Err(ConfigError("channels must be between 1 and 32"));
        }
        if !self.short_fft_size.is_power_of_two() || self.short_fft_size < 128 {
            return Err(ConfigError("short_fft_size must be a power of two >= 128"));
        }
        if !self.long_fft_size.is_power_of_two() || self.long_fft_size < self.short_fft_size {
            return Err(ConfigError(
                "long_fft_size must be a power of two >= short_fft_size",
            ));
        }
        if self.short_hop_size == 0 || self.short_hop_size > self.short_fft_size {
            return Err(ConfigError("short_hop_size must be in 1..=short_fft_size"));
        }
        if self.long_hop_size == 0 || self.long_hop_size > self.long_fft_size {
            return Err(ConfigError("long_hop_size must be in 1..=long_fft_size"));
        }
        if self.min_frequency_hz <= 0.0
            || self.max_frequency_hz <= self.min_frequency_hz
            || self.max_frequency_hz > self.sample_rate as f32 * 0.5
        {
            return Err(ConfigError("invalid filterbank frequency range"));
        }
        if self.min_bpm < 20.0 || self.max_bpm <= self.min_bpm || self.max_bpm > 400.0 {
            return Err(ConfigError("invalid BPM range"));
        }
        if !(2.0..=30.0).contains(&self.beat_history_seconds) {
            return Err(ConfigError("beat_history_seconds must be in 2..=30"));
        }
        if self.high_pass_hz < 0.0 || self.high_pass_hz >= self.sample_rate as f32 * 0.25 {
            return Err(ConfigError("invalid high-pass cutoff"));
        }
        if !(0.01..=1.0).contains(&self.agc_target) || self.agc_max_gain < 1.0 {
            return Err(ConfigError("invalid AGC settings"));
        }
        Ok(())
    }

    /// Duration of a short-analysis hop in seconds.
    #[must_use]
    pub fn short_hop_seconds(&self) -> f32 {
        self.short_hop_size as f32 / self.sample_rate as f32
    }
}

/// Light-rendering parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    /// Nominal render rate. The renderer remains sample-clock driven.
    pub frames_per_second: f32,
    /// Maximum per-channel output.
    pub max_brightness: f32,
    /// Perceptual output gamma. Use `1.0` when the transport expects linear light.
    pub output_gamma: f32,
    /// Meaning of the fourth output channel.
    pub output_mode: OutputMode,
    /// Amount of warm RGB energy transferred into a physical amber channel.
    pub amber_mix: f32,
    /// Stable user-selected hue offset in turns.
    pub hue_offset: f32,
    /// Overall saturation multiplier.
    pub saturation: f32,
    /// Strength of beat-synchronous animation.
    pub beat_strength: f32,
    /// Strength of transient accents.
    pub accent_strength: f32,
    /// Strength of deterministic per-light spatial variation.
    pub spatial_strength: f32,
    /// Limits abrupt full-field luminance changes. `1.0` effectively disables it.
    pub max_luma_step: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            frames_per_second: 120.0,
            max_brightness: 0.92,
            output_gamma: 2.2,
            output_mode: OutputMode::Alpha,
            amber_mix: 0.65,
            hue_offset: 0.0,
            saturation: 1.0,
            beat_strength: 1.0,
            accent_strength: 1.0,
            spatial_strength: 0.75,
            max_luma_step: 0.85,
        }
    }
}

impl RenderConfig {
    /// Validates renderer settings.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if ![
            self.frames_per_second,
            self.max_brightness,
            self.output_gamma,
            self.amber_mix,
            self.hue_offset,
            self.saturation,
            self.beat_strength,
            self.accent_strength,
            self.spatial_strength,
            self.max_luma_step,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            return Err(ConfigError("renderer settings must be finite"));
        }
        if !(1.0..=1_000.0).contains(&self.frames_per_second) {
            return Err(ConfigError("frames_per_second must be in 1..=1000"));
        }
        if !(0.0..=1.0).contains(&self.max_brightness) {
            return Err(ConfigError("max_brightness must be in 0..=1"));
        }
        if !(0.1..=5.0).contains(&self.output_gamma) {
            return Err(ConfigError("output_gamma must be in 0.1..=5"));
        }
        if !(0.0..=1.0).contains(&self.amber_mix)
            || !(0.0..=2.0).contains(&self.saturation)
            || !(0.0..=2.0).contains(&self.beat_strength)
            || !(0.0..=2.0).contains(&self.accent_strength)
            || !(0.0..=2.0).contains(&self.spatial_strength)
            || !(0.0..=1.0).contains(&self.max_luma_step)
        {
            return Err(ConfigError(
                "one or more renderer strengths are out of range",
            ));
        }
        Ok(())
    }
}

/// Complete engine configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineConfig {
    /// Analysis settings.
    pub analysis: AnalysisConfig,
    /// Rendering settings.
    pub render: RenderConfig,
}

impl EngineConfig {
    /// Validates both analysis and rendering settings.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.analysis.validate()?;
        self.render.validate()?;
        Ok(())
    }
}

/// Human-readable configuration error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigError(pub &'static str);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ConfigError {}
