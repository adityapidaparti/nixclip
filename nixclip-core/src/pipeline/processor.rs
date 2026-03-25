//! Content processing — transforms raw clipboard MIME payloads into a
//! [`ProcessedEntry`].

use tracing::{debug, warn};

use crate::{ContentClass, EntryMetadata, MimePayload, ProcessedEntry};
use crate::error::{NixClipError, Result};
use super::classifier;

/// Maximum number of *characters* (not bytes) kept in `preview_text`.
const PREVIEW_CHAR_LIMIT: usize = 32_768;

// ---------------------------------------------------------------------------
// ContentProcessor
// ---------------------------------------------------------------------------

pub struct ContentProcessor;

impl ContentProcessor {
    /// Process a set of MIME payloads into a [`ProcessedEntry`].
    ///
    /// Classification is performed with [`classifier::classify_with_content`]
    /// using the `text/plain` payload (if present) to allow URL promotion.
    /// Each content class is then dispatched to a dedicated internal handler.
    pub fn process(
        offers: Vec<MimePayload>,
        source_app: Option<String>,
    ) -> Result<ProcessedEntry> {
        let offered_mimes: Vec<String> = offers.iter().map(|p| p.mime.clone()).collect();

        // Peek at plain text content for URL promotion.
        let plain_text_preview = offers
            .iter()
            .find(|p| p.mime == "text/plain")
            .map(|p| String::from_utf8_lossy(&p.data).into_owned());

        let content_class = classifier::classify_with_content(
            &offered_mimes,
            plain_text_preview.as_deref(),
        )
        .ok_or_else(|| {
            NixClipError::Pipeline("no recognised MIME type in clipboard offer".to_string())
        })?;

        debug!(%content_class, "classified clipboard content");

        let entry = match content_class {
            ContentClass::Image => Self::process_image(&offers)?,
            ContentClass::Files => Self::process_files(&offers)?,
            ContentClass::RichText => Self::process_richtext(&offers)?,
            ContentClass::Url => Self::process_url(&offers)?,
            ContentClass::Text => Self::process_text(&offers)?,
        };

        // Attach source_app (stored by caller in NewEntry, not in ProcessedEntry
        // directly, but we expose it via the public API for completeness).
        let _ = source_app; // used by caller when building NewEntry
        Ok(entry)
    }

    // -----------------------------------------------------------------------
    // Per-class handlers
    // -----------------------------------------------------------------------

    /// Decode `text/plain` bytes as UTF-8 (lossy), truncate to
    /// [`PREVIEW_CHAR_LIMIT`] *characters*, and hash the raw bytes.
    fn process_text(offers: &[MimePayload]) -> Result<ProcessedEntry> {
        let payload = offers
            .iter()
            .find(|p| p.mime == "text/plain")
            .ok_or_else(|| {
                NixClipError::Pipeline("text class but no text/plain payload found".to_string())
            })?;
        let text = String::from_utf8_lossy(&payload.data);
        let preview_text = truncate_chars(&text, PREVIEW_CHAR_LIMIT);
        let canonical_hash = *blake3::hash(&payload.data).as_bytes();

        Ok(ProcessedEntry {
            content_class: ContentClass::Text,
            preview_text: Some(preview_text),
            canonical_hash,
            representations: offers.to_vec(),
            thumbnail: None,
            metadata: EntryMetadata::default(),
        })
    }

    /// Same as [`process_text`] but also extracts the domain from the URL
    /// and stores it in [`EntryMetadata::url_domain`].
    fn process_url(offers: &[MimePayload]) -> Result<ProcessedEntry> {
        let payload = offers
            .iter()
            .find(|p| p.mime == "text/plain")
            .ok_or_else(|| {
                NixClipError::Pipeline("url class but no text/plain payload found".to_string())
            })?;
        let text = String::from_utf8_lossy(&payload.data);
        let trimmed = text.trim();
        let preview_text = truncate_chars(trimmed, PREVIEW_CHAR_LIMIT);
        let canonical_hash = *blake3::hash(&payload.data).as_bytes();

        let url_domain = extract_domain(trimmed);
        debug!(domain = ?url_domain, "extracted URL domain");

        Ok(ProcessedEntry {
            content_class: ContentClass::Url,
            preview_text: Some(preview_text),
            canonical_hash,
            representations: offers.to_vec(),
            thumbnail: None,
            metadata: EntryMetadata {
                url_domain,
                ..Default::default()
            },
        })
    }

