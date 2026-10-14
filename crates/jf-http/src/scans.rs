//! Background library scanning with progress reporting.
//!
//! A scan of a large library takes minutes, so it runs detached from the
//! request that started it and reports through the server-sent event stream.
//! Two things make this safe to run concurrently:
//!
//! * A single in-flight scan at a time. Two scans would race on the same rows
//!   and produce duplicate items.
//! * Progress is kept per library, so the dashboard can show what is happening
//!   without polling the database.

use crate::state::AppState;
use jf_core::BaseItemKind;
use jf_plugins::{MetadataQuery, MetadataResult};
use jf_scan::ScanReport;
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Where a library is in its scan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// `Idle`, `Scanning`, `Done`, or `Failed`.
    pub state: String,
    /// Fraction complete, 0.0 to 1.0. `None` while the total is unknown.
    pub progress: Option<f64>,
    /// Items added or updated by this scan.
    pub processed: usize,
    /// Human-readable summary once finished.
    pub detail: Option<String>,
}

/// A plain OS mutex rather than a tokio one: the scanner reports progress
/// from inside a blocking, synchronous callback, where `.await` is not
/// available. Every critical section here is a map insert or a clone, so
/// parking a task on it would be a bug anyway.
type StatusMap = Arc<std::sync::Mutex<HashMap<String, ScanStatus>>>;

/// Tracks scan state and serializes scan runs.
///
/// The shared fields are `Arc` so `refresh_all` can hand them to a spawned
/// task without borrowing `self`, which would not be `'static`.
pub struct ScanManager {
    /// Bound after `AppState` is constructed, since the manager lives inside it.
    app: Arc<Mutex<Option<Arc<AppState>>>>,
    status: StatusMap,
    /// Held for the duration of a scan so a second request cannot start one.
    running: Arc<Mutex<()>>,
}

impl ScanManager {
    /// Build an unbound manager. Call [`ScanManager::bind`] once the owning
    /// `AppState` exists.
    pub fn empty() -> Self {
        Self {
            app: Arc::new(Mutex::new(None)),
            status: Arc::new(std::sync::Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(())),
        }
    }

    /// Attach the application state.
    pub fn bind(&self, app: Arc<AppState>) {
        // Called during startup, before any scan can be requested.
        if let Ok(mut slot) = self.app.try_lock() {
            *slot = Some(app);
        }
    }

    /// Current status for every library that has been scanned this run.
    pub async fn statuses(&self) -> HashMap<String, ScanStatus> {
        self.status.lock().unwrap().clone()
    }

    /// Whether a scan is currently running.
    ///
    /// Uses `try_lock` so this never blocks: a caller asking "is anything
    /// happening?" should get an immediate answer either way.
    pub async fn is_running(&self) -> bool {
        self.running.try_lock().is_err()
    }

    /// Status for one library, if known.
    pub async fn status(&self, library_id: &str) -> Option<ScanStatus> {
        self.status.lock().unwrap().get(library_id).cloned()
    }

