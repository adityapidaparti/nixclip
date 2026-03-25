/// Deep integration tests for the NixClip storage layer.
///
/// These tests probe areas not covered by the existing `storage_test.rs`:
///
///   1. Atomic blob write (temp-file lifecycle / crash-safety pattern)
///   2. FTS rebuild correctness
///   3. Orphan blob cleanup
///   4. Prune by retention age (entries backdated via direct SQL)
///   5. Blob threshold boundary (exactly 64 KiB payload)
///   6. Schema version tracking
///   7. Transaction rollback on insert failure (forced via constraint violation)
///   8. Concurrent read/write safety (multiple threads sharing a Mutex<ClipStore>)
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use nixclip_core::config::{GeneralConfig, Retention};
use nixclip_core::storage::schema::SCHEMA_VERSION;
use nixclip_core::storage::{BlobStore, ClipStore};
use nixclip_core::{ContentClass, MimePayload, NewEntry, Query};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Inline threshold as defined in clip_store.rs.
const BLOB_THRESHOLD: usize = 64 * 1024;

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
        metadata: Default::default(),
    }
}

fn make_entry_with_data(seed: u8, data: Vec<u8>, mime: &str) -> NewEntry {
    let hash = *blake3::hash(&data).as_bytes();
    let mut final_hash = hash;
    // Mix in seed so callers can create distinct entries with same-size payloads.
    final_hash[31] ^= seed;
    NewEntry {
        content_class: ContentClass::Image,
        preview_text: None,
        canonical_hash: final_hash,
        representations: vec![MimePayload {
            mime: mime.to_string(),
            data,
        }],
        source_app: None,
        ephemeral: false,
        metadata: Default::default(),
    }
}

fn open_temp_store() -> (ClipStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");
    (store, dir)
}

// ---------------------------------------------------------------------------
// 1. Atomic blob write – crash-safety pattern
// ---------------------------------------------------------------------------

/// Verifies the write-to-tmp-then-rename sequence used by `BlobStore::store`.
///
/// The test cannot truly simulate a mid-write crash in a unit test, but it
/// validates the observable crash-safety invariants:
///
///   a) A `.tmp/` subdirectory is created inside the blob root.
///   b) After a successful `store()` call the final content-addressed path
///      exists and the original temp file has been renamed away (no leftover
///      `.tmp/*.tmp` files remain after a clean write).
///   c) The stored bytes are byte-for-byte identical to the input.
///   d) Calling `store()` a second time with the same hash is idempotent.
#[test]
fn atomic_blob_write_creates_final_path_and_no_leftover_tmp() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blob_dir = dir.path().join("blobs");
    let store = BlobStore::new(&blob_dir).expect("BlobStore::new");

    // (a) The .tmp directory must already exist after construction.
    assert!(
        blob_dir.join(".tmp").exists(),
        ".tmp directory should be created by BlobStore::new"
    );

    let data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
    let hash = *blake3::hash(&data).as_bytes();

    // (b) After store(), final path exists.
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let prefix = &hex[..4];
    let expected_rel = format!("{prefix}/{hex}");
    let expected_abs = blob_dir.join(&expected_rel);

    let rel = store.store(&hash, &data).expect("store");
    assert_eq!(
        rel, expected_rel,
        "returned relative path should match expected pattern"
    );
    assert!(
        expected_abs.exists(),
        "final blob file should exist after store()"
    );

    // (b-cont) No leftover temp files remain after a clean write.
    let tmp_leftovers: Vec<_> = std::fs::read_dir(blob_dir.join(".tmp"))
        .expect("read .tmp dir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        tmp_leftovers.is_empty(),
        "no temp files should remain after a successful write, found: {:?}",
        tmp_leftovers
            .iter()
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );

    // (c) Round-trip the data.
    let loaded = store.load(&rel).expect("load");
    assert_eq!(loaded, data, "loaded bytes must match stored bytes");

    // (d) Second store with the same hash is a no-op; the file still exists.
    let rel2 = store.store(&hash, &data).expect("store idempotent");
    assert_eq!(rel2, rel);
    assert!(expected_abs.exists());
}

