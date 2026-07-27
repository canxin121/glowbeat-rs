//! Render a custom two-zone venue layout.

use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut points = Vec::new();
    for row in 0..12 {
        for column in 0..20 {
            let x = column as f32 / 19.0 * 2.0 - 1.0;
            let y = row as f32 / 11.0 * 2.0 - 1.0;
            let zone = if column < 10 { 0.25 } else { 0.75 };
            points.push(LightPoint::new((row * 20 + column) as u32, x, y, 0.0, zone));
        }
    }
    let mut engine = StreamingEngine::new(EngineConfig::default(), points.len())?;
    let mut output = vec![Rgba::default(); points.len()];
    engine.push_interleaved(&vec![0.0; 4_096]);
    engine.render_now(&points, &mut output)?;
    println!("rendered a {}-point venue field", output.len());
    Ok(())
}
