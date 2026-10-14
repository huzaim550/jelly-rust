//! `jf-admin` — a command-line front end for server operations.
//!
//! The HTTP API is not built yet, so this is how you exercise the server
//! end-to-end from a terminal: create users, register libraries, run scans,
//! and inspect what the Python plugin host loaded.

use anyhow::{Context, Result};
use jf_core::{BaseItemKind, UserConfiguration, UserPolicy};
use jf_db::users::NewUser;
use jf_db::Database;
use jf_plugins::PluginHost;
use jf_scan::{LibraryConfig, Scanner};
use std::path::PathBuf;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");

    match command {
        "init" => cmd_init(&args).await,
        "create-user" => cmd_create_user(&args).await,
        "list-users" => cmd_list_users().await,
        "add-library" => cmd_add_library(&args).await,
        "scan" => cmd_scan().await,
        "items" => cmd_items(&args).await,
        "plugins" => cmd_plugins().await,
        "probe" => cmd_probe(&args),
        "parse" => cmd_parse(&args),
        "--version" | "-V" => {
            println!("jf-admin {VERSION}");
            Ok(())
        }
        _ => {
            print_help();
            if command == "help" || command == "--help" || command == "-h" {
                Ok(())
            } else {
                anyhow::bail!("unknown command `{command}`; try `jf-admin help`")
            }
        }
    }
}

fn print_help() {
    println!(
        r#"jf-admin {VERSION} — server administration

USAGE
  jf-admin <command> [args]

COMMANDS
  init                          Create the database and mark setup complete
  create-user <name> [pw]       Create a user (omit pw for passwordless)
  list-users                    List accounts and their admin flag
  add-library <path> <type>     Register a library (movies|tvshows|music|photos|books)
  scan                          Scan every library
  items [library]               Show indexed items
  plugins                       Load the plugins directory and report results
  probe <file>                  Run ffprobe on one file
  parse <name>                  Show how a filename is interpreted

LIBRARY TYPES
  movies tvshows music photos books

ENVIRONMENT
  JF_DATA_DIR   data directory (default: ./jf-data)
  FFMPEG_PATH   ffmpeg binary   (default: ffmpeg from PATH)
  FFPROBE_PATH  ffprobe binary  (default: ffprobe from PATH)
  RUST_LOG      log level       (default: info)
"#
    );
}

/// Root of the server's data directory.
fn data_dir() -> PathBuf {
    std::env::var("JF_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./jf-data"))
}

/// Open the database, creating the directory if needed.
async fn open_db() -> Result<Database> {
    let dir = data_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    Database::connect(dir.join("jf.db"))
        .await
        .context("opening database")
}

/// Ensure the server row exists before anything else.
async fn ensure_server(db: &Database) -> Result<String> {
    let id = db.ensure_server("jellyfin-clone", VERSION).await?;
    if !db.startup_completed().await.unwrap_or(false) {
        db.complete_startup_wizard().await?;
    }
    Ok(id)
}

async fn cmd_init(_args: &[String]) -> Result<()> {
    let db = open_db().await?;
    let id = ensure_server(&db).await?;

    println!("database ready at {}", data_dir().join("jf.db").display());
    println!("server id: {id}");

    let users = db.user_count().await?;
    if users == 0 {
        println!("\nNo users yet. Create the first one:");
        println!("  jf-admin create-user admin");
        println!();
    }
    Ok(())
}

async fn cmd_create_user(args: &[String]) -> Result<()> {
    let name = args
        .get(1)
        .context("usage: jf-admin create-user <name> [password]")?;
    let password = args.get(2).map(String::as_str);

    let db = open_db().await?;
    let server_id = ensure_server(&db).await?;

    // The first account is an administrator; there would otherwise be no way
    // to promote anyone later through the CLI.
    let first_user = db.user_count().await? == 0;
    let policy = if first_user { UserPolicy::administrator() } else { UserPolicy::default() };

    match password {
        Some(pw) => {
            let user = db
                .create_user(NewUser {
                    username: name,
                    password: Some(pw),
                    server_id: &server_id,
                    policy,
                    configuration: UserConfiguration::default(),
                })
                .await?;
            println!(
                "created user `{}` ({}){}",
                user.username,
                user.id,
                if first_user { " — administrator" } else { "" }
            );
        }
        None => {
            let user = db
                .create_user(NewUser {
                    username: name,
                    password: None,
                    server_id: &server_id,
                    policy,
                    configuration: UserConfiguration::default(),
                })
                .await?;
            println!(
                "created passwordless user `{}` ({}) — anyone can sign in as this name",
                user.username, user.id
            );
        }
    }
    Ok(())
}

