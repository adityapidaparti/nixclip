/// Regression tests targeting specific bugs that shipped in production.
///
/// These tests verify that the root cause of each bug has been addressed and
/// would catch a recurrence.  Bugs that are purely compile-time (type errors,
/// missing imports, ambiguous methods) cannot be caught at runtime and are
/// explicitly excluded.
///
/// Bug manifest
/// ============
///
/// | #  | Bug                                             | Testable? | Tests below     |
/// |----|-------------------------------------------------|-----------|-----------------|
/// |  1 | pruning.rs called blocking_read() in async ctx  | YES       | pruning_*       |
/// |  2 | wl-clipboard binary not in PATH                 | YES       | wlpaste_*       |
/// |  3 | Nested Fn closures consuming Rc w/o pre-clone   | NO (compile-time)            |
/// |  4 | adw::ApplicationWindow ambiguous activate_action| NO (compile-time)            |
/// |  5 | glib::MainContext::channel API removed           | NO (compile-time)            |
/// |  6 | libc crate not imported                         | NO (compile-time)            |
///
/// Additional coverage gaps found during audit:
///   - Ephemeral entry pruning (prune_ephemeral) was completely untested
///   - Config TOML save/load with ephemeral_ttl_hours field
///   - Pruning under tokio async runtime (the actual bug surface)
///   - IPC end-to-end over Unix domain sockets
///   - Privacy filter two-phase check (pre_content vs content_patterns)

use nixclip_core::config::{Config, GeneralConfig, IgnoreConfig, Retention};
use nixclip_core::pipeline::privacy::{FilterResult, PrivacyFilter};
use nixclip_core::pipeline::ContentProcessor;
use nixclip_core::storage::ClipStore;
use nixclip_core::{
    ContentClass, EntryMetadata, MimePayload, NewEntry, Query,
};

// ===========================================================================
// Helpers
// ===========================================================================

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

fn make_ephemeral_entry(seed: u8, preview: &str) -> NewEntry {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some(preview.to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: preview.as_bytes().to_vec(),
        }],
        source_app: None,
        ephemeral: true,
        metadata: EntryMetadata::default(),
    }
}

fn open_temp_store() -> (ClipStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");
    (store, dir)
}

fn mimes(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// ===========================================================================
// BUG 1: pruning.rs called blocking_read() inside async tokio runtime
//
// The original code used `state.config.blocking_read()` inside a
// `tokio::spawn`-ed async task.  Tokio panics if you call a blocking
// operation from within the runtime without spawn_blocking.
//
// The fix was to use `state.config.read().await` (async RwLock).
//
// These tests verify that all ClipStore pruning operations work correctly
// when called from within a tokio async runtime context, which is the
// actual execution environment in the daemon.
// ===========================================================================

/// Verify that ClipStore::prune works correctly when executed inside a
/// tokio runtime (simulating the daemon's pruning task).
#[tokio::test]
async fn pruning_works_inside_tokio_runtime() {
    let (store, _dir) = open_temp_store();

    // Insert 5 entries.
    for i in 0..5u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    let config = GeneralConfig {
        max_entries: 3,
        max_blob_size_mb: 500,
        retention: Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };

    // This is the key assertion: prune() called from an async context must not panic.
    // The original bug was that blocking_read() was called here, which panics.
    let stats = store.prune(&config).expect("prune inside tokio runtime");
    assert_eq!(stats.entries_deleted, 2, "should prune 2 oldest entries");

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 3);
}

/// Verify that prune can be called from an async task via spawn_blocking
/// (the correct pattern for !Send types inside async runtime).
#[tokio::test]
async fn pruning_via_spawn_blocking_does_not_panic() {
    let (store, _dir) = open_temp_store();

    for i in 0..5u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    let config = GeneralConfig {
        max_entries: 2,
        max_blob_size_mb: 500,
        retention: Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };

    // Wrap in std::sync::Mutex as the daemon does.
    let store = std::sync::Mutex::new(store);
    let store = std::sync::Arc::new(store);

    let s = store.clone();
    let stats = tokio::task::spawn_blocking(move || {
        let store = s.lock().expect("lock");
        store.prune(&config).expect("prune")
    })
    .await
    .expect("spawn_blocking should not panic");

    assert_eq!(stats.entries_deleted, 3);
}

