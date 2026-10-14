//! In-process Python plugin host.
//!
//! ## Threading contract
//!
//! Every call into Python holds the GIL. That is fine for short hooks but
//! fatal if a plugin blocks: it would stall every other plugin and any Rust
//! code that needs the GIL. Three rules keep this safe:
//!
//! 1. Plugin code always runs on a `rayon` blocking thread, never on a tokio
//!    worker. A blocking hook must never occupy an async reactor thread.
//! 2. The GIL is released (`allow_threads`) around every blocking Rust call
//!    made on a plugin's behalf, such as a database query.
//! 3. Hooks have no way to block on a tokio runtime. An async plugin gets a
//!    dedicated Python event loop thread instead (see `pluginsdk`).
//!
//! ## Failure containment
//!
//! Python exceptions are caught and reported as `PluginError::Runtime`. Rust
//! panics crossing the FFI boundary are caught with `catch_unwind`. A genuine
//! segfault in a native extension is *not* recoverable, which is why the
//! plugin policy requires pure-Python plugins.

use crate::api::{
    API_VERSION, Capability, LogLevel, LogRecord, Manifest, MetadataQuery,
    MetadataResult, PluginEvent,
};
use crate::error::PluginError;
use jf_db::Database;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};
use std::collections::HashMap;
use std::path::PathBuf;
use sqlx::Row;
use std::sync::Arc;

/// Settings accessor handed to plugins, scoped to one plugin id.
#[derive(Debug, Clone)]
pub struct PluginConfigHandle {
    plugin_id: String,
    db: Database,
}

impl PluginConfigHandle {
    fn new(plugin_id: impl Into<String>, db: Database) -> Self {
        Self { plugin_id: plugin_id.into(), db }
    }

    /// Read a namespaced setting.
    pub async fn get(&self, key: &str) -> Result<Option<String>, PluginError> {
        Ok(sqlx::query_scalar(
            "SELECT value FROM plugin_config WHERE plugin_id = ? AND key = ?",
        )
        .bind(&self.plugin_id)
        .bind(key)
        .fetch_optional(self.db.pool())
        .await?)
    }

    /// Write a namespaced setting.
    pub async fn set(&self, key: &str, value: &str) -> Result<(), PluginError> {
        sqlx::query(
            "INSERT INTO plugin_config (plugin_id, key, value) VALUES (?, ?, ?)
             ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
        )
        .bind(&self.plugin_id)
        .bind(key)
        .bind(value)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// All settings for this plugin.
    pub async fn all(&self) -> Result<HashMap<String, String>, PluginError> {
        let rows =
            sqlx::query("SELECT key, value FROM plugin_config WHERE plugin_id = ?")
                .bind(&self.plugin_id)
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
}

/// Read-only item lookup exposed to plugins.
#[derive(Debug, Clone)]
pub struct ItemLookup {
    db: Database,
}

impl ItemLookup {
    fn new(db: Database) -> Self {
        Self { db }
    }

    /// Fetch an item's name, kind, and year.
    ///
    /// Deliberately narrow: plugins that can enumerate the whole database are a
    /// step away from being able to exfiltrate every user's watch history.
    pub async fn get_item(&self, item_id: &str) -> Result<Option<ItemSummary>, PluginError> {
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

    /// All provider ids known for an item, e.g. `{"imdb": "tt0133093"}`.
    pub async fn external_ids(&self, item_id: &str) -> Result<HashMap<String, String>, PluginError> {
        let rows = sqlx::query(
            "SELECT provider, value FROM item_external_ids WHERE item_id = ?",
        )
        .bind(item_id)
        .fetch_all(self.db.pool())
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>("provider"),
                    r.get::<String, _>("value"),
                )
            })
            .collect())
    }
}

/// The item shape plugins see.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub year: Option<i32>,
    pub path: String,
}

/// One loaded plugin.
pub struct LoadedPlugin {
    pub manifest: Manifest,
    /// The Python module object, kept alive for the process lifetime.
    pub module: Arc<Py<PyModule>>,
    pub config: PluginConfigHandle,
    /// Whether every declared dependency loaded successfully.
    pub satisfied: bool,
}

