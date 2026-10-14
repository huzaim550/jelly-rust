//! Plugin host: supervises the Node.js plugin runtime.
//!
//! Plugins run in a separate Node process and communicate over newline-delimited
//! JSON on stdio. That is a deliberate choice over an in-process engine:
//!
//! * **npm works.** Plugin authors can use any package, which is the difference
//!   between a plugin system people write plugins for and one they don't.
//! * **A plugin cannot take down the server.** A runaway loop or a stack
//!   overflow kills the child process, which is restarted; with an embedded
//!   engine it would hang or crash the server itself.
//! * **Async is natural.** Network calls in a plugin are ordinary promises.
//!
//! The cost is a per-call serialization hop and a Node dependency, both of which
//! are invisible next to the benefits above. See `pluginhost/` for the other
//! side of the protocol.

use crate::api::{
    API_VERSION, Capability, ItemSummary, LogLevel, LogRecord, Manifest, MetadataQuery,
    MetadataResult, PluginError, PluginEvent,
};
use jf_db::Database;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

/// A message from the server to the plugin host.
#[derive(Debug, Serialize)]
// `rename_all_fields` is the important half: without it serde renames only
// the variant names, and every field keeps its snake_case spelling.
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum HostRequest {
    /// Discover and load every plugin in the directory.
    Load { plugins_dir: String },
    /// Ask a plugin to resolve metadata.
    FetchMetadata { plugin: String, query: MetadataQuery, timeout_ms: u64 },
    /// Deliver an event to every subscriber.
    Event { event: PluginEvent },
    /// Run a plugin's teardown and exit.
    Shutdown,
}

/// A message from the plugin host to the server.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum HostResponse {
    /// A reply correlated by `id`.
    #[serde(rename = "result")]
    Result { id: u64, ok: bool, #[serde(default)] value: Option<serde_json::Value>, #[serde(default)] error: Option<String> },
    /// An unsolicited log line from a plugin.
    Log { record: LogRecord },
    /// The host process exited unexpectedly.
    Exited { code: Option<i32> },
}

/// Write one line to the host, serializing writers.
async fn write_line(
    writer: &Arc<Mutex<Option<ChildStdin>>>,
    lock: &Arc<Mutex<()>>,
    line: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let _guard = lock.lock().await;
    let mut guard = writer.lock().await;
    let Some(stdin) = guard.as_mut() else { return Ok(()) };

    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

/// Serialize a query result, dropping it on failure rather than failing the reply.
fn json_of<T: Serialize>(value: Result<Option<T>, PluginError>) -> Option<serde_json::Value> {
    value.ok().flatten().and_then(|v| serde_json::to_value(v).ok())
}

/// Read a plugin's namespaced setting. Used by the reader task.
async fn config_get(db: &Database, plugin: &str, key: &str) -> Result<Option<String>, PluginError> {
    Ok(sqlx::query_scalar(
        "SELECT value FROM plugin_config WHERE plugin_id = ? AND key = ?",
    )
    .bind(plugin)
    .bind(key)
    .fetch_optional(db.pool())
    .await?)
}

/// Write a plugin's namespaced setting.
async fn config_set(db: &Database, plugin: &str, key: &str, value: &str) -> Result<(), PluginError> {
    sqlx::query(
        "INSERT INTO plugin_config (plugin_id, key, value) VALUES (?, ?, ?)
         ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
    )
    .bind(plugin)
    .bind(key)
    .bind(value)
    .execute(db.pool())
    .await?;
    Ok(())
}

/// All settings for a plugin.
async fn config_all(
    db: &Database,
    plugin: &str,
) -> Result<HashMap<String, String>, PluginError> {
    use sqlx::Row;

    let rows = sqlx::query("SELECT key, value FROM plugin_config WHERE plugin_id = ?")
        .bind(plugin)
        .fetch_all(db.pool())
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("key"), r.get::<String, _>("value")))
        .collect())
}

/// Fetch an item summary. Used by the reader task.
async fn get_item(db: &Database, item_id: &str) -> Result<Option<ItemSummary>, PluginError> {
    use sqlx::Row;

    let row = sqlx::query("SELECT id, name, kind, production_year, path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(db.pool())
        .await?;

    Ok(row.map(|r| ItemSummary {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        year: r.get("production_year"),
        path: r.get("path"),
    }))
}

/// A request a plugin made of the supervisor, from inside a hook.
///
/// These arrive on the same channel as the supervisor's own requests, so the
/// reader must recognise them or every `config.get` in every plugin logs a
/// parse error.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum OtherRequest {
    GetItem { id: u64, item_id: String },
    ConfigGet { id: u64, plugin_id: String, key: String },
    ConfigSet { id: u64, plugin_id: String, key: String, value: String },
    ConfigAll { id: u64, plugin_id: String },
}

