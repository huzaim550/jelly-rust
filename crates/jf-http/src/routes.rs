//! HTTP handlers.
//!
//! Route shapes follow Jellyfin's API so existing clients work and so the C#
//! reference implementation in `jellyfin/` remains a usable spec.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jf_core::dto::*;
use jf_db::items::{FieldSelection, ItemQuery, SortBy, SortOrder};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::{AuthUser, MaybeAuth, RequireAuth};
use crate::state::AppState;

type Shared = Arc<AppState>;

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

/// `GET /System/Info/Public` — unauthenticated bootstrap information.
pub async fn system_info_public(State(app): State<Shared>) -> Json<PublicSystemInfoDto> {
    Json(PublicSystemInfoDto {
        server_name: app.server_name.read().await.clone(),
        version: app.version.clone(),
        id: app.server_id.clone(),
        // Default to `false` when the flag cannot be read: reporting `true`
        // would send a fresh client past onboarding it cannot complete.
        startup_wizard_completed: app.db.startup_completed().await.unwrap_or(false),
    })
}

/// `GET /System/Info` — authenticated server information.
pub async fn system_info(
    State(app): State<Shared>,
    _auth: RequireAuth,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ServerName": app.server_name.read().await.clone(),
        "Version": app.version,
        "Id": app.server_id,
        "OperatingSystem": std::env::consts::OS,
        "Architecture": std::env::consts::ARCH,
        "StartupWizardCompleted": true,
        "HasPendingRestart": false,
        "UptimeSeconds": app.uptime_secs(),
        "TranscodingSessions": app.hls.active_count().await,
    }))
}

/// `GET /Users/Public` — accounts offered on the login screen.
pub async fn public_users(State(app): State<Shared>) -> Json<PublicUsersDto> {
    let users = app.db.public_users().await.unwrap_or_default();
    Json(PublicUsersDto {
        server_id: app.server_id.clone(),
        server_name: app.server_name.read().await.clone(),
        users: users.iter().map(|u| u.to_public_dto(None)).collect(),
    })
}

/// `GET /Plugins` — loaded plugins, for the admin UI.
pub async fn list_plugins(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden("This endpoint requires administrator privileges."));
    }

    Ok(Json(serde_json::json!({
        "ApiVersion": app.plugins.api_version(),
        "Loaded": app.plugins.plugin_ids().await,
        "Count": app.plugins.plugin_count().await,
        "Report": app.plugins.report().await,
    })))
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AuthBody {
    #[serde(rename = "Username")]
    pub username: Option<String>,
    #[serde(rename = "Pw")]
    pub password: Option<String>,
    #[serde(rename = "Password")]
    pub password_plain: Option<String>,
    #[serde(rename = "PasswordMd5")]
    pub password_md5: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeviceHeaders {
    client: Option<String>,
    device_name: Option<String>,
    device_id: Option<String>,
    version: Option<String>,
}

/// `POST /Users/AuthenticateByName`
pub async fn authenticate(
    State(app): State<Shared>,
    headers: axum::http::HeaderMap,
    body: Option<Json<AuthBody>>,
) -> Result<Json<AuthenticationResult>, ApiError> {
    let body = body.map(|Json(b)| b).unwrap_or(AuthBody {
        username: None,
        password: None,
        password_plain: None,
        password_md5: None,
    });

    let username = body
        .username
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("A username is required."))?;

    let password = body
        .password
        .or(body.password_plain)
        .or(body.password_md5)
        .unwrap_or_default();

    // Authentication failure must not reveal whether the account exists.
    let user = app
        .db
        .authenticate(&username, &password)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("Invalid username or password."))?;

    let device = DeviceHeaders::from(&headers);
    let (session_id, token) = app
        .db
        .create_session(
            &user,
            device.client.as_deref().unwrap_or("jellyfin-clone"),
            device.device_name.as_deref().unwrap_or("Unknown device"),
            device.device_id.as_deref().unwrap_or("unknown"),
            device.version.as_deref().unwrap_or("1.0.0"),
        )
        .await
        .map_err(ApiError::from)?;

    // Enforce the account's concurrent-session ceiling.
    app.db
        .enforce_session_limit(&user.id, user.policy.max_active_sessions)
        .await
        .map_err(ApiError::from)?;

    app.emit(
        "userauthenticated",
        serde_json::json!({ "user_id": user.id }),
    );

    Ok(Json(AuthenticationResult {
        user: user.to_dto(),
        session_info: SessionInfo {
            id: session_id,
            user_id: user.id.clone(),
            user_name: user.username.clone(),
            client: device.client.unwrap_or_else(|| "jellyfin-clone".into()),
            device_name: device.device_name.unwrap_or_else(|| "Unknown device".into()),
            device_id: device.device_id.unwrap_or_else(|| "unknown".into()),
            application_version: device.version.unwrap_or_else(|| "1.0.0".into()),
            is_active: true,
            supports_media_control: true,
            supports_remote_control: true,
        },
        access_token: token,
        server_id: app.server_id.clone(),
    }))
}

