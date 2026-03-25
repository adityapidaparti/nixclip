use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk::prelude::*;
use nixclip_core::{ContentClass, EntrySummary};

/// A single row in the clipboard history list.
pub struct EntryRow {
    pub container: gtk::Box,
    pub entry: EntrySummary,
}

impl EntryRow {
    pub fn new(entry: &EntrySummary) -> Self {
        // Outer horizontal box.
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(6);
        container.set_margin_bottom(6);

        // --- Badge ---
        let badge = build_badge(entry.content_class);
        container.append(&badge);

        // --- Center: preview + source app (vertical) ---
        let center_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        center_box.set_hexpand(true);
        center_box.set_valign(gtk::Align::Center);

        if entry.content_class == ContentClass::Image {
            // Show thumbnail if available, otherwise a placeholder label.
            if let Some(ref thumb_data) = entry.thumbnail {
                if let Some(picture) = build_thumbnail(thumb_data) {
                    center_box.append(&picture);
                } else {
                    let fallback = gtk::Label::new(Some("[Image]"));
                    fallback.add_css_class("dim-label");
                    fallback.set_halign(gtk::Align::Start);
                    center_box.append(&fallback);
                }
            } else {
                let fallback = gtk::Label::new(Some("[Image]"));
                fallback.add_css_class("dim-label");
                fallback.set_halign(gtk::Align::Start);
                center_box.append(&fallback);
            }
        } else {
            // Text-based preview: up to 2 lines, ellipsized.
            let preview_text = entry
                .preview_text
                .as_deref()
                .unwrap_or("[No preview]");
            let preview = gtk::Label::new(Some(preview_text));
            preview.set_halign(gtk::Align::Start);
            preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
            preview.set_lines(2);
            preview.set_max_width_chars(60);
            preview.set_wrap(true);
            preview.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            if entry.content_class == ContentClass::Text {
                preview.add_css_class("monospace");
            }
            center_box.append(&preview);
        }

        // Source app (muted, small).
        if let Some(ref app) = entry.source_app {
            let source_label = gtk::Label::new(Some(&format!("\u{2500}\u{2500} {app}")));
            source_label.add_css_class("dim-label");
            source_label.add_css_class("caption");
            source_label.set_halign(gtk::Align::Start);
            center_box.append(&source_label);
        }

        container.append(&center_box);

        // --- Right side: timestamp + pin (vertical) ---
        let right_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        right_box.set_valign(gtk::Align::Center);
        right_box.set_halign(gtk::Align::End);

        let time_str = format_relative_time(entry.last_seen_at);
        let time_label = gtk::Label::new(Some(&time_str));
        time_label.add_css_class("dim-label");
        time_label.add_css_class("caption");
        right_box.append(&time_label);

        if entry.pinned {
            let pin_icon = gtk::Image::from_icon_name("view-pin-symbolic");
            pin_icon.set_pixel_size(16);
            pin_icon.set_halign(gtk::Align::End);
            right_box.append(&pin_icon);
        }

        container.append(&right_box);

        Self {
            container,
            entry: entry.clone(),
        }
    }
}

/// Build the colored content-class badge.
fn build_badge(class: ContentClass) -> gtk::Label {
    let (text, css_class) = match class {
        ContentClass::Text => ("TXT", "badge-text"),
        ContentClass::RichText => ("HTML", "badge-richtext"),
        ContentClass::Image => ("IMG", "badge-image"),
        ContentClass::Files => ("FILE", "badge-files"),
        ContentClass::Url => ("URL", "badge-url"),
    };

    let label = gtk::Label::new(Some(text));
    label.add_css_class(css_class);
    label.set_valign(gtk::Align::Center);
    label.set_width_chars(5);
    label
}

/// Try to build a small thumbnail from PNG/JPEG bytes.
fn build_thumbnail(data: &[u8]) -> Option<gtk::Picture> {
    let bytes = glib::Bytes::from(data);
    let stream = gtk4::gio::MemoryInputStream::from_bytes(&bytes);
    let pixbuf = gdk::gdk_pixbuf::Pixbuf::from_stream(
        &stream,
        gtk4::gio::Cancellable::NONE,
    )
    .ok()?;

    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    // Constrain to a reasonable size.
    picture.set_size_request(120, 90);
    picture.set_halign(gtk::Align::Start);
    Some(picture)
}

/// Format a Unix-millis timestamp into a human-readable relative string.
pub fn format_relative_time(millis: i64) -> String {
    let now_ms = glib::DateTime::now_local()
        .map(|dt| dt.to_unix() * 1000)
        .unwrap_or(0);

    if now_ms == 0 || millis == 0 {
        return "just now".to_string();
    }

    let diff_secs = (now_ms - millis) / 1000;
    if diff_secs < 0 {
        return "just now".to_string();
    }

    let diff = diff_secs as u64;
    match diff {
        0..=59 => "just now".to_string(),
        60..=3599 => {
            let m = diff / 60;
            if m == 1 { "1m".to_string() } else { format!("{m}m") }
        }
        3600..=86399 => {
            let h = diff / 3600;
            if h == 1 { "1h".to_string() } else { format!("{h}h") }
        }
        86400..=2591999 => {
            let d = diff / 86400;
            if d == 1 { "1d".to_string() } else { format!("{d}d") }
        }
        _ => {
            let w = diff / 604800;
            if w == 1 { "1w".to_string() } else { format!("{w}w") }
        }
    }
}
