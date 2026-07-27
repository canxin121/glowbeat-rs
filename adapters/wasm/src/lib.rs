//! WebAssembly bindings for browser AudioWorklet/player integrations.
//!
//! The fast path exposes stable pointers to preallocated input/output buffers.
//! JavaScript writes PCM into WebAssembly memory, calls `push_input()`, calls
//! `render()`, and reads flattened RGBA without per-frame Rust allocation.

use glowbeat::{
    AnalysisConfig, EngineConfig, FullTrackAnalyzer, LightPoint, LightingScore, RenderConfig, Rgba,
    ScorePlayer, StreamingEngine,
};
use wasm_bindgen::prelude::*;

fn js_error(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn validate_layout_index(index: usize, len: usize) -> Result<(), JsValue> {
    if index < len {
        Ok(())
    } else {
        Err(JsValue::from_str("light index is out of range"))
    }
}

fn flatten_rgba(source: &[Rgba], destination: &mut [f32]) {
    for (rgba, chunk) in source.iter().zip(destination.chunks_exact_mut(4)) {
        chunk.copy_from_slice(&[rgba.r, rgba.g, rgba.b, rgba.a]);
    }
}

fn fill_ring_layout(lights: &mut [LightPoint]) {
    if lights.is_empty() {
        return;
    }
    let count = lights.len() as f32;
    for (index, light) in lights.iter_mut().enumerate() {
        let t = index as f32 / count;
        let angle = core::f32::consts::TAU * t;
        *light = LightPoint::new(index as u32, angle.cos(), angle.sin(), 0.0, t);
    }
}

fn fill_line_layout(lights: &mut [LightPoint]) {
    match lights.len() {
        0 => {}
        1 => lights[0] = LightPoint::new(0, 0.0, 0.0, 0.0, 0.5),
        count => {
            for (index, light) in lights.iter_mut().enumerate() {
                let t = index as f32 / (count - 1) as f32;
                *light = LightPoint::new(index as u32, t * 2.0 - 1.0, 0.0, 0.0, t);
            }
        }
    }
}

/// Causal unknown-audio engine for browser callbacks and AudioWorklets.
#[wasm_bindgen]
pub struct WasmStreamingEngine {
    engine: StreamingEngine,
    lights: Vec<LightPoint>,
    rgba: Vec<Rgba>,
    flat_output: Vec<f32>,
    input: Vec<f32>,
}

#[wasm_bindgen]
impl WasmStreamingEngine {
    /// Creates a streaming engine with a ring light layout.
    ///
    /// `input_capacity` is the maximum scalar sample count written before one
    /// `push_input()` call, not the number of interleaved sample frames.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sample_rate: u32,
        channels: usize,
        light_count: usize,
        input_capacity: usize,
    ) -> Result<WasmStreamingEngine, JsValue> {
        if light_count == 0 || input_capacity == 0 {
            return Err(JsValue::from_str(
                "light_count and input_capacity must be greater than zero",
            ));
        }
        let mut config = EngineConfig::default();
        config.analysis.sample_rate = sample_rate;
        config.analysis.channels = channels;
        config.analysis.max_frequency_hz = config
            .analysis
            .max_frequency_hz
            .min(sample_rate as f32 * 0.5 - 1.0);
        let lights = LightPoint::ring(light_count);
        let mut engine = StreamingEngine::new(config, light_count).map_err(js_error)?;
        let maximum_sample_frames = input_capacity / channels.max(1) + 1;
        engine.reserve_input_sample_frames(maximum_sample_frames);
        Ok(Self {
            engine,
            lights,
            rgba: vec![Rgba::default(); light_count],
            flat_output: vec![0.0; light_count.saturating_mul(4)],
            input: vec![0.0; input_capacity],
        })
    }

    /// Pointer to the preallocated scalar PCM input buffer in WebAssembly memory.
    #[must_use]
    pub fn input_ptr(&mut self) -> usize {
        self.input.as_mut_ptr() as usize
    }

    /// Maximum scalar sample count accepted by [`Self::push_input`].
    #[must_use]
    pub fn input_capacity(&self) -> usize {
        self.input.len()
    }

    /// Processes PCM already written to `input_ptr()`.
    pub fn push_input(&mut self, sample_count: usize) -> Result<u64, JsValue> {
        if sample_count > self.input.len() {
            return Err(JsValue::from_str("sample_count exceeds input capacity"));
        }
        let report = self.engine.push_interleaved(&self.input[..sample_count]);
        Ok(report.sample_index)
    }

    /// Convenience path that copies a JavaScript `Float32Array` into the engine.
    pub fn push_pcm(&mut self, samples: &[f32]) -> u64 {
        self.engine.push_interleaved(samples).sample_index
    }

    /// Renders at an absolute audio sample-clock position into `output_ptr()`.
    pub fn render(&mut self, sample_index: u64) -> Result<(), JsValue> {
        self.engine
            .render_at(sample_index, &self.lights, &mut self.rgba)
            .map_err(js_error)?;
        flatten_rgba(&self.rgba, &mut self.flat_output);
        Ok(())
    }

    /// Renders at the latest consumed sample-clock position.
    pub fn render_now(&mut self) -> Result<(), JsValue> {
        let sample_index = self.engine.sample_index();
        self.render(sample_index)
    }

    /// Pointer to flattened `r,g,b,a` `f32` output in WebAssembly memory.
    #[must_use]
    pub fn output_ptr(&self) -> usize {
        self.flat_output.as_ptr() as usize
    }

    /// Number of `f32` values at [`Self::output_ptr`].
    #[must_use]
    pub fn output_len(&self) -> usize {
        self.flat_output.len()
    }

    /// Convenience copy of flattened RGBA output.
    #[must_use]
    pub fn output_copy(&self) -> Vec<f32> {
        self.flat_output.clone()
    }

    /// Replaces one light's spatial coordinate and stable identifier.
    pub fn set_light(
        &mut self,
        index: usize,
        id: u32,
        x: f32,
        y: f32,
        z: f32,
        zone: f32,
    ) -> Result<(), JsValue> {
        validate_layout_index(index, self.lights.len())?;
        self.lights[index] = LightPoint::new(id, x, y, z, zone);
        Ok(())
    }

    /// Restores a ring layout without reallocating the output buffers.
    pub fn use_ring_layout(&mut self) {
        fill_ring_layout(&mut self.lights);
    }

    /// Uses a centered line layout without reallocating the output buffers.
    pub fn use_line_layout(&mut self) {
        fill_line_layout(&mut self.lights);
    }

    /// Current absolute sample-clock position.
    #[must_use]
    pub fn sample_index(&self) -> u64 {
        self.engine.sample_index()
    }

    /// Current BPM estimate.
    #[must_use]
    pub fn bpm(&self) -> f32 {
        self.engine.latest_frame().map_or(0.0, |frame| frame.bpm)
    }

    /// Current continuous beat phase.
    #[must_use]
    pub fn beat_phase(&self) -> f32 {
        self.engine
            .latest_frame()
            .map_or(0.0, |frame| frame.beat_phase)
    }

    /// Current beat confidence.
    #[must_use]
    pub fn beat_confidence(&self) -> f32 {
        self.engine
            .latest_frame()
            .map_or(0.0, |frame| frame.beat_confidence)
    }

    /// Current loudness proxy.
    #[must_use]
    pub fn loudness(&self) -> f32 {
        self.engine
            .latest_frame()
            .map_or(0.0, |frame| frame.loudness)
    }
}

