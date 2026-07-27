//! Analyze a complete WAVE file, cache a score, and render from sample clock.

use glowbeat::wav::read_wav;
use glowbeat::{AnalysisConfig, FullTrackAnalyzer, LightPoint, RenderConfig, Rgba, ScorePlayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: cargo run --example full_track -- input.wav")?;
    let audio = read_wav(path)?;
    let mut analysis = AnalysisConfig::default();
    analysis.sample_rate = audio.sample_rate;
    analysis.channels = audio.channels as usize;
    analysis.max_frequency_hz = analysis
        .max_frequency_hz
        .min(audio.sample_rate as f32 * 0.5 - 1.0);
    let analyzer = FullTrackAnalyzer::new(analysis)?;
    let score = analyzer.analyze_pcm(&audio.samples, audio.sample_rate, audio.channels as usize)?;
    score.save("track.gbs")?;

    let lights = LightPoint::line(128);
    let mut rgba = vec![Rgba::default(); lights.len()];
    let mut player = ScorePlayer::new(score, RenderConfig::default(), lights.len())?;

    // In an embedded player, use its actual audio sample position here.
    for sample_position in (0..audio.sample_frames() as u64).step_by(800) {
        player.render(sample_position, &lights, &mut rgba)?;
        // Send `rgba` with a render timestamp to the lamps.
    }
    println!("rendered {} lights", rgba.len());
    Ok(())
}
