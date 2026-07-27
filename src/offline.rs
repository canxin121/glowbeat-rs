//! Non-causal full-track analysis and narrative LightingScore generation.

use crate::config::{AnalysisConfig, ConfigError};
use crate::dsp::MusicObserver;
use crate::math::{cosine_similarity, hash_u32, smoothstep, wrap01};
use crate::score::{LightingScore, SectionCue};
use crate::types::{
    AccentEvent, AccentKind, MusicFrame, SceneKind, SpatialMode, NUM_BANDS, NUM_CHROMA,
};

/// Quality-first analyzer for audio that is completely available before or
/// during playback.
#[derive(Clone, Debug)]
pub struct FullTrackAnalyzer {
    config: AnalysisConfig,
}

impl FullTrackAnalyzer {
    /// Creates an analyzer using the supplied base settings.
    pub fn new(config: AnalysisConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Returns the base analysis configuration.
    #[must_use]
    pub const fn config(&self) -> &AnalysisConfig {
        &self.config
    }

    /// Analyzes interleaved PCM using the analyzer's configured sample rate and
    /// channel count.
    pub fn analyze_interleaved(&self, samples: &[f32]) -> Result<LightingScore, OfflineError> {
        self.analyze_pcm(samples, self.config.sample_rate, self.config.channels)
    }

    /// Analyzes interleaved PCM with explicit stream metadata.
    ///
    /// This is the main embedded-player entry point after the application has
    /// decoded any container/codec into `f32` PCM.
    pub fn analyze_pcm(
        &self,
        samples: &[f32],
        sample_rate: u32,
        channels: usize,
    ) -> Result<LightingScore, OfflineError> {
        if samples.is_empty() {
            return Err(OfflineError::EmptyAudio);
        }
        if channels == 0 || samples.len() < channels || samples.len() % channels != 0 {
            return Err(OfflineError::InvalidAudio("invalid channel layout"));
        }
        let mut config = self.config.clone();
        config.sample_rate = sample_rate;
        config.channels = channels;
        config.max_frequency_hz = config
            .max_frequency_hz
            .min(sample_rate as f32 * 0.5 - 1.0)
            .max(config.min_frequency_hz + 1.0);
        config.validate()?;

        let mut observer = MusicObserver::new(config.clone())?;
        let mut block_frames = Vec::with_capacity(128);
        let estimated_frames = (samples.len() / channels / config.short_hop_size).saturating_add(4);
        let mut frames = Vec::with_capacity(estimated_frames);
        let block_sample_frames = 16_384usize;
        let block_scalars = block_sample_frames.saturating_mul(channels);
        for block in samples.chunks(block_scalars.max(channels)) {
            let complete = block.len() / channels * channels;
            observer.process_interleaved(&block[..complete], &mut block_frames);
            frames.extend(block_frames.iter().cloned());
        }
        if frames.is_empty() {
            return Err(OfflineError::InvalidAudio(
                "audio is shorter than the configured analysis window",
            ));
        }

        let duration_samples = (samples.len() / channels) as u64;
        let hop_seconds = config.short_hop_size as f32 / sample_rate as f32;
        zero_phase_enhance(&mut frames, hop_seconds);
        refine_beat_grid(&mut frames, hop_seconds, config.min_bpm, config.max_bpm);
        add_future_context(&mut frames, hop_seconds);
        let mut sections = build_sections(
            &frames,
            sample_rate,
            config.short_hop_size,
            duration_samples,
        );
        apply_section_narrative(&mut frames, &mut sections);
        let accents = collect_accents(&frames);

        Ok(LightingScore {
            version: 1,
            sample_rate,
            channels: u16::try_from(channels)
                .map_err(|_| OfflineError::InvalidAudio("channel count exceeds score format"))?,
            analysis_hop: config.short_hop_size as u32,
            duration_samples,
            frames,
            sections,
            accents,
        })
    }
}

impl Default for FullTrackAnalyzer {
    fn default() -> Self {
        Self::new(AnalysisConfig::default()).expect("default analysis config is valid")
    }
}

/// Full-track analysis error.
#[derive(Debug)]
pub enum OfflineError {
    /// Configuration validation failed.
    Config(ConfigError),
    /// No audio samples were supplied.
    EmptyAudio,
    /// PCM metadata or duration is invalid.
    InvalidAudio(&'static str),
}

impl core::fmt::Display for OfflineError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "configuration error: {error}"),
            Self::EmptyAudio => formatter.write_str("audio input is empty"),
            Self::InvalidAudio(message) => write!(formatter, "invalid audio: {message}"),
        }
    }
}

