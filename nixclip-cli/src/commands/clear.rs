//! `nixclip clear` — remove unpinned (or all) clipboard history entries.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, include_pinned: bool) -> Result<()> {
    if include_pinned {
        // Clear all: first clear unpinned, then we'd need to unpin + clear,
        // but the daemon only exposes ClearUnpinned. To clear everything,
        // query all pinned entries, unpin them, then clear unpinned.
        eprintln!(
            "Warning: --include-pinned will unpin all entries before clearing. \
             This cannot be undone."
        );

        // Fetch all pinned entries.
        let query_msg = ClientMessage::query(None, None, 0, 10000);
        let pinned_ids = match client.request(&query_msg).await? {
            ServerMessage::QueryResult { entries, .. } => entries
                .into_iter()
                .filter(|e| e.pinned)
                .map(|e| e.id)
                .collect::<Vec<_>>(),
            ServerMessage::Error { message, .. } => {
                eprintln!("Error from daemon: {}", message);
                std::process::exit(1);
            }
            other => {
                eprintln!("Unexpected response from daemon: {:?}", other);
                std::process::exit(1);
            }
        };

        // Unpin each pinned entry.
        for id in pinned_ids {
            let unpin_msg = ClientMessage::pin(id, false);
            match client.request(&unpin_msg).await? {
                ServerMessage::Ok { .. } => {}
                ServerMessage::Error { message, .. } => {
                    eprintln!("Error unpinning entry {}: {}", id, message);
                    std::process::exit(1);
                }
                other => {
                    eprintln!("Unexpected response from daemon: {:?}", other);
                    std::process::exit(1);
                }
            }
        }
    }

    // Now clear all unpinned entries.
    let msg = ClientMessage::clear_unpinned();
    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            if include_pinned {
                println!("All clipboard history cleared.");
            } else {
                println!("Unpinned clipboard history cleared.");
            }
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
