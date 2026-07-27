//! Allocation-stable JNI bridge for Android's live microphone and playback capture paths.

use std::sync::Mutex;

use jni::objects::{JClass, JFloatArray, JShortArray};
use jni::sys::{jfloat, jint, jlong};
use jni::EnvUnowned;

use crate::{LiveEngine, LiveSettings};

const RESULT_LENGTH: usize = 14;

struct AndroidRealtimeEngine {
    engine: LiveEngine,
    pcm_i16: Vec<i16>,
    pcm_f32: Vec<f32>,
    result: [f32; RESULT_LENGTH],
}

impl AndroidRealtimeEngine {
    fn new(sample_rate: u32) -> Option<Self> {
        let engine = LiveEngine::new_mono(sample_rate).ok()?;
        Some(Self {
            engine,
            pcm_i16: Vec::new(),
            pcm_f32: Vec::new(),
            result: [0.0; RESULT_LENGTH],
        })
    }

    fn process(&mut self, mode: i32, sensitivity: f32) -> bool {
        self.pcm_f32.clear();
        self.pcm_f32
            .extend(self.pcm_i16.iter().map(|sample| *sample as f32 / 32_768.0));
        let Some(frame) = self
            .engine
            .process_mono(&self.pcm_f32, LiveSettings::from_wire(mode, sensitivity))
        else {
            return false;
        };

        self.result = [
            frame.color.r,
            frame.color.g,
            frame.color.b,
            frame.metrics.peak,
            frame.metrics.loudness,
            frame.metrics.low,
            frame.metrics.mid,
            frame.metrics.high,
            frame.metrics.spectral_flux,
            frame.metrics.impact,
            f32::from(frame.metrics.beat_event),
            frame.metrics.bpm,
            frame.metrics.beat_confidence,
            frame.metrics.musical_event,
        ];
        true
    }
}

#[no_mangle]
pub extern "system" fn Java_com_gemlight_app_audio_GlowBeatNative_nativeCreate(
    _env: EnvUnowned,
    _class: JClass,
    sample_rate: jint,
) -> jlong {
    if sample_rate <= 0 {
        return 0;
    }
    AndroidRealtimeEngine::new(sample_rate as u32)
        .map(|engine| Box::into_raw(Box::new(Mutex::new(engine))) as jlong)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_com_gemlight_app_audio_GlowBeatNative_nativeDestroy(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: Kotlin owns one handle and closes it exactly once after its capture loop stops.
        unsafe { drop(Box::from_raw(handle as *mut Mutex<AndroidRealtimeEngine>)) };
    }
}

#[no_mangle]
pub extern "system" fn Java_com_gemlight_app_audio_GlowBeatNative_nativeProcess(
    mut unowned_env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    input: JShortArray,
    sample_count: jint,
    mode: jint,
    sensitivity: jfloat,
    output: JFloatArray,
) -> jint {
    unowned_env
        .with_env(|env| -> Result<jint, jni::errors::Error> {
            if handle == 0 || sample_count <= 0 || sample_count as usize > 32_768 {
                return Ok(-1);
            }
            // SAFETY: The handle remains owned by Kotlin for this synchronous call.
            let mutex = unsafe { &*(handle as *const Mutex<AndroidRealtimeEngine>) };
            let Ok(mut engine) = mutex.lock() else {
                return Ok(-2);
            };
            let count = sample_count as usize;
            engine.pcm_i16.resize(count, 0);
            input.get_region(env, 0, &mut engine.pcm_i16)?;
            if !engine.process(mode, sensitivity) {
                return Ok(0);
            }
            output.set_region(env, 0, &engine.result)?;
            Ok(1)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