impl std::error::Error for OfflineError {}

impl From<ConfigError> for OfflineError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

fn zero_phase_enhance(frames: &mut [MusicFrame], hop_seconds: f32) {
    if frames.len() < 3 {
        return;
    }
    let loudness: Vec<f32> = frames.iter().map(|frame| frame.loudness).collect();
    let centroid: Vec<f32> = frames.iter().map(|frame| frame.spectral_centroid).collect();
    let smoothed_loudness = centered_triangle_smooth(&loudness, (0.055 / hop_seconds) as usize);
    let smoothed_centroid = centered_triangle_smooth(&centroid, (0.14 / hop_seconds) as usize);
    for (index, frame) in frames.iter_mut().enumerate() {
        frame.loudness = (0.74 * frame.loudness + 0.26 * smoothed_loudness[index]).clamp(0.0, 1.0);
        frame.spectral_centroid = smoothed_centroid[index].clamp(0.0, 1.0);
    }

    // The causal chroma already has useful stability. A symmetric pass removes
    // residual lag so palette changes align to harmonic boundaries.
    let chroma_radius = ((0.42 / hop_seconds).round() as usize).max(1);
    let original: Vec<[f32; NUM_CHROMA]> = frames.iter().map(|frame| frame.chroma).collect();
    for index in 0..frames.len() {
        let start = index.saturating_sub(chroma_radius);
        let end = (index + chroma_radius + 1).min(frames.len());
        let mut average = [0.0_f32; NUM_CHROMA];
        let mut weight_sum = 0.0;
        for (source, source_chroma) in original.iter().enumerate().take(end).skip(start) {
            let distance = index.abs_diff(source) as f32 / (chroma_radius + 1) as f32;
            let weight = (1.0 - distance).max(0.0);
            for (destination, value) in average.iter_mut().zip(*source_chroma) {
                *destination += weight * value;
            }
            weight_sum += weight;
        }
        if weight_sum > 1.0e-9 {
            for value in &mut average {
                *value /= weight_sum;
            }
            let sum = average.iter().sum::<f32>();
            if sum > 1.0e-9 {
                for value in &mut average {
                    *value /= sum;
                }
            }
            frames[index].chroma = average;
        }
    }
}