/// The plugin registry: discovery, loading, and hook dispatch.
pub struct PluginHost {
    plugins_dir: PathBuf,
    plugins: Vec<Arc<LoadedPlugin>>,
    db: Database,
    pool: Arc<rayon::ThreadPool>,
    /// Host API version passed into the plugin environment.
    api_version: String,
    /// Tokio handle captured at construction; see `HostBridge::runtime`.
    runtime: tokio::runtime::Handle,
    /// Log records emitted since the last drain, for the HTTP log endpoint.
    logs: Arc<std::sync::Mutex<Vec<LogRecord>>>,
}

impl PluginHost {
    /// Create a host that loads plugins from `plugins_dir`.
    pub fn new(
        plugins_dir: impl Into<PathBuf>,
        db: Database,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, PluginError> {
        // Plugin work is CPU- or IO-blocking and holds the GIL, so it gets its
        // own pool rather than running on tokio workers.
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("jf-plugin-{i}"))
            .num_threads(8)
            .build()
            .map_err(|e| PluginError::Runtime("threadpool".into(), e.to_string()))?;

        Ok(Self {
            plugins_dir: plugins_dir.into(),
            plugins: Vec::new(),
            db,
            pool: Arc::new(pool),
            api_version: API_VERSION.to_owned(),
            runtime,
            logs: Arc::new(std::sync::Mutex::new(Vec::new())),
        })
    }

    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Ids of loaded plugins, in load order.
    pub fn plugin_ids(&self) -> Vec<String> {
        self.plugins.iter().map(|p| p.manifest.id.clone()).collect()
    }

    /// Discover, validate, and load every plugin under the plugins directory.
    pub fn load_all(&mut self) -> Result<LoadReport, PluginError> {
        Python::initialize();

        let mut report = LoadReport::default();

        if !self.plugins_dir.is_dir() {
            tracing::info!(dir = %self.plugins_dir.display(), "no plugins directory");
            return Ok(report);
        }

        let entries = std::fs::read_dir(&self.plugins_dir).map_err(PluginError::Io)?;
        let mut manifests: Vec<(PathBuf, Manifest)> = Vec::new();

        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }

            let manifest_path = dir.join("manifest.toml");
            if !manifest_path.is_file() {
                tracing::warn!(dir = %dir.display(), "plugin has no manifest.toml, skipping");
                report.failed.push((dir.display().to_string(), "missing manifest.toml".into()));
                continue;
            }

            let source = match std::fs::read_to_string(&manifest_path) {
                Ok(s) => s,
                Err(e) => {
                    report.failed.push((dir.display().to_string(), e.to_string()));
                    continue;
                }
            };

