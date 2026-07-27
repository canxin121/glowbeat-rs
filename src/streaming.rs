//! Unified low-latency streaming engine.

use crate::config::{ConfigError, EngineConfig};
use crate::director::{LightDirector, RenderError};
use crate::dsp::MusicObserver;
use crate::types::{LightPoint, MusicFrame, Rgba};

/// Summary of one streaming processing call.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessReport {
    /// Complete input PCM frames consumed.
    pub input_frames: usize,
    /// Music-analysis frames produced.
    pub output_frames: usize,
    /// Absolute mono sample index after processing.
    pub sample_index: u64,
    /// Deterministic short-window latency in PCM samples.
    pub analysis_latency_samples: usize,
}

/// Causal music observer and light director for microphone, screen capture, and
/// any other unknown live audio source.
///
/// The engine performs no platform I/O and uses no locks. A production audio
/// callback should copy PCM into a fixed-capacity SPSC queue; a dedicated DSP
/// worker then calls this type.
#[derive(Clone, Debug)]
pub struct StreamingEngine {
    config: EngineConfig,
    observer: MusicObserver,
    director: LightDirector,
    music_scratch: Vec<MusicFrame>,
    latest_music: Option<MusicFrame>,
}

impl StreamingEngine {
    /// Creates an engine with temporal renderer state for `light_capacity`
    /// outputs. The actual light coordinates are supplied on each render call.
    pub fn new(config: EngineConfig, light_capacity: usize) -> Result<Self, ConfigError> {
        config.validate()?;
        if light_capacity == 0 {
            return Err(ConfigError("light_capacity must be nonzero"));
        }
        let observer = MusicObserver::new(config.analysis.clone())?;
        let director = LightDirector::new(
            config.render.clone(),
            config.analysis.sample_rate,
            light_capacity,
        )?;
        Ok(Self {
            config,
            observer,
            director,
            music_scratch: Vec::with_capacity(256),
            latest_music: None,
        })
    }

    /// Returns the complete engine configuration.
    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Returns the current audio sample position.
    #[must_use]
    pub const fn sample_index(&self) -> u64 {
        self.observer.sample_index()
    }

    /// Returns the most recent music frame, if the initial FFT window has filled.
    #[must_use]
    pub fn latest_frame(&self) -> Option<&MusicFrame> {
        self.latest_music.as_ref()
    }

    /// Compatibility alias for [`Self::latest_frame`].
    #[must_use]
    pub fn latest_music(&self) -> Option<&MusicFrame> {
        self.latest_frame()
    }

    /// Returns the deterministic short-window analysis delay in samples.
    #[must_use]
    pub const fn analysis_latency_samples(&self) -> usize {
        self.config.analysis.short_fft_size / 2
    }

    /// Number of lights with preallocated renderer history.
    #[must_use]
    pub fn light_capacity(&self) -> usize {
        self.director.light_capacity()
    }

    /// Grows renderer history without resetting existing lights.
    pub fn reserve_lights(&mut self, light_capacity: usize) {
        self.director.reserve_lights(light_capacity);
    }

    /// Reserves enough analysis-frame scratch for an unusually large input
    /// block. Normal audio callbacks do not need this method.
    pub fn reserve_input_sample_frames(&mut self, input_sample_frames: usize) {
        let produced = input_sample_frames / self.config.analysis.short_hop_size + 2;
        if produced > self.music_scratch.capacity() {
            self.music_scratch
                .reserve(produced - self.music_scratch.capacity());
        }
    }

    /// Resets causal history after a source change or discontinuity.
    pub fn reset(&mut self) {
        self.observer = MusicObserver::new(self.config.analysis.clone())
            .expect("existing analysis configuration remains valid");
        self.director.reset();
        self.music_scratch.clear();
        self.latest_music = None;
    }

    /// Pushes interleaved PCM into the causal observer.
    pub fn push_interleaved(&mut self, input: &[f32]) -> ProcessReport {
        self.observer
            .process_interleaved(input, &mut self.music_scratch);
        let output_frames = self.music_scratch.len();
        for music in &self.music_scratch {
            self.director.ingest_frame(music);
            self.latest_music = Some(music.clone());
        }
        ProcessReport {
            input_frames: input.len() / self.config.analysis.channels,
            output_frames,
            sample_index: self.observer.sample_index(),
            analysis_latency_samples: self.analysis_latency_samples(),
        }
    }

    /// Pushes mono PCM into the causal observer.
    pub fn push_mono(&mut self, input: &[f32]) -> ProcessReport {
        self.observer.process_mono(input, &mut self.music_scratch);
        let output_frames = self.music_scratch.len();
        for music in &self.music_scratch {
            self.director.ingest_frame(music);
            self.latest_music = Some(music.clone());
        }
        ProcessReport {
            input_frames: input.len(),
            output_frames,
            sample_index: self.observer.sample_index(),
            analysis_latency_samples: self.analysis_latency_samples(),
        }
    }

    /// Renders the latest causal state at an explicit audio sample position.
    ///
    /// This is the preferred API when the light output rate is independent from
    /// the analysis hop rate (for example 120 FPS lighting with 187.5 Hz
    /// analysis at 48 kHz/256 samples).
    pub fn render_at(
        &mut self,
        sample_index: u64,
        lights: &[LightPoint],
        output: &mut [Rgba],
    ) -> Result<(), RenderError> {
        self.director.render(sample_index, lights, output)
    }

    /// Renders at the most recently consumed audio sample.
    pub fn render_now(
        &mut self,
        lights: &[LightPoint],
        output: &mut [Rgba],
    ) -> Result<(), RenderError> {
        self.render_at(self.observer.sample_index(), lights, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_engine_emits_colors() {
        let mut config = EngineConfig::default();
        config.analysis.channels = 1;
        let points = LightPoint::ring(16);
        let mut engine = StreamingEngine::new(config, points.len()).unwrap();
        let sample_rate = engine.config().analysis.sample_rate as usize;
        let mut audio = vec![0.0_f32; sample_rate];
        for (index, sample) in audio.iter_mut().enumerate() {
            *sample =
                0.2 * (core::f32::consts::TAU * 110.0 * index as f32 / sample_rate as f32).sin();
        }
        let report = engine.push_mono(&audio);
        let mut colors = vec![Rgba::default(); points.len()];
        engine.render_now(&points, &mut colors).unwrap();
        assert!(report.output_frames > 50);
        assert!(colors.iter().all(|color| color.is_finite()));
    }
}
