//! GlowBeat is a cross-platform music-to-light engine written in pure Rust.
//!
//! It supports two production paths:
//! - [`StreamingEngine`]: causal, low-latency PCM analysis for microphone,
//!   screen-capture audio, live streams, and arbitrary external audio callbacks.
//! - [`FullTrackAnalyzer`]: non-causal analysis and anticipatory lighting scores
//!   when the complete song is available to an embedded player.
//!
//! The DSP and renderer have no third-party dependencies. Platform audio I/O is
//! intentionally kept outside the real-time core: applications push interleaved
//! `f32` PCM into the engine from their own audio callback.

#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod config;
pub mod director;
pub mod dsp;
pub mod live;
pub mod math;
pub mod offline;
pub mod protocol;
pub mod score;
pub mod streaming;
pub mod types;
pub mod wav;

#[cfg(all(target_os = "android", feature = "android-jni"))]
mod android_jni;

#[cfg(feature = "ffi")]
pub mod ffi;

pub use config::{AnalysisConfig, EngineConfig, OutputMode, RenderConfig};
pub use director::{LightDirector, RenderContext, RenderError};
pub use live::{LiveEngine, LiveFrame, LiveMetrics, LiveSettings, LiveStyle};
pub use offline::{FullTrackAnalyzer, OfflineError};
pub use score::{LightingScore, ScoreIoError, ScorePlayer, ScorePlayerError, SectionCue};
pub use streaming::{ProcessReport, StreamingEngine};
pub use types::{
    AccentEvent, AccentKind, LightPoint, MusicFrame, Rgba, SceneKind, SpatialMode, NUM_BANDS,
    NUM_CHROMA,
};