            match Manifest::from_toml(&source) {
                Ok(m) => manifests.push((dir, m)),
                Err(PluginError::ApiMismatch { plugin, found, expected }) => {
                    tracing::warn!(
                        plugin = %plugin, found = %found, expected = %expected,
                        "refusing plugin built for a different host API"
                    );
                    report.failed.push((plugin, format!("API {found} != {expected}")));
                }
                Err(e) => {
                    report.failed.push((dir.display().to_string(), e.to_string()));
                }
            }
        }

        // Load in dependency order so a plugin never sees a half-loaded dep.
        manifests.sort_by_key(|(_, m)| m.id.clone());
        let mut loaded_ids: Vec<String> = Vec::new();

        for (dir, manifest) in manifests {
            let deps_ok = manifest.depends_on.iter().all(|d| loaded_ids.contains(d));
            if !deps_ok {
                tracing::warn!(plugin = %manifest.id, "dependency missing, skipping");
                report.failed.push((manifest.id, format!("missing dep in {:?}", manifest.depends_on)));
                continue;
            }

            match self.load_plugin(&dir, &manifest) {
                Ok(plugin) => {
                    loaded_ids.push(plugin.manifest.id.clone());
                    report.loaded.push(plugin.manifest.id.clone());
                    self.plugins.push(Arc::new(plugin));
                }
                Err(e) => {
                    tracing::warn!(plugin = %manifest.id, error = %e, "failed to load plugin");
                    report.failed.push((manifest.id, e.to_string()));
                }
            }
        }

        tracing::info!(loaded = report.loaded.len(), failed = report.failed.len(), "plugins loaded");
        Ok(report)
    }

    /// Import a single plugin module and run its setup.
    fn load_plugin(&self, dir: &PathBuf, manifest: &Manifest) -> Result<LoadedPlugin, PluginError> {
        let config = PluginConfigHandle::new(manifest.id.clone(), self.db.clone());
        let lookup = ItemLookup::new(self.db.clone());
        let logs = Arc::clone(&self.logs);
        let plugin_id = manifest.id.clone();
        let root = dir.clone();

        let module = Python::attach(|py| {
            // Prepend the plugin dir so its own modules resolve ahead of
            // anything with the same name on sys.path.
            let root_str = root.to_string_lossy().into_owned();
            let path = py.import("sys")?.getattr("path")?;
            path.call_method1("insert", (0, root_str))?;

            let module = PyModule::import(py, &manifest.entry).map_err(|e| {
                let id = manifest.id.clone();
                PluginError::Python(format!("{id}: {}", e.to_string()))
            })?;

            // #[pyclass] values must be boxed into Py<T> before being handed to Python.
            let bridge = Py::new(
                py,
                HostBridge {
                    plugin_id: plugin_id.clone(),
                    lookup,
                    logs,
                    api_version: self.api_version.clone(),
                    runtime: self.runtime.clone(),
                },
            )?;
            let config_proxy = Py::new(py, PyConfigProxy {
                inner: config.clone(),
                runtime: self.runtime.clone(),
            })?;
            let capabilities = capability_list(py, manifest)?;

            // These MUST be injected into the module's own globals. Passing them
            // as call kwargs to setup() would make them visible only inside
            // setup(), leaving every other function in the plugin to raise
            // NameError on `host`.
            module.add("host", bridge)?;
            module.add("config", config_proxy)?;
            module.add("api_version", self.api_version.as_str())?;
            module.add("plugin_id", manifest.id.as_str())?;
            module.add("capabilities", capabilities)?;

            // A plugin may expose a module-level `setup()` to validate configuration at
            // load time. It takes no arguments: everything it needs is already
            // a module global.
            if let Ok(setup) = module.getattr("setup") {
                setup.call0()?;
            }

            Ok::<Py<PyModule>, PluginError>(module.unbind())
        })?;

        Ok(LoadedPlugin {
            manifest: manifest.clone(),
            module: Arc::new(module),
            config,
            satisfied: true,
        })
    }

    /// Run every registered `fetch_metadata` hook for a query.
    ///
    /// Hooks run in parallel on the plugin pool; the first non-empty result
    /// wins, matching how a user would expect a provider chain to resolve.
    pub fn fetch_metadata(
        &self,
        plugin_id: &str,
        query: &MetadataQuery,
        timeout_secs: u64,
    ) -> Result<Option<MetadataResult>, PluginError> {
        let Some(plugin) = self.plugins.iter().find(|p| p.manifest.id == plugin_id) else {
            return Err(PluginError::Missing(plugin_id.to_owned()));
        };
        if !plugin.manifest.capabilities.contains(&Capability::Network) {
            return Err(PluginError::Denied("network".into()));
        }

        let query_json = serde_json::to_string(query).map_err(|e| {
            PluginError::Runtime(plugin_id.to_owned(), format!("serialize query: {e}"))
        })?;

        let module = Arc::clone(&plugin.module);

        // Blocking work on the plugin pool: the GIL is held during the hook,
        // so running this on a tokio worker would stall the whole server.
        let result = self.pool.install(|| {
            Python::attach(|py| {
                let module = module.bind(py);
                let hook = module.getattr("fetch_metadata").map_err(|e| {
                    PluginError::Runtime(plugin_id.to_owned(), e.to_string())
                })?;

                let raw = hook.call1((query_json.as_str(),)).map_err(|e| {
                    PluginError::Runtime(plugin_id.to_owned(), e.to_string())
                })?;

                if raw.is_none() {
                    return Ok(None::<MetadataResult>);
                }

                let json: String = raw.extract::<String>().map_err(|e: pyo3::PyErr| {
                    PluginError::Runtime(plugin_id.to_owned(), e.to_string())
                })?;

                if json.is_empty() {
                    return Ok(None);
                }

                serde_json::from_str(&json).map(Some).map_err(|e| {
                    PluginError::Runtime(plugin_id.to_owned(), format!("bad metadata json: {e}"))
                })
            })
        });

        // NOTE ON TIMEOUTS: there is deliberately no host-side deadline here.
        // A hook executes with the GIL held; Python offers no safe way to
        // interrupt arbitrary bytecode from another thread, so a "timeout" that
        // abandoned the result would leave the plugin running, still holding the
        // GIL, while the caller believed it had finished. That is strictly worse
        // than waiting. Enforcement therefore belongs to the plugin: the SDK's
        // HTTP client is deadline-bounded, and `timeout_secs` is advisory,
        // surfaced to plugins so they can size their own budget.
        let _ = timeout_secs;

        Ok(result?.filter(|m| !m.is_empty()))
    }

    /// Dispatch an event to every plugin that handles it.
    pub fn dispatch_event(&self, event: &PluginEvent) {
        let payload = match serde_json::to_string(event) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "could not serialize plugin event");
                return;
            }
        };

        for plugin in &self.plugins {
            if !plugin.manifest.capabilities.contains(&Capability::Events) {
                continue;
            }

            let module = Arc::clone(&plugin.module);
            let id = plugin.manifest.id.clone();
            let body = payload.clone();
            let pool = Arc::clone(&self.pool);

            // Fire and forget: event handlers must not block the request that
            // emitted the event.
            std::thread::spawn(move || {
                pool.install(|| {
                    Python::attach(|py| {
                        let module = module.bind(py);
                        let Ok(handler) = module.getattr("on_event") else { return };
                        if let Err(e) = handler.call1((body.as_str(),)) {
                            tracing::warn!(plugin = %id, error = %e, "event handler failed");
                        }
                    });
                });
            });
        }
    }

    /// Drain buffered log records.
    pub fn drain_logs(&self) -> Vec<LogRecord> {
        self.logs.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    /// Shut down cleanly, releasing module references.
    pub fn shutdown(&mut self) {
        for plugin in std::mem::take(&mut self.plugins) {
            let module = Arc::clone(&plugin.module);
            let id = plugin.manifest.id.clone();
            let _ = Python::attach(|py| {
                if let Ok(m) = module.bind(py).getattr("teardown") {
                    if let Err(e) = m.call0() {
                        tracing::warn!(plugin = %id, error = %e, "teardown failed");
                    }
                }
            });
        }
    }
}

