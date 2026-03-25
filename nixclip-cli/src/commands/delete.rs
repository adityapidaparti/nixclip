//! `nixclip delete <ids...>` — permanently delete one or more clipboard entries.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, ids: Vec<i64>) -> Result<()> {
    if ids.is_empty() {
        eprintln!("No IDs specified. Usage: nixclip delete <id> [<id>...]");
        std::process::exit(1);
    }

    let count = ids.len();
    let msg = ClientMessage::delete(ids);

    match client.request(&msg).await? {
        ServerMessage::Ok { .. } => {
            if count == 1 {
                println!("Entry deleted.");
            } else {
                println!("{} entries deleted.", count);
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}
