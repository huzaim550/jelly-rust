//! Transcoding pipeline: capability decisions and ffmpeg command construction.

use jf_core::{MediaProtocol, MediaSourceInfo, MediaStream, MediaStreamType, MediaType, RuntimeTicks};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use thiserror::Error;

/// Why a transcode was or was not needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PlayMethod {
    DirectPlay,
    DirectStream,
    Transcode,
}

/// What the client can handle natively.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Container list the client accepts, e.g. `["mp4", "mkv"]`.
    pub direct_play_containers: Vec<String>,
    #[serde(default)]
    pub direct_stream_containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
    pub supports_hls: bool,
    pub supports_direct_stream: bool,
    pub supports_transcoding: bool,
    pub max_bitrate: Option<i64>,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
    /// Rough playback-speed ceiling, used for the width/height headroom test.
    pub max_audio_channels: Option<i32>,
}

impl ClientCapabilities {
    /// Baseline matching what a modern browser can play directly.
    pub fn web() -> Self {
        Self {
            direct_play_containers: vec![
                "mp4".into(),
                "m4v".into(),
                "webm".into(),
                "mov".into(),
                "mkv".into(),
                "avi".into(),
                "ts".into(),
                "mp3".into(),
                "m4a".into(),
                "aac".into(),
                "flac".into(),
                "ogg".into(),
                "opus".into(),
                "wav".into(),
                "jpg".into(),
                "png".into(),
            ],
            direct_stream_containers: vec![
                "mp4".into(),
                "m4v".into(),
                "webm".into(),
                "mp3".into(),
                "m4a".into(),
                "aac".into(),
            ],
            video_codecs: vec![
                "h264".into(),
                "hevc".into(),
                "vp8".into(),
                "vp9".into(),
                "av1".into(),
                "mpeg4".into(),
            ],
            audio_codecs: vec![
                "aac".into(),
                "mp3".into(),
                "opus".into(),
                "vorbis".into(),
                "flac".into(),
                "ac3".into(),
                "eac3".into(),
            ],
            supports_hls: true,
            supports_direct_stream: true,
            supports_transcoding: true,
            max_bitrate: None,
            max_width: None,
            max_height: None,
            max_audio_channels: None,
        }
    }
}

/// Stream selection requested by a client.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamSelection {
    #[serde(default)]
    pub audio_stream_index: Option<i32>,
    #[serde(default)]
    pub subtitle_stream_index: Option<i32>,
    #[serde(default)]
    pub max_audio_channels: Option<i32>,
    #[serde(default)]
    pub max_bitrate: Option<i64>,
    #[serde(default)]
    pub video_bitrate: Option<i64>,
    #[serde(default)]
    pub audio_bitrate: Option<i64>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub subtitle_stream_method: Option<String>,
    #[serde(default)]
    pub max_width: Option<i32>,
    #[serde(default)]
    pub max_height: Option<i32>,
}

/// A decision about how to play an item, with the ffmpeg plan that follows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackPlan {
    pub play_method: PlayMethod,
    pub protocol: MediaProtocol,
    pub container: String,
    pub path: PathBuf,
    pub needs_video_transcode: bool,
    pub needs_audio_transcode: bool,
    pub needs_subtitle_transcode: bool,
    pub direct_stream: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub runtime_ticks: RuntimeTicks,
    /// The ffmpeg argv, present when transcoding.
    pub command: Option<Vec<String>>,
    /// The output path inside the HLS session directory.
    pub output_path: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum TranscodeError {
    #[error("transcoding is disabled by server configuration")]
    Disabled,
    #[error("media path does not exist: {0}")]
    MissingInput(PathBuf),
    #[error("ffmpeg is not available: {0}")]
    NoFfmpeg(String),
    #[error("ffmpeg exited with {status}: {stderr}")]
    FfmpegFailed { status: String, stderr: String },
    #[error("no video or audio stream in source")]
    NoPlayableStream,
}

/// Target container/codec pair for HLS output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlsProfile {
    pub video_codec: &'static str,
    pub audio_codec: &'static str,
    pub container: &'static str,
}

impl HlsProfile {
    /// HEVC/AAC variant for Apple devices.
    pub const HEVC: HlsProfile =
        HlsProfile { video_codec: "libx265", audio_codec: "aac", container: "ts" };

    /// H.264/AAC, the broadly compatible default.
    pub const AVC: HlsProfile =
        HlsProfile { video_codec: "libx264", audio_codec: "aac", container: "ts" };