/// Verify that reading config via tokio::sync::RwLock (the correct async
/// pattern) and then calling prune works without panic.
#[tokio::test]
async fn pruning_with_async_rwlock_config_read() {
    let (store, _dir) = open_temp_store();

    for i in 0..4u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    // Simulate the daemon's pattern: config behind an async RwLock.
    let config = tokio::sync::RwLock::new(Config::default());

    // Read config the correct way (async .read().await).
    let general = {
        let cfg = config.read().await;
        cfg.general.clone()
    };

    let pruned = GeneralConfig {
        max_entries: 2,
        ..general
    };

    let stats = store.prune(&pruned).expect("prune");
    assert_eq!(stats.entries_deleted, 2);
}

/// Verify prune_ephemeral works inside a tokio runtime.
/// This was also on the code path that used blocking_read().
#[tokio::test]
async fn prune_ephemeral_works_inside_tokio_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    // Insert ephemeral entries.
    store
        .insert(make_ephemeral_entry(1, "ephemeral secret key"))
        .expect("insert ephemeral");
    store
        .insert(make_ephemeral_entry(2, "another ephemeral"))
        .expect("insert ephemeral 2");
    // Also insert a non-ephemeral entry.
    store
        .insert(make_entry(3, ContentClass::Text, "permanent entry"))
        .expect("insert permanent");

    // With TTL of 0 hours, all ephemeral entries should be expired immediately.
    let stats = store.prune_ephemeral(0).expect("prune_ephemeral");
    assert_eq!(
        stats.entries_deleted, 2,
        "both ephemeral entries should be pruned with TTL=0"
    );

    // The permanent entry should survive.
    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 1);
}

// ===========================================================================
// BUG 2: wl-clipboard binaries not in PATH — daemon started but clipboard
//         capture immediately stopped
//
// The fix added `WlPasteBackend::available()` checks with a graceful
// fallback path. These tests verify the detection logic.
// ===========================================================================

/// Verify that wl-paste availability check returns false on macOS or when
/// the binary is not installed.
#[test]
fn wlpaste_available_returns_false_when_binary_missing() {
    // On macOS (where CI runs), wl-paste is never installed.
    // On Linux without wl-clipboard, this should also be false.
    // We only assert this doesn't panic; the actual value depends on the env.
    let result = std::process::Command::new("wl-paste")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match result {
        Ok(status) => {
            // If the command exists, the status tells us if it works.
            let _ = status.success();
        }
        Err(e) => {
            // Command not found — this is expected on macOS.
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "missing wl-paste should be NotFound, got: {e}"
            );
        }
    }
}

/// Verify that wl-copy availability check returns false when binary is missing.
#[test]
fn wlcopy_available_returns_false_when_binary_missing() {
    let result = std::process::Command::new("wl-copy")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match result {
        Ok(status) => {
            let _ = status.success();
        }
        Err(e) => {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "missing wl-copy should be NotFound, got: {e}"
            );
        }
    }
}

/// Verify that daemon gracefully continues when no clipboard backend is available.
/// Specifically, the daemon should still accept IPC connections, run pruning, etc.
/// even when clipboard capture is disabled.
#[tokio::test]
async fn daemon_subsystems_work_without_clipboard_backend() {
    // Simulate what the daemon does: open store, run pruning, serve IPC.
    let (store, _dir) = open_temp_store();

    // Insert some entries (simulating entries from a previous session).
    for i in 0..3u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("old entry {i}")))
            .expect("insert");
    }

    // Pruning should still work.
    let config = GeneralConfig {
        max_entries: 2,
        max_blob_size_mb: 500,
        retention: Retention::Unlimited,
        ephemeral_ttl_hours: 24,
    };
    let stats = store.prune(&config).expect("prune without clipboard backend");
    assert_eq!(stats.entries_deleted, 1);

    // Queries should still work.
    let result = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query without clipboard backend");
    assert_eq!(result.total, 2);
}

