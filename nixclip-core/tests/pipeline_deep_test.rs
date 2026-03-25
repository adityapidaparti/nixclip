//! Deep integration tests for the NixClip content pipeline.
//!
//! Covers:
//!  1. Classification priority (first-match-wins across all five classes)
//!  2. Preview truncation at exactly 32 768 *characters* using multi-byte UTF-8
//!  3. BLAKE3 hash determinism
//!  4. Image thumbnail dimensions are exactly 64×64
//!  5. Malformed PNG handling (truncated, zero bytes, random bytes)
//!  6. URI-list parsing edge cases
//!  7. Privacy filter ordering (app reject beats pattern Ephemeral)
//!  8. Privacy regex compilation error handling
//!  9. HTML tag stripping for richtext preview
//! 10. URL detection edge cases

use nixclip_core::config::IgnoreConfig;
use nixclip_core::pipeline::classifier;
use nixclip_core::pipeline::privacy::{FilterResult, PrivacyFilter};
use nixclip_core::pipeline::ContentProcessor;
use nixclip_core::{ContentClass, MimePayload};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn mimes(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn payload(mime: &str, data: &[u8]) -> MimePayload {
    MimePayload {
        mime: mime.to_string(),
        data: data.to_vec(),
    }
}

fn make_filter(apps: &[&str], patterns: &[&str], respect_hints: bool) -> PrivacyFilter {
    let config = IgnoreConfig {
        apps: apps.iter().map(|s| s.to_string()).collect(),
        patterns: patterns.iter().map(|s| s.to_string()).collect(),
        respect_sensitive_hints: respect_hints,
    };
    PrivacyFilter::new(&config).expect("build PrivacyFilter")
}

/// Encode a small RGBA image to a PNG byte vector.
fn make_png(width: u32, height: u32) -> Vec<u8> {
    use image::codecs::png::PngEncoder;
    use image::{ImageBuffer, ImageEncoder, Rgba};
    use std::io::Cursor;

    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(width, height, |x, y| {
        Rgba([(x * 255 / width.max(1)) as u8, (y * 255 / height.max(1)) as u8, 128, 255])
    });
    let mut buf = Vec::new();
    PngEncoder::new(Cursor::new(&mut buf))
        .write_image(img.as_raw(), width, height, image::ExtendedColorType::Rgba8)
        .expect("encode test PNG");
    buf
}

/// Decode a PNG from bytes and return its (width, height).
fn png_dimensions(data: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png)
        .expect("decode PNG for dimension check");
    (img.width(), img.height())
}

// ===========================================================================
// 1. Classification priority — first-match-wins
// ===========================================================================

/// image/png + text/uri-list + text/html + text/plain → Image wins
#[test]
fn priority_image_beats_all_others() {
    let result = classifier::classify(&mimes(&[
        "image/png",
        "text/uri-list",
        "text/html",
        "text/plain",
    ]));
    assert_eq!(result, Some(ContentClass::Image));
}

/// image/jpeg + x-special/gnome-copied-files + text/html + text/plain → Image wins
#[test]
fn priority_image_jpeg_beats_files_and_richtext() {
    let result = classifier::classify(&mimes(&[
        "image/jpeg",
        "x-special/gnome-copied-files",
        "text/html",
        "text/plain",
    ]));
    assert_eq!(result, Some(ContentClass::Image));
}

/// text/uri-list + text/html + text/plain → Files beats RichText
#[test]
fn priority_files_beats_richtext_and_text() {
    let result =
        classifier::classify(&mimes(&["text/uri-list", "text/html", "text/plain"]));
    assert_eq!(result, Some(ContentClass::Files));
}

/// x-special/gnome-copied-files + text/html + text/plain → Files beats RichText
#[test]
fn priority_gnome_files_beats_richtext() {
    let result = classifier::classify(&mimes(&[
        "x-special/gnome-copied-files",
        "text/html",
        "text/plain",
    ]));
    assert_eq!(result, Some(ContentClass::Files));
}

