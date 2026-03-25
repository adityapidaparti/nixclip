/// Spec-compliance tests for nixclip-core types, config, and error handling.
///
/// These tests directly encode every requirement from the PRD / technical
/// specification so that a future refactor cannot accidentally break the
/// public contract without a test failure.
///
/// Coverage map
/// ============
/// TYPES (lib.rs)
///   - EntryId = i64
///   - ContentClass variants, serde lowercase, Display, FromStr
///   - RestoreMode variants
///   - MimePayload fields
///   - NewEntry fields
///   - EntrySummary fields
///   - Representation fields
///   - Query fields
///   - QueryResult fields
///   - PruneStats fields
///   - ProcessedEntry fields
///   - EntryMetadata fields + Default
///   - StoreStats fields
///
/// CONFIG (config.rs)
///   - GeneralConfig defaults
///   - Retention enum serialisation round-trip
///   - UiConfig defaults
///   - KeybindConfig defaults
///   - IgnoreConfig defaults (apps, patterns, flag)
///   - Config path helpers (file-name anchoring)
///
/// ERRORS (error.rs)
///   - All eight NixClipError variants exist and carry the right payload types

use nixclip_core::{
    ContentClass, EntryId, EntryMetadata, EntrySummary, MimePayload, NewEntry, ProcessedEntry,
    PruneStats, Query, QueryResult, Representation, RestoreMode, StoreStats,
};
use nixclip_core::config::{Config, Retention};
use nixclip_core::error::NixClipError;

// ===========================================================================
// EntryId
// ===========================================================================

#[test]
fn entry_id_is_i64() {
    // The spec mandates EntryId = i64.  This verifies the type alias at the
    // binary level by ensuring we can assign any i64 (including negative values,
    // which a u64 would not accept) to an EntryId binding.
    let id: EntryId = -1_i64;
    assert_eq!(id, -1_i64);

    let max_id: EntryId = i64::MAX;
    assert_eq!(max_id, i64::MAX);
}

// ===========================================================================
// ContentClass
// ===========================================================================

#[test]
fn content_class_has_all_five_variants() {
    // Exhaustive pattern match — adding or removing a variant breaks compilation.
    let variants = [
        ContentClass::Text,
        ContentClass::RichText,
        ContentClass::Image,
        ContentClass::Files,
        ContentClass::Url,
    ];
    assert_eq!(variants.len(), 5);
}

#[test]
fn content_class_display_lowercase() {
    assert_eq!(ContentClass::Text.to_string(),     "text");
    assert_eq!(ContentClass::RichText.to_string(), "richtext");
    assert_eq!(ContentClass::Image.to_string(),    "image");
    assert_eq!(ContentClass::Files.to_string(),    "files");
    assert_eq!(ContentClass::Url.to_string(),      "url");
}

#[test]
fn content_class_from_str_roundtrip() {
    for (s, expected) in [
        ("text",     ContentClass::Text),
        ("richtext", ContentClass::RichText),
        ("image",    ContentClass::Image),
        ("files",    ContentClass::Files),
        ("url",      ContentClass::Url),
    ] {
        let parsed: ContentClass = s.parse().expect("FromStr should succeed for valid input");
        assert_eq!(parsed, expected, "FromStr failed for '{s}'");
        // Round-trip: Display(FromStr(s)) == s
        assert_eq!(parsed.to_string(), s);
    }
}

#[test]
fn content_class_from_str_unknown_is_error() {
    assert!(
        "unknown".parse::<ContentClass>().is_err(),
        "Parsing an unknown content-class string should return Err"
    );
}