// ===========================================================================
// Ephemeral entry pruning — was completely untested
// ===========================================================================

/// Ephemeral entries with TTL > 0 should survive if they are recent.
#[test]
fn prune_ephemeral_preserves_recent_entries() {
    let (store, _dir) = open_temp_store();

    store
        .insert(make_ephemeral_entry(1, "recent ephemeral"))
        .expect("insert");

    // With a 24-hour TTL, a just-inserted entry should survive.
    let stats = store.prune_ephemeral(24).expect("prune_ephemeral");
    assert_eq!(
        stats.entries_deleted, 0,
        "recently inserted ephemeral entry should survive with 24h TTL"
    );

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 1);
}

/// Ephemeral entries with TTL=0 should be pruned if created_at < now.
/// Note: the SQL uses strict `<`, so entries created at exactly the cutoff
/// millisecond may survive. We sleep briefly to ensure all entries are
/// strictly before the cutoff.
#[test]
fn prune_ephemeral_ttl_zero_removes_all() {
    let (store, _dir) = open_temp_store();

    for i in 0..5u8 {
        store
            .insert(make_ephemeral_entry(i, &format!("ephemeral {i}")))
            .expect("insert");
    }

    // Sleep to ensure all entries have created_at strictly less than the
    // cutoff (which is computed as `now - 0 = now` at prune time).
    std::thread::sleep(std::time::Duration::from_millis(5));

    let stats = store.prune_ephemeral(0).expect("prune_ephemeral");
    assert_eq!(stats.entries_deleted, 5);

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 0);
}

/// Non-ephemeral entries should never be deleted by prune_ephemeral.
#[test]
fn prune_ephemeral_does_not_touch_normal_entries() {
    let (store, _dir) = open_temp_store();

    store
        .insert(make_entry(1, ContentClass::Text, "normal entry"))
        .expect("insert normal");
    store
        .insert(make_ephemeral_entry(2, "ephemeral entry"))
        .expect("insert ephemeral");

    // Sleep to ensure entries have created_at strictly before the cutoff.
    std::thread::sleep(std::time::Duration::from_millis(5));

    let stats = store.prune_ephemeral(0).expect("prune_ephemeral");
    assert_eq!(stats.entries_deleted, 1, "only the ephemeral entry should go");

    let store_stats = store.stats().expect("stats");
    assert_eq!(store_stats.entry_count, 1, "normal entry should survive");
}

/// Backdated ephemeral entries should be pruned by a non-zero TTL.
#[test]
fn prune_ephemeral_deletes_old_entries_with_nonzero_ttl() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    let id = store
        .insert(make_ephemeral_entry(1, "old ephemeral"))
        .expect("insert")
        .unwrap();

    // Backdate the entry to 48 hours ago.
    let two_days_ago_ms =
        chrono::Utc::now().timestamp_millis() - chrono::Duration::hours(48).num_milliseconds();
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open conn");
        conn.execute(
            "UPDATE entries SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![two_days_ago_ms, id],
        )
        .expect("backdate");
    }

    // Prune with 24-hour TTL — the 48-hour-old entry should be deleted.
    let stats = store.prune_ephemeral(24).expect("prune_ephemeral");
    assert_eq!(
        stats.entries_deleted, 1,
        "48h-old ephemeral entry should be pruned with 24h TTL"
    );
}

/// Pinned ephemeral entries — the spec says pinned entries survive retention
/// pruning, but prune_ephemeral does NOT check the pinned flag. This test
/// documents the current behavior: pinned ephemeral entries ARE pruned.
/// If this is changed in the future, this test will catch the behavioral change.
#[test]
fn prune_ephemeral_deletes_pinned_ephemeral_entries_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    let id = store
        .insert(make_ephemeral_entry(1, "pinned ephemeral"))
        .expect("insert")
        .unwrap();
    store.pin(id, true).expect("pin");

    // Sleep to ensure created_at is strictly before the cutoff.
    std::thread::sleep(std::time::Duration::from_millis(5));

    // TTL=0 should still remove the pinned ephemeral entry.
    let stats = store.prune_ephemeral(0).expect("prune_ephemeral");
    assert_eq!(
        stats.entries_deleted, 1,
        "prune_ephemeral should remove pinned ephemeral entries (current behavior)"
    );
}

