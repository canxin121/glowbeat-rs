//! Stable C ABI for mobile, desktop, game-engine, and other language bindings.
//!
//! See `include/glowbeat.h`. All exported functions validate null pointers and
//! return negative status codes on failure. Opaque handles must not be used
//! concurrently without synchronization supplied by the caller.

use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;

use crate::config::{EngineConfig, OutputMode, RenderConfig};
use crate::offline::FullTrackAnalyzer;
use crate::score::{LightingScore, ScorePlayer};
use crate::streaming::StreamingEngine;
use crate::types::{LightPoint, Rgba};

/// ABI-compatible engine construction settings.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GlowbeatFfiConfig {
    /// Input sample rate for the streaming engine.
    pub sample_rate: u32,
    /// Interleaved input channels for the streaming engine.
    pub channels: u32,
    /// Maximum number of light points rendered per call.
    pub light_capacity: u32,
    /// Zero for alpha, one for a physical amber channel.
    pub output_mode: u32,
    /// Nominal render FPS.
    pub render_fps: f32,
    /// Maximum output brightness.
    pub max_brightness: f32,
    /// Output gamma.
    pub output_gamma: f32,
    /// Stable hue offset in turns.
    pub hue_offset: f32,
    /// Warm RGB energy transferred into a physical amber channel.
    pub amber_mix: f32,
    /// Global saturation multiplier.
    pub saturation: f32,
    /// Beat-synchronous layer strength.
    pub beat_strength: f32,
    /// Transient-accent layer strength.
    pub accent_strength: f32,
    /// Per-light spatial variation strength.
    pub spatial_strength: f32,
    /// Maximum per-render luminance step.
    pub max_luma_step: f32,
}

impl Default for GlowbeatFfiConfig {
    fn default() -> Self {
        let config = EngineConfig::default();
        Self {
            sample_rate: config.analysis.sample_rate,
            channels: config.analysis.channels as u32,
            light_capacity: 1_024,
            output_mode: config.render.output_mode as u32,
            render_fps: config.render.frames_per_second,
            max_brightness: config.render.max_brightness,
            output_gamma: config.render.output_gamma,
            hue_offset: config.render.hue_offset,
            amber_mix: config.render.amber_mix,
            saturation: config.render.saturation,
            beat_strength: config.render.beat_strength,
            accent_strength: config.render.accent_strength,
            spatial_strength: config.render.spatial_strength,
            max_luma_step: config.render.max_luma_step,
        }
    }
}

/// Compact processing report for the C ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GlowbeatFfiReport {
    /// Complete input sample frames consumed.
    pub sample_frames: u64,
    /// Analysis frames generated.
    pub analysis_frames: u64,
    /// Current absolute sample-clock position.
    pub sample_index: u64,
    /// Trailing scalar samples ignored.
    pub trailing_samples: u64,
}

/// Compact real-time status for diagnostics and UI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GlowbeatFfiStatus {
    /// Current sample-clock position.
    pub sample_index: u64,
    /// Perceptual loudness proxy.
    pub loudness: f32,
    /// Estimated BPM.
    pub bpm: f32,
    /// Beat phase in `[0, 1)`.
    pub beat_phase: f32,
    /// Beat confidence.
    pub beat_confidence: f32,
    /// Buildup score.
    pub buildup: f32,
    /// Confirmed drop pulse.
    pub drop: f32,
    /// Scene enum value.
    pub scene: u32,
}

/// Metadata for an analyzed full-track score.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GlowbeatFfiScoreInfo {
    /// Audio sample rate.
    pub sample_rate: u32,
    /// Original channel count.
    pub channels: u32,
    /// Dense analysis hop in samples.
    pub analysis_hop: u32,
    /// Reserved for ABI-compatible extensions.
    pub reserved: u32,
    /// Duration in audio sample frames.
    pub duration_samples: u64,
    /// Number of dense music frames.
    pub frame_count: u64,
    /// Number of narrative sections.
    pub section_count: u64,
    /// Number of sparse accent events.
    pub accent_count: u64,
}