fn refine_beat_grid(frames: &mut [MusicFrame], dt: f32, min_bpm: f32, max_bpm: f32) {
    if frames.len() < (3.0 / dt) as usize {
        return;
    }
    let onset: Vec<f32> = frames
        .iter()
        .map(|frame| {
            (0.52 * frame.onset_low
                + 0.36 * frame.onset_mid
                + 0.18 * frame.onset_high
                + 0.62 * frame.impact
                + 0.18 * frame.flux)
                .clamp(0.0, 1.5)
        })
        .collect();
    let mean = onset.iter().sum::<f32>() / onset.len() as f32;
    let min_lag = (60.0 / max_bpm / dt).floor().max(1.0) as usize;
    let max_lag = (60.0 / min_bpm / dt).ceil() as usize;
    let max_lag = max_lag.min(onset.len() / 3);
    if max_lag <= min_lag {
        return;
    }

    let tracker_bpm = robust_median(
        &frames
            .iter()
            .filter(|frame| frame.beat_confidence > 0.12)
            .map(|frame| frame.bpm)
            .collect::<Vec<_>>(),
    )
    .unwrap_or(120.0);
    let mut best_lag = min_lag;
    let mut best_score = f32::NEG_INFINITY;
    let mut positive_scores = Vec::with_capacity(max_lag - min_lag + 1);
    for lag in min_lag..=max_lag {
        let mut numerator: f32 = 0.0;
        let mut left_energy: f32 = 0.0;
        let mut right_energy: f32 = 0.0;
        for index in lag..onset.len() {
            let left = onset[index] - mean;
            let right = onset[index - lag] - mean;
            numerator = left.mul_add(right, numerator);
            left_energy = left.mul_add(left, left_energy);
            right_energy = right.mul_add(right, right_energy);
        }
        let correlation = if left_energy > 1.0e-9 && right_energy > 1.0e-9 {
            numerator / (left_energy * right_energy).sqrt()
        } else {
            0.0
        };
        let bpm = 60.0 / (lag as f32 * dt);
        let continuity = (-0.5 * ((bpm / tracker_bpm.max(1.0)).ln() / 0.30).powi(2)).exp();
        let half_lag = lag / 2;
        let double_lag = lag.saturating_mul(2);
        let harmonic_hint = if half_lag >= min_lag || double_lag <= max_lag {
            1.0
        } else {
            0.97
        };
        let score = correlation * (0.76 + 0.24 * continuity) * harmonic_hint;
        positive_scores.push(score.max(0.0));
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }
    let median_score = robust_median(&positive_scores).unwrap_or(0.0);
    let activity = (mean * 5.0).clamp(0.0, 1.0);
    let confidence =
        ((best_score - median_score) / (best_score.abs() + 1.0e-5)).clamp(0.0, 1.0) * activity;
    if best_score <= 0.03 || confidence < 0.09 {
        // Free-tempo or onset-sparse material: retain the causal multi-hypothesis
        // tracker's output rather than forcing a grid onto it.
        bidirectional_bpm_smooth(frames, dt);
        return;
    }

    let bpm = 60.0 / (best_lag as f32 * dt);
    let phase_steps = best_lag.max(1);
    let mut best_offset = 0usize;
    let mut best_phase_score = f32::NEG_INFINITY;
    for offset in 0..phase_steps {
        let mut score = 0.0;
        let mut count = 0usize;
        let mut index = offset;
        while index < onset.len() {
            let center = onset[index];
            let neighbor = onset
                .get(index.saturating_sub(1))
                .copied()
                .unwrap_or(0.0)
                .max(onset.get(index + 1).copied().unwrap_or(0.0));
            score += center + 0.35 * neighbor;
            count += 1;
            index += best_lag;
        }
        if count > 0 {
            score /= count as f32;
        }
        if score > best_phase_score {
            best_phase_score = score;
            best_offset = offset;
        }
    }

    let mut beat_strengths = Vec::new();
    let mut index = best_offset;
    while index < frames.len() {
        beat_strengths.push(onset[index]);
        index += best_lag;
    }
    let (meter, downbeat_residue, downbeat_confidence) = infer_offline_meter(&beat_strengths);

    let beat_period = best_lag as f32;
    let mut previous_phase = 0.0;
    for (frame_index, frame) in frames.iter_mut().enumerate() {
        let position = (frame_index as f32 - best_offset as f32) / beat_period;
        let beat_number = position.floor() as i64;
        let phase = wrap01(position);
        let beat_event = frame_index > 0 && phase < previous_phase;
        let positive_beat = beat_number.max(0) as u64;
        let beat_in_bar =
            ((positive_beat + meter as u64 - downbeat_residue as u64) % meter as u64) as u8;
        frame.bpm = bpm;
        frame.beat_phase = phase;
        frame.beat_confidence = (0.45 + 0.55 * confidence).clamp(0.0, 1.0);
        frame.beat_event = beat_event;
        frame.meter = meter;
        frame.beat_in_bar = beat_in_bar;
        frame.bar_phase = (beat_in_bar as f32 + phase) / meter as f32;
        frame.downbeat_confidence = downbeat_confidence;
        frame.downbeat_event = beat_event && beat_in_bar == 0;
        previous_phase = phase;
    }
}

fn infer_offline_meter(beat_strengths: &[f32]) -> (u8, u8, f32) {
    if beat_strengths.len() < 8 {
        return (4, 0, 0.0);
    }
    let mut best_meter = 4_u8;
    let mut best_residue = 0_u8;
    let mut best_contrast = f32::NEG_INFINITY;
    for meter in [3_u8, 4_u8] {
        let mut residue_mean = vec![0.0_f32; meter as usize];
        let mut residue_count = vec![0_u32; meter as usize];
        for (index, &strength) in beat_strengths.iter().enumerate() {
            let residue = index % meter as usize;
            residue_mean[residue] += strength;
            residue_count[residue] += 1;
        }
        for (sum, count) in residue_mean.iter_mut().zip(residue_count) {
            *sum /= count.max(1) as f32;
        }
        let overall = residue_mean.iter().sum::<f32>() / meter as f32;
        for (residue, &value) in residue_mean.iter().enumerate() {
            let contrast = (value - overall) / (overall.abs() + 0.02);
            if contrast > best_contrast {
                best_contrast = contrast;
                best_meter = meter;
                best_residue = residue as u8;
            }
        }
    }
    (
        best_meter,
        best_residue,
        smoothstep(0.03, 0.45, best_contrast),
    )
}

fn bidirectional_bpm_smooth(frames: &mut [MusicFrame], dt: f32) {
    let radius = ((1.6 / dt).round() as usize).max(1);
    let original: Vec<f32> = frames.iter().map(|frame| frame.bpm).collect();
    let smooth = centered_triangle_smooth(&original, radius);
    for (frame, bpm) in frames.iter_mut().zip(smooth) {
        frame.bpm = bpm.clamp(20.0, 400.0);
    }
}

