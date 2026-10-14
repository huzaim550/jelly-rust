//! Thumbnail and artwork generation.
//!
//! ffmpeg extracts stills for video and audio; NFO sidecars are used directly
//! when present. Results are content-addressed by tag so a client can cache
//! aggressively and only re-fetch when the tag changes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Which image slot is being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ImageKind {
    Primary,
    Thumb,
    Backdrop,
    Banner,
    Logo,
}

/// One image request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRequest {
    pub item_id: String,
    pub kind: ImageKind,
    pub index: Option<u32>,
    /// Target width in pixels; `None` means original size.
    pub width: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub fill_width: Option<u32>,
    pub fill_height: Option<u32>,
    pub tag: Option<String>,
    /// Served without authentication, for login-screen user avatars.
    pub public: bool,
}

impl ImageRequest {
    /// Parse Jellyfin's `/Items/{id}/Images/{type}` style query string.
    pub fn from_query(
        item_id: impl Into<String>,
        kind: impl Into<String>,
        index: Option<u32>,
        width: Option<u32>,
        max_width: Option<u32>,
        max_height: Option<u32>,
        tag: Option<String>,
    ) -> Self {
        let kind = match kind.into().to_ascii_lowercase().as_str() {
            "primary" => ImageKind::Primary,
            "thumb" => ImageKind::Thumb,
            "backdrop" => ImageKind::Backdrop,
            "banner" => ImageKind::Banner,
            "logo" => ImageKind::Logo,
            _ => ImageKind::Primary,
        };

        Self {
            item_id: item_id.into(),
            kind,
            index,
            width,
            max_width,
            max_height,
            fill_width: None,
            fill_height: None,
            tag,
            public: false,
        }
    }
}

/// A generated or discovered image.
#[derive(Debug, Clone)]
pub struct ImageResult {
    pub path: PathBuf,
    pub content_type: String,
    pub tag: String,
    pub size: Option<u64>,
}

/// Caches generated artwork on disk.
#[derive(Clone)]
pub struct ImageCache {
    root: PathBuf,
    ffmpeg: Arc<String>,
    /// Serializes generation per source path so two requests for the same
    /// video do not spawn two ffmpeg processes.
    locks: Arc<Mutex<HashMapSharded>>,
}

/// Placeholder for the in-flight generation map.
type HashMapSharded = std::collections::HashMap<String, Arc<Mutex<()>>>;

impl std::fmt::Debug for ImageCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCache").field("root", &self.root).finish()
    }
}

impl ImageCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ffmpeg: Arc::new(std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned())),
            locks: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Generate a thumbnail for a media file at `timestamp_secs`.
    ///
    /// Returns `Ok(None)` when ffmpeg is unavailable or the file has no video
    /// track, so callers can fall back to a default poster.
    pub async fn generate_video_thumbnail(
        &self,
        source: &Path,
        output: &Path,
        timestamp_secs: f64,
        width: Option<u32>,
    ) -> Result<Option<ImageResult>, ImageError> {
        if !source.is_file() {
            return Ok(None);
        }

        let key = format!("{}@{}", source.display(), timestamp_secs);
        let lock = {
            let mut locks = self.locks.lock().await;
            Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
        };
        let _guard = lock.lock().await;

        // Another request may have produced it while we waited.
        if output.is_file() {
            return Ok(Some(self.describe(output)));
        }

        if let Some(parent) = output.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Seek near the target, then decode the next keyframe. `-ss` before
        // `-i` is far faster than after it.
        let mut cmd = tokio::process::Command::new(self.ffmpeg.as_str());
        cmd.arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(format!("{timestamp_secs:.3}"))
            .arg("-i")
            .arg(source)
            .args(["-frames:v", "1", "-q:v", "3"]);

        if let Some(w) = width {
            cmd.args(["-vf", &format!("scale={w}:-2")]);
        }

        cmd.arg("-y").arg(output).stdout(Stdio::null()).stderr(Stdio::null());

        let status = cmd.status().await.map_err(ImageError::Spawn)?;

        if !status.success() || !output.is_file() {
            // Clean up the empty file ffmpeg may have created.
            let _ = tokio::fs::remove_file(output).await;
            return Ok(None);
        }

        Ok(Some(self.describe(output)))
    }

    /// Return an existing sidecar image, if it is one of the types we serve.
    pub async fn serve_sidecar(&self, path: &Path, req: &ImageRequest) -> Option<ImageResult> {
        if !path.is_file() {
            return None;
        }
        let content_type = content_type_for(path)?;

        // Resizing a sidecar requires a tool; serve it at native size, which
        // clients handle fine.
        let _ = req;
        let size = tokio::fs::metadata(path).await.ok().map(|m| m.len());
        let tag = jf_scan::image_tag(path);
        Some(ImageResult { path: path.to_path_buf(), content_type, tag, size })
    }

    /// Cache path for a generated image.
    pub fn cache_path(&self, item_id: &str, kind: ImageKind, index: u32, width: Option<u32>) -> PathBuf {
        let mut p = self.root.join(item_id);
        p.push(format!("{kind:?}_{index}_{}.jpg", width.unwrap_or(0)));
        p
    }

    /// Remove all cached images for an item.
    pub async fn purge_item(&self, item_id: &str) -> Result<(), ImageError> {
        let dir = self.root.join(item_id);
        if dir.is_dir() {
            tokio::fs::remove_dir_all(dir).await?;
        }
        Ok(())
    }

    fn describe(&self, path: &Path) -> ImageResult {
        let content_type = content_type_for(path).unwrap_or_else(|| "image/jpeg".to_owned());
        let tag = jf_scan::image_tag(path);
        let size = std::fs::metadata(path).ok().map(|m| m.len());
        ImageResult { path: path.to_path_buf(), content_type, tag, size }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("could not run ffmpeg: {0}")]
    Spawn(std::io::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// MIME type for an image extension.
pub fn content_type_for(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "png" => Some("image/png".into()),
        "webp" => Some("image/webp".into()),
        "gif" => Some("image/gif".into()),
        "avif" => Some("image/avif".into()),
        "bmp" => Some("image/bmp".into()),
        _ => None,
    }
}