/// Opaque heap-allocated streaming engine used by the C ABI.
pub struct GlowbeatEngineHandle {
    engine: StreamingEngine,
}

/// Opaque heap-allocated full-track score used by the C ABI.
pub struct GlowbeatScoreHandle {
    score: LightingScore,
}

/// Opaque heap-allocated full-track score player used by the C ABI.
pub struct GlowbeatPlayerHandle {
    player: ScorePlayer,
}

/// Returns the ABI version implemented by this build.
#[no_mangle]
pub extern "C" fn glowbeat_abi_version() -> u32 {
    1
}

/// Returns recommended construction defaults.
#[no_mangle]
pub extern "C" fn glowbeat_default_config() -> GlowbeatFfiConfig {
    GlowbeatFfiConfig::default()
}

/// Creates a causal streaming engine or returns null for invalid settings.
#[no_mangle]
pub extern "C" fn glowbeat_engine_create(
    ffi_config: GlowbeatFfiConfig,
) -> *mut GlowbeatEngineHandle {
    catch_unwind(AssertUnwindSafe(|| {
        let Some((config, light_capacity)) = streaming_config_from_ffi(ffi_config) else {
            return core::ptr::null_mut();
        };
        let Ok(engine) = StreamingEngine::new(config, light_capacity) else {
            return core::ptr::null_mut();
        };
        Box::into_raw(Box::new(GlowbeatEngineHandle { engine }))
    }))
    .unwrap_or(core::ptr::null_mut())
}

/// Destroys a streaming engine. Passing null is allowed.
///
/// # Safety
///
/// A non-null `handle` must have been returned by [`glowbeat_engine_create`], must not have been
/// destroyed previously, and must not be accessed concurrently during this call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_engine_destroy(handle: *mut GlowbeatEngineHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The caller promises that `handle` was returned by create and
        // has not previously been destroyed.
        unsafe { drop(Box::from_raw(handle)) };
    }));
}

/// Pushes interleaved PCM. Returns zero on success or a negative error code.
///
/// # Safety
///
/// `handle` must be a live engine handle with exclusive access for this call. When
/// `sample_count > 0`, `samples` must reference that many readable `f32` values. A non-null
/// `report` must reference writable, properly aligned storage.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_engine_push_f32(
    handle: *mut GlowbeatEngineHandle,
    samples: *const f32,
    sample_count: usize,
    report: *mut GlowbeatFfiReport,
) -> i32 {
    if handle.is_null() || (sample_count > 0 && samples.is_null()) {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Pointers and lengths are validated above; the caller owns the
        // buffers for the duration of this call.
        let engine = unsafe { &mut *handle };
        let input: &[f32] = if sample_count == 0 {
            &[]
        } else {
            // SAFETY: Validated non-null pointer and caller-provided length.
            unsafe { slice::from_raw_parts(samples, sample_count) }
        };
        let result = engine.engine.push_interleaved(input);
        if !report.is_null() {
            // SAFETY: Non-null output pointer supplied by the caller.
            unsafe {
                *report = GlowbeatFfiReport {
                    sample_frames: result.input_frames as u64,
                    analysis_frames: result.output_frames as u64,
                    sample_index: result.sample_index,
                    trailing_samples: (sample_count % engine.engine.config().analysis.channels)
                        as u64,
                };
            }
        }
        0
    }))
    .unwrap_or(-9)
}

/// Renders streaming state at an absolute audio sample-clock position.
///
/// # Safety
///
/// `handle` must be a live engine handle with exclusive access. When `point_count > 0`, `points`
/// must reference that many readable points and `output` that many writable, non-overlapping
/// colors. All buffers must remain valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_engine_render(
    handle: *mut GlowbeatEngineHandle,
    sample_index: u64,
    points: *const LightPoint,
    point_count: usize,
    output: *mut Rgba,
) -> i32 {
    if handle.is_null() || (point_count > 0 && (points.is_null() || output.is_null())) {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Pointer validity is checked above.
        let engine = unsafe { &mut *handle };
        let point_slice = unsafe { const_slice_or_empty(points, point_count) };
        let output_slice = unsafe { mut_slice_or_empty(output, point_count) };
        match engine
            .engine
            .render_at(sample_index, point_slice, output_slice)
        {
            Ok(()) => 0,
            Err(_) => -2,
        }
    }))
    .unwrap_or(-9)
}

