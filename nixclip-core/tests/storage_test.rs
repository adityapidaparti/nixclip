use nixclip_core::config::GeneralConfig;
/// Integration tests for ClipStore.
///
/// Each test creates an isolated SQLite database and blob directory via
/// `tempfile::tempdir()` so tests can run in parallel without interference.
use nixclip_core::storage::ClipStore;
use nixclip_core::{ContentClass, EntryMetadata, MimePayload, NewEntry, Query};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Build a minimal `NewEntry` with a unique hash derived from `seed`.
fn make_entry(seed: u8, class: ContentClass, preview: &str) -> NewEntry {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    NewEntry {
        content_class: class,
        preview_text: Some(preview.to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: preview.as_bytes().to_vec(),
        }],
        source_app: None,
        ephemeral: false,
        metadata: EntryMetadata::default(),
    }
}

/// Open a `ClipStore` inside a temporary directory.
fn open_temp_store() -> (ClipStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");
    (store, dir)
}

// ---------------------------------------------------------------------------
// open
// ---------------------------------------------------------------------------

#[test]
fn open_creates_fresh_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("new.db");
    let blob_dir = dir.path().join("blobs");

    // Neither path exists yet — open should create them.
    let store = ClipStore::open(&db_path, &blob_dir).expect("open");
    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 0);
    assert!(db_path.exists(), "db file should be created");
    assert!(blob_dir.exists(), "blob dir should be created");
}

// ---------------------------------------------------------------------------
// insert / query
// ---------------------------------------------------------------------------

#[test]
fn insert_returns_some_id() {
    let (store, _dir) = open_temp_store();
    let entry = make_entry(1, ContentClass::Text, "hello world");
    let id = store.insert(entry).expect("insert");
    assert!(id.is_some(), "first insert should return Some(id)");
}

#[test]
fn query_returns_inserted_entry() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "rust is great"))
        .expect("insert");

    let result = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query");

    assert_eq!(result.total, 1);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(
        result.entries[0].preview_text.as_deref(),
        Some("rust is great")
    );
}

#[test]
fn query_filters_by_content_class() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "plain text"))
        .expect("insert");
    store
        .insert(make_entry(2, ContentClass::Url, "https://example.com"))
        .expect("insert");

    let text_only = store
        .query(Query {
            text: None,
            content_class: Some(ContentClass::Text),
            offset: 0,
            limit: 10,
        })
        .expect("query text");

    assert_eq!(text_only.total, 1);
    assert_eq!(text_only.entries[0].content_class, ContentClass::Text);

    let url_only = store
        .query(Query {
            text: None,
            content_class: Some(ContentClass::Url),
            offset: 0,
            limit: 10,
        })
        .expect("query url");

    assert_eq!(url_only.total, 1);
    assert_eq!(url_only.entries[0].content_class, ContentClass::Url);
}

#[test]
fn query_respects_limit_and_offset() {
    let (store, _dir) = open_temp_store();
    for i in 0..5u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    // First page: 2 entries.
    let page1 = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 2,
        })
        .expect("page1");
    assert_eq!(page1.total, 5);
    assert_eq!(page1.entries.len(), 2);

    // Second page: 2 more.
    let page2 = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 2,
            limit: 2,
        })
        .expect("page2");
    assert_eq!(page2.total, 5);
    assert_eq!(page2.entries.len(), 2);
}

#[test]
fn query_full_text_search() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "the quick brown fox"))
        .expect("insert");
    store
        .insert(make_entry(2, ContentClass::Text, "hello world"))
        .expect("insert");

    let result = store
        .query(Query {
            text: Some("quick".to_string()),
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("fts query");

    assert_eq!(result.total, 1);
    assert!(result.entries[0]
        .preview_text
        .as_deref()
        .unwrap()
        .contains("quick"));
}

// ---------------------------------------------------------------------------
// Last-entry deduplication
// ---------------------------------------------------------------------------

#[test]
fn dedup_same_hash_returns_none() {
    let (store, _dir) = open_temp_store();

    let mut hash = [0u8; 32];
    hash[0] = 42;

    let entry1 = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("duplicate".to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: b"duplicate".to_vec(),
        }],
        source_app: None,
        ephemeral: false,
        metadata: EntryMetadata::default(),
    };
    let entry2 = entry1.clone();

    let id1 = store.insert(entry1).expect("first insert");
    assert!(id1.is_some(), "first insert should yield Some(id)");

    let id2 = store.insert(entry2).expect("second insert");
    assert!(id2.is_none(), "duplicate insert should yield None (dedup)");

    // The store should still have exactly one entry.
    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 1);
}

