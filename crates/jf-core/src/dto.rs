//! Jellyfin-compatible DTOs.
//!
//! Mirrors `MediaBrowser.Model.Dto` field-for-field where a client depends on
//! it. Auth and user shapes cover multi-user servers with per-user policies.

use crate::{BaseItemKind, CollectionType, MediaType, NameIdPair, PersonRef, RuntimeTicks};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Login request body for `POST /Users/AuthenticateByName`.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthenticateUserByName {
    #[serde(rename = "Username")]
    pub username: String,
    #[serde(rename = "Pw")]
    pub password: Option<String>,
    #[serde(rename = "Password", default)]
    pub password_plain: Option<String>,
}

impl AuthenticateUserByName {
    /// Jellyfin clients send either `Pw` (hashed) or `Password` (plaintext).
    pub fn secret(&self) -> &str {
        self.password
            .as_deref()
            .or(self.password_plain.as_deref())
            .unwrap_or_default()
    }
}

/// Successful login response.
#[derive(Debug, Clone, Serialize)]
pub struct AuthenticationResult {
    #[serde(rename = "User")]
    pub user: UserDto,
    #[serde(rename = "SessionInfo")]
    pub session_info: SessionInfo,
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
}

/// Server-side session metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "UserId")]
    pub user_id: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "Client")]
    pub client: String,
    #[serde(rename = "DeviceName")]
    pub device_name: String,
    #[serde(rename = "DeviceId")]
    pub device_id: String,
    #[serde(rename = "ApplicationVersion")]
    pub application_version: String,
    #[serde(rename = "IsActive")]
    pub is_active: bool,
    #[serde(rename = "SupportsMediaControl")]
    pub supports_media_control: bool,
    #[serde(rename = "SupportsRemoteControl")]
    pub supports_remote_control: bool,
}

// ---------------------------------------------------------------------------
// Users and per-user policy
// ---------------------------------------------------------------------------

/// Per-user access policy. Every field is independent so libraries and folders
/// can be granted or denied per user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserPolicy {
    /// Users an administrator may impersonate. An empty vec means "all".
    #[serde(rename = "EnableAllFolders")]
    pub enable_all_folders: bool,
    #[serde(rename = "EnabledFolders")]
    pub enabled_folders: Vec<String>,
    #[serde(rename = "EnableMediaPlayback")]
    pub enable_media_playback: bool,
    #[serde(rename = "EnableAudioPlaybackTranscoding")]
    pub enable_audio_playback_transcoding: bool,
    #[serde(rename = "EnableVideoPlaybackTranscoding")]
    pub enable_video_playback_transcoding: bool,
    #[serde(rename = "EnablePlaybackRemuxing")]
    pub enable_playback_remuxing: bool,
    #[serde(rename = "EnableContentDeletion")]
    pub enable_content_deletion: bool,
    #[serde(rename = "EnableContentDownloading")]
    pub enable_content_downloading: bool,
    #[serde(rename = "EnableSyncTranscoding")]
    pub enable_sync_transcoding: bool,
    #[serde(rename = "EnableMediaStreamEditing")]
    pub enable_media_stream_editing: bool,
    #[serde(rename = "IsAdministrator")]
    pub is_administrator: bool,
    #[serde(rename = "IsHidden")]
    pub is_hidden: bool,
    #[serde(rename = "IsDisabled")]
    pub is_disabled: bool,
    #[serde(rename = "EnableLiveTvManagement")]
    pub enable_live_tv_management: bool,
    #[serde(rename = "EnableLiveTvAccess")]
    pub enable_live_tv_access: bool,
    #[serde(rename = "MaxActiveSessions")]
    pub max_active_sessions: i32,
    #[serde(rename = "AuthenticationProviderId")]
    pub authentication_provider_id: String,
    #[serde(rename = "PasswordResetProviderId")]
    pub password_reset_provider_id: String,
    #[serde(rename = "SyncPlayAccess")]
    pub sync_play_access: String,
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            enable_all_folders: true,
            enabled_folders: Vec::new(),
            enable_media_playback: true,
            enable_audio_playback_transcoding: true,
            enable_video_playback_transcoding: true,
            enable_playback_remuxing: true,
            enable_content_deletion: false,
            enable_content_downloading: true,
            enable_sync_transcoding: true,
            enable_media_stream_editing: false,
            is_administrator: false,
            is_hidden: false,
            is_disabled: false,
            enable_live_tv_management: false,
            enable_live_tv_access: false,
            max_active_sessions: 0,
            authentication_provider_id: "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
                .into(),
            password_reset_provider_id: "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider"
                .into(),
            sync_play_access: "CreateAndJoinGroups".into(),
        }
    }
}