/// Verify that the two-level directory structure is correct:
/// `{hex_prefix_4}/{full_hex}`.
#[test]
fn blob_path_uses_two_level_directory_structure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blob_dir = dir.path().join("blobs");
    let store = BlobStore::new(&blob_dir).expect("BlobStore::new");

    let data = b"two level path check";
    let hash = *blake3::hash(data).as_bytes();
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();

    let rel = store.store(&hash, data).expect("store");

    // Relative path must be exactly `{first4hex}/{full64hex}`.
    let parts: Vec<&str> = rel.splitn(2, '/').collect();
    assert_eq!(parts.len(), 2, "path must have exactly two components");
    assert_eq!(
        parts[0],
        &hex[..4],
        "prefix directory must be first 4 hex chars"
    );
    assert_eq!(
        parts[1],
        hex.as_str(),
        "file name must be the full hex string"
    );
}

// ---------------------------------------------------------------------------
// 2. FTS rebuild correctness
// ---------------------------------------------------------------------------

/// After `rebuild_fts()` old search terms that belonged to deleted entries
/// must no longer match, and terms from surviving entries must still match.
#[test]
fn fts_rebuild_removes_stale_terms_and_preserves_live_terms() {
    let (store, _dir) = open_temp_store();

    let id_live = store
        .insert(make_entry(
            1,
            ContentClass::Text,
            "persistent rainbow unicorn",
        ))
        .expect("insert live")
        .unwrap();
    let id_dead = store
        .insert(make_entry(
            2,
            ContentClass::Text,
            "ephemeral tornado deleted",
        ))
        .expect("insert dead")
        .unwrap();

    // Delete the dead entry then rebuild.
    store.delete(&[id_dead]).expect("delete dead");
    store.rebuild_fts().expect("rebuild_fts");

    // The deleted term must not appear in search results.
    let dead_result = store
        .query(Query {
            text: Some("tornado".to_string()),
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query dead term");
    assert_eq!(
        dead_result.total, 0,
        "deleted entry's terms must not appear after FTS rebuild"
    );

    // The live entry's terms must still be findable.
    let live_result = store
        .query(Query {
            text: Some("rainbow".to_string()),
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query live term");
    assert_eq!(live_result.total, 1);
    assert_eq!(live_result.entries[0].id, id_live);
}

/// Inserting an entry, immediately rebuilding FTS, then querying must still
/// find the entry — rebuilding from an empty state must repopulate correctly.
#[test]
fn fts_rebuild_repopulates_from_scratch() {
    let (store, _dir) = open_temp_store();

    store
        .insert(make_entry(
            10,
            ContentClass::Text,
            "checksum cryptography blake3",
        ))
        .expect("insert");
    store
        .insert(make_entry(
            11,
            ContentClass::Text,
            "network latency throughput",
        ))
        .expect("insert");

    // Explicitly rebuild (simulates what happens after bulk deletions).
    store.rebuild_fts().expect("rebuild_fts");

    let result = store
        .query(Query {
            text: Some("cryptography".to_string()),
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query after rebuild");

    assert_eq!(result.total, 1);
    assert!(result.entries[0]
        .preview_text
        .as_deref()
        .unwrap_or("")
        .contains("cryptography"));
}

// ---------------------------------------------------------------------------
// 3. Orphan blob cleanup
// ---------------------------------------------------------------------------

/// Create a blob file that is NOT referenced by any DB row, then call
/// `cleanup_orphans`. The file must be deleted and the returned byte count
/// must equal the file size.
#[test]
fn cleanup_orphans_removes_unreferenced_blobs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blob_dir = dir.path().join("blobs");
    let bs = BlobStore::new(&blob_dir).expect("BlobStore::new");

    // Plant a real (but orphaned) blob.
    let orphan_data: Vec<u8> = vec![0xAA; 1024];
    let hash = *blake3::hash(&orphan_data).as_bytes();
    let orphan_rel = bs.store(&hash, &orphan_data).expect("store orphan");
    let orphan_abs = blob_dir.join(&orphan_rel);
    assert!(
        orphan_abs.exists(),
        "orphan blob should be on disk before cleanup"
    );

    // Claim no paths are valid (empty set = nothing is referenced).
    let valid: HashSet<String> = HashSet::new();
    let freed = bs.cleanup_orphans(&valid).expect("cleanup_orphans");

    assert!(
        !orphan_abs.exists(),
        "orphan blob should be deleted after cleanup_orphans"
    );
    assert_eq!(freed, 1024, "bytes freed must match orphan file size");
}

/// Referenced blobs must not be touched by `cleanup_orphans`.
#[test]
fn cleanup_orphans_preserves_referenced_blobs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blob_dir = dir.path().join("blobs");
    let bs = BlobStore::new(&blob_dir).expect("BlobStore::new");

    let data: Vec<u8> = vec![0xBB; 512];
    let hash = *blake3::hash(&data).as_bytes();
    let rel = bs.store(&hash, &data).expect("store");

    // Mark this path as valid.
    let mut valid = HashSet::new();
    valid.insert(rel.clone());

    let freed = bs.cleanup_orphans(&valid).expect("cleanup_orphans");

    assert!(
        blob_dir.join(&rel).exists(),
        "referenced blob must survive cleanup_orphans"
    );
    assert_eq!(
        freed, 0,
        "zero bytes should be freed when blob is referenced"
    );
}

/// End-to-end orphan GC via `prune`: plant a blob file in the blob directory
/// that is NOT referenced by any database row, then call `prune`.  The orphan
/// GC step inside prune must delete the file and report the freed bytes.
///
/// Note: `delete()` removes blob files it knows about through the DB, but it
/// cannot know about files planted outside of a normal insert (e.g. blobs left
/// behind by a crash after the file was written but before the DB row was
/// committed).  `prune` is responsible for cleaning those up.
#[test]
fn prune_gc_reclaims_orphan_blobs_from_deleted_entries() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    // Insert a valid entry so the store is non-empty (prune always runs the
    // orphan GC step regardless of whether it deleted any entries by age/count).
    store
        .insert(make_entry(5, ContentClass::Text, "anchor entry"))
        .expect("insert anchor");

    // Plant an orphan blob directly into the blob store (bypassing the DB).
    // Use a BlobStore instance pointing at the same directory.
    let orphan_bs = BlobStore::new(&blob_dir).expect("BlobStore for orphan");
    let orphan_data: Vec<u8> = vec![0xFF; 4096];
    let orphan_hash = *blake3::hash(&orphan_data).as_bytes();
    let orphan_rel = orphan_bs
        .store(&orphan_hash, &orphan_data)
        .expect("plant orphan blob");
    let orphan_abs = blob_dir.join(&orphan_rel);
    assert!(
        orphan_abs.exists(),
        "orphan blob must be on disk before prune"
    );

    // Prune with unlimited retention and a large max_entries so only orphan GC fires.
    let config = GeneralConfig {
        max_entries: 10000,
        max_blob_size_mb: 500,
        retention: Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };

    let prune_stats = store.prune(&config).expect("prune");

    // The orphan file must be gone.
    assert!(
        !orphan_abs.exists(),
        "prune must delete orphan blob files not referenced in the DB"
    );

    // bytes_freed must account for the orphan.
    assert!(
        prune_stats.bytes_freed >= 4096,
        "bytes_freed should include the orphan blob size (4096), got {}",
        prune_stats.bytes_freed
    );
}

// ---------------------------------------------------------------------------
// 4. Prune by retention age (backdated timestamps via direct SQL)
// ---------------------------------------------------------------------------

/// Inserts two entries with timestamps far in the past (beyond 7-day retention)
/// and one recent entry. `prune` with `Days7` retention must delete exactly the
/// two old entries.
#[test]
fn prune_retention_age_deletes_old_entries_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    // Insert three entries normally (all get current timestamps).
    let id_old1 = store
        .insert(make_entry(
            1,
            ContentClass::Text,
            "ancient history entry one",
        ))
        .expect("insert old1")
        .unwrap();
    let id_old2 = store
        .insert(make_entry(
            2,
            ContentClass::Text,
            "ancient history entry two",
        ))
        .expect("insert old2")
        .unwrap();
    store
        .insert(make_entry(3, ContentClass::Text, "fresh recent entry"))
        .expect("insert fresh");

    // Backdate the two old entries to 10 days ago via direct SQL.
    // We use rusqlite through the public integrity_check path to confirm
    // the DB is accessible, then open a second connection for backdating.
    //
    // Since ClipStore does not expose raw SQL execution we open a second
    // rusqlite connection to the same WAL database for the backdating surgery.
    let ten_days_ago_ms =
        chrono::Utc::now().timestamp_millis() - chrono::Duration::days(10).num_milliseconds();

    {
        let conn = rusqlite::Connection::open(&db_path).expect("open second conn");
        conn.execute(
            "UPDATE entries SET last_seen_at = ?1 WHERE id IN (?2, ?3)",
            rusqlite::params![ten_days_ago_ms, id_old1, id_old2],
        )
        .expect("backdate entries");
    }

    let config = GeneralConfig {
        max_entries: 10000,
        max_blob_size_mb: 500,
        retention: Retention::Days7,
        ephemeral_ttl_hours: 24,
    };

    let prune_stats = store.prune(&config).expect("prune");

    assert_eq!(
        prune_stats.entries_deleted, 2,
        "prune should delete the 2 backdated entries, got: {:?}",
        prune_stats.entries_deleted
    );

    let store_stats = store.stats().expect("stats after prune");
    assert_eq!(
        store_stats.entry_count, 1,
        "only the fresh entry should survive"
    );
}

/// Pinned entries must be spared even when their `last_seen_at` is past the
/// retention window.
#[test]
fn prune_retention_does_not_delete_pinned_entries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    let id_pinned = store
        .insert(make_entry(10, ContentClass::Text, "pinned old entry"))
        .expect("insert pinned")
        .unwrap();
    store.pin(id_pinned, true).expect("pin");

    let id_old = store
        .insert(make_entry(11, ContentClass::Text, "unpinned old entry"))
        .expect("insert old")
        .unwrap();

    let ten_days_ago_ms =
        chrono::Utc::now().timestamp_millis() - chrono::Duration::days(10).num_milliseconds();

    {
        let conn = rusqlite::Connection::open(&db_path).expect("second conn");
        conn.execute(
            "UPDATE entries SET last_seen_at = ?1 WHERE id IN (?2, ?3)",
            rusqlite::params![ten_days_ago_ms, id_pinned, id_old],
        )
        .expect("backdate");
    }

    let config = GeneralConfig {
        max_entries: 10000,
        max_blob_size_mb: 500,
        retention: Retention::Days7,
        ephemeral_ttl_hours: 24,
    };

    let prune_stats = store.prune(&config).expect("prune");
    assert_eq!(
        prune_stats.entries_deleted, 1,
        "only the unpinned old entry should be deleted"
    );

    let store_stats = store.stats().expect("stats");
    assert_eq!(
        store_stats.entry_count, 1,
        "pinned entry must survive retention pruning"
    );

    let surviving = store.get_entry(id_pinned).expect("get pinned");
    assert!(surviving.pinned);
}

// ---------------------------------------------------------------------------
// 5. Blob threshold boundary (exactly 64 KiB)
// ---------------------------------------------------------------------------

/// A payload of exactly BLOB_THRESHOLD bytes (65536) sits at the boundary.
///
/// The source uses a strict less-than comparison:
///   `if rep.data.len() < BLOB_THRESHOLD { /* inline */ } else { /* blob */ }`
///
/// Therefore `len == BLOB_THRESHOLD` evaluates to `false` and the payload is
/// routed to the blob store, NOT stored inline.  This test documents and
/// verifies that boundary behaviour.
#[test]
fn blob_threshold_exact_size_goes_to_blob_store() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    // Exactly at threshold: len == BLOB_THRESHOLD  =>  routed to blob store
    // because the condition is `len < BLOB_THRESHOLD` (strict less-than).
    let exact_data: Vec<u8> = vec![0x55; BLOB_THRESHOLD];
    let entry = make_entry_with_data(20, exact_data.clone(), "application/octet-stream");

    let id = store
        .insert(entry)
        .expect("insert exact threshold entry")
        .unwrap();

    // At least one prefix sub-directory must exist in blob_dir.
    let prefix_dirs: Vec<_> = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != ".tmp")
        .collect();

    assert!(
        !prefix_dirs.is_empty(),
        "payload of exactly BLOB_THRESHOLD (65536) bytes must go to blob store \
         because the condition is strictly less-than"
    );

    // Round-trip via blob path must return the original bytes.
    let reps = store.get_representations(id).expect("get_representations");
    assert_eq!(reps.len(), 1);
    assert_eq!(
        reps[0].data, exact_data,
        "round-trip of blob-stored data must match"
    );
}

