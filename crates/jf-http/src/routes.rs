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
use serde::{Deserialize, Serialize};
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
    #[serde(rename = "parentId", alias = "ParentId")]
    pub parent_id: Option<String>,
    #[serde(rename = "includeItemTypes", alias = "IncludeItemTypes")]
    pub include_item_types: Option<String>,
    #[serde(rename = "mediaTypes", alias = "MediaTypes")]
    pub media_types: Option<String>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    pub search_term: Option<String>,
    #[serde(rename = "sortBy", alias = "SortBy")]
    pub sort_by: Option<String>,
    #[serde(rename = "sortOrder", alias = "SortOrder")]
    pub sort_order: Option<String>,
    #[serde(rename = "startIndex", alias = "StartIndex")]
    pub start_index: Option<usize>,
    #[serde(rename = "limit", alias = "Limit")]
    pub limit: Option<usize>,
    #[serde(rename = "recursive", alias = "Recursive")]
    pub recursive: Option<bool>,
    /// Comma-separated genre names.
    pub genres: Option<String>,
    #[serde(rename = "fields", alias = "Fields")]
    pub fields: Option<String>,
    #[serde(rename = "filters", alias = "Filters")]
    pub filters: Option<String>,
    #[serde(rename = "enableUserData", alias = "EnableUserData")]
    pub enable_user_data: Option<bool>,
    #[serde(rename = "enableImages", alias = "EnableImages")]
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

/// Either a single item or a page of items.
///
/// The two are distinguished by shape rather than by route: Jellyfin serves
/// both from `/Users/{userId}/Items/{id}`, and registering that path twice
/// panics the router at startup, so one handler must decide which to return.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ItemOrListing {
    Item(jf_core::BaseItemDto),
    Listing(jf_core::QueryResult<jf_core::BaseItemDto>),
}

