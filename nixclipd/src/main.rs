mod hotkey;
mod ipc_server;
mod pruning;
mod screen_lock;
mod watcher;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use nixclip_core::config::Config;
use nixclip_core::error::Result;
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
    let config = match &cli.config_path {
        Some(path) => Config::load(path).unwrap_or_else(|e| {
            tracing::warn!(path, error = %e, "failed to load config; using defaults");
            Config::default()
        }),
        None => Config::load_or_default(),
    };

    let config_path = cli
        .config_path
        .as_deref()
        .map(String::from)
        .unwrap_or_else(|| Config::config_path().display().to_string());

    let db_path = Config::db_path();
    let blob_dir = Config::blob_dir();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        config_path = %config_path,
        db_path = %db_path.display(),
        "nixclipd starting"
    );

    // -----------------------------------------------------------------------
    // Ensure data directories exist
    // -----------------------------------------------------------------------
    std::fs::create_dir_all(Config::data_dir()).map_err(nixclip_core::NixClipError::Io)?;
    std::fs::create_dir_all(&blob_dir).map_err(nixclip_core::NixClipError::Io)?;
    std::fs::create_dir_all(Config::config_dir()).map_err(nixclip_core::NixClipError::Io)?;

    // -----------------------------------------------------------------------
    // Open ClipStore
    // -----------------------------------------------------------------------
    let store = ClipStore::open(&db_path, &blob_dir)?;

    // -----------------------------------------------------------------------
    // Build shared state
    // -----------------------------------------------------------------------
    let privacy_filter = PrivacyFilter::new(&config.ignore).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to compile privacy filter patterns; using permissive defaults");
        PrivacyFilter::new(&Default::default()).expect("default privacy filter must compile")
    });
    let search_engine = SearchEngine::new(db_path.clone());
    let (new_entry_tx, _initial_rx) = tokio::sync::broadcast::channel::<EntrySummary>(256);

    let state = Arc::new(AppState {
        store: std::sync::Mutex::new(store),
        config: tokio::sync::RwLock::new(config),
        privacy_filter: tokio::sync::RwLock::new(privacy_filter),
        search_engine,
        is_locked: AtomicBool::new(false),
        new_entry_tx,
    });

    // -----------------------------------------------------------------------
    // Spawn subsystems
    // -----------------------------------------------------------------------
    let watcher_handle = tokio::spawn({
        let s = state.clone();
        async move {
            if let Err(e) = watcher::run(s).await {
                error!(error = %e, "watcher exited with error");
            }
        }
    });

    let ipc_handle = tokio::spawn({
        let s = state.clone();
        async move {
            if let Err(e) = ipc_server::run(s).await {
                error!(error = %e, "ipc server exited with error");
            }
        }
    });

    let hotkey_handle = tokio::spawn({
        let s = state.clone();
        async move {
            if let Err(e) = hotkey::run(s).await {
                error!(error = %e, "hotkey listener exited with error");
            }
        }
    });

    let screen_lock_handle = tokio::spawn({
        let s = state.clone();
        async move {
            if let Err(e) = screen_lock::run(s).await {
                error!(error = %e, "screen lock listener exited with error");
            }
        }
    });

    let pruning_handle = tokio::spawn({
        let s = state.clone();
        async move {
            if let Err(e) = pruning::run(s).await {
                error!(error = %e, "pruning task exited with error");
            }
        }
    });

    info!("all subsystems started");

    // -----------------------------------------------------------------------
    // Wait for shutdown signal or any task to exit
    // -----------------------------------------------------------------------
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received SIGINT, shutting down");
        }
        _ = signal_terminate() => {
            info!("received SIGTERM, shutting down");
        }
        _ = watcher_handle => {
            info!("watcher task exited");
        }
        _ = ipc_handle => {
            info!("ipc server task exited");
        }
        _ = hotkey_handle => {
            info!("hotkey task exited");
        }
        _ = screen_lock_handle => {
            info!("screen lock task exited");
        }
        _ = pruning_handle => {
            info!("pruning task exited");
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

#[cfg(not(unix))]
async fn signal_terminate() {
    // On non-Unix platforms, just pend forever so `select!` works.
    std::future::pending::<()>().await;
}
