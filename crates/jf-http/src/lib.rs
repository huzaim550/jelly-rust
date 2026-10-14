//! HTTP surface for jellyfin-clone.
//!
//! Route shapes follow Jellyfin's API so existing clients work against this
//! server and the C# reference in `jellyfin/` stays a usable spec.
//!
//! The Python web client under `pydist/` is embedded in the binary, so a
//! deployment is a single file plus its data directory.

pub mod admin;
pub mod auth;
pub mod next_up;
pub mod routes;
pub mod scans;
pub mod state;

use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use state::AppState;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Serve the built web client from `dist`.
///
/// The client is served from disk rather than embedded: `dist/` is a build
/// artifact, and embedding it would make every `cargo build` depend on a prior
/// `npm run build`. This mirrors how Jellyfin itself ships its web client as a
/// directory next to the server.
async fn serve_asset(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path.is_empty() || path == "index.html" {
        return index_response().await;
    }

    let root = web_root();
    let file = root.join(path);

    // Reject traversal outside the client root.
    if file.starts_with(&root) && file.is_file() {
        return serve_file(&file).await;
    }

    // A request naming a file that is genuinely missing must 404. Serving HTML
    // where a `.js` bundle is expected turns a packaging mistake into a
    // baffling JavaScript error instead of a clear 404.
    if looks_like_asset(path) {
        return not_found();
    }

    // Otherwise this is a client-side route (`/item/:id`, `/search`, ...), so
    // hand back the shell and let the client router resolve it.
    index_response().await
}

/// Whether a path names a file rather than a client route.
///
/// Only extension-bearing final segments qualify, so `/assets/index.js` 404s
/// when missing while `/item/abc123` falls through to the app shell.
fn looks_like_asset(path: &str) -> bool {
    let Some(last) = path.rsplit('/').next() else {
        return false;
    };
    match last.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && !ext.is_empty() && !ext.contains('/'),
        None => false,
    }
}

async fn index_response() -> Response {
    let file = web_root().join("index.html");
    if file.is_file() {
        return serve_file(&file).await;
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
        MISSING_CLIENT_HTML,
    )
        .into_response()
}

