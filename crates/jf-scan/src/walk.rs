//! Directory traversal and filesystem watching.

use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use walkdir::WalkDir;

/// Extensions treated as playable media.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "m4v", "mov", "wmv", "flv", "webm", "mpg", "mpeg", "ts", "m2ts", "mts",
    "ogv", "3gp", "divx", "vob",
];

/// Extensions treated as audio.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["flac", "mp3", "ogg", "oga", "opus", "m4a", "m4b", "wav", "wma", "aac", "aiff", "ape"];

/// Sidecar files that are never media themselves.
const IGNORED: &[&str] = &["nfo", "jpg", "jpeg", "png", "txt", "srt", "vobsub", "idx", "sub"];

/// Whether a path's extension marks it as a media file.
///
/// Sidecars (`.nfo`, `.srt`, artwork) live beside media in the same
/// directories and must never be indexed as items themselves, so they are
/// checked and rejected before the media extension lists.
pub fn is_media_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else { return false };
    let ext = ext.to_ascii_lowercase();

    if IGNORED.contains(&ext.as_str()) {
        return false;
    }

    VIDEO_EXTENSIONS.contains(&ext.as_str()) || AUDIO_EXTENSIONS.contains(&ext.as_str())
}

/// A filesystem change worth rescanning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeKind {
    pub path: String,
    pub kind: String,
}

/// Collect every media file under `root`, skipping hidden and system dirs.
pub fn collect_media_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip(e.path(), root))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| is_media_file(p))
        .collect()
}

/// Collect image sidecars (posters, fanart) under `root`.
pub fn collect_images(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip(e.path(), root))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))
                .unwrap_or(false)
        })
        .collect()
}

/// Prune hidden files and noisy directories (`@eaDir`, `.trash`, etc.).
fn should_skip(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return true,
    };
    name.starts_with('.')
        || name == "@eaDir"
        || name == "#recycle"
        || name == "@Recycle"
        || name == "System Volume Information"
}

/// Watch `roots` for changes, invoking `on_change` from a background thread.
///
/// Returns a handle that stops watching when dropped. Errors are logged rather
/// than propagated: a watcher failing should degrade to manual scans, not
/// break startup.
pub fn watch<F>(roots: &[PathBuf], mut on_change: F) -> Option<notify::RecommendedWatcher>
where
    F: FnMut(ChangeKind) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "could not create filesystem watcher");
            return None;
        }
    };

    let mut any = false;
    for root in roots {
        match watcher.watch(root, RecursiveMode::Recursive) {
            Ok(()) => any = true,
            Err(e) => tracing::warn!(path = %root.display(), error = %e, "could not watch path"),
        }
    }
    if !any {
        return None;
    }

    std::thread::Builder::new()
        .name("jf-watch".into())
        .spawn(move || {
            // Coalesce bursts of events into distinct paths so a copy of a
            // season directory does not trigger hundreds of rescans.
            let mut pending: HashSet<String> = HashSet::new();
            while let Ok(Ok(event)) = rx.recv() {
                let kind = match event.kind {
                    EventKind::Create(_) => "created",
                    EventKind::Remove(_) => "removed",
                    EventKind::Modify(_) => "modified",
                    _ => continue,
                };
                for path in &event.paths {
                    pending.insert(format!("{kind}|{}", path.to_string_lossy()));
                }

                match rx.recv_timeout(std::time::Duration::from_millis(750)) {
                    Ok(_) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }

                for entry in std::mem::take(&mut pending) {
                    if let Some((kind, path)) = entry.split_once('|') {
                        on_change(ChangeKind { path: path.to_owned(), kind: kind.to_owned() });
                    }
                }
            }
        })
        .ok()?;

    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_detection() {
        assert!(is_media_file(Path::new("a.mkv")));
        assert!(is_media_file(Path::new("a.MP4")));
        assert!(is_media_file(Path::new("a.flac")));
        assert!(!is_media_file(Path::new("a.nfo")));
        assert!(!is_media_file(Path::new("poster.jpg")));
        assert!(!is_media_file(Path::new("README")));
    }

    #[test]
    fn walks_recursively_skipping_noise() {
        let dir = std::env::temp_dir().join(format!("jf-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Season 1")).unwrap();
        std::fs::create_dir_all(dir.join("@eaDir")).unwrap();
        std::fs::write(dir.join("a.mkv"), b"x").unwrap();
        std::fs::write(dir.join("Season 1/b.mkv"), b"x").unwrap();
        std::fs::write(dir.join("@eaDir/c.mkv"), b"x").unwrap();

        let mut found = collect_media_files(&dir);
        found.sort();
        assert_eq!(found.len(), 2, "expected 2 media files, got {found:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}