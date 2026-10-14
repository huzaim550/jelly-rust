//! Core domain types and Jellyfin-compatible DTOs.
//!
//! Field names intentionally mirror `MediaBrowser.Model.Dto` so that existing
//! Jellyfin clients can authenticate against this server and so the C#
//! reference implementation in `jellyfin/` acts as a live spec.

pub mod dto;
pub mod ids;

pub use dto::*;
pub use ids::{item_id, new_item_id, IdParseError};

use serde::{Deserialize, Serialize};

/// Media type. Serializes to Jellyfin's `MediaType` string values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    #[serde(rename = "Video")]
    Video,
    #[serde(rename = "Audio")]
    Audio,
    #[serde(rename = "Photo")]
    Photo,
    #[serde(rename = "Book")]
    Book,
}

/// Base item kind. Serializes to Jellyfin's `Type` string values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum BaseItemKind {
    AggregateFolder,
    UserRootFolder,
    UserView,
    CollectionFolder,
    Folder,
    Movie,
    Series,
    Season,
    Episode,
    BoxSet,
    Video,
    MusicAlbum,
    MusicArtist,
    Audio,
    Playlist,
    PhotoAlbum,
    Recording,
    Program,
    TvChannel,
    Person,
    Studio,
    Genre,
    MusicGenre,
}

/// Collection type, used to shape top-level library views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CollectionType {
    Movies,
    TvShows,
    Music,
    MusicVideos,
    Books,
    Photos,
    Playlists,
    Mixed,
}

/// Tick count for a runtime, in 100-nanosecond intervals (Jellyfin convention).
pub type RuntimeTicks = i64;

pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_MINUTE: i64 = 60 * TICKS_PER_SECOND;

/// Convert a duration in seconds to runtime ticks.
pub fn secs_to_ticks(secs: f64) -> RuntimeTicks {
    (secs * TICKS_PER_SECOND as f64).round() as i64
}

/// Convert runtime ticks to a duration in seconds.
pub fn ticks_to_secs(ticks: RuntimeTicks) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64
}

/// One contributor attached to an item (actor, director, writer, artist).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameIdPair {
    pub name: String,
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// A person or artist reference stored on an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonRef {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub role: String,
    #[serde(rename = "primaryImageTag", skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
}

/// A single tagged image (poster, backdrop, thumb, logo, banner).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageTags {
    #[serde(flatten)]
    pub map: std::collections::BTreeMap<ImageType, String>,
}

/// The image slots Jellyfin clients know how to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ImageType {
    #[serde(rename = "Primary")]
    Primary,
    #[serde(rename = "Backdrop")]
    Backdrop,
    #[serde(rename = "Thumb")]
    Thumb,
    #[serde(rename = "Logo")]
    Logo,
    #[serde(rename = "Banner")]
    Banner,
}

/// Streaming protocol used for playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MediaProtocol {
    File,
    Http,
    Hls,
}

/// A playable media source for an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSourceInfo {
    pub id: String,
    #[serde(rename = "Protocol")]
    pub protocol: MediaProtocol,
    pub path: String,
    pub container: Option<String>,
    pub size: Option<i64>,
    #[serde(rename = "RunTimeTicks")]
    pub run_time_ticks: RuntimeTicks,
    #[serde(rename = "MediaStreams", skip_serializing_if = "Vec::is_empty", default)]
    pub media_streams: Vec<MediaStream>,
}

/// A single video, audio, or subtitle stream within a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStream {
    #[serde(rename = "Codec")]
    pub codec: Option<String>,
    #[serde(rename = "Type")]
    pub stream_type: MediaStreamType,
    pub index: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(rename = "Language", skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "IsDefault")]
    pub is_default: bool,
    #[serde(rename = "IsForced")]
    pub is_forced: bool,
    #[serde(rename = "IsExternal")]
    pub is_external: bool,
}

/// Distinguishes video/audio/subtitle streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaStreamType {
    Video,
    Audio,
    Subtitle,
    Data,
}

/// Aggregate result of querying `/Items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult<T> {
    #[serde(rename = "Items")]
    pub items: Vec<T>,
    #[serde(rename = "TotalRecordCount")]
    pub total_record_count: usize,
    #[serde(rename = "StartIndex")]
    pub start_index: usize,
}