#[test]
fn dedup_different_hash_inserts_new_entry() {
    let (store, _dir) = open_temp_store();

    let id1 = store
        .insert(make_entry(1, ContentClass::Text, "first"))
        .expect("insert 1");
    let id2 = store
        .insert(make_entry(2, ContentClass::Text, "second"))
        .expect("insert 2");

    assert!(id1.is_some());
    assert!(id2.is_some());
    assert_ne!(id1.unwrap(), id2.unwrap());

    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 2);
}

// ---------------------------------------------------------------------------
// Blob threshold
// ---------------------------------------------------------------------------

#[test]
fn large_payload_goes_to_blob_store() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    // Payload > 64 KiB — should land in the blob store.
    let large_data: Vec<u8> = vec![0xAB_u8; 65 * 1024];
    let hash = *blake3::hash(&large_data).as_bytes();

    let entry = NewEntry {
        content_class: ContentClass::Image,
        preview_text: None,
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "image/png".to_string(),
            data: large_data,
        }],
        source_app: None,
        ephemeral: false,
        metadata: EntryMetadata::default(),
    };

    store.insert(entry).expect("insert large entry");

    // At least one file should exist in the blob directory (under any sub-dir).
    let blob_file_count = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            // Only count actual prefix sub-directories (not .tmp).
            e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != ".tmp"
        })
        .count();

    assert!(
        blob_file_count >= 1,
        "blob store should contain at least one prefix directory"
    );
}

#[test]
fn small_payload_stored_inline() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    // Payload < 64 KiB — should be stored inline in SQLite.
    let small_data = b"small inline payload";
    let hash = *blake3::hash(small_data).as_bytes();

    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("small".to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: small_data.to_vec(),
        }],
        source_app: None,
        ephemeral: false,
        metadata: EntryMetadata::default(),
    };

    store.insert(entry).expect("insert small entry");

    // No prefix subdirs should be created in the blob directory.
    let has_blob_files = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .filter_map(|e| e.ok())
        .any(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != ".tmp");

    assert!(!has_blob_files, "small payload should not go to blob store");
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

#[test]
fn delete_removes_entry() {
    let (store, _dir) = open_temp_store();
    let id = store
        .insert(make_entry(1, ContentClass::Text, "to delete"))
        .expect("insert")
        .unwrap();

    store.delete(&[id]).expect("delete");

    let result = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query after delete");

    assert_eq!(result.total, 0);
}

#[test]
fn delete_empty_slice_is_noop() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "safe"))
        .expect("insert");

    store.delete(&[]).expect("delete empty");

    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 1);
}

#[test]
fn delete_multiple_entries() {
    let (store, _dir) = open_temp_store();
    let id1 = store
        .insert(make_entry(1, ContentClass::Text, "a"))
        .expect("insert")
        .unwrap();
    let id2 = store
        .insert(make_entry(2, ContentClass::Text, "b"))
        .expect("insert")
        .unwrap();
    store
        .insert(make_entry(3, ContentClass::Text, "c"))
        .expect("insert");

    store.delete(&[id1, id2]).expect("delete");

    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 1);
}

// ---------------------------------------------------------------------------
// pin
// ---------------------------------------------------------------------------

#[test]
fn pin_sets_pinned_flag() {
    let (store, _dir) = open_temp_store();
    let id = store
        .insert(make_entry(1, ContentClass::Text, "pin me"))
        .expect("insert")
        .unwrap();

    store.pin(id, true).expect("pin");

    let entry = store.get_entry(id).expect("get_entry");
    assert!(entry.pinned, "entry should be pinned");
}

#[test]
fn unpin_clears_pinned_flag() {
    let (store, _dir) = open_temp_store();
    let id = store
        .insert(make_entry(1, ContentClass::Text, "unpin me"))
        .expect("insert")
        .unwrap();

    store.pin(id, true).expect("pin");
    store.pin(id, false).expect("unpin");

    let entry = store.get_entry(id).expect("get_entry");
    assert!(!entry.pinned, "entry should be unpinned");
}

#[test]
fn pinned_entries_sort_first() {
    let (store, _dir) = open_temp_store();
    let id1 = store
        .insert(make_entry(1, ContentClass::Text, "unpinned"))
        .expect("insert")
        .unwrap();
    let id2 = store
        .insert(make_entry(2, ContentClass::Text, "pinned"))
        .expect("insert")
        .unwrap();

    store.pin(id2, true).expect("pin");

    let result = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query");

    assert_eq!(result.entries[0].id, id2, "pinned entry should come first");
    assert_eq!(result.entries[1].id, id1);
}