impl UserPolicy {
    /// An administrator with unrestricted access and deletions enabled.
    pub fn administrator() -> Self {
        Self {
            is_administrator: true,
            enable_content_deletion: true,
            ..Default::default()
        }
    }

    /// Whether this policy permits reading `folder_id`.
    pub fn can_access_folder(&self, folder_id: &str) -> bool {
        self.enable_all_folders
            || self.enabled_folders.iter().any(|f| f == folder_id)
    }
}

/// Per-user UI and playback preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfiguration {
    #[serde(rename = "PlayDefaultAudioTrack")]
    pub play_default_audio_track: bool,
    #[serde(rename = "SubtitleLanguagePreference")]
    pub subtitle_language_preference: String,
    #[serde(rename = "DisplayMissingEpisodes")]
    pub display_missing_episodes: bool,
    #[serde(rename = "GroupedFolders")]
    pub grouped_folders: Vec<Vec<String>>,
    #[serde(rename = "SubtitleMode")]
    pub subtitle_mode: String,
    #[serde(rename = "DisplayCollectionsView")]
    pub display_collections_view: bool,
    #[serde(rename = "EnableLocalPassword")]
    pub enable_local_password: bool,
    #[serde(rename = "OrderedViews")]
    pub ordered_views: Vec<String>,
    #[serde(rename = "LatestItemsExcludes")]
    pub latest_items_excludes: Vec<String>,
    #[serde(rename = "MyMediaExcludes")]
    pub my_media_excludes: Vec<String>,
    #[serde(rename = "HidePlayedInLatest")]
    pub hide_played_in_latest: bool,
    #[serde(rename = "RememberAudioSelections")]
    pub remember_audio_selections: bool,
    #[serde(rename = "RememberSubtitleSelections")]
    pub remember_subtitle_selections: bool,
    #[serde(rename = "EnableNextEpisodeAutoPlay")]
    pub enable_next_episode_auto_play: bool,
}

impl Default for UserConfiguration {
    fn default() -> Self {
        Self {
            play_default_audio_track: true,
            subtitle_language_preference: String::new(),
            display_missing_episodes: true,
            grouped_folders: Vec::new(),
            subtitle_mode: "Default".into(),
            display_collections_view: false,
            enable_local_password: false,
            ordered_views: Vec::new(),
            latest_items_excludes: Vec::new(),
            my_media_excludes: Vec::new(),
            hide_played_in_latest: true,
            remember_audio_selections: true,
            remember_subtitle_selections: true,
            enable_next_episode_auto_play: true,
        }
    }
}

/// A user profile on this server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "PrimaryImageTag")]
    pub primary_image_tag: Option<String>,
    #[serde(rename = "HasPassword")]
    pub has_password: bool,
    #[serde(rename = "HasConfiguredPassword")]
    pub has_configured_password: bool,
    #[serde(rename = "EnableAutoLogin")]
    pub enable_auto_login: bool,
    #[serde(rename = "LastLoginDate", skip_serializing_if = "Option::is_none")]
    pub last_login_date: Option<String>,
    #[serde(rename = "LastActivityDate", skip_serializing_if = "Option::is_none")]
    pub last_activity_date: Option<String>,
    #[serde(rename = "Configuration", default)]
    pub configuration: UserConfiguration,
    #[serde(rename = "Policy", default)]
    pub policy: UserPolicy,
}

/// User record as stored by the database crate, including the password hash.
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    /// Argon2 PHC string. `None` for passwordless users.
    pub password_hash: Option<String>,
    pub server_id: String,
    pub last_login_date: Option<String>,
    pub last_activity_date: Option<String>,
    pub created_at: String,
    pub policy: UserPolicy,
    pub configuration: UserConfiguration,
}

