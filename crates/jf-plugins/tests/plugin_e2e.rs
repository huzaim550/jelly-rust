//! End-to-end plugin host tests.
//!
//! These load the real plugin in `plugins/` through the real pyo3 bridge, so
//! they cover manifest validation, `sys.path` injection, `#[pyclass]` injection
//! of `host`/`config`, the `setup()` handshake, and JSON hook marshalling
//! together. A unit test with a mock cannot catch a mismatch between the Rust
//! and Python halves of the plugin API.

use jf_plugins::api::{MetadataQuery, PluginEvent};
use jf_plugins::PluginHost;
use std::collections::HashMap;
use std::path::PathBuf;

/// Absolute path to the repository's `plugins/` directory.
fn plugins_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/crates/jf-plugins
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repository root")
        .join("plugins")
}

fn sdk_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repository root")
        .join("pluginsdk")
}

/// Make `jellyplug` importable by the plugin.
///
/// A real deployment installs the SDK into the interpreter's site-packages; in
/// tests it is a source checkout, so it goes on `PYTHONPATH`.
///
/// The environment is mutated before the interpreter is initialized and behind a
/// `Once`, so no other test thread can observe a torn read. That is what makes
/// the `unsafe` block sound despite tests running in parallel.
fn with_sdk_on_pythonpath() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let existing = std::env::var("PYTHONPATH").unwrap_or_default();
        let sdk = sdk_dir().to_string_lossy().into_owned();
        let joined = if existing.is_empty() {
            sdk
        } else {
            format!("{sdk}{}{existing}", std::path::MAIN_SEPARATOR)
        };
        // SAFETY: runs exactly once, before any Python interpreter is
        // initialized in this process and before other tests read these vars.
        unsafe { std::env::set_var("PYTHONPATH", joined) };
    });
}

/// Whether a TMDB credential is present in the ambient environment.
fn has_tmdb_credentials() -> bool {
    std::env::var("TMDB_API_KEY")
        .or_else(|_| std::env::var("TMDB_API_TOKEN"))
        .is_ok_and(|v| !v.is_empty())
}

#[tokio::test]
async fn loads_real_tmdb_plugin_from_disk() {
    with_sdk_on_pythonpath();

    let db = jf_db::Database::connect_memory().await.expect("in-memory db");
    let mut host = PluginHost::new(plugins_dir(), db, tokio::runtime::Handle::current()).expect("host");

    let report = host.load_all().expect("load pass");
    assert!(
        report.failed.is_empty(),
        "no plugin should fail to load, got: {:?}",
        report.failed
    );
    assert!(
        report.loaded.contains(&"tmdb".to_owned()),
        "expected the tmdb plugin to load, got {:?}",
        report.loaded
    );
    assert_eq!(host.plugin_count(), 1);
}

#[tokio::test]
async fn tmdb_hook_returns_none_without_an_api_key() {
    with_sdk_on_pythonpath();

    let db = jf_db::Database::connect_memory().await.expect("in-memory db");
    let mut host = PluginHost::new(plugins_dir(), db, tokio::runtime::Handle::current()).expect("host");
    host.load_all().expect("load pass");

    // With no API key configured the plugin must decline cleanly rather than
    // raising: a provider without credentials is normal, not an error.
    //
    // The environment is intentionally left alone here (mutating it from one
    // test while another runs in parallel would be a data race). If a key does
    // happen to be exported, a network call follows instead, so that assertion
    // is skipped rather than made flaky.
    let query = MetadataQuery {
        title: "The Matrix".into(),
        year: Some(1999),
        kind: jf_core::BaseItemKind::Movie,
        known_ids: HashMap::new(),
        preferred_languages: vec!["en".into()],
    };

    let result = host
        .fetch_metadata("tmdb", &query, 10)
        .expect("hook call must never propagate a Python error to the caller");

    if !has_tmdb_credentials() {
        assert!(result.is_none(), "expected no metadata without a key");
    }
}

#[tokio::test]
async fn unknown_plugin_is_reported_not_panicked() {
    let db = jf_db::Database::connect_memory().await.expect("in-memory db");
    let host = PluginHost::new(std::env::temp_dir().join("jf-no-such-plugins"), db, tokio::runtime::Handle::current()).expect("host");

    let query = MetadataQuery {
        title: "Anything".into(),
        year: None,
        kind: jf_core::BaseItemKind::Movie,
        known_ids: HashMap::new(),
        preferred_languages: vec![],
    };

    let err = host.fetch_metadata("does-not-exist", &query, 5).unwrap_err();
    assert!(matches!(err, jf_plugins::PluginError::Missing(_)), "got {err:?}");
}

#[tokio::test]
async fn event_dispatch_to_unloaded_plugins_is_a_no_op() {
    let db = jf_db::Database::connect_memory().await.expect("in-memory db");
    let host = PluginHost::new(std::env::temp_dir().join("jf-no-plugins-events"), db, tokio::runtime::Handle::current()).expect("host");

    // Must not panic or block even though nothing is listening.
    host.dispatch_event(&PluginEvent::ScanComplete { library_id: "abc".into() });
    host.dispatch_event(&PluginEvent::PlaybackStop {
        item_id: "i".into(),
        user_id: "u".into(),
        position_ticks: 100,
    });
}

#[tokio::test]
async fn manifest_with_wrong_api_version_is_refused() {
    use jf_plugins::Manifest;

    // Each plugin lives in its own subdirectory of the plugins root.
    let root = std::env::temp_dir().join("jf-bad-plugins");
    let dir = root.join("fromthefuture");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("manifest.toml"),
        r#"
        id = "fromthefuture"
        name = "Future Plugin"
        version = "9.0.0"
        api_version = "2.0"
        entry = "nope"
        capabilities = []
        "#,
    )
    .expect("write manifest");

    let db = jf_db::Database::connect_memory().await.expect("in-memory db");
    let mut host = PluginHost::new(&root, db, tokio::runtime::Handle::current()).expect("host");
    let report = host.load_all().expect("load pass");

    assert!(report.loaded.is_empty(), "a future-API plugin must not load");
    assert_eq!(report.failed.len(), 1, "got {:?}", report.failed);
    assert_eq!(report.failed[0].0, "fromthefuture");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn manifest_round_trip_is_stable() {
    let manifest = jf_plugins::Manifest::from_toml(
        r#"
        id = "omdb"
        name = "OMDb"
        version = "1.0.0"
        api_version = "1.0"
        entry = "omdb_plugin"
        capabilities = ["network", "read_items"]
        metadata_providers = ["omdb", "imdb"]
        "#,
    )
    .expect("manifest parses");

    assert_eq!(manifest.id, "omdb");
    assert_eq!(manifest.metadata_providers, vec!["omdb", "imdb"]);
    assert!(manifest
        .capabilities
        .contains(&jf_plugins::Capability::Network));
}