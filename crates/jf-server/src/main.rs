//! The jellyfin-clone server.
//!
//! Serves the Jellyfin-compatible API and the Python web client. The client is
//! CPython compiled to WebAssembly (Pyodide), embedded in this binary along
//! with the runtime, so a deployment is one file plus its data directory.
//!
//! Usage:
//!   jellyfin-clone                    start the server
//!   jellyfin-clone --port 9000        listen on a different port
//!   jellyfin-clone --data ./data      use a different data directory
//!   jellyfin-clone --scan             scan all libraries once, then exit

use anyhow::{Context, Result};
use jf_http::{
    state::{AppState, ServerAction},
    ServerConfig,
};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
struct Args {
    config: ServerConfig,
    scan_once: bool,
    verbose: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config: ServerConfig::default(),
        scan_once: false,
        verbose: false,
    };

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < raw.len() {
        match raw[i].as_str() {
            "--port" | "-p" => {
                i += 1;
                let value = raw.get(i).context("--port requires a value")?;
                args.config.port = value
                    .parse()
                    .with_context(|| format!("invalid port `{value}`"))?;
            }
            "--host" => {
                i += 1;
                args.config.host = raw
                    .get(i)
                    .context("--host requires a value")?
                    .clone();
            }
            "--data" | "-d" => {
                i += 1;
                args.config.data_dir = std::path::PathBuf::from(
                    raw.get(i).context("--data requires a value")?,
                );
            }
            "--scan" => args.scan_once = true,
            "--verbose" | "-v" => args.verbose = true,
            "--version" | "-V" => {
                println!("jellyfin-clone {VERSION}");
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument `{other}`; try --help"),
        }
        i += 1;
    }

    Ok(args)
}

fn print_help() {
    println!(
        r#"jellyfin-clone {VERSION}

A media server with a Python (Pyodide) web client and a Python plugin host.

USAGE
  jellyfin-clone [options]

OPTIONS
  -p, --port <port>     Port to listen on          (default: 8096)
      --host <addr>     Address to bind            (default: 0.0.0.0)
  -d, --data <dir>      Data directory             (default: ./jf-data)
      --scan            Scan every library once and exit
  -v, --verbose         Debug logging
  -V, --version         Print the version
  -h, --help            Show this help

ENVIRONMENT
  JF_DATA_DIR   Same as --data
  FFMPEG_PATH   Path to ffmpeg    (default: from PATH)
  FFPROBE_PATH  Path to ffprobe   (default: from PATH)
  RUST_LOG      Log filter        (default: info)

MANAGING LIBRARIES AND USERS
  jf-admin init
  jf-admin create-user <name> [password]
  jf-admin add-library /path/to/media movies
  jf-admin scan
"#
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;

    let default_filter = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.into()),
        )
        .with_target(false)
        .init();

    let paths = jf_http::state::DataPaths::new(args.config.data_dir.clone());
    paths.ensure().await.context("creating data directories")?;

    let db = jf_db::Database::connect(&paths.database)
        .await
        .context("opening database")?;

    // Note: the startup-wizard flag is deliberately NOT set here. It is set by
    // `POST /Startup/Complete`, and setting it at boot would skip onboarding
    // entirely, leaving a fresh server with no way to create the first account.
    let server_id = db
        .ensure_server("jellyfin-clone", VERSION)
        .await
        .context("initializing server")?;

    // This starts the Node plugin host and loads every plugin, so a broken
    // plugin is reported at startup rather than on a later metadata request.
    let app = AppState::new(db, paths, VERSION, tokio::runtime::Handle::current())
        .await
        .context("building application state")?;

    let report = app.plugins.report().await;
    if !report.loaded.is_empty() {
        let ids: Vec<&str> = report.loaded.iter().map(|p| p.id.as_str()).collect();
        tracing::info!("loaded plugins: {}", ids.join(", "));
    }
    for failure in &report.failed {
        tracing::warn!(plugin = %failure.id, error = %failure.reason, "plugin failed to load");
    }

    // Remove transcode scratch directories left by a previous run.
    app.hls.cleanup_orphans().await;

    let user_count = app.db.user_count().await.unwrap_or(0);

    if args.scan_once {
        let reports = app.scanner.scan_all().await.context("scanning")?;
        for report in &reports {
            if let Err(error) = jf_http::scans::refresh_metadata(&app, &report.library_id).await {
                tracing::warn!(
                    library = %report.library_id,
                    error = %error,
                    "OMDb metadata refresh failed after scan"
                );
            }
            println!(
                "library {}: {} added, {} updated, {} removed, {} skipped, {} errors",
                report.library_id,
                report.added,
                report.updated,
                report.removed,
                report.skipped,
                report.errors
            );
        }
        return Ok(());
    }

    let addr = format!("{}:{}", args.config.host, args.config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    let router = jf_http::router(Arc::clone(&app));
    let control = app.server_control.subscribe();

    println!();
    println!("  jellyfin-clone {VERSION}");
    println!("  server id:  {server_id}");
    println!("  data dir:   {}", app.paths.root.display());
    println!("  plugins:    {}", app.paths.plugins.display());
    println!("  listening:  http://{addr}");
    println!();

    if user_count == 0 {
        println!("  No accounts yet. Create one, then sign in:");
        println!("      cargo run -p jf-admin -- create-user admin");
        println!();
    }

    if std::path::Path::new("web/dist/index.html").is_file() {
        println!("  Serving the web client from web/dist. Ctrl-C to stop.");
    } else {
        println!("  The web client has not been built yet:");
        println!("      cd web && npm install && npm run build");
        println!("  The API is available regardless. Ctrl-C to stop.");
    }
    println!();

    // Reap idle transcode sessions periodically.
    let reaper = {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                ticker.tick().await;
                app.hls.reap_idle().await;
            }
        })
    };

    let (action_tx, action_rx) = tokio::sync::oneshot::channel();
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let action = shutdown_signal(control).await;
            let _ = action_tx.send(action);
        })
        .await;
    let action = action_rx.await.unwrap_or(ServerAction::Shutdown);

    reaper.abort();
    result.context("server error")?;

    // Give plugins a chance to release resources.
    app.plugins.shutdown().await;

    if action == ServerAction::Restart {
        let executable = std::env::current_exe().context("finding server executable")?;
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        drop(app);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new(executable).args(args).exec();
            return Err(error).context("replacing server process");
        }
        #[cfg(not(unix))]
        {
            std::process::Command::new(executable)
                .args(args)
                .spawn()
                .context("starting replacement server process")?;
            tracing::info!("replacement server process started");
            return Ok(());
        }
    }

    println!("\nstopped");
    Ok(())
}

/// Resolve on Ctrl-C, SIGTERM, or an authenticated administrator request.
async fn shutdown_signal(
    mut control: tokio::sync::watch::Receiver<Option<ServerAction>>,
) -> ServerAction {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let action = tokio::select! {
        _ = ctrl_c => ServerAction::Shutdown,
        _ = terminate => ServerAction::Shutdown,
        changed = control.changed() => {
            if changed.is_ok() {
                control.borrow().unwrap_or(ServerAction::Shutdown)
            } else {
                ServerAction::Shutdown
            }
        }
    };

    tracing::info!("shutdown signal received");
    action
}