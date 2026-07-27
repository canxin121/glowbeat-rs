//! Feed arbitrary interleaved PCM blocks into the causal engine.

use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = EngineConfig::default();
    config.analysis.sample_rate = 48_000;
    config.analysis.channels = 2;

    let lights = LightPoint::ring(64);
    let mut rgba = vec![Rgba::default(); lights.len()];
    let mut engine = StreamingEngine::new(config, lights.len())?;

    // Replace this synthetic callback block with PCM from microphone,
    // ScreenCaptureKit, WASAPI loopback, PipeWire, Android playback capture,
    // ReplayKit, a decoder, or a network stream.
    let callback_pcm = vec![0.0_f32; 256 * 2];
    for _ in 0..200 {
        engine.push_interleaved(&callback_pcm);
        engine.render_now(&lights, &mut rgba)?;
        // Send `rgba` to the application's lamp transport here.
    }
    println!("latest music state: {:?}", engine.latest_frame());
    println!(
        "first light: {:?}",
        rgba.first().copied().unwrap_or_default()
    );
    Ok(())
}