    /// Start a scan of every library, detached from the caller.
    ///
    /// Returns immediately; the dashboard follows progress over SSE. A second
    /// call while a scan is running waits for that scan rather than starting a
    /// second one.
    pub async fn refresh_all(&self) -> Result<(), jf_db::DbError> {
        let Some(app) = self.app.lock().await.clone() else {
            // Unbound: nothing to scan. Treat as a no-op rather than panicking.
            return Ok(());
        };
        let libraries = app.scanner.libraries().await?;
        if libraries.is_empty() {
            app.emit("scanComplete", serde_json::json!({ "libraries": 0 }));
            return Ok(());
        }

        let status = Arc::clone(&self.status);
        let running = Arc::clone(&self.running);

        // Publish queued state before returning, so a client polling
        // immediately after requesting a scan cannot miss the start.
        for library in &libraries {
            set_status(
                &status,
                &library.id,
                ScanStatus {
                    state: "Queued".into(),
                    progress: None,
                    processed: 0,
                    detail: None,
                },
            );
        }

        tokio::spawn(async move {
            let _guard = running.lock().await;

            for library in &libraries {
                set_status(
                    &status,
                    &library.id,
                    ScanStatus {
                        state: "Scanning".into(),
                        progress: Some(0.0),
                        processed: 0,
                        detail: None,
                    },
                );

                app.emit(
                    "scanStarted",
                    serde_json::json!({
                        "libraryId": library.id,
                        "name": library.name,
                        "collectionType": library.collection_type,
                    }),
                );

                match app
                    .scanner
                    .scan_library(
                        library,
                        Some(&mut |progress: jf_scan::ScanProgress| {
                            // Keep the polled status in step so the dashboard
                            // percentage climbs file by file, and push the same
                            // number over SSE for clients that listen live.
                            set_status(
                                &status,
                                &library.id,
                                ScanStatus {
                                    state: "Scanning".into(),
                                    progress: Some(progress.fraction),
                                    processed: progress.processed,
                                    detail: Some(progress.path.clone()),
                                },
                            );
                            app.emit(
                                "scanProgress",
                                serde_json::json!({
                                    "libraryId": progress.library_id,
                                    "processed": progress.processed,
                                    "total": progress.total,
                                    "fraction": progress.fraction,
                                    "path": progress.path,
                                }),
                            );
                        }),
                    )
                    .await
                {
                    Ok(report) => {
                        if let Err(error) = refresh_metadata(&app, &library.id).await {
                            tracing::warn!(
                                library = %library.name,
                                error = %error,
                                "metadata refresh failed"
                            );
                        }
                        let detail = describe(&report);
                        tracing::info!(library = %library.name, %detail, "scan finished");

                        set_status(
                            &status,
                            &library.id,
                            ScanStatus {
                                state: "Done".into(),
                                progress: Some(1.0),
                                processed: report.added + report.updated,
                                detail: Some(detail),
                            },
                        );

                        app.emit(
                            "scanComplete",
                            serde_json::json!({
                                "libraryId": library.id,
                                "name": library.name,
                                "added": report.added,
                                "updated": report.updated,
                                "removed": report.removed,
                                "errors": report.errors,
                            }),
                        );

                        // Tell plugins a scan finished so metadata providers can react.
                        app.plugins
                            .dispatch_event(jf_plugins::PluginEvent::ScanComplete {
                                library_id: library.id.clone(),
                            })
                            .await;
                    }
                    Err(e) => {
                        tracing::error!(library = %library.name, error = %e, "scan failed");

                        set_status(
                            &status,
                            &library.id,
                            ScanStatus {
                                state: "Failed".into(),
                                progress: None,
                                processed: 0,
                                detail: Some(e.to_string()),
                            },
                        );

                        app.emit(
                            "scanFailed",
                            serde_json::json!({
                                "libraryId": library.id,
                                "name": library.name,
                                "error": e.to_string(),
                            }),
                        );
                    }
                }
            }
        });

        Ok(())
    }
}

