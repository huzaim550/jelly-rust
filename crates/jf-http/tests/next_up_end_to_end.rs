//! End-to-end check of "Next Up" against a real scanned library.
//!
//! The unit tests in `src/next_up.rs` cover the ranking rules, but they hand
//! the function finished `BaseItemDto`s. This test goes through the whole
//! chain — encode media, scan it as a TV library, mark an episode watched,
//! ask the database which series are started — because the part that was
//! actually broken sat in the middle of that chain: the scanner never wrote
//! `items.series_id`, so the query returned nothing and "Next Up" was
//! permanently empty no matter how correct the ranking was.

use jf_core::dto::{BaseItemDto, UserConfiguration, UserPolicy};
use jf_core::BaseItemKind;
use jf_db::items::{FieldSelection, ItemQuery};
use jf_db::users::NewUser;
use jf_db::Database;
use jf_http::next_up::select_next_up;
use jf_scan::{LibraryConfig, Scanner};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

const SERIES: &str = "Twin Peaks";

/// Encode one second of real media; `None` when ffmpeg is unavailable.
fn source_clip(dir: &Path) -> Option<std::path::PathBuf> {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return None;
    }
    let file = dir.join("source.mkv");
    let ok = Command::new("ffmpeg")
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "aac"])
        .arg(&file)
        .output()
        .map(|s| s.status.success())
        .unwrap_or(false);
    ok.then_some(file)
}

/// Every unwatched episode in the library — the exact candidate set the
/// handler builds before ranking it.
async fn episodes(db: &Database, user: &str, server_id: &str) -> Vec<BaseItemDto> {
    let query = ItemQuery {
        item_types: vec![BaseItemKind::Episode],
        unplayed_only: true,
        recursive: true,
        limit: 500,
        ..Default::default()
    };
    db.query_items(Some(user), &query, &FieldSelection::all(), server_id)
        .await
        .expect("query episodes")
        .items
}

async fn started_series(db: &Database, user: &str) -> HashSet<String> {
    db.watched_series_for_user(user)
        .await
        .expect("watched series")
        .into_iter()
        .collect()
}

fn episode_number(items: &[BaseItemDto], n: i32) -> &BaseItemDto {
    items
        .iter()
        .find(|e| e.index_number == Some(n))
        .unwrap_or_else(|| panic!("episode {n} should be in the library"))
}

#[tokio::test]
async fn next_up_continues_a_real_scanned_series() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(source) = source_clip(dir.path()) else {
        eprintln!("skipped: ffmpeg unavailable");
        return;
    };

    // Three episodes of one season, named the way a real library would be.
    for n in 1..=3 {
        std::fs::copy(
            &source,
            dir.path().join(format!("{SERIES} - S01E{n:02} - Episode {n}.mkv")),
        )
        .expect("copy fixture");
    }
    let _ = std::fs::remove_file(&source);

    let db = Database::connect_memory().await.expect("db");
    let user = db
        .create_user(NewUser {
            username: "viewer",
            password: None,
            server_id: "server",
            policy: UserPolicy::administrator(),
            configuration: UserConfiguration::default(),
        })
        .await
        .expect("create user");
    let user_id = user.id.clone();

    let scanner = Scanner::new(db.clone());
    let mut cfg = LibraryConfig {
        id: String::new(),
        name: "Shows".into(),
        path: dir.path().to_path_buf(),
        collection_type: "tvshows".into(),
    };
    cfg.id = scanner.add_library(&cfg).await.expect("add library");
    let report = scanner.scan_library(&cfg, None).await.expect("scan");
    assert_eq!(report.added, 3, "all three episodes indexed: {report:?}");
    assert_eq!(report.errors, 0);

    let all = episodes(&db, &user_id, "server").await;
    assert_eq!(all.len(), 3, "scanner found every episode");

    // The series folder is created from the filename, and every episode must
    // point back at it.
    let series_id = all[0].series_id.clone().expect("episode has a series id");
    assert!(
        all.iter().all(|e| e.series_id.as_deref() == Some(series_id.as_str())),
        "all episodes share one series id"
    );

    // Nothing watched yet: an untouched library offers nothing.
    let watched = started_series(&db, &user_id).await;
    assert!(watched.is_empty(), "an untouched library offers nothing");
    assert!(select_next_up(all.clone(), &watched, 20).is_empty());

    // Mark episode 1 watched, the way the player reports completion.
    let runtime = episode_number(&all, 1).run_time_ticks.expect("runtime");
    assert!(runtime > 0, "the fixture has a real duration");
    let position = runtime * 3 / 2;
    db.set_progress(&user_id, &episode_number(&all, 1).id, position, None)
        .await
        .expect("mark played");

    let watched = started_series(&db, &user_id).await;
    assert_eq!(watched.len(), 1, "one started series: {watched:?}");

    let next = select_next_up(episodes(&db, &user_id, "server").await, &watched, 20);
    assert_eq!(next.len(), 1, "one next up per series");
    assert_eq!(next[0].index_number, Some(2), "the episode after the watched one");
    assert_eq!(next[0].series_id.as_deref(), Some(series_id.as_str()));

    // Watching that too advances to episode 3.
    db.set_progress(&user_id, &episode_number(&all, 2).id, position, None)
        .await
        .expect("mark played");

    let watched = started_series(&db, &user_id).await;
    let next = select_next_up(episodes(&db, &user_id, "server").await, &watched, 20);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].index_number, Some(3));
}