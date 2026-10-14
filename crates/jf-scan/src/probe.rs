//! ffprobe wrapper: reads container format and stream layout.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("ffprobe not found on PATH")]
    NotFound,
    #[error("ffprobe failed for {path}: {stderr}")]
    Failed { path: String, stderr: String },
    #[error("could not parse ffprobe output: {0}")]
    Parse(String),
}

/// One stream inside a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub index: i32,
    pub stream_type: StreamType,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub channels: Option<i32>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<i32>,
    pub bitrate: Option<i64>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
}

impl StreamType {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamType::Video => "Video",
            StreamType::Audio => "Audio",
            StreamType::Subtitle => "Subtitle",
            StreamType::Data => "Data",
            StreamType::Attachment => "Attachment",
        }
    }

    fn from_codec_type(raw: &str) -> Option<Self> {
        match raw {
            "video" => Some(StreamType::Video),
            "audio" => Some(StreamType::Audio),
            "subtitle" => Some(StreamType::Subtitle),
            "data" => Some(StreamType::Data),
            "attachment" => Some(StreamType::Attachment),
            _ => None,
        }
    }
}

/// Map an ffprobe container alias to an extension-style name.
///
/// Only used as a fallback when the file has no extension; the extension is
/// authoritative because it is what the client actually receives.
pub fn normalise_container(format_name: &str) -> String {
    // ffprobe reports aliases as a comma-separated list; the first one is the
    // format's own name and the rest are compatible spellings.
    let first = format_name
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    match first.as_str() {
        "matroska" | "webm" => "mkv".to_owned(),
        "mpegts" => "ts".to_owned(),
        // An unknown name passes through unchanged rather than being mangled.
        other => other.to_owned(),
    }
}

/// Result of probing one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub path: String,
    pub container: Option<String>,
    pub duration_secs: Option<f64>,
    pub size: Option<i64>,
    pub bitrate: Option<i64>,
    pub streams: Vec<StreamInfo>,
}

/// ffprobe JSON output for one stream.
#[derive(Debug, Deserialize)]
struct RawStream {
    index: i32,
    codec_type: String,
    codec_name: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    channels: Option<i32>,
    channel_layout: Option<String>,
    sample_rate: Option<String>,
    bit_rate: Option<String>,
    disposition: Option<RawDisposition>,
    tags: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct RawDisposition {
    default: Option<u8>,
    forced: Option<u8>,
    attached_pic: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    filename: Option<String>,
    format_name: Option<String>,
    duration: Option<String>,
    size: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawProbe {
    streams: Vec<RawStream>,
    format: RawFormat,
}

/// Runs ffprobe against files.
#[derive(Debug, Clone)]
pub struct Prober {
    binary: String,
    timeout: Duration,
}

impl Default for Prober {
    fn default() -> Self {
        Self::new()
    }
}

impl Prober {
    /// Use the `ffprobe` found on `PATH`.
    pub fn new() -> Self {
        Self { binary: "ffprobe".into(), timeout: Duration::from_secs(30) }
    }

    /// Use a specific ffprobe binary (for bundled builds).
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self { binary: binary.into(), timeout: Duration::from_secs(30) }
    }

    /// Probe one file, returning `Ok(None)` if it is not a media container.
    pub fn probe(&self, path: &Path) -> Result<ProbeResult, ProbeError> {
        if !path.is_file() {
            return Ok(ProbeResult {
                path: path.to_string_lossy().into_owned(),
                container: None,
                duration_secs: None,
                size: None,
                bitrate: None,
                streams: Vec::new(),
            });
        }

        let output = Command::new(&self.binary)
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ])
            .arg(path)
            .output()
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                ProbeError::NotFound
            } else {
                ProbeError::Failed {
                    path: path.to_string_lossy().into_owned(),
                    stderr: e.to_string(),
                }
            })?;

        // ffprobe exits non-zero for unreadable files but still prints JSON.
        let raw: RawProbe = if output.stdout.is_empty() {
            return Err(ProbeError::Failed {
                path: path.to_string_lossy().into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        } else {
            serde_json::from_slice(&output.stdout)
                .map_err(|e| ProbeError::Parse(e.to_string()))?
        };

        Ok(self.build_result(path, raw))
    }

    /// Probe, treating any failure as "not a media file".
    pub fn probe_lossy(&self, path: &Path) -> Option<ProbeResult> {
        match self.probe(path) {
            Ok(r) if r.container.is_some() => Some(r),
            _ => None,
        }
    }

    fn build_result(&self, path: &Path, raw: RawProbe) -> ProbeResult {
        let streams = raw
            .streams
            .into_iter()
            .filter_map(|s| {
                let stream_type = StreamType::from_codec_type(&s.codec_type)?;
                // Cover art arrives as an attached_pic video stream; drop it so
                // it is not mistaken for a real video track.
                if stream_type == StreamType::Video
                    && s.disposition.as_ref().and_then(|d| d.attached_pic).unwrap_or(0) != 0
                {
                    return None;
                }
                let tags = s.tags.unwrap_or_default();
                let lang = tags
                    .get("language")
                    .or_else(|| tags.get("LANGUAGE"))
                    .cloned();

                Some(StreamInfo {
                    index: s.index,
                    stream_type,
                    codec: s.codec_name,
                    language: lang,
                    title: tags.get("title").or_else(|| tags.get("TITLE")).cloned(),
                    width: s.width,
                    height: s.height,
                    channels: s.channels,
                    channel_layout: s.channel_layout,
                    sample_rate: s.sample_rate.as_deref().and_then(|v| v.parse().ok()),
                    bitrate: s.bit_rate.as_deref().and_then(|v| v.parse().ok()),
                    is_default: s
                        .disposition
                        .as_ref()
                        .and_then(|d| d.default)
                        .unwrap_or(0)
                        != 0,
                    is_forced: s
                        .disposition
                        .as_ref()
                        .and_then(|d| d.forced)
                        .unwrap_or(0)
                        != 0,
                    is_external: false,
                })
            })
            .collect();

        // Use the file extension as the container name.
        //
        // ffprobe's `format_name` is a comma-separated list of aliases
        // ("matroska,webm"), which matches nothing in a client's direct-play
        // list — those are extension names like "mkv". Taking the first alias
        // meant every .mkv file was judged unplayable and forced a transcode.
        let container = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .or_else(|| {
                raw.format.format_name.as_ref().and_then(|f| {
                    f.split(',')
                        .next()
                        .map(|alias| normalise_container(alias))
                })
            });

        ProbeResult {
            path: path.to_string_lossy().into_owned(),
            container,
            duration_secs: raw.format.duration.as_deref().and_then(|d| d.parse().ok()),
            size: raw.format.size.as_deref().and_then(|s| s.parse().ok()),
            bitrate: raw.format.bit_rate.as_deref().and_then(|s| s.parse().ok()),
            streams,
        }
    }
}

