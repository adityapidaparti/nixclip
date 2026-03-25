//! `nixclip search` — full-text search of clipboard history.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::list::{entry_json, print_entry_row, print_table_header};
use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(
    client: &mut IpcClient,
    query: &str,
    limit: u32,
    content_type: Option<String>,
    json: bool,
) -> Result<()> {
    let msg = ClientMessage::query(Some(query.to_string()), content_type, 0, limit);

    match client.request(&msg).await? {
        ServerMessage::QueryResult { entries, total, .. } => {
            if json {
                for entry in &entries {
                    println!("{}", entry_json(entry, false));
                }
            } else {
                if entries.is_empty() {
                    println!("No entries found matching {:?}.", query);
                    return Ok(());
                }

                print_table_header();
                for entry in &entries {
                    print_entry_row(entry);
                }
                println!();
                println!("Found {} of {} matching entries.", entries.len(), total);
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}
