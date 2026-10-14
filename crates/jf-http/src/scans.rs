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
use jf_scan::ScanReport;
use serde::Serialize;
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
        let status = Arc::clone(&self.status);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            let _guard = running.lock().await;

            let libraries = match app.scanner.libraries().await {
                Ok(libs) => libs,
                Err(e) => {
                    tracing::error!(error = %e, "could not list libraries for scan");
                    return;
                }
            };

            if libraries.is_empty() {
                app.emit("scanComplete", serde_json::json!({ "libraries": 0 }));
                return;
            }

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
        "no changes".to_owned()
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
        assert_eq!(describe(&empty), "no changes");

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