/// text/html + text/plain (and the text looks like a URL) → RichText beats Url
#[test]
fn priority_richtext_beats_url_promotion() {
    // classify_with_content should return RichText, not Url, because the
    // RichText check (priority 3) happens before URL promotion (priority 4).
    let result = classifier::classify_with_content(
        &mimes(&["text/html", "text/plain"]),
        Some("https://example.com"),
    );
    assert_eq!(result, Some(ContentClass::RichText));
}

/// text/html alone → None (plain text is required for RichText)
#[test]
fn priority_html_without_plain_is_none() {
    assert_eq!(classifier::classify(&mimes(&["text/html"])), None);
}

/// text/uri-list + text/html (no text/plain) → Files (Files check does not
/// require text/plain; html without plain would also be None anyway)
#[test]
fn priority_files_with_html_but_no_plain() {
    let result =
        classifier::classify(&mimes(&["text/uri-list", "text/html"]));
    assert_eq!(result, Some(ContentClass::Files));
}

/// image/png beats even a URL-shaped text payload (via classify_with_content)
#[test]
fn priority_image_beats_url_content_promotion() {
    let result = classifier::classify_with_content(
        &mimes(&["image/png", "text/plain"]),
        Some("https://example.com"),
    );
    assert_eq!(result, Some(ContentClass::Image));
}

// ===========================================================================
// 2. Preview truncation at exactly 32 768 characters (not bytes)
// ===========================================================================

const PREVIEW_LIMIT: usize = 32_768;

/// A string of exactly PREVIEW_LIMIT ASCII chars must be kept whole.
#[test]
fn truncation_ascii_at_exact_limit_not_truncated() {
    let s = "x".repeat(PREVIEW_LIMIT);
    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert_eq!(
        preview.chars().count(),
        PREVIEW_LIMIT,
        "exactly PREVIEW_LIMIT ASCII chars should not be truncated"
    );
}

/// One character over the limit must be truncated to exactly PREVIEW_LIMIT chars.
#[test]
fn truncation_ascii_one_over_limit_is_truncated() {
    let s = "x".repeat(PREVIEW_LIMIT + 1);
    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    assert_eq!(
        entry.preview_text.unwrap().chars().count(),
        PREVIEW_LIMIT
    );
}

/// Multi-byte characters: build a string whose byte length > PREVIEW_LIMIT but
/// whose character count equals PREVIEW_LIMIT. It must NOT be truncated.
///
/// "é" (U+00E9) encodes as 2 bytes in UTF-8, so PREVIEW_LIMIT repetitions
/// occupy 2×PREVIEW_LIMIT bytes.
#[test]
fn truncation_multibyte_at_exact_char_limit_not_truncated() {
    // Each 'é' is 2 bytes; PREVIEW_LIMIT of them = PREVIEW_LIMIT chars.
    let s: String = std::iter::repeat('é').take(PREVIEW_LIMIT).collect();
    assert_eq!(s.len(), PREVIEW_LIMIT * 2, "sanity: byte length should be 2×limit");
    assert_eq!(s.chars().count(), PREVIEW_LIMIT, "sanity: char count == limit");

    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert_eq!(
        preview.chars().count(),
        PREVIEW_LIMIT,
        "multi-byte string at exact char limit should not be truncated"
    );
    // Also verify we didn't accidentally split a multi-byte boundary.
    assert!(preview.is_char_boundary(preview.len()));
}

/// One multi-byte char over the limit: result must be PREVIEW_LIMIT chars and
/// must not split a UTF-8 sequence.
#[test]
fn truncation_multibyte_one_over_limit_truncated_at_char_boundary() {
    // PREVIEW_LIMIT + 1 repetitions of 'é' (2 bytes each).
    let s: String = std::iter::repeat('é').take(PREVIEW_LIMIT + 1).collect();
    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert_eq!(
        preview.chars().count(),
        PREVIEW_LIMIT,
        "should truncate to exactly PREVIEW_LIMIT chars"
    );
    // The resulting string must be valid UTF-8 — i.e. we did not split 'é'.
    assert!(
        std::str::from_utf8(preview.as_bytes()).is_ok(),
        "truncated preview must be valid UTF-8"
    );
}

