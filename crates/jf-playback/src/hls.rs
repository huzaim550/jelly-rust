//! HLS session lifecycle.
//!
//! Each transcoding session gets a scratch directory holding the playlist and
//! its segments. Sessions are reaped when idle so abandoned playback does not
//! leak ffmpeg processes or disk space.

use crate::transcode::{PlaybackPlan, Transcoder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::process::Child;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum HlsError {
    #[error("no such transcode session: {0}")]
    Unknown(String),
    #[error("transcode session directory is missing")]
    MissingDirectory,
    #[error("playlist not ready: {0}")]
    NotReady(String),
    #[error(transparent)]
    Transcode(#[from] crate::transcode::TranscodeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A live transcode session.
pub struct HlsSession {
    pub id: String,
    pub play_session_id: String,
    pub item_id: String,
    pub user_id: String,
    pub directory: PathBuf,
    pub plan: PlaybackPlan,
    pub created_at: Instant,
    pub last_access: Mutex<Instant>,
    process: Mutex<Option<Child>>,
}

/// How long a session may sit idle before being reaped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// Creates, tracks, and reaps HLS sessions.
#[derive(Clone)]
pub struct HlsSessionManager {
    root: PathBuf,
    transcoder: Arc<Transcoder>,
    sessions: Arc<Mutex<HashMap<String, Arc<HlsSession>>>>,
}

impl HlsSessionManager {
    /// Create a manager rooted at `root` (e.g. `<cache>/transcode`).
    pub fn new(root: impl Into<PathBuf>, transcoder: Arc<Transcoder>) -> Self {
        Self {
            root: root.into(),
            transcoder,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start a transcode for `plan`, returning the session id.
    ///
    /// The ffmpeg process is spawned immediately; the playlist is written once
    /// ffmpeg has produced the first segment.
    pub async fn start(
        &self,
        session_id: String,
        play_session_id: String,
        item_id: String,
        user_id: String,
        plan: PlaybackPlan,
    ) -> Result<Arc<HlsSession>, HlsError> {
        let directory = self.root.join(&session_id);
        tokio::fs::create_dir_all(&directory).await?;

        let command = plan.command.clone().ok_or(HlsError::MissingDirectory)?;
        let Some(output) = plan.output_path.clone() else {
            return Err(HlsError::MissingDirectory);
        };

        // Segment paths in the plan were built for the scratch dir, so rebuild
        // them here to point at this session. The rewritten .m3u8 path is
        // *this session's* playlist — not the plan's scratch output path —
        // otherwise wait_for_playlist looks in the wrong directory.
        let session_playlist = directory.join("output.m3u8");
        let command = retarget_output(&command, &directory, &session_playlist);

        let mut child = tokio::process::Command::new(self.transcoder_ffmpeg())
            .args(&command)
            .kill_on_drop(true)
.spawn()
            .map_err(|e| crate::transcode::TranscodeError::NoFfmpeg(e.to_string()))?;

        let session = Arc::new(HlsSession {
            id: session_id.clone(),
            play_session_id,
            item_id,
            user_id,
            directory,
            plan,
            created_at: Instant::now(),
            last_access: Mutex::new(Instant::now()),
            process: Mutex::new(Some(child)),
        });

        self.sessions.lock().await.insert(session_id, Arc::clone(&session));
        Ok(session)
    }

    fn transcoder_ffmpeg(&self) -> String {
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned())
    }

    /// Look up a session by id.
    pub async fn get(&self, id: &str) -> Option<Arc<HlsSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Look up a session by the client-facing play session id.
    pub async fn get_by_play_session(&self, play_session_id: &str) -> Option<Arc<HlsSession>> {
        let sessions = self.sessions.lock().await;
        sessions.values().find(|s| s.play_session_id == play_session_id).cloned()
    }

    /// Resolve a request path (`/segment00001.ts`) to a file inside the session.
    pub async fn segment_path(&self, id: &str, relative: &str) -> Result<PathBuf, HlsError> {
        let session = self.get(id).await.ok_or_else(|| HlsError::Unknown(id.to_owned()))?;

        // Reject traversal: only a bare filename from the session dir is valid.
        let name = Path::new(relative)
            .file_name()
            .ok_or_else(|| HlsError::MissingDirectory)?
            .to_string_lossy()
            .into_owned();
        let path = session.directory.join(&name);

        if !path.starts_with(&session.directory) || !path.is_file() {
            return Err(HlsError::NotReady(name));
        }

        session.touch().await;
        Ok(path)
    }

    /// Path to the playlist for a session.
    pub async fn playlist_path(&self, id: &str) -> Result<PathBuf, HlsError> {
        let session = self.get(id).await.ok_or_else(|| HlsError::Unknown(id.to_owned()))?;
        let path = session.directory.join("output.m3u8");
        session.touch().await;
        Ok(path)
    }

    /// Stop a session, killing ffmpeg and deleting its directory.
    pub async fn stop(&self, id: &str) -> Result<(), HlsError> {
        let session = self.sessions.lock().await.remove(id);
        let Some(session) = session else { return Ok(()) };

        if let Some(mut child) = session.process.lock().await.take() {
            let _ = child.kill().await;
        }
        let _ = tokio::fs::remove_dir_all(&session.directory).await;
        Ok(())
    }

    /// Stop every session belonging to a user, e.g. on logout.
    pub async fn stop_user(&self, user_id: &str) {
        let ids: Vec<String> = {
            let sessions = self.sessions.lock().await;
            sessions
                .values()
                .filter(|s| s.user_id == user_id)
                .map(|s| s.id.clone())
                .collect()
        };
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }

    /// Kill and delete sessions idle beyond the timeout.
    pub async fn reap_idle(&self) {
        let stale: Vec<String> = {
            let sessions = self.sessions.lock().await;
            let mut ids = Vec::new();
            for (id, session) in sessions.iter() {
                let idle = *session.last_access.lock().await;
                if idle.elapsed() > IDLE_TIMEOUT {
                    ids.push(id.clone());
                }
            }
            ids
        };

        for id in stale {
            tracing::debug!(session = %id, "reaping idle transcode session");
            let _ = self.stop(&id).await;
        }
    }

    /// Remove all session directories left behind by a previous run.
    ///
    /// Called at startup: ffmpeg processes from a crashed server are gone, but
    /// their scratch files are not.
    pub async fn cleanup_orphans(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else { return };
        let live: Vec<String> = self.sessions.lock().await.keys().cloned().collect();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !live.contains(&name) {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }

    /// Number of active sessions, for diagnostics.
    pub async fn active_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

}

impl HlsSession {
    async fn touch(&self) {
        *self.last_access.lock().await = Instant::now();
    }

    /// Seconds since the session started, for the playback info response.
    pub fn elapsed_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }

    /// Wait for ffmpeg to write the playlist, then return its path.
    ///
    /// ffmpeg creates the playlist asynchronously, so a client that requests it
    /// immediately would otherwise get a 404 and report playback as broken.
    /// Returns the path whether or not the file appeared, so the caller can
    /// produce a meaningful error from the read that follows.
    pub async fn wait_for_playlist(&self, timeout: std::time::Duration) -> Result<PathBuf, HlsError> {
        let path = self.directory.join("output.m3u8");
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                // A playlist with no segments yet is not playable, so keep
                // waiting for ffmpeg to finish the first one.
                let usable = tokio::fs::read_to_string(&path)
                    .await
                    .map(|text| !text.trim().is_empty())
                    .unwrap_or(false);
                if usable {
                    return Ok(path);
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Ok(path);
            }

            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }
}

/// Rewrite output paths in a command to point at `directory`.
fn retarget_output(command: &[String], directory: &Path, playlist: &Path) -> Vec<String> {
    let mut out = command.to_vec();
    // Replace any absolute path containing the old scratch dir.
    for arg in out.iter_mut() {
        if arg.contains("/transcode/") && (arg.ends_with(".m3u8") || arg.ends_with(".ts") || arg.ends_with("init.mp4")) {
            let name = Path::new(arg.as_str())
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            if name.contains('%') {
                *arg = directory.join(name).to_string_lossy().into_owned();
            } else if arg.ends_with(".m3u8") {
                *arg = playlist.to_string_lossy().into_owned();
            } else {
                *arg = directory.join(name).to_string_lossy().into_owned();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::{HlsProfile, PlayMethod};

    fn dummy_plan() -> PlaybackPlan {
        PlaybackPlan {
            play_method: PlayMethod::Transcode,
            protocol: jf_core::MediaProtocol::Hls,
            container: "ts".into(),
            path: PathBuf::from("/media/in.mkv"),
            needs_video_transcode: true,
            needs_audio_transcode: true,
            needs_subtitle_transcode: false,
            direct_stream: false,
            video_codec: Some("libx264".into()),
            audio_codec: Some("aac".into()),
            audio_stream_index: Some(1),
            subtitle_stream_index: None,
            runtime_ticks: 7_200_000_000,
            command: None,
            output_path: None,
        }
    }

    #[tokio::test]
    async fn unknown_session_lookup_is_none() {
        let m = HlsSessionManager::new(std::env::temp_dir().join("jf-hls-test"), Arc::new(Transcoder::new()));
        assert!(m.get("nope").await.is_none());
        assert!(m.get_by_play_session("nope").await.is_none());
        assert_eq!(m.active_count().await, 0);
    }

    #[tokio::test]
    async fn stop_is_idempotent() {
        let m = HlsSessionManager::new(std::env::temp_dir().join("jf-hls-test2"), Arc::new(Transcoder::new()));
        assert!(m.stop("missing").await.is_ok());
    }

    #[tokio::test]
    async fn segment_path_rejects_traversal() {
        let dir = std::env::temp_dir().join("jf-hls-traversal");
        let m = HlsSessionManager::new(&dir, Arc::new(Transcoder::new()));

        let session_dir = dir.join("sess1");
        tokio::fs::create_dir_all(&session_dir).await.unwrap();
        tokio::fs::write(session_dir.join("segment00000.ts"), b"x").await.unwrap();
        tokio::fs::write("/tmp/jf-secret.txt", b"secret").await.unwrap();

        let session = Arc::new(HlsSession {
            id: "sess1".into(),
            play_session_id: "ps1".into(),
            item_id: "item".into(),
            user_id: "user".into(),
            directory: session_dir,
            plan: dummy_plan(),
            created_at: Instant::now(),
            last_access: Mutex::new(Instant::now()),
            process: Mutex::new(None),
        });
        m.sessions.lock().await.insert("sess1".into(), session);

        // A legitimate segment resolves.
        assert!(m.segment_path("sess1", "segment00000.ts").await.is_ok());

        // Traversal attempts must not escape the session directory.
        assert!(m.segment_path("sess1", "../../jf-secret.txt").await.is_err());
        assert!(m.segment_path("sess1", "/tmp/jf-secret.txt").await.is_err());

        let _ = tokio::fs::remove_dir_all(&dir).await;
        let _ = std::fs::remove_file("/tmp/jf-secret.txt");
    }

    #[test]
    fn retarget_output_rewrites_paths() {
        let cmd = vec![
            "-i".to_string(),
            "/media/in.mkv".to_string(),
            "-hls_segment_filename".to_string(),
            "/var/cache/transcode/old/segment%05d.ts".to_string(),
            "/var/cache/transcode/old/output.m3u8".to_string(),
        ];
        let out = retarget_output(&cmd, Path::new("/var/cache/transcode/new"), Path::new("/var/cache/transcode/new/output.m3u8"));
        assert!(out.iter().any(|a| a.contains("/new/segment%05d.ts")));
        assert_eq!(out.last().unwrap(), "/var/cache/transcode/new/output.m3u8");
        // The input must be untouched.
        assert_eq!(out[1], "/media/in.mkv");
    }

    #[test]
    fn profile_constants_are_consistent() {
        assert_eq!(HlsProfile::AVC.container, "ts");
        assert_eq!(HlsProfile::AVC_FMP4.container, "mp4");
    }
}