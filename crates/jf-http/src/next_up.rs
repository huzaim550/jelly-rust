//! `/Users/{userId}/Shows/NextUp`.
//!
//! Jellyfin's "Next Up" row is episode continuity, not a sort of every
//! unwatched file: for each series the user has started, it offers the lowest
//! unwatched episode in (season, episode) order. Two rules make it useful:
//!
//! * only *started* series appear — a series with nothing played has no
//!   "next" episode, only a catalogue;
//! * only one episode per series appears, however far behind the user is.

use crate::routes::authorize_user;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::auth::RequireAuth;
use crate::routes::ApiError;
use jf_core::dto::BaseItemDto;
use jf_db::items::{FieldSelection, ItemQuery, SortBy};

type Shared = Arc<AppState>;

/// `GET /Users/{userId}/Shows/NextUp`
pub async fn next_up(
    State(app): State<Shared>,
    Path(user_id): Path<String>,
    auth: RequireAuth,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<jf_core::QueryResult<BaseItemDto>>, ApiError> {
    authorize_user(&auth.0, &user_id)?;
    let limit = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 100);

    let watched = watched_series(&app, auth.0.id()).await?;

    if watched.is_empty() {
        return Ok(Json(jf_core::QueryResult {
            items: Vec::new(),
            total_record_count: 0,
            start_index: 0,
        }));
    }

    // Candidates: every unwatched episode, ranked per series below. The limit
    // is deliberately generous so a user who is far behind on many shows still
    // gets one entry for each rather than having whole series dropped.
    let query = ItemQuery {
        item_types: vec![jf_core::BaseItemKind::Episode],
        unplayed_only: true,
        sort_by: Some(SortBy::SortName),
        limit: 500,
        recursive: true,
        ..Default::default()
    };

    let all = app
        .db
        .query_items(
            Some(&user_id),
            &query,
            &FieldSelection::all(),
            &app.server_id,
        )
        .await
        .map_err(ApiError::from)?;

    let chosen = select_next_up(all.items, &watched, limit as usize);
    let total = chosen.len();

    Ok(Json(jf_core::QueryResult {
        items: chosen,
        total_record_count: total,
        start_index: 0,
    }))
}

/// Series with at least one finished episode for this user.
async fn watched_series(app: &AppState, user_id: &str) -> Result<HashSet<String>, ApiError> {
    let ids = app
        .db
        .watched_series_for_user(user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(ids.into_iter().collect())
}

/// Pick the lowest unwatched episode of each started series.
///
/// Takes episodes in any order: the ranking by (season, episode) happens here,
/// so the caller cannot accidentally depend on the database's sort order.
pub fn select_next_up(
    mut episodes: Vec<BaseItemDto>,
    watched: &HashSet<String>,
    limit: usize,
) -> Vec<BaseItemDto> {
    episodes.sort_by(|a, b| {
        a.series_id
            .as_deref()
            .unwrap_or_default()
            .cmp(b.series_id.as_deref().unwrap_or_default())
            .then_with(|| a.parent_index_number.unwrap_or(0).cmp(&b.parent_index_number.unwrap_or(0)))
            .then_with(|| a.index_number.unwrap_or(0).cmp(&b.index_number.unwrap_or(0)))
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut seen: HashSet<String> = HashSet::new();
    let mut chosen = Vec::new();

    for episode in episodes {
        let Some(series_id) = episode.series_id.clone() else { continue };
        if !watched.contains(&series_id) {
            continue;
        }
        // A series only offers one episode; the first one wins, and sorting
        // put the lowest (season, episode) first.
        if !seen.insert(series_id) {
            continue;
        }
        chosen.push(episode);
        if chosen.len() >= limit {
            break;
        }
    }

    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(series: &str, season: i32, number: i32, name: &str) -> BaseItemDto {
        // `BaseItemDto` has no `Default` and no serde defaults, so a fixture
        // has to spell out every non-optional field.
        serde_json::from_value(serde_json::json!({
            "Id": format!("{series}-{season}-{number}"),
            "Name": name,
            "Type": "Episode",
            "ServerId": "server",
            "IsFolder": false,
            "Path": format!("/media/{series}/S{season:02}E{number:02}.mkv"),
            "Genres": [],
            "Taglines": [],
            "People": [],
            "Tags": [],
            "Artists": [],
            "BackdropImageTags": [],
            "CanDelete": true,
            "CanDownload": true,
            "IsLiveStream": false,
            "ExternalUrls": [],
            "SeriesId": series,
            "ParentIndexNumber": season,
            "IndexNumber": number,
        }))
        .expect("item dto")
    }

    fn series(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lowest_unwatched_episode_is_chosen_per_series() {
        // Deliberately shuffled and unfiltered: the ranking is this
        // function's job, not the database's.
        let episodes = vec![
            episode("a", 1, 3, "Third"),
            episode("a", 1, 1, "First"),
            episode("a", 1, 2, "Second"),
        ];
        let watched = series(&["a"]);

        let chosen = select_next_up(episodes, &watched, 20);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].name, "First");
    }

    #[test]
    fn unstarted_series_are_excluded() {
        let episodes = vec![
            episode("a", 1, 1, "A1"),
            episode("b", 1, 1, "B1"),
            episode("c", 1, 1, "C1"),
        ];
        // The user only started `a`.
        let chosen = select_next_up(episodes, &series(&["a"]), 20);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].series_id.as_deref(), Some("a"));
    }

    #[test]
    fn one_episode_per_series_even_when_far_behind() {
        let episodes = vec![
            episode("a", 1, 1, "A1"),
            episode("a", 1, 2, "A2"),
            episode("a", 1, 3, "A3"),
            episode("b", 2, 5, "B5"),
            episode("b", 1, 4, "B4"),
        ];
        let watched = series(&["a", "b"]);

        let chosen = select_next_up(episodes, &watched, 20);
        assert_eq!(chosen.len(), 2);

        // The earliest episode *by season first*, not by file order: `a` opens
        // at S01E01 while `b` has nothing in season 1, so S02E05 is next.
        let picked: Vec<(&str, &str)> = chosen
            .iter()
            .map(|e| (e.series_id.as_deref().unwrap_or(""), e.name.as_str()))
            .collect();
        assert_eq!(picked, vec![("a", "A1"), ("b", "B4")]);
    }

    #[test]
    fn limit_caps_the_number_of_series() {
        let episodes = vec![
            episode("a", 1, 1, "A1"),
            episode("b", 1, 1, "B1"),
            episode("c", 1, 1, "C1"),
        ];
        let chosen = select_next_up(episodes, &series(&["a", "b", "c"]), 2);
        assert_eq!(chosen.len(), 2);
    }

    #[test]
    fn episodes_without_a_series_are_dropped() {
        let mut orphan = episode("", 1, 1, "Loose");
        orphan.series_id = None;
        orphan.kind = jf_core::BaseItemKind::Video;

        let chosen = select_next_up(vec![orphan], &series(&["a"]), 20);
        assert!(chosen.is_empty());
    }

    #[test]
    fn nothing_started_yields_nothing() {
        let episodes = vec![episode("a", 1, 1, "A1"), episode("b", 1, 1, "B1")];
        assert!(select_next_up(episodes, &HashSet::new(), 20).is_empty());
    }
}