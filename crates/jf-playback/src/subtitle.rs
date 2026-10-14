//! Subtitle extraction and conversion.
//!
//! Subtitle tracks inside a container are extracted to a sidecar file on
//! demand so browsers and TV clients can load them over HTTP.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;

/// Subtitle delivery format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleKind {
    /// Text-based, rendered by the client (`text/vtt`).
    Vtt,
    /// Embedded image-based (PGS, VOBSUB).
    Srt,
}

/// A subtitle track request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleRequest {
    pub item_id: String,
    pub media_path: PathBuf,
    pub stream_index: i32,
    pub format: SubtitleKind,
    /// Start and end offsets in runtime ticks, for partial extraction.
    pub start_ticks: Option<i64>,
    pub end_ticks: Option<i64>,
    pub language: Option<String>,
}

#[derive(Debug, Error)]
pub enum SubtitleError {
    #[error("no ffmpeg available: {0}")]
    NoFfmpeg(String),
    #[error("could not extract subtitles: {0}")]
    Extract(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convert runtime ticks to ffmpeg's timestamp format (`HH:MM:SS.mmm`).
pub fn ticks_to_timestamp(ticks: i64) -> String {
    let total_ms = (ticks.max(0) / 10_000).max(0);
    let ms = total_ms % 1000;
    let secs = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        ms
    )
}

/// A hand-rolled SRT-to-WebVTT converter.
///
/// Deliberately minimal: handles BOM, CRLF, comma decimal separators, and
/// optional cue numbering. Anything it cannot parse is passed through as text,
/// which browsers tolerate.
pub fn srt_to_vtt(srt: &str) -> String {
    let body = srt.strip_prefix('\u{feff}').unwrap_or(srt);
    let mut out = String::with_capacity(body.len() + 16);
    out.push_str("WEBVTT\n\n");

    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches('\r');

        // Timing line: "00:00:01,000 --> 00:00:04,000"
        if trimmed.contains("-->") {
            let converted = trimmed.replace(",000", ".000").replace(',', ".");
            // Comma decimals only: HH:MM:SS,mmm
            let converted = fix_comma_millis(&trimmed);
            let _ = converted;
            out.push_str(&converted);
            out.push('\n');
            continue;
        }

        // Skip leading cue numbers: a line that is purely digits before a
        // timing line.
        if trimmed.chars().all(|c| c.is_ascii_digit()) && trimmed.len() > 0 {
            if lines.peek().map(|l| l.contains("-->")).unwrap_or(false) {
                continue;
            }
        }

        out.push_str(trimmed);
        out.push('\n');
    }

    out
}

/// Rewrite `HH:MM:SS,mmm` timestamps as `HH:MM:SS.mmm`.
fn fix_comma_millis(line: &str) -> String {
    let mut parts = line.splitn(2, "-->");
    let Some(start) = parts.next() else { return line.to_owned() };
    let Some(rest) = parts.next() else { return line.to_owned() };

    let start = fix_timestamp_token(start.trim());
    let end_token = rest.trim();
    // Preserve any trailing cue settings (position, alignment) after the end ts.
    let end_and_settings = end_token.split_once(char::is_whitespace).unwrap_or((end_token, ""));
    let end = fix_timestamp_token(end_and_settings.0);

    if end_and_settings.1.is_empty() {
        format!("{start} --> {end}")
    } else {
        format!("{start} --> {end} {}", end_and_settings.1)
    }
}

/// Replace the comma before the milliseconds with a dot.
fn fix_timestamp_token(token: &str) -> String {
    match token.rsplit_once(',') {
        Some((head, millis)) if millis.len() == 3 && millis.chars().all(|c| c.is_ascii_digit()) => {
            format!("{head}.{millis}")
        }
        _ => token.to_owned(),
    }
}

/// Extract a subtitle stream to a file, converting text formats to WebVTT.
pub async fn extract(
    req: &SubtitleRequest,
    output: &Path,
    ffmpeg: Option<&str>,
) -> Result<SubtitleKind, SubtitleError> {
    if !req.media_path.is_file() {
        return Err(SubtitleError::Extract(format!(
            "media file missing: {}",
            req.media_path.display()
        )));
    }
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let binary = ffmpeg
        .map(str::to_owned)
        .or_else(|| std::env::var("FFMPEG_PATH").ok())
        .unwrap_or_else(|| "ffmpeg".to_owned());

    let mut cmd = tokio::process::Command::new(&binary);
    cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-i").arg(&req.media_path);

    // Clipping before the input is much faster than after it.
    if let Some(start) = req.start_ticks {
        cmd.arg("-ss").arg(ticks_to_timestamp(start));
    }

    cmd.args(["-map", &format!("0:{}", req.stream_index)]);

    if let Some(end) = req.end_ticks {
        // `-t` is a duration, so subtract the start.
        let start = req.start_ticks.unwrap_or(0);
        let duration = ticks_to_timestamp((end - start).max(0));
        cmd.arg("-t").arg(duration);
    }

    // WebVTT via ffmpeg's webvtt muxer for text tracks; copy image tracks raw.
    if req.format == SubtitleKind::Vtt {
        cmd.args(["-f", "webvtt"]);
    }
    cmd.arg("-y").arg(output).stdout(Stdio::null()).stderr(Stdio::piped());

    let output_res = cmd.output().await.map_err(|e| SubtitleError::NoFfmpeg(e.to_string()))?;

    if !output_res.status.success() {
        let stderr = String::from_utf8_lossy(&output_res.stderr).trim().to_owned();
        let _ = tokio::fs::remove_file(output).await;
        return Err(SubtitleError::Extract(stderr));
    }

    Ok(req.format)
}