    /// Process `text/html` + `text/plain` payloads.
    ///
    /// - `preview_text` = plain-text payload (preferred) or HTML with tags stripped.
    /// - `canonical_hash` = BLAKE3 of the HTML bytes.
    /// - `representations` = [html, plain] (in that order).
    fn process_richtext(offers: &[MimePayload]) -> Result<ProcessedEntry> {
        let html_payload = offers
            .iter()
            .find(|p| p.mime == "text/html")
            .ok_or_else(|| {
                NixClipError::Pipeline("richtext: missing text/html payload".to_string())
            })?;

        let plain_payload = offers.iter().find(|p| p.mime == "text/plain");

        let preview_text = if let Some(plain) = plain_payload {
            let t = String::from_utf8_lossy(&plain.data);
            truncate_chars(t.trim(), PREVIEW_CHAR_LIMIT)
        } else {
            let html = String::from_utf8_lossy(&html_payload.data);
            let stripped = strip_html_tags(&html);
            truncate_chars(stripped.trim(), PREVIEW_CHAR_LIMIT)
        };

        let canonical_hash = *blake3::hash(&html_payload.data).as_bytes();

        let mut representations = vec![html_payload.clone()];
        if let Some(plain) = plain_payload {
            representations.push(plain.clone());
        }

        Ok(ProcessedEntry {
            content_class: ContentClass::RichText,
            preview_text: Some(preview_text),
            canonical_hash,
            representations,
            thumbnail: None,
            metadata: EntryMetadata::default(),
        })
    }

    /// Decode the image, generate a 64×64 PNG thumbnail, and record dimensions.
    ///
    /// If decoding fails, the error is logged and processing continues without
    /// a thumbnail. The raw bytes are always stored.
    fn process_image(offers: &[MimePayload]) -> Result<ProcessedEntry> {
        use image::ImageFormat;

        let payload = offers
            .iter()
            .find(|p| p.mime == "image/png")
            .or_else(|| offers.iter().find(|p| p.mime == "image/jpeg"))
            .ok_or_else(|| {
                NixClipError::Pipeline("image MIME listed but no payload found".to_string())
            })?;

        // Determine image format from MIME type.
        let format = match payload.mime.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            other => {
                return Err(NixClipError::Image(format!(
                    "unsupported image MIME type: {other}"
                )))
            }
        };

        let canonical_hash = *blake3::hash(&payload.data).as_bytes();

        // Attempt to decode; failures are non-fatal — we still store the raw bytes.
        let (thumbnail, image_dimensions) =
            match Self::decode_and_thumbnail(&payload.data, format) {
                Ok((thumb, dims)) => (Some(thumb), Some(dims)),
                Err(e) => {
                    warn!(error = %e, "image decode failed; storing raw bytes without thumbnail");
                    (None, None)
                }
            };

