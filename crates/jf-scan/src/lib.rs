//! Library scanner.
//!
//! Responsibilities:
//! * walk library roots and detect added/removed/renamed files,
//! * parse `movie (2021).mkv`-style filenames into structured hints,
//! * read local NFO sidecar metadata,
//! * probe containers with ffprobe and extract media streams,
//! * persist items, streams, images, and external ids.
//!
//! Metadata *providers* (TMDB, OMDb, ...) are not implemented here. They plug
//! in through `jf_plugins`' `register_metadata_provider` hook, which keeps this
//! crate free of network concerns and API keys.

pub mod naming;
pub mod nfo;
pub mod probe;
pub mod walk;

pub use naming::{NameParser, ParsedName};
pub use probe::{ProbeResult, Prober};

use jf_db::{Database, Result as DbResult};
use rayon::prelude::*;
use sqlx::Row;
use jf_core::new_item_id;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for a single library.
#[derive(Debug, Clone)]
pub struct LibraryConfig {
    pub id: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub collection_type: String,
}

/// Coarse progress, emitted once per file during a scan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanProgress {
    pub library_id: String,
    pub processed: usize,
    pub total: usize,
    pub fraction: f64,
    pub path: String,
}

/// Aggregated counters for a scan run.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ScanReport {
    pub library_id: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// A library scanner bound to the database.
pub struct Scanner {
    db: Database,
    prober: Arc<Prober>,
}

impl Scanner {
    pub fn new(db: Database) -> Self {
        Self { db, prober: Arc::new(Prober::new()) }
    }

    /// Register a library root. Idempotent on path.
    pub async fn add_library(&self, cfg: &LibraryConfig) -> DbResult<String> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM libraries WHERE path = ?")
                .bind(cfg.path.to_string_lossy().to_string())
                .fetch_optional(self.db.pool())
                .await?;

        if let Some(id) = existing {
            return Ok(id);
        }

        let id = if cfg.id.is_empty() { new_item_id() } else { cfg.id.clone() };
        sqlx::query(
            "INSERT INTO libraries (id, name, path, collection_type, created_at, enabled)
             VALUES (?, ?, ?, ?, ?, 1)",
        )
        .bind(&id)
        .bind(&cfg.name)
        .bind(cfg.path.to_string_lossy().to_string())
        .bind(&cfg.collection_type)
        .bind(jf_db::users::now_iso())
        .execute(self.db.pool())
        .await?;

