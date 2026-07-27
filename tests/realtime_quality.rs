use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

fn click_track(sample_rate: u32, seconds: f32, bpm: f32, channels: usize) -> Vec<f32> {
    let frames = (sample_rate as f32 * seconds) as usize;
    let period = sample_rate as f32 * 60.0 / bpm;
    let mut output = vec![0.0_f32; frames * channels];
    for frame in 0..frames {
        let beat_position = frame as f32 / period;
        let phase = beat_position.fract();
        let beat = beat_position.floor() as usize;
        let envelope = if phase < 0.11 {
            (-phase / 0.023).exp() * if beat % 4 == 0 { 0.9 } else { 0.55 }
        } else {
            0.0
        };
        let tone = (core::f32::consts::TAU * 75.0 * frame as f32 / sample_rate as f32).sin();
        for channel in 0..channels {
            output[frame * channels + channel] = envelope * tone;
        }
    }
    output
}

#[test]
fn streaming_tracks_regular_pulse_and_renders_safely() {
    let mut config = EngineConfig::default();
    config.analysis.channels = 2;
    let sample_rate = config.analysis.sample_rate;
    let pcm = click_track(sample_rate, 14.0, 120.0, 2);
    let points = LightPoint::ring(96);
    let mut rgba = vec![Rgba::default(); points.len()];
    let mut engine = StreamingEngine::new(config, points.len()).unwrap();
    for chunk in pcm.chunks(256 * 2) {
        engine.push_interleaved(chunk);
        engine.render_now(&points, &mut rgba).unwrap();
    }
    let frame = engine.latest_frame().expect("analysis frame");
    assert!(frame.bpm.is_finite());
    assert!((70.0..=190.0).contains(&frame.bpm));
    assert!(frame.beat_phase.is_finite());
    assert!(rgba.iter().all(|value| value.is_finite()));
    assert!(rgba.iter().all(|value| {
        [value.r, value.g, value.b, value.a]
            .iter()
            .all(|channel| (0.0..=1.0).contains(channel))
    }));
}

#[test]
fn non_finite_input_does_not_poison_output() {
    let points = LightPoint::line(8);
    let mut output = vec![Rgba::default(); 8];
    let mut engine = StreamingEngine::new(EngineConfig::default(), 8).unwrap();
    let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0];
    for _ in 0..2_048 {
        engine.push_interleaved(&input);
    }
    engine.render_now(&points, &mut output).unwrap();
    assert!(output.iter().all(|value| value.is_finite()));
}
