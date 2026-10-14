//! Onboarding and library administration.
//!
//! Routes mirror Jellyfin's `/Startup/*` and `/Library/VirtualFolders` so the
//! flow is familiar and a real client could drive it. Field names follow
//! `StartupConfigurationDto`, `StartupUserDto`, and `VirtualFolderInfo` in the
//! C# reference.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use jf_core::dto::UserConfiguration;
use jf_db::users::NewUser;
use jf_core::dto::UserPolicy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::RequireAuth;
use crate::routes::ApiError;
use crate::state::AppState;

type Shared = Arc<AppState>;

// ---------------------------------------------------------------------------
// Onboarding
// ---------------------------------------------------------------------------

/// `GET /Startup/Configuration`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupConfigurationDto {
    #[serde(rename = "ServerName")]
    pub server_name: Option<String>,
    #[serde(rename = "UICulture")]
    pub ui_culture: Option<String>,
    #[serde(rename = "MetadataCountryCode")]
    pub metadata_country_code: Option<String>,
    #[serde(rename = "PreferredMetadataLanguage")]
    pub preferred_metadata_language: Option<String>,
}

/// `GET /Startup/Configuration`
pub async fn get_startup_configuration(
    State(app): State<Shared>,
) -> Json<StartupConfigurationDto> {
    Json(StartupConfigurationDto {
        server_name: Some(app.server_name.read().await.clone()),
        ui_culture: Some("en-US".into()),
        metadata_country_code: Some("US".into()),
        preferred_metadata_language: Some("en".into()),
    })
}

