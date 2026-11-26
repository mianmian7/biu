//! Native Rust audio playback engine (Symphonia + CPAL + ffmpeg fallback + PlayQueue + Souvlaki).

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