async fn cmd_list_users() -> Result<()> {
    let db = open_db().await?;
    let users = db.all_users().await?;

    if users.is_empty() {
        println!("no users; create one with `jf-admin create-user <name>`");
        return Ok(());
    }

    println!("{:<20} {:<34} {:<6} {:<12} {}", "NAME", "ID", "ADMIN", "PASSWORD", "LAST LOGIN");
    for u in users {
        println!(
            "{:<20} {:<34} {:<6} {:<12} {}",
            u.username,
            u.id,
            if u.policy.is_administrator { "yes" } else { "no" },
            if u.password_hash.is_some() { "set" } else { "none" },
            u.last_login_date.as_deref().unwrap_or("never"),
        );
    }
    Ok(())
}

async fn cmd_add_library(args: &[String]) -> Result<()> {
    let path = args.get(1).context("usage: jf-admin add-library <path> <type>")?;
    let kind = args.get(2).map(String::as_str).unwrap_or("movies");

    let root = PathBuf::from(path);
    if !root.is_dir() {
        anyhow::bail!("{} is not a directory", root.display());
    }

    let db = open_db().await?;
    let scanner = Scanner::new(db);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    let id = scanner
        .add_library(&LibraryConfig {
            id: String::new(),
            name,
            path: root.clone(),
            collection_type: kind.to_ascii_lowercase(),
        })
        .await?;

    println!("registered library {id} -> {} ({kind})", root.display());
    println!("run `jf-admin scan` to index it");
    Ok(())
}

async fn cmd_scan() -> Result<()> {
    let db = open_db().await?;
    let scanner = Scanner::new(db);

    let libraries = scanner.libraries().await?;
    if libraries.is_empty() {
        println!("no libraries registered; add one with `jf-admin add-library <path> <type>`");
        return Ok(());
    }

    for lib in &libraries {
        println!("scanning {} ({}) ...", lib.name, lib.collection_type);
    }

    let reports = scanner.scan_all().await?;
    println!();
    println!("{:<24} {:>7} {:>7} {:>8} {:>8} {:>7}", "LIBRARY", "ADDED", "UPDATED", "REMOVED", "SKIPPED", "ERRORS");
    for r in &reports {
        let name = libraries
            .iter()
            .find(|l| l.id == r.library_id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| r.library_id.clone());
        println!(
            "{:<24} {:>7} {:>7} {:>8} {:>8} {:>7}",
            name, r.added, r.updated, r.removed, r.skipped, r.errors
        );
    }
    Ok(())
}

async fn cmd_items(args: &[String]) -> Result<()> {
    use sqlx::Row;

    let db = open_db().await?;
    let filter = args.get(1).map(String::as_str);

    let rows = if let Some(name) = filter {
        sqlx::query(
            "SELECT i.id, i.name, i.kind, i.production_year, i.run_time_ticks, i.path
             FROM items i JOIN libraries l ON l.id = i.library_id
             WHERE l.name = ? AND i.is_folder = 0 ORDER BY i.sort_name",
        )
        .bind(name)
        .fetch_all(db.pool())
        .await?
    } else {
        sqlx::query(
            "SELECT id, name, kind, production_year, run_time_ticks, path
             FROM items WHERE is_folder = 0 ORDER BY sort_name",
        )
        .fetch_all(db.pool())
        .await?
    };

    let row_count = rows.len();
    if rows.is_empty() {
        println!("no items; run `jf-admin scan` first");
        return Ok(());
    }

    println!("{:<40} {:<12} {:>6} {:>8}  {}", "NAME", "KIND", "YEAR", "RUNTIME", "PATH");
    for r in rows {
        let ticks: i64 = r.get("run_time_ticks");
        let runtime = if ticks > 0 {
            format!("{}:{:02}:{:02}", jf_core::ticks_to_secs(ticks) as u64 / 3600, (jf_core::ticks_to_secs(ticks) as u64 / 60) % 60, (jf_core::ticks_to_secs(ticks) as u64) % 60)
        } else {
            "-".into()
        };
        let year: Option<i32> = r.get("production_year");
        println!(
            "{:<40} {:<12} {:>6} {:>8}  {}",
            truncate(&r.get::<String, _>("name"), 40),
            r.get::<String, _>("kind"),
            year.map(|y| y.to_string()).unwrap_or_else(|| "-".into()),
            runtime,
            r.get::<String, _>("path"),
        );
    }
    println!("\n{row_count} item(s)");
    Ok(())
}