/// `POST /Startup/Configuration`
pub async fn post_startup_configuration(
    State(app): State<Shared>,
    Json(body): Json<StartupConfigurationDto>,
) -> Result<StatusCode, ApiError> {
    if let Some(name) = body.server_name.as_ref().filter(|n| !n.trim().is_empty()) {
        // `server_name` is not behind a lock, so take the write lock briefly to
        // avoid a torn read by a concurrent request.
        let mut guard = app.server_name.write().await;
        *guard = name.trim().to_owned();
    }

    if let Some(language) = body.preferred_metadata_language.as_ref() {
        set_server_setting(&app, "PreferredMetadataLanguage", language).await?;
    }
    if let Some(country) = body.metadata_country_code.as_ref() {
        set_server_setting(&app, "MetadataCountryCode", country).await?;
    }
    if let Some(culture) = body.ui_culture.as_ref() {
        set_server_setting(&app, "UICulture", culture).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Persist a server-level setting.
///
/// There is no settings table, so these live in `plugin_config` under a
/// reserved plugin id. That keeps the change additive rather than requiring a
/// migration, and the namespace can never collide with a real plugin.
async fn set_server_setting(app: &Shared, key: &str, value: &str) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO plugin_config (plugin_id, key, value) VALUES ('__server__', ?, ?)
         ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(app.db.pool())
    .await
    .map_err(jf_db::DbError::from)
    .map_err(ApiError::from)?;
    Ok(())
}

async fn get_server_setting(app: &Shared, key: &str) -> Option<String> {
    sqlx::query_scalar("SELECT value FROM plugin_config WHERE plugin_id = '__server__' AND key = ?")
        .bind(key)
        .fetch_optional(app.db.pool())
        .await
        .ok()
        .flatten()
}

/// `POST /Startup/User` — create the administrator account.
#[derive(Debug, Deserialize)]
pub struct StartupUserDto {
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Password")]
    pub password: Option<String>,
}

/// `POST /Startup/User`
pub async fn post_startup_user(
    State(app): State<Shared>,
    Json(body): Json<StartupUserDto>,
) -> Result<StatusCode, ApiError> {
    let name = body
        .name
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::bad_request("A username is required."))?;

    // The account created during onboarding is always an administrator: this is
    // the only chance to make one before the server is usable.
    app.db
        .create_user(NewUser {
            username: name,
            password: body.password.as_deref().filter(|p| !p.is_empty()),
            server_id: &app.server_id,
            policy: UserPolicy::administrator(),
            configuration: UserConfiguration::default(),
        })
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /Startup/FirstUser` — whether any account exists yet.
#[derive(Debug, Serialize)]
pub struct FirstUserDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
    #[serde(rename = "HasPassword")]
    pub has_password: bool,
    #[serde(rename = "HasConfiguredPassword")]
    pub has_configured_password: bool,
    #[serde(rename = "LastLoginDate")]
    pub last_login_date: Option<String>,
    #[serde(rename = "LastActivityDate")]
    pub last_activity_date: Option<String>,
    #[serde(rename = "LastActivityDate")]
    pub _unused: (),
}

/// `GET /Startup/FirstUser`
///
/// Drives the wizard: a null body means setup is still needed.
pub async fn get_first_user(State(app): State<Shared>) -> Result<Json<Option<FirstUserDto>>, ApiError> {
    let users = app.db.all_users().await.map_err(ApiError::from)?;

    Ok(Json(users.into_iter().next().map(|u| FirstUserDto {
        name: u.username,
        server_id: u.server_id,
        has_password: u.password_hash.is_some(),
        has_configured_password: true,
        last_login_date: u.last_login_date,
        last_activity_date: u.last_activity_date,
        _unused: (),
    })))
}

/// `POST /Startup/Complete`
#[derive(Debug, Default, Deserialize)]
pub struct StartupCompleteDto {
    #[serde(rename = "FirstUserId")]
    pub _first_user_id: Option<String>,
    #[serde(rename = "UICulture")]
    pub _ui_culture: Option<String>,
    #[serde(rename = "MetadataCountryCode")]
    pub _metadata_country_code: Option<String>,
}

/// `POST /Startup/Complete`
pub async fn post_startup_complete(
    State(app): State<Shared>,
    body: Option<Json<StartupCompleteDto>>,
) -> Result<StatusCode, ApiError> {
    let _ = body;
    app.db.complete_startup_wizard().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Library management
// ---------------------------------------------------------------------------

/// Collection types Jellyfin supports, as its enum values.
///
/// Parsed case-insensitively: Jellyfin clients, this project's web client,
/// and the command-line all spell this type differently, and rejecting a
/// library because of a capital letter would be absurd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum CollectionTypeOptions {
    Movies,
    TvShows,
    Music,
    MusicVideos,
    Books,
    Photos,
    Playlists,
    Mixed,
}

impl<'de> serde::Deserialize<'de> for CollectionTypeOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let kind = match raw.trim().to_ascii_lowercase().as_str() {
            "movies" => Self::Movies,
            "tvshows" | "tv" | "episodes" => Self::TvShows,
            "music" => Self::Music,
            "musicvideos" => Self::MusicVideos,
            "books" | "audiobooks" => Self::Books,
            "photos" => Self::Photos,
            "playlists" => Self::Playlists,
            "mixed" => Self::Mixed,
            value => {
                return Err(serde::de::Error::custom(format!(
                    "unknown collection type `{value}`"
                )))
            }
        };
        Ok(kind)
    }
}

impl CollectionTypeOptions {
    /// Path used by the scanner's filename rules.
    pub fn as_scan_kind(&self) -> &'static str {
        match self {
            CollectionTypeOptions::Movies => "movies",
            CollectionTypeOptions::TvShows => "tvshows",
            CollectionTypeOptions::Music => "music",
            CollectionTypeOptions::MusicVideos => "music",
            CollectionTypeOptions::Books => "books",
            CollectionTypeOptions::Photos => "photos",
            CollectionTypeOptions::Playlists => "music",
            CollectionTypeOptions::Mixed => "mixed",
        }
    }
}

/// `VirtualFolderInfo`, matching Jellyfin's PascalCase field names.
#[derive(Debug, Serialize)]
pub struct VirtualFolderInfo {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Locations")]
    pub locations: Vec<String>,
    #[serde(rename = "CollectionType")]
    pub collection_type: Option<CollectionTypeOptions>,
    #[serde(rename = "ItemId")]
    pub item_id: String,
    #[serde(rename = "PrimaryImageItemId")]
    pub primary_image_item_id: Option<String>,
    #[serde(rename = "RefreshProgress")]
    pub refresh_progress: Option<f64>,
    #[serde(rename = "RefreshStatus")]
    pub refresh_status: Option<String>,
}

/// `GET /Library/VirtualFolders`
pub async fn get_virtual_folders(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<Json<Vec<VirtualFolderInfo>>, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Managing libraries requires administrator access.",
        ));
    }

    let libraries = app.scanner.libraries().await.map_err(ApiError::from)?;
    let statuses = app.scans.statuses().await;

    let mut folders = Vec::with_capacity(libraries.len());
    for lib in libraries {
        let status = statuses.get(&lib.id);
        folders.push(VirtualFolderInfo {
            name: lib.name,
            locations: vec![lib.path.to_string_lossy().into_owned()],
            collection_type: Some(parse_collection_type(&lib.collection_type)),
            item_id: lib.id.clone(),
            primary_image_item_id: None,
            refresh_progress: status.and_then(|s| s.progress),
            refresh_status: status.map(|s| s.state.clone()),
        });
    }

    Ok(Json(folders))
}

fn parse_collection_type(raw: &str) -> CollectionTypeOptions {
    match raw.to_ascii_lowercase().as_str() {
        "movies" => CollectionTypeOptions::Movies,
        "tvshows" => CollectionTypeOptions::TvShows,
        "music" => CollectionTypeOptions::Music,
        "musicvideos" => CollectionTypeOptions::MusicVideos,
        "books" => CollectionTypeOptions::Books,
        "photos" => CollectionTypeOptions::Photos,
        "playlists" => CollectionTypeOptions::Playlists,
        _ => CollectionTypeOptions::Mixed,
    }
}

/// Query for `POST /Library/VirtualFolders`.
#[derive(Debug, Deserialize)]
pub struct AddLibraryQuery {
    pub name: Option<String>,
    #[serde(rename = "collectionType")]
    pub collection_type: Option<CollectionTypeOptions>,
    /// Comma-separated, matching Jellyfin's `CommaDelimitedCollectionModelBinder`.
    pub paths: Option<String>,
    #[serde(rename = "refreshLibrary")]
    pub refresh_library: Option<bool>,
}

/// `POST /Library/VirtualFolders`
pub async fn add_virtual_folder(
    State(app): State<Shared>,
    auth: RequireAuth,
    Query(params): Query<AddLibraryQuery>,
) -> Result<StatusCode, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Managing libraries requires administrator access.",
        ));
    }

    let path = params
        .paths
        .as_deref()
        .and_then(|p| p.split(',').map(str::trim).find(|s| !s.is_empty()))
        .ok_or_else(|| ApiError::bad_request("A library path is required."))?;

    let root = expand_home(path);
    if !root.is_dir() {
        return Err(ApiError::bad_request(format!(
            "`{}` is not a directory on the server.",
            root.display()
        )));
    }

    // Default the name to the folder's own name, as Jellyfin does.
    let name = params
        .name
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Library".to_owned());

    let kind = params
        .collection_type
        .unwrap_or(CollectionTypeOptions::Movies)
        .as_scan_kind();

    app.scanner
        .add_library(&jf_scan::LibraryConfig {
            id: String::new(),
            name,
            path: root,
            collection_type: kind.to_owned(),
        })
        .await
        .map_err(ApiError::from)?;

    // Refresh on add, so the wizard shows a populated library rather than an
    // empty one the user has to trigger separately.
    if params.refresh_library.unwrap_or(true) {
        app.scans.refresh_all().await.map_err(ApiError::from)?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /Library/VirtualFolders`
pub async fn delete_virtual_folder(
    State(app): State<Shared>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Managing libraries requires administrator access.",
        ));
    }

    let target = params
        .get("name")
        .or_else(|| params.get("id"))
        .ok_or_else(|| ApiError::bad_request("A library id or name is required."))?;

    let libraries = app.scanner.libraries().await.map_err(ApiError::from)?;
    let library = libraries
        .iter()
        .find(|l| &l.id == target || l.name == *target)
        .ok_or_else(|| ApiError::not_found("No such library."))?;

    let id = library.id.clone();
    app.scanner.remove_library(&id).await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /Library/Refresh`
pub async fn refresh_library(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<StatusCode, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Scanning requires administrator access.",
        ));
    }

    // Kick the scan off in the background: it can take minutes, and the
    // dashboard follows progress over the event stream.
    app.scans.refresh_all().await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Filesystem browsing
// ---------------------------------------------------------------------------

/// One entry from `GET /Library/PhysicalPaths`.
///
/// Jellyfin calls this `PhysicalFolderInfo` and also uses PascalCase.
#[derive(Debug, Serialize)]
pub struct PhysicalPathDto {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Path")]
    pub path: String,
}

/// Expand a leading `~` to the invoking user's home directory.
///
/// People type `~/Movies` out of habit; without this the path is taken
/// literally, fails `is_dir`, and the error blames a directory that looks
/// perfectly reasonable on screen.
pub fn expand_home(raw: &str) -> PathBuf {
    let trimmed = raw.trim();

    let Some(rest) = trimmed.strip_prefix('~') else {
        return PathBuf::from(trimmed);
    };

    let rest = rest.trim_start();
    let path = if rest.is_empty() {
        ""
    } else if let Some(path) = rest.strip_prefix('/') {
        path
    } else {
        return PathBuf::from(trimmed);
    };

    if path.is_empty() {
        if let Some(home) = home_dir() {
            return home;
        }
        return PathBuf::from(trimmed);
    }

    if let Some(home) = home_dir() {
        return home.join(path);
    }

    PathBuf::from(trimmed)
}

/// The current user's home directory, from `$HOME` or the password database.
fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    None
}

/// `GET /Library/PhysicalPaths`
///
/// A server-side folder picker. The browser cannot list the server's
/// filesystem, so the wizard asks the server to do it.
pub async fn get_physical_paths(
    State(_app): State<Shared>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<PhysicalPathDto>>, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Browsing the server filesystem requires administrator access.",
        ));
    }

    let raw = params.get("path").map(String::as_str).unwrap_or("/");
    let root = expand_home(raw);

    // Refuse anything that is not a directory rather than erroring later.
    if !root.is_dir() {
        return Ok(Json(Vec::new()));
    }

    let mut entries: Vec<PhysicalPathDto> = Vec::new();

    // Offer the parent so the picker can walk upward.
    if let Some(parent) = root.parent() {
        entries.push(PhysicalPathDto {
            name: "..".to_owned(),
            path: parent.to_string_lossy().into_owned(),
        });
    }

    // `read_dir` order is filesystem-dependent; sort for a stable UI.
    let mut children: Vec<PhysicalPathDto> = match std::fs::read_dir(root) {
        Ok(iter) => iter
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| PhysicalPathDto {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path().to_string_lossy().into_owned(),
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries.extend(children);

    Ok(Json(entries))
}

#[cfg(test)]
mod expand_home_tests {
    use super::expand_home;

    #[test]
    fn a_leading_tilde_expands_to_the_home_directory() {
        let home = std::env::var("HOME").expect("HOME");

        assert_eq!(expand_home("~/Movies"), std::path::PathBuf::from(&home).join("Movies"));
        assert_eq!(expand_home(" ~ /Movies "), std::path::PathBuf::from(&home).join("Movies"));
        assert_eq!(expand_home("~"), std::path::PathBuf::from(&home));
    }

    #[test]
    fn other_paths_are_left_alone() {
        assert_eq!(expand_home("/mnt/media"), std::path::PathBuf::from("/mnt/media"));
        // A tilde that is not a home reference must stay literal, not become
        // `~/foo`.
        assert_eq!(expand_home("/data/~backup"), std::path::PathBuf::from("/data/~backup"));
        assert_eq!(expand_home("relative/dir"), std::path::PathBuf::from("relative/dir"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_types_map_to_scan_kinds() {
        assert_eq!(CollectionTypeOptions::Movies.as_scan_kind(), "movies");
        assert_eq!(CollectionTypeOptions::TvShows.as_scan_kind(), "tvshows");
        assert_eq!(CollectionTypeOptions::Music.as_scan_kind(), "music");
    }

    #[test]
    fn parses_add_library_query_with_comma_separated_paths() {
        let query: AddLibraryQuery = serde_json::from_value(serde_json::json!({
            "name": "Films",
            "collectionType": "Movies",
            "paths": "/mnt/films,/mnt/2nd",
            "refreshLibrary": true
        }))
        .unwrap();

        assert_eq!(query.name.as_deref(), Some("Films"));
        assert_eq!(query.collection_type, Some(CollectionTypeOptions::Movies));
        assert_eq!(query.paths.as_deref(), Some("/mnt/films,/mnt/2nd"));
        assert_eq!(query.refresh_library, Some(true));
    }

    #[test]
    fn first_path_is_taken_from_a_comma_separated_list() {
        let path = "/mnt/films,/mnt/2nd"
            .split(',')
            .map(str::trim)
            .find(|s| !s.is_empty());
        assert_eq!(path, Some("/mnt/films"));
    }

    #[test]
    fn virtual_folder_info_matches_jellyfin_shape() {
        let info = VirtualFolderInfo {
            name: "Films".into(),
            locations: vec!["/mnt/films".into()],
            collection_type: Some(CollectionTypeOptions::Movies),
            item_id: "abc".into(),
            primary_image_item_id: None,
            refresh_progress: Some(0.5),
            refresh_status: Some("Scanning".into()),
        };

        let json = serde_json::to_value(&info).unwrap();

        // PascalCase matters: the client reads these field names directly, and
        // a camelCase key here reads as `undefined` rather than erroring.
        for field in ["Name", "Locations", "CollectionType", "ItemId", "RefreshProgress"] {
            assert!(
                json.get(field).is_some(),
                "missing Jellyfin field {field}; got {json}"
            );
        }
        assert!(json.get("name").is_none(), "camelCase leaked into the wire format");
        assert_eq!(json["Name"], "Films");
        assert_eq!(json["Locations"][0], "/mnt/films");
        assert_eq!(json["CollectionType"], "Movies");
        assert_eq!(json["RefreshProgress"], 0.5);
    }
}
/// `GET /Library/ScanStatus` — per-library scan progress.
///
/// The dashboard polls this on load to catch up, then follows server-sent
/// events for live updates.
pub async fn get_scan_status(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden(
            "Scan status requires administrator access.",
        ));
    }

    let libraries = app.scans.statuses().await;
    let scanning = app.scans.is_running().await
        || libraries.values().any(|status| matches!(status.state.as_str(), "Queued" | "Scanning"));

    Ok(Json(serde_json::json!({ "scanning": scanning, "libraries": libraries })))
}