fn add_future_context(frames: &mut [MusicFrame], dt: f32) {
    let count = frames.len();
    if count == 0 {
        return;
    }
    let short = ((0.75 / dt).round() as usize).max(1);
    let medium = ((2.8 / dt).round() as usize).max(short + 1);
    let impact_window = ((0.20 / dt).round() as usize).max(1);
    let energy: Vec<f32> = frames
        .iter()
        .map(|frame| (0.72 * frame.loudness + 0.28 * average(&frame.bands)).clamp(0.0, 1.0))
        .collect();
    let prefix = prefix_sum(&energy);
    let onset_density: Vec<f32> = frames
        .iter()
        .map(|frame| (frame.flux + frame.impact + frame.onset_low + frame.onset_mid).min(1.0))
        .collect();
    let onset_prefix = prefix_sum(&onset_density);

    let mut future_build = vec![0.0_f32; count];
    for (index, destination) in future_build.iter_mut().enumerate() {
        let past_start = index.saturating_sub(medium);
        let past = range_mean(&prefix, past_start, index + 1);
        let near_future = range_mean(&prefix, index, (index + short).min(count));
        let far_future = range_mean(
            &prefix,
            (index + short).min(count),
            (index + medium).min(count),
        );
        let future_onsets = range_mean(&onset_prefix, index, (index + medium / 2).min(count));
        let slope = (far_future - past).max(0.0);
        let immediate = (near_future - past).max(0.0);
        *destination = (0.58 * smoothstep(0.015, 0.24, slope)
            + 0.22 * smoothstep(0.01, 0.15, immediate)
            + 0.20 * future_onsets)
            .clamp(0.0, 1.0);
    }

    let mut next_peak = vec![0.0_f32; count];
    for index in 0..count {
        let end = (index + impact_window).min(count);
        let mut peak: f32 = 0.0;
        for frame in &frames[index..end] {
            peak = peak.max(frame.impact.max(frame.onset_low));
        }
        next_peak[index] = peak;
    }

    for index in 0..count {
        let build = future_build[index].max(frames[index].buildup * 0.60);
        frames[index].buildup = build.clamp(0.0, 1.0);
        let drop_confirmed = frames[index].downbeat_event
            && next_peak[index] > 0.30
            && index > 0
            && future_build[index.saturating_sub(1)] > 0.34;
        if drop_confirmed {
            frames[index].drop = frames[index]
                .drop
                .max((0.58 + 0.42 * next_peak[index]).clamp(0.0, 1.0));
            frames[index].scene = SceneKind::Drop;
        } else if build > 0.52 && frames[index].scene != SceneKind::Drop {
            frames[index].scene = SceneKind::Build;
        }
    }
}