pub async fn refresh_metadata(app: &AppState, library_id: &str) -> anyhow::Result<usize> {
    let report = app.plugins.report().await;
    if !report.loaded.iter().any(|plugin| plugin.id == "omdb") {
        return Ok(0);
    }

    let configured = app
        .plugins
        .plugin_config_get("omdb", "api_key")
        .await?
        .is_some_and(|key| !key.trim().is_empty())
        || std::env::var("OMDB_API_KEY").is_ok_and(|key| !key.trim().is_empty());
    if !configured {
        return Ok(0);
    }

    let rows = sqlx::query(
        "SELECT id, name, kind, production_year, series_name,
                parent_index_number, index_number
         FROM items
         WHERE library_id = ? AND is_folder = 0 AND date_last_refreshed IS NULL
           AND kind IN ('Movie', 'Series', 'Episode')",
    )
    .bind(library_id)
    .fetch_all(app.db.pool())
    .await?;

    let mut refreshed = 0;
    for row in rows {
        let item_id: String = row.get("id");
        let name: String = row.get("name");
        let raw_kind: String = row.get("kind");
        let series_name: Option<String> = row.get("series_name");
        let season: Option<i32> = row.get("parent_index_number");
        let episode: Option<i32> = row.get("index_number");

        let (kind, title) = match raw_kind.as_str() {
            "Movie" => (BaseItemKind::Movie, name),
            "Series" => (BaseItemKind::Series, name),
            "Episode" => {
                let Some(series_name) = series_name else { continue };
                let (Some(season), Some(episode)) = (season, episode) else { continue };
                (
                    BaseItemKind::Episode,
                    format!("{series_name} S{season:02}E{episode:02}"),
                )
            }
            _ => continue,
        };

        let known_ids = sqlx::query(
            "SELECT provider, value FROM item_external_ids WHERE item_id = ?",
        )
        .bind(&item_id)
        .fetch_all(app.db.pool())
        .await?
        .into_iter()
        .map(|id| (id.get::<String, _>("provider"), id.get::<String, _>("value")))
        .collect::<HashMap<_, _>>();

        let query = MetadataQuery {
            title,
            year: row.get::<Option<i32>, _>("production_year").map(|year| year as u32),
            kind,
            known_ids,
            preferred_languages: vec!["en".to_owned()],
        };

        match app.plugins.fetch_metadata("omdb", &query, 10_000).await {
            Ok(Some(metadata)) => {
                persist_metadata(app, &item_id, &metadata).await?;
                refreshed += 1;
            }
            Ok(None) => {
                sqlx::query("UPDATE items SET date_last_refreshed = ? WHERE id = ?")
                    .bind(jf_db::users::now_iso())
                    .bind(&item_id)
                    .execute(app.db.pool())
                    .await?;
            }
            Err(error) => {
                tracing::warn!(item = %item_id, error = %error, "OMDb metadata lookup failed");
            }
        }
    }

    Ok(refreshed)
}