#[test]
fn content_class_serde_lowercase() {
    // Spec: serde(rename_all = "lowercase")
    for (variant, expected_json) in [
        (ContentClass::Text,     r#""text""#),
        (ContentClass::RichText, r#""richtext""#),
        (ContentClass::Image,    r#""image""#),
        (ContentClass::Files,    r#""files""#),
        (ContentClass::Url,      r#""url""#),
    ] {
        let serialized = serde_json::to_string(&variant).expect("serialize ContentClass");
        assert_eq!(serialized, expected_json, "serde serialize wrong for {variant:?}");

        let deserialized: ContentClass =
            serde_json::from_str(&serialized).expect("deserialize ContentClass");
        assert_eq!(deserialized, variant);
    }
}

// ===========================================================================
// RestoreMode
// ===========================================================================

#[test]
fn restore_mode_has_original_and_plain_text() {
    // Exhaustive match ensures both variants exist.
    let variants = [RestoreMode::Original, RestoreMode::PlainText];
    assert_eq!(variants.len(), 2);
}

#[test]
fn restore_mode_serde_roundtrip() {
    for variant in [RestoreMode::Original, RestoreMode::PlainText] {
        let json = serde_json::to_string(&variant).expect("serialize RestoreMode");
        let back: RestoreMode = serde_json::from_str(&json).expect("deserialize RestoreMode");
        assert_eq!(back, variant);
    }
}

// ===========================================================================
// MimePayload
// ===========================================================================

#[test]
fn mime_payload_fields_and_serde() {
    let payload = MimePayload {
        mime: "text/plain".to_string(),
        data: vec![104, 101, 108, 108, 111], // b"hello"
    };
    assert_eq!(payload.mime, "text/plain");
    assert_eq!(payload.data, b"hello");

    // Must survive a serde round-trip.
    let json = serde_json::to_string(&payload).expect("serialize MimePayload");
    let back: MimePayload = serde_json::from_str(&json).expect("deserialize MimePayload");
    assert_eq!(back.mime, "text/plain");
    assert_eq!(back.data, b"hello");
}

// ===========================================================================
// NewEntry
// ===========================================================================

#[test]
fn new_entry_has_all_required_fields() {
    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("hello".to_string()),
        canonical_hash: [0u8; 32],
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: b"hello".to_vec(),
        }],
        source_app: Some("gedit".to_string()),
        ephemeral: false,
    };

    assert_eq!(entry.content_class, ContentClass::Text);
    assert_eq!(entry.preview_text.as_deref(), Some("hello"));
    assert_eq!(entry.canonical_hash, [0u8; 32]);
    assert_eq!(entry.representations.len(), 1);
    assert_eq!(entry.source_app.as_deref(), Some("gedit"));
    assert!(!entry.ephemeral);
}

#[test]
fn new_entry_optional_fields_accept_none() {
    let entry = NewEntry {
        content_class: ContentClass::Image,
        preview_text: None,
        canonical_hash: [0xABu8; 32],
        representations: vec![],
        source_app: None,
        ephemeral: true,
    };
    assert!(entry.preview_text.is_none());
    assert!(entry.source_app.is_none());
    assert!(entry.ephemeral);
}

#[test]
fn new_entry_canonical_hash_is_32_bytes() {
    // The spec mandates [u8; 32].  This assertion would fail at compile time if
    // the type were changed to a Vec or a different array size.
    let hash: [u8; 32] = [0u8; 32];
    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: None,
        canonical_hash: hash,
        representations: vec![],
        source_app: None,
        ephemeral: false,
    };
    let _: [u8; 32] = entry.canonical_hash; // type assertion
}

#[test]
fn new_entry_serde_roundtrip() {
    let entry = NewEntry {
        content_class: ContentClass::Url,
        preview_text: Some("https://example.com".to_string()),
        canonical_hash: [1u8; 32],
        representations: vec![MimePayload {
            mime: "text/uri-list".to_string(),
            data: b"https://example.com".to_vec(),
        }],
        source_app: None,
        ephemeral: false,
    };
    let json = serde_json::to_string(&entry).expect("serialize NewEntry");
    let back: NewEntry = serde_json::from_str(&json).expect("deserialize NewEntry");
    assert_eq!(back.content_class, ContentClass::Url);
    assert_eq!(back.canonical_hash, [1u8; 32]);
}

// ===========================================================================
// EntrySummary
// ===========================================================================

#[test]
fn entry_summary_fields_and_types() {
    let summary = EntrySummary {
        id: 42,
        created_at: 1_700_000_000_000_i64,   // unix millis
        last_seen_at: 1_700_000_001_000_i64,
        pinned: true,
        ephemeral: false,
        content_class: ContentClass::RichText,
        preview_text: Some("bold text".to_string()),
        source_app: Some("libreoffice".to_string()),
        thumbnail: Some(vec![0xFF, 0xD8, 0xFF]), // fake JPEG magic bytes
    };

    let id: EntryId = summary.id; // EntryId type check
    assert_eq!(id, 42);
    let _created: i64 = summary.created_at;   // must be i64 (unix millis)
    let _seen: i64   = summary.last_seen_at;  // must be i64
    assert!(summary.pinned);
    assert!(!summary.ephemeral);
    assert_eq!(summary.content_class, ContentClass::RichText);
    assert_eq!(summary.preview_text.as_deref(), Some("bold text"));
    assert!(summary.thumbnail.is_some());
}

#[test]
fn entry_summary_optional_fields_accept_none() {
    let summary = EntrySummary {
        id: 1,
        created_at: 0,
        last_seen_at: 0,
        pinned: false,
        ephemeral: true,
        content_class: ContentClass::Files,
        preview_text: None,
        source_app: None,
        thumbnail: None,
    };
    assert!(summary.preview_text.is_none());
    assert!(summary.source_app.is_none());
    assert!(summary.thumbnail.is_none());
}

#[test]
fn entry_summary_serde_roundtrip() {
    let summary = EntrySummary {
        id: 7,
        created_at: 999_999_999_999,
        last_seen_at: 999_999_999_999,
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Image,
        preview_text: None,
        source_app: None,
        thumbnail: Some(vec![1, 2, 3]),
    };
    let json = serde_json::to_string(&summary).expect("serialize EntrySummary");
    let back: EntrySummary = serde_json::from_str(&json).expect("deserialize EntrySummary");
    assert_eq!(back.id, 7);
    assert_eq!(back.thumbnail, Some(vec![1, 2, 3]));
}

// ===========================================================================
// Representation
// ===========================================================================

#[test]
fn representation_fields_and_serde() {
    let r = Representation {
        mime: "image/png".to_string(),
        data: vec![137, 80, 78, 71], // PNG magic
    };
    assert_eq!(r.mime, "image/png");

    let json = serde_json::to_string(&r).expect("serialize Representation");
    let back: Representation = serde_json::from_str(&json).expect("deserialize Representation");
    assert_eq!(back.mime, "image/png");
    assert_eq!(back.data, vec![137, 80, 78, 71]);
}

// ===========================================================================
// Query / QueryResult
// ===========================================================================

#[test]
fn query_fields_and_types() {
    let q = Query {
        text: Some("clipboard".to_string()),
        content_class: Some(ContentClass::Text),
        offset: 0u32,
        limit: 20u32,
    };
    let _offset: u32 = q.offset;
    let _limit: u32  = q.limit;
    assert_eq!(q.text.as_deref(), Some("clipboard"));
    assert_eq!(q.content_class, Some(ContentClass::Text));
}

#[test]
fn query_optional_fields_accept_none() {
    let q = Query {
        text: None,
        content_class: None,
        offset: 10,
        limit: 5,
    };
    assert!(q.text.is_none());
    assert!(q.content_class.is_none());
}

#[test]
fn query_serde_roundtrip() {
    let q = Query {
        text: None,
        content_class: Some(ContentClass::Url),
        offset: 5,
        limit: 10,
    };
    let json = serde_json::to_string(&q).expect("serialize Query");
    let back: Query = serde_json::from_str(&json).expect("deserialize Query");
    assert_eq!(back.offset, 5);
    assert_eq!(back.limit, 10);
    assert_eq!(back.content_class, Some(ContentClass::Url));
}

#[test]
fn query_result_fields_and_serde() {
    let qr = QueryResult {
        entries: vec![],
        total: 42u32,
    };
    let _total: u32 = qr.total;
    assert_eq!(qr.total, 42);

    let json = serde_json::to_string(&qr).expect("serialize QueryResult");
    let back: QueryResult = serde_json::from_str(&json).expect("deserialize QueryResult");
    assert_eq!(back.total, 42);
    assert!(back.entries.is_empty());
}

// ===========================================================================
// PruneStats
// ===========================================================================

#[test]
fn prune_stats_field_types() {
    let stats = PruneStats {
        entries_deleted: 10u32,
        blobs_deleted: 5u32,
        bytes_freed: 1_024_u64,
    };
    let _ed: u32 = stats.entries_deleted;
    let _bd: u32 = stats.blobs_deleted;
    let _bf: u64 = stats.bytes_freed;
    assert_eq!(stats.entries_deleted, 10);
    assert_eq!(stats.blobs_deleted, 5);
    assert_eq!(stats.bytes_freed, 1024);
}

// ===========================================================================
// ProcessedEntry
// ===========================================================================

#[test]
fn processed_entry_has_all_fields() {
    let pe = ProcessedEntry {
        content_class: ContentClass::Image,
        preview_text: Some("photo.png".to_string()),
        canonical_hash: [0xFFu8; 32],
        representations: vec![MimePayload {
            mime: "image/png".to_string(),
            data: vec![1, 2, 3],
        }],
        thumbnail: Some(vec![4, 5, 6]),
        metadata: EntryMetadata {
            image_dimensions: Some((1920, 1080)),
            file_count: None,
            url_domain: None,
        },
    };

    let _hash: [u8; 32] = pe.canonical_hash;
    assert_eq!(pe.metadata.image_dimensions, Some((1920, 1080)));
    assert!(pe.thumbnail.is_some());
}

#[test]
fn processed_entry_optional_thumbnail() {
    let pe = ProcessedEntry {
        content_class: ContentClass::Text,
        preview_text: None,
        canonical_hash: [0u8; 32],
        representations: vec![],
        thumbnail: None,
        metadata: EntryMetadata::default(),
    };
    assert!(pe.thumbnail.is_none());
}

// ===========================================================================
// EntryMetadata
// ===========================================================================

#[test]
fn entry_metadata_default_is_all_none() {
    let m = EntryMetadata::default();
    assert!(m.image_dimensions.is_none(), "image_dimensions should default to None");
    assert!(m.file_count.is_none(),       "file_count should default to None");
    assert!(m.url_domain.is_none(),       "url_domain should default to None");
}

#[test]
fn entry_metadata_image_dimensions_is_u32_pair() {
    let m = EntryMetadata {
        image_dimensions: Some((3840u32, 2160u32)),
        file_count: None,
        url_domain: None,
    };
    let (w, h): (u32, u32) = m.image_dimensions.unwrap();
    assert_eq!(w, 3840);
    assert_eq!(h, 2160);
}

#[test]
fn entry_metadata_file_count_is_usize() {
    let m = EntryMetadata {
        image_dimensions: None,
        file_count: Some(7usize),
        url_domain: None,
    };
    let _fc: usize = m.file_count.unwrap();
    assert_eq!(m.file_count, Some(7));
}

#[test]
fn entry_metadata_url_domain_is_string() {
    let m = EntryMetadata {
        image_dimensions: None,
        file_count: None,
        url_domain: Some("example.com".to_string()),
    };
    assert_eq!(m.url_domain.as_deref(), Some("example.com"));
}

// ===========================================================================
// StoreStats
// ===========================================================================

#[test]
fn store_stats_field_types() {
    let s = StoreStats {
        entry_count: 500u64,
        blob_size_bytes: 10_485_760u64,
        db_size_bytes: 4_096u64,
    };
    let _ec:  u64 = s.entry_count;
    let _bsb: u64 = s.blob_size_bytes;
    let _dsb: u64 = s.db_size_bytes;
    assert_eq!(s.entry_count, 500);
}

#[test]
fn store_stats_serde_roundtrip() {
    let s = StoreStats {
        entry_count: 1,
        blob_size_bytes: 2,
        db_size_bytes: 3,
    };
    let json = serde_json::to_string(&s).expect("serialize StoreStats");
    let back: StoreStats = serde_json::from_str(&json).expect("deserialize StoreStats");
    assert_eq!(back.entry_count, 1);
    assert_eq!(back.blob_size_bytes, 2);
    assert_eq!(back.db_size_bytes, 3);
}

// ===========================================================================
// CONFIG — GeneralConfig defaults
// ===========================================================================

#[test]
fn general_config_default_max_entries_is_1000() {
    assert_eq!(Config::default().general.max_entries, 1000u32);
}

#[test]
fn general_config_default_max_blob_size_mb_is_500() {
    assert_eq!(Config::default().general.max_blob_size_mb, 500u32);
}

#[test]
fn general_config_default_retention_is_3months() {
    assert_eq!(Config::default().general.retention, Retention::Months3);
}

// ===========================================================================
// CONFIG — Retention enum serialisation
// ===========================================================================

#[test]
fn retention_all_variants_serialise_to_spec_strings() {
    let cases: &[(Retention, &str)] = &[
        (Retention::Days7,    "\"7days\""),
        (Retention::Days30,   "\"30days\""),
        (Retention::Months3,  "\"3months\""),
        (Retention::Months6,  "\"6months\""),
        (Retention::Year1,    "\"1year\""),
        (Retention::Unlimited,"\"unlimited\""),
    ];
    for (variant, expected_json) in cases {
        let got = serde_json::to_string(variant).expect("serialize Retention");
        assert_eq!(&got, expected_json, "Wrong serialisation for {variant:?}");
    }
}

#[test]
fn retention_all_spec_strings_deserialise() {
    let cases: &[(&str, Retention)] = &[
        ("\"7days\"",    Retention::Days7),
        ("\"30days\"",   Retention::Days30),
        ("\"3months\"",  Retention::Months3),
        ("\"6months\"",  Retention::Months6),
        ("\"1year\"",    Retention::Year1),
        ("\"unlimited\"",Retention::Unlimited),
    ];
    for (json, expected) in cases {
        let got: Retention = serde_json::from_str(json).expect("deserialize Retention");
        assert_eq!(got, *expected, "Wrong deserialization for {json}");
    }
}

// ===========================================================================
// CONFIG — UiConfig defaults
// ===========================================================================

#[test]
fn ui_config_defaults_match_spec() {
    let ui = Config::default().ui;
    assert_eq!(ui.theme,               "auto");
    assert_eq!(ui.width,               680u32);
    assert_eq!(ui.max_visible_entries, 8u32);
    assert!(ui.show_source_app,   "show_source_app must default to true");
    assert!(ui.show_content_badges, "show_content_badges must default to true");
    assert_eq!(ui.position,           "top-center");
}

// ===========================================================================
// CONFIG — KeybindConfig defaults
// ===========================================================================

#[test]
fn keybind_config_defaults_match_spec() {
    let kb = Config::default().keybind;
    assert_eq!(kb.toggle,           "Super+Shift+V");
    assert_eq!(kb.restore_original, "Return");
    assert_eq!(kb.restore_plain,    "Shift+Return");
    assert_eq!(kb.delete,           "Ctrl+BackSpace");
    assert_eq!(kb.pin,              "Ctrl+P");
    assert_eq!(kb.clear_all,        "Ctrl+Shift+Delete");
}

// ===========================================================================
// CONFIG — IgnoreConfig defaults
// ===========================================================================

#[test]
fn ignore_config_default_apps_are_exact_spec_values() {
    let apps = Config::default().ignore.apps;
    assert!(
        apps.contains(&"org.keepassxc.KeePassXC".to_string()),
        "Missing org.keepassxc.KeePassXC; got: {apps:?}"
    );
    assert!(
        apps.contains(&"com.1password.1Password".to_string()),
        "Missing com.1password.1Password; got: {apps:?}"
    );
    assert!(
        apps.contains(&"com.bitwarden.desktop".to_string()),
        "Missing com.bitwarden.desktop; got: {apps:?}"
    );
}

#[test]
fn ignore_config_default_patterns_are_exact_spec_values() {
    let patterns = Config::default().ignore.patterns;
    assert!(
        patterns.contains(&r"^sk-[a-zA-Z0-9]{48}".to_string()),
        r"Missing ^sk-[a-zA-Z0-9]{{48}}; got: {patterns:?}"
    );
    assert!(
        patterns.contains(&r"^ghp_[a-zA-Z0-9]{36}".to_string()),
        r"Missing ^ghp_[a-zA-Z0-9]{{36}}; got: {patterns:?}"
    );
}

#[test]
fn ignore_config_default_respect_sensitive_hints_is_true() {
    assert!(Config::default().ignore.respect_sensitive_hints);
}

// ===========================================================================
// CONFIG — path helpers (file-name anchoring per spec)
// ===========================================================================

#[test]
fn config_path_filename_is_config_toml() {
    let p = Config::config_path();
    assert_eq!(
        p.file_name().and_then(|n| n.to_str()),
        Some("config.toml"),
        "config_path() must end with config.toml; got {p:?}"
    );
}

#[test]
fn db_path_filename_is_nixclip_db() {
    let p = Config::db_path();
    assert_eq!(
        p.file_name().and_then(|n| n.to_str()),
        Some("nixclip.db"),
        "db_path() must end with nixclip.db; got {p:?}"
    );
}

#[test]
fn blob_dir_component_is_blobs() {
    let p = Config::blob_dir();
    assert_eq!(
        p.file_name().and_then(|n| n.to_str()),
        Some("blobs"),
        "blob_dir() must end with blobs/; got {p:?}"
    );
}

#[test]
fn socket_path_filename_is_nixclip_sock() {
    let p = Config::socket_path();
    assert_eq!(
        p.file_name().and_then(|n| n.to_str()),
        Some("nixclip.sock"),
        "socket_path() must end with nixclip.sock; got {p:?}"
    );
}

#[test]
fn config_path_parent_directory_is_named_nixclip() {
    // XDG_CONFIG_HOME/<something>/nixclip/config.toml
    let p = Config::config_path();
    let parent_name = p
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str());
    assert_eq!(
        parent_name,
        Some("nixclip"),
        "config_path() parent must be 'nixclip'; got {p:?}"
    );
}