/// What a load pass discovered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadReport {
    pub loaded: Vec<LoadedPluginInfo>,
    pub failed: Vec<FailedPlugin>,
    pub api_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub metadata_providers: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedPlugin {
    pub id: String,
    pub reason: String,
}

/// A pending request awaiting its reply.
type Pending = oneshot::Sender<Result<serde_json::Value, String>>;

/// Supervises the Node plugin host process.
pub struct PluginHost {
    /// Stdin of the child, used to send requests.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// In-flight requests, keyed by correlation id.
    pending: Arc<Mutex<HashMap<u64, Pending>>>,
    /// Next correlation id.
    next_id: Arc<Mutex<u64>>,
    /// The most recent load report, cached so `/Plugins` needs no round trip.
    report: Arc<Mutex<LoadReport>>,
    /// Buffered plugin log lines.
    logs: Arc<Mutex<Vec<LogRecord>>>,
    /// Serializes requests so ids are issued atomically with their writes.
    send_lock: Arc<Mutex<()>>,
    /// Held so shutdown can await the reader task.
    child: Mutex<Option<Child>>,
    db: Database,
    plugins_dir: PathBuf,
}

/// Default ceiling for a single hook call.
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

impl PluginHost {
    /// Start the Node host and load every plugin under `plugins_dir`.
    pub async fn start(
        plugins_dir: impl Into<PathBuf>,
        db: Database,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, PluginError> {
        let plugins_dir = plugins_dir.into();
        // Read the directory before moving it into the struct, so the load
        // pass below can use the same value.
        let plugins_root = plugins_dir.clone();
        let host = Self {
            stdin: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            report: Arc::new(Mutex::new(LoadReport::default())),
            logs: Arc::new(Mutex::new(Vec::new())),
            send_lock: Arc::new(Mutex::new(())),
            child: Mutex::new(None),
            db,
            plugins_dir,
        };

        // No plugins directory is not an error; a server with no plugins is
        // perfectly valid.
        if plugins_root.is_dir() {
            host.spawn_host(&runtime).await?;
            let report = host.load().await?;
            if !report.loaded.is_empty() || !report.failed.is_empty() {
                tracing::info!(
                    loaded = report.loaded.len(),
                    failed = report.failed.len(),
                    "javascript plugins loaded"
                );
            }
        } else {
            tracing::info!(dir = %plugins_root.display(), "no plugins directory");
        }

        Ok(host)
    }

    /// Spawn the Node process and wire up its stdio.
    async fn spawn_host(&self, runtime: &tokio::runtime::Handle) -> Result<(), PluginError> {
        let entry = plugin_host_entry().ok_or_else(|| {
            PluginError::Host(
                "could not locate pluginhost/dist/index.js; run `npm install && npm run build` in pluginhost/"
                    .into(),
            )
        })?;

        // Make the plugin host's own node_modules reachable by plugins that were
        // copied elsewhere. Their relative `@jellyfin-clone/pluginhost` link
        // only resolves from their original location, which breaks any install
        // into a different directory. NODE_PATH is one of the roots Node
        // consults before walking up from the importing file, so adding the
        // host's package directory makes the SDK importable everywhere.
        let mut env: Vec<(String, std::ffi::OsString)> = Vec::new();
        if let Some(host_root) = entry.parent().and_then(Path::parent) {
            let mut search = host_root.to_string_lossy().into_owned();
            if let Some(existing) = std::env::var_os("NODE_PATH") {
                search.push(':');
                search.push_str(&existing.to_string_lossy());
            }
            env.push(("NODE_PATH".to_owned(), search.into()));
        }

        let mut cmd = Command::new("node");
        cmd.arg(&entry).current_dir(
            entry
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new(".")),
        );
        for (key, value) in &env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                PluginError::Host(format!(
                    "could not start `node {}`: {e}. Is Node.js installed?",
                    entry.display()
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            PluginError::Host("plugin host stdin unavailable".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PluginError::Host("plugin host stdout unavailable".into())
        })?;

        *self.stdin.lock().await = Some(stdin);
        *self.child.lock().await = Some(child);

        // Reader task: dispatches every line the host emits.
        let pending = Arc::clone(&self.pending);
        let logs = Arc::clone(&self.logs);
        let report = Arc::clone(&self.report);
        let db = self.db.clone();
        let writer = Arc::clone(&self.stdin);
        let send_lock = Arc::clone(&self.send_lock);

        runtime.spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Two shapes share the channel: replies and logs from the
                // host, and requests the host answers on a plugin's behalf.
                // The latter must be tried first: a `configGet` is not a
                // HostResponse, and parsing it as one would drop the reply.
                if let Ok(other) = serde_json::from_str::<OtherRequest>(trimmed) {
                    let (id, value) = match other {
                        OtherRequest::GetItem { id, item_id } => {
                            (id, json_of(get_item(&db, &item_id).await))
                        }
                        OtherRequest::ConfigGet { id, plugin_id, key } => {
                            (id, json_of(config_get(&db, &plugin_id, &key).await))
                        }
                        OtherRequest::ConfigSet { id, plugin_id, key, value: v } => {
                            let ok = config_set(&db, &plugin_id, &key, &v).await.is_ok();
                            (id, Some(json!(ok)))
                        }
                        OtherRequest::ConfigAll { id, plugin_id } => {
                            let all = config_all(&db, &plugin_id).await.ok();
                            (id, all.and_then(|v| serde_json::to_value(v).ok()))
                        }
                    };

                    let line = json!({"type": "result", "id": id, "ok": true, "value": value});
                    if write_line(&writer, &send_lock, &line.to_string()).await.is_err() {
                        tracing::warn!(id, "could not answer plugin request");
                    }
                    continue;
                }

                let response: HostResponse = match serde_json::from_str(trimmed) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(error = %e, line = trimmed, "unparseable host message");
                        continue;
                    }
                };