        Ok(ProcessedEntry {
            content_class: ContentClass::Image,
            preview_text: None,
            canonical_hash,
            representations: offers.to_vec(),
            thumbnail,
            metadata: EntryMetadata {
                image_dimensions,
                ..Default::default()
            },
        })
    }

    /// Decode image bytes and produce a 64×64 PNG thumbnail.
    ///
    /// Separated so that the caller can catch errors without panicking.
    fn decode_and_thumbnail(
        data: &[u8],
        format: image::ImageFormat,
    ) -> std::result::Result<(Vec<u8>, (u32, u32)), NixClipError> {
        use image::imageops::FilterType;
        use image::ImageEncoder;
        use image::codecs::png::PngEncoder;
        use std::io::Cursor;

        let img = image::load_from_memory_with_format(data, format)
            .map_err(|e| NixClipError::Image(format!("failed to decode image: {e}")))?;

        let (width, height) = (img.width(), img.height());

        let thumbnail = image::imageops::resize(&img, 64, 64, FilterType::Lanczos3);

        // Encode thumbnail as PNG into a Vec<u8>.
        let mut png_bytes: Vec<u8> = Vec::new();
        let encoder = PngEncoder::new(Cursor::new(&mut png_bytes));
        encoder
            .write_image(
                thumbnail.as_raw(),
                64,
                64,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| NixClipError::Image(format!("failed to encode thumbnail: {e}")))?;

        Ok((png_bytes, (width, height)))
    }

    /// Parse a `text/uri-list` (RFC 2483): one URI per line, `#` lines are
    /// comments. Extract `file://` URIs and use their filename as preview.
    fn process_files(offers: &[MimePayload]) -> Result<ProcessedEntry> {
        let payload = offers
            .iter()
            .find(|p| p.mime == "text/uri-list")
            .or_else(|| offers.iter().find(|p| p.mime == "x-special/gnome-copied-files"))
            .ok_or_else(|| {
                NixClipError::Pipeline("files MIME listed but no payload found".to_string())
            })?;
        let text = String::from_utf8_lossy(&payload.data);

        let uris: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();

        let filenames: Vec<String> = uris
            .iter()
            .map(|uri| extract_filename_from_uri(uri))
            .collect();

        let file_count = filenames.len();
        let preview_text = truncate_chars(&filenames.join("\n"), PREVIEW_CHAR_LIMIT);

        let canonical_hash = *blake3::hash(&payload.data).as_bytes();

        Ok(ProcessedEntry {
            content_class: ContentClass::Files,
            preview_text: Some(preview_text),
            canonical_hash,
            representations: offers.to_vec(),
            thumbnail: None,
            metadata: EntryMetadata {
                file_count: Some(file_count),
                ..Default::default()
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `limit` Unicode scalar values (chars).
///
/// Uses [`str::char_indices`] to avoid splitting multi-byte sequences.
fn truncate_chars(s: &str, limit: usize) -> String {
    match s.char_indices().nth(limit) {
        Some((byte_idx, _)) => s[..byte_idx].to_string(),
        None => s.to_string(),
    }
}

/// Extract the domain (host) from a URL string.
///
/// Handles both scheme-prefixed URLs (`http://example.com/path`) and bare
/// domain-style strings (`example.com/path`).
fn extract_domain(url: &str) -> Option<String> {
    // Strip scheme if present.
    let without_scheme = if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else {
        url
    };

    // The host is everything up to the first `/`, `?`, `#`, or `:` (port).
    let host = without_scheme
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or(without_scheme);

    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Extract a human-readable filename from a URI.
///
/// For `file://` URIs the path component's last segment is returned (percent-
/// decoded where possible). For other schemes the full URI is returned as-is.
fn extract_filename_from_uri(uri: &str) -> String {
    let path = if let Some(rest) = uri.strip_prefix("file://") {
        // Strip optional host (e.g. `file://localhost/home/...`).
        let path_start = if rest.starts_with('/') {
            rest
        } else {
            rest.split_once('/').map(|(_, path)| path).unwrap_or(rest)
        };
        path_start
    } else {
        return uri.to_string();
    };

    // Last non-empty path segment.
    let filename = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path);

    // Best-effort percent-decode.
    percent_decode(filename)
}

/// Minimal percent-decoding for filenames extracted from `file://` URIs.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                let hex = format!("{h1}{h2}");
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    // Only replace safe ASCII bytes.
                    if byte.is_ascii() {
                        out.push(byte as char);
                        continue;
                    }
                }
                // Fall back: push the original characters.
                out.push('%');
                out.push(h1);
                out.push(h2);
            } else {
                out.push('%');
                if let Some(h) = h1 { out.push(h); }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Strip HTML tags using a simple state machine — no external parser needed.
///
/// Collapses runs of whitespace and trims the result.
pub(crate) fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_space = false;

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                // Emit a space to separate words where tags appear between them.
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            '>' => {
                in_tag = false;
            }
            _ if in_tag => {
                // Skip tag contents.
            }
            c if c.is_whitespace() => {
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            c => {
                out.push(c);
                last_was_space = false;
            }
        }
    }

    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_payload(mime: &str, data: &[u8]) -> MimePayload {
        MimePayload {
            mime: mime.to_string(),
            data: data.to_vec(),
        }
    }

    // --- truncate_chars ---

    #[test]
    fn truncate_ascii() {
        let s = "hello world";
        assert_eq!(truncate_chars(s, 5), "hello");
        assert_eq!(truncate_chars(s, 100), "hello world");
    }

    #[test]
    fn truncate_multibyte() {
        // "é" is 2 bytes but 1 char.
        let s = "héllo";
        assert_eq!(truncate_chars(s, 3), "hél");
    }

    // --- extract_domain ---

    #[test]
    fn domain_with_scheme() {
        assert_eq!(
            extract_domain("https://www.example.com/foo?bar=1"),
            Some("www.example.com".to_string())
        );
    }

    #[test]
    fn domain_bare() {
        assert_eq!(
            extract_domain("example.com/foo"),
            Some("example.com".to_string())
        );
    }

    // --- extract_filename_from_uri ---

    #[test]
    fn filename_from_file_uri() {
        assert_eq!(
            extract_filename_from_uri("file:///home/user/documents/report.pdf"),
            "report.pdf"
        );
    }

    #[test]
    fn filename_percent_decoded() {
        assert_eq!(
            extract_filename_from_uri("file:///home/user/my%20file.txt"),
            "my file.txt"
        );
    }

    #[test]
    fn filename_non_file_uri() {
        let uri = "https://example.com/file.txt";
        assert_eq!(extract_filename_from_uri(uri), uri);
    }

    // --- strip_html_tags ---

    #[test]
    fn strip_basic_html() {
        assert_eq!(strip_html_tags("<b>Hello</b> <i>World</i>"), "Hello World");
    }

    #[test]
    fn strip_preserves_content() {
        assert_eq!(
            strip_html_tags("<p>Some <strong>important</strong> text.</p>"),
            "Some important text."
        );
    }

    // --- process_text ---

    #[test]
    fn process_text_basic() {
        let payload = make_payload("text/plain", b"hello world");
        let entry = ContentProcessor::process(vec![payload], None).unwrap();
        assert_eq!(entry.content_class, ContentClass::Text);
        assert_eq!(entry.preview_text.as_deref(), Some("hello world"));
        assert_eq!(entry.canonical_hash, *blake3::hash(b"hello world").as_bytes());
    }

    #[test]
    fn process_text_truncates_at_char_boundary() {
        // 32769 ASCII chars — should be truncated to 32768.
        let long = "a".repeat(PREVIEW_CHAR_LIMIT + 1);
        let payload = make_payload("text/plain", long.as_bytes());
        let entry = ContentProcessor::process(vec![payload], None).unwrap();
        assert_eq!(
            entry.preview_text.as_ref().unwrap().len(),
            PREVIEW_CHAR_LIMIT
        );
    }

    // --- process_url ---

    #[test]
    fn process_url_extracts_domain() {
        let payload = make_payload("text/plain", b"https://example.com/path?q=1");
        let entry = ContentProcessor::process(vec![payload], None).unwrap();
        assert_eq!(entry.content_class, ContentClass::Url);
        assert_eq!(
            entry.metadata.url_domain.as_deref(),
            Some("example.com")
        );
    }

    // --- process_files ---

    #[test]
    fn process_files_basic() {
        let uri_list = b"# comment\nfile:///home/user/a.txt\nfile:///home/user/b.pdf\n";
        let payload = make_payload("text/uri-list", uri_list);
        let entry = ContentProcessor::process(vec![payload], None).unwrap();
        assert_eq!(entry.content_class, ContentClass::Files);
        assert_eq!(entry.metadata.file_count, Some(2));
        let preview = entry.preview_text.unwrap();
        assert!(preview.contains("a.txt"));
        assert!(preview.contains("b.pdf"));
    }

    // --- no recognised MIME ---

    #[test]
    fn process_unknown_mime_errors() {
        let payload = make_payload("application/octet-stream", b"\x00\x01");
        assert!(ContentProcessor::process(vec![payload], None).is_err());
    }
}