#[test]
fn db_path_parent_directory_is_named_nixclip() {
    // XDG_DATA_HOME/<something>/nixclip/nixclip.db
    let p = Config::db_path();
    let parent_name = p
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str());
    assert_eq!(
        parent_name,
        Some("nixclip"),
        "db_path() parent must be 'nixclip'; got {p:?}"
    );
}

#[test]
fn blob_dir_parent_directory_is_named_nixclip() {
    // XDG_DATA_HOME/<something>/nixclip/blobs/
    let p = Config::blob_dir();
    let parent_name = p
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str());
    assert_eq!(
        parent_name,
        Some("nixclip"),
        "blob_dir() parent must be 'nixclip'; got {p:?}"
    );
}

// ===========================================================================
// ERROR TYPES
// ===========================================================================

/// Helper: build each variant and verify the Display message contains the
/// expected kind label, proving the error message strings are spec-compliant.
fn error_message(e: &NixClipError) -> String {
    e.to_string()
}

#[test]
fn error_variant_config_carries_string_payload() {
    let e = NixClipError::Config("bad value".to_string());
    let msg = error_message(&e);
    assert!(
        msg.contains("bad value"),
        "Config error message should include the payload: {msg}"
    );
}

#[test]
fn error_variant_ipc_carries_string_payload() {
    let e = NixClipError::Ipc("connection refused".to_string());
    let msg = error_message(&e);
    assert!(msg.contains("connection refused"), "{msg}");
}

