//! Shared server state.

use jf_db::Database;
use jf_playback::{HlsSessionManager, ImageCache, Transcoder};
use jf_plugins::PluginHost;
use jf_scan::Scanner;
use std::path::PathBuf;
use std::sync::Arc;

use crate::scans::ScanManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAction {
    Shutdown,
    Restart,
}

/// Directory layout for a server's mutable data.
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
    pub database: PathBuf,
    pub plugins: PathBuf,
    pub cache: PathBuf,
    pub transcode: PathBuf,
    pub thumbnails: PathBuf,
}

impl DataPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();

        // An explicit directory takes precedence. In a source checkout, load
        // the built-in plugin from the repository so it is available without
        // copying it (and its linked node_modules) into the data directory.
        // Packaged deployments set JF_PLUGINS_DIR; otherwise plugins live in
        // the data directory as before.
        let checkout_plugins = PathBuf::from("plugins");
        let bundled_omdb = checkout_plugins.join("omdb/plugin.json").is_file()
            && checkout_plugins.join("omdb/dist/index.js").is_file();
        let plugins = std::env::var("JF_PLUGINS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                if bundled_omdb {
                    checkout_plugins
                } else {
                    root.join("plugins")
                }
            });

        Self {
            database: root.join("jf.db"),
            plugins,
            cache: root.join("cache"),
            transcode: root.join("cache").join("transcode"),
            thumbnails: root.join("cache").join("thumbnails"),
            root,
        }
    }

    /// Create every directory the server writes to.
    pub async fn ensure(&self) -> anyhow::Result<()> {
        for dir in [
            &self.root,
            &self.plugins,
            &self.cache,
            &self.transcode,
            &self.thumbnails,
        ] {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(())
    }
}

/// Everything a request handler needs.
pub struct AppState {
    pub db: Database,
    pub scanner: Arc<Scanner>,
    /// Owns the Node plugin process. It is internally synchronized, so it is
    /// shared directly rather than wrapped in another lock.
    pub plugins: Arc<PluginHost>,
    /// Background library scanning with per-library progress.
    pub scans: Arc<ScanManager>,
    pub transcoder: Arc<Transcoder>,
    pub hls: HlsSessionManager,
    pub images: ImageCache,
    pub paths: DataPaths,
    pub server_id: String,
    /// Mutable so the onboarding wizard can rename the server.
    pub server_name: tokio::sync::RwLock<String>,
    pub version: String,
    pub started_at: std::time::Instant,
    /// Broadcast channel for server-sent events.
    pub events: tokio::sync::broadcast::Sender<String>,
    /// Server lifecycle requests from administrator endpoints.
    pub server_control: tokio::sync::watch::Sender<Option<ServerAction>>,
}

impl AppState {
    /// Build the shared state, starting the plugin host.
    pub async fn new(
        db: Database,
        paths: DataPaths,
        version: &str,
        runtime: tokio::runtime::Handle,
    ) -> anyhow::Result<Arc<Self>> {
        let server_id = db.ensure_server("jellyfin-clone", version).await?;

        let scanner = Arc::new(Scanner::new(db.clone()));
        let transcoder = Arc::new(Transcoder::new());
        let hls = HlsSessionManager::new(paths.transcode.clone(), Arc::clone(&transcoder));
        let images = ImageCache::new(paths.thumbnails.clone());

        // Plugins load here rather than on first use, so a broken plugin is
        // reported at startup instead of on a metadata request hours later.
        let plugins = Arc::new(
            PluginHost::start(paths.plugins.clone(), db.clone(), runtime)
                .await
                .map_err(|e| anyhow::anyhow!("could not start the plugin host: {e}"))?,
        );

        let (events, _) = tokio::sync::broadcast::channel(256);
        let (server_control, _) = tokio::sync::watch::channel(None);

        let mut state = Self {
            db,
            scanner,
            plugins,
            // Replaced immediately below, once the `Arc` exists.
            scans: Arc::new(ScanManager::empty()),
            transcoder,
            hls,
            images,
            paths,
            server_id,
            server_name: tokio::sync::RwLock::new("jellyfin-clone".to_owned()),
            version: version.to_owned(),
            started_at: std::time::Instant::now(),
            events,
            server_control,
        };

        // The scan manager needs the finished `Arc<AppState>` to reach the
        // scanner, so it is bound after construction rather than before.
        let shared = Arc::new(state);
        shared.scans.bind(Arc::clone(&shared));

        Ok(shared)
    }

    /// Broadcast a server-sent event to connected clients.
    pub fn emit(&self, kind: &str, payload: serde_json::Value) {
        let body = format!("{kind}|{}", payload);
        // A send error just means nobody is listening.
        let _ = self.events.send(body);
    }

    /// Run a plugin's metadata hooks for an item, first result wins.
    pub async fn fetch_metadata(
        &self,
        query: &jf_plugins::MetadataQuery,
    ) -> Option<jf_plugins::MetadataResult> {
        self.plugins.fetch_metadata("", query, 10_000).await.ok().flatten()
    }

    /// Uptime in seconds.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}