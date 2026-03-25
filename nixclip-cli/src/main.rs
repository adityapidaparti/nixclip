mod commands;
mod ipc_client;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nixclip_core::config::Config;

use commands::config_cmd::ConfigAction as CmdConfigAction;
use ipc_client::IpcClient;

#[derive(Parser)]
#[command(
    name = "nixclip",
    about = "NixClip clipboard manager CLI",
    long_about = "nixclip is the command-line interface for the NixClip clipboard manager daemon.\n\
                  The daemon (nixclipd) must be running for most commands to work.\n\
                  Run `nixclip doctor` to diagnose connection issues."
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    List {
        #[arg(long, default_value = "10")]
        limit: u32,

        #[arg(long, name = "type")]
        content_type: Option<String>,
    },

    Search {
        query: String,

        #[arg(long, default_value = "10")]
        limit: u32,

        #[arg(long, name = "type")]
        content_type: Option<String>,
    },

    Show {
        id: i64,
    },

    Paste {
        id: i64,

        #[arg(long)]
        plain: bool,
    },

    Delete {
        ids: Vec<i64>,
    },

    Clear {
        #[arg(long)]
        include_pinned: bool,
    },

    Pin {
        id: i64,
    },

    Unpin {
        id: i64,
    },

    Config {
        #[command(subcommand)]
        action: Option<ConfigSubcommand>,
    },

    Doctor,

    Stats,
}

#[derive(Subcommand)]
enum ConfigSubcommand {
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    if let Err(err) = run(cli).await {
        eprintln!("Error: {}", err);

        if matches!(err, nixclip_core::NixClipError::Ipc(_)) {
            eprintln!(
                "\nCould not connect to nixclipd. Is the daemon running?\n\
                 Try: nixclip doctor"
            );
        }

        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> nixclip_core::Result<()> {
    let Cli {
        json,
        socket,
        command,
    } = cli;
    let socket_path = socket.unwrap_or_else(Config::socket_path);

    match command {
        Commands::Doctor => {
            commands::doctor::run(json).await?;
        }

        Commands::List {
            limit,
            content_type,
        } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::list::run(&mut client, limit, content_type, json).await?;
        }

        Commands::Search {
            query,
            limit,
            content_type,
        } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::search::run(&mut client, &query, limit, content_type, json).await?;
        }

        Commands::Show { id } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::show::run(&mut client, id, json).await?;
        }

        Commands::Paste { id, plain } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::paste::run(&mut client, id, plain).await?;
        }

        Commands::Delete { ids } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::delete::run(&mut client, ids).await?;
        }

        Commands::Clear { include_pinned } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::clear::run(&mut client, include_pinned).await?;
        }

        Commands::Pin { id } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::pin::run_pin(&mut client, id).await?;
        }

        Commands::Unpin { id } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::pin::run_unpin(&mut client, id).await?;
        }

        Commands::Config { action } => {
            let mut client = IpcClient::connect(&socket_path).await?;
            let cmd_action = action.map(|a| match a {
                ConfigSubcommand::Set { key, value } => CmdConfigAction::Set { key, value },
            });
            commands::config_cmd::run(&mut client, cmd_action, json).await?;
        }

        Commands::Stats => {
            let mut client = IpcClient::connect(&socket_path).await?;
            commands::stats::run(&mut client, json).await?;
        }
    }

    Ok(())
}