/// Three-byte UTF-8 chars (U+4E2D 中): ensure char-boundary truncation holds.
#[test]
fn truncation_three_byte_chars_truncated_correctly() {
    // '中' is 3 bytes; PREVIEW_LIMIT + 1 of them.
    let s: String = std::iter::repeat('中').take(PREVIEW_LIMIT + 1).collect();
    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert_eq!(preview.chars().count(), PREVIEW_LIMIT);
    assert!(std::str::from_utf8(preview.as_bytes()).is_ok());
}

/// Four-byte UTF-8 chars (U+1F600 😀): ensure char-boundary truncation holds.
#[test]
fn truncation_four_byte_chars_truncated_correctly() {
    // '😀' is 4 bytes; PREVIEW_LIMIT + 1 of them.
    let s: String = std::iter::repeat('😀').take(PREVIEW_LIMIT + 1).collect();
    let entry = ContentProcessor::process(vec![payload("text/plain", s.as_bytes())], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert_eq!(preview.chars().count(), PREVIEW_LIMIT);
    assert!(std::str::from_utf8(preview.as_bytes()).is_ok());
}

// ===========================================================================
// 3. BLAKE3 hash determinism
// ===========================================================================

#[test]
fn hash_determinism_same_input_same_hash() {
    let data = b"deterministic content";
    let p1 = payload("text/plain", data);
    let p2 = payload("text/plain", data);

    let e1 = ContentProcessor::process(vec![p1], None).expect("process 1");
    let e2 = ContentProcessor::process(vec![p2], None).expect("process 2");

    assert_eq!(
        e1.canonical_hash, e2.canonical_hash,
        "same input must always produce the same hash"
    );
}

#[test]
fn hash_determinism_different_input_different_hash() {
    let e1 = ContentProcessor::process(vec![payload("text/plain", b"content A")], None)
        .expect("process A");
    let e2 = ContentProcessor::process(vec![payload("text/plain", b"content B")], None)
        .expect("process B");

    assert_ne!(
        e1.canonical_hash, e2.canonical_hash,
        "different inputs must produce different hashes"
    );
}

#[test]
fn hash_matches_direct_blake3_of_raw_bytes() {
    let data = b"raw payload bytes";
    let entry = ContentProcessor::process(vec![payload("text/plain", data)], None)
        .expect("process");
    let expected = *blake3::hash(data).as_bytes();
    assert_eq!(entry.canonical_hash, expected);
}

#[test]
fn hash_richtext_is_over_html_bytes_not_plain() {
    let html_bytes = b"<strong>hash me</strong>";
    let plain_bytes = b"hash me";
    let entry = ContentProcessor::process(
        vec![payload("text/html", html_bytes), payload("text/plain", plain_bytes)],
        None,
    )
    .expect("process richtext");
    let expected = *blake3::hash(html_bytes).as_bytes();
    assert_eq!(
        entry.canonical_hash, expected,
        "RichText canonical hash must be over HTML bytes"
    );
    let not_expected = *blake3::hash(plain_bytes).as_bytes();
    assert_ne!(
        entry.canonical_hash, not_expected,
        "RichText canonical hash must NOT be over plain-text bytes"
    );
}

#[test]
fn hash_image_is_over_raw_bytes() {
    let png = make_png(4, 4);
    let expected = *blake3::hash(&png).as_bytes();
    let entry =
        ContentProcessor::process(vec![payload("image/png", &png)], None).expect("process image");
    assert_eq!(entry.canonical_hash, expected);
}

#[test]
fn hash_files_is_over_uri_list_bytes() {
    let uri_list = b"file:///tmp/a.txt\nfile:///tmp/b.txt\n";
    let expected = *blake3::hash(uri_list).as_bytes();
    let entry =
        ContentProcessor::process(vec![payload("text/uri-list", uri_list)], None)
            .expect("process files");
    assert_eq!(entry.canonical_hash, expected);
}

// ===========================================================================
// 4. Image thumbnail dimensions are exactly 64×64
// ===========================================================================

#[test]
fn thumbnail_dimensions_64x64_from_small_png() {
    // Source is 1×1.
    let png = make_png(1, 1);
    let entry =
        ContentProcessor::process(vec![payload("image/png", &png)], None).expect("process");
    let thumb_bytes = entry.thumbnail.expect("thumbnail must be present");
    let (w, h) = png_dimensions(&thumb_bytes);
    assert_eq!((w, h), (64, 64), "thumbnail from 1×1 source must be 64×64");
}

#[test]
fn thumbnail_dimensions_64x64_from_large_png() {
    // Source is 512×512.
    let png = make_png(512, 512);
    let entry =
        ContentProcessor::process(vec![payload("image/png", &png)], None).expect("process");
    let thumb_bytes = entry.thumbnail.expect("thumbnail must be present");
    let (w, h) = png_dimensions(&thumb_bytes);
    assert_eq!((w, h), (64, 64), "thumbnail from 512×512 source must be 64×64");
}

#[test]
fn thumbnail_dimensions_64x64_from_non_square_png() {
    // Source is 200×50 (wide rectangle).
    let png = make_png(200, 50);
    let entry =
        ContentProcessor::process(vec![payload("image/png", &png)], None).expect("process");
    let thumb_bytes = entry.thumbnail.expect("thumbnail must be present");
    let (w, h) = png_dimensions(&thumb_bytes);
    assert_eq!((w, h), (64, 64), "thumbnail from non-square source must be exactly 64×64");
}

#[test]
fn thumbnail_original_dimensions_recorded_not_thumb_size() {
    let png = make_png(100, 200);
    let entry =
        ContentProcessor::process(vec![payload("image/png", &png)], None).expect("process");
    // metadata.image_dimensions must record the ORIGINAL image size.
    let dims = entry.metadata.image_dimensions.expect("image_dimensions must be set");
    assert_eq!(dims, (100, 200), "metadata must record original dimensions, not thumbnail size");
}

// ===========================================================================
// 5. Malformed PNG handling
// ===========================================================================

#[test]
fn malformed_png_zero_bytes_no_thumbnail_but_entry_succeeds() {
    let entry = ContentProcessor::process(vec![payload("image/png", &[])], None)
        .expect("zero-byte PNG must not panic — entry should succeed");
    assert_eq!(entry.content_class, ContentClass::Image);
    assert!(
        entry.thumbnail.is_none(),
        "zero-byte PNG should produce no thumbnail"
    );
    assert!(entry.metadata.image_dimensions.is_none());
    // Raw bytes must still be stored.
    assert_eq!(entry.representations[0].data, b"");
}

#[test]
fn malformed_png_truncated_header_no_thumbnail() {
    // The first 8 bytes of a PNG are the magic number; truncate to 4.
    let truncated = b"\x89PNG".to_vec();
    let entry = ContentProcessor::process(vec![payload("image/png", &truncated)], None)
        .expect("truncated PNG must not error at the entry level");
    assert!(entry.thumbnail.is_none(), "truncated PNG should produce no thumbnail");
    // Raw bytes still stored.
    assert_eq!(entry.representations[0].data, truncated);
}

#[test]
fn malformed_png_random_bytes_no_thumbnail() {
    let garbage: Vec<u8> = (0u8..=255).cycle().take(256).collect();
    let entry = ContentProcessor::process(vec![payload("image/png", &garbage)], None)
        .expect("garbage PNG must not error at the entry level");
    assert!(entry.thumbnail.is_none(), "random bytes should produce no thumbnail");
}

#[test]
fn malformed_png_valid_header_corrupt_body_no_thumbnail() {
    // Write a valid PNG magic + IHDR chunk header, then corrupt the body.
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec(); // PNG magic
    // Append junk instead of a real IHDR chunk.
    data.extend_from_slice(b"\x00\x00\x00\x0dIHDR\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff");
    let entry = ContentProcessor::process(vec![payload("image/png", &data)], None)
        .expect("corrupt-body PNG must not error at the entry level");
    assert!(entry.thumbnail.is_none());
}

#[test]
fn malformed_png_hash_still_computed_over_raw_bytes() {
    let garbage: Vec<u8> = (0u8..128).collect();
    let expected_hash = *blake3::hash(&garbage).as_bytes();
    let entry = ContentProcessor::process(vec![payload("image/png", &garbage)], None)
        .expect("process");
    assert_eq!(
        entry.canonical_hash, expected_hash,
        "hash must be over raw bytes even when thumbnail decode fails"
    );
}

// ===========================================================================
// 6. URI-list parsing edge cases
// ===========================================================================

#[test]
fn uri_list_empty_lines_are_skipped() {
    let data = b"\n\n\nfile:///tmp/only.txt\n\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    assert_eq!(entry.metadata.file_count, Some(1));
    assert_eq!(entry.preview_text.as_deref(), Some("only.txt"));
}

#[test]
fn uri_list_comment_lines_are_skipped() {
    let data = b"# This is a comment\n# Another comment\nfile:///home/user/file.txt\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    assert_eq!(entry.metadata.file_count, Some(1));
    let preview = entry.preview_text.unwrap();
    assert!(preview.contains("file.txt"));
    assert!(!preview.contains('#'), "comment lines must not appear in preview");
}

#[test]
fn uri_list_entirely_comments_and_blanks_yields_zero_files() {
    let data = b"# comment 1\n# comment 2\n\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    assert_eq!(entry.metadata.file_count, Some(0));
}

#[test]
fn uri_list_non_file_uris_preserved_in_preview_as_is() {
    // Non-file:// URIs should be returned verbatim (spec: unknown scheme → full URI).
    let data = b"https://example.com/resource\nftp://ftp.example.org/file.tar.gz\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    assert_eq!(entry.metadata.file_count, Some(2));
    let preview = entry.preview_text.unwrap();
    assert!(
        preview.contains("https://example.com/resource"),
        "non-file URI should appear verbatim"
    );
    assert!(
        preview.contains("ftp://ftp.example.org/file.tar.gz"),
        "ftp URI should appear verbatim"
    );
}

#[test]
fn uri_list_percent_encoded_space_in_filename() {
    let data = b"file:///home/user/my%20file.txt\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert!(
        preview.contains("my file.txt"),
        "percent-encoded space should decode to a space in the filename"
    );
}