/// Reads current streaming music status. Returns zero on success.
///
/// # Safety
///
/// `handle` must be a live engine handle that is not mutated concurrently, and `status` must
/// reference writable, properly aligned storage for one [`GlowbeatFfiStatus`].
#[no_mangle]
pub unsafe extern "C" fn glowbeat_engine_status(
    handle: *const GlowbeatEngineHandle,
    status: *mut GlowbeatFfiStatus,
) -> i32 {
    if handle.is_null() || status.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Both pointers were validated and remain borrowed only during
        // this call.
        let handle = unsafe { &*handle };
        let frame = handle.engine.latest_frame().cloned().unwrap_or_default();
        // SAFETY: Valid non-null output pointer supplied by the caller.
        unsafe {
            *status = GlowbeatFfiStatus {
                sample_index: handle.engine.sample_index(),
                loudness: frame.loudness,
                bpm: frame.bpm,
                beat_phase: frame.beat_phase,
                beat_confidence: frame.beat_confidence,
                buildup: frame.buildup,
                drop: frame.drop,
                scene: frame.scene as u32,
            };
        }
        0
    }))
    .unwrap_or(-9)
}

/// Analyzes a complete interleaved `f32` track and returns an owned score.
///
/// # Safety
///
/// `samples` must reference `sample_count` readable `f32` values and remain valid for this call.
/// The buffer must not be mutated concurrently. The returned handle, when non-null, must be
/// released exactly once with [`glowbeat_score_destroy`].
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_analyze_f32(
    samples: *const f32,
    sample_count: usize,
    sample_rate: u32,
    channels: u32,
) -> *mut GlowbeatScoreHandle {
    if samples.is_null() || sample_count == 0 || channels == 0 {
        return core::ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(channels_usize) = usize::try_from(channels) else {
            return core::ptr::null_mut();
        };
        if sample_count % channels_usize != 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: The pointer is non-null and valid for `sample_count` elements
        // for the duration of this synchronous call.
        let input = unsafe { slice::from_raw_parts(samples, sample_count) };
        let analyzer = FullTrackAnalyzer::default();
        let Ok(score) = analyzer.analyze_pcm(input, sample_rate, channels_usize) else {
            return core::ptr::null_mut();
        };
        Box::into_raw(Box::new(GlowbeatScoreHandle { score }))
    }))
    .unwrap_or(core::ptr::null_mut())
}

/// Deserializes a portable `.gbs` score and returns an owned handle.
///
/// # Safety
///
/// `bytes` must reference `byte_count` readable bytes and remain valid for this call. The returned
/// handle, when non-null, must be released exactly once with [`glowbeat_score_destroy`].
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_deserialize(
    bytes: *const u8,
    byte_count: usize,
) -> *mut GlowbeatScoreHandle {
    if bytes.is_null() || byte_count == 0 {
        return core::ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The caller guarantees a readable buffer for this call.
        let input = unsafe { slice::from_raw_parts(bytes, byte_count) };
        let mut cursor = Cursor::new(input);
        let Ok(score) = LightingScore::read_from(&mut cursor) else {
            return core::ptr::null_mut();
        };
        Box::into_raw(Box::new(GlowbeatScoreHandle { score }))
    }))
    .unwrap_or(core::ptr::null_mut())
}

/// Destroys a score handle. Passing null is allowed.
///
/// # Safety
///
/// A non-null `handle` must be an owned handle returned by a GlowBeat score constructor, must not
/// have been destroyed previously, and must not be accessed concurrently during this call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_destroy(handle: *mut GlowbeatScoreHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The pointer came from a GlowBeat score constructor and has not
        // already been destroyed.
        unsafe { drop(Box::from_raw(handle)) };
    }));
}

