#![cfg(feature = "ffi")]

use core::mem::{align_of, size_of};

use glowbeat::ffi::*;
use glowbeat::{LightPoint, Rgba};

#[test]
fn c_layout_sizes_are_stable() {
    assert_eq!(size_of::<GlowbeatFfiConfig>(), 56);
    assert_eq!(align_of::<GlowbeatFfiConfig>(), 4);
    assert_eq!(size_of::<GlowbeatFfiReport>(), 32);
    assert_eq!(size_of::<GlowbeatFfiStatus>(), 40);
    assert_eq!(size_of::<GlowbeatFfiScoreInfo>(), 48);
    assert_eq!(size_of::<LightPoint>(), 20);
    assert_eq!(size_of::<Rgba>(), 16);
}

#[test]
fn streaming_and_full_track_ffi_lifecycle() {
    let mut config = glowbeat_default_config();
    config.sample_rate = 48_000;
    config.channels = 1;
    config.light_capacity = 8;

    let engine = glowbeat_engine_create(config);
    assert!(!engine.is_null());
    let audio = synthetic_clicks(48_000, 4.0, 120.0);
    let mut report = GlowbeatFfiReport::default();
    assert_eq!(
        unsafe { glowbeat_engine_push_f32(engine, audio.as_ptr(), audio.len(), &mut report) },
        0
    );
    assert!(report.analysis_frames > 0);

    let points = LightPoint::ring(8);
    let mut colors = vec![Rgba::default(); points.len()];
    assert_eq!(
        unsafe {
            glowbeat_engine_render(
                engine,
                report.sample_index,
                points.as_ptr(),
                points.len(),
                colors.as_mut_ptr(),
            )
        },
        0
    );
    assert!(colors.iter().all(|value| value.is_finite()));
    let mut status = GlowbeatFfiStatus::default();
    assert_eq!(unsafe { glowbeat_engine_status(engine, &mut status) }, 0);
    unsafe { glowbeat_engine_destroy(engine) };

    let score = unsafe { glowbeat_score_analyze_f32(audio.as_ptr(), audio.len(), 48_000, 1) };
    assert!(!score.is_null());
    let mut info = GlowbeatFfiScoreInfo::default();
    assert_eq!(unsafe { glowbeat_score_info(score, &mut info) }, 0);
    assert_eq!(info.sample_rate, 48_000);
    assert!(info.frame_count > 0);

    let mut byte_count = 0usize;
    assert_eq!(
        unsafe { glowbeat_score_serialized_size(score, &mut byte_count) },
        0
    );
    assert!(byte_count > 64);
    let mut bytes = vec![0_u8; byte_count];
    let mut written = 0usize;
    assert_eq!(
        unsafe { glowbeat_score_serialize(score, bytes.as_mut_ptr(), bytes.len(), &mut written,) },
        0
    );
    assert_eq!(written, bytes.len());

    let restored = unsafe { glowbeat_score_deserialize(bytes.as_ptr(), bytes.len()) };
    assert!(!restored.is_null());
    let player = unsafe { glowbeat_player_create(restored, config) };
    assert!(!player.is_null());
    assert_eq!(
        unsafe {
            glowbeat_player_render(
                player,
                48_000,
                points.as_ptr(),
                points.len(),
                colors.as_mut_ptr(),
            )
        },
        0
    );
    assert_eq!(unsafe { glowbeat_player_seek(player, 0) }, 0);

    unsafe {
        glowbeat_player_destroy(player);
        glowbeat_score_destroy(restored);
        glowbeat_score_destroy(score);
    }
}

fn synthetic_clicks(sample_rate: u32, seconds: f32, bpm: f32) -> Vec<f32> {
    let count = (sample_rate as f32 * seconds) as usize;
    let period = (sample_rate as f32 * 60.0 / bpm) as usize;
    let mut audio = vec![0.0_f32; count];
    for beat in (0..count).step_by(period.max(1)) {
        let length = (sample_rate as usize / 80).min(count - beat);
        for offset in 0..length {
            let envelope = (-(offset as f32) / (sample_rate as f32 * 0.003)).exp();
            audio[beat + offset] += 0.85 * envelope;
        }
    }
    audio
}
