//! `nixclip stats` — display clipboard history statistics.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::Result;

use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, json: bool) -> Result<()> {
    // Query with limit=0 to get the total count without fetching entries.
    let msg = ClientMessage::query(None, None, 0, 0);

    match client.request(&msg).await? {
        ServerMessage::QueryResult { entries: _, total, .. } => {
            // Also query pinned entries count.
            let pinned_msg = ClientMessage::query(None, None, 0, 10000);
            let (pinned_count, text_count, image_count, url_count, files_count, richtext_count) =
                match client.request(&pinned_msg).await? {
                    ServerMessage::QueryResult { entries, .. } => {
                        let pinned = entries.iter().filter(|e| e.pinned).count();
                        let text = entries
                            .iter()
                            .filter(|e| {
                                matches!(e.content_class, nixclip_core::ContentClass::Text)
                            })
                            .count();
                        let image = entries
                            .iter()
                            .filter(|e| {
                                matches!(e.content_class, nixclip_core::ContentClass::Image)
                            })
                            .count();
                        let url = entries
                            .iter()
                            .filter(|e| {
                                matches!(e.content_class, nixclip_core::ContentClass::Url)
                            })
                            .count();
                        let files = entries
                            .iter()
                            .filter(|e| {
                                matches!(e.content_class, nixclip_core::ContentClass::Files)
                            })
                            .count();
                        let richtext = entries
                            .iter()
                            .filter(|e| {
                                matches!(e.content_class, nixclip_core::ContentClass::RichText)
                            })
                            .count();
                        (pinned, text, image, url, files, richtext)
                    }
                    _ => (0, 0, 0, 0, 0, 0),
                };

            if json {
                let obj = serde_json::json!({
                    "total_entries": total,
                    "pinned": pinned_count,
                    "by_type": {
                        "text": text_count,
                        "image": image_count,
                        "url": url_count,
                        "files": files_count,
                        "richtext": richtext_count,
                    }
                });
                println!("{}", obj);
            } else {
                println!("Clipboard History Statistics");
                println!("{}", "─".repeat(30));
                println!("Total entries:   {}", total);
                println!("Pinned:          {}", pinned_count);
                println!();
                println!("By type:");
                println!("  Text:          {}", text_count);
                println!("  Rich text:     {}", richtext_count);
                println!("  Image:         {}", image_count);
                println!("  URL:           {}", url_count);
                println!("  Files:         {}", files_count);
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