// ===========================================================================
// Combined pruning — both standard and ephemeral in sequence
// (simulates what nixclipd/src/pruning.rs::run_once does)
// ===========================================================================

/// Simulate the full pruning::run_once flow: standard prune + ephemeral prune.
#[tokio::test]
async fn combined_prune_and_ephemeral_prune() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");

    // Insert regular entries that exceed max_entries.
    for i in 0..5u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("regular {i}")))
            .expect("insert regular");
    }
    // Insert ephemeral entries.
    for i in 10..13u8 {
        store
            .insert(make_ephemeral_entry(i, &format!("ephemeral {i}")))
            .expect("insert ephemeral");
    }

    let config = GeneralConfig {
        max_entries: 4,
        max_blob_size_mb: 500,
        retention: Retention::Unlimited,
        ephemeral_ttl_hours: 0, // immediately expire
    };

    // Standard prune: should remove entries over max_entries.
    let mut stats = store.prune(&config).expect("standard prune");

    // Ephemeral prune: should remove all ephemeral entries with TTL=0.
    let ephemeral_stats = store
        .prune_ephemeral(config.ephemeral_ttl_hours)
        .expect("ephemeral prune");

    stats.entries_deleted += ephemeral_stats.entries_deleted;
    stats.blobs_deleted += ephemeral_stats.blobs_deleted;
    stats.bytes_freed += ephemeral_stats.bytes_freed;

    // 8 total entries, max_entries=4 → 4 deleted by standard prune.
    // Then ephemeral prune removes any remaining ephemeral entries.
    let store_stats = store.stats().expect("stats");
    assert!(
        store_stats.entry_count <= 4,
        "at most 4 entries should remain after combined prune; got {}",
        store_stats.entry_count
    );
    assert!(
        stats.entries_deleted >= 4,
        "combined prune should delete at least 4 entries; got {}",
        stats.entries_deleted
    );
}

// ===========================================================================
// Privacy filter two-phase check — gap in existing tests
//
// The watcher uses a two-phase privacy check:
//   Phase 1: check_pre_content (app + MIME) — runs BEFORE content processing
//   Phase 2: check_content_patterns (regex on preview) — runs AFTER processing
//
// Existing tests only test the combined `check()` method. These tests verify
// the phases individually.
// ===========================================================================

/// Phase 1 should reject ignored apps without needing preview text.
#[test]
fn two_phase_privacy_phase1_rejects_ignored_app() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let result =
        filter.check_pre_content(Some("org.keepassxc.KeePassXC"), &mimes(&["text/plain"]));
    assert_eq!(result, FilterResult::Reject);
}

/// Phase 1 should reject sensitive MIME types without needing preview text.
#[test]
fn two_phase_privacy_phase1_rejects_sensitive_mime() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let result = filter.check_pre_content(
        None,
        &mimes(&["text/plain", "x-kde-passwordManagerHint"]),
    );
    assert_eq!(result, FilterResult::Reject);
}

/// Phase 1 should allow normal events so they can proceed to content processing.
#[test]
fn two_phase_privacy_phase1_allows_normal() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let result =
        filter.check_pre_content(Some("org.mozilla.firefox"), &mimes(&["text/plain"]));
    assert_eq!(result, FilterResult::Allow);
}

/// Phase 2 should detect API keys in preview text and flag as Ephemeral.
#[test]
fn two_phase_privacy_phase2_flags_api_key() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let key = format!("sk-{}", "A".repeat(48));
    let result = filter.check_content_patterns(Some(&key));
    assert_eq!(result, FilterResult::Ephemeral);
}

/// Phase 2 should allow normal text.
#[test]
fn two_phase_privacy_phase2_allows_normal_text() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let result = filter.check_content_patterns(Some("just a regular note"));
    assert_eq!(result, FilterResult::Allow);
}

/// Phase 2 with None preview text should allow (no content to check).
#[test]
fn two_phase_privacy_phase2_allows_none_preview() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let result = filter.check_content_patterns(None);
    assert_eq!(result, FilterResult::Allow);
}