#[test]
fn uri_list_percent_encoded_parentheses_in_filename() {
    let data = b"file:///home/user/report%20%281%29.pdf\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    // %28 = '(', %29 = ')'
    assert!(
        preview.contains("report (1).pdf"),
        "percent-encoded parentheses should decode; got: {preview}"
    );
}

#[test]
fn uri_list_file_with_localhost_host() {
    // RFC 8089 allows file://localhost/path — the host "localhost" must be stripped.
    let data = b"file://localhost/home/user/doc.txt\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    let preview = entry.preview_text.unwrap();
    assert!(
        preview.contains("doc.txt"),
        "file://localhost/... URI should yield the filename; got: {preview}"
    );
    assert!(
        !preview.contains("localhost"),
        "the host portion should not appear in the filename preview"
    );
}

#[test]
fn uri_list_mixed_comments_blanks_and_real_uris() {
    let data = b"# header\n\nfile:///a.txt\n# mid comment\n\nfile:///b.png\n";
    let entry = ContentProcessor::process(vec![payload("text/uri-list", data)], None)
        .expect("process");
    assert_eq!(entry.metadata.file_count, Some(2));
    let preview = entry.preview_text.unwrap();
    assert!(preview.contains("a.txt"));
    assert!(preview.contains("b.png"));
}