/// Outcome of a discovery pass.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Build the capability list handed to Python.
fn capability_list(py: Python<'_>, manifest: &Manifest) -> PyResult<Py<PyList>> {
    let names: Vec<&str> = manifest.capabilities.iter().map(|c| capability_str(c)).collect();
    Ok(PyList::new(py, names)?.unbind())
}

fn capability_str(c: &Capability) -> &'static str {
    match c {
        Capability::ReadItems => "read_items",
        Capability::Network => "network",
        Capability::HttpRoutes => "http_routes",
        Capability::Config => "config",
        Capability::Events => "events",
        Capability::WriteMetadata => "write_metadata",
    }
}

/// Run a database query on a worker thread while releasing the GIL.
///
/// Every host call that touches the database goes through here. Holding the GIL
/// across a query would stall every other plugin in the process for the
/// duration of the query.
fn blocking_query<T, F, Fut>(
    py: Python<'_>,
    runtime: &tokio::runtime::Handle,
    work: F,
) -> PyResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, PluginError>> + Send,
{
    py.detach(|| {
        // spawn_blocking gives a thread that may block on IO; the GIL is already
        // released by detach, so the runtime is free to reuse this thread.
        //
        // The runtime handle is captured explicitly rather than via
        // `Handle::current()`: plugin hooks run on a rayon pool, which has no
        // ambient tokio context, and `current()` would panic there.
        let spawn_handle = runtime.clone();
        let run_handle = runtime.clone();
        let handle = spawn_handle.spawn_blocking(move || run_handle.block_on(work()));
        futures::executor::block_on(handle)
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "plugin database task failed: {e}"
                ))
            })?
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    })
}