        info!(library = %cfg.name, path = %cfg.path.display(), "library added");
        Ok(id)
    }

    pub async fn remove_library(&self, id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM libraries WHERE id = ?")
            .bind(id)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    pub async fn libraries(&self) -> DbResult<Vec<LibraryConfig>> {
        let rows = sqlx::query("SELECT id, name, path, collection_type FROM libraries WHERE enabled = 1")
            .fetch_all(self.db.pool())
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| LibraryConfig {
                id: r.get("id"),
                name: r.get("name"),
                path: std::path::PathBuf::from(r.get::<String, _>("path")),
                collection_type: r.get("collection_type"),
            })
            .collect())
    }

    /// Scan every registered library sequentially.
    pub async fn scan_all(&self) -> DbResult<Vec<ScanReport>> {
        let mut reports = Vec::new();
        for lib in self.libraries().await? {
            reports.push(self.scan_library(&lib, None).await?);
        }
        Ok(reports)
    }

    /// Scan one library: discover files, probe them, and reconcile the tree.
    pub async fn scan_library(
        &self,
        lib: &LibraryConfig,
        mut progress: Option<&mut (dyn FnMut(ScanProgress) + Send)>,
    ) -> DbResult<ScanReport> {
        let mut report = ScanReport { library_id: lib.id.clone(), ..Default::default() };

        if !lib.path.is_dir() {
            warn!(path = %lib.path.display(), "library path is not a directory");
            report.errors += 1;
            return Ok(report);
        }

        let root_id = self.ensure_library_root(lib).await?;

        // Collect candidate media files off the async runtime; ffprobe is slow
        // and blocking, and libraries can hold tens of thousands of entries.
        let entries = walk::collect_media_files(&lib.path);

        // Probe across all cores. `Prober::probe` is synchronous and blocking,
        // which is exactly what rayon worker threads are for, so this needs no
        // spawn_blocking hop. Ordering does not matter: results are keyed by
        // path and used to reconcile the tree afterwards.
        let prober = Arc::clone(&self.prober);
        let probes: Vec<(std::path::PathBuf, Option<ProbeResult>)> = entries
            .par_iter()
            .map(|path| (path.clone(), prober.probe_lossy(path)))
            .collect();

        let parser = NameParser::new();
        let mut seen_paths: Vec<String> = Vec::with_capacity(entries.len());

        for (i, (path, probe)) in probes.iter().enumerate() {
            let path_str = path.to_string_lossy().to_string();
            seen_paths.push(path_str.clone());

            // Report before persisting, so a slow parser does not look like
            // the scan has stalled on the UI.
            if let Some(cb) = progress.as_mut() {
                let processed = i + 1;
                let total = probes.len();
                cb(ScanProgress {
                    library_id: lib.id.clone(),
                    processed,
                    total,
                    fraction: if total == 0 { 1.0 } else { processed as f64 / total as f64 },
                    path: path_str.clone(),
                });
            }

            if probe.is_none() {
                // Not a container ffprobe understands; still track it so a
                // transient ffprobe failure does not delete a known item.
                debug!(path = %path_str, "no probe result");
                report.skipped += 1;
                continue;
            }

            let parsed = parser.parse(&path);
            let (parent_id, series_id) =
                self.ensure_parent(lib, &root_id, &lib.path, &path, Some(&parsed))
                    .await?;

            match self
                .upsert_item(lib, &path, Some(&parsed), probe.as_ref(), &parent_id, series_id.as_deref())
                .await
            {
                Ok(true) => report.added += 1,
                Ok(false) => report.updated += 1,
                Err(e) => {
                    warn!(path = %path_str, error = %e, "failed to persist item");
                    report.errors += 1;
                }
            }
        }

        report.removed = self.reconcile_removed(&root_id, &seen_paths).await?;
        info!(library = %lib.name, ?report, "scan complete");
        Ok(report)
    }

    /// Ensure the library's root aggregate-folder item exists.
    async fn ensure_library_root(&self, lib: &LibraryConfig) -> DbResult<String> {
        // Scoped by kind: in a flat TV library the series folder also sits on
        // the library path, and the root must not adopt it.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM items WHERE library_id = ? AND path = ? AND kind = 'UserView'")
                .bind(&lib.id)
                .bind(lib.path.to_string_lossy().to_string())
                .fetch_optional(self.db.pool())
                .await?;

        if let Some(id) = existing {
            // Refresh the kind on rescan so a root created by an older version
            // self-heals instead of staying mis-typed forever.
            sqlx::query("UPDATE items SET kind = 'UserView' WHERE id = ?")
                .bind(&id)
                .execute(self.db.pool())
                .await?;
            return Ok(id);
        }

        let id = new_item_id();
        // The root is a navigation node. Its collection type is what tells a
        // client how to present the library; its kind stays a folder so it is
        // never mistaken for a playable item.
        let kind = "UserView";

        sqlx::query(
            "INSERT INTO items
                (id, library_id, kind, media_type, name, sort_name, path, is_folder,
                 date_created, date_modified, genres_json, tags_json, artists_json,
                 taglines_json, people_json, media_sources_json, image_tags_json, backdrop_json)
             VALUES (?, ?, ?, NULL, ?, ?, ?, 1, ?, ?, '[]', '[]', '[]', '[]', '[]', '[]', '{}', '[]')",
        )
        .bind(&id)
        .bind(&lib.id)
        .bind(kind)
        .bind(&lib.name)
        .bind(&lib.name)
        .bind(lib.path.to_string_lossy().to_string())
        .bind(jf_db::users::now_iso())
        .bind(jf_db::users::now_iso())
        .execute(self.db.pool())
        .await?;

        Ok(id)
    }

    /// Resolve (creating if needed) the Series and Season folders a media file
    /// belongs to, mirroring Jellyfin's `Show/Season N/episode` layout.
    ///
    /// Returns the parent id together with the owning series id, which is
    /// `None` for anything outside a TV library. Episodes need both: the
    /// parent links them into the folder tree, while the series id is what
    /// "Next Up" groups and filters on.
    async fn ensure_parent(
        &self,
        lib: &LibraryConfig,
        root_id: &str,
        lib_root: &Path,
        path: &Path,
        parsed: Option<&ParsedName>,
    ) -> DbResult<(String, Option<String>)> {
        let is_tv = lib.collection_type.eq_ignore_ascii_case("tvshows");
        if !is_tv {
            return Ok((root_id.to_owned(), None));
        }

        let Some(parsed) = parsed else { return Ok((root_id.to_owned(), None)) };

        // Folders are identified by the directory that holds them, never by the
        // media file's own path: two episodes of one series would otherwise
        // each claim a different "series" folder and overwrite each other.
        let relative_file = path.strip_prefix(lib_root).unwrap_or(path);
        let episode_dir = relative_file.parent().unwrap_or(Path::new(""));

        // In Jellyfin's layout the show folder is the one holding the season
        // folders, so step up when the containing directory is a season.
        let series_dir = match episode_dir.file_name() {
            Some(name) if naming::is_season_dir(&name.to_string_lossy()) => {
                episode_dir.parent().filter(|p| !p.as_os_str().is_empty())
            }
            Some(_) => Some(episode_dir).filter(|p| !p.as_os_str().is_empty()),
            None => None,
        };

        // The layout is the better source of a show name; the filename is the
        // fallback for libraries that keep every episode flat in one folder.
        let show = series_dir
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter(|n| !n.is_empty())
            .or_else(|| parsed.show_name());

        let mut parent = root_id.to_owned();
        let mut series_id: Option<String> = None;

        // Show folder.
        if let Some(show) = show {
            parent = self
                .ensure_folder(
                    lib,
                    &parent,
                    &show,
                    "Series",
                    series_dir.unwrap_or(episode_dir),
                    None,
                )
                .await?;
            series_id = Some(parent.clone());
        }

        // Season folder. It hangs off the series, so it inherits the series id.
        if let Some(season) = parsed.season_number() {
            let season_name = format!("Season {season}");
            parent = self
                .ensure_folder(lib, &parent, &season_name, "Season", episode_dir, series_id.as_deref())
                .await?;
        }

        Ok((parent, series_id))
    }

    async fn ensure_folder(
        &self,
        lib: &LibraryConfig,
        parent_id: &str,
        name: &str,
        kind: &str,
        relative: &Path,
        series_id: Option<&str>,
    ) -> DbResult<String> {
        let path = if relative.as_os_str().is_empty() {
            // The file sits directly in the library root, so the series and
            // season folders belong to the root itself.
            lib.path.to_string_lossy().into_owned()
        } else {
            format!("{}/{}", lib.path.to_string_lossy(), relative.display())
        };

        // Scoped by kind on purpose: a flat TV library puts its episodes
        // straight in the library root, so the series folder lands on the same
        // path as the root `UserView` row and must not adopt it.
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM items WHERE library_id = ? AND path = ? AND kind = ?",
        )
        .bind(&lib.id)
        .bind(&path)
        .bind(kind)
        .fetch_optional(self.db.pool())
        .await?
        {
            // A series is its own series; a season belongs to its parent
            // series. Repairing on every scan means libraries indexed by an
            // older build pick the ids up without needing a rescan.
            let desired: Option<String> = if kind == "Series" {
                Some(id.clone())
            } else {
                series_id.map(str::to_owned)
            };
            if let Some(desired) = desired {
                sqlx::query("UPDATE items SET series_id = ? WHERE id = ? AND series_id IS NULL")
                    .bind(&desired)
                    .bind(&id)
                    .execute(self.db.pool())
                    .await?;
            }
            return Ok(id);
        }

        let id = new_item_id();
        let owner: Option<String> = if kind == "Series" {
            Some(id.clone())
        } else {
            series_id.map(str::to_owned)
        };
        sqlx::query(
            "INSERT INTO items
                (id, library_id, kind, media_type, name, sort_name, path, is_folder,
                 parent_id, series_id, date_created, date_modified, genres_json, tags_json,
                 artists_json, taglines_json, people_json, media_sources_json,
                 image_tags_json, backdrop_json)
             VALUES (?, ?, ?, NULL, ?, ?, ?, 1, ?, ?, ?, ?, '[]', '[]', '[]', '[]', '[]', '[]', '{}', '[]')",
        )
        .bind(&id)
        .bind(&lib.id)
        .bind(kind)
        .bind(name)
        .bind(name)
        .bind(&path)
        .bind(parent_id)
        .bind(&owner)
        .bind(jf_db::users::now_iso())
        .bind(jf_db::users::now_iso())
        .execute(self.db.pool())
        .await?;

        Ok(id)
    }

    /// Insert or update one media item. Returns `true` if newly created.
    async fn upsert_item(
        &self,
        lib: &LibraryConfig,
        path: &Path,
        parsed: Option<&ParsedName>,
        probe: Option<&ProbeResult>,
        parent_id: &str,
        series_id: Option<&str>,
    ) -> DbResult<bool> {
        let path_str = path.to_string_lossy().to_string();
        let probe = probe.expect("probe checked by caller");

        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM items WHERE path = ? AND is_folder = 0")
                .bind(&path_str)
                .fetch_optional(self.db.pool())
                .await?;

        // Capture the flag before `existing` is consumed by the match below.
        let is_new = existing.is_none();
        let id = existing.unwrap_or_else(new_item_id);

        let name = parsed.map(|p| p.title.clone()).unwrap_or_else(|| {
            path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        });

        let (kind, media_type) = classify(&lib.collection_type, parsed);

        let year = parsed.and_then(|p| p.year).map(|y| y as i32);
        let runtime = probe.duration_secs.map(jf_core::secs_to_ticks);

        let media_sources = serde_json::to_string(&[probe.to_media_source_info()]).unwrap_or_else(|_| "[]".into());

        if !is_new {
            sqlx::query(
                "UPDATE items SET name = ?, sort_name = ?, size = ?, container = ?,
                    date_modified = ?, production_year = ?, run_time_ticks = ?,
                    parent_id = ?, series_id = ?, media_sources_json = ?
                 WHERE id = ?",
            )
            .bind(&name)
            .bind(parsed.map(|p| p.sort_title()).unwrap_or_else(|| name.clone()))
            .bind(probe.size)
            .bind(&probe.container)
            .bind(jf_db::users::now_iso())
            .bind(year)
            .bind(runtime)
            .bind(parent_id)
            .bind(series_id)
            .bind(&media_sources)
            .bind(&id)
            .execute(self.db.pool())
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO items
                    (id, library_id, kind, media_type, name, sort_name, path, size,
                     container, production_year, run_time_ticks, parent_id,
                     series_id, series_name, season_name, episode_title, index_number,
                     parent_index_number, date_created, date_modified, date_last_refreshed,
                     genres_json, tags_json, artists_json, taglines_json, people_json,
                     media_sources_json, image_tags_json, backdrop_json, is_folder)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '[]', '[]', '[]', '[]', '[]', ?, '{}', '[]', 0)",
            )
            .bind(&id)
            .bind(&lib.id)
            .bind(kind)
            .bind(media_type)
            .bind(&name)
            .bind(parsed.map(|p| p.sort_title()).unwrap_or_else(|| name.clone()))
            .bind(&path_str)
            .bind(probe.size)
            .bind(&probe.container)
            .bind(year)
            .bind(runtime)
            .bind(parent_id)
            .bind(series_id)
            .bind(parsed.and_then(|p| p.show_name()))
            .bind(parsed.and_then(|p| p.season_number()).map(|s| format!("Season {s}")))
            .bind(parsed.and_then(|p| p.episode_title.clone()))
            .bind(parsed.and_then(|p| p.episode_number()).map(|n| n as i32))
            .bind(parsed.and_then(|p| p.season_number()).map(|n| n as i32))
            .bind(jf_db::users::now_iso())
            .bind(jf_db::users::now_iso())
            .bind(jf_db::users::now_iso())
            .bind(&media_sources)
            .execute(self.db.pool())
            .await?;
        }

        self.replace_streams(&id, probe).await?;
        Ok(is_new)
    }

    /// Streams are immutable per file, so replace wholesale on each scan.
    async fn replace_streams(&self, item_id: &str, probe: &ProbeResult) -> DbResult<()> {
        sqlx::query("DELETE FROM media_streams WHERE item_id = ?")
            .bind(item_id)
            .execute(self.db.pool())
            .await?;

        for stream in &probe.streams {
            sqlx::query(
                "INSERT INTO media_streams
                    (id, item_id, stream_index, stream_type, codec, language, title,
                     width, height, channels, channel_layout, sample_rate, bitrate,
                     is_default, is_forced, is_external, path)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(new_item_id())
            .bind(item_id)
            .bind(stream.index)
            .bind(stream.stream_type.as_str())
            .bind(&stream.codec)
            .bind(&stream.language)
            .bind(&stream.title)
            .bind(stream.width)
            .bind(stream.height)
            .bind(stream.channels)
            .bind(&stream.channel_layout)
            .bind(stream.sample_rate)
            .bind(stream.bitrate)
            .bind(stream.is_default as i64)
            .bind(stream.is_forced as i64)
            .bind(stream.is_external as i64)
            .bind::<Option<String>>(None)
            .execute(self.db.pool())
            .await?;
        }

        Ok(())
    }

    /// Delete items whose backing file disappeared, then drop the series and
    /// season folders that nothing points at any more.
    async fn reconcile_removed(&self, root_id: &str, seen: &[String]) -> DbResult<usize> {
        // Only media files are compared against `seen`: a Series or Season row
        // is named after a *directory*, which never appears in the file list,
        // so including folders here would delete the whole TV tree on every
        // scan. Folders are pruned separately, below.
        let library_id: String =
            sqlx::query_scalar("SELECT library_id FROM items WHERE id = ?")
                .bind(root_id)
                .fetch_one(self.db.pool())
                .await?;

        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, path FROM items
             WHERE library_id = ? AND is_folder = 0 AND id != ?",
        )
        .bind(&library_id)
        .bind(root_id)
        .fetch_all(self.db.pool())
        .await?;

        let mut removed = 0usize;
        for (id, path) in rows {
            if !seen.iter().any(|s| s == &path) {
                // Delete by id: two rows can legitimately share a path (a flat
                // TV library's series folder and the library root).
                sqlx::query("DELETE FROM items WHERE id = ?")
                    .bind(&id)
                    .execute(self.db.pool())
                    .await?;
                removed += 1;
            }
        }

        // A series or season with no children left is a leftover from a folder
        // the user deleted or renamed.
        let stale: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM items
             WHERE library_id = ? AND kind IN ('Series', 'Season')
               AND id NOT IN (SELECT parent_id FROM items WHERE parent_id IS NOT NULL)",
        )
        .bind(&library_id)
        .fetch_all(self.db.pool())
        .await?;

        for id in stale {
            sqlx::query("DELETE FROM items WHERE id = ?")
                .bind(&id)
                .execute(self.db.pool())
                .await?;
            removed += 1;
        }

        Ok(removed)
    }

    /// Record a resolved external provider id (set by metadata plugins).
    pub async fn set_external_id(
        &self,
        item_id: &str,
        provider: &str,
        key: &str,
        value: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO item_external_ids (item_id, provider, provider_key, value)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(item_id, provider, provider_key)
             DO UPDATE SET value = excluded.value",
        )
        .bind(item_id)
        .bind(provider)
        .bind(key)
        .bind(value)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// Look up an item by a provider id, used to attach fetched metadata.
    pub async fn find_by_external_id(
        &self,
        provider: &str,
        key: &str,
        value: &str,
    ) -> DbResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT item_id FROM item_external_ids
             WHERE provider = ? AND provider_key = ? AND value = ?",
        )
        .bind(provider)
        .bind(key)
        .bind(value)
        .fetch_optional(self.db.pool())
        .await?)
    }
}

