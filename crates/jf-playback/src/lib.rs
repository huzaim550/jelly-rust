//! Playback: direct play, ffmpeg HLS transcoding, subtitles, and images.
//!
//! ffmpeg is driven as a subprocess rather than through libav bindings: it
//! matches how Jellyfin works, avoids an ABI-coupled C dependency, and lets a
//! user point at their own ffmpeg build via `FFMPEG_PATH`.

pub mod hls;
pub mod image;
pub mod subtitle;
pub mod transcode;

pub use hls::{HlsSession, HlsSessionManager};
pub use image::{ImageCache, ImageRequest};
pub use subtitle::{SubtitleKind, SubtitleRequest};
pub use transcode::{ClientCapabilities, DirectStream, PlaybackPlan, PlayMethod, StreamSelection, Transcoder};

/// Locate the ffmpeg/ffprobe binaries.
///
/// Prefers `FFMPEG_PATH`/`FFPROBE_PATH` so users can supply their own build,
/// then falls back to `PATH`.
pub fn resolve_binaries() -> (String, String) {
    let ffmpeg = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned());
    let ffprobe = std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".to_owned());
    (ffmpeg, ffprobe)
}