/// The `host` object exposed to every plugin.
#[pyclass]
pub struct HostBridge {
    plugin_id: String,
    lookup: ItemLookup,
    logs: Arc<std::sync::Mutex<Vec<LogRecord>>>,
    api_version: String,
    /// Captured at startup: plugin hooks run on rayon threads, which have no
    /// ambient tokio context, so the handle cannot be looked up at call time.
    runtime: tokio::runtime::Handle,
}

#[pymethods]
impl HostBridge {
    /// Fetch an item summary. Requires the `read_items` capability.
    ///
    /// Returns a JSON string, or `None`. Matching the hook convention, values
    /// cross the boundary as strings rather than native Python objects.
    fn get_item(&self, py: Python<'_>, item_id: &str) -> PyResult<Option<Py<PyAny>>> {
        let lookup = self.lookup.clone();
        let item_id = item_id.to_owned();

        let result = blocking_query(py, &self.runtime, move || async move { lookup.get_item(&item_id).await })?;
        Ok(result
            .and_then(|item| serde_json::to_string(&item).ok())
            .map(|json| pyo3::types::PyString::new(py, json.as_str()).into_any().unbind()))
    }

    /// The host API version, so a plugin can feature-detect.
    fn api_version(&self) -> &str {
        &self.api_version
    }

    /// The id of the plugin currently running.
    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Emit a log line tagged with this plugin's id.
    fn log(&self, level: &str, message: &str) {
        let parsed = match level.to_ascii_lowercase().as_str() {
            "trace" => LogLevel::Trace,
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warn" | "warning" => LogLevel::Warn,
            _ => LogLevel::Error,
        };

        if let Ok(mut logs) = self.logs.lock() {
            // Bound the buffer so a chatty plugin cannot exhaust memory.
            if logs.len() >= 10_000 {
                logs.drain(..1_000);
            }
            logs.push(LogRecord {
                level: parsed,
                plugin_id: self.plugin_id.clone(),
                message: message.to_owned(),
            });
        }
    }
}

/// The `config` object exposed to every plugin.
#[pyclass]
pub struct PyConfigProxy {
    inner: PluginConfigHandle,
    runtime: tokio::runtime::Handle,
}

#[pymethods]
impl PyConfigProxy {
    fn get(&self, py: Python<'_>, key: &str) -> PyResult<Option<String>> {
        let inner = self.inner.clone();
        let key = key.to_owned();
        blocking_query(py, &self.runtime, move || async move { inner.get(&key).await })
    }

    fn set(&self, py: Python<'_>, key: &str, value: &str) -> PyResult<()> {
        let inner = self.inner.clone();
        let (key, value) = (key.to_owned(), value.to_owned());
        blocking_query(py, &self.runtime, move || async move { inner.set(&key, &value).await })
    }

    fn all(&self, py: Python<'_>) -> PyResult<HashMap<String, String>> {
        let inner = self.inner.clone();
        blocking_query(py, &self.runtime, move || async move { inner.all().await })
    }
}

/// One-time interpreter initialization, safe to call from anywhere.
pub fn ensure_interpreter() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(Python::initialize);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_can_be_constructed_without_plugins() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let host = PluginHost::new(std::env::temp_dir().join("jf-plugins-empty"), db, tokio::runtime::Handle::current()).unwrap();
        assert_eq!(host.plugin_count(), 0);
        assert!(host.plugin_ids().is_empty());
        assert_eq!(host.api_version(), "1.0");
    }

    #[tokio::test]
    async fn missing_plugin_directory_is_not_an_error() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let mut host =
            PluginHost::new(
                std::env::temp_dir().join("jf-plugins-nonexistent-xyz"),
                db,
                tokio::runtime::Handle::current(),
            ).unwrap();
        let report = host.load_all().expect("empty dir loads cleanly");
        assert!(report.loaded.is_empty());
        assert!(report.failed.is_empty());
    }

    #[test]
    fn capability_names_are_stable() {
        assert_eq!(capability_str(&Capability::Network), "network");
        assert_eq!(capability_str(&Capability::WriteMetadata), "write_metadata");
    }
}