    /// fMP4 for modern browsers, avoiding MPEG-TS overhead.
    pub const AVC_FMP4: HlsProfile =
        HlsProfile { video_codec: "libx264", audio_codec: "aac", container: "mp4" };

    /// Audio-only.
    pub const AUDIO: HlsProfile =
        HlsProfile { video_codec: "none", audio_codec: "aac", container: "aac" };
}

/// Builds ffmpeg command lines and decides between direct play and transcode.
#[derive(Debug, Clone)]
pub struct Transcoder {
    ffmpeg: String,
    /// Conservative default ladder. Heights are chosen so ffmpeg can scale
    /// without upscaling.
    enabled: bool,
    max_bitrate: i64,
    allow_hevc: bool,
}

impl Default for Transcoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcoder {
    pub fn new() -> Self {
        Self {
            ffmpeg: std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned()),
            enabled: true,
            max_bitrate: 120_000_000,
            allow_hevc: true,
        }
    }

    pub fn with_ffmpeg(path: impl Into<String>) -> Self {
        Self { ffmpeg: path.into(), ..Self::new() }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Decide how to play `source` for a client with `caps`.
    ///
    /// Returns `Ok(None)` when the item can be played directly, meaning the
    /// caller should serve the original file rather than spin up ffmpeg.
    pub fn plan(
        &self,
        source: &MediaSourceInfo,
        item_type: MediaType,
        caps: &ClientCapabilities,
        selection: &StreamSelection,
        output_dir: &PathBuf,
    ) -> Result<Option<PlaybackPlan>, TranscodeError> {
        let path = PathBuf::from(&source.path);
        let container = source.container.clone().unwrap_or_default();
        let container_lower = container.to_ascii_lowercase();

        let audio = pick_audio(source, selection);
        let video = pick_video(source);
        let subtitle = pick_subtitle(source, selection);

        let audio_codec = audio.and_then(|s| s.codec.as_ref()).map(|c| c.to_ascii_lowercase());
        let video_codec = video.and_then(|s| s.codec.as_ref()).map(|c| c.to_ascii_lowercase());

        // --- Can we hand back the original file untouched? ---
        let container_ok = caps
            .direct_play_containers
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&container_lower));
        let video_ok = match (&video_codec, item_type) {
            // A photo or audio-only file has no video track to check.
            (None, _) | (_, MediaType::Audio) | (_, MediaType::Photo) => true,
            (Some(c), _) => caps.video_codecs.iter().any(|v| v.eq_ignore_ascii_case(c)),
        };
        let audio_ok = match (&audio_codec, item_type) {
            (None, MediaType::Video) => false,
            _ => audio_codec.as_ref().is_some_and(|c| {
                caps.audio_codecs.iter().any(|a| a.eq_ignore_ascii_case(c))
            }),
        };

        // Respect the stricter of the client's stated ceiling and its per-request ask.
        let bitrate_ceiling = selection.max_bitrate.or(caps.max_bitrate);
        let bitrate_ok = match bitrate_ceiling {
            // An unknown source bitrate cannot be compared, so it is allowed.
            None => true,
            Some(max) => estimate_bitrate(source) <= max,
        };

        // The effective height ceiling is the stricter of what the client can display
        // and what it explicitly asked for. Checking only `caps.max_height` would
        // let a 4K source play "directly" to a client that requested 720p.
        let height_ceiling = selection
            .max_height
            .or(selection.max_width)
            .or(caps.max_height);
        let resolution_ok = match (video, height_ceiling) {
            (Some(v), Some(max)) => v.height.unwrap_or(0) <= max,
            _ => true,
        };

        let subtitle_ok = match subtitle {
            None => true,
            // A sidecar subtitle stream is fine; only embedded ones force work.
            Some(s) => !s.is_external || caps.supports_transcoding,
        };

        if container_ok && video_ok && audio_ok && bitrate_ok && resolution_ok && subtitle_ok {
            return Ok(None);
        }

        if !self.enabled || !caps.supports_transcoding {
            // Client cannot handle the source and cannot transcode; fall back to
            // serving the file as-is rather than failing.
            return Ok(None);
        }

        // --- Build the transcode plan ---
        let is_audio_only = matches!(item_type, MediaType::Audio) || video.is_none();
        let profile = if is_audio_only {
            HlsProfile::AUDIO
        } else if self.allow_hevc && self.prefer_hevc(&video_codec, caps) {
            HlsProfile::HEVC
        } else {
            HlsProfile::AVC_FMP4
        };

        // A stream needs re-encoding when the client cannot decode it natively, or
        // when the source exceeds a ceiling the client set.
        let needs_video_transcode = !video_ok || !resolution_ok || !bitrate_ok;
        let needs_audio_transcode = !audio_ok;

        let output_path = output_dir.join("output.m3u8");
        let command = self.build_hls_command(
            &path,
            output_dir,
            &profile,
            audio,
            video,
            subtitle,
            selection,
            is_audio_only,
        );

        Ok(Some(PlaybackPlan {
            play_method: PlayMethod::Transcode,
            protocol: MediaProtocol::Hls,
            container: profile.container.to_owned(),
            path,
            needs_video_transcode,
            needs_audio_transcode,
            needs_subtitle_transcode: subtitle.is_some(),
            direct_stream: false,
            video_codec: needs_video_transcode
                .then(|| profile.video_codec.to_owned()),
            audio_codec: needs_audio_transcode.then(|| profile.audio_codec.to_owned()),
            audio_stream_index: audio.map(|s| s.index),
            subtitle_stream_index: subtitle.map(|s| s.index),
            runtime_ticks: source.run_time_ticks,
            command: Some(command),
            output_path: Some(output_path),
        }))
    }

    /// Serve the original bytes for direct play.
    pub fn direct_stream(&self, source: &MediaSourceInfo) -> DirectStream {
        DirectStream {
            path: PathBuf::from(&source.path),
            content_type: container_mime(&source.container),
            size: source.size,
        }
    }

    fn prefer_hevc(&self, source_codec: &Option<String>, caps: &ClientCapabilities) -> bool {
        // Preserve HEVC when the client already supports it: re-encoding to
        // H.264 would be slow and lossy for no benefit.
        source_codec.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("hevc"))
            && caps.video_codecs.iter().any(|v| v.eq_ignore_ascii_case("hevc"))
    }

    /// Construct the full ffmpeg argv for an HLS output.
    #[allow(clippy::too_many_arguments)]
    fn build_hls_command(
        &self,
        input: &PathBuf,
        output_dir: &PathBuf,
        profile: &HlsProfile,
        audio: Option<&MediaStream>,
        video: Option<&MediaStream>,
        subtitle: Option<&MediaStream>,
        selection: &StreamSelection,
        audio_only: bool,
    ) -> Vec<String> {
        let mut cmd: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "warning".into(),
            "-i".into(),
            input.to_string_lossy().into_owned(),
        ];

        // Map only the streams we actually want.
        if let Some(v) = video.filter(|_| !audio_only) {
            cmd.extend(["-map".into(), format!("0:{}", v.index)]);
        }
        if let Some(a) = audio {
            cmd.extend(["-map".into(), format!("0:{}", a.index)]);
        } else if audio_only {
            // Audio-only with no explicit selection: take the first audio.
            cmd.extend(["-map".into(), "0:a:0?".into()]);
        }
        // Subtitles are dropped; HLS subtitle support varies too much
        // between clients to rely on, and burning in kills direct seek.
        cmd.extend(["-sn".into(), "-dn".into()]);

        if audio_only {
            cmd.extend(["-vn".into()]);
            cmd.extend([
                "-c:a".into(),
                profile.audio_codec.into(),
                "-b:a".into(),
                format!("{}k", selection.audio_bitrate.unwrap_or(192_000) / 1000),
                "-ar".into(),
                "48000".into(),
                "-ac".into(),
                selection.max_audio_channels.unwrap_or(2).to_string(),
            ]);
        } else {
            cmd.extend(["-c:v".into(), profile.video_codec.into()]);
            if let Some(height) = selection.max_height.or(selection.max_width) {
                // Scale to a standard ladder rung, never above the source.
                let target = standard_height(height);
                cmd.extend([
                    "-vf".into(),
                    format!("scale=-2:'min({target},ih)'"),
                ]);
            }
            let v_bitrate = selection.video_bitrate.unwrap_or(self.max_bitrate / 4);
            let v_buf = (v_bitrate * 3 / 2).to_string();
            cmd.extend([
                "-b:v".into(),
                format!("{}k", v_bitrate / 1000),
                "-maxrate".into(),
                v_buf.clone(),
                "-bufsize".into(),
                v_buf,
                // keyint 2 * GOP keeps HLS segments aligned with keyframes.
                "-g".into(),
                "48".into(),
                "-keyint_min".into(),
                "48".into(),
                "-sc_threshold".into(),
                "0".into(),
                "-profile:v".into(),
                "main".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                // Closed GOP + faststart so segments are independently seekable.
                "-force_key_frames".into(),
                "expr:gte(t,n_forced*2)".into(),
            ]);
            cmd.extend([
                "-c:a".into(),
                profile.audio_codec.into(),
                "-b:a".into(),
                format!("{}k", selection.audio_bitrate.unwrap_or(192_000) / 1000),
                "-ac".into(),
                selection.max_audio_channels.unwrap_or(2).to_string(),
            ]);
        }

        // HLS output options.
        let segment_name = output_dir.join("segment%05d.ts");
        let playlist = output_dir.join("output.m3u8");
        cmd.extend([
            "-f".into(),
            "hls".into(),
            "-hls_time".into(),
            "4".into(),
            "-hls_playlist_type".into(),
            "vod".into(),
            "-hls_flags".into(),
            "independent_segments".into(),
            "-hls_segment_filename".into(),
            segment_name.to_string_lossy().into_owned(),
            "-start_number".into(),
            "0".into(),
        ]);

        // fMP4 needs an init segment and a different muxer extension.
        if profile.container == "mp4" {
            cmd.extend([
                "-hls_segment_type".into(),
                "fmp4".into(),
                "-hls_fmp4_init_filename".into(),
                "init.mp4".into(),
            ]);
            cmd.push(playlist.to_string_lossy().into_owned());
            cmd.extend([
                "-movflags".into(),
                "+faststart".into(),
            ]);
        } else {
            cmd.push(playlist.to_string_lossy().into_owned());
        }

        cmd
    }

    /// Run an ffmpeg command to completion, capturing stderr.
    pub async fn run(&self, command: &[String]) -> Result<(), TranscodeError> {
        let mut child = tokio::process::Command::new(&self.ffmpeg)
            .args(command)
            // ffmpeg writes progress to stderr and would otherwise inherit the
            // server's stdout, corrupting HTTP responses when spawned in-process.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| TranscodeError::NoFfmpeg(e.to_string()))?;

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| TranscodeError::NoFfmpeg(e.to_string()))?;

        if !output.status.success() {
            return Err(TranscodeError::FfmpegFailed {
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }
}

/// A file served without re-encoding.
#[derive(Debug, Clone)]
pub struct DirectStream {
    pub path: PathBuf,
    pub content_type: String,
    pub size: Option<i64>,
}

/// Approximate source bitrate in bits/sec, from size and duration.
fn estimate_bitrate(source: &MediaSourceInfo) -> i64 {
    match (source.size, source.run_time_ticks) {
        (Some(size), ticks) if ticks > 0 => {
            ((size as f64) * 8.0 * 10_000_000.0 / ticks as f64) as i64
        }
        _ => 0,
    }
}

/// Choose the audio stream honoring the client's request.
fn pick_audio<'a>(
    source: &'a MediaSourceInfo,
    selection: &StreamSelection,
) -> Option<&'a MediaStream> {
    let streams: Vec<&MediaStream> = source
        .media_streams
        .iter()
        .filter(|s| s.stream_type == MediaStreamType::Audio)
        .collect();

    if let Some(idx) = selection.audio_stream_index {
        if let Some(found) = streams.iter().copied().find(|s| s.index == idx) {
            return Some(found);
        }
    }

    streams
        .iter()
        .copied()
        .find(|s| s.is_default)
        .or_else(|| streams.first().copied())
}