/// A payload of BLOB_THRESHOLD + 1 bytes is above the threshold and must go to
/// the blob store.
#[test]
fn blob_threshold_one_over_goes_to_blob_store() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    // One byte over: len == BLOB_THRESHOLD + 1  =>  stored on disk.
    let over_data: Vec<u8> = vec![0x66; BLOB_THRESHOLD + 1];
    let entry = make_entry_with_data(21, over_data.clone(), "application/octet-stream");

    let id = store
        .insert(entry)
        .expect("insert over-threshold entry")
        .unwrap();

    let prefix_dirs: Vec<_> = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != ".tmp")
        .collect();

    assert!(
        !prefix_dirs.is_empty(),
        "payload of BLOB_THRESHOLD+1 bytes must go to blob store"
    );

    // Round-trip to confirm blob is readable back.
    let reps = store.get_representations(id).expect("get_representations");
    assert_eq!(reps.len(), 1);
    assert_eq!(
        reps[0].data, over_data,
        "round-trip of blob-stored data must match"
    );
}

/// Payload of exactly BLOB_THRESHOLD - 1 bytes (one below) is definitively inline.
#[test]
fn blob_threshold_one_under_is_stored_inline() {
    let (store, dir) = open_temp_store();
    let blob_dir = dir.path().join("blobs");

    let under_data: Vec<u8> = vec![0x77; BLOB_THRESHOLD - 1];
    let entry = make_entry_with_data(22, under_data.clone(), "application/octet-stream");

    store.insert(entry).expect("insert under-threshold");

    let prefix_dirs: Vec<_> = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.file_name() != ".tmp")
        .collect();

    assert!(
        prefix_dirs.is_empty(),
        "payload of BLOB_THRESHOLD-1 bytes must be stored inline"
    );
}