impl UserRecord {
    /// Project to the full wire DTO.
    pub fn to_dto(&self) -> UserDto {
        UserDto {
            name: self.username.clone(),
            server_id: self.server_id.clone(),
            id: self.id.clone(),
            primary_image_tag: None,
            has_password: self.password_hash.is_some(),
            has_configured_password: true,
            enable_auto_login: false,
            last_login_date: self.last_login_date.clone(),
            last_activity_date: self.last_activity_date.clone(),
            configuration: self.configuration.clone(),
            policy: self.policy.clone(),
        }
    }

    /// Project to the login-screen DTO, omitting config and timestamps.
    pub fn to_public_dto(&self, avatar: Option<String>) -> PublicUserDto {
        PublicUserDto {
            name: self.username.clone(),
            id: self.id.clone(),
            server_id: self.server_id.clone(),
            primary_image_tag: avatar,
            has_password: self.password_hash.is_some(),
            has_configured_password: true,
            policy: self.policy.clone(),
        }
    }
}

/// Minimal user shape returned by unauthenticated public endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct PublicUserDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
    #[serde(rename = "PrimaryImageTag")]
    pub primary_image_tag: Option<String>,
    #[serde(rename = "HasPassword")]
    pub has_password: bool,
    #[serde(rename = "HasConfiguredPassword")]
    pub has_configured_password: bool,
    #[serde(rename = "Policy")]
    pub policy: UserPolicy,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// The wire representation of a library item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseItemDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Type")]
    pub kind: BaseItemKind,
    #[serde(rename = "CollectionType", skip_serializing_if = "Option::is_none")]
    pub collection_type: Option<CollectionType>,
    #[serde(rename = "IsFolder")]
    pub is_folder: bool,
    #[serde(rename = "MediaType", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<MediaType>,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Size", skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(rename = "Container", skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(rename = "DateCreated", skip_serializing_if = "Option::is_none")]
    pub date_created: Option<String>,
    #[serde(rename = "ProductionYear", skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(rename = "IndexNumber", skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(rename = "ParentIndexNumber", skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(rename = "Overview", skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(rename = "Genres", skip_serializing_if = "Vec::is_empty", default)]
    pub genres: Vec<String>,
    #[serde(rename = "Taglines", skip_serializing_if = "Vec::is_empty", default)]
    pub taglines: Vec<String>,
    #[serde(rename = "RunTimeTicks", skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<RuntimeTicks>,
    #[serde(rename = "OfficialRating", skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(rename = "CommunityRating", skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f32>,
    #[serde(rename = "CriticRating", skip_serializing_if = "Option::is_none")]
    pub critic_rating: Option<f32>,
    #[serde(rename = "Status", skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "ParentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "SortName", skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(rename = "SeriesName", skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(rename = "SeasonName", skip_serializing_if = "Option::is_none")]
    pub season_name: Option<String>,
    #[serde(rename = "EpisodeTitle", skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    #[serde(rename = "SeriesId", skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(rename = "SeriesPrimaryImageTag", skip_serializing_if = "Option::is_none")]
    pub series_primary_image_tag: Option<String>,
    #[serde(rename = "SeasonId", skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(rename = "PremiereDate", skip_serializing_if = "Option::is_none")]
    pub premiere_date: Option<String>,
    #[serde(rename = "People", skip_serializing_if = "Vec::is_empty", default)]
    pub people: Vec<PersonRef>,
    #[serde(rename = "Tags", skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
    #[serde(rename = "Album", skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(rename = "AlbumId", skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(rename = "AlbumArtist", skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(rename = "Artists", skip_serializing_if = "Vec::is_empty", default)]
    pub artists: Vec<String>,
    #[serde(rename = "AlbumPrimaryImageTag", skip_serializing_if = "Option::is_none")]
    pub album_primary_image_tag: Option<String>,
    #[serde(rename = "ImageTags", skip_serializing_if = "Option::is_none")]
    pub image_tags: Option<std::collections::BTreeMap<String, String>>,
    #[serde(rename = "BackdropImageTags", skip_serializing_if = "Vec::is_empty", default)]
    pub backdrop_image_tags: Vec<String>,
    #[serde(rename = "ParentBackdropItemId", skip_serializing_if = "Option::is_none")]
    pub parent_backdrop_item_id: Option<String>,
    #[serde(rename = "ParentThumbItemId", skip_serializing_if = "Option::is_none")]
    pub parent_thumb_item_id: Option<String>,
    #[serde(rename = "SeriesThumbImageTag", skip_serializing_if = "Option::is_none")]
    pub series_thumb_image_tag: Option<String>,
    #[serde(rename = "CanDelete")]
    pub can_delete: bool,
    #[serde(rename = "CanDownload")]
    pub can_download: bool,
    #[serde(rename = "IsLiveStream")]
    pub is_live_stream: bool,
    #[serde(rename = "ExternalUrls", skip_serializing_if = "Vec::is_empty", default)]
    pub external_urls: Vec<ExternalUrlRef>,
    /// Per-user playback state. Present only on authenticated item responses;
    /// it is what makes "continue watching" and resume positions work.
    #[serde(rename = "UserData", skip_serializing_if = "Option::is_none")]
    pub user_data: Option<UserItemDataDto>,
}