                match response {
                    HostResponse::Result { id, ok, value, error } => {
                        if let Some(sender) = pending.lock().await.remove(&id) {
                            let _ = sender.send(if ok {
                                Ok(value.unwrap_or(serde_json::Value::Null))
                            } else {
                                Err(error.unwrap_or_else(|| "unknown plugin error".into()))
                            });
                        }
                    }
                    HostResponse::Log { record } => {
                        tracing::debug!(
                            plugin = %record.plugin_id,
                            level = ?record.level,
                            message = %record.message,
                            "plugin log"
                        );
                        let mut buffer = logs.lock().await;
                        // Bound the buffer so a chatty plugin cannot exhaust
                        // memory.
                        if buffer.len() >= 10_000 {
                            buffer.drain(..1_000);
                        }
                        buffer.push(record);
                    }
                    HostResponse::Exited { code } => {
                        tracing::error!(?code, "plugin host exited");
                        // Fail anything still waiting rather than hanging.
                        let mut waiting = pending.lock().await;
                        for (_, sender) in waiting.drain() {
                            let _ = sender
                                .send(Err("plugin host exited unexpectedly".into()));
                        }
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Discover and load plugins.
    pub async fn load(&self) -> Result<LoadReport, PluginError> {
        // The Node child runs with its working directory set to `pluginhost/`,
        // so a relative path here would resolve against the wrong root.
        let absolute = self
            .plugins_dir
            .canonicalize()
            .unwrap_or_else(|_| self.plugins_dir.clone());

        let value = self
            .request(
                HostRequest::Load {
                    plugins_dir: absolute.to_string_lossy().into_owned(),
                },
                None,
            )
            .await?;

        let report: LoadReport =
            serde_json::from_value(value).map_err(|e| PluginError::Host(e.to_string()))?;

        // `start` logs the summary; this only records the report for /Plugins.
        *self.report.lock().await = report.clone();

        Ok(report)
    }

    /// Ask a plugin to resolve metadata for an item.
    ///
    /// Returns `Ok(None)` when the provider declines, which is normal: it means
    /// "I have nothing", and the server should try the next provider.
    pub async fn fetch_metadata(
        &self,
        plugin: &str,
        query: &MetadataQuery,
        timeout_ms: u64,
    ) -> Result<Option<MetadataResult>, PluginError> {
        let value = self
            .request(
                HostRequest::FetchMetadata {
                    plugin: plugin.to_owned(),
                    query: query.clone(),
                    timeout_ms,
                },
                Some(timeout_ms),
            )
            .await?;

        if value.is_null() {
            return Ok(None);
        }

        let result: MetadataResult =
            serde_json::from_value(value).map_err(|e| PluginError::Runtime(plugin.to_owned(), e.to_string()))?;
        Ok(Some(result).filter(|r| !r.is_empty()))
    }

    /// Deliver an event to every plugin that subscribes.
    ///
    /// Fire and forget: an event handler must never delay the request that
    /// produced it.
    pub async fn dispatch_event(&self, event: PluginEvent) {
        if let Err(e) = self
            .write(&HostRequest::Event { event })
            .await
        {
            tracing::warn!(error = %e, "could not deliver plugin event");
        }
    }

    /// The most recent load report.
    pub async fn report(&self) -> LoadReport {
        self.report.lock().await.clone()
    }

    /// Ids of loaded plugins.
    pub async fn plugin_ids(&self) -> Vec<String> {
        self.report.lock().await.loaded.iter().map(|p| p.id.clone()).collect()
    }

    pub async fn plugin_count(&self) -> usize {
        self.report.lock().await.loaded.len()
    }

    pub fn api_version(&self) -> &'static str {
        API_VERSION
    }

    /// Drain buffered plugin log records.
    pub async fn drain_logs(&self) -> Vec<LogRecord> {
        let mut buffer = self.logs.lock().await;
        std::mem::take(&mut *buffer)
    }

    /// Read a plugin's namespaced setting.
    pub async fn plugin_config_get(&self, plugin: &str, key: &str) -> Result<Option<String>, PluginError> {
        Ok(sqlx::query_scalar(
            "SELECT value FROM plugin_config WHERE plugin_id = ? AND key = ?",
        )
        .bind(plugin)
        .bind(key)
        .fetch_optional(self.db.pool())
        .await?)
    }

    /// Write a plugin's namespaced setting.
    pub async fn plugin_config_set(
        &self,
        plugin: &str,
        key: &str,
        value: &str,
    ) -> Result<(), PluginError> {
        sqlx::query(
            "INSERT INTO plugin_config (plugin_id, key, value) VALUES (?, ?, ?)
             ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
        )
        .bind(plugin)
        .bind(key)
        .bind(value)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// All settings for a plugin.
    pub async fn plugin_config_all(
        &self,
        plugin: &str,
    ) -> Result<HashMap<String, String>, PluginError> {
        use sqlx::Row;

        let rows = sqlx::query("SELECT key, value FROM plugin_config WHERE plugin_id = ?")
            .bind(plugin)
            .fetch_all(self.db.pool())
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>("key"),
                    r.get::<String, _>("value"),
                )
            })
            .collect())
    }

    /// Fetch an item summary for a plugin.
    pub async fn get_item(&self, item_id: &str) -> Result<Option<ItemSummary>, PluginError> {
        use sqlx::Row;

        let row = sqlx::query("SELECT id, name, kind, production_year, path FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(self.db.pool())
            .await?;

        Ok(row.map(|r| ItemSummary {
            id: r.get("id"),
            name: r.get("name"),
            kind: r.get("kind"),
            year: r.get("production_year"),
            path: r.get("path"),
        }))
    }

    /// Send a request and await its reply.
    async fn request(
        &self,
        request: HostRequest,
        timeout_ms: Option<u64>,
    ) -> Result<serde_json::Value, PluginError> {
        // A request with no id (fire-and-forget) still needs to be written.
        let Some(id) = self.next_id().await else {
            return Err(PluginError::Host("plugin host is not running".into()));
        };

        let mut envelope = serde_json::to_value(&request)
            .map_err(|e| PluginError::Host(format!("could not encode request: {e}")))?;
        if let Some(map) = envelope.as_object_mut() {
            map.insert("id".into(), serde_json::json!(id));
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write_raw(&serde_json::to_string(&envelope).unwrap_or_default())
            .await?;

        let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(PluginError::Host(message)),
            // The reader task dropped the sender: the host is gone.
            Ok(Err(_)) => Err(PluginError::Host(
                "plugin host closed the connection".into(),
            )),
            // A hung plugin. The reply may still arrive later; the id stays in
            // `pending` until then, and is dropped when it does.
            Err(_) => Err(PluginError::Host(format!(
                "plugin did not respond within {}ms",
                timeout.as_millis()
            ))),
        }
    }

    /// Answer a plugin-initiated request.
    ///
    /// Does not create a pending entry: the caller is the plugin, waiting on
    /// the other side of the pipe for exactly this line.
    async fn respond<T: Serialize>(&self, id: u64, ok: bool, value: Option<T>) {
        let line = serde_json::json!({
            "type": "result",
            "id": id,
            "ok": ok,
            "value": value,
        });
        if let Err(e) = self.write_raw(&line.to_string()).await {
            tracing::warn!(error = %e, "could not answer plugin request");
        }
    }

    async fn write(&self, request: &HostRequest) -> Result<(), PluginError> {
        let line = serde_json::to_string(request)
            .map_err(|e| PluginError::Host(format!("could not encode request: {e}")))?;
        self.write_raw(&line).await
    }

    async fn write_raw(&self, line: &str) -> Result<(), PluginError> {
        // One writer at a time keeps interleaved partial lines off the pipe.
        let _guard = self.send_lock.lock().await;
        let mut guard = self.stdin.lock().await;

        let Some(stdin) = guard.as_mut() else {
            return Err(PluginError::Host("plugin host is not running".into()));
        };

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| PluginError::Host(format!("could not send to plugin host: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| PluginError::Host(format!("could not send to plugin host: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| PluginError::Host(format!("could not flush to plugin host: {e}")))?;

        Ok(())
    }

    async fn next_id(&self) -> Option<u64> {
        let mut guard = self.next_id.lock().await;
        let id = *guard;
        *guard += 1;
        Some(id)
    }

    /// Shut the host down, giving plugins a chance to clean up.
    pub async fn shutdown(&self) {
        // Best effort: a failure here means the host already died.
        let _ = self.write(&HostRequest::Shutdown).await;

        if let Some(mut child) = self.child.lock().await.take() {
            // Give it a moment to exit cleanly before killing it.
            let deadline = std::time::Duration::from_millis(500);
            match tokio::time::timeout(deadline, child.wait()).await {
                Ok(Ok(_)) => {}
                _ => {
                    let _ = child.kill().await;
                }
            }
        }

        *self.stdin.lock().await = None;
    }
}

/// Locate the compiled Node plugin host.
///
/// Looks beside the executable first so a packaged build works, then falls back
/// to the repository layout for development.
fn plugin_host_entry() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("JF_PLUGIN_HOST") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        // target/debug/jellyfin-clone -> repo root
        for up in [3, 4] {
            if let Some(root) = exe.ancestors().nth(up) {
                candidates.push(root.join("pluginhost/dist/index.js"));
            }
        }
    }

    candidates.push(PathBuf::from("pluginhost/dist/index.js"));
    candidates.into_iter().find(|p| p.is_file())
}