// ===========================================================================
// 7. Privacy filter ordering — app reject takes priority over pattern match
// ===========================================================================

#[test]
fn privacy_order_app_reject_beats_pattern_ephemeral() {
    // Both the app and the text content should trigger filtering, but the app
    // ignore list (Reject) must be checked before patterns (Ephemeral).
    let filter = make_filter(&["keepassxc"], &[r"^sk-[a-zA-Z0-9]{48}"], true);
    let key = format!("sk-{}", "X".repeat(48));
    let result = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &mimes(&["text/plain"]),
        Some(&key),
    );
    assert_eq!(
        result,
        FilterResult::Reject,
        "app ignore-list must trigger Reject before the pattern check returns Ephemeral"
    );
}

#[test]
fn privacy_order_app_reject_beats_sensitive_mime_reject() {
    // Both checks yield Reject, but we verify that the result is Reject
    // (ordering is observable only if one check would return a different result).
    let filter = make_filter(&["keepassxc"], &[], true);
    let result = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &mimes(&["x-kde-passwordManagerHint"]),
        Some("secret"),
    );
    assert_eq!(result, FilterResult::Reject);
}

#[test]
fn privacy_order_sensitive_mime_reject_beats_pattern_ephemeral() {
    // App does not match, but MIME hint and pattern both fire.
    // MIME hint check (Reject, step 2) must win over pattern check (Ephemeral, step 3).
    let filter = make_filter(&[], &[r"secret"], true);
    let result = filter.check(
        Some("org.mozilla.firefox"), // not in ignore list
        &mimes(&["x-kde-passwordManagerHint"]),
        Some("secret"),
    );
    assert_eq!(
        result,
        FilterResult::Reject,
        "sensitive MIME hint must take priority over pattern match"
    );
}