/// External id link on an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUrlRef {
    pub name: String,
    pub url: String,
}

impl ExternalUrlRef {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self { name: name.into(), url: url.into() }
    }
}

/// A named reference returned by item queries (people, genres, studios).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemRefDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "ServerId", skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Library views
// ---------------------------------------------------------------------------

/// Response for `GET /Users/{userId}/Views`.
#[derive(Debug, Clone, Serialize)]
pub struct UserViewsDto {
    #[serde(rename = "Items")]
    pub items: Vec<BaseItemDto>,
    #[serde(rename = "TotalRecordCount")]
    pub total_record_count: usize,
    #[serde(rename = "StartIndex")]
    pub start_index: usize,
}

// ---------------------------------------------------------------------------
// User data (per-user playback state)
// ---------------------------------------------------------------------------

/// Per-user playback state for one item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserItemDataDto {
    #[serde(rename = "PlaybackPositionTicks")]
    pub playback_position_ticks: RuntimeTicks,
    #[serde(rename = "PlayCount")]
    pub play_count: i32,
    #[serde(rename = "IsFavorite")]
    pub is_favorite: bool,
    #[serde(rename = "Played")]
    pub played: bool,
    #[serde(rename = "LastPlayedDate", skip_serializing_if = "Option::is_none")]
    pub last_played_date: Option<String>,
    #[serde(rename = "UnplayedItemCount")]
    pub unplayed_item_count: i32,
}

impl Default for UserItemDataDto {
    fn default() -> Self {
        Self {
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            played: false,
            last_played_date: None,
            unplayed_item_count: 0,
        }
    }
}

/// Request body for `POST /UserPlayedItems/{id}/Progress`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybackProgressDto {
    #[serde(rename = "PositionTicks")]
    pub position_ticks: Option<RuntimeTicks>,
    #[serde(rename = "IsPaused")]
    pub is_paused: Option<bool>,
    #[serde(rename = "CanSeek")]
    pub can_seek: Option<bool>,
    #[serde(rename = "PlayMethod")]
    pub play_method: Option<String>,
    #[serde(rename = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    #[serde(rename = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
}

/// Request body for `POST /Sessions/Playing`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybackStartInfoDto {
    #[serde(rename = "ItemId")]
    pub item_id: String,
    #[serde(rename = "PositionTicks")]
    pub position_ticks: Option<RuntimeTicks>,
    #[serde(rename = "CanSeek")]
    pub can_seek: Option<bool>,
    #[serde(rename = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    #[serde(rename = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(rename = "PlaySessionId")]
    pub play_session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

/// Response for `GET /System/Info/Public`.
#[derive(Debug, Clone, Serialize)]
pub struct PublicSystemInfoDto {
    #[serde(rename = "ServerName")]
    pub server_name: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "StartupWizardCompleted")]
    pub startup_wizard_completed: bool,
}

/// Response for `GET /Users/Public`.
#[derive(Debug, Clone, Serialize)]
pub struct PublicUsersDto {
    #[serde(rename = "ServerId")]
    pub server_id: String,
    #[serde(rename = "ServerName")]
    pub server_name: String,
    #[serde(rename = "Users")]
    pub users: Vec<PublicUserDto>,
}

/// Re-export for convenience in handler code.
pub type NamesResult = Vec<NameIdPair>;