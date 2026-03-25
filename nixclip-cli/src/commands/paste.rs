//! `nixclip paste <id>` — restore a clipboard entry to the system clipboard.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::{RestoreMode, Result};

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, id: i64, plain: bool) -> Result<()> {
    let mode = if plain {
        RestoreMode::PlainText
    } else {
        RestoreMode::Original
    };

    let msg = ClientMessage::restore(id, mode);

    match client.request(&msg).await? {
        ServerMessage::RestoreResult { success, error, .. } => {
            if success {
                println!("Entry {} restored to clipboard.", id);
            } else {
                let err_msg = error.as_deref().unwrap_or("unknown error");
                eprintln!("Failed to restore entry {}: {}", id, err_msg);
                std::process::exit(1);
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}
