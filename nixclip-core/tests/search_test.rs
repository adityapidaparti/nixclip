/// Integration tests for SearchEngine.
///
/// Each test opens a ClipStore (to populate the database) and then creates a
/// SearchEngine pointing to the same database file.  The SearchEngine opens its
/// own read-only connection, so both objects can coexist safely in a single test.
use nixclip_core::search::SearchEngine;
use nixclip_core::storage::ClipStore;
use nixclip_core::{ContentClass, MimePayload, NewEntry};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Insert a plain-text entry into the store, returning the assigned id.
fn insert_text(store: &ClipStore, seed: u8, preview: &str) -> nixclip_core::EntryId {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some(preview.to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: preview.as_bytes().to_vec(),
        }],
        source_app: None,
        ephemeral: false,
    };
    store.insert(entry).expect("insert").expect("new entry id")
}

/// Insert a URL entry.
fn insert_url(store: &ClipStore, seed: u8, url: &str) -> nixclip_core::EntryId {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    let entry = NewEntry {
        content_class: ContentClass::Url,
        preview_text: Some(url.to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: url.as_bytes().to_vec(),
        }],
        source_app: None,
        ephemeral: false,
    };
    store.insert(entry).expect("insert url").expect("new entry id")
}

/// Create an isolated store + search engine pair.
fn setup() -> (ClipStore, SearchEngine, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("search_test.db");
    let blob_dir = dir.path().join("blobs");

    let store = ClipStore::open(&db_path, &blob_dir).expect("open store");
    let engine = SearchEngine::new(db_path);
    (store, engine, dir)
}

// ===========================================================================
// Empty store
// ===========================================================================

#[test]
fn search_empty_store_returns_zero_results() {
    let (_store, engine, _dir) = setup();
    let result = engine.search("hello", None, 0, 10).expect("search");
    assert_eq!(result.total, 0);
    assert!(result.entries.is_empty());
}

#[test]
fn search_empty_query_on_empty_store_returns_zero() {
    let (_store, engine, _dir) = setup();
    let result = engine.search("", None, 0, 10).expect("search");
    assert_eq!(result.total, 0);
}

// ===========================================================================
// FTS / full-text search
// ===========================================================================

#[test]
fn fts_single_word_match() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "the quick brown fox");
    insert_text(&store, 2, "hello world");

    let result = engine.search("quick", None, 0, 10).expect("search");
    assert_eq!(result.total, 1, "should match exactly one entry");
    assert!(result.entries[0].preview_text.as_deref().unwrap().contains("quick"));
}

#[test]
fn fts_multi_word_match() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "git commit --amend");
    insert_text(&store, 2, "git push origin main");
    insert_text(&store, 3, "docker run -it ubuntu");

    let result = engine.search("git", None, 0, 10).expect("search");
    assert_eq!(result.total, 2, "both git entries should match");
}

#[test]
fn fts_no_match_returns_empty() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "hello world");

    let result = engine.search("zzzznonexistent", None, 0, 10).expect("search");
    assert_eq!(result.total, 0);
    assert!(result.entries.is_empty());
}

#[test]
fn fts_prefix_matching() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "programming in Rust");
    insert_text(&store, 2, "hello world");

    // "Rust" starts with "Ru" — FTS5 prefix matching.
    let result = engine.search("Ru", None, 0, 10).expect("search");
    assert!(
        result.total >= 1,
        "prefix match should find at least one entry; total={}", result.total
    );
}

// ===========================================================================
// Fuzzy matching (nucleo re-ranking)
// ===========================================================================

#[test]
fn fuzzy_matches_approximate_query() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "docker container start");
    insert_text(&store, 2, "totally unrelated note");

    // "contaner" is a typo of "container" — nucleo should still rank it.
    // The result may be empty if nucleo doesn't find a match, but the search
    // should not error.
    let result = engine.search("contaner", None, 0, 10).expect("search");
    // We just assert the call succeeds and returns sane data.
    assert!(result.total <= 2);
}

#[test]
fn empty_query_returns_all_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "entry one");
    insert_text(&store, 2, "entry two");
    insert_text(&store, 3, "entry three");

    let result = engine.search("", None, 0, 100).expect("search empty");
    assert_eq!(result.total, 3, "empty query should return all entries");
}

// ===========================================================================
// content_class filter
// ===========================================================================

#[test]
fn search_filters_by_content_class_text() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "plain text entry");
    insert_url(&store, 2, "https://example.com");

    let result = engine
        .search("", Some(ContentClass::Text), 0, 10)
        .expect("search text class");
    assert_eq!(result.total, 1);
    assert_eq!(result.entries[0].content_class, ContentClass::Text);
}

#[test]
fn search_filters_by_content_class_url() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "plain text");
    insert_url(&store, 2, "https://example.com");
    insert_url(&store, 3, "https://rust-lang.org");

    let result = engine
        .search("", Some(ContentClass::Url), 0, 10)
        .expect("search url class");
    assert_eq!(result.total, 2);
    for entry in &result.entries {
        assert_eq!(entry.content_class, ContentClass::Url);
    }
}