/// Choose the first video stream.
fn pick_video(source: &MediaSourceInfo) -> Option<&MediaStream> {
    source
        .media_streams
        .iter()
        .find(|s| s.stream_type == MediaStreamType::Video)
}

/// Choose the subtitle stream honoring the client's request.
fn pick_subtitle<'a>(
    source: &'a MediaSourceInfo,
    selection: &StreamSelection,
) -> Option<&'a MediaStream> {
    let streams: Vec<&MediaStream> = source
        .media_streams
        .iter()
        .filter(|s| s.stream_type == MediaStreamType::Subtitle)
        .collect();

    // An explicit request to disable subtitles is index -1.
    if selection.subtitle_stream_index == Some(-1) {
        return None;
    }

    if let Some(idx) = selection.subtitle_stream_index {
        if let Some(found) = streams.iter().copied().find(|s| s.index == idx) {
            return Some(found);
        }
    }

    let preferred = streams.iter().copied().find(|s| s.is_default);
    // Only offer a subtitle if the user actually asked for one; a default
    // flag alone should not turn subtitles on.
    if selection.subtitle_stream_index.is_some() { preferred } else { None }
}

/// Snap a height to a standard encoding rung, never upscaling.
fn standard_height(max: i32) -> i32 {
    const LADDER: [i32; 7] = [2160, 1440, 1080, 720, 480, 360, 240];
    LADDER.iter().copied().find(|h| *h <= max).unwrap_or(240)
}