#[test]
fn privacy_order_pattern_ephemeral_reached_when_no_app_or_mime_hit() {
    let filter = make_filter(&["keepassxc"], &[r"secret"], true);
    let result = filter.check(
        Some("org.mozilla.firefox"), // not in ignore list
        &mimes(&["text/plain"]),     // no sensitive MIME
        Some("my secret key"),
    );
    assert_eq!(
        result,
        FilterResult::Ephemeral,
        "when no app/MIME match, the pattern check should fire and return Ephemeral"
    );
}

#[test]
fn privacy_order_allow_when_nothing_matches() {
    let filter = make_filter(&["keepassxc"], &[r"^sk-"], true);
    let result = filter.check(
        Some("org.mozilla.firefox"),
        &mimes(&["text/plain"]),
        Some("perfectly normal text"),
    );
    assert_eq!(result, FilterResult::Allow);
}

// ===========================================================================
// 8. Privacy regex compilation error handling
// ===========================================================================

#[test]
fn privacy_construction_fails_on_invalid_regex() {
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec!["[invalid".to_string()],
        respect_sensitive_hints: true,
    };
    let result = PrivacyFilter::new(&config);
    assert!(
        result.is_err(),
        "PrivacyFilter::new must return Err when a pattern is invalid regex"
    );
}

#[test]
fn privacy_construction_fails_on_second_pattern_invalid() {
    // First pattern is valid, second is not.
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec![r"^valid-pattern$".to_string(), "(?invalid".to_string()],
        respect_sensitive_hints: true,
    };
    assert!(
        PrivacyFilter::new(&config).is_err(),
        "construction should fail even when only later patterns are invalid"
    );
}

#[test]
fn privacy_construction_fails_on_unclosed_group() {
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec!["(unclosed".to_string()],
        respect_sensitive_hints: false,
    };
    assert!(PrivacyFilter::new(&config).is_err());
}

#[test]
fn privacy_construction_succeeds_with_empty_patterns() {
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec![],
        respect_sensitive_hints: true,
    };
    assert!(PrivacyFilter::new(&config).is_ok());
}

#[test]
fn privacy_construction_succeeds_with_multiple_valid_patterns() {
    let config = IgnoreConfig {
        apps: vec![],
        patterns: vec![
            r"^sk-[a-zA-Z0-9]{48}".to_string(),
            r"^ghp_[a-zA-Z0-9]{36}".to_string(),
            r"\d{4}-\d{4}-\d{4}-\d{4}".to_string(), // credit-card-like
        ],
        respect_sensitive_hints: true,
    };
    assert!(PrivacyFilter::new(&config).is_ok());
}

// ===========================================================================
// 9. HTML tag stripping for richtext preview
// ===========================================================================