// ---------------------------------------------------------------------------
// 6. Schema version tracking
// ---------------------------------------------------------------------------

/// After `ClipStore::open` the `schema_version` table must contain exactly one
/// row with value 1.
#[test]
fn schema_version_is_set_to_1_after_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("schema_ver.db");
    let blob_dir = dir.path().join("blobs");

    ClipStore::open(&db_path, &blob_dir).expect("open store");

    // Read schema_version via a fresh connection.
    let conn = rusqlite::Connection::open(&db_path).expect("open second conn");
    let version: u32 = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("read schema_version");

    assert_eq!(
        version, SCHEMA_VERSION,
        "schema_version must match the current schema constant after initial open"
    );
}

/// Re-opening the same database must not re-insert a second version row; the
/// version table must remain a single row.
#[test]
fn schema_version_is_stable_across_multiple_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("reopen.db");
    let blob_dir = dir.path().join("blobs");

    // Open twice.
    ClipStore::open(&db_path, &blob_dir).expect("first open");
    ClipStore::open(&db_path, &blob_dir).expect("second open");

    let conn = rusqlite::Connection::open(&db_path).expect("inspect conn");
    let row_count: u32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
        .expect("count schema_version rows");

    assert_eq!(
        row_count, 1,
        "schema_version must have exactly one row after multiple opens, found {row_count}"
    );
}

