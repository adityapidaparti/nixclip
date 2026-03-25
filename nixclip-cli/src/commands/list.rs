//! `nixclip list` — display recent clipboard history.

use nixclip_core::ipc::{ClientMessage, ServerMessage};
use nixclip_core::{ContentClass, EntrySummary, Result};

use crate::commands::{daemon_error, unexpected_response};
use crate::ipc_client::IpcClient;

/// Format a Unix timestamp (milliseconds) as a human-readable age string.
pub fn format_age(millis: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let diff_secs = ((now_ms - millis) / 1000).max(0);

    if diff_secs < 60 {
        format!("{}s ago", diff_secs)
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else if diff_secs < 7 * 86400 {
        format!("{}d ago", diff_secs / 86400)
    } else {
        time_from_unix_secs(millis / 1000)
    }
}

fn time_from_unix_secs(secs: i64) -> String {
    let days = secs / 86400;
    let (_, month, day) = days_to_ymd(days);
    let month_name = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(month - 1) as usize];
    format!("{} {}", month_name, day)
}

fn days_to_ymd(mut days: i64) -> (i64, i64, i64) {
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub fn format_preview(entry: &EntrySummary) -> String {
    match &entry.content_class {
        ContentClass::Image => "[Image]".to_string(),
        ContentClass::Files => "[Files]".to_string(),
        _ => {
            let Some(text) = entry.preview_text.as_deref().map(str::trim) else {
                return String::new();
            };

            if text.chars().count() > 38 {
                let truncated: String = text.chars().take(36).collect();
                format!("{}…", truncated)
            } else {
                text.to_string()
            }
        }
    }
}

pub fn print_entry_row(entry: &EntrySummary) {
    let pin_marker = if entry.pinned { "*" } else { " " };
    println!(
        " {pin_marker}{id:<5}│ {type_:<9}│ {preview:<38}│ {age}",
        pin_marker = pin_marker,
        id = entry.id,
        type_ = entry.content_class.as_str(),
        preview = format_preview(entry),
        age = format_age(entry.created_at),
    );
}

pub fn print_table_header() {
    println!(" ID    │ Type      │ Preview                               │ Age");
    println!("───────┼───────────┼───────────────────────────────────────┼────────");
}

pub fn entry_json(entry: &EntrySummary, has_thumbnail: bool) -> serde_json::Value {
    let mut value = serde_json::json!({
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

    if has_thumbnail {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("has_thumbnail".to_string(), serde_json::json!(true));
        }
    }

    value
}

pub async fn run(
    client: &mut IpcClient,
    limit: u32,
    content_type: Option<String>,
    json: bool,
) -> Result<()> {
    let msg = ClientMessage::query(None, content_type, 0, limit);

    match client.request(&msg).await? {
        ServerMessage::QueryResult { entries, total, .. } => {
            if json {
                for entry in &entries {
                    println!("{}", entry_json(entry, false));
                }
            } else {
                if entries.is_empty() {
                    println!("No clipboard history found.");
                    return Ok(());
                }

                print_table_header();
                for entry in &entries {
                    print_entry_row(entry);
                }
                println!();
                println!(
                    "Showing {} of {} entries. Use --limit to see more.",
                    entries.len(),
                    total
                );
            }
        }
        ServerMessage::Error { message, .. } => daemon_error(message),
        other => unexpected_response(other),
    }

    Ok(())
}