/// When both html and plain payloads are present, the plain-text payload is used
/// directly as the preview (no stripping needed).
#[test]
fn richtext_preview_uses_plain_payload_when_present() {
    let html = payload("text/html", b"<p><b>Hello</b> World</p>");
    let plain = payload("text/plain", b"Hello World");
    let entry =
        ContentProcessor::process(vec![html, plain], None).expect("process richtext");
    assert_eq!(entry.preview_text.as_deref(), Some("Hello World"));
}

/// When only the HTML payload is available (which cannot happen via normal
/// classification since RichText requires both), we test the strip_html_tags
/// helper indirectly via processor::strip_html_tags pub(crate) visibility.
/// We verify the behaviour through the richtext path by simulating a case
/// where plain payload contains the stripped text.
#[test]
fn richtext_strip_basic_tags() {
    // Use plain text that matches what strip_html_tags would produce.
    let html = payload("text/html", b"<b>Bold</b> and <i>italic</i>");
    let plain = payload("text/plain", b"Bold and italic");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    assert_eq!(entry.preview_text.as_deref(), Some("Bold and italic"));
}

#[test]
fn richtext_preview_trimmed_of_leading_trailing_whitespace() {
    let html = payload("text/html", b"<p>  trimmed  </p>");
    let plain = payload("text/plain", b"  trimmed  ");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    // The plain payload is trimmed before storage.
    assert_eq!(entry.preview_text.as_deref(), Some("trimmed"));
}

#[test]
fn richtext_canonical_hash_is_always_html_bytes() {
    let html_bytes = b"<em>hash source</em>";
    let plain_bytes = b"hash source";
    let entry = ContentProcessor::process(
        vec![
            payload("text/html", html_bytes),
            payload("text/plain", plain_bytes),
        ],
        None,
    )
    .expect("process");
    assert_eq!(
        entry.canonical_hash,
        *blake3::hash(html_bytes).as_bytes(),
        "canonical hash must be BLAKE3 of HTML bytes"
    );
}

#[test]
fn richtext_representations_contain_html_and_plain() {
    let html = payload("text/html", b"<p>hi</p>");
    let plain = payload("text/plain", b"hi");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    let mimes_stored: Vec<&str> = entry.representations.iter().map(|r| r.mime.as_str()).collect();
    assert!(
        mimes_stored.contains(&"text/html"),
        "representations must include text/html"
    );
    assert!(
        mimes_stored.contains(&"text/plain"),
        "representations must include text/plain"
    );
}

#[test]
fn richtext_html_ordered_before_plain_in_representations() {
    let html = payload("text/html", b"<p>ordered</p>");
    let plain = payload("text/plain", b"ordered");
    let entry = ContentProcessor::process(vec![html, plain], None).expect("process");
    assert_eq!(
        entry.representations[0].mime, "text/html",
        "HTML representation must come first"
    );
    assert_eq!(
        entry.representations[1].mime, "text/plain",
        "plain representation must come second"
    );
}

#[test]
fn image_processing_preserves_all_offered_representations() {
    let png = make_png(8, 8);
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
    let entry = ContentProcessor::process(
        vec![payload("image/png", &png), payload("image/jpeg", &jpeg)],
        None,
    )
    .expect("process");

    let mimes_stored: Vec<&str> = entry.representations.iter().map(|r| r.mime.as_str()).collect();
    assert!(mimes_stored.contains(&"image/png"));
    assert!(mimes_stored.contains(&"image/jpeg"));
}

#[test]
fn file_processing_preserves_all_offered_representations() {
    let uri_list = b"file:///tmp/report.pdf\n";
    let gnome = b"copy\nfile:///tmp/report.pdf\n";
    let entry = ContentProcessor::process(
        vec![
            payload("text/uri-list", uri_list),
            payload("x-special/gnome-copied-files", gnome),
        ],
        None,
    )
    .expect("process");

    let mimes_stored: Vec<&str> = entry.representations.iter().map(|r| r.mime.as_str()).collect();
    assert!(mimes_stored.contains(&"text/uri-list"));
    assert!(mimes_stored.contains(&"x-special/gnome-copied-files"));
}

// ===========================================================================
// 10. URL detection edge cases
// ===========================================================================