/// The output extension for a subtitle format.
pub fn extension_for(kind: SubtitleKind) -> &'static str {
    match kind {
        SubtitleKind::Vtt => "vtt",
        SubtitleKind::Srt => "srt",
    }
}

/// The HTTP content type for a subtitle format.
pub fn content_type_for(kind: SubtitleKind) -> &'static str {
    match kind {
        SubtitleKind::Vtt => "text/vtt; charset=utf-8",
        SubtitleKind::Srt => "application/x-subrip; charset=utf-8",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_formatting() {
        // Ticks are 100ns units, so one second is 10_000_000.
        assert_eq!(ticks_to_timestamp(0), "00:00:00.000");
        assert_eq!(ticks_to_timestamp(10_000_000), "00:00:01.000");
        assert_eq!(ticks_to_timestamp(610_000_000), "00:01:01.000");
        // 1h 1m 1.5s = 3661.5s * 10_000_000 ticks.
        assert_eq!(ticks_to_timestamp(36_615_000_000), "01:01:01.500");
        assert_eq!(ticks_to_timestamp(900_000_000), "00:01:30.000");
        assert_eq!(ticks_to_timestamp(-5), "00:00:00.000", "negative clamps to zero");
    }

    #[test]
    fn timestamp_matches_jf_core_conversion() {
        // Cross-check against the canonical conversion so the two cannot drift.
        for secs in [0.0, 1.0, 61.5, 3661.5, 7199.0] {
            let ticks = jf_core::secs_to_ticks(secs);
            assert_eq!(
                ticks_to_timestamp(ticks),
                format!("{:02}:{:02}:{:06.3}", (secs / 3600.0) as u32, ((secs / 60.0) as u32) % 60, secs % 60.0),
                "mismatch at {secs}s"
            );
        }
    }

    #[test]
    fn srt_converts_to_webvtt() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello there\n\n2\n00:00:05,500 --> 00:00:08,000\nSecond line\n";
        let vtt = srt_to_vtt(srt);

        assert!(vtt.starts_with("WEBVTT\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:04.000"), "got: {vtt}");
        assert!(vtt.contains("Hello there"));
        assert!(!vtt.contains("\n1\n"), "cue numbers should be stripped");
        assert!(!vtt.contains(','), "no commas should remain in timestamps");
    }

    #[test]
    fn srt_handles_bom_and_crlf() {
        let srt = "\u{feff}00:00:01,000 --> 00:00:02,000\r\nText\r\n";
        let vtt = srt_to_vtt(srt);
        assert!(!vtt.contains('\u{feff}'));
        assert!(vtt.contains("00:00:01.000 --> 00:00:02.000"));
        assert!(vtt.contains("Text"));
        assert!(!vtt.contains('\r'));
    }

    #[test]
    fn srt_preserves_cue_settings() {
        let srt = "00:00:01,000 --> 00:00:04,000 line:90% align:center\nHi\n";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("00:00:04.000 line:90% align:center"), "got: {vtt}");
    }

    #[test]
    fn already_vtt_input_is_stable() {
        // A WebVTT file uses dots already; the converter must not double them.
        let vtt_in = "WEBVTT\n\n00:00:01.000 --> 00:00:04.000\nHi\n";
        let out = srt_to_vtt(vtt_in);
        assert!(out.contains("00:00:01.000 --> 00:00:04.000"));
        assert!(!out.contains(".000.000"));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for(SubtitleKind::Vtt), "text/vtt; charset=utf-8");
        assert_eq!(extension_for(SubtitleKind::Vtt), "vtt");
    }

    #[tokio::test]
    async fn extract_reports_missing_media() {
        let req = SubtitleRequest {
            item_id: "i".into(),
            media_path: PathBuf::from("/nonexistent.mkv"),
            stream_index: 2,
            format: SubtitleKind::Vtt,
            start_ticks: None,
            end_ticks: None,
            language: None,
        };
        let out = std::env::temp_dir().join("jf-sub-test/out.vtt");
        assert!(extract(&req, &out, None).await.is_err());
    }
}