// ---------------------------------------------------------------------------
// clear_unpinned
// ---------------------------------------------------------------------------

#[test]
fn clear_unpinned_removes_only_unpinned() {
    let (store, _dir) = open_temp_store();
    let id_keep = store
        .insert(make_entry(1, ContentClass::Text, "keep me"))
        .expect("insert")
        .unwrap();
    store.pin(id_keep, true).expect("pin");

    store
        .insert(make_entry(2, ContentClass::Text, "remove me"))
        .expect("insert");
    store
        .insert(make_entry(3, ContentClass::Text, "also remove"))
        .expect("insert");

    store.clear_unpinned().expect("clear_unpinned");

    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 1, "only the pinned entry should survive");

    let entry = store.get_entry(id_keep).expect("get pinned entry");
    assert!(entry.pinned);
}

#[test]
fn clear_unpinned_on_empty_store_is_noop() {
    let (store, _dir) = open_temp_store();
    store
        .clear_unpinned()
        .expect("clear_unpinned on empty store");
    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 0);
}

// ---------------------------------------------------------------------------
// prune
// ---------------------------------------------------------------------------

#[test]
fn prune_enforces_max_entries() {
    let (store, _dir) = open_temp_store();

    // Insert 5 entries.
    for i in 0..5u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
        // Small sleep substitute: adjust last_seen_at by tweaking hash so they
        // get unique rows (the timestamp difference may be zero in fast tests,
        // but the prune uses ORDER BY last_seen_at ASC for overflow deletion).
    }

    let config = GeneralConfig {
        max_entries: 3,
        max_blob_size_mb: 500,
        retention: nixclip_core::config::Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };

    let prune_stats = store.prune(&config).expect("prune");
    assert_eq!(
        prune_stats.entries_deleted, 2,
        "should prune 2 oldest entries"
    );

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 3);
}

#[test]
fn prune_respects_unlimited_retention() {
    let (store, _dir) = open_temp_store();

    for i in 0..3u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    let config = GeneralConfig {
        max_entries: 1000,
        max_blob_size_mb: 500,
        retention: nixclip_core::config::Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };

    let prune_stats = store.prune(&config).expect("prune");
    assert_eq!(
        prune_stats.entries_deleted, 0,
        "unlimited retention should not delete"
    );

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 3);
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------

#[test]
fn stats_entry_count_matches_inserts() {
    let (store, _dir) = open_temp_store();
    let stats0 = store.stats().expect("stats before");
    assert_eq!(stats0.entry_count, 0);

    store
        .insert(make_entry(1, ContentClass::Text, "a"))
        .expect("insert");
    store
        .insert(make_entry(2, ContentClass::Text, "b"))
        .expect("insert");

    let stats2 = store.stats().expect("stats after");
    assert_eq!(stats2.entry_count, 2);
}

#[test]
fn stats_db_size_is_positive_after_insert() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "hello"))
        .expect("insert");
    let stats = store.stats().expect("stats");
    assert!(stats.db_size_bytes > 0, "db file should have nonzero size");
}

// ---------------------------------------------------------------------------
// get_representations
// ---------------------------------------------------------------------------

#[test]
fn get_representations_returns_stored_data() {
    let (store, _dir) = open_temp_store();
    let data = b"representation data";
    let hash = *blake3::hash(data).as_bytes();

    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("rep test".to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: data.to_vec(),
        }],
        source_app: None,
        ephemeral: false,
        metadata: EntryMetadata::default(),
    };

    let id = store.insert(entry).expect("insert").unwrap();
    let reps = store.get_representations(id).expect("get_representations");

    assert_eq!(reps.len(), 1);
    assert_eq!(reps[0].mime, "text/plain");
    assert_eq!(reps[0].data, data);
}

// ---------------------------------------------------------------------------
// integrity_check
// ---------------------------------------------------------------------------

#[test]
fn fresh_store_passes_integrity_check() {
    let (store, _dir) = open_temp_store();
    store
        .insert(make_entry(1, ContentClass::Text, "integrity test"))
        .expect("insert");
    let issues = store.integrity_check().expect("integrity_check");
    assert!(
        issues.is_empty(),
        "fresh store should have no integrity issues: {:?}",
        issues
    );
}