/// Serializable non-causal analysis result for a complete song.
#[wasm_bindgen]
pub struct WasmLightingScore {
    score: LightingScore,
}

#[wasm_bindgen]
impl WasmLightingScore {
    /// Performs full-track analysis on complete interleaved PCM.
    pub fn analyze(
        samples: &[f32],
        sample_rate: u32,
        channels: usize,
    ) -> Result<WasmLightingScore, JsValue> {
        let mut config = AnalysisConfig::default();
        config.sample_rate = sample_rate;
        config.channels = channels;
        config.max_frequency_hz = config.max_frequency_hz.min(sample_rate as f32 * 0.5 - 1.0);
        let analyzer = FullTrackAnalyzer::new(config).map_err(js_error)?;
        let score = analyzer
            .analyze_pcm(samples, sample_rate, channels)
            .map_err(js_error)?;
        Ok(Self { score })
    }

    /// Restores a cached `.gbs` score.
    pub fn deserialize(bytes: &[u8]) -> Result<WasmLightingScore, JsValue> {
        let mut cursor = std::io::Cursor::new(bytes);
        let score = LightingScore::read_from(&mut cursor).map_err(js_error)?;
        Ok(Self { score })
    }

    /// Serializes this score in portable `.gbs` form.
    pub fn serialize(&self) -> Result<Vec<u8>, JsValue> {
        let mut bytes = Vec::new();
        self.score.write_to(&mut bytes).map_err(js_error)?;
        Ok(bytes)
    }