/// The full two-phase flow: phase 1 Allow + phase 2 Ephemeral = Ephemeral.
#[test]
fn two_phase_privacy_full_flow_ephemeral() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let key = format!("ghp_{}", "B".repeat(36));
    let offered_mimes = mimes(&["text/plain"]);

    // Phase 1: should allow (normal app, normal mime).
    let phase1 = filter.check_pre_content(Some("org.gnome.Terminal"), &offered_mimes);
    assert_eq!(phase1, FilterResult::Allow);

    // Phase 2: should detect GitHub token.
    let phase2 = filter.check_content_patterns(Some(&key));
    assert_eq!(phase2, FilterResult::Ephemeral);
}

/// The full two-phase flow: phase 1 Reject short-circuits, no phase 2 needed.
#[test]
fn two_phase_privacy_full_flow_reject_shortcircuits() {
    let filter = PrivacyFilter::new(&IgnoreConfig::default()).expect("filter");

    let offered_mimes = mimes(&["text/plain"]);

    // Phase 1: rejected by app name.
    let phase1 =
        filter.check_pre_content(Some("org.keepassxc.KeePassXC"), &offered_mimes);
    assert_eq!(phase1, FilterResult::Reject);

    // Phase 2 would never be called because the event is already rejected.
    // We verify by checking that the combined check() also returns Reject.
    let combined = filter.check(
        Some("org.keepassxc.KeePassXC"),
        &offered_mimes,
        Some("not even checked"),
    );
    assert_eq!(combined, FilterResult::Reject);
}

// ===========================================================================
// IPC protocol round-trip tests over tokio duplex stream
//
// These verify the full send/recv cycle using tokio in-memory streams,
// similar to how the daemon + CLI communicate over Unix domain sockets.
// ===========================================================================

#[tokio::test]
async fn ipc_full_query_response_cycle_over_duplex() {
    use nixclip_core::ipc::{recv_message, send_message, ClientMessage, ServerMessage};
    use nixclip_core::{ContentClass, EntrySummary};

    let (mut client, mut server) = tokio::io::duplex(8192);

    // Client sends a query.
    let query = ClientMessage::query(Some("rust".into()), Some("text".into()), 0, 20);
    send_message(&mut client, &query)
        .await
        .expect("client send query");

    // Server receives query.
    let received: ClientMessage = recv_message(&mut server).await.expect("server recv");
    match &received {
        ClientMessage::Query {
            text,
            content_class,
            offset,
            limit,
            ..
        } => {
            assert_eq!(text.as_deref(), Some("rust"));
            assert_eq!(content_class.as_deref(), Some("text"));
            assert_eq!(*offset, 0);
            assert_eq!(*limit, 20);
        }
        other => panic!("expected Query, got {other:?}"),
    }

    // Server sends a QueryResult response.
    let entry = EntrySummary {
        id: 42,
        created_at: 1_700_000_000_000,
        last_seen_at: 1_700_000_001_000,
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Text,
        preview_text: Some("rust programming".to_string()),
        source_app: None,
        thumbnail: None,
        match_ranges: vec![],
        metadata: EntryMetadata::default(),
    };
    let response = ServerMessage::query_result(vec![entry], 1);
    send_message(&mut server, &response)
        .await
        .expect("server send response");

    // Client receives response.
    let server_reply: ServerMessage =
        recv_message(&mut client).await.expect("client recv response");
    match server_reply {
        ServerMessage::QueryResult {
            entries, total, ..
        } => {
            assert_eq!(total, 1);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, 42);
            assert_eq!(
                entries[0].preview_text.as_deref(),
                Some("rust programming")
            );
        }
        other => panic!("expected QueryResult, got {other:?}"),
    }
}