// ---------------------------------------------------------------------------
// 7. Transaction rollback on insert failure
// ---------------------------------------------------------------------------

/// Inserting two representations with the same (entry_id, mime) PRIMARY KEY
/// must cause the entire transaction to roll back: the entry row itself must
/// not persist.
///
/// We trigger this by inserting a NewEntry whose `representations` vec
/// contains two elements with the same MIME type, violating the PRIMARY KEY
/// constraint on `representations(entry_id, mime)`.
#[test]
fn insert_with_duplicate_mime_rolls_back_entire_transaction() {
    let (store, _dir) = open_temp_store();

    let hash = *blake3::hash(b"rollback test").as_bytes();
    let duplicate_mime_entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("rollback test".to_string()),
        canonical_hash: hash,
        representations: vec![
            MimePayload {
                mime: "text/plain".to_string(),
                data: b"first".to_vec(),
            },
            // Same MIME type — violates PRIMARY KEY(entry_id, mime).
            MimePayload {
                mime: "text/plain".to_string(),
                data: b"second".to_vec(),
            },
        ],
        source_app: None,
        ephemeral: false,
        metadata: Default::default(),
    };

    // The insert must return an error.
    let result = store.insert(duplicate_mime_entry);
    assert!(
        result.is_err(),
        "inserting duplicate MIME types must fail with an error"
    );

    // The entry row must not have been persisted (transaction rolled back).
    let stats = store.stats().expect("stats after failed insert");
    assert_eq!(
        stats.entry_count, 0,
        "no entry should be committed after a rolled-back insert transaction"
    );
}