/// Read a plugin's manifest from disk.
///
/// Exposed so tooling can validate a plugin without starting the host.
pub fn read_manifest(plugin_dir: &Path) -> Result<Manifest, PluginError> {
    let path = plugin_dir.join("plugin.json");
    let source = std::fs::read_to_string(&path).map_err(PluginError::Io)?;
    Manifest::parse(&source)
}

/// Map a log level to its string form, for display.
pub fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn host() -> PluginHost {
        let db = Database::connect_memory().await.expect("db");
        PluginHost {
            stdin: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            report: Arc::new(Mutex::new(LoadReport::default())),
            logs: Arc::new(Mutex::new(Vec::new())),
            send_lock: Arc::new(Mutex::new(())),
            child: Mutex::new(None),
            db,
            plugins_dir: PathBuf::from("/nonexistent-plugins"),
        }
    }

    #[tokio::test]
    async fn starting_without_a_plugins_directory_succeeds() {
        let db = Database::connect_memory().await.expect("db");
        let host = PluginHost::start(
            std::env::temp_dir().join("jf-definitely-missing-plugins"),
            db,
            tokio::runtime::Handle::current(),
        )
        .await
        .expect("a missing plugins directory must not be an error");

        assert_eq!(host.plugin_count().await, 0);
        assert_eq!(host.api_version(), "1.0");
    }

    #[tokio::test]
    async fn requests_without_a_running_host_report_clearly() {
        let h = host().await;
        let query = MetadataQuery {
            title: "Anything".into(),
            year: None,
            kind: jf_core::BaseItemKind::Movie,
            known_ids: HashMap::new(),
            preferred_languages: vec![],
        };

        let err = h.fetch_metadata("tmdb", &query, 100).await.unwrap_err();
        match err {
            PluginError::Host(message) => assert!(
                message.contains("not running"),
                "expected a clear message, got: {message}"
            ),
            other => panic!("expected Host error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn manifest_is_read_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"id":"x","name":"X","version":"1","apiVersion":"1.0","entry":"index.js"}"#,
        )
        .unwrap();

        let manifest = read_manifest(dir.path()).expect("manifest parses");
        assert_eq!(manifest.id, "x");
    }

    /// The wire format is shared with a TypeScript implementation, and a field
    /// renamed on one side only fails *silently*: the plugin sees `undefined`
    /// and returns an empty result rather than erroring. These pin the exact
    /// JSON the Node side parses.
    #[test]
    fn request_fields_are_camel_case() {
        let request = HostRequest::Load {
            plugins_dir: "/srv/plugins".into(),
        };
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["type"], "load");
        assert_eq!(json["pluginsDir"], "/srv/plugins");
        assert!(
            json.get("plugins_dir").is_none(),
            "snake_case leaked into the wire format"
        );
    }

    #[test]
    fn fetch_metadata_request_is_camel_case() {
        let request = HostRequest::FetchMetadata {
            plugin: "tmdb".into(),
            query: MetadataQuery {
                title: "Dune".into(),
                year: Some(2021),
                kind: jf_core::BaseItemKind::Movie,
                known_ids: HashMap::from([("imdb".to_owned(), "tt1160419".to_owned())]),
                preferred_languages: vec!["en".into()],
            },
            timeout_ms: 5_000,
        };
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["type"], "fetchMetadata");
        assert_eq!(json["timeoutMs"], 5_000);
        // The nested query must be camelCase too, or the SDK sees undefined.
        assert_eq!(json["query"]["knownIds"]["imdb"], "tt1160419");
        assert_eq!(json["query"]["preferredLanguages"][0], "en");
    }

    #[test]
    fn metadata_result_is_camel_case_for_the_sdk() {
        let result = MetadataResult {
            title: Some("Dune".into()),
            original_title: Some("Dune".into()),
            run_time_ticks: Some(9_240_000_000),
            external_ids: HashMap::from([("tmdb".to_owned(), "438631".to_owned())]),
            ..Default::default()
        };
        let json = serde_json::to_value(&result).unwrap();

        assert_eq!(json["originalTitle"], "Dune");
        assert_eq!(json["runTimeTicks"], 9_240_000_000u64);
        assert_eq!(json["externalIds"]["tmdb"], "438631");
        assert!(json.get("original_title").is_none());
    }

    #[test]
    fn events_are_flat_to_match_the_sdk_union() {
        // The SDK reads `event.itemId`, not `event.payload.itemId`.
        let json = serde_json::to_value(PluginEvent::ItemDiscovered {
            item_id: "abc".into(),
            library_id: "lib".into(),
        })
        .unwrap();

        assert_eq!(json["type"], "itemDiscovered");
        assert_eq!(json["itemId"], "abc");
        assert_eq!(json["libraryId"], "lib");
        assert!(
            json.get("payload").is_none(),
            "events must be adjacently tagged, not nested under `payload`"
        );
    }

    /// The plugin host emits this exact line the first time a plugin reads a
    /// setting. If it fails to parse, every `config.get` in every plugin is
    /// silently dropped and the plugin falls back to no credentials.
    #[test]
    fn plugin_initiated_config_request_parses() {
        let line = r#"{"type":"configGet","id":1000001,"pluginId":"tmdb","key":"api_key"}"#;
        let parsed: OtherRequest = serde_json::from_str(line).expect("configGet must parse");

        match parsed {
            OtherRequest::ConfigGet { id, plugin_id, key } => {
                assert_eq!(id, 1_000_001);
                assert_eq!(plugin_id, "tmdb");
                assert_eq!(key, "api_key");
            }
            other => panic!("expected ConfigGet, got {other:?}"),
        }
    }

    #[test]
    fn every_plugin_request_shape_parses() {
        let cases = [
            r#"{"type":"getItem","id":5,"itemId":"abc"}"#,
            r#"{"type":"configGet","id":6,"pluginId":"p","key":"k"}"#,
            r#"{"type":"configSet","id":7,"pluginId":"p","key":"k","value":"v"}"#,
            r#"{"type":"configAll","id":8,"pluginId":"p"}"#,
        ];

        for line in cases {
            assert!(
                serde_json::from_str::<OtherRequest>(line).is_ok(),
                "failed to parse: {line}"
            );
        }
    }

    #[test]
    fn log_levels_map_to_names() {
        assert_eq!(level_name(LogLevel::Warn), "warn");
        assert_eq!(level_name(LogLevel::Error), "error");
    }
}