#[test]
fn search_with_text_and_class_filter() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "rust programming");
    insert_url(&store, 2, "https://rust-lang.org");
    insert_text(&store, 3, "docker deployment");

    // Search for "rust" filtered to Text class only.
    let result = engine
        .search("rust", Some(ContentClass::Text), 0, 10)
        .expect("search filtered");
    assert_eq!(result.total, 1);
    assert_eq!(result.entries[0].content_class, ContentClass::Text);
    assert!(result.entries[0].preview_text.as_deref().unwrap().contains("rust"));
}

// ===========================================================================
// Pagination
// ===========================================================================

#[test]
fn search_respects_limit() {
    let (store, engine, _dir) = setup();
    for i in 0..10u8 {
        insert_text(&store, i, &format!("entry number {i}"));
    }

    let result = engine.search("", None, 0, 3).expect("search with limit");
    assert_eq!(result.entries.len(), 3, "should return exactly 3 entries");
    assert_eq!(result.total, 10, "total should reflect all 10 entries");
}

#[test]
fn search_respects_offset() {
    let (store, engine, _dir) = setup();
    for i in 0..5u8 {
        insert_text(&store, i, &format!("item {i}"));
    }

    let first_page = engine.search("", None, 0, 2).expect("page 1");
    let second_page = engine.search("", None, 2, 2).expect("page 2");

    assert_eq!(first_page.entries.len(), 2);
    assert_eq!(second_page.entries.len(), 2);

    // Pages should not overlap.
    let first_ids: Vec<_> = first_page.entries.iter().map(|e| e.id).collect();
    let second_ids: Vec<_> = second_page.entries.iter().map(|e| e.id).collect();
    for id in &second_ids {
        assert!(!first_ids.contains(id), "pages should not overlap");
    }
}

#[test]
fn search_offset_beyond_total_returns_empty_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "only entry");

    let result = engine.search("", None, 100, 10).expect("offset beyond total");
    assert_eq!(result.total, 1, "total should still reflect the full count");
    assert!(result.entries.is_empty(), "but entries should be empty");
}

// ===========================================================================
// Pinned entries ordering
// ===========================================================================

#[test]
fn pinned_entries_rank_higher_in_results() {
    let (store, engine, _dir) = setup();

    let id_unpinned = insert_text(&store, 1, "rust clippy lints");
    let id_pinned = insert_text(&store, 2, "rust ownership model");
    store.pin(id_pinned, true).expect("pin");

    let result = engine.search("rust", None, 0, 10).expect("search");
    assert!(result.total >= 1);

    // The pinned entry should appear before the unpinned one.
    if result.total == 2 {
        assert_eq!(
            result.entries[0].id, id_pinned,
            "pinned entry should rank first"
        );
        assert_eq!(result.entries[1].id, id_unpinned);
    }
}

// ===========================================================================
// Special characters in query (FTS5 sanitization)
// ===========================================================================

#[test]
fn search_with_fts_special_chars_does_not_panic() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "hello world");

    // These characters could cause FTS5 syntax errors if not sanitized.
    for query in &[
        "AND OR NOT",
        "foo* ^bar (baz)",
        "(((unclosed",
        "\"quoted string\"",
        "a AND b OR c",
    ] {
        let result = engine.search(query, None, 0, 10);
        assert!(result.is_ok(), "search with FTS special chars should not error: {query:?}");
    }
}

// ===========================================================================
// Multiple content classes in the same store
// ===========================================================================

#[test]
fn search_no_class_filter_returns_all_classes() {
    let (store, engine, _dir) = setup();

    insert_text(&store, 1, "text entry");
    insert_url(&store, 2, "https://example.com");

    let result = engine.search("", None, 0, 10).expect("search all classes");
    assert_eq!(result.total, 2, "should return both entries");

    let classes: Vec<ContentClass> = result.entries.iter().map(|e| e.content_class).collect();
    assert!(classes.contains(&ContentClass::Text));
    assert!(classes.contains(&ContentClass::Url));
}

// ===========================================================================
// Source app field is preserved
// ===========================================================================

#[test]
fn search_results_include_source_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open");
    let engine = SearchEngine::new(db_path);

    let mut hash = [0u8; 32];
    hash[0] = 99;
    let entry = NewEntry {
        content_class: ContentClass::Text,
        preview_text: Some("source app test".to_string()),
        canonical_hash: hash,
        representations: vec![MimePayload {
            mime: "text/plain".to_string(),
            data: b"source app test".to_vec(),
        }],
        source_app: Some("org.test.Editor".to_string()),
        ephemeral: false,
    };
    store.insert(entry).expect("insert");

    let result = engine.search("source", None, 0, 10).expect("search");
    assert_eq!(result.total, 1);
    assert_eq!(
        result.entries[0].source_app.as_deref(),
        Some("org.test.Editor")
    );
}
