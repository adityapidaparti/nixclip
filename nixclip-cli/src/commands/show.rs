//! `nixclip show <id>` — display full detail for a single clipboard entry.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::list::{entry_json, format_age};
use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, id: i64, json: bool) -> Result<()> {
    let msg = ClientMessage::query(None, None, 0, 1000);

    match client.request(&msg).await? {
        ServerMessage::QueryResult { entries, .. } => {
            let entry = entries.iter().find(|e| e.id == id);

            match entry {
                None => {
                    eprintln!("Entry {} not found.", id);
                    std::process::exit(1);
                }
                Some(entry) => {
                    if json {
                        println!("{}", entry_json(entry, true));
                    } else {
                        println!("ID:           {}", entry.id);
                        println!("Type:         {}", entry.content_class);
                        println!("Pinned:       {}", if entry.pinned { "yes" } else { "no" });
                        println!(
                            "Ephemeral:    {}",
                            if entry.ephemeral { "yes" } else { "no" }
                        );
                        println!(
                            "Created:      {} ({})",
                            entry.created_at,
                            format_age(entry.created_at)
                        );
                        println!("Last seen:    {}", format_age(entry.last_seen_at));
                        if let Some(app) = &entry.source_app {
                            println!("Source app:   {}", app);
                        }
                        if let Some(thumbnail) = &entry.thumbnail {
                            println!("Thumbnail:    {} bytes", thumbnail.len());
                        }
                        println!();
                        if let Some(text) = &entry.preview_text {
                            println!("Preview:");
                            println!("{}", text);
                        } else {
                            println!("[No text preview available]");
                        }
                    }
                }
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}