impl ProbeResult {
    /// Convert to the DTO shape stored in `items.media_sources_json`.
    pub fn to_media_source_info(&self) -> jf_core::MediaSourceInfo {
        use jf_core::{MediaProtocol, MediaStream, MediaStreamType};

        let streams = self
            .streams
            .iter()
            .map(|s| MediaStream {
                codec: s.codec.clone(),
                stream_type: match s.stream_type {
                    StreamType::Video => MediaStreamType::Video,
                    StreamType::Audio => MediaStreamType::Audio,
                    StreamType::Subtitle => MediaStreamType::Subtitle,
                    _ => MediaStreamType::Data,
                },
                index: s.index,
                width: s.width,
                height: s.height,
                channels: s.channels,
                language: s.language.clone(),
                is_default: s.is_default,
                is_forced: s.is_forced,
                is_external: s.is_external,
            })
            .collect();

        jf_core::MediaSourceInfo {
            id: jf_core::new_item_id(),
            protocol: MediaProtocol::File,
            path: self.path.clone(),
            container: self.container.clone(),
            size: self.size,
            run_time_ticks: self.duration_secs.map(jf_core::secs_to_ticks).unwrap_or(0),
            media_streams: streams,
        }
    }

    /// The first video stream, if any.
    pub fn primary_video(&self) -> Option<&StreamInfo> {
        self.streams.iter().find(|s| s.stream_type == StreamType::Video)
    }

    /// The default audio stream, falling back to the first one.
    pub fn primary_audio(&self) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|s| s.stream_type == StreamType::Audio && s.is_default)
            .or_else(|| self.streams.iter().find(|s| s.stream_type == StreamType::Audio))
    }

    /// Default subtitle stream: flagged default, else forced, else none.
    pub fn primary_subtitle(&self) -> Option<&StreamInfo> {
        let subs: Vec<&StreamInfo> =
            self.streams.iter().filter(|s| s.stream_type == StreamType::Subtitle).collect();
        subs.iter()
            .copied()
            .find(|s| s.is_default)
            .or_else(|| subs.iter().copied().find(|s| s.is_forced))
    }
}

#[cfg(test)]
mod container_tests {
    use super::normalise_container;

    #[test]
    fn ffprobe_aliases_map_to_extension_names() {
        // These are the aliases ffprobe actually emits for the containers a
        // browser client is most likely to meet.
        assert_eq!(normalise_container("matroska,webm"), "mkv");
        assert_eq!(normalise_container("matroska"), "mkv");
        assert_eq!(normalise_container("mpegts"), "ts");
        assert_eq!(normalise_container("mov,mp4,m4a"), "mov");
        // An unknown name is passed through rather than mangled.
        assert_eq!(normalise_container("flac"), "flac");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_type_round_trip() {
        assert_eq!(StreamType::from_codec_type("video"), Some(StreamType::Video));
        assert_eq!(StreamType::from_codec_type("bogus"), None);
        assert_eq!(StreamType::Audio.as_str(), "Audio");
    }

    #[test]
    fn probe_missing_file_is_not_an_error() {
        let p = Prober::new();
        let r = p.probe(Path::new("/nonexistent/file.mkv")).unwrap();
        assert!(r.container.is_none());
        assert!(r.streams.is_empty());
    }

    #[test]
    fn probe_real_file() {
        // Only meaningful when ffprobe is installed; skip otherwise.
        if Command::new("ffprobe").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join("jf-probe-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tone.mkv");

        let status = Command::new("ffmpeg")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "aac"])
            .arg(&file)
            .output();
        if !status.map(|s| s.status.success()).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let result = Prober::new().probe(&file).unwrap();
        assert!(result.duration_secs.unwrap_or(0.0) > 0.5);
        assert!(result.primary_audio().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}