/// MIME type for a container extension.
pub fn container_mime(container: &Option<String>) -> String {
    match container.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mkv") | Some("matroska") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("ts") | Some("mpegts") => "video/mp2t",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("aac") => "audio/aac",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jf_core::TICKS_PER_SECOND;

    fn source_with_streams(container: &str, video: Option<(&str, i32, i32)>, audio: Option<&str>) -> MediaSourceInfo {
        let mut streams = Vec::new();
        if let Some((codec, width, height)) = video {
            streams.push(MediaStream {
                codec: Some(codec.into()),
                stream_type: MediaStreamType::Video,
                index: 0,
                width: Some(width),
                height: Some(height),
                channels: None,
                language: Some("eng".into()),
                is_default: true,
                is_forced: false,
                is_external: false,
            });
        }
        if let Some(codec) = audio {
            streams.push(MediaStream {
                codec: Some(codec.into()),
                stream_type: MediaStreamType::Audio,
                index: 1,
                width: None,
                height: None,
                channels: Some(6),
                language: Some("eng".into()),
                is_default: true,
                is_forced: false,
                is_external: false,
            });
        }
        MediaSourceInfo {
            id: "src".into(),
            protocol: MediaProtocol::File,
            path: "/media/movie.mkv".into(),
            container: Some(container.into()),
            size: Some(4_000_000_000),
            run_time_ticks: 7200 * TICKS_PER_SECOND,
            media_streams: streams,
        }
    }

    #[test]
    fn compatible_source_needs_no_transcode() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        let src = source_with_streams("mp4", Some(("h264", 1920, 1080)), Some("aac"));
        let plan = t.plan(&src, MediaType::Video, &caps, &StreamSelection::default(), &PathBuf::from("/tmp")).unwrap();
        assert!(plan.is_none(), "compatible source should play directly");
    }

    #[test]
    fn incompatible_codec_forces_transcode() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        // VC-1 and DTS are both outside the browser capability list.
        let src = source_with_streams("mkv", Some(("vc1", 1920, 1080)), Some("dts"));
        let plan = t.plan(&src, MediaType::Video, &caps, &StreamSelection::default(), &PathBuf::from("/tmp")).unwrap();
        let plan = plan.expect("should transcode");
        assert_eq!(plan.play_method, PlayMethod::Transcode);
        assert!(plan.needs_video_transcode);
        assert!(plan.needs_audio_transcode);
        assert_eq!(plan.protocol, MediaProtocol::Hls);
    }

    #[test]
    fn supported_video_with_unsupported_audio_only_reencodes_audio() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        // VP9 plays natively in browsers; DTS audio does not.
        let src = source_with_streams("mkv", Some(("vp9", 1920, 1080)), Some("dts"));
        let plan = t.plan(&src, MediaType::Video, &caps, &StreamSelection::default(), &PathBuf::from("/tmp")).unwrap();
        let plan = plan.expect("should transcode");
        assert!(plan.needs_audio_transcode);
        assert!(!plan.needs_video_transcode, "video is already decodable");
    }

    #[test]
    fn height_ceiling_forces_scale_filter() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        let src = source_with_streams("mp4", Some(("h264", 3840, 2160)), Some("aac"));
        let sel = StreamSelection { max_height: Some(720), ..Default::default() };
        let plan = t.plan(&src, MediaType::Video, &caps, &sel, &PathBuf::from("/tmp")).unwrap().unwrap();
        let cmd = plan.command.unwrap();
        let vf = cmd.iter().position(|a| a == "-vf").expect("scale filter present");
        assert!(cmd[vf + 1].contains("720"), "expected 720p rung, got {}", cmd[vf + 1]);
    }

    #[test]
    fn never_upscales_beyond_source_height() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        let src = source_with_streams("mp4", Some(("h264", 640, 360)), Some("aac"));
        let sel = StreamSelection { max_height: Some(2160), ..Default::default() };
        let plan = t.plan(&src, MediaType::Video, &caps, &sel, &PathBuf::from("/tmp")).unwrap();
        // 360p source into a 2160 ceiling still fits the client's ladder, so
        // it plays directly rather than being needlessly re-encoded.
        assert!(plan.is_none());
    }

    #[test]
    fn subtitle_can_be_disabled_explicitly() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        let mut src = source_with_streams("mp4", Some(("h264", 1280, 720)), Some("aac"));
        src.media_streams.push(MediaStream {
            codec: Some("subrip".into()),
            stream_type: MediaStreamType::Subtitle,
            index: 2,
            width: None,
            height: None,
            channels: None,
            language: Some("eng".into()),
            is_default: true,
            is_forced: false,
            is_external: false,
        });
        let sel = StreamSelection { subtitle_stream_index: Some(-1), ..Default::default() };
        assert!(t.plan(&src, MediaType::Video, &caps, &sel, &PathBuf::from("/tmp")).unwrap().is_none());
    }

    #[test]
    fn disabled_transcoder_falls_back_to_direct() {
        let t = Transcoder::new().disabled();
        let caps = ClientCapabilities::web();
        let src = source_with_streams("mkv", Some(("vp9", 1920, 1080)), Some("dts"));
        assert!(t.plan(&src, MediaType::Video, &caps, &StreamSelection::default(), &PathBuf::from("/tmp")).unwrap().is_none());
    }

    #[test]
    fn audio_only_uses_audio_profile() {
        let t = Transcoder::new();
        let caps = ClientCapabilities::web();
        // Browsers play FLAC natively, so a FLAC source must NOT transcode.
        let flac = source_with_streams("flac", None, Some("flac"));
        assert!(
            t.plan(&flac, MediaType::Audio, &caps, &StreamSelection::default(), &PathBuf::from("/tmp"))
                .unwrap()
                .is_none(),
            "flac should play directly"
        );

        // Uncompressed PCM in a WAV container is not browser-playable, so this
        // must fall through to the audio-only AAC profile.
        let wav = source_with_streams("wav", None, Some("pcm_s16le"));
        let plan = t
            .plan(&wav, MediaType::Audio, &caps, &StreamSelection::default(), &PathBuf::from("/tmp"))
            .unwrap()
            .expect("pcm should transcode");
        let cmd = plan.command.unwrap();
        assert!(cmd.contains(&"aac".to_owned()), "expected aac encoding: {cmd:?}");
        assert!(cmd.contains(&"-vn".to_owned()), "expected video disabled");
    }

    #[test]
    fn mime_types_map_correctly() {
        assert_eq!(container_mime(&Some("mkv".into())), "video/x-matroska");
        assert_eq!(container_mime(&Some("mp4".into())), "video/mp4");
        assert_eq!(container_mime(&None), "application/octet-stream");
    }

    #[test]
    fn bitrate_estimation_detects_heavy_source() {
        let t = Transcoder::new();
        let src = source_with_streams("mp4", Some(("h264", 1920, 1080)), Some("aac"));
        // 4GB over 2h is roughly 4.4 Mbps, so a 2 Mbps ceiling must transcode
        // and a 20 Mbps ceiling must not.
        let tight = ClientCapabilities { max_bitrate: Some(2_000_000), ..ClientCapabilities::web() };
        let loose = ClientCapabilities { max_bitrate: Some(20_000_000), ..ClientCapabilities::web() };

        assert!(
            t.plan(&src, MediaType::Video, &tight, &StreamSelection::default(), &PathBuf::from("/tmp"))
                .unwrap()
                .is_some(),
            "source above the bitrate ceiling should transcode"
        );
        assert!(
            t.plan(&src, MediaType::Video, &loose, &StreamSelection::default(), &PathBuf::from("/tmp"))
                .unwrap()
                .is_none(),
            "source below the bitrate ceiling should play directly"
        );
    }

    #[test]
    fn per_request_bitrate_is_respected() {
        let t = Transcoder::new();
        // No client ceiling, but the request itself asks for a low bitrate.
        let caps = ClientCapabilities::web();
        let src = source_with_streams("mp4", Some(("h264", 1920, 1080)), Some("aac"));
        let sel = StreamSelection { max_bitrate: Some(2_000_000), ..Default::default() };
        assert!(
            t.plan(&src, MediaType::Video, &caps, &sel, &PathBuf::from("/tmp"))
                .unwrap()
                .is_some(),
            "a per-request bitrate ceiling should force transcoding"
        );
    }

    #[test]
    fn standard_height_ladder() {
        assert_eq!(standard_height(2160), 2160);
        assert_eq!(standard_height(1000), 720);
        assert_eq!(standard_height(100), 240);
    }
}