//! `nixclip pin <id>` / `nixclip unpin <id>` — pin or unpin clipboard entries.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

/// Pin an entry so it is excluded from automatic pruning and `nixclip clear`.
pub async fn run_pin(client: &mut IpcClient, id: i64) -> Result<()> {
    set_pin_state(client, id, true).await
}

pub async fn run_unpin(client: &mut IpcClient, id: i64) -> Result<()> {
    set_pin_state(client, id, false).await
}

async fn set_pin_state(client: &mut IpcClient, id: i64, pinned: bool) -> Result<()> {
    let msg = ClientMessage::pin(id, pinned);
    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            println!(
                "Entry {} {}.",
                id,
                if pinned { "pinned" } else { "unpinned" }
            );
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }
    Ok(())
}
