use nixclip_core::config::IgnoreConfig;
/// Integration tests for the pipeline: classifier, ContentProcessor, PrivacyFilter.
use nixclip_core::pipeline::classifier;
use nixclip_core::pipeline::{ContentProcessor, PrivacyFilter};
use nixclip_core::{ContentClass, MimePayload};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn mimes(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn make_payload(mime: &str, data: &[u8]) -> MimePayload {
    MimePayload {
        mime: mime.to_string(),
        data: data.to_vec(),
    }
}

// ===========================================================================
// classifier::classify
// ===========================================================================

#[test]
fn classify_image_png_wins_over_text() {
    let result = classifier::classify(&mimes(&["image/png", "text/plain"]));
    assert_eq!(result, Some(ContentClass::Image));
}

#[test]
fn classify_image_jpeg_alone() {
    let result = classifier::classify(&mimes(&["image/jpeg"]));
    assert_eq!(result, Some(ContentClass::Image));
}

#[test]
fn classify_files_uri_list() {
    let result = classifier::classify(&mimes(&["text/uri-list"]));
    assert_eq!(result, Some(ContentClass::Files));
}

#[test]
fn classify_files_gnome_copied() {
    let result = classifier::classify(&mimes(&["x-special/gnome-copied-files"]));
    assert_eq!(result, Some(ContentClass::Files));
}

#[test]
fn classify_richtext_html_and_plain() {
    let result = classifier::classify(&mimes(&["text/html", "text/plain"]));
    assert_eq!(result, Some(ContentClass::RichText));
}

#[test]
fn classify_html_only_is_none() {
    // html without plain text is unclassifiable.
    let result = classifier::classify(&mimes(&["text/html"]));
    assert_eq!(result, None);
}

#[test]
fn classify_plain_only_is_text() {
    let result = classifier::classify(&mimes(&["text/plain"]));
    assert_eq!(result, Some(ContentClass::Text));
}

#[test]
fn classify_empty_mimes_is_none() {
    let result = classifier::classify(&[]);
    assert_eq!(result, None);
}

#[test]
fn classify_unknown_mime_is_none() {
    let result = classifier::classify(&mimes(&["application/octet-stream"]));
    assert_eq!(result, None);
}

// ---------------------------------------------------------------------------
// Priority ordering
// ---------------------------------------------------------------------------

#[test]
fn classify_image_beats_files() {
    // image/png should win over text/uri-list.
    let result = classifier::classify(&mimes(&["image/png", "text/uri-list"]));
    assert_eq!(result, Some(ContentClass::Image));
}

#[test]
fn classify_files_beats_richtext() {
    let result = classifier::classify(&mimes(&["text/uri-list", "text/html", "text/plain"]));
    assert_eq!(result, Some(ContentClass::Files));
}

// ===========================================================================
// classifier::classify_with_content
// ===========================================================================

#[test]
fn classify_with_content_promotes_http_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("https://example.com/path"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

#[test]
fn classify_with_content_promotes_bare_domain() {
    let result = classifier::classify_with_content(&mimes(&["text/plain"]), Some("example.com"));
    assert_eq!(result, Some(ContentClass::Url));
}

#[test]
fn classify_with_content_no_promotion_for_whitespace() {
    let result = classifier::classify_with_content(&mimes(&["text/plain"]), Some("hello world"));
    assert_eq!(result, Some(ContentClass::Text));
}

#[test]
fn classify_with_content_no_promotion_when_no_text() {
    let result = classifier::classify_with_content(&mimes(&["text/plain"]), None);
    assert_eq!(result, Some(ContentClass::Text));
}

#[test]
fn classify_with_content_image_beats_url_content() {
    // Even if the plain text looks like a URL, image/png wins.
    let result = classifier::classify_with_content(
        &mimes(&["image/png", "text/plain"]),
        Some("https://example.com"),
    );
    assert_eq!(result, Some(ContentClass::Image));
}

#[test]
fn classify_with_content_richtext_not_promoted_to_url() {
    // RichText (html+plain) should not be promoted to Url even with url content.
    let result = classifier::classify_with_content(
        &mimes(&["text/html", "text/plain"]),
        Some("https://example.com"),
    );
    assert_eq!(result, Some(ContentClass::RichText));
}

// ===========================================================================
// ContentProcessor::process
// ===========================================================================

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[test]
fn processor_text_basic() {
    let payload = make_payload("text/plain", b"hello world");
    let entry = ContentProcessor::process(vec![payload], None).expect("process");
    assert_eq!(entry.content_class, ContentClass::Text);
    assert_eq!(entry.preview_text.as_deref(), Some("hello world"));
    // Canonical hash should equal blake3 of the raw bytes.
    let expected_hash = *blake3::hash(b"hello world").as_bytes();
    assert_eq!(entry.canonical_hash, expected_hash);
}

#[test]
fn processor_text_truncates_long_content() {
    // Build a string > 32768 chars and verify truncation.
    let long = "a".repeat(33_000);
    let payload = make_payload("text/plain", long.as_bytes());
    let entry = ContentProcessor::process(vec![payload], None).expect("process");
    let preview_len = entry.preview_text.as_ref().unwrap().chars().count();
    assert!(
        preview_len <= 32_768,
        "preview_text should be at most 32768 chars, got {preview_len}"
    );
}

#[test]
fn processor_text_preserves_source_app() {
    // source_app is stored by caller; processor receives and ignores it —
    // but the call must not fail.
    let payload = make_payload("text/plain", b"test");
    let entry = ContentProcessor::process(vec![payload], Some("org.test.App".to_string()))
        .expect("process with source_app");
    assert_eq!(entry.content_class, ContentClass::Text);
}

// ---------------------------------------------------------------------------
// URL
// ---------------------------------------------------------------------------

#[test]
fn processor_url_extracts_domain_https() {
    let payload = make_payload("text/plain", b"https://www.rust-lang.org/tools/install");
    let entry = ContentProcessor::process(vec![payload], None).expect("process");
    assert_eq!(entry.content_class, ContentClass::Url);
    assert_eq!(
        entry.metadata.url_domain.as_deref(),
        Some("www.rust-lang.org")
    );
}

#[test]
fn processor_url_extracts_domain_http() {
    let payload = make_payload("text/plain", b"http://example.com/page");
    let entry = ContentProcessor::process(vec![payload], None).expect("process");
    assert_eq!(entry.content_class, ContentClass::Url);
    assert_eq!(entry.metadata.url_domain.as_deref(), Some("example.com"));
}

#[test]
fn processor_url_bare_domain() {
    let payload = make_payload("text/plain", b"example.com/path");
    let entry = ContentProcessor::process(vec![payload], None).expect("process");
    assert_eq!(entry.content_class, ContentClass::Url);
    assert!(entry.metadata.url_domain.is_some());
}

// ---------------------------------------------------------------------------
// RichText
// ---------------------------------------------------------------------------

#[test]
fn processor_richtext_uses_plain_preview() {
    let html = make_payload("text/html", b"<b>Hello</b>");
    let plain = make_payload("text/plain", b"Hello");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    assert_eq!(entry.content_class, ContentClass::RichText);
    // Preview should come from the plain-text payload.
    assert_eq!(entry.preview_text.as_deref(), Some("Hello"));
}

#[test]
fn processor_richtext_strips_html_when_no_plain() {
    let html = make_payload("text/html", b"<p>World</p>");
    let entry = ContentProcessor::process(vec![html], None);
    // Without text/plain, richtext classification fails (both html+plain required).
    // So this should return an error or classify differently.
    // classify_with_content requires html+plain for RichText, else None → error.
    assert!(
        entry.is_err(),
        "html without plain should not be classifiable as RichText"
    );
}

#[test]
fn processor_richtext_canonical_hash_from_html() {
    let html_bytes = b"<b>Canonical</b>";
    let html = make_payload("text/html", html_bytes);
    let plain = make_payload("text/plain", b"Canonical");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    let expected = *blake3::hash(html_bytes).as_bytes();
    assert_eq!(entry.canonical_hash, expected);
}

// ---------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------

/// Build a minimal valid 1×1 RGBA PNG as a byte vector.
fn make_minimal_png() -> Vec<u8> {
    use image::codecs::png::PngEncoder;
    use image::ImageEncoder;
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;

    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(1, 1, Rgba([255, 0, 128, 255]));
    let mut buf = Vec::new();
    let encoder = PngEncoder::new(Cursor::new(&mut buf));
    encoder
        .write_image(img.as_raw(), 1, 1, image::ExtendedColorType::Rgba8)
        .expect("encode test PNG");
    buf
}

#[test]
fn processor_image_png_generates_thumbnail() {
    let png_bytes = make_minimal_png();
    let payload = make_payload("image/png", &png_bytes);
    let entry = ContentProcessor::process(vec![payload], None).expect("process image");
    assert_eq!(entry.content_class, ContentClass::Image);
    assert!(
        entry.thumbnail.is_some(),
        "thumbnail should be generated for valid PNG"
    );
    assert!(entry.metadata.image_dimensions.is_some());
}

#[test]
fn processor_image_png_records_dimensions() {
    let png_bytes = make_minimal_png();
    let payload = make_payload("image/png", &png_bytes);
    let entry = ContentProcessor::process(vec![payload], None).expect("process image");
    let dims = entry.metadata.image_dimensions.expect("image dimensions");
    // Our test image is 1×1.
    assert_eq!(dims, (1, 1));
}

#[test]
fn processor_image_invalid_data_stores_raw_without_thumbnail() {
    // Garbage bytes — decoding fails but the processor should still succeed
    // by logging a warning and producing no thumbnail.
    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02];
    let payload = make_payload("image/png", &garbage);
    let entry = ContentProcessor::process(vec![payload], None).expect("process bad image");
    assert_eq!(entry.content_class, ContentClass::Image);
    assert!(
        entry.thumbnail.is_none(),
        "invalid image should produce no thumbnail"
    );
    assert!(entry.metadata.image_dimensions.is_none());
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[test]
fn processor_files_basic_uri_list() {
    let uri_list = b"# copied files\nfile:///home/user/document.pdf\nfile:///home/user/image.png\n";
    let payload = make_payload("text/uri-list", uri_list);
    let entry = ContentProcessor::process(vec![payload], None).expect("process files");
    assert_eq!(entry.content_class, ContentClass::Files);
    assert_eq!(entry.metadata.file_count, Some(2));
    let preview = entry.preview_text.unwrap();
    assert!(preview.contains("document.pdf"));
    assert!(preview.contains("image.png"));
}

#[test]
fn processor_files_single_file() {
    let uri_list = b"file:///tmp/single.txt\n";
    let payload = make_payload("text/uri-list", uri_list);
    let entry = ContentProcessor::process(vec![payload], None).expect("process single file");
    assert_eq!(entry.metadata.file_count, Some(1));
}

#[test]
fn processor_files_percent_decoded_name() {
    let uri_list = b"file:///home/user/my%20document.txt\n";
    let payload = make_payload("text/uri-list", uri_list);
    let entry = ContentProcessor::process(vec![payload], None).expect("process encoded file");
    let preview = entry.preview_text.unwrap();
    assert!(
        preview.contains("my document.txt"),
        "filename should be percent-decoded"
    );
}

#[test]
fn processor_unknown_mime_returns_error() {
    let payload = make_payload("application/octet-stream", b"\x00\xFF");
    let result = ContentProcessor::process(vec![payload], None);
    assert!(result.is_err(), "unrecognised MIME should return an error");
}

// ===========================================================================
// PrivacyFilter
// ===========================================================================

fn make_filter(apps: &[&str], patterns: &[&str], respect_hints: bool) -> PrivacyFilter {
    let config = IgnoreConfig {
        apps: apps.iter().map(|s| s.to_string()).collect(),
        patterns: patterns.iter().map(|s| s.to_string()).collect(),
        respect_sensitive_hints: respect_hints,
    };
    PrivacyFilter::new(&config).expect("build filter")
}

// ---------------------------------------------------------------------------
// Ignored apps
// ---------------------------------------------------------------------------

#[test]
fn privacy_rejects_keepassxc_by_default() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let result = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &mimes(&["text/plain"]),
        Some("secret password"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_rejects_1password_by_default() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let result = filter.check(
        Some("com.1password.1Password"),
        &mimes(&["text/plain"]),
        Some("password123"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_rejects_custom_ignored_app() {
    let filter = make_filter(&["com.secret.App"], &[], true);
    let result = filter.check(
        Some("com.secret.App"),
        &mimes(&["text/plain"]),
        Some("data"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_case_insensitive_app_match() {
    let filter = make_filter(&["KeePassXC"], &[], true);
    // App id uses different casing.
    let result = filter.check(
        Some("org.keepassxc.keepassxc"),
        &mimes(&["text/plain"]),
        Some("secret"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_allows_unrelated_app() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let result = filter.check(
        Some("org.mozilla.firefox"),
        &mimes(&["text/plain"]),
        Some("regular text"),
    );
    assert_eq!(result, nixclip_core::pipeline::privacy::FilterResult::Allow);
}

// ---------------------------------------------------------------------------
// Sensitive MIMEs
// ---------------------------------------------------------------------------

#[test]
fn privacy_rejects_kde_password_hint() {
    let filter = make_filter(&[], &[], true);
    let result = filter.check(
        Some("org.mozilla.firefox"),
        &mimes(&["text/plain", "x-kde-passwordManagerHint"]),
        Some("text"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_rejects_nspasteboard_concealed() {
    let filter = make_filter(&[], &[], true);
    let result = filter.check(None, &mimes(&["org.nspasteboard.ConcealedType"]), None);
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_rejects_nspasteboard_transient() {
    let filter = make_filter(&[], &[], true);
    let result = filter.check(None, &mimes(&["org.nspasteboard.TransientType"]), None);
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_rejects_password_substring_in_mime() {
    let filter = make_filter(&[], &[], true);
    let result = filter.check(None, &mimes(&["application/x-password-data"]), None);
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_allows_sensitive_mimes_when_hints_disabled() {
    let filter = make_filter(&[], &[], false);
    let result = filter.check(None, &mimes(&["x-kde-passwordManagerHint"]), None);
    assert_eq!(result, nixclip_core::pipeline::privacy::FilterResult::Allow);
}

// ---------------------------------------------------------------------------
// Regex patterns
// ---------------------------------------------------------------------------

#[test]
fn privacy_ephemeral_on_openai_key_pattern() {
    // Default patterns include `^sk-[a-zA-Z0-9]{48}`.
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let key = format!("sk-{}", "A".repeat(48));
    let result = filter.check(None, &mimes(&["text/plain"]), Some(&key));
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Ephemeral
    );
}

#[test]
fn privacy_ephemeral_on_github_token_pattern() {
    // Default patterns include `^ghp_[a-zA-Z0-9]{36}`.
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let token = format!("ghp_{}", "B".repeat(36));
    let result = filter.check(None, &mimes(&["text/plain"]), Some(&token));
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Ephemeral
    );
}

#[test]
fn privacy_allows_normal_text() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("default filter");
    let result = filter.check(None, &mimes(&["text/plain"]), Some("just a normal note"));
    assert_eq!(result, nixclip_core::pipeline::privacy::FilterResult::Allow);
}

#[test]
fn privacy_invalid_regex_returns_error() {
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec!["[invalid".to_string()],
        respect_sensitive_hints: true,
    };
    assert!(
        PrivacyFilter::new(&config).is_err(),
        "invalid regex should cause construction failure"
    );
}

// ---------------------------------------------------------------------------
// Priority ordering
// ---------------------------------------------------------------------------

#[test]
fn privacy_app_rejection_before_sensitive_mime() {
    // App is ignored — should Reject even though mime alone would also Reject.
    let filter = make_filter(&["keepassxc"], &[], true);
    let result = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &mimes(&["x-kde-passwordManagerHint"]),
        Some("secret"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_app_rejection_before_pattern_match() {
    let filter = make_filter(&["keepassxc"], &[r"secret"], true);
    let result = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &mimes(&["text/plain"]),
        Some("secret"),
    );
    // Should be Reject (app match) not Ephemeral (pattern match).
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}

#[test]
fn privacy_no_source_app_still_filters_mimes() {
    let filter = make_filter(&[], &[], true);
    let result = filter.check(
        None,
        &mimes(&["org.nspasteboard.ConcealedType"]),
        Some("text"),
    );
    assert_eq!(
        result,
        nixclip_core::pipeline::privacy::FilterResult::Reject
    );
}
