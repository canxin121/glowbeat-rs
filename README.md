# GlowBeat

GlowBeat is a low-latency Rust music-to-light engine for live PCM streams and
complete tracks. It analyzes rhythm, spectrum, harmony, musical energy, and
transients, then renders bounded RGBA lighting values.

## Features

- Causal streaming analysis for microphones, playback capture, and live audio.
- Full-track analysis, portable `.gbs` scores, and sample-clock playback.
- Beat tracking, BPM, beat phase, 24 logarithmic bands, chroma, buildup, and drop detection.
- Perceptual color rendering with alpha or physical amber output.
- `LiveEngine` presets for GEM pulse, spectrum, and aurora effects.
- Rust API, C ABI, Android JNI bridge, Swift and C# bindings, CPAL, and WebAssembly adapters.

## Quick start

Rust 1.97.1 is used for development. The core crate supports Rust 1.85 and later.

```bash
cargo test --all-targets --all-features
cargo build --release --all-features
```

```rust
use glowbeat::{LiveEngine, LiveSettings};

let mut engine = LiveEngine::new_mono(48_000)?;
let frame = engine.process_mono(&pcm, LiveSettings::default());
```

For a lower-level integration, push interleaved `f32` PCM into
`StreamingEngine` and render with your own `LightPoint` layout.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
