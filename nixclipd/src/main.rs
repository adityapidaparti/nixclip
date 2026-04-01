mod hotkey;
mod ipc_server;
mod pruning;
mod screen_lock;
mod watcher;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nixclip_core::config::Config;
use nixclip_core::error::{NixClipError, Result};
use nixclip_core::pipeline::PrivacyFilter;
use nixclip_core::search::SearchEngine;
use nixclip_core::storage::ClipStore;
use nixclip_core::EntrySummary;
use tracing::{error, info};

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Shared state accessible by every daemon subsystem.
pub struct AppState {
    /// The SQLite-backed clipboard store.
    ///
    /// `rusqlite::Connection` is `!Send`, so we wrap in `std::sync::Mutex` and
    /// access from blocking tasks.
    pub store: std::sync::Mutex<ClipStore>,

    /// Current configuration, reloadable at runtime.
    pub config: tokio::sync::RwLock<Config>,

    /// The config file this daemon instance loaded.
    pub config_path: PathBuf,

    /// Privacy filter built from the current ignore rules.
    pub privacy_filter: tokio::sync::RwLock<PrivacyFilter>,

    /// Full-text / fuzzy search index.
    pub search_engine: SearchEngine,

    /// Whether the screen is currently locked (skip captures while true).
    pub is_locked: AtomicBool,

    /// Broadcast channel for newly captured entries.
    pub new_entry_tx: tokio::sync::broadcast::Sender<EntrySummary>,
}

// ---------------------------------------------------------------------------
// CLI argument parsing (minimal, no extra deps)
// ---------------------------------------------------------------------------

struct CliArgs {
    config_path: Option<String>,
    verbose: bool,
}

fn parse_args() -> CliArgs {
    let mut args = CliArgs {
        config_path: None,
        verbose: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                args.config_path = iter.next();
            }
            "-v" | "--verbose" => {
                args.verbose = true;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(1);
            }
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        error!(error = %e, "daemon exiting with error");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = parse_args();

    // -----------------------------------------------------------------------
    // Logging
    // -----------------------------------------------------------------------
    init_tracing(cli.verbose);

    // -----------------------------------------------------------------------
    // Configuration
    // -----------------------------------------------------------------------
    let config_path = cli
        .config_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(Config::existing_config_path)
        .unwrap_or_else(Config::config_path);
    let config = match &cli.config_path {
        Some(path) => Config::load(path)?,
        None => Config::load_or_default()?,
    };

    let db_path = Config::db_path();
    let blob_dir = Config::blob_dir();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        config_path = %config_path.display(),
        db_path = %db_path.display(),
        "nixclipd starting"
    );

    // -----------------------------------------------------------------------
    // Ensure data directories exist
    // -----------------------------------------------------------------------
    std::fs::create_dir_all(Config::data_dir()).map_err(nixclip_core::NixClipError::Io)?;
    std::fs::create_dir_all(&blob_dir).map_err(nixclip_core::NixClipError::Io)?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(nixclip_core::NixClipError::Io)?;
    }

    // -----------------------------------------------------------------------
    // Open ClipStore
    // -----------------------------------------------------------------------
    let store = ClipStore::open(&db_path, &blob_dir)?;

    // -----------------------------------------------------------------------
    // Build shared state
    // -----------------------------------------------------------------------
    let privacy_filter = PrivacyFilter::new(&config.ignore)?;
    let search_engine = SearchEngine::new(db_path.clone());
    let (new_entry_tx, _initial_rx) = tokio::sync::broadcast::channel::<EntrySummary>(256);

    let state = Arc::new(AppState {
        store: std::sync::Mutex::new(store),
        config: tokio::sync::RwLock::new(config),
        config_path: config_path.clone(),
        privacy_filter: tokio::sync::RwLock::new(privacy_filter),
        search_engine,
        is_locked: AtomicBool::new(false),
        new_entry_tx,
    });

    // -----------------------------------------------------------------------
    // Spawn subsystems
    // -----------------------------------------------------------------------
    // The watcher is non-critical: if clipboard capture fails (e.g. no
    // Wayland data-control protocol), the daemon should keep running so
    // IPC, pruning, and hotkey listening still work.
    tokio::spawn({
        let s = state.clone();
        async move {
            match watcher::run(s).await {
                Ok(()) => tracing::warn!("clipboard watcher exited (no backend available or wl-paste stopped)"),
                Err(e) => tracing::warn!(error = %e, "clipboard watcher exited with error"),
            }
        }
    });
    let ipc_handle = tokio::spawn(ipc_server::run(state.clone()));

    tokio::spawn({
        let s = state.clone();
        async move {
            match hotkey::run(s).await {
                Ok(()) => info!("hotkey listener exited"),
                Err(e) => error!(error = %e, "hotkey listener exited with error"),
            }
        }
    });

    tokio::spawn({
        let s = state.clone();
        async move {
            match screen_lock::run(s).await {
                Ok(()) => info!("screen lock listener exited"),
                Err(e) => error!(error = %e, "screen lock listener exited with error"),
            }
        }
    });

    tokio::spawn({
        let s = state.clone();
        async move {
            match pruning::run(s).await {
                Ok(()) => info!("pruning task exited"),
                Err(e) => error!(error = %e, "pruning task exited with error"),
            }
        }
    });

    info!("all subsystems started");

    // -----------------------------------------------------------------------
    // Wait for shutdown signal or critical task exit.
    // -----------------------------------------------------------------------
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received SIGINT, shutting down");
        }
        _ = signal_terminate() => {
            info!("received SIGTERM, shutting down");
        }
        result = ipc_handle => {
            critical_task_result("ipc server", result)?;
        }
    }

    // -----------------------------------------------------------------------
    // Graceful shutdown
    // -----------------------------------------------------------------------
    info!("cleaning up");

    // Remove the IPC socket so stale-file detection works on next start.
    let socket_path = Config::socket_path();
    if socket_path.exists() {
        if let Err(e) = std::fs::remove_file(&socket_path) {
            tracing::warn!(path = %socket_path.display(), error = %e, "failed to remove socket");
        }
    }

    info!("nixclipd shut down cleanly");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;

    let default_filter = if verbose { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}

/// Wait for SIGTERM on Unix. On non-Unix platforms this future never resolves.
#[cfg(unix)]
async fn signal_terminate() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sig = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    sig.recv().await;
}

fn critical_task_result(
    task_name: &str,
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(Ok(())) => Err(NixClipError::Pipeline(format!(
            "{task_name} exited unexpectedly"
        ))),
        Ok(Err(error)) => Err(NixClipError::Pipeline(format!(
            "{task_name} exited with error: {error}"
        ))),
        Err(error) => Err(NixClipError::Pipeline(format!(
            "{task_name} task crashed: {error}"
        ))),
    }
}

#[cfg(not(unix))]
async fn signal_terminate() {
    // On non-Unix platforms, just pend forever so `select!` works.
    std::future::pending::<()>().await;
}
