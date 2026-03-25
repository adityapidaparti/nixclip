//! `nixclip clear` — remove unpinned (or all) clipboard history entries.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, include_pinned: bool) -> Result<()> {
    if include_pinned {
        eprintln!(
            "Warning: --include-pinned will unpin all entries before clearing. \
             This cannot be undone."
        );

        for id in pinned_ids(client).await? {
            let msg = ClientMessage::pin(id, false);
            match client.request(&msg).await? {
                ServerMessage::Ok { .. } => {}
                ServerMessage::Error { message, .. } => {
                    eprintln!("Error unpinning entry {}: {}", id, message);
                    std::process::exit(1);
                }
                other => unexpected_response(other),
            }
        }
    }

    let msg = ClientMessage::clear_unpinned();
    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            if include_pinned {
                println!("All clipboard history cleared.");
            } else {
                println!("Unpinned clipboard history cleared.");
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}

async fn pinned_ids(client: &mut IpcClient) -> Result<Vec<i64>> {
    let query_msg = ClientMessage::query(None, None, 0, 10000);
    match client.request(&query_msg).await? {
        ServerMessage::QueryResult { entries, .. } => Ok(entries
            .into_iter()
            .filter(|entry| entry.pinned)
            .map(|entry| entry.id)
            .collect()),
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }
}
