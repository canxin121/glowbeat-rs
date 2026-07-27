//! Demonstrates latency compensation using an absolute audio sample clock.

use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EngineConfig::default();
    let sample_rate = config.analysis.sample_rate;
    let lights = LightPoint::ring(32);
    let mut output = vec![Rgba::default(); lights.len()];
    let mut engine = StreamingEngine::new(config, lights.len())?;

    engine.push_interleaved(&vec![0.0; 512 * 2]);
    let measured_transport_latency_seconds = 0.018_f32;
    let render_ahead = (sample_rate as f32 * measured_transport_latency_seconds) as u64;
    let render_at = engine.sample_index().saturating_add(render_ahead);
    engine.render_at(render_at, &lights, &mut output)?;
    println!("render timestamp: {render_at} samples");
    Ok(())
}
