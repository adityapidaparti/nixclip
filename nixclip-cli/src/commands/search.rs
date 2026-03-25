//! `nixclip search` — full-text search of clipboard history.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::list::{format_age, print_entry_row, print_table_header};
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
                    let obj = serde_json::json!({
                        "id": entry.id,
                        "content_class": entry.content_class.as_str(),
                        "preview": entry.preview_text,
                        "pinned": entry.pinned,
                        "ephemeral": entry.ephemeral,
                        "created_at": entry.created_at,
                        "last_seen_at": entry.last_seen_at,
                        "source_app": entry.source_app,
                        "age": format_age(entry.created_at),
                    });
                    println!("{}", obj);
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