async fn cmd_plugins() -> Result<()> {
    let db = open_db().await?;
    // Default to the data directory's plugin folder, matching the server.
    let dir = std::env::var("JF_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir().join("plugins"));

    println!("plugins directory: {}", dir.display());

    // Start the same Node host the server would, so this shows the real result.
    let host = PluginHost::start(&dir, db, tokio::runtime::Handle::current())
        .await
        .context("starting plugin host")?;

    let report = host.report().await;

    println!("host API version: {}", host.api_version());
    println!();

    if report.loaded.is_empty() {
        println!("no plugins loaded");
    } else {
        println!("LOADED");
        for plugin in &report.loaded {
            let providers = plugin.metadata_providers.join(", ");
            println!(
                "  + {} v{} ({}){}",
                plugin.id,
                plugin.version,
                plugin.name,
                if providers.is_empty() {
                    String::new()
                } else {
                    format!(" — providers: {providers}")
                }
            );
        }
    }

    if !report.failed.is_empty() {
        println!("\nFAILED");
        for failure in &report.failed {
            println!("  - {}: {}", failure.id, failure.reason);
        }
    }

    let logs = host.drain_logs().await;
    host.shutdown().await;
    if !logs.is_empty() {
        println!("\nPLUGIN LOG");
        for record in logs {
            println!("  [{:?}] {}: {}", record.level, record.plugin_id, record.message);
        }
    }

    println!("\n{} plugin(s) loaded, {} failed", report.loaded.len(), report.failed.len());
    Ok(())
}

fn cmd_probe(args: &[String]) -> Result<()> {
    let path = args
        .get(1)
        .context("usage: jf-admin probe <file>")?;
    let prober = jf_scan::Prober::new();
    let result = prober.probe(std::path::Path::new(path))?;

    println!("file:      {}", result.path);
    println!("container: {}", result.container.as_deref().unwrap_or("-"));
    println!("size:      {} bytes", result.size.unwrap_or(0));
    println!(
        "duration:  {}",
        result.duration_secs.map(|d| format!("{d:.2}s")).unwrap_or_else(|| "-".into())
    );
    println!("streams:   {}", result.streams.len());

    for s in &result.streams {
        let detail = match s.stream_type {
            jf_scan::probe::StreamType::Video => format!(
                "{}x{}",
                s.width.unwrap_or(0),
                s.height.unwrap_or(0)
            ),
            jf_scan::probe::StreamType::Audio => format!(
                "{}ch {}Hz",
                s.channels.unwrap_or(0),
                s.sample_rate.unwrap_or(0)
            ),
            _ => String::new(),
        };
        println!(
            "  #{:<3} {:<9} {:<12} {:<8} {:<10} {}{}",
            s.index,
            format!("{:?}", s.stream_type),
            s.codec.as_deref().unwrap_or("-"),
            s.language.as_deref().unwrap_or("-"),
            s.title.as_deref().unwrap_or("-"),
            detail,
            if s.is_default { "  (default)" } else { "" },
        );
    }
    Ok(())
}

fn cmd_parse(args: &[String]) -> Result<()> {
    let name = args.get(1).context("usage: jf-admin parse <filename>")?;
    let parsed = jf_scan::NameParser::new().parse_name(name);

    println!("input:      {name}");
    println!("title:      {}", parsed.title);
    println!("sort title: {}", parsed.sort_title());
    println!("year:       {}", parsed.year.map(|y| y.to_string()).unwrap_or_else(|| "-".into()));
    println!("season:     {}", parsed.season.map(|s| s.to_string()).unwrap_or_else(|| "-".into()));
    println!("episode:    {}", parsed.episode.map(|e| e.to_string()).unwrap_or_else(|| "-".into()));
    println!("ep title:   {}", parsed.episode_title.as_deref().unwrap_or("-"));

    let kind = match (parsed.season, parsed.episode) {
        (Some(_), Some(_)) => BaseItemKind::Episode,
        (Some(_), None) => BaseItemKind::Season,
        (None, _) => BaseItemKind::Movie,
    };
    println!("classified: {kind:?}");
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}