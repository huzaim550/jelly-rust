//! The plugin capability API.
//!
//! This is the contract between the server and every plugin, and it is the
//! part of the design that outlived the runtime change from Python to
//! JavaScript. Plugins run in a Node subprocess and never touch the server:
//! they receive a narrow handle and are confined to what it exposes.
//!
//! The boundary is deliberately JSON. Values cross it as strings and plain
//! objects, so the host never has to serialize arbitrary runtime objects, and a
//! plugin cannot smuggle a live reference back into the server.

use jf_core::BaseItemKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Host API version. A plugin whose `apiVersion` major differs is refused.
///
/// Bump the major on a breaking change; bump the minor when adding capability.
pub const API_VERSION: &str = "1.0";

/// What a plugin declares about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// Host API version this plugin was written against.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// Module to import, relative to the plugin root.
    pub entry: String,
    /// Capabilities requested. Anything not listed is denied.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Metadata providers this plugin supplies, e.g. `["tmdb", "omdb"]`.
    #[serde(rename = "metadataProviders", default)]
    pub metadata_providers: Vec<String>,
    /// Plugin ids this one depends on; load order follows these.
    #[serde(rename = "dependsOn", default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::ReadItems => "readItems",
            Capability::Network => "network",
            Capability::HttpRoutes => "httpRoutes",
            Capability::Config => "config",
            Capability::Events => "events",
            Capability::WriteMetadata => "writeMetadata",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("plugin `{plugin}` targets host API {found}, but this server speaks {expected}")]
    ApiMismatch { plugin: String, found: String, expected: String },
    #[error("plugin `{0}` is not loaded")]
    Missing(String),
    #[error("plugin `{0}` failed: {1}")]
    Runtime(String, String),
    #[error("capability `{0}` was requested but not granted")]
    Denied(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("plugin host is unavailable: {0}")]
    Host(String),
}

impl Manifest {
    /// Parse a `plugin.json` manifest.
    pub fn parse(source: &str) -> Result<Self, PluginError> {
        let manifest: Manifest = serde_json::from_str(source)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), PluginError> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(PluginError::InvalidManifest(format!(
                "illegal plugin id `{}`",
                self.id
            )));
        }
        if !self.api_version.starts_with("1.") {
            return Err(PluginError::ApiMismatch {
                plugin: self.id.clone(),
                found: self.api_version.clone(),
                expected: API_VERSION.to_owned(),
            });
        }
        if self.entry.is_empty() {
            return Err(PluginError::InvalidManifest("`entry` is required".into()));
        }
        Ok(())
    }
}

/// What a metadata provider is asked to resolve.
///
/// Field names are camelCase to match the TypeScript SDK. A mismatch here is
/// silent in the dangerous direction: a plugin simply never sees the data, and
/// returns a result with everything missing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataQuery {
    pub title: String,
    #[serde(default)]
    pub year: Option<u32>,
    pub kind: BaseItemKind,
    /// Provider ids already known, from an NFO or another provider.
    #[serde(default)]
    pub known_ids: HashMap<String, String>,
    /// Languages to prefer, most preferred first.
    #[serde(default)]
    pub preferred_languages: Vec<String>,
}

/// What a metadata provider returns. CamelCase, matching the TypeScript SDK.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premiere_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critic_rating: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub studios: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub people: Vec<MetadataPerson>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<MetadataImage>,
    /// Provider ids to persist, keyed by provider name.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_ids: HashMap<String, String>,
}

impl MetadataResult {
    pub fn is_empty(&self) -> bool {
        *self == MetadataResult::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPerson {
    pub name: String,
    #[serde(default)]
    pub role: String,
    pub kind: PersonKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonKind {
    Actor,
    Director,
    Writer,
    Producer,
    GuestStar,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataImage {
    /// Absolute URL, or a filename relative to the media file.
    pub path: String,
    #[serde(default)]
    pub kind: ImageKind,
    #[serde(default)]
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageKind {
    #[default]
    Primary,
    Backdrop,
    Logo,
    Banner,
    Thumb,
}

/// Events delivered to plugins.
///
/// The SDK types these as a flat discriminated union (`{ type, itemId }`), so
/// the variants are encoded adjacently rather than with a nested `payload`
/// object. Encoding them as `{ type, payload: {...} }` would deserialize fine
/// here and be `undefined` in TypeScript.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// A log line emitted by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    pub level: LogLevel,
    pub plugin_id: String,
    pub message: String,
}

/// The item shape plugins see. Deliberately narrow: a plugin that can
/// enumerate the whole database is one step from reading every user's watch
/// history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "id": "tmdb",
        "name": "TMDB Metadata Provider",
        "version": "1.0.0",
        "apiVersion": "1.0",
        "entry": "index.js",
        "capabilities": ["readItems", "network", "writeMetadata"],
        "metadataProviders": ["tmdb"]
    }"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.id, "tmdb");
        assert_eq!(m.entry, "index.js");
        assert_eq!(m.metadata_providers, vec!["tmdb"]);
        assert!(m.capabilities.contains(&Capability::Network));
        assert!(m.capabilities.contains(&Capability::WriteMetadata));
    }

    #[test]
    fn rejects_future_api_version() {
        let bad = GOOD.replace("\"1.0\"", "\"2.0\"");
        let err = Manifest::parse(&bad).unwrap_err();
        assert!(matches!(err, PluginError::ApiMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_illegal_id() {
        let bad = GOOD.replace("\"tmdb\"", "\"../escape\"").replace("\"1.0.0\"", "\"1.0.0\"");
        let err = Manifest::parse(&bad.replace("\"id\": \"tmdb\"", "\"id\": \"../escape\""))
            .unwrap_err();
        assert!(matches!(err, PluginError::InvalidManifest(_)), "got {err:?}");
    }

    #[test]
    fn requires_entry() {
        let bad = GOOD.replace("\"entry\": \"index.js\",", "");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn capability_names_are_stable() {
        assert_eq!(Capability::Network.as_str(), "network");
        assert_eq!(Capability::WriteMetadata.as_str(), "writeMetadata");
    }

    #[test]
    fn metadata_result_emptiness() {
        assert!(MetadataResult::default().is_empty());
        let mut r = MetadataResult::default();
        r.title = Some("x".into());
        assert!(!r.is_empty());
    }

    #[test]
    fn metadata_result_omits_empty_fields() {
        // A provider that fills in one field must not emit a wall of nulls.
        let json = serde_json::to_string(&MetadataResult {
            title: Some("Dune".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(json.contains("\"title\":\"Dune\""));
        assert!(!json.contains("overview"));
        assert!(!json.contains("genres"));
    }

    #[test]
    fn query_round_trips_across_the_boundary() {
        let query = MetadataQuery {
            title: "The Matrix".into(),
            year: Some(1999),
            kind: BaseItemKind::Movie,
            known_ids: HashMap::from([("imdb".to_owned(), "tt0133093".to_owned())]),
            preferred_languages: vec!["en".into()],
        };
        let json = serde_json::to_string(&query).unwrap();
        let back: MetadataQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "The Matrix");
        assert_eq!(back.known_ids.get("imdb").unwrap(), "tt0133093");
    }
}