    /// Audio sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.score.sample_rate
    }

    /// Track duration in sample frames.
    #[must_use]
    pub fn duration_samples(&self) -> u64 {
        self.score.duration_samples
    }

    /// Number of generated high-level sections.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.score.sections.len()
    }
}

/// Sample-clock renderer for a [`WasmLightingScore`].
#[wasm_bindgen]
pub struct WasmScorePlayer {
    player: ScorePlayer,
    lights: Vec<LightPoint>,
    rgba: Vec<Rgba>,
    flat_output: Vec<f32>,
}

#[wasm_bindgen]
impl WasmScorePlayer {
    /// Creates a score player with a ring light layout.
    #[wasm_bindgen(constructor)]
    pub fn new(score: &WasmLightingScore, light_count: usize) -> Result<WasmScorePlayer, JsValue> {
        if light_count == 0 {
            return Err(JsValue::from_str("light_count must be greater than zero"));
        }
        let player = ScorePlayer::new(score.score.clone(), RenderConfig::default(), light_count)
            .map_err(js_error)?;
        Ok(Self {
            player,
            lights: LightPoint::ring(light_count),
            rgba: vec![Rgba::default(); light_count],
            flat_output: vec![0.0; light_count.saturating_mul(4)],
        })
    }

    /// Resets temporal state after a player seek.
    pub fn seek(&mut self, sample_index: u64) -> Result<(), JsValue> {
        self.player.seek(sample_index).map_err(js_error)
    }

    /// Renders at the embedded player's absolute audio sample position.
    pub fn render(&mut self, sample_index: u64) -> Result<(), JsValue> {
        self.player
            .render(sample_index, &self.lights, &mut self.rgba)
            .map_err(js_error)?;
        flatten_rgba(&self.rgba, &mut self.flat_output);
        Ok(())
    }

    /// Pointer to flattened `r,g,b,a` output in WebAssembly memory.
    #[must_use]
    pub fn output_ptr(&self) -> usize {
        self.flat_output.as_ptr() as usize
    }

    /// Number of `f32` values at [`Self::output_ptr`].
    #[must_use]
    pub fn output_len(&self) -> usize {
        self.flat_output.len()
    }

    /// Convenience copy of flattened RGBA output.
    #[must_use]
    pub fn output_copy(&self) -> Vec<f32> {
        self.flat_output.clone()
    }

    /// Replaces one light's spatial coordinate and stable identifier.
    pub fn set_light(
        &mut self,
        index: usize,
        id: u32,
        x: f32,
        y: f32,
        z: f32,
        zone: f32,
    ) -> Result<(), JsValue> {
        validate_layout_index(index, self.lights.len())?;
        self.lights[index] = LightPoint::new(id, x, y, z, zone);
        Ok(())
    }

    /// Restores a ring layout without reallocating renderer buffers.
    pub fn use_ring_layout(&mut self) {
        fill_ring_layout(&mut self.lights);
    }

    /// Uses a centered line layout without reallocating renderer buffers.
    pub fn use_line_layout(&mut self) {
        fill_line_layout(&mut self.lights);
    }
}
