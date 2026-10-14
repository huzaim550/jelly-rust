//! The plugin capability API.
//!
//! This is the contract between the host and every plugin, in either language.
//! Plugins never touch the server directly; they receive one of these handles
//! and are confined to what it exposes. Keeping the surface small and
//! versioned is what lets plugins keep working across releases.

use jf_core::{BaseItemKind, MediaType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Host API version. A plugin whose `api_version` major differs is refused.
///
/// Bump the major when a change breaks existing plugins; bump the minor when
/// adding capabilities.
pub const API_VERSION: &str = "1.0";

/// What a plugin declares about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Host API version this plugin was written against.
    pub api_version: String,
    /// Python entry module, relative to the plugin root.
    pub entry: String,
    /// Capabilities the plugin requests. Anything not listed is denied.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Providers this plugin supplies, e.g. `["tmdb", "omdb"]`.
    #[serde(default)]
    pub metadata_providers: Vec<String>,
    /// Plugin ids this one depends on; load order follows these.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read libraries and items.
    ReadItems,
    /// Fetch metadata from a network provider.
    Network,
    /// Add its own HTTP routes under `/Plugins/{id}/...`.
    HttpRoutes,
    /// Read and write its own namespaced settings.
    Config,
    /// Register event hooks.
    Events,
    /// Write metadata back onto items.
    WriteMetadata,
}

impl Manifest {
    /// Parse a `manifest.toml`.
    pub fn from_toml(source: &str) -> Result<Self, PluginError> {
        let manifest: Manifest =
            toml::from_str(source).map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), PluginError> {
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
            return Err(PluginError::InvalidManifest(format!("illegal plugin id `{}`", self.id)));
        }
        if !self.api_version.starts_with("1.") {
            return Err(PluginError::ApiMismatch {
                plugin: self.id.clone(),
                found: self.api_version.clone(),
                expected: API_VERSION.to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("plugin `{plugin}` targets host API {found}, but this server speaks {expected}")]
    ApiMismatch { plugin: String, found: String, expected: String },
    #[error("plugin `{0}` is missing")]
    Missing(String),
    #[error("plugin `{0}` raised: {1}")]
    Runtime(String, String),
    #[error("capability `{0}` was requested but not granted")]
    Denied(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("python error: {0}")]
    Python(String),
}

impl From<pyo3::PyErr> for PluginError {
    fn from(err: pyo3::PyErr) -> Self {
        // Python exceptions are ordinary plugin failures: they must surface as
        // errors, never panic across the FFI boundary.
        PluginError::Python(err.to_string())
    }
}

/// What a metadata provider is asked to resolve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataQuery {
    /// Filename hints, already parsed.
    pub title: String,
    pub year: Option<u32>,
    pub kind: BaseItemKind,
    /// Provider ids already known (from NFO or another provider).
    pub known_ids: HashMap<String, String>,
    /// Languages to prefer, most preferred first.
    pub preferred_languages: Vec<String>,
}

/// What a metadata provider returns.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetadataResult {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub official_rating: Option<String>,
    pub status: Option<String>,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub run_time_ticks: Option<i64>,
    pub people: Vec<MetadataPerson>,
    /// Images as URLs or local filenames.
    pub images: Vec<MetadataImage>,
    /// Provider ids to persist, keyed by provider name.
    pub external_ids: HashMap<String, String>,
}

impl MetadataResult {
    pub fn is_empty(&self) -> bool {
        *self == MetadataResult::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataPerson {
    pub name: String,
    #[serde(default)]
    pub role: String,
    pub kind: PersonKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonKind {
    Actor,
    Director,
    Writer,
    Producer,
    GuestStar,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataImage {
    /// Absolute URL, or a filename relative to the media file.
    pub path: String,
    #[serde(default)]
    pub kind: ImageKind,
    #[serde(default)]
    pub index: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    #[default]
    Primary,
    Backdrop,
    Logo,
    Banner,
    Thumb,
}

/// Events plugins can subscribe to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum PluginEvent {
    /// A library scan finished.
    ScanComplete { library_id: String },
    /// An item was added or updated by the scanner.
    ItemDiscovered { item_id: String, library_id: String },
    /// Playback started for an item.
    PlaybackStart { item_id: String, user_id: String },
    /// Playback stopped.
    PlaybackStop { item_id: String, user_id: String, position_ticks: i64 },
    /// A user logged in.
    UserAuthenticated { user_id: String },
}

/// Result of a registered hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    /// `None` means the hook had nothing to contribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataResult>,
    /// Set when the hook wants to modify item metadata in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patched_item: Option<serde_json::Value>,
}

/// Logging sink passed into Python.
#[derive(Debug, Clone, Serialize)]
pub struct LogRecord {
    pub level: LogLevel,
    pub plugin_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Media kinds a plugin may be asked about.
pub fn kind_is_video(kind: BaseItemKind) -> bool {
    matches!(kind, BaseItemKind::Movie | BaseItemKind::Episode | BaseItemKind::Video)
}

/// The media type implied by an item kind.
pub fn media_type_for(kind: BaseItemKind) -> Option<MediaType> {
    match kind {
        BaseItemKind::Movie | BaseItemKind::Episode | BaseItemKind::Video => Some(MediaType::Video),
        BaseItemKind::Audio => Some(MediaType::Audio),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
        id = "tmdb"
        name = "TMDB Provider"
        version = "0.1.0"
        api_version = "1.0"
        entry = "tmdb_plugin"
        capabilities = ["read_items", "network", "write_metadata"]
        metadata_providers = ["tmdb"]
    "#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::from_toml(GOOD).unwrap();
        assert_eq!(m.id, "tmdb");
        assert_eq!(m.metadata_providers, vec!["tmdb"]);
        assert!(m.capabilities.contains(&Capability::Network));
        assert!(m.capabilities.contains(&Capability::WriteMetadata));
    }

    #[test]
    fn rejects_future_api_version() {
        let bad = GOOD.replace("1.0", "2.0");
        let err = Manifest::from_toml(&bad).unwrap_err();
        assert!(matches!(err, PluginError::ApiMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_illegal_id() {
        let bad = GOOD.replace(r#"id = "tmdb""#, r#"id = "../escape""#);
        assert!(matches!(Manifest::from_toml(&bad), Err(PluginError::InvalidManifest(_))));
    }

    #[test]
    fn metadata_result_emptiness() {
        assert!(MetadataResult::default().is_empty());
        let mut r = MetadataResult::default();
        r.title = Some("x".into());
        assert!(!r.is_empty());
    }

    #[test]
    fn kind_helpers() {
        assert!(kind_is_video(BaseItemKind::Movie));
        assert!(!kind_is_video(BaseItemKind::Series));
        assert_eq!(media_type_for(BaseItemKind::Episode), Some(MediaType::Video));
        assert_eq!(media_type_for(BaseItemKind::Series), None);
    }
}