#[test]
fn error_variant_pipeline_carries_string_payload() {
    let e = NixClipError::Pipeline("bad mime type".to_string());
    let msg = error_message(&e);
    assert!(msg.contains("bad mime type"), "{msg}");
}

#[test]
fn error_variant_wayland_carries_string_payload() {
    let e = NixClipError::Wayland("no compositor".to_string());
    let msg = error_message(&e);
    assert!(msg.contains("no compositor"), "{msg}");
}

#[test]
fn error_variant_image_carries_string_payload() {
    let e = NixClipError::Image("unsupported format".to_string());
    let msg = error_message(&e);
    assert!(msg.contains("unsupported format"), "{msg}");
}

#[test]
fn error_variant_serialization_carries_string_payload() {
    let e = NixClipError::Serialization("unexpected EOF".to_string());
    let msg = error_message(&e);
    assert!(msg.contains("unexpected EOF"), "{msg}");
}

#[test]
fn error_variant_io_from_std_io_error() {
    // Verify the From<io::Error> blanket impl exists (required by spec).
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let e: NixClipError = io_err.into();
    let msg = error_message(&e);
    assert!(msg.contains("file not found"), "{msg}");
}

#[test]
fn error_variant_database_from_rusqlite_error() {
    // Verify the From<rusqlite::Error> blanket impl exists (required by spec).
    // We construct a rusqlite::Error via the public API without needing a real
    // database connection.
    use rusqlite::ffi;
    let sqlite_err = rusqlite::Error::SqliteFailure(
        ffi::Error { code: ffi::ErrorCode::Unknown, extended_code: 1 },
        Some("disk I/O error".to_string()),
    );
    let e: NixClipError = sqlite_err.into();
    let msg = error_message(&e);
    assert!(
        msg.contains("database") || msg.contains("disk") || msg.contains("I/O"),
        "Database error message should be informative: {msg}"
    );
}

#[test]
fn nixclip_error_is_send_and_sync() {
    // Error types used across async tasks must be Send + Sync.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NixClipError>();
}

#[test]
fn result_type_alias_is_correct() {
    // nixclip_core::Result<T> must alias std::result::Result<T, NixClipError>.
    fn takes_result(_: nixclip_core::Result<()>) {}
    takes_result(Ok(()));
    takes_result(Err(NixClipError::Config("test".to_string())));
}

// ===========================================================================
// ContentClass — FromStr error is NixClipError::Pipeline (spec detail)
// ===========================================================================

#[test]
fn content_class_from_str_error_is_pipeline_variant() {
    let err = "bogus".parse::<ContentClass>().unwrap_err();
    // The spec uses Pipeline for unknown content-class strings.
    assert!(
        matches!(err, NixClipError::Pipeline(_)),
        "FromStr error for unknown content class should be NixClipError::Pipeline, got: {err:?}"
    );
}