/// After a rolled-back insert a subsequent valid insert must succeed,
/// confirming the store is still usable.
#[test]
fn store_remains_usable_after_rolled_back_insert() {
    let (store, _dir) = open_temp_store();

    // Trigger a failing insert (duplicate mime).
    let hash1 = *blake3::hash(b"will fail").as_bytes();
    let bad_entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("fail".to_string()),
        canonical_hash: hash1,
        representations: vec![
            MimePayload {
                mime: "text/plain".to_string(),
                data: b"a".to_vec(),
            },
            MimePayload {
                mime: "text/plain".to_string(),
                data: b"b".to_vec(),
            },
        ],
        source_app: None,
        ephemeral: false,
        metadata: Default::default(),
    };
    let _ = store.insert(bad_entry); // error expected but ignored here

    // A valid insert must still work.
    let good_entry = make_entry(99, ContentClass::Text, "recovery works");
    let id = store
        .insert(good_entry)
        .expect("good insert after failed one");
    assert!(
        id.is_some(),
        "store must accept valid inserts after a failed transaction"
    );

    let stats = store.stats().expect("stats");
    assert_eq!(stats.entry_count, 1);
}

// ---------------------------------------------------------------------------
// 8. Concurrent read/write safety
// ---------------------------------------------------------------------------

/// Spawn N writer threads and M reader threads sharing a `Mutex<ClipStore>`.
/// Each writer inserts a unique entry; each reader runs a query.  The test
/// passes if no thread panics and the final entry count equals N.
#[test]
fn concurrent_read_write_via_mutex() {
    const WRITERS: usize = 8;
    const READERS: usize = 4;

    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("concurrent.db");
    let blob_dir = dir.path().join("blobs");

    let store = Arc::new(Mutex::new(
        ClipStore::open(&db_path, &blob_dir).expect("open store"),
    ));

    let mut handles = Vec::new();

    // Writer threads.
    for i in 0..WRITERS {
        let store_arc = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            let entry = make_entry(
                i as u8,
                ContentClass::Text,
                &format!("concurrent entry {i}"),
            );
            let store = store_arc.lock().expect("lock for write");
            store.insert(entry).expect("concurrent insert");
        }));
    }

    // Reader threads.
    for _ in 0..READERS {
        let store_arc = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            let store = store_arc.lock().expect("lock for read");
            store
                .query(Query {
                    text: None,
                    content_class: None,
                    offset: 0,
                    limit: 100,
                })
                .expect("concurrent query");
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    let final_store = store.lock().expect("final lock");
    let stats = final_store.stats().expect("final stats");
    assert_eq!(
        stats.entry_count, WRITERS as u64,
        "all {WRITERS} writer inserts must be committed"
    );
}

/// Interleaved writes and reads must leave the store in a consistent state.
/// Each round: a writer inserts, then a reader immediately queries and confirms
/// the running total is non-decreasing.
#[test]
fn interleaved_insert_and_query_maintain_consistency() {
    let (store, _dir) = open_temp_store();
    let store = Arc::new(Mutex::new(store));

    const ROUNDS: usize = 10;
    let mut last_seen_count = 0u64;

    for i in 0..ROUNDS {
        // Write.
        {
            let s = store.lock().expect("write lock");
            s.insert(make_entry(
                i as u8,
                ContentClass::Text,
                &format!("round {i}"),
            ))
            .expect("insert round");
        }

        // Read and verify non-decreasing count.
        {
            let s = store.lock().expect("read lock");
            let stats = s.stats().expect("stats");
            assert!(
                stats.entry_count >= last_seen_count,
                "entry count must be non-decreasing: was {last_seen_count}, now {}",
                stats.entry_count
            );
            last_seen_count = stats.entry_count;
        }
    }

    assert_eq!(last_seen_count, ROUNDS as u64);
}