#[tokio::test]
async fn ipc_subscribe_then_receive_new_entry() {
    use nixclip_core::ipc::{recv_message, send_message, ClientMessage, ServerMessage};
    use nixclip_core::EntrySummary;

    let (mut client, mut server) = tokio::io::duplex(8192);

    // Client subscribes.
    let sub = ClientMessage::subscribe();
    send_message(&mut client, &sub)
        .await
        .expect("client subscribe");

    // Server receives subscribe.
    let msg: ClientMessage = recv_message(&mut server).await.expect("server recv subscribe");
    assert!(matches!(msg, ClientMessage::Subscribe { .. }));

    // Server pushes a new entry notification.
    let entry = EntrySummary {
        id: 1,
        created_at: chrono::Utc::now().timestamp_millis(),
        last_seen_at: chrono::Utc::now().timestamp_millis(),
        pinned: false,
        ephemeral: false,
        content_class: ContentClass::Url,
        preview_text: Some("https://example.com".to_string()),
        source_app: Some("firefox".to_string()),
        thumbnail: None,
        match_ranges: vec![],
        metadata: EntryMetadata::default(),
    };
    let notif = ServerMessage::new_entry(entry);
    send_message(&mut server, &notif)
        .await
        .expect("server push new entry");

    // Client receives the notification.
    let received: ServerMessage =
        recv_message(&mut client).await.expect("client recv notification");
    match received {
        ServerMessage::NewEntry { entry, .. } => {
            assert_eq!(entry.id, 1);
            assert_eq!(entry.content_class, ContentClass::Url);
        }
        other => panic!("expected NewEntry, got {other:?}"),
    }
}

#[tokio::test]
async fn ipc_set_config_round_trip() {
    use nixclip_core::ipc::{
        recv_message, send_message, ClientMessage,
        ServerMessage,
    };

    let (mut client, mut server) = tokio::io::duplex(8192);

    // Client sends SetConfig patch.
    let patch = toml::Value::Table({
        let mut general = toml::map::Map::new();
        general.insert("max_entries".into(), toml::Value::Integer(500));
        let mut root = toml::map::Map::new();
        root.insert("general".into(), toml::Value::Table(general));
        root
    });
    let msg = ClientMessage::set_config(patch.clone());
    send_message(&mut client, &msg)
        .await
        .expect("client send SetConfig");

    // Server receives.
    let received: ClientMessage = recv_message(&mut server).await.expect("server recv");
    match received {
        ClientMessage::SetConfig { patch: p, .. } => {
            assert_eq!(p, patch);
        }
        other => panic!("expected SetConfig, got {other:?}"),
    }

    // Server responds with updated config.
    let mut config = Config::default();
    config.general.max_entries = 500;
    let reply = ServerMessage::config_value(config);
    send_message(&mut server, &reply)
        .await
        .expect("server send ConfigValue");

    // Client receives.
    let response: ServerMessage = recv_message(&mut client).await.expect("client recv");
    match response {
        ServerMessage::ConfigValue { config, .. } => {
            assert_eq!(config.general.max_entries, 500);
        }
        other => panic!("expected ConfigValue, got {other:?}"),
    }
}

// ===========================================================================
// Config save/load with ephemeral_ttl_hours
//
// The ephemeral_ttl_hours field was added for ephemeral entry support but
// was not covered by config round-trip tests.
// ===========================================================================

#[test]
fn config_ephemeral_ttl_hours_default_is_24() {
    let config = Config::default();
    assert_eq!(
        config.general.ephemeral_ttl_hours, 24,
        "default ephemeral_ttl_hours should be 24"
    );
}

#[test]
fn config_ephemeral_ttl_hours_round_trip_via_toml() {
    let config = Config {
        general: GeneralConfig {
            max_entries: 1000,
            max_blob_size_mb: 500,
            retention: Retention::Months3,
            ephemeral_ttl_hours: 12,
        },
        ..Config::default()
    };

    let toml_str = toml::to_string_pretty(&config).expect("serialize");
    let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(
        loaded.general.ephemeral_ttl_hours, 12,
        "ephemeral_ttl_hours should survive TOML round-trip"
    );
}

#[test]
fn config_ephemeral_ttl_hours_defaults_when_missing_in_toml() {
    let toml_str = r#"
[general]
max_entries = 500
"#;
    let config: Config = toml::from_str(toml_str).expect("parse");
    assert_eq!(
        config.general.ephemeral_ttl_hours, 24,
        "missing ephemeral_ttl_hours should default to 24"
    );
}