fn build_sections(
    frames: &[MusicFrame],
    sample_rate: u32,
    hop_size: usize,
    duration_samples: u64,
) -> Vec<SectionCue> {
    if frames.is_empty() {
        return Vec::new();
    }
    let dt = hop_size as f32 / sample_rate as f32;
    let min_gap = ((4.0 / dt).round() as usize).max(1);
    let max_gap = ((28.0 / dt).round() as usize).max(min_gap + 1);
    let novelty_radius = ((0.85 / dt).round() as usize).max(1);
    let energy: Vec<f32> = frames.iter().map(|frame| frame.loudness).collect();
    let smooth_energy = centered_triangle_smooth(&energy, novelty_radius);
    let mut novelty = vec![0.0_f32; frames.len()];
    for index in 1..frames.len() {
        let previous = index.saturating_sub(novelty_radius);
        let future = (index + novelty_radius).min(frames.len() - 1);
        let band_change =
            1.0 - cosine_similarity(&frames[previous].bands, &frames[future].bands).max(0.0);
        let chroma_change =
            1.0 - cosine_similarity(&frames[previous].chroma, &frames[future].chroma).max(0.0);
        let energy_change = (smooth_energy[future] - smooth_energy[previous]).abs();
        novelty[index] = (0.48 * band_change
            + 0.16 * chroma_change
            + 0.70 * energy_change
            + 0.24 * frames[index].novelty)
            .clamp(0.0, 1.0);
    }
    let median = robust_median(&novelty).unwrap_or(0.0);
    let deviations: Vec<f32> = novelty.iter().map(|value| (value - median).abs()).collect();
    let mad = robust_median(&deviations).unwrap_or(0.0);
    let threshold = (median + 1.8 * mad + 0.045).clamp(0.08, 0.65);

    let mut candidates: Vec<(usize, f32)> = (1..frames.len().saturating_sub(1))
        .filter(|&index| {
            novelty[index] >= novelty[index - 1]
                && novelty[index] > novelty[index + 1]
                && novelty[index] >= threshold
        })
        .map(|index| (index, novelty[index]))
        .collect();
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    let mut selected = vec![0usize];
    for (candidate, _) in candidates {
        if selected
            .iter()
            .all(|existing| existing.abs_diff(candidate) >= min_gap)
        {
            selected.push(snap_to_downbeat(frames, candidate, min_gap / 2));
        }
    }
    // `frames.len()` is a sentinel boundary so the final analysis frame is
    // included in the last section. Add it before splitting long spans so a
    // homogeneous full track still receives useful narrative subdivisions.
    // The sentinel itself is never passed to `snap_to_downbeat`.
    let final_frame = frames.len();
    if selected.last().copied().unwrap_or(0) < final_frame {
        selected.push(final_frame);
    }
    selected.sort_unstable();
    selected.dedup();

    // Long homogeneous spans are split on the nearest downbeat so the visual
    // director still has room for gradual narrative evolution. The explicit
    // progress guard prevents an infinite loop when downbeat snapping returns
    // an existing boundary.
    let mut cursor = 0usize;
    while cursor + 1 < selected.len() {
        let left = selected[cursor];
        let right = selected[cursor + 1];
        if right.saturating_sub(left) > max_gap {
            let target = (left + max_gap).min(final_frame.saturating_sub(1));
            let split = snap_to_downbeat(frames, target, min_gap);
            if split > left && split < right && !selected.contains(&split) {
                selected.push(split);
                selected.sort_unstable();
            } else {
                cursor += 1;
            }
        } else {
            cursor += 1;
        }
    }

    let mut raw_sections = Vec::new();
    for pair in selected.windows(2) {
        let start = pair[0];
        let end = pair[1].max(start + 1).min(frames.len());
        raw_sections.push(section_features(frames, start, end));
    }
    if raw_sections.is_empty() {
        raw_sections.push(section_features(frames, 0, frames.len()));
    }
    assign_repeat_groups(&mut raw_sections);

    let intensities: Vec<f32> = raw_sections
        .iter()
        .map(|section| section.intensity)
        .collect();
    let minimum = intensities.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = intensities
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let denominator = (maximum - minimum).max(1.0e-5);
    let total = raw_sections.len().max(1);
    let mut cues = Vec::with_capacity(raw_sections.len());
    for (index, section) in raw_sections.into_iter().enumerate() {
        let normalized_intensity = ((section.intensity - minimum) / denominator).clamp(0.0, 1.0);
        let position = index as f32 / total.saturating_sub(1).max(1) as f32;
        let repeat_bonus = if section.repeat_group > 0 {
            0.08 * position
        } else {
            0.0
        };
        let narrative_rank =
            (0.72 * normalized_intensity + 0.20 * position + repeat_bonus).clamp(0.0, 1.0);
        let average_bpm = section.average_bpm.max(40.0);
        let beats_of_anticipation = match section.scene {
            SceneKind::Drop => 8.0,
            SceneKind::Intense => 4.0,
            SceneKind::Transition | SceneKind::Build => 4.0,
            _ => 2.0,
        };
        let anticipation_samples =
            (beats_of_anticipation * 60.0 / average_bpm * sample_rate as f32) as u64;
        let blackout_strength = if matches!(section.scene, SceneKind::Drop)
            || (matches!(section.scene, SceneKind::Intense) && narrative_rank > 0.72)
        {
            (0.24 + 0.38 * narrative_rank).clamp(0.0, 0.66)
        } else {
            0.0
        };
        let start_sample = frames[section.start_frame]
            .sample_index
            .saturating_sub(hop_size as u64);
        let end_sample = if section.end_frame >= frames.len() {
            duration_samples
        } else {
            frames[section.end_frame].sample_index
        }
        .max(start_sample + 1);
        cues.push(SectionCue {
            start_sample,
            end_sample,
            scene: section.scene,
            repeat_group: section.repeat_group,
            intensity: normalized_intensity,
            palette_hue: section.palette_hue,
            palette_chroma: section.tonal_confidence,
            spatial_mode: choose_spatial_mode(section.scene, section.repeat_group, index),
            motif_seed: hash_u32(
                (section.repeat_group as u32).wrapping_mul(0x9e37_79b9)
                    ^ index as u32
                    ^ start_sample as u32,
            ),
            anticipation_samples,
            blackout_strength,
            narrative_rank,
        });
    }
    if let Some(first) = cues.first_mut() {
        first.start_sample = 0;
    }
    if let Some(last) = cues.last_mut() {
        last.end_sample = duration_samples.max(last.start_sample + 1);
    }
    for index in 1..cues.len() {
        let boundary = cues[index].start_sample;
        cues[index - 1].end_sample = boundary.max(cues[index - 1].start_sample + 1);
    }
    cues
}

