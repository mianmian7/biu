//! Native Rust audio playback engine (Symphonia + CPAL + ffmpeg fallback + PlayQueue + Souvlaki).

pub mod decoder;
pub mod engine;
pub mod media_controls;
pub mod queue;
pub mod resampler;
mod ring;
pub mod source;
pub mod spectrum;

#[cfg(test)]
pub(crate) mod test_util;

pub use decoder::{Decoder, DecoderBackend, DecoderOptions, open_decoder};
pub use engine::{
    AudioEngine, AudioEvent, EngineConfig, OutputMode, PlaybackSnapshot, PlaybackState,
};
pub use media_controls::{MediaControlAction, MediaControlsBridge, MediaControlsConfig};
pub use queue::{PlayQueue, Removed};
pub use source::{AudioSource, GrowingFileMediaSource};
pub use spectrum::{SPECTRUM_BARS, SpectrumAnalyzer};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