async fn persist_metadata(
    app: &AppState,
    item_id: &str,
    metadata: &MetadataResult,
) -> anyhow::Result<()> {
    let genres = serde_json::to_string(&metadata.genres)?;
    let taglines = metadata
        .tagline
        .as_ref()
        .map(|tagline| serde_json::to_string(&[tagline]))
        .transpose()?;
    let people = metadata
        .people
        .iter()
        .enumerate()
        .map(|(index, person)| {
            serde_json::json!({
                "id": format!("{item_id}-person-{index}"),
                "name": person.name,
                "type": if person.role.is_empty() {
                    format!("{:?}", person.kind)
                } else {
                    person.role.clone()
                },
                "primaryImageTag": null,
            })
        })
        .collect::<Vec<_>>();
    let people = serde_json::to_string(&people)?;

    sqlx::query(
        "UPDATE items SET
            name = COALESCE(?, name),
            sort_name = COALESCE(?, sort_name),
            production_year = COALESCE(?, production_year),
            overview = COALESCE(?, overview),
            premiere_date = COALESCE(?, premiere_date),
            community_rating = COALESCE(?, community_rating),
            official_rating = COALESCE(?, official_rating),
            status = COALESCE(?, status),
            run_time_ticks = COALESCE(?, run_time_ticks),
            genres_json = CASE WHEN ? = '[]' THEN genres_json ELSE ? END,
            taglines_json = COALESCE(?, taglines_json),
            people_json = CASE WHEN ? = '[]' THEN people_json ELSE ? END,
            date_last_refreshed = ?
         WHERE id = ?",
    )
    .bind(&metadata.title)
    .bind(&metadata.sort_title)
    .bind(metadata.year)
    .bind(&metadata.overview)
    .bind(&metadata.premiere_date)
    .bind(metadata.community_rating)
    .bind(&metadata.official_rating)
    .bind(&metadata.status)
    .bind(metadata.run_time_ticks)
    .bind(&genres)
    .bind(&genres)
    .bind(taglines)
    .bind(&people)
    .bind(&people)
    .bind(jf_db::users::now_iso())
    .bind(item_id)
    .execute(app.db.pool())
    .await?;

    for (provider, value) in &metadata.external_ids {
        sqlx::query(
            "INSERT INTO item_external_ids (item_id, provider, provider_key, value)
             VALUES (?, ?, 'default', ?)
             ON CONFLICT(item_id, provider, provider_key)
             DO UPDATE SET value = excluded.value",
        )
        .bind(item_id)
        .bind(provider)
        .bind(value)
        .execute(app.db.pool())
        .await?;
    }

    for image in &metadata.images {
        if image.kind != jf_plugins::ImageKind::Primary
            || !image.path.starts_with("https://m.media-amazon.com/")
        {
            continue;
        }

        let tag = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO images (item_id, image_type, index_num, path, width, height, size, tag)
             VALUES (?, 'Primary', ?, ?, ?, ?, NULL, ?)
             ON CONFLICT(item_id, image_type, index_num)
             DO UPDATE SET path = excluded.path, width = excluded.width,
                           height = excluded.height, tag = excluded.tag
             WHERE images.path LIKE 'https://m.media-amazon.com/%'",
        )
        .bind(item_id)
        .bind(image.index as i64)
        .bind(&image.path)
        .bind(image.width.map(i64::from))
        .bind(image.height.map(i64::from))
        .bind(&tag)
        .execute(app.db.pool())
        .await?;

        let saved_path: Option<String> = sqlx::query_scalar(
            "SELECT path FROM images WHERE item_id = ? AND image_type = 'Primary' AND index_num = ?",
        )
        .bind(item_id)
        .bind(image.index as i64)
        .fetch_optional(app.db.pool())
        .await?;
        if saved_path.as_deref() == Some(image.path.as_str()) {
            let image_tags_raw: String =
                sqlx::query_scalar("SELECT image_tags_json FROM items WHERE id = ?")
                    .bind(item_id)
                    .fetch_one(app.db.pool())
                    .await?;
            let mut image_tags: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&image_tags_raw).unwrap_or_default();
            image_tags.insert("Primary".to_owned(), tag);
            sqlx::query("UPDATE items SET image_tags_json = ? WHERE id = ?")
                .bind(serde_json::to_string(&image_tags)?)
                .bind(item_id)
                .execute(app.db.pool())
                .await?;
        }
    }

    Ok(())
}

fn describe(report: &ScanReport) -> String {
    let mut parts = Vec::new();
    if report.added > 0 {
        parts.push(format!("{} added", report.added));
    }
    if report.updated > 0 {
        parts.push(format!("{} updated", report.updated));
    }
    if report.removed > 0 {
        parts.push(format!("{} removed", report.removed));
    }
    if report.errors > 0 {
        parts.push(format!("{} errors", report.errors));
    }
    if parts.is_empty() {
        "No supported media files found. Check the folder and its file formats.".to_owned()
    } else {
        parts.join(", ")
    }
}

fn set_status(status: &StatusMap, library_id: &str, value: ScanStatus) {
    status
        .lock()
        .unwrap()
        .insert(library_id.to_owned(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_summary_is_readable() {
        let report = ScanReport {
            library_id: "l".into(),
            added: 3,
            updated: 2,
            removed: 1,
            skipped: 0,
            errors: 0,
        };
        assert_eq!(describe(&report), "3 added, 2 updated, 1 removed");

        let empty = ScanReport { library_id: "l".into(), ..Default::default() };
        assert_eq!(
            describe(&empty),
            "No supported media files found. Check the folder and its file formats."
        );

        let with_errors = ScanReport { errors: 4, ..Default::default() };
        assert_eq!(describe(&with_errors), "4 errors");
    }

    #[test]
    fn status_serializes_for_the_dashboard() {
        let status = ScanStatus {
            state: "Scanning".into(),
            progress: Some(0.5),
            processed: 10,
            detail: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "Scanning");
        assert_eq!(json["progress"], 0.5);
        assert_eq!(json["processed"], 10);
    }
}