/// Reads immutable score metadata. Returns zero on success.
///
/// # Safety
///
/// `handle` must be a live score handle that is not destroyed concurrently, and `info` must
/// reference writable, properly aligned storage for one [`GlowbeatFfiScoreInfo`].
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_info(
    handle: *const GlowbeatScoreHandle,
    info: *mut GlowbeatFfiScoreInfo,
) -> i32 {
    if handle.is_null() || info.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Both pointers were checked for null and are used only here.
        let score = unsafe { &(*handle).score };
        // SAFETY: `info` is a non-null writable out pointer.
        unsafe {
            *info = GlowbeatFfiScoreInfo {
                sample_rate: score.sample_rate,
                channels: u32::from(score.channels),
                analysis_hop: score.analysis_hop,
                reserved: 0,
                duration_samples: score.duration_samples,
                frame_count: score.frames.len() as u64,
                section_count: score.sections.len() as u64,
                accent_count: score.accents.len() as u64,
            };
        }
        0
    }))
    .unwrap_or(-9)
}

/// Computes the serialized score size in bytes. Returns zero on success.
///
/// # Safety
///
/// `handle` must be a live score handle that is not destroyed concurrently, and `byte_count` must
/// reference writable, properly aligned storage for one `usize`.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_serialized_size(
    handle: *const GlowbeatScoreHandle,
    byte_count: *mut usize,
) -> i32 {
    if handle.is_null() || byte_count.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Both pointers are non-null and borrowed for this call.
        let score = unsafe { &(*handle).score };
        let Ok(size) = score.serialized_len() else {
            return -4;
        };
        // SAFETY: `byte_count` is a valid writable out pointer.
        unsafe { *byte_count = size };
        0
    }))
    .unwrap_or(-9)
}

/// Serializes a score into caller-owned memory.
///
/// # Safety
///
/// `handle` must be a live score handle. `written` must reference writable storage for one
/// `usize`. When `capacity > 0`, `output` must reference at least `capacity` writable bytes and
/// must not overlap `written` or the score allocation.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_score_serialize(
    handle: *const GlowbeatScoreHandle,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if handle.is_null() || written.is_null() || (capacity > 0 && output.is_null()) {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Handle and out pointer are non-null for this call.
        let score = unsafe { &(*handle).score };
        let Ok(required) = score.serialized_len() else {
            return -4;
        };
        // SAFETY: Writable out pointer checked above.
        unsafe { *written = required };
        if capacity < required || output.is_null() {
            return -2;
        }
        // SAFETY: Caller supplied at least `required` writable bytes.
        let output_slice = unsafe { slice::from_raw_parts_mut(output, required) };
        let mut cursor = Cursor::new(output_slice);
        if score.write_to(&mut cursor).is_err() {
            return -4;
        }
        0
    }))
    .unwrap_or(-9)
}

/// Creates a sample-clock full-track player by cloning the score.
///
/// # Safety
///
/// `score` must be a live score handle that is not destroyed or mutated during this call. The
/// returned player, when non-null, must be released exactly once with
/// [`glowbeat_player_destroy`].
#[no_mangle]
pub unsafe extern "C" fn glowbeat_player_create(
    score: *const GlowbeatScoreHandle,
    ffi_config: GlowbeatFfiConfig,
) -> *mut GlowbeatPlayerHandle {
    if score.is_null() {
        return core::ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let Some(render_config) = render_config_from_ffi(ffi_config) else {
            return core::ptr::null_mut();
        };
        let Ok(light_capacity) = usize::try_from(ffi_config.light_capacity) else {
            return core::ptr::null_mut();
        };
        if light_capacity == 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: Non-null immutable score handle for this call.
        let score_clone = unsafe { (*score).score.clone() };
        let Ok(player) = ScorePlayer::new(score_clone, render_config, light_capacity) else {
            return core::ptr::null_mut();
        };
        Box::into_raw(Box::new(GlowbeatPlayerHandle { player }))
    }))
    .unwrap_or(core::ptr::null_mut())
}