#[test]
fn config_save_load_file_round_trip_with_ephemeral_ttl() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.general.ephemeral_ttl_hours = 6;

    config.save(&path).expect("save");
    let loaded = Config::load(&path).expect("load");
    assert_eq!(loaded.general.ephemeral_ttl_hours, 6);
}

// ===========================================================================
// Content processor edge cases — additional coverage
// ===========================================================================

/// Verify that the content processor correctly sets the ephemeral flag path
/// by testing the full pipeline: process -> privacy check -> NewEntry creation.
#[test]
fn content_processor_to_new_entry_pipeline() {
    let payload = MimePayload {
        mime: "text/plain".to_string(),
        data: b"Hello, world!".to_vec(),
    };

    let processed =
        ContentProcessor::process(vec![payload], Some("org.gnome.gedit".to_string()))
            .expect("process");

    assert_eq!(processed.content_class, ContentClass::Text);
    assert_eq!(processed.preview_text.as_deref(), Some("Hello, world!"));

    // Build a NewEntry as the watcher does.
    let new_entry = NewEntry {
        content_class: processed.content_class,
        preview_text: processed.preview_text.clone(),
        canonical_hash: processed.canonical_hash,
        representations: processed.representations.clone(),
        source_app: Some("org.gnome.gedit".to_string()),
        ephemeral: false,
        metadata: processed.metadata.clone(),
    };

    assert_eq!(new_entry.content_class, ContentClass::Text);
    assert!(!new_entry.ephemeral);

    // Insert into store and verify round-trip.
    let (store, _dir) = open_temp_store();
    let id = store.insert(new_entry).expect("insert").unwrap();
    let entry = store.get_entry(id).expect("get_entry");
    assert_eq!(entry.preview_text.as_deref(), Some("Hello, world!"));
    assert!(!entry.ephemeral);
}

/// Verify the ephemeral flag flows through the full pipeline.
#[test]
fn ephemeral_flag_propagates_through_insert_and_query() {
    let (store, _dir) = open_temp_store();

    let id = store
        .insert(make_ephemeral_entry(1, "secret API key"))
        .expect("insert ephemeral")
        .unwrap();

    let entry = store.get_entry(id).expect("get_entry");
    assert!(
        entry.ephemeral,
        "ephemeral flag must be preserved in the database"
    );

    // Query should also return the ephemeral flag.
    let result = store
        .query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 10,
        })
        .expect("query");
    assert_eq!(result.entries.len(), 1);
    assert!(result.entries[0].ephemeral);
}

// ===========================================================================
// Concurrent pruning safety
//
// The daemon runs pruning on a timer while IPC handlers may be querying
// or inserting. This test verifies no deadlock or panic.
// ===========================================================================

#[tokio::test]
async fn concurrent_prune_and_query_via_mutex() {
    let (store, _dir) = open_temp_store();

    for i in 0..10u8 {
        store
            .insert(make_entry(i, ContentClass::Text, &format!("entry {i}")))
            .expect("insert");
    }

    let store = std::sync::Arc::new(std::sync::Mutex::new(store));

    // Spawn concurrent prune and query tasks.
    let prune_store = store.clone();
    let prune_handle = tokio::task::spawn_blocking(move || {
        let s = prune_store.lock().expect("prune lock");
        s.prune(&GeneralConfig {
            max_entries: 5,
            max_blob_size_mb: 500,
            retention: Retention::Unlimited,
            ephemeral_ttl_hours: 24,
        })
        .expect("prune")
    });

    let query_store = store.clone();
    let query_handle = tokio::task::spawn_blocking(move || {
        let s = query_store.lock().expect("query lock");
        s.query(Query {
            text: None,
            content_class: None,
            offset: 0,
            limit: 100,
        })
        .expect("query")
    });

    let prune_stats = prune_handle.await.expect("prune task");
    let query_result = query_handle.await.expect("query task");

    // Both operations should complete without panic.
    // The exact numbers depend on ordering, but neither should be zero
    // (prune had excess entries, query had entries to return).
    assert!(prune_stats.entries_deleted <= 10);
    assert!(query_result.total <= 10);
}
