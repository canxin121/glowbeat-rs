use glowbeat::{
    AnalysisConfig, FullTrackAnalyzer, LightPoint, LightingScore, RenderConfig, Rgba, ScorePlayer,
};

fn test_song(sample_rate: u32, seconds: usize) -> Vec<f32> {
    let frames = sample_rate as usize * seconds;
    let mut output = vec![0.0_f32; frames];
    for (frame, sample) in output.iter_mut().enumerate() {
        let time = frame as f32 / sample_rate as f32;
        let section_gain = if time < seconds as f32 * 0.5 {
            0.35
        } else {
            0.78
        };
        let phase = (time * 2.0).fract();
        let kick = if phase < 0.08 {
            (-phase / 0.018).exp()
        } else {
            0.0
        };
        let harmony = 0.18 * (core::f32::consts::TAU * 220.0 * time).sin()
            + 0.10 * (core::f32::consts::TAU * 329.63 * time).sin();
        *sample = (section_gain * (0.75 * kick + harmony)).clamp(-1.0, 1.0);
    }
    output
}

#[test]
fn full_track_analysis_serializes_and_seeks() {
    let sample_rate = 24_000;
    let samples = test_song(sample_rate, 12);
    let config = AnalysisConfig {
        sample_rate,
        channels: 1,
        max_frequency_hz: 10_000.0,
        ..AnalysisConfig::default()
    };
    let analyzer = FullTrackAnalyzer::new(config).unwrap();
    let score = analyzer.analyze_pcm(&samples, sample_rate, 1).unwrap();
    assert!(!score.frames.is_empty());
    assert!(!score.sections.is_empty());

    let mut bytes = Vec::new();
    score.write_to(&mut bytes).unwrap();
    let restored = LightingScore::read_from(&mut bytes.as_slice()).unwrap();
    assert_eq!(score, restored);

    let points = LightPoint::ring(48);
    let mut rgba = vec![Rgba::default(); points.len()];
    let mut player = ScorePlayer::new(restored, RenderConfig::default(), points.len()).unwrap();
    player
        .render(sample_rate as u64 * 8, &points, &mut rgba)
        .unwrap();
    player
        .render(sample_rate as u64 * 2, &points, &mut rgba)
        .unwrap();
    assert!(rgba.iter().all(|value| value.is_finite()));
}