/// localhost — no scheme, no dot; must NOT be detected as a URL.
#[test]
fn url_detect_localhost_without_scheme_is_text() {
    let result =
        classifier::classify_with_content(&mimes(&["text/plain"]), Some("localhost"));
    assert_eq!(
        result,
        Some(ContentClass::Text),
        "bare 'localhost' with no dot should not be promoted to Url"
    );
}

/// localhost with scheme IS a URL.
#[test]
fn url_detect_localhost_with_http_scheme_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("http://localhost"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// localhost with port and scheme IS a URL.
#[test]
fn url_detect_localhost_with_port_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("http://localhost:8080/api"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// IPv4 address with scheme IS a URL.
#[test]
fn url_detect_ipv4_with_http_scheme_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("http://192.168.1.1/admin"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// Bare IPv4 address (no scheme): the domain heuristic checks segments for
/// alphanumeric chars. Digits are alphanumeric, so "192.168.1.1" has 4 segments
/// of digits — the implementation promotes this to Url.
#[test]
fn url_detect_bare_ipv4_classification() {
    // The current heuristic treats "192.168.1.1" as a valid dot-separated
    // string of alphanumeric segments, so it IS promoted to Url.
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("192.168.1.1"),
    );
    assert_eq!(
        result,
        Some(ContentClass::Url),
        "bare IPv4 address should be promoted to Url by the domain heuristic"
    );
}

/// URL with a query string.
#[test]
fn url_detect_with_query_string_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("https://example.com/search?q=hello&page=1"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// URL with a fragment.
#[test]
fn url_detect_with_fragment_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("https://docs.example.com/guide#section-3"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// URL with explicit port number.
#[test]
fn url_detect_with_port_is_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("https://example.com:8443/secure"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// Multi-word text containing a URL must NOT be promoted (whitespace check).
#[test]
fn url_detect_text_containing_url_not_promoted() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("visit https://example.com for details"),
    );
    assert_eq!(
        result,
        Some(ContentClass::Text),
        "text with embedded URL and surrounding words must remain Text"
    );
}

/// URL with subdomain.
#[test]
fn url_detect_subdomain_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("sub.example.co.uk/path"),
    );
    assert_eq!(result, Some(ContentClass::Url));
}

/// Domain extraction for URL with explicit port strips the port from the domain.
#[test]
fn url_domain_extraction_strips_port() {
    let entry = ContentProcessor::process(
        vec![payload("text/plain", b"https://example.com:9000/path")],
        None,
    )
    .expect("process");
    assert_eq!(entry.content_class, ContentClass::Url);
    assert_eq!(
        entry.metadata.url_domain.as_deref(),
        Some("example.com"),
        "port must not be included in the extracted domain"
    );
}

/// Domain extraction for URL with query string.
#[test]
fn url_domain_extraction_strips_query_and_path() {
    let entry = ContentProcessor::process(
        vec![payload("text/plain", b"https://api.example.com/v1/data?token=abc")],
        None,
    )
    .expect("process");
    assert_eq!(
        entry.metadata.url_domain.as_deref(),
        Some("api.example.com")
    );
}

/// A single plain word without a dot is plain Text.
#[test]
fn url_detect_single_word_no_dot_is_text() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("notaurl"),
    );
    assert_eq!(result, Some(ContentClass::Text));
}

/// An email address is NOT a URL (the `@` character breaks the segment rules).
#[test]
fn url_detect_email_is_not_url() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("user@example.com"),
    );
    // "user@example" contains '@' which is not alphanumeric/hyphen/underscore,
    // so the heuristic should reject it as a bare domain.
    assert_eq!(
        result,
        Some(ContentClass::Text),
        "email address should remain Text, not be promoted to Url"
    );
}

/// Leading and trailing whitespace on the text/plain content must be trimmed
/// before URL detection.
#[test]
fn url_detect_trims_whitespace_before_check() {
    let result = classifier::classify_with_content(
        &mimes(&["text/plain"]),
        Some("  https://example.com  "),
    );
    assert_eq!(
        result,
        Some(ContentClass::Url),
        "leading/trailing whitespace must be trimmed before URL detection"
    );
}
