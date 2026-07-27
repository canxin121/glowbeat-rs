//! Dependency-free DSP building blocks and the unified streaming observer.

mod beat;
mod chroma;
mod filters;
mod onset;
mod scene;
mod spectrum;

pub use beat::{BeatFrame, BeatTracker};
pub use chroma::{ChromaAnalyzer, ChromaFrame};
pub use filters::{HopMetrics, Preprocessor};
pub use onset::{OnsetDetector, OnsetFrame};
pub use scene::{SceneFrame, SceneTracker};
pub use spectrum::{SpectrumAnalyzer, SpectrumFeatures};

use crate::config::AnalysisConfig;
use crate::types::MusicFrame;

/// Causal music observer shared by the public streaming engine and offline pass.
#[derive(Clone, Debug)]
pub struct MusicObserver {
    config: AnalysisConfig,
    preprocessor: Preprocessor,
    ring: Vec<f32>,
    ring_write: usize,
    sample_index: u64,
    short_spectrum: SpectrumAnalyzer,
    long_spectrum: SpectrumAnalyzer,
    onset: OnsetDetector,
    beat: BeatTracker,
    chroma: ChromaAnalyzer,
    scene: SceneTracker,
    latest_chroma: ChromaFrame,
    short_samples_since_frame: usize,
    long_samples_since_frame: usize,
}

impl MusicObserver {
    /// Creates a fully preallocated observer.
    pub fn new(config: AnalysisConfig) -> Result<Self, crate::config::ConfigError> {
        config.validate()?;
        let hop_seconds = config.short_hop_seconds();
        let ring_size = config.long_fft_size;
        Ok(Self {
            preprocessor: Preprocessor::new(
                config.sample_rate,
                config.high_pass_hz,
                config.agc_target,
                config.agc_max_gain,
            ),
            ring: vec![0.0; ring_size],
            ring_write: 0,
            sample_index: 0,
            short_spectrum: SpectrumAnalyzer::new_with_hop(
                config.sample_rate,
                config.short_fft_size,
                config.short_hop_size,
                config.min_frequency_hz,
                config.max_frequency_hz,
            ),
            long_spectrum: SpectrumAnalyzer::new_with_hop(
                config.sample_rate,
                config.long_fft_size,
                config.long_hop_size,
                config.min_frequency_hz,
                config.max_frequency_hz,
            ),
            onset: OnsetDetector::new(hop_seconds),
            beat: BeatTracker::new(
                hop_seconds,
                config.min_bpm,
                config.max_bpm,
                config.beat_history_seconds,
            ),
            chroma: ChromaAnalyzer::new(),
            scene: SceneTracker::new(hop_seconds),
            latest_chroma: ChromaFrame::default(),
            short_samples_since_frame: 0,
            long_samples_since_frame: 0,
            config,
        })
    }

    /// Input sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Expected number of interleaved channels.
    #[must_use]
    pub const fn channels(&self) -> usize {
        self.config.channels
    }

    /// Current absolute sample position.
    #[must_use]
    pub const fn sample_index(&self) -> u64 {
        self.sample_index
    }

    /// Pushes interleaved PCM and appends zero or more analysis frames.
    ///
    /// `output` is cleared but its allocation is retained.
    pub fn process_interleaved(&mut self, samples: &[f32], output: &mut Vec<MusicFrame>) {
        output.clear();
        let channels = self.config.channels;
        let complete_frames = samples.len() / channels;
        for frame_index in 0..complete_frames {
            let base = frame_index * channels;
            let mut mono = 0.0;
            for channel in 0..channels {
                mono += samples[base + channel];
            }
            mono /= channels as f32;
            self.push_mono_sample(mono, output);
        }
    }

    /// Pushes mono PCM and appends zero or more analysis frames.
    pub fn process_mono(&mut self, samples: &[f32], output: &mut Vec<MusicFrame>) {
        output.clear();
        for &sample in samples {
            self.push_mono_sample(sample, output);
        }
    }

    fn push_mono_sample(&mut self, sample: f32, output: &mut Vec<MusicFrame>) {
        let conditioned = self.preprocessor.process(sample);
        self.ring[self.ring_write] = conditioned;
        self.ring_write = (self.ring_write + 1) % self.ring.len();
        self.sample_index = self.sample_index.wrapping_add(1);
        self.short_samples_since_frame += 1;
        self.long_samples_since_frame += 1;

        if self.sample_index >= self.config.long_fft_size as u64
            && self.long_samples_since_frame >= self.config.long_hop_size
        {
            self.long_samples_since_frame %= self.config.long_hop_size;
            let _ = self.long_spectrum.analyze_ring(&self.ring, self.ring_write);
            let long_dt = self.config.long_hop_size as f32 / self.config.sample_rate as f32;
            self.latest_chroma = self.chroma.update(
                self.long_spectrum.magnitudes(),
                self.config.sample_rate,
                self.config.long_fft_size,
                long_dt,
            );
        }

        if self.sample_index >= self.config.short_fft_size as u64
            && self.short_samples_since_frame >= self.config.short_hop_size
        {
            self.short_samples_since_frame %= self.config.short_hop_size;
            output.push(self.make_short_frame());
        }
    }

    fn make_short_frame(&mut self) -> MusicFrame {
        let metrics = self.preprocessor.finish_hop();
        let spectrum = self
            .short_spectrum
            .analyze_ring(&self.ring, self.ring_write);
        let onset = self.onset.update(&spectrum.bands);
        let onset_drive = (0.60 * onset.low
            + 0.42 * onset.mid
            + 0.08 * onset.high
            + 0.48 * onset.impact
            + 0.18 * onset.flux)
            .clamp(0.0, 1.5);
        let beat = self.beat.update(onset_drive);
        let scene = self.scene.update(
            metrics.loudness,
            spectrum.total_energy,
            &spectrum.bands,
            &self.latest_chroma.values,
            onset_drive.min(1.0),
            onset.impact,
            beat.downbeat_event,
            spectrum.centroid,
        );
        MusicFrame {
            sample_index: self.sample_index,
            time_seconds: self.sample_index as f64 / self.config.sample_rate as f64,
            loudness: metrics.loudness,
            peak: metrics.peak,
            bands: spectrum.bands,
            chroma: self.latest_chroma.values,
            tonal_confidence: self.latest_chroma.confidence,
            spectral_centroid: spectrum.centroid,
            flux: onset.flux,
            onset_low: onset.low,
            onset_mid: onset.mid,
            onset_high: onset.high,
            impact: onset.impact,
            bpm: beat.bpm,
            beat_phase: beat.phase,
            beat_confidence: beat.confidence,
            beat_in_bar: beat.beat_in_bar,
            meter: beat.meter,
            bar_phase: beat.bar_phase,
            downbeat_confidence: beat.downbeat_confidence,
            beat_event: beat.beat_event,
            downbeat_event: beat.downbeat_event,
            energy_trend: scene.energy_trend,
            buildup: scene.buildup,
            drop: scene.drop,
            novelty: scene.novelty,
            scene: scene.scene,
        }
    }
}