#[derive(Clone)]
struct RawSection {
    start_frame: usize,
    end_frame: usize,
    scene: SceneKind,
    intensity: f32,
    average_bpm: f32,
    palette_hue: f32,
    tonal_confidence: f32,
    signature: [f32; NUM_BANDS + NUM_CHROMA + 2],
    repeat_group: u16,
}

fn section_features(frames: &[MusicFrame], start: usize, end: usize) -> RawSection {
    let end = end.max(start + 1).min(frames.len());
    let slice = &frames[start..end];
    let mut signature = [0.0_f32; NUM_BANDS + NUM_CHROMA + 2];
    let mut intensity = 0.0;
    let mut onset_density = 0.0;
    let mut buildup = 0.0;
    let mut drop = 0.0_f32;
    let mut bpm = 0.0;
    let mut tonal_confidence = 0.0;
    for frame in slice {
        intensity += frame.loudness;
        onset_density += (frame.flux + frame.impact).clamp(0.0, 1.0);
        buildup += frame.buildup;
        drop = drop.max(frame.drop);
        bpm += frame.bpm;
        tonal_confidence += frame.tonal_confidence;
        for (destination, value) in signature[..NUM_BANDS].iter_mut().zip(frame.bands) {
            *destination += value;
        }
        for (destination, value) in signature[NUM_BANDS..NUM_BANDS + NUM_CHROMA]
            .iter_mut()
            .zip(frame.chroma)
        {
            *destination += value;
        }
    }
    let scale = 1.0 / slice.len() as f32;
    for value in &mut signature {
        *value *= scale;
    }
    intensity *= scale;
    onset_density *= scale;
    buildup *= scale;
    bpm *= scale;
    tonal_confidence *= scale;
    signature[NUM_BANDS + NUM_CHROMA] = intensity;
    signature[NUM_BANDS + NUM_CHROMA + 1] = onset_density;
    let scene = if intensity < 0.06 {
        SceneKind::Silence
    } else if drop > 0.30 {
        SceneKind::Drop
    } else if buildup > 0.50 {
        SceneKind::Build
    } else if intensity > 0.60 || onset_density > 0.53 {
        SceneKind::Intense
    } else if intensity < 0.24 && onset_density < 0.24 {
        SceneKind::Calm
    } else if slice
        .iter()
        .map(|frame| frame.novelty)
        .fold(0.0_f32, f32::max)
        > 0.55
    {
        SceneKind::Transition
    } else {
        SceneKind::Groove
    };
    let (palette_hue, concentration) = chroma_hue(&signature[NUM_BANDS..NUM_BANDS + NUM_CHROMA]);
    RawSection {
        start_frame: start,
        end_frame: end,
        scene,
        intensity: (0.72 * intensity + 0.28 * onset_density).clamp(0.0, 1.0),
        average_bpm: bpm,
        palette_hue,
        tonal_confidence: (tonal_confidence * concentration).clamp(0.0, 1.0),
        signature,
        repeat_group: 0,
    }
}

fn assign_repeat_groups(sections: &mut [RawSection]) {
    let mut next_group = 1_u16;
    for index in 0..sections.len() {
        if sections[index].repeat_group != 0 {
            continue;
        }
        let mut matches = Vec::new();
        for other in index + 1..sections.len() {
            let duration_left = sections[index]
                .end_frame
                .saturating_sub(sections[index].start_frame);
            let duration_right = sections[other]
                .end_frame
                .saturating_sub(sections[other].start_frame);
            let duration_ratio = duration_left.min(duration_right) as f32
                / duration_left.max(duration_right).max(1) as f32;
            let similarity =
                cosine_similarity(&sections[index].signature, &sections[other].signature);
            if duration_ratio > 0.58 && similarity > 0.90 {
                matches.push(other);
            }
        }
        if !matches.is_empty() {
            sections[index].repeat_group = next_group;
            for matched in matches {
                sections[matched].repeat_group = next_group;
            }
            next_group = next_group.saturating_add(1);
        }
    }
}