/// Map a library's collection type plus filename hints onto an item kind.
fn classify(collection_type: &str, parsed: Option<&ParsedName>) -> (&'static str, Option<&'static str>) {
    if collection_type.eq_ignore_ascii_case("movies") {
        ("Movie", Some("Video"))
    } else if collection_type.eq_ignore_ascii_case("tvshows") {
        match parsed.and_then(|p| p.season_number()) {
            Some(_) => ("Episode", Some("Video")),
            None => ("Series", None),
        }
    } else if collection_type.eq_ignore_ascii_case("music") {
        ("Audio", Some("Audio"))
    } else if collection_type.eq_ignore_ascii_case("photos") {
        ("Photo", Some("Photo"))
    } else if collection_type.eq_ignore_ascii_case("books") {
        ("Book", Some("Book"))
    } else {
        ("Video", Some("Video"))
    }
}

/// Stable short digest of a string, used for cache-busting image tags.
///
/// NFO artwork filenames conventionally use an MD5 of the sidecar path. MD5 is
/// unsuitable here (collision attacks are cheap and the value is client-facing),
/// so this uses the first 24 hex chars of SHA-256 instead: same shape, no
/// cryptographic weakness, and the only requirement is that it is stable.
pub fn md5_short(input: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(input.as_bytes()))[..24].to_owned()
}