/// `GET /Users/{userId}/Items/{itemId}`
///
/// Returns the item itself, or — when the id names a folder — that folder's
/// children. The client relies on this: it lists a library by requesting the
/// library's own id, so without the folder branch every library appeared empty.
pub async fn user_item(
    State(app): State<Shared>,
    Path((user_id, item_id)): Path<(String, String)>,
    auth: RequireAuth,
    Query(params): Query<ItemsQuery>,
) -> Result<Json<ItemOrListing>, ApiError> {
    authorize_user(&auth.0, &user_id)?;

    let item = app
        .db
        .item_by_id(&item_id, Some(&user_id), &app.server_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Item not found."))?;

    if !item.is_folder {
        return Ok(Json(ItemOrListing::Item(item)));
    }

    // Listing a folder. `recursive` defaults to true because Jellyfin clients
    // send it explicitly but hand-rolled callers do not, and a one-level
    // listing of a flat movie library shows nothing.
    let fields = FieldSelection::parse(params.fields.as_deref());
    let query = build_item_query(&ItemsQuery {
        parent_id: Some(item_id),
        recursive: Some(params.recursive.unwrap_or(true)),
        ..params
    });

    let listing = app
        .db
        .query_items(Some(&user_id), &query, &fields, &app.server_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(ItemOrListing::Listing(listing)))
}

#[cfg(test)]
mod query_param_tests {
    use super::{build_item_query, ItemsQuery};

    fn parse(q: &str) -> ItemsQuery {
        serde_urlencoded::from_str::<ItemsQuery>(q).expect("query should deserialize")
    }

    #[test]
    fn accepts_jellyfin_pascal_case_and_our_camel_case() {
        // Jellyfin's own clients send PascalCase; our web client sends a mix.
        // Both must work, because a silently dropped `Recursive` turns a
        // library listing into a single-level list that looks empty.
        for term in ["Recursive=true", "recursive=true"] {
            assert_eq!(parse(term).recursive, Some(true), "{term}");
        }
        for limit in ["Limit=5", "limit=5"] {
            assert_eq!(parse(limit).limit, Some(5), "{limit}");
        }
        for search in ["SearchTerm=dune", "searchTerm=dune"] {
            assert_eq!(parse(search).search_term.as_deref(), Some("dune"), "{search}");
        }
        for sort in ["SortBy=DateCreated", "sortBy=DateCreated"] {
            assert_eq!(parse(sort).sort_by.as_deref(), Some("DateCreated"), "{sort}");
        }
        for fields in ["Fields=Overview", "fields=Overview"] {
            assert_eq!(parse(fields).fields.as_deref(), Some("Overview"), "{fields}");
        }
    }

    #[test]
    fn a_limit_is_bounded_so_a_client_cannot_ask_for_everything() {
        // `build_item_query` clamps; assert the raw value is optional and that
        // the clamp exists, since an unbounded limit is a denial-of-service.
        assert_eq!(parse("Limit=999999").limit, Some(999_999));
        assert_eq!(build_item_query(&parse("Limit=999999")).limit, 5000);
    }
}

#[cfg(test)]
mod item_or_listing_tests {
    use super::*;

    fn folder() -> jf_core::BaseItemDto {
        // Built from the wire form on purpose: the fixture is then exactly what
        // the server would send, so the test cannot drift from the real shape.
        serde_json::from_value(serde_json::json!({
            "Name": "Films",
            "ServerId": "server-1",
            "Id": "lib-1",
            "Type": "CollectionFolder",
            "IsFolder": true,
            "Path": "/media/movies",
            "CanDelete": false,
            "CanDownload": true,
            "IsLiveStream": false,
        }))
        .expect("folder fixture should deserialize")
    }

    fn movie() -> jf_core::BaseItemDto {
        serde_json::from_value(serde_json::json!({
            "Name": "Alien",
            "ServerId": "server-1",
            "Id": "m1",
            "Type": "Movie",
            "IsFolder": false,
            "ProductionYear": 1979,
            "Path": "/media/movies/Alien (1979).mkv",
            "CanDelete": true,
            "CanDownload": true,
            "IsLiveStream": false,
        }))
        .expect("movie fixture should deserialize")
    }

    #[test]
    fn a_folder_serializes_as_a_listing_and_a_movie_as_an_item() {
        // The distinction is the whole point of this endpoint: the client tells
        // them apart by whether an `Items` key is present.
        let listing = serde_json::to_value(ItemOrListing::Listing(jf_core::QueryResult {
            items: vec![movie()],
            total_record_count: 1,
            start_index: 0,
        }))
        .unwrap();

        assert!(listing.get("Items").is_some(), "listing lost its Items key");
        assert_eq!(listing["TotalRecordCount"], 1);

        let item = serde_json::to_value(ItemOrListing::Item(movie())).unwrap();
        assert!(item.get("Items").is_none(), "a movie must not look like a listing");
        assert_eq!(item["Name"], "Alien");
    }

    #[test]
    fn a_folder_carries_the_fields_the_client_navigates_by() {
        let json = serde_json::to_value(ItemOrListing::Item(folder())).unwrap();
        assert_eq!(json["Id"], "lib-1");
        assert_eq!(json["IsFolder"], true);
    }
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

    // A series counts as "started" once the user has marked any of its
    // episodes played. Only those series have a meaningful "next up".
    // Series with no watched episode have nothing to be "next" to.
    let watched_series: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT i.series_id
         FROM items i
         JOIN user_data ud ON ud.item_id = i.id
         WHERE ud.user_id = ?
           AND i.type = 'Episode'
           AND i.series_id IS NOT NULL
           AND (ud.played = 1 OR ud.play_count > 0)",
    )
    .bind(auth.0.id())
    .fetch_all(app.db.pool())
    .await
    .map_err(|e| ApiError::from(jf_db::DbError::from(e)))?;

    if watched_series.is_empty() {
        return Ok(Json(jf_core::QueryResult {
            items: Vec::new(),
            total_record_count: 0,
            start_index: 0,
        }));
    }

    // Candidates: unwatched episodes that belong to a started series.
    // Sorted so that within a series we pick the lowest (season, episode)
    // first. `publish` order is fine for ordering different series because
    // the final selection respects `limit` only.
    let candidate_query = ItemQuery {
        item_types: vec![jf_core::BaseItemKind::Episode],
        unplayed_only: true,
        sort_by: Some(SortBy::SortName),
        limit: 500,
        recursive: true,
        ..Default::default()
    };

    let all = app
        .db
        .query_items(Some(&user_id), &candidate_query, &FieldSelection::all(), &app.server_id)
        .await
        .map_err(ApiError::from)?;

    // One "next up" per series, chosen by (season, episode).
    let mut chosen: Vec<jf_core::BaseItemDto> = Vec::new();
    let mut seen_series: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Iterate episodes in series/season/episode order.
    let mut ordered: Vec<_> = all.items.into_iter().collect();
    ordered.sort_by(|a, b| {
        use std::cmp::Ordering;
        a.series_id.clone().map(String::from).unwrap_or_default().as_str().cmp(
            &b.series_id.clone().map(String::from).unwrap_or_default().as_str())
            .then_with(|| a.parent_index_number.unwrap_or(0).cmp(&b.parent_index_number.unwrap_or(0)))
            .then_with(|| a.index_number.unwrap_or(0).cmp(&b.index_number.unwrap_or(0)))
    });

    // Move episodes out of `ordered` while tracking the owning series as a
    // plain `String`, so the borrow of `episode.series_id` ends before the
    // list is consumed.
    let mut iter = ordered.into_iter();
    while let Some(episode) = iter.next() {
        let Some(series_id) = episode.series_id.clone() else { continue };
        if !watched_series.contains(&series_id) {
            continue;
        }
        if seen_series.contains(&series_id) {
            // Close the closure over a clone so `seen_series` and `episode`
            // never alias each other.
            let _ = ();
            continue;
        }

        seen_series.insert(series_id);
        chosen.push(episode);
        if chosen.len() >= limit {
            break;
        }
    }

    let total = chosen.len();
    Ok(Json(jf_core::QueryResult {
        items: chosen,
        total_record_count: total,
        start_index: 0,
    }))
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
    Query(params): Query<PlaybackQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (item, playable) = playable_source(&app, &item_id, &auth).await?;

    // Decide between direct play and transcode. This used to report `Hls`
    // whenever a plan existed while the stream endpoint went on serving the
    // original file, so a client that believed the `Protocol` got a file it
    // could not parse.
    let media_type = item.media_type.unwrap_or(jf_core::MediaType::Video);
    let caps = caps_from_query(&params);
    let selection = selection_from_query(&params);
    let scratch = app.paths.transcode.join("plan");

    let plan = app
        .transcoder
        .plan(&playable, media_type, &caps, &selection, &scratch)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let play_session_id = format!("jf-{item_id}");

    let mut source = serde_json::json!({
        "Id": playable.id,
        "Path": playable.path,
        "Container": playable.container,
        "Size": playable.size,
        "RunTimeTicks": playable.run_time_ticks,
        "MediaStreams": playable.media_streams,
        "SupportsDirectPlay": plan.is_none(),
        "SupportsDirectStream": false,
        "SupportsTranscoding": true,
        "Protocol": if plan.is_some() { "Hls" } else { "File" },
    });

    // URLs are relative rather than absolute. Building them from the `Host`
    // header would mean trusting a client-supplied value, and the browser
    // client — the only consumer — resolves them correctly either way.
    if let Some(ref plan) = plan {
        // The HLS endpoint the client can actually fetch. The session is
        // created lazily in `hls_master`, so browsing a library does not leave
        // ffmpeg processes running.
        source["TranscodingUrl"] = serde_json::json!(format!(
            "/Videos/{item_id}/master.m3u8?static=true{}",
            encode_playback_query(&params)
        ));
        source["TranscodingSubProtocol"] = serde_json::json!("hls");
        source["TranscodingContainer"] = serde_json::json!("ts");
        source["DefaultAudioStreamIndex"] = serde_json::json!(plan.audio_stream_index);
        source["NeedsVideoTranscode"] = serde_json::json!(plan.needs_video_transcode);
        source["NeedsAudioTranscode"] = serde_json::json!(plan.needs_audio_transcode);
    } else {
        source["DirectStreamUrl"] = serde_json::json!(format!(
            "/Videos/{item_id}/stream?static=true{}",
            encode_playback_query(&params)
        ));
    }

    Ok(Json(serde_json::json!({
        "MediaSources": [source],
        "PlaySessionId": play_session_id,
        "ErrorCode": 0,
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

// ---------------------------------------------------------------------------
// Adaptive streaming (HLS)
// ---------------------------------------------------------------------------

/// Capability and quality hints a client sends with a playback request.
///
/// Mirrors the subset of Jellyfin's parameters that changes the decision:
/// anything absent means "no constraint from the client".
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackQuery {
    #[serde(rename = "MaxStreamingBitrate")]
    pub max_streaming_bitrate: Option<i64>,
    #[serde(rename = "MaxWidth")]
    pub max_width: Option<i32>,
    #[serde(rename = "MaxHeight")]
    pub max_height: Option<i32>,
    #[serde(rename = "VideoCodec")]
    pub video_codec: Option<String>,
    #[serde(rename = "AudioCodec")]
    pub audio_codec: Option<String>,
    #[serde(rename = "TranscodingProtocol")]
    pub transcoding_protocol: Option<String>,
    #[serde(rename = "EnableAutoStreamCopyBitrates")]
    pub auto_stream: Option<bool>,
}

/// Build the stream selection a client is asking for.
fn selection_from_query(params: &PlaybackQuery) -> jf_playback::StreamSelection {
    jf_playback::StreamSelection {
        max_bitrate: params.max_streaming_bitrate,
        max_width: params.max_width,
        max_height: params.max_height,
        video_codec: params.video_codec.clone(),
        audio_codec: params.audio_codec.clone(),
        ..Default::default()
    }
}

/// Build the capabilities implied by a client's request.
///
/// A client that names a codec is stating what it can decode, so the list is
/// narrowed to that. Without this, a client asking for `h264` would still be
/// offered direct play of an `hevc` file and would fail at playback time.
fn caps_from_query(params: &PlaybackQuery) -> jf_playback::ClientCapabilities {
    let mut caps = jf_playback::ClientCapabilities::web();

    if let Some(codec) = params.video_codec.as_deref().filter(|c| !c.is_empty()) {
        caps.video_codecs = vec![codec.to_ascii_lowercase()];
    }
    if let Some(codec) = params.audio_codec.as_deref().filter(|c| !c.is_empty()) {
        caps.audio_codecs = vec![codec.to_ascii_lowercase()];
    }
    if let Some(max) = params.max_height {
        caps.max_height = Some(max);
    }

    caps
}

/// Re-encode a client's playback parameters so they survive the round trip
/// into the transcode URL.
///
/// Without this the client would ask for `master.m3u8`, the server would plan
/// again with default (unconstrained) capabilities, and every request would
/// transcode at full quality regardless of what was asked for.
fn encode_playback_query(params: &PlaybackQuery) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(v) = params.max_streaming_bitrate {
        parts.push(format!("MaxStreamingBitrate={v}"));
    }
    if let Some(v) = params.max_width {
        parts.push(format!("MaxWidth={v}"));
    }
    if let Some(v) = params.max_height {
        parts.push(format!("MaxHeight={v}"));
    }
    if let Some(v) = params.video_codec.as_deref().filter(|c| !c.is_empty()) {
        parts.push(format!("VideoCodec={}", urlencode(v)));
    }
    if let Some(v) = params.audio_codec.as_deref().filter(|c| !c.is_empty()) {
        parts.push(format!("AudioCodec={}", urlencode(v)));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("&{}", parts.join("&"))
    }
}

/// Percent-encode a query value.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Resolve an item to a playable source, honouring playback policy.
async fn playable_source(
    app: &Shared,
    item_id: &str,
    auth: &RequireAuth,
) -> Result<(jf_core::BaseItemDto, jf_core::MediaSourceInfo), ApiError> {
    if !auth.0.user.policy.enable_media_playback {
        return Err(ApiError::forbidden("Media playback is disabled for this account."));
    }

    let item = app
        .db
        .item_by_id(item_id, Some(auth.0.id()), &app.server_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Item not found."))?;

    let sources: Vec<jf_core::MediaSourceInfo> =
        jf_db::Database::json_column(&media_sources_json(app, item_id).await?, "items", "media_sources_json")
            .unwrap_or_default();

    let playable = sources
        .iter()
        .find(|s| s.protocol == jf_core::MediaProtocol::File && std::path::Path::new(&s.path).is_file())
        .or_else(|| sources.first())
        .cloned()
        .ok_or_else(|| ApiError::not_found("This item has no playable media."))?;

    Ok((item, playable))
}

/// `GET /Videos/{itemId}/master.m3u8` — the HLS playlist.
///
/// Starting the session here rather than in `PlaybackInfo` matches Jellyfin:
/// the client only fetches the playlist once it has actually committed to
/// playing, so a user who browses and leaves does not leave an ffmpeg process
/// running.
pub async fn hls_master(
    State(app): State<Shared>,
    Path(item_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<PlaybackQuery>,
) -> Result<Response, ApiError> {
    let (item, playable) = playable_source(&app, &item_id, &auth).await?;
    let media_type = item.media_type.unwrap_or(jf_core::MediaType::Video);

    let caps = caps_from_query(&params);
    let selection = selection_from_query(&params);
    let scratch = app.paths.transcode.join("plan");

    let Some(plan) = app
        .transcoder
        .plan(&playable, media_type, &caps, &selection, &scratch)
        .map_err(|e| ApiError::internal(e.to_string()))?
    else {
        // The client can play this file as-is, so hand back the original rather
        // than an empty playlist. `<video>` follows this URL directly.
        return stream_video(State(app), Path(item_id), auth).await;
    };

    let play_session_id = format!("jf-{item_id}");
    // One session per (item, user): a second request replaces the first, so
    // re-opening a video does not accumulate ffmpeg processes.
    let session_id = format!("{}-{}", item_id, auth.0.id());
    let session_id_for_segments = session_id.clone();

    // Replace any previous session for this item before starting a new one.
    let _ = app.hls.stop(&session_id).await;

    let session = app
        .hls
        .start(
            session_id,
            play_session_id,
            item_id.clone(),
            auth.0.id().to_owned(),
            plan,
        )
        .await
        .map_err(|e| ApiError::internal(format!("could not start transcode: {e}")))?;

    // ffmpeg writes the playlist asynchronously; wait briefly for the first
    // segment rather than returning 404 and leaving the player to give up.
    let playlist_path = session
        .wait_for_playlist(std::time::Duration::from_secs(10))
        .await
        .map_err(|e| ApiError::internal(format!("transcode did not start: {e}")))?;

    let body_text = std::fs::read_to_string(&playlist_path).map_err(|e| {
        ApiError::internal(format!("could not read playlist: {e}"))
    })?;

    // ffmpeg writes relative segment names; the browser would resolve them
    // against master.m3u8 and hit a route that does not exist. Rewrite to
    // absolute segment URLs.
    let mut out = String::with_capacity(body_text.len());
    for line in body_text.lines() {
        let name = line.rsplit('/').next().unwrap_or(line);
        if line.ends_with(".ts") || line.ends_with(".mp4") || line.ends_with(".m4s") {
            out.push_str("/transcode/");
            out.push_str(&session_id_for_segments);
            out.push('/');
            out.push_str(name);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/vnd.apple.mpegurl")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-store")),
        ],
        out,
    )
        .into_response())
}

/// `GET /transcode/{sessionId}/{segment}` — one HLS segment.
pub async fn hls_segment(
    State(app): State<Shared>,
    Path((session_id, segment)): Path<(String, String)>,
    auth: RequireAuth,
) -> Result<Response, ApiError> {
    let path = app
        .hls
        .segment_path(&session_id, &segment)
        .await
        .map_err(|e| ApiError::not_found(format!("segment not available: {e}")))?;

    // A session belongs to the user who started it. Checking here stops one
    // account pulling another's in-flight transcode off a guessed id.
    let session = app
        .hls
        .get(&session_id)
        .await
        .ok_or_else(|| ApiError::not_found("No such session."))?;
    if session.user_id != auth.0.id() {
        return Err(ApiError::forbidden("That session belongs to another user."));
    }

    let mime = if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if segment.ends_with(".mp4") || segment.ends_with(".m4s") {
        "video/iso.segment"
    } else {
        "video/mp2t"
    };

    let body = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::not_found(format!("segment unavailable: {e}")))?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap()),
            (header::CACHE_CONTROL, HeaderValue::from_static("max-age=31536000")),
        ],
        body,
    )
        .into_response())
}
