//! `nixclip pin <id>` / `nixclip unpin <id>` — pin or unpin clipboard entries.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::ipc_client::IpcClient;

/// Pin an entry so it is excluded from automatic pruning and `nixclip clear`.
pub async fn run_pin(client: &mut IpcClient, id: i64) -> Result<()> {
    let msg = ClientMessage::pin(id, true);

    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            println!("Entry {} pinned.", id);
        }
        ServerMessage::Error { message, .. } => {
            eprintln!("Error from daemon: {}", message);
            std::process::exit(1);
        }
        other => {
            eprintln!("Unexpected response from daemon: {:?}", other);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Unpin a previously-pinned entry.
pub async fn run_unpin(client: &mut IpcClient, id: i64) -> Result<()> {
    let msg = ClientMessage::pin(id, false);

    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            println!("Entry {} unpinned.", id);
        }
        ServerMessage::Error { message, .. } => {
            eprintln!("Error from daemon: {}", message);
            std::process::exit(1);
        }
        other => {
            eprintln!("Unexpected response from daemon: {:?}", other);
            std::process::exit(1);
        }
    }

    Ok(())
}