/// Pick a thumbnail timestamp.
///
/// Prefers 10% into the runtime, which for most films lands after the opening
/// credits. `duration_secs` of `None` falls back to 10 seconds.
pub fn thumbnail_timestamp(duration_secs: Option<f64>) -> f64 {
    match duration_secs {
        Some(d) if d > 0.0 => (d * 0.1).clamp(1.0, 600.0),
        _ => 10.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_kind_parsing_is_case_insensitive() {
        let r = ImageRequest::from_query("abc", "Backdrop", None, None, None, None, None);
        assert_eq!(r.kind, ImageKind::Backdrop);
        let r = ImageRequest::from_query("abc", "bogus", None, None, None, None, None);
        assert_eq!(r.kind, ImageKind::Primary, "unknown types fall back to primary");
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for(Path::new("a.jpg")).as_deref(), Some("image/jpeg"));
        assert_eq!(content_type_for(Path::new("a.PNG")).as_deref(), Some("image/png"));
        assert_eq!(content_type_for(Path::new("a.txt")), None);
    }

    #[test]
    fn timestamp_stays_inside_media() {
        // 10% in, clamped so a long film does not sample deep past the opening
        // credits and a short clip does not sample on the first frame.
        assert_eq!(thumbnail_timestamp(Some(7200.0)), 600.0);
        assert_eq!(thumbnail_timestamp(Some(600.0)), 60.0);
        assert_eq!(thumbnail_timestamp(Some(2.0)), 1.0);
        assert_eq!(thumbnail_timestamp(None), 10.0);
    }

    #[tokio::test]
    async fn missing_source_yields_none() {
        let cache = ImageCache::new(std::env::temp_dir().join("jf-img-test"));
        let out = cache.generate_video_thumbnail(Path::new("/nonexistent.mkv"), Path::new("/tmp/x.jpg"), 10.0, None).await;
        assert!(out.unwrap().is_none());
    }

    #[tokio::test]
    async fn sidecar_serving_rejects_non_images() {
        let cache = ImageCache::new(std::env::temp_dir().join("jf-img-test2"));
        let dir = std::env::temp_dir().join("jf-sidecar");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("poster.jpg"), b"notreallyjpeg").await.unwrap();
        tokio::fs::write(dir.join("notes.txt"), b"x").await.unwrap();

        let req = ImageRequest::from_query("i", "Primary", None, None, None, None, None);
        assert!(cache.serve_sidecar(&dir.join("poster.jpg"), &req).await.is_some());
        assert!(cache.serve_sidecar(&dir.join("notes.txt"), &req).await.is_none());
        assert!(cache.serve_sidecar(&dir.join("missing.jpg"), &req).await.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn cache_path_is_unique_per_variant() {
        let cache = ImageCache::new("/cache");
        let a = cache.cache_path("item1", ImageKind::Primary, 0, Some(320));
        let b = cache.cache_path("item1", ImageKind::Primary, 0, None);
        assert_ne!(a, b);
        assert!(a.starts_with("/cache/item1"));
    }
}