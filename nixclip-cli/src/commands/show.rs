//! `nixclip show <id>` — display full detail for a single clipboard entry.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::commands::list::format_age;
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, id: i64, json: bool) -> Result<()> {
    // Query for the specific entry by using a text filter on the id is not
    // ideal; the cleanest approach is to query with a very large offset range
    // around the known id. Since the daemon API doesn't yet support querying
    // by id directly, we fetch recent entries and filter client-side, or we
    // use limit=1 with a search. For robustness we'll query with a high limit
    // and filter to the requested id.
    //
    // A cleaner approach: query with no text, no class, offset=0, limit=0
    // would return no entries. Instead we query with a reasonable window and
    // search linearly. If the daemon ever adds a GetEntry message this can be
    // simplified.

    // Attempt a targeted query: fetch everything (limit=1000, offset from id).
    // We use limit=1000 as a reasonable upper bound for now.
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
                            "has_thumbnail": entry.thumbnail.is_some(),
                        });
                        println!("{}", obj);
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
