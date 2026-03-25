//! nixclip — command-line interface for the NixClip clipboard manager.
//!
//! This binary is named "nixclip" (set via [[bin]] in Cargo.toml).
//! It communicates with the nixclipd daemon over a Unix domain socket.

mod commands;
mod ipc_client;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nixclip_core::config::Config;
use nixclip_core::NixClipError;

use commands::config_cmd::ConfigAction as CmdConfigAction;
use ipc_client::IpcClient;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "nixclip",
    about = "NixClip clipboard manager CLI",
    long_about = "nixclip is the command-line interface for the NixClip clipboard manager daemon.\n\
                  The daemon (nixclipd) must be running for most commands to work.\n\
                  Run `nixclip doctor` to diagnose connection issues."
)]
struct Cli {
    /// Output results as JSON (one object per line, suitable for piping to jq).
    #[arg(long, global = true)]
    json: bool,

    /// Path to the nixclipd Unix socket (overrides the default location).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List recent clipboard history entries.
    List {
        /// Maximum number of entries to show.
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Filter by content type (text, richtext, image, files, url).
        #[arg(long, name = "type")]
        content_type: Option<String>,
    },

    /// Search clipboard history by text.
    Search {
        /// Search query string.
        query: String,

        /// Maximum number of results to show.
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Filter by content type (text, richtext, image, files, url).
        #[arg(long, name = "type")]
        content_type: Option<String>,
    },

    /// Show full details for a single clipboard entry.
    Show {
        /// Entry ID (shown by `nixclip list`).
        id: i64,
    },

    /// Restore a clipboard entry to the system clipboard.
    Paste {
        /// Entry ID to restore.
        id: i64,

        /// Restore as plain text (strips formatting).
        #[arg(long)]
        plain: bool,
    },

    /// Permanently delete one or more clipboard entries.
    Delete {
        /// One or more entry IDs to delete.
        ids: Vec<i64>,
    },

    /// Remove clipboard history entries (unpinned by default).
    Clear {
        /// Also remove pinned entries (cannot be undone).
        #[arg(long)]
        include_pinned: bool,
    },

    /// Pin a clipboard entry (protects it from auto-pruning and clear).
    Pin {
        /// Entry ID to pin.
        id: i64,
    },

    /// Unpin a previously-pinned clipboard entry.
    Unpin {
        /// Entry ID to unpin.
        id: i64,
    },

    /// View or modify daemon configuration.
    Config {
        #[command(subcommand)]
        action: Option<ConfigSubcommand>,
    },

    /// Run diagnostic checks on the nixclip installation.
    Doctor,

    /// Display clipboard history statistics.
    Stats,
}

#[derive(Subcommand)]
enum ConfigSubcommand {
    /// Set a configuration value (e.g. nixclip config set general.max_entries 500).
    Set {
        /// Configuration key (may be dotted, e.g. "general.max_entries").
        key: String,

        /// New value.
        value: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Initialize a simple tracing subscriber (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        // Format user-facing errors helpfully.
        eprintln!("Error: {}", e);

        // Suggest `doctor` for IPC errors.
        if matches!(e, NixClipError::Ipc(_)) {
            eprintln!(
                "\nCould not connect to nixclipd. Is the daemon running?\n\
                 Try: nixclip doctor"
            );
        }

        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> nixclip_core::Result<()> {
    let socket_path = cli
        .socket
        .clone()
        .unwrap_or_else(Config::socket_path);

    match cli.command {
        // Commands that do NOT require a daemon connection.
        Commands::Doctor => {
            commands::doctor::run(cli.json).await?;
        }

        // Commands that require a daemon connection.
        Commands::List {
            limit,
            content_type,
        } => {
            let mut client = connect(&socket_path).await?;
            commands::list::run(&mut client, limit, content_type, cli.json).await?;
        }

        Commands::Search {
            query,
            limit,
            content_type,
        } => {
            let mut client = connect(&socket_path).await?;
            commands::search::run(&mut client, &query, limit, content_type, cli.json).await?;
        }

        Commands::Show { id } => {
            let mut client = connect(&socket_path).await?;
            commands::show::run(&mut client, id, cli.json).await?;
        }

        Commands::Paste { id, plain } => {
            let mut client = connect(&socket_path).await?;
            commands::paste::run(&mut client, id, plain).await?;
        }

        Commands::Delete { ids } => {
            let mut client = connect(&socket_path).await?;
            commands::delete::run(&mut client, ids).await?;
        }

        Commands::Clear { include_pinned } => {
            let mut client = connect(&socket_path).await?;
            commands::clear::run(&mut client, include_pinned).await?;
        }

        Commands::Pin { id } => {
            let mut client = connect(&socket_path).await?;
            commands::pin::run_pin(&mut client, id).await?;
        }

        Commands::Unpin { id } => {
            let mut client = connect(&socket_path).await?;
            commands::pin::run_unpin(&mut client, id).await?;
        }

        Commands::Config { action } => {
            let mut client = connect(&socket_path).await?;
            let cmd_action = action.map(|a| match a {
                ConfigSubcommand::Set { key, value } => CmdConfigAction::Set { key, value },
            });
            commands::config_cmd::run(&mut client, cmd_action, cli.json).await?;
        }

        Commands::Stats => {
            let mut client = connect(&socket_path).await?;
            commands::stats::run(&mut client, cli.json).await?;
        }
    }

    Ok(())
}

/// Connect to the daemon, producing a user-friendly error on failure.
async fn connect(socket_path: &std::path::Path) -> nixclip_core::Result<IpcClient> {
    IpcClient::connect(socket_path).await.map_err(|e| {
        NixClipError::Ipc(format!(
            "Could not connect to nixclipd at {}.\n\
             Is the daemon running? Try: nixclip doctor\n\
             (Original error: {})",
            socket_path.display(),
            e
        ))
    })
}