fn apply_section_narrative(frames: &mut [MusicFrame], sections: &mut [SectionCue]) {
    let frame_count = frames.len();
    for section in sections.iter() {
        let start = frames.partition_point(|frame| frame.sample_index < section.start_sample);
        let end = frames.partition_point(|frame| frame.sample_index < section.end_sample);
        let bounded_start = start.min(frame_count);
        let bounded_end = end.min(frame_count);
        for frame in &mut frames[bounded_start..bounded_end] {
            if !matches!(frame.scene, SceneKind::Drop | SceneKind::Build) {
                frame.scene = section.scene;
            }
            // Preserve moment-to-moment level while giving repeated/final
            // sections an intentional long-range intensity hierarchy.
            frame.loudness =
                (frame.loudness * (0.92 + 0.08 * section.narrative_rank)).clamp(0.0, 1.0);
        }
    }

    // A repeated motif keeps the same palette. Later repetitions receive a
    // slight controlled hue evolution rather than an unrelated new color.
    for index in 0..sections.len() {
        let group = sections[index].repeat_group;
        if group == 0 {
            continue;
        }
        if let Some(first_index) = sections
            .iter()
            .position(|candidate| candidate.repeat_group == group)
        {
            let occurrence = sections[..=index]
                .iter()
                .filter(|candidate| candidate.repeat_group == group)
                .count()
                .saturating_sub(1);
            let base_hue = sections[first_index].palette_hue;
            let base_mode = sections[first_index].spatial_mode;
            let base_seed = sections[first_index].motif_seed;
            sections[index].palette_hue = wrap01(base_hue + occurrence as f32 * 0.018);
            sections[index].spatial_mode = base_mode;
            sections[index].motif_seed = base_seed;
        }
    }
}

fn collect_accents(frames: &[MusicFrame]) -> Vec<AccentEvent> {
    let mut accents = Vec::with_capacity(frames.len() / 2);
    for frame in frames {
        if frame.onset_low > 0.05 {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: AccentKind::Low,
                strength: frame.onset_low,
            });
        }
        if frame.onset_mid > 0.05 {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: AccentKind::Mid,
                strength: frame.onset_mid,
            });
        }
        if frame.onset_high > 0.05 {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: AccentKind::High,
                strength: frame.onset_high,
            });
        }
        if frame.impact > 0.10 {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: AccentKind::Impact,
                strength: frame.impact,
            });
        }
        if frame.beat_event {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: if frame.downbeat_event {
                    AccentKind::Downbeat
                } else {
                    AccentKind::Beat
                },
                strength: frame.beat_confidence,
            });
        }
        if frame.drop > 0.20 {
            accents.push(AccentEvent {
                sample_index: frame.sample_index,
                kind: AccentKind::Drop,
                strength: frame.drop,
            });
        }
    }
    accents.sort_by_key(|accent| accent.sample_index);
    accents
}

fn choose_spatial_mode(scene: SceneKind, repeat_group: u16, index: usize) -> SpatialMode {
    match scene {
        SceneKind::Silence => SpatialMode::Uniform,
        SceneKind::Calm => {
            if index % 2 == 0 {
                SpatialMode::Mirror
            } else {
                SpatialMode::Wave
            }
        }
        SceneKind::Groove => {
            if repeat_group % 2 == 0 {
                SpatialMode::Wave
            } else {
                SpatialMode::Zones
            }
        }
        SceneKind::Intense => {
            if index % 3 == 0 {
                SpatialMode::Zones
            } else {
                SpatialMode::Radial
            }
        }
        SceneKind::Build => SpatialMode::Spiral,
        SceneKind::Drop => SpatialMode::Radial,
        SceneKind::Transition => SpatialMode::Sparkle,
    }
}

fn snap_to_downbeat(frames: &[MusicFrame], target: usize, radius: usize) -> usize {
    let start = target.saturating_sub(radius);
    let end = (target + radius + 1).min(frames.len());
    let mut best: Option<(usize, usize, f32)> = None;
    for (index, frame) in frames.iter().enumerate().take(end).skip(start) {
        if frame.downbeat_event || (frame.beat_event && frame.beat_in_bar == 0) {
            let distance = target.abs_diff(index);
            let confidence = frame.downbeat_confidence + frame.impact * 0.20;
            if best.is_none_or(|current| {
                distance < current.1 || (distance == current.1 && confidence > current.2)
            }) {
                best = Some((index, distance, confidence));
            }
        }
    }
    best.map_or(target.min(frames.len().saturating_sub(1)), |value| value.0)
}