/// Destroys a full-track player. Passing null is allowed.
///
/// # Safety
///
/// A non-null `handle` must have been returned by [`glowbeat_player_create`], must not have been
/// destroyed previously, and must not be accessed concurrently during this call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_player_destroy(handle: *mut GlowbeatPlayerHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The pointer came from player creation and is uniquely owned.
        unsafe { drop(Box::from_raw(handle)) };
    }));
}

/// Seeks a full-track player to an absolute audio sample position.
///
/// # Safety
///
/// `handle` must be a live player handle with exclusive access for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_player_seek(
    handle: *mut GlowbeatPlayerHandle,
    sample_index: u64,
) -> i32 {
    if handle.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Non-null uniquely borrowed player handle.
        let player = unsafe { &mut (*handle).player };
        match player.seek(sample_index) {
            Ok(()) => 0,
            Err(_) => -5,
        }
    }))
    .unwrap_or(-9)
}

/// Renders full-track state at an absolute audio sample-clock position.
///
/// # Safety
///
/// `handle` must be a live player handle with exclusive access. When `point_count > 0`, `points`
/// must reference that many readable points and `output` that many writable, non-overlapping
/// colors. All buffers must remain valid for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn glowbeat_player_render(
    handle: *mut GlowbeatPlayerHandle,
    sample_index: u64,
    points: *const LightPoint,
    point_count: usize,
    output: *mut Rgba,
) -> i32 {
    if handle.is_null() || (point_count > 0 && (points.is_null() || output.is_null())) {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Pointers and lengths were validated above.
        let player = unsafe { &mut (*handle).player };
        let point_slice = unsafe { const_slice_or_empty(points, point_count) };
        let output_slice = unsafe { mut_slice_or_empty(output, point_count) };
        match player.render(sample_index, point_slice, output_slice) {
            Ok(()) => 0,
            Err(_) => -5,
        }
    }))
    .unwrap_or(-9)
}

fn streaming_config_from_ffi(config: GlowbeatFfiConfig) -> Option<(EngineConfig, usize)> {
    let channels = usize::try_from(config.channels).ok()?;
    let light_capacity = usize::try_from(config.light_capacity).ok()?;
    if channels == 0 || light_capacity == 0 {
        return None;
    }
    let mut engine = EngineConfig::default();
    engine.analysis.sample_rate = config.sample_rate;
    engine.analysis.channels = channels;
    let nyquist_limit = config.sample_rate as f32 * 0.5 - 1.0;
    engine.analysis.max_frequency_hz = engine
        .analysis
        .max_frequency_hz
        .min(nyquist_limit)
        .max(engine.analysis.min_frequency_hz + 1.0);
    engine.render = render_config_from_ffi(config)?;
    engine.validate().ok()?;
    Some((engine, light_capacity))
}

fn render_config_from_ffi(config: GlowbeatFfiConfig) -> Option<RenderConfig> {
    let output_mode = match config.output_mode {
        0 => OutputMode::Alpha,
        1 => OutputMode::Amber,
        _ => return None,
    };
    let render = RenderConfig {
        frames_per_second: config.render_fps,
        max_brightness: config.max_brightness,
        output_gamma: config.output_gamma,
        output_mode,
        amber_mix: config.amber_mix,
        hue_offset: config.hue_offset,
        saturation: config.saturation,
        beat_strength: config.beat_strength,
        accent_strength: config.accent_strength,
        spatial_strength: config.spatial_strength,
        max_luma_step: config.max_luma_step,
    };
    render.validate().ok()?;
    Some(render)
}

unsafe fn const_slice_or_empty<'a, T>(pointer: *const T, length: usize) -> &'a [T] {
    if length == 0 {
        &[]
    } else {
        // SAFETY: Every caller validates non-null and caller-owned range.
        unsafe { slice::from_raw_parts(pointer, length) }
    }
}

unsafe fn mut_slice_or_empty<'a, T>(pointer: *mut T, length: usize) -> &'a mut [T] {
    if length == 0 {
        &mut []
    } else {
        // SAFETY: Every caller validates non-null and caller-owned writable range.
        unsafe { slice::from_raw_parts_mut(pointer, length) }
    }
}