async fn serve_file(file: &std::path::Path) -> Response {
    let mime = mime_guess::from_path(file).first_or_octet_stream().to_string();

    // Vite emits content-hashed filenames under /assets, so those are safe to
    // cache indefinitely. index.html must not be cached or clients keep
    // loading an old bundle after a deploy.
    let cache = if file.to_string_lossy().contains("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    match tokio::fs::read(file).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap()),
                (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        r#"{"Message":"Not found","Status":404}"#,
    )
        .into_response()
}

/// Directory holding the built web client.
///
/// Overridable with `JF_WEB_DIR` so a packaged build can ship the client
/// anywhere; defaults to `web/dist` beside the working directory.
fn web_root() -> std::path::PathBuf {
    std::env::var("JF_WEB_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("web/dist"))
}

/// Shown when the client has not been built.
const MISSING_CLIENT_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>jellyfin-clone</title>
<style>
 body{background:#0e1116;color:#e6e9ef;font:15px/1.6 system-ui,sans-serif;
      display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}
 div{max-width:520px;padding:32px}
 code{background:#1f242d;padding:2px 7px;border-radius:4px}
 h1{font-size:21px;margin:0 0 12px}
 p{color:#8b95a5}
</style></head>
<body><div>
 <h1>The web client has not been built</h1>
 <p>Run this once, then reload:</p>
 <p><code>cd web &amp;&amp; npm install &amp;&amp; npm run build</code></p>
 <p>The API is still available — try
    <code>/System/Info/Public</code> or <code>/Users/Public</code>.</p>
</div></body></html>"#;

/// Route prefixes owned by the API.
///
/// A request under one of these that matches no route is a genuine API 404 and
/// must return JSON, not the HTML shell.
const API_PREFIXES: &[&str] = &[
    "/Users", "/Sessions", "/System", "/Items", "/Shows", "/Videos", "/Plugins", "/Events",
    "/UserPlayedItems", "/UserFavoriteItems",
];

/// Decide whether an unmatched path is an API call or a client route.
async fn dispatch_unmatched(uri: axum::http::Uri) -> Response {
    let path = uri.path().to_owned();

    if API_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        return routes::api_not_found(uri).await.into_response();
    }

    serve_asset(uri).await
}

/// Build the application router.
pub fn router(app: Arc<AppState>) -> Router {
    Router::new()
        // System
        .route("/System/Info/Public", get(routes::system_info_public))
        .route("/System/Info", get(routes::system_info))
        .route("/System/Shutdown", post(routes::shutdown_server))
        .route("/System/Restart", post(routes::restart_server))
        .route("/Plugins", get(routes::list_plugins))
        .route(
            "/Plugins/omdb/Configuration",
            get(admin::get_omdb_api_key)
                .post(admin::post_omdb_api_key)
                .delete(admin::delete_omdb_api_key),
        )
        // Auth
        .route("/Users/AuthenticateByName", post(routes::authenticate))
        .route("/Users/Public", get(routes::public_users))
        .route("/Users/Me", get(routes::current_user))
        .route("/Sessions/Logout", delete(routes::logout))
        .route("/Sessions", get(routes::sessions))
        .route("/Users/{userId}/Policy", post(routes::update_policy))
        .route("/Users/{userId}/Configuration", post(routes::update_configuration))
        // Library
        .route("/Users/{userId}/Views", get(routes::user_views))
        .route("/Users/{userId}/Items/Resume", get(routes::resume_items))
        .route("/Users/{userId}/Items", get(routes::user_items))
        .route("/Users/{userId}/Items/{itemId}", get(routes::user_item))
        .route("/Users/{userId}/Shows/NextUp", get(next_up::next_up))
        .route("/Shows/{seriesId}/Seasons", get(routes::show_seasons))
        .route("/Shows/{seriesId}/Episodes", get(routes::show_episodes))
        // Playback
        .route("/Items/{itemId}/PlaybackInfo", get(routes::playback_info))
        .route("/Videos/{itemId}/stream", get(routes::stream_video))
        .route("/Videos/{itemId}/master.m3u8", get(routes::hls_master))
        .route("/transcode/{sessionId}/{segment}", get(routes::hls_segment))
        .route("/UserPlayedItems/{itemId}/Progress", post(routes::report_progress))
        .route("/UserFavoriteItems/{itemId}", post(routes::favorite))
        .route("/UserFavoriteItems/{itemId}", delete(routes::favorite))
        .route("/Sessions/Playing", post(routes::playing))
        .route("/Sessions/Playing/Stopped", post(routes::stopped))
        // Onboarding
        .route("/Startup/Configuration", get(admin::get_startup_configuration))
        .route("/Startup/Configuration", post(admin::post_startup_configuration))
        .route("/Startup/FirstUser", get(admin::get_first_user))
        .route("/Startup/User", post(admin::post_startup_user))
        .route("/Startup/Complete", post(admin::post_startup_complete))
        // Library administration
        .route("/Library/VirtualFolders", get(admin::get_virtual_folders))
        .route("/Library/VirtualFolders", post(admin::add_virtual_folder))
        .route("/Library/VirtualFolders", axum::routing::delete(admin::delete_virtual_folder))
        .route("/Library/PhysicalPaths", get(admin::get_physical_paths))
        .route("/Library/Refresh", post(admin::refresh_library))
        .route("/Library/ScanStatus", get(admin::get_scan_status))
        // Images and events
        .route("/Items/{itemId}/Images/{kind}", get(routes::item_image))
        .route("/Events", get(routes::events))
        // One fallback for everything unmatched. Registering a second
        // `.fallback()` on the API router shadowed the client shell, so a
        // refresh on any client-side route 404'd instead of booting the app.
        .fallback(get(dispatch_unmatched))
        .layer(CompressionLayer::new())
        // The browser client needs credentialed cross-origin requests during
        // development, when it is served from a different port.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}

/// Configuration for a server instance.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub data_dir: std::path::PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_owned(),
            port: 8096,
            data_dir: std::env::var("JF_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("./jf-data")),
        }
    }
}

impl ServerConfig {
    /// Build the shared state and return a ready-to-serve router.
    pub async fn build(&self, version: &str) -> anyhow::Result<Router> {
        let paths = state::DataPaths::new(self.data_dir.clone());
        paths.ensure().await?;

        let db = jf_db::Database::connect(&paths.database).await?;
        let app = AppState::new(db, paths, version, tokio::runtime::Handle::current()).await?;
        Ok(router(app))
    }
}