fn centered_triangle_smooth(values: &[f32], radius: usize) -> Vec<f32> {
    if values.is_empty() || radius == 0 {
        return values.to_vec();
    }
    let mut output = vec![0.0; values.len()];
    for (index, destination) in output.iter_mut().enumerate() {
        let start = index.saturating_sub(radius);
        let end = (index + radius + 1).min(values.len());
        let mut sum = 0.0;
        let mut weight_sum = 0.0;
        for (source, &value) in values.iter().enumerate().take(end).skip(start) {
            let distance = index.abs_diff(source) as f32;
            let weight = (radius + 1) as f32 - distance;
            sum += value * weight;
            weight_sum += weight;
        }
        *destination = sum / weight_sum.max(1.0e-9);
    }
    output
}

fn prefix_sum(values: &[f32]) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(values.len() + 1);
    prefix.push(0.0);
    for &value in values {
        let next = prefix.last().copied().unwrap_or(0.0) + value;
        prefix.push(next);
    }
    prefix
}

fn range_mean(prefix: &[f32], start: usize, end: usize) -> f32 {
    let end = end.min(prefix.len().saturating_sub(1));
    let start = start.min(end);
    if end == start {
        0.0
    } else {
        (prefix[end] - prefix[start]) / (end - start) as f32
    }
}

fn average(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn robust_median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut copy = values.to_vec();
    copy.sort_by(f32::total_cmp);
    let middle = copy.len() / 2;
    if copy.len() % 2 == 0 {
        Some((copy[middle - 1] + copy[middle]) * 0.5)
    } else {
        Some(copy[middle])
    }
}

fn chroma_hue(values: &[f32]) -> (f32, f32) {
    let mut x: f32 = 0.0;
    let mut y: f32 = 0.0;
    let mut total: f32 = 0.0;
    for (pitch, &value) in values.iter().take(NUM_CHROMA).enumerate() {
        let position = (pitch * 7) % NUM_CHROMA;
        let angle = core::f32::consts::TAU * position as f32 / NUM_CHROMA as f32;
        x += value * angle.cos();
        y += value * angle.sin();
        total += value;
    }
    if total <= 1.0e-9 {
        (0.72, 0.0)
    } else {
        (
            wrap01(y.atan2(x) / core::f32::consts::TAU),
            (x.mul_add(x, y * y).sqrt() / total).clamp(0.0, 1.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_click_track(sample_rate: u32, seconds: f32, bpm: f32) -> Vec<f32> {
        let count = (sample_rate as f32 * seconds) as usize;
        let mut samples = vec![0.0_f32; count];
        let period = (sample_rate as f32 * 60.0 / bpm) as usize;
        for beat in (0..count).step_by(period.max(1)) {
            for offset in 0..(sample_rate as usize / 100).min(count - beat) {
                let envelope = (-(offset as f32) / (sample_rate as f32 * 0.003)).exp();
                samples[beat + offset] +=
                    envelope * if beat / period % 4 == 0 { 0.9 } else { 0.55 };
            }
        }
        samples
    }

    #[test]
    fn homogeneous_tracks_receive_safe_narrative_splits() {
        let sample_rate = 48_000_u32;
        let hop = 256_usize;
        let duration_samples = u64::from(sample_rate) * 65;
        let frame_count = duration_samples as usize / hop;
        let mut frames = Vec::with_capacity(frame_count);
        for index in 0..frame_count {
            let mut frame = MusicFrame::default();
            frame.sample_index = ((index + 1) * hop) as u64;
            frame.time_seconds = frame.sample_index as f64 / sample_rate as f64;
            frame.loudness = 0.24;
            frame.bpm = 120.0;
            frame.scene = SceneKind::Calm;
            frames.push(frame);
        }
        let sections = build_sections(&frames, sample_rate, hop, duration_samples);
        assert!(sections.len() >= 3);
        assert_eq!(sections.first().unwrap().start_sample, 0);
        assert_eq!(sections.last().unwrap().end_sample, duration_samples);
        assert!(sections
            .windows(2)
            .all(|pair| pair[0].end_sample == pair[1].start_sample));
    }

    #[test]
    fn builds_full_track_score() {
        let sample_rate = 48_000;
        let samples = synthetic_click_track(sample_rate, 12.0, 120.0);
        let analyzer = FullTrackAnalyzer::default();
        let score = analyzer.analyze_pcm(&samples, sample_rate, 1).unwrap();
        assert!(!score.frames.is_empty());
        assert!(!score.sections.is_empty());
        assert!(score.duration_samples > 0);
        let median_bpm = robust_median(
            &score
                .frames
                .iter()
                .map(|frame| frame.bpm)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!((median_bpm - 120.0).abs() < 8.0);
    }
}
