use std::hint::black_box;
use std::time::Instant;

use glowbeat::{EngineConfig, LightPoint, Rgba, StreamingEngine};

fn main() {
    let points = LightPoint::ring(1_024);
    let mut output = vec![Rgba::default(); points.len()];
    let mut engine = StreamingEngine::new(EngineConfig::default(), points.len()).unwrap();
    let pcm = vec![0.0_f32; 256 * 2];
    let iterations = 10_000_u64;
    let start = Instant::now();
    for _ in 0..iterations {
        engine.push_interleaved(black_box(&pcm));
        engine
            .render_now(black_box(&points), black_box(&mut output))
            .unwrap();
    }
    let elapsed = start.elapsed();
    let audio_seconds = iterations as f64 * 256.0 / 48_000.0;
    let realtime_factor = audio_seconds / elapsed.as_secs_f64();
    println!("processed {audio_seconds:.2}s audio + 1024 lights in {elapsed:?}");
    println!("realtime factor: {realtime_factor:.2}x");
    println!(
        "last pixel: {:?}",
        output.last().copied().unwrap_or_default()
    );

    if let Ok(value) = std::env::var("GLOWBEAT_MIN_REALTIME_FACTOR") {
        let minimum = value.parse::<f64>().unwrap_or_else(|_| {
            eprintln!("GLOWBEAT_MIN_REALTIME_FACTOR must be a finite number");
            std::process::exit(2);
        });
        if !minimum.is_finite() || minimum <= 0.0 {
            eprintln!("GLOWBEAT_MIN_REALTIME_FACTOR must be greater than zero");
            std::process::exit(2);
        }
        if realtime_factor < minimum {
            eprintln!("performance regression: {realtime_factor:.2}x < required {minimum:.2}x");
            std::process::exit(1);
        }
    }
}