/// Content tag for an image, so clients can cache-bust on change.
pub fn image_tag(path: &Path) -> String {
    let meta = std::fs::metadata(path);
    let (len, mtime) = match meta {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    };

    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(len.to_le_bytes());
    if let Some(t) = mtime.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
        hasher.update(t.as_nanos().to_le_bytes());
    }
    hex::encode(hasher.finalize())[..32].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Encode a one-second clip of real media, for tests that need the scanner
    /// to actually index something rather than skip it.
    ///
    /// Returns `None` when ffmpeg is unavailable, so tests can bail out instead
    /// of failing on a machine without it.
    fn real_media_fixture(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return None;
        }

        let file = dir.join(name);
        let ok = Command::new("ffmpeg")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "aac"])
            .arg(&file)
            .output()
            .map(|s| s.status.success())
            .unwrap_or(false);

        if !ok {
            return None;
        }
        Some(file)
    }

    /// Progress must be reported once per file, with monotonically increasing
    /// counters, so the dashboard percentage climbs instead of jumping from
    /// 0 straight to 100 when the scan finally finishes.
    #[tokio::test]
    async fn scan_reports_progress_for_every_file() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Four separately-named but identical movies. Real bytes are required:
        // the scanner skips files ffprobe cannot open, so dummy `.mkv` files
        // would never reach the progress callback with an item to add.
        let source = match real_media_fixture(dir.path(), "source.mkv") {
            Some(p) => p,
            None => {
                eprintln!("skipped: ffmpeg unavailable");
                return;
            }
        };
        for i in 1..=4 {
            std::fs::copy(&source, dir.path().join(format!("Film {i:02} (2001).mkv")))
                .expect("copy fixture");
        }
        let _ = std::fs::remove_file(&source);

        let db = Database::connect_memory().await.expect("memory db");
        let scanner = Scanner::new(db);

        let mut cfg = LibraryConfig {
            id: String::new(),
            name: "Movies".into(),
            path: dir.path().to_path_buf(),
            collection_type: "movies".into(),
        };
        cfg.id = scanner.add_library(&cfg).await.expect("add library");

        let mut seen: Vec<ScanProgress> = Vec::new();
        let report = scanner
            .scan_library(&cfg, Some(&mut |p| seen.push(p)))
            .await
            .expect("scan");

        assert_eq!(report.added, 4, "every fixture should be indexed; {report:?}");
        assert_eq!(report.skipped, 0, "none should be unprobeable");
        assert_eq!(seen.len(), 4, "one callback per file");

        let processed: Vec<usize> = seen.iter().map(|p| p.processed).collect();
        assert_eq!(processed, vec![1, 2, 3, 4], "counters are 1-based and climb");

        assert!(seen.iter().all(|p| p.total == 4), "total is known up front");

        let fractions: Vec<f64> = seen.iter().map(|p| p.fraction).collect();
        assert!(
            fractions.windows(2).all(|w| w[0] < w[1]),
            "fraction must strictly increase: {fractions:?}"
        );
        assert!((fractions[0] - 0.25).abs() < 1e-9);
        assert!((fractions[3] - 1.0).abs() < 1e-9);

        assert!(
            seen.iter().all(|p| p.path.ends_with(".mkv")),
            "the reported path is the file being handled"
        );
        assert_eq!(seen[0].library_id, cfg.id);
    }

    /// A scan with nothing to do still reports completion rather than never
    /// firing the callback (the UI would sit at 0% forever).
    #[tokio::test]
    async fn scan_of_an_empty_library_reports_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::connect_memory().await.expect("memory db");
        let scanner = Scanner::new(db);

        let mut cfg = LibraryConfig {
            id: String::new(),
            name: "Empty".into(),
            path: dir.path().to_path_buf(),
            collection_type: "movies".into(),
        };
        cfg.id = scanner.add_library(&cfg).await.expect("add library");

        let mut seen: Vec<ScanProgress> = Vec::new();
        let report = scanner
            .scan_library(&cfg, Some(&mut |p| seen.push(p)))
            .await
            .expect("scan");

        assert_eq!(report.added, 0);
        assert!(seen.is_empty());
    }
}