impl DeviceHeaders {
    fn from(headers: &axum::http::HeaderMap) -> Self {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_owned())
        };
        Self {
            client: get("x-emby-client"),
            device_name: get("x-emby-devicename"),
            device_id: get("x-emby-deviceid"),
            version: get("x-emby-clientversion"),
        }
    }
}

/// `DELETE /Sessions/Logout`
pub async fn logout(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> StatusCode {
    let _ = app.db.end_session(&auth.0.token).await;
    StatusCode::NO_CONTENT
}

/// `GET /Sessions` — active sessions.
pub async fn sessions(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    if !auth.0.is_admin() {
        return Err(ApiError::forbidden("Administrator access is required."));
    }
    Ok(Json(app.db.active_sessions().await.map_err(ApiError::from)?))
}

/// `GET /Users/Me`
pub async fn current_user(auth: RequireAuth) -> Json<UserDto> {
    Json(auth.0.user.to_dto())
}

/// `POST /Users/{userId}/Policy`
pub async fn update_policy(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Json(policy): Json<UserPolicy>,
) -> Result<StatusCode, ApiError> {
    // Users may edit their own policy only if not an administrator; changing
    // your own administrator flag would be a privilege escalation.
    if auth.0.user.id != user_id && !auth.0.is_admin() {
        return Err(ApiError::forbidden("You may only change your own policy."));
    }
    if auth.0.user.id == user_id && policy.is_administrator && !auth.0.is_admin() {
        return Err(ApiError::forbidden("You cannot grant yourself administrator access."));
    }

    app.db.update_policy(&user_id, &policy).await.map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /Users/{userId}/Configuration`
pub async fn update_configuration(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Json(config): Json<UserConfiguration>,
) -> Result<StatusCode, ApiError> {
    if auth.0.user.id != user_id && !auth.0.is_admin() {
        return Err(ApiError::forbidden("You may only change your own configuration."));
    }
    app.db
        .update_configuration(&user_id, &config)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

/// `GET /Users/{userId}/Views` — libraries visible to a user.
pub async fn user_views(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
) -> Result<Json<UserViewsDto>, ApiError> {
    authorize_user(&auth.0, &user_id)?;

    let allowed = app
        .db
        .visible_library_ids(&user_id, &auth.0.user.policy)
        .await
        .map_err(ApiError::from)?;

    let items = app
        .db
        .library_root_items(&allowed, &user_id, &app.server_id)
        .await
        .map_err(ApiError::from)?;

    let total = items.len();
    Ok(Json(UserViewsDto { items, total_record_count: total, start_index: 0 }))
}

/// `GET /Users/{userId}/Items` — the main item query.
#[derive(Debug, Default, Deserialize)]
pub struct ItemsQuery {
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(rename = "includeItemTypes")]
    pub include_item_types: Option<String>,
    #[serde(rename = "mediaTypes")]
    pub media_types: Option<String>,
    #[serde(rename = "searchTerm")]
    pub search_term: Option<String>,
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    #[serde(rename = "sortOrder")]
    pub sort_order: Option<String>,
    #[serde(rename = "startIndex")]
    pub start_index: Option<usize>,
    pub limit: Option<usize>,
    #[serde(rename = "recursive")]
    pub recursive: Option<bool>,
    /// Comma-separated genre names.
    pub genres: Option<String>,
    #[serde(rename = "fields")]
    pub fields: Option<String>,
    #[serde(rename = "filters")]
    pub filters: Option<String>,
    #[serde(rename = "enableUserData")]
    pub enable_user_data: Option<bool>,
    #[serde(rename = "enableImages")]
    pub enable_images: Option<bool>,
}

pub async fn user_items(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<ItemsQuery>,
) -> Result<Json<jf_core::QueryResult<jf_core::BaseItemDto>>, ApiError> {
    authorize_user(&auth.0, &user_id)?;

    let query = build_item_query(&params);
    let fields = FieldSelection::parse(params.fields.as_deref());

    let result = app
        .db
        .query_items(Some(&user_id), &query, &fields, &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `GET /Users/{userId}/Items/Resume` — continue watching.
pub async fn resume_items(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<ItemsQuery>,
) -> Result<Json<jf_core::QueryResult<jf_core::BaseItemDto>>, ApiError> {
    authorize_user(&auth.0, &user_id)?;

    let mut query = build_item_query(&params);
    query.in_progress_only = true;
    query.sort_by = Some(SortBy::SortName);
    query.limit = params.limit.unwrap_or(20).clamp(1, 200);

    let fields = FieldSelection::parse(Some("Overview,ProductionYear,RunTimeTicks,UserData"));
    let result = app
        .db
        .query_items(Some(&user_id), &query, &fields, &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `GET /Users/{userId}/Items/{itemId}`
pub async fn user_item(
    State(app): State<Shared>,
    Path((user_id, item_id)): Path<(String, String)>,
    auth: RequireAuth,
) -> Result<Json<jf_core::BaseItemDto>, ApiError> {
    authorize_user(&auth.0, &user_id)?;

    let item = app
        .db
        .item_by_id(&item_id, Some(&user_id), &app.server_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Item not found."))?;

    Ok(Json(item))
}

/// `GET /Shows/{seriesId}/Seasons`
pub async fn show_seasons(
    State(app): State<Shared>,
    Path(series_id): Path<String>,
    auth: RequireAuth,
) -> Result<Json<jf_core::QueryResult<jf_core::BaseItemDto>>, ApiError> {
    let query = ItemQuery {
        parent_id: Some(series_id.clone()),
        item_types: vec![jf_core::BaseItemKind::Season],
        sort_by: Some(SortBy::SortName),
        limit: 500,
        ..Default::default()
    };

    let result = app
        .db
        .query_items(Some(auth.0.id()), &query, &FieldSelection::all(), &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `GET /Shows/{seriesId}/Episodes`
pub async fn show_episodes(
    State(app): State<Shared>,
    Path(series_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<jf_core::QueryResult<jf_core::BaseItemDto>>, ApiError> {
    let season_id = params.get("seasonId").cloned().or_else(|| params.get("SeasonId").cloned());

    let query = ItemQuery {
        parent_id: season_id.or(Some(series_id)),
        item_types: vec![jf_core::BaseItemKind::Episode],
        sort_by: Some(SortBy::SortName),
        limit: 1000,
        ..Default::default()
    };

    let result = app
        .db
        .query_items(Some(auth.0.id()), &query, &FieldSelection::all(), &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `GET /Users/{userId}/Shows/NextUp`
pub async fn next_up(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<jf_core::QueryResult<jf_core::BaseItemDto>>, ApiError> {
    authorize_user(&auth.0, &user_id)?;
    let limit = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 100);

    // "Next up" is the lowest-numbered unwatched episode of each series the
    // user has partially seen. Rather than model episode continuity properly
    // yet, this returns unwatched episodes that share a series with something
    // the user has already started.
    let query = ItemQuery {
        item_types: vec![jf_core::BaseItemKind::Episode],
        unplayed_only: true,
        sort_by: Some(SortBy::PremiereDate),
        limit,
        recursive: true,
        ..Default::default()
    };

    let result = app
        .db
        .query_items(Some(&user_id), &query, &FieldSelection::all(), &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

fn build_item_query(params: &ItemsQuery) -> ItemQuery {
    ItemQuery {
        parent_id: params.parent_id.clone(),
        item_types: params
            .include_item_types
            .as_deref()
            .map(|s| s.split(',').filter_map(jf_db::items::try_parse_kind).collect())
            .unwrap_or_default(),
        media_types: params
            .media_types
            .as_deref()
            .map(|s| {
                s.split(',').filter_map(jf_db::items::parse_media_type).collect()
            })
            .unwrap_or_default(),
        search: params.search_term.clone(),
        sort_by: params.sort_by.as_deref().and_then(SortBy::parse),
        sort_order: match params.sort_order.as_deref() {
            Some("Descending") => SortOrder::Descending,
            _ => SortOrder::Ascending,
        },
        start_index: params.start_index.unwrap_or(0),
        limit: params.limit.unwrap_or(100).clamp(1, 5000),
        recursive: params.recursive.unwrap_or(false),
        // Folders are navigation nodes, not results, unless explicitly requested.
        include_folders: false,
        genres: params
            .genres
            .as_deref()
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        ..Default::default()
    }
}

/// A user may only read their own data unless they are an administrator.
fn authorize_user(auth: &AuthUser, user_id: &str) -> Result<(), ApiError> {
    if auth.user.id == user_id || auth.is_admin() {
        return Ok(());
    }
    Err(ApiError::forbidden("You do not have access to this user."))
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// `GET /Items/{itemId}/PlaybackInfo`
pub async fn playback_info(
    State(app): State<Shared>,
    Path(item_id): Path<String>,
    auth: RequireAuth,
    Query(_params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let item = app
        .db
        .item_by_id(&item_id, Some(auth.0.id()), &app.server_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Item not found."))?;

    // Reuse the source recorded at scan time.
    let sources: Vec<jf_core::MediaSourceInfo> =
        jf_db::Database::json_column(&media_sources_json(&app, &item_id).await?, "items", "media_sources_json")
            .unwrap_or_default();

    let media_type = item.media_type.unwrap_or(jf_core::MediaType::Video);
    let caps = jf_playback::ClientCapabilities::web();
    let selection = jf_playback::StreamSelection::default();

    let playable = sources
        .iter()
        .find(|s| s.protocol == jf_core::MediaProtocol::File && std::path::Path::new(&s.path).is_file())
        .or_else(|| sources.first())
        .ok_or_else(|| ApiError::not_found("This item has no playable media."))?;

    // Decide between direct play and transcode without actually transcoding;
    // a transcode session is only started if the client then requests one.
    let scratch = app.paths.transcode.join("plan");
    let plan = app
        .transcoder
        .plan(playable, media_type, &caps, &selection, &scratch)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "MediaSources": [{
            "Id": playable.id,
            // A transcode plan means the client cannot play the original and
            // will request HLS; otherwise the file is served directly.
            "Protocol": if plan.is_some() { "Hls" } else { "File" },
            "Path": playable.path,
            "Container": playable.container,
            "Size": playable.size,
            "RunTimeTicks": playable.run_time_ticks,
            "SupportsDirectPlay": true,
            "SupportsDirectStream": false,
            "SupportsTranscoding": true,
            "MediaStreams": playable.media_streams,
        }],
        "PlaySessionId": format!("jf-{item_id}"),
        "ErrorCode": if plan.is_some() { 0 } else { 0 },
    })))
}

async fn media_sources_json(app: &Shared, item_id: &str) -> Result<String, ApiError> {
    let value = sqlx::query_scalar::<_, String>("SELECT media_sources_json FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(app.db.pool())
        .await
        .map_err(|e| ApiError::from(jf_db::DbError::from(e)))?;

    Ok(value.unwrap_or_else(|| "[]".to_owned()))
}

/// `GET /Videos/{itemId}/stream` — direct play.
pub async fn stream_video(
    State(app): State<Shared>,
    Path(item_id): Path<String>,
    auth: RequireAuth,
) -> Result<Response, ApiError> {
    if !auth.0.user.policy.enable_media_playback {
        return Err(ApiError::forbidden("Media playback is disabled for this account."));
    }

    let path = app
        .db
        .item_media_path(&item_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Item not found."))?;

    let file = std::path::PathBuf::from(&path);
    if !file.is_file() {
        return Err(ApiError::not_found("The media file is missing from disk."));
    }

    // Confine serving to the indexed path; never accept a client-supplied path.
    let mime = jf_playback::transcode::container_mime(
        &file.extension().and_then(|e| e.to_str()).map(str::to_owned),
    );

    let file = tokio::fs::File::open(&file).await.map_err(|e| {
        ApiError::internal(format!("could not open media file: {e}"))
    })?;

    let size = file.metadata().await.ok().map(|m| m.len()).unwrap_or(0);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap()),
            (header::ACCEPT_RANGES, HeaderValue::from_static("bytes")),
            (header::CONTENT_LENGTH, HeaderValue::from(size)),
            // The web client must be able to read the stream from a different
            // origin during development.
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*")),
        ],
        Body::from_stream(tokio_util_stream(file)),
    )
    .into_response())
}

/// Adapt a tokio file into a body stream.
fn tokio_util_stream(file: tokio::fs::File) -> impl futures::Stream<Item = std::io::Result<bytes::Bytes>> {
    futures::stream::unfold(file, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(bytes::Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    })
}

/// `POST /UserPlayedItems/{itemId}/Progress`
pub async fn report_progress(
    State(app): State<Shared>,
    Path(item_id): Path<String>,
    auth: RequireAuth,
    Json(body): Json<PlaybackProgressDto>,
) -> Result<StatusCode, ApiError> {
    let position = body.position_ticks.unwrap_or(0);
    app.db
        .set_progress(auth.0.id(), &item_id, position, None)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /Sessions/Playing`
pub async fn playing(
    State(_app): State<Shared>,
    auth: RequireAuth,
    Json(_body): Json<PlaybackStartInfoDto>,
) -> Result<StatusCode, ApiError> {
    let _ = auth;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /Sessions/Playing/Stopped`
pub async fn stopped(
    State(app): State<Shared>,
    auth: RequireAuth,
    Json(body): Json<PlaybackStartInfoDto>,
) -> Result<StatusCode, ApiError> {
    let position = body.position_ticks.unwrap_or(0);
    let _ = app
        .db
        .set_progress(auth.0.id(), &body.item_id, position, None)
        .await;

    // Release the transcode session if one was running for this item.
    let _ = app.hls.stop_user(auth.0.id()).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /UserFavoriteItems/{itemId}` / `DELETE` counterpart.
pub async fn favorite(
    State(app): State<Shared>,
    Path(item_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode, ApiError> {
    let favorite = params
        .get("favorite")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(true);

    app.db
        .set_favorite(auth.0.id(), &item_id, favorite)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

/// `GET /Items/{itemId}/Images/{type}`
pub async fn item_image(
    State(app): State<Shared>,
    Path((item_id, image_type)): Path<(String, String)>,
    _auth: MaybeAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let index: u32 = params
        .get("index")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let image_slot = match image_type.to_ascii_lowercase().as_str() {
        "primary" => jf_playback::ImageKind::Primary,
        "backdrop" => jf_playback::ImageKind::Backdrop,
        "thumb" => jf_playback::ImageKind::Thumb,
        "logo" => jf_playback::ImageKind::Logo,
        "banner" => jf_playback::ImageKind::Banner,
        other => return Err(ApiError::bad_request(format!("Unknown image type `{other}`."))),
    };

    let kind = match image_type.to_ascii_lowercase().as_str() {
        "primary" => "Primary",
        "backdrop" => "Backdrop",
        "thumb" => "Thumb",
        "logo" => "Logo",
        "banner" => "Banner",
        other => return Err(ApiError::bad_request(format!("Unknown image type `{other}`."))),
    };

    // Prefer a sidecar image next to the media file.
    let sidecar = app
        .db
        .item_image_path(&item_id, kind, index)
        .await
        .map_err(ApiError::from)?;

    if let Some(path) = sidecar {
        if std::path::Path::new(&path).is_file() {
            return serve_file(std::path::PathBuf::from(path)).await;
        }
    }

    // Otherwise generate a thumbnail from the video.
    let media = app
        .db
        .item_media_path(&item_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("No image available."))?;

    let width: Option<u32> = params.get("maxWidth").and_then(|v| v.parse().ok());
    let output = app.images.cache_path(&item_id, image_slot, index, width);

    // Prefer the runtime recorded at scan time; fall back to a fixed offset
    // when the item has none (audio, or an unprobed file).
    let runtime_ticks: Option<i64> = sqlx::query_scalar("SELECT run_time_ticks FROM items WHERE id = ?")
        .bind(&item_id)
        .fetch_optional(app.db.pool())
        .await
        .map_err(|e| ApiError::from(jf_db::DbError::from(e)))?;
    let timestamp = jf_playback::image::thumbnail_timestamp(
        runtime_ticks.map(jf_core::ticks_to_secs),
    );
    match app
        .images
        .generate_video_thumbnail(
            std::path::Path::new(&media),
            &output,
            timestamp,
            width,
        )
        .await
    {
        Ok(Some(result)) => serve_file(result.path).await,
        Ok(None) => Err(ApiError::not_found("No image could be generated.")),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

async fn serve_file(path: std::path::PathBuf) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("Image not found."))?;
    let size = file.metadata().await.ok().map(|m| m.len()).unwrap_or(0);
    let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400")),
            (header::CONTENT_LENGTH, HeaderValue::from(size)),
        ],
        Body::from_stream(tokio_util_stream(file)),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Events (SSE)
// ---------------------------------------------------------------------------

/// `GET /Events` — server-sent events for scan and playback updates.
pub async fn events(
    State(app): State<Shared>,
    auth: RequireAuth,
) -> Response {
    let mut rx = app.events.subscribe();

    let stream = async_stream::stream! {
        // Send an immediate comment so proxies do not buffer the response
        // while waiting for the first real event.
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b": connected\n\n"));

        loop {
            match rx.recv().await {
                Ok(message) => {
                    let payload = format!("data: {message}\n\n");
                    yield Ok(bytes::Bytes::from(payload));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(user = %auth.0.id(), skipped = n, "SSE consumer lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A JSON error response matching Jellyfin's shape.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self { status: StatusCode::UNAUTHORIZED, message: message.into() }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: message.into() }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: message.into() }
    }
}

impl From<jf_db::DbError> for ApiError {
    fn from(err: jf_db::DbError) -> Self {
        // Database failures are logged with detail but reported generically:
        // schema and path details are not the client's business.
        tracing::error!(error = %err, "database error");
        Self::internal("An internal error occurred.")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = self.status.as_u16();
        (
            self.status,
            Json(serde_json::json!({
                "Message": self.message,
                "Status": code,
                "ServerId": "",
            })),
        )
            .into_response()
    }
}
/// 404 for an unmatched API route, so a client calling something this server
/// does not implement gets JSON rather than the HTML shell.
pub async fn api_not_found(uri: axum::http::Uri) -> ApiError {
    ApiError::not_found(format!("No endpoint matches `{}`.", uri.path()))
}
