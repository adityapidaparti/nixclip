//! `nixclip stats` — display clipboard history statistics.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::{ContentClass, Result};

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

pub async fn run(client: &mut IpcClient, json: bool) -> Result<()> {
    let msg = ClientMessage::query(None, None, 0, 0);

    match client.request(&msg).await? {
        ServerMessage::QueryResult { total, .. } => {
            let pinned_msg = ClientMessage::query(None, None, 0, 10000);
            let (pinned_count, text_count, image_count, url_count, files_count, richtext_count) =
                match client.request(&pinned_msg).await? {
                    ServerMessage::QueryResult { entries, .. } => count_entries(&entries),
                    ServerMessage::Error { message, .. } => daemon_error(message),
                    other => unexpected_response(other),
                };

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "total_entries": total,
                        "pinned": pinned_count,
                        "by_type": {
                            "text": text_count,
                            "image": image_count,
                            "url": url_count,
                            "files": files_count,
                            "richtext": richtext_count,
                        }
                    })
                );
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
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}

fn count_entries(
    entries: &[nixclip_core::EntrySummary],
) -> (usize, usize, usize, usize, usize, usize) {
    entries.iter().fold((0, 0, 0, 0, 0, 0), |counts, entry| {
        let (pinned, text, image, url, files, richtext) = counts;
        (
            pinned + usize::from(entry.pinned),
            text + usize::from(matches!(entry.content_class, ContentClass::Text)),
            image + usize::from(matches!(entry.content_class, ContentClass::Image)),
            url + usize::from(matches!(entry.content_class, ContentClass::Url)),
            files + usize::from(matches!(entry.content_class, ContentClass::Files)),
            richtext + usize::from(matches!(entry.content_class, ContentClass::RichText)),
        )
    })
}
