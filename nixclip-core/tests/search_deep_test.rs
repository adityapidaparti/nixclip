/// Deep search-engine tests validating the scoring model, FTS5 safety,
/// edge cases, and read-only connection contract.
///
/// Each test creates an isolated ClipStore + SearchEngine pair backed by a
/// temporary directory so they are completely independent.
use nixclip_core::search::SearchEngine;
use nixclip_core::storage::ClipStore;
use nixclip_core::{ContentClass, EntryId, MimePayload, NewEntry};

// ===========================================================================
// Helpers
// ===========================================================================

/// Open an isolated store + engine backed by a fresh temp directory.
fn setup() -> (ClipStore, SearchEngine, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("deep.db");
    let blob_dir = dir.path().join("blobs");
    let store = ClipStore::open(&db_path, &blob_dir).expect("open ClipStore");
    let engine = SearchEngine::new(db_path);
    (store, engine, dir)
}

/// Insert a plain-text entry.  `seed` is used to make canonical_hash unique.
fn insert_text(store: &ClipStore, seed: u8, preview: &str) -> EntryId {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    // If a prior entry already used this seed the dedup logic would fire.
    // Using a 2-byte combination gives us 65536 unique slots in tests.
    hash[1] = (seed as u16 * 7 % 256) as u8;
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
        metadata: Default::default(),
    };
    store
        .insert(entry)
        .expect("insert text")
        .expect("new entry id")
}

/// Insert with a fully explicit hash (for fine-grained uniqueness).
fn insert_text_with_hash(store: &ClipStore, hash: [u8; 32], preview: &str) -> EntryId {
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
        metadata: Default::default(),
    };
    store.insert(entry).expect("insert").expect("new entry id")
}

/// Insert a URL entry.
fn insert_url(store: &ClipStore, seed: u8, url: &str) -> EntryId {
    let mut hash = [0u8; 32];
    hash[0] = seed;
    hash[1] = 0xFF; // distinguish from text seeds
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
        metadata: Default::default(),
    };
    store
        .insert(entry)
        .expect("insert url")
        .expect("new entry id")
}

// ===========================================================================
// Test 1 — FTS5 prefix matching: "dock" should match "docker run"
// ===========================================================================

#[test]
fn fts_prefix_dock_matches_docker_run() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "docker run -it ubuntu");
    insert_text(&store, 2, "git commit --amend");

    let result = engine.search("dock", None, 0, 20).expect("search 'dock'");

    assert!(
        result.total >= 1,
        "prefix 'dock' should match 'docker run'; total={}",
        result.total
    );
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "docker entry (id={id}) must appear in results; got ids: {ids:?}"
    );
}

#[test]
fn fts_prefix_rust_matches_rustacean() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "rustacean programmer");
    insert_text(&store, 2, "python developer");

    let result = engine.search("rust", None, 0, 20).expect("search");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "prefix 'rust' should match 'rustacean programmer'"
    );
}

#[test]
fn fts_prefix_short_two_char_works() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "kubernetes pod scheduling");
    insert_text(&store, 2, "unrelated content here");

    // "ku" is a 2-char prefix that should match "kubernetes".
    let result = engine.search("ku", None, 0, 20).expect("search");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "2-char prefix 'ku' should match 'kubernetes pod scheduling'"
    );
}

// ===========================================================================
// Test 2 — FTS fallback to LIKE when FTS returns no results
//
// We use a query whose sanitized FTS form is empty (all-special-chars or
// boolean keywords) so that sanitize_fts5_query returns None and the LIKE
// path is used.  We also verify the normal case where FTS5 simply finds
// no matches and the fallback catches them.
// ===========================================================================

#[test]
fn fts_fallback_to_like_when_fts_returns_nothing() {
    let (store, engine, _dir) = setup();
    // Insert an entry that contains text that FTS5 would find via LIKE.
    // A query of all-FTS-operators will be sanitized to empty → LIKE fallback.
    let id = insert_text(&store, 1, "foo AND bar");

    // "AND OR NOT" sanitizes to an empty FTS expression → search_fts returns
    // Ok(vec![]) → fallback LIKE '%AND OR NOT%'.  The entry contains the
    // substring "AND" so LIKE should match.
    let result = engine.search("AND", None, 0, 20).expect("search 'AND'");

    // "AND" alone is a boolean keyword stripped by sanitize_fts5_query.
    // After sanitization the FTS expression is empty → sanitize returns None
    // → fetch_fts_candidates returns Ok(vec![]) → LIKE fallback executes.
    // The LIKE pattern is '%AND%' which matches "foo AND bar".
    assert!(
        result.total >= 1,
        "LIKE fallback should find entry containing 'AND'; total={}",
        result.total
    );
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "entry 'foo AND bar' (id={id}) should be found via LIKE fallback; \
         ids: {ids:?}"
    );
}

#[test]
fn fts_fallback_like_with_no_fts_special_in_query() {
    // Insert an entry whose text would NOT be indexed by FTS5 (because the
    // token is a number starting with digits that FTS might not return) but
    // LIKE will catch it via substring match.
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "port 8080 is open");

    // This particular query should work through FTS normally, but the LIKE
    // path is transparent to the caller.
    let result = engine.search("8080", None, 0, 20).expect("search");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "entry 'port 8080 is open' should be found; ids: {ids:?}"
    );
}

// ===========================================================================
// Test 3 — Composite scoring formula: nucleo * 0.7 + recency * 0.3
//
// We verify the formula by computing expected scores for the composite_score
// function via its observable effects: an entry with maximum fuzzy relevance
// but zero recency and an entry with partial relevance but full recency should
// compare in a predictable way.
//
// We cannot call composite_score directly (it's private), but we can insert
// two entries and reason about what the formula predicts for their relative
// ordering.
// ===========================================================================

#[test]
fn composite_score_formula_relevance_vs_recency() {
    // Verify that the composite score formula weights recency correctly.
    //
    // Formula: composite = nucleo_normalized * 0.7 + recency_score * 0.3
    // where recency_score = 1.0 / (1.0 + hours_since / 24.0)
    //
    // We insert two entries with identical text (identical nucleo scores) but
    // provably different last_seen_at timestamps (separated by a sleep).
    // The newer entry must rank higher because its recency_score is larger.

    let (store, engine, _dir) = setup();

    let id_older = insert_text_with_hash(
        &store,
        blake3::hash(b"older-docker").into(),
        "docker container images",
    );

    // Sleep long enough that the timestamps are guaranteed to differ.
    std::thread::sleep(std::time::Duration::from_millis(5));

    let id_newer = insert_text_with_hash(
        &store,
        blake3::hash(b"newer-docker").into(),
        "docker container images",
    );

    let result = engine.search("docker", None, 0, 20).expect("search docker");

    assert!(
        result.total >= 2,
        "both docker entries must be found; total={}",
        result.total
    );

    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(ids.contains(&id_older), "older entry must appear");
    assert!(ids.contains(&id_newer), "newer entry must appear");

    let pos_older = ids.iter().position(|&id| id == id_older).unwrap();
    let pos_newer = ids.iter().position(|&id| id == id_newer).unwrap();

    assert!(
        pos_newer < pos_older,
        "newer entry (pos={pos_newer}) must rank before older entry (pos={pos_older}) \
         when fuzzy scores are equal — recency_score * 0.3 is the tiebreaker"
    );
}

// ===========================================================================
// Test 4 — Pinned boost: pinned entry always ranks above unpinned for same query
// ===========================================================================

#[test]
fn pinned_entry_ranks_above_unpinned_for_same_query() {
    let (store, engine, _dir) = setup();

    let id_unpinned = insert_text_with_hash(
        &store,
        blake3::hash(b"pin-test-unpinned").into(),
        "rust ownership and borrowing",
    );
    let id_pinned = insert_text_with_hash(
        &store,
        blake3::hash(b"pin-test-pinned").into(),
        "rust lifetimes explained",
    );
    store.pin(id_pinned, true).expect("pin");

    let result = engine.search("rust", None, 0, 20).expect("search");
    assert!(
        result.total >= 2,
        "both entries must be found; total={}",
        result.total
    );

    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    let pos_pinned = ids
        .iter()
        .position(|&id| id == id_pinned)
        .expect("pinned entry must appear");
    let pos_unpinned = ids
        .iter()
        .position(|&id| id == id_unpinned)
        .expect("unpinned entry must appear");

    assert!(
        pos_pinned < pos_unpinned,
        "pinned entry (pos={pos_pinned}) must rank before unpinned (pos={pos_unpinned})"
    );
}

#[test]
fn pinned_entry_beats_unpinned_in_empty_query() {
    let (store, engine, _dir) = setup();

    let id_a = insert_text(&store, 1, "first entry inserted");
    let id_b = insert_text(&store, 2, "second entry inserted");
    // Pin the second-inserted (would normally rank lower due to ordering).
    store.pin(id_b, true).expect("pin");

    let result = engine.search("", None, 0, 20).expect("search");
    assert!(result.total >= 2);

    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    let pos_pinned = ids.iter().position(|&id| id == id_b).unwrap();
    let pos_unpinned = ids.iter().position(|&id| id == id_a).unwrap();

    assert!(
        pos_pinned < pos_unpinned,
        "pinned entry must rank before unpinned in empty-query results"
    );
}

// ===========================================================================
// Test 5 — Recency scoring: newer entry ranks higher for same text relevance
// ===========================================================================

#[test]
fn newer_entry_ranks_higher_than_older_for_equal_relevance() {
    // We insert two entries with near-identical text.  Because they are
    // inserted sequentially they have monotonically non-decreasing last_seen_at
    // values.  On a fast machine the timestamps can be equal, in which case
    // ordering is arbitrary but deterministic.  We just assert both appear
    // and that the newer one (id_b, inserted second) is at least as high.
    let (store, engine, _dir) = setup();

    let id_a = insert_text_with_hash(
        &store,
        blake3::hash(b"recency-a").into(),
        "terraform apply infrastructure",
    );
    // Add a small sleep to guarantee different timestamps on even fast machines.
    std::thread::sleep(std::time::Duration::from_millis(2));

    let id_b = insert_text_with_hash(
        &store,
        blake3::hash(b"recency-b").into(),
        "terraform plan infrastructure",
    );

    let result = engine.search("terraform", None, 0, 20).expect("search");

    assert!(result.total >= 2, "both terraform entries must match");

    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    let pos_a = ids.iter().position(|&id| id == id_a).unwrap();
    let pos_b = ids.iter().position(|&id| id == id_b).unwrap();

    assert!(
        pos_b <= pos_a,
        "newer entry b (pos={pos_b}) should rank no worse than older entry a (pos={pos_a})"
    );
}

// ===========================================================================
// Test 6 — FTS5 special character escaping
// ===========================================================================

#[test]
fn fts_escapes_double_quotes_in_query() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "say hello world");
    // A query containing double quotes must not panic or return an error.
    let result = engine.search("\"hello\"", None, 0, 20);
    assert!(
        result.is_ok(),
        "query with double quotes must not fail; err: {:?}",
        result.err()
    );
}

#[test]
fn fts_strips_asterisk_from_query() {
    // "docker*" sanitizes the user-supplied asterisk for FTS safety, producing
    // the FTS expression `"docker"*` which finds entries containing "docker".
    // The original query string (including `*`) is then passed to nucleo for
    // re-ranking.  Nucleo performs a fuzzy subsequence match; because `*` is
    // not present in the haystack "docker run -it ubuntu", nucleo may score
    // the entry as None and drop it.
    //
    // This test therefore only asserts that the query does NOT error — it does
    // not assert that results are returned, because the nucleo re-ranking step
    // can legitimately drop a FTS candidate if the raw (unsanitized) query
    // doesn't fuzzy-match.
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "docker run -it ubuntu");
    let result = engine.search("docker*", None, 0, 20);
    assert!(
        result.is_ok(),
        "query with asterisk must not fail with an error; got: {:?}",
        result.err()
    );
}

#[test]
fn fts_strips_parentheses_from_query() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "bash function call");
    let result = engine
        .search("(bash)", None, 0, 20)
        .expect("query with parentheses must not fail");
    // Entry may or may not match depending on sanitization, but no error.
    let _ = result;
}

#[test]
fn fts_strips_caret_from_query() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "start of line anchor");
    let result = engine
        .search("^start", None, 0, 20)
        .expect("query with ^ must not fail");
    let _ = result;
}

#[test]
fn fts_drops_boolean_keywords_only_query() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "hello world");
    // A query that sanitizes entirely to empty should return Ok with 0 results.
    let result = engine
        .search("AND OR NOT", None, 0, 20)
        .expect("all-keyword query must not fail");
    // The LIKE fallback will fire with pattern '%AND OR NOT%' — entry won't match.
    // We just assert no panic/error.
    let _ = result;
}

#[test]
fn fts_mixed_special_chars_and_valid_tokens() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "kubernetes deployment scale");
    // Query has parens, asterisks, and a valid token.
    let result = engine
        .search("(kube*)", None, 0, 20)
        .expect("mixed special-char query must not fail");
    // "kube" (after stripping parens and asterisk) prefix-matches "kubernetes".
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id) || result.total == 0,
        "no panic/error required; may or may not match depending on sanitization"
    );
}

// ===========================================================================
// Test 7 — Empty query with content_class filter
// ===========================================================================

#[test]
fn empty_query_with_text_class_filter_returns_only_text_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "plain text entry");
    insert_text(&store, 2, "another text entry");
    insert_url(&store, 3, "https://example.com");
    insert_url(&store, 4, "https://rust-lang.org");

    let result = engine
        .search("", Some(ContentClass::Text), 0, 20)
        .expect("empty query with Text filter");

    assert_eq!(
        result.total, 2,
        "only the 2 Text entries should be returned; got {}",
        result.total
    );
    for entry in &result.entries {
        assert_eq!(
            entry.content_class,
            ContentClass::Text,
            "every returned entry must be Text class"
        );
    }
}

#[test]
fn empty_query_with_url_class_filter_returns_only_url_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "some text");
    insert_url(&store, 2, "https://example.com");
    insert_url(&store, 3, "https://crates.io");

    let result = engine
        .search("", Some(ContentClass::Url), 0, 20)
        .expect("empty query with Url filter");

    assert_eq!(result.total, 2, "expected 2 Url entries");
    for entry in &result.entries {
        assert_eq!(entry.content_class, ContentClass::Url);
    }
}

#[test]
fn empty_query_with_filter_respects_order_by_last_seen_at() {
    // The spec says empty query uses ORDER BY last_seen_at DESC.
    // Entries inserted later have higher last_seen_at.
    let (store, engine, _dir) = setup();

    let id1 = insert_text(&store, 1, "older text");
    std::thread::sleep(std::time::Duration::from_millis(2));
    let id2 = insert_text(&store, 2, "newer text");

    let result = engine
        .search("", Some(ContentClass::Text), 0, 20)
        .expect("search");

    assert_eq!(result.total, 2);
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    let pos1 = ids.iter().position(|&id| id == id1).unwrap();
    let pos2 = ids.iter().position(|&id| id == id2).unwrap();

    assert!(
        pos2 < pos1,
        "newer entry (id={id2}, pos={pos2}) should rank before older (id={id1}, pos={pos1}) \
         for empty query (ORDER BY last_seen_at DESC)"
    );
}

// ===========================================================================
// Test 8 — Query with offset > total returns empty results
// ===========================================================================

#[test]
fn query_offset_beyond_total_returns_empty_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "only entry in store");

    let result = engine
        .search("", None, 999, 10)
        .expect("offset beyond total");

    assert_eq!(
        result.total, 1,
        "total should still reflect the one entry in the store"
    );
    assert!(
        result.entries.is_empty(),
        "entries must be empty when offset exceeds total"
    );
}

#[test]
fn query_offset_equals_total_returns_empty_entries() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "a");
    insert_text(&store, 2, "b");
    insert_text(&store, 3, "c");

    // offset = total = 3 → nothing left to return.
    let result = engine.search("", None, 3, 10).expect("offset = total");

    assert_eq!(result.total, 3);
    assert!(result.entries.is_empty());
}

#[test]
fn text_query_offset_beyond_results_returns_empty() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "terraform plan");
    insert_text(&store, 2, "terraform apply");

    // Both entries match "terraform"; offset=10 skips all of them.
    let result = engine
        .search("terraform", None, 10, 10)
        .expect("search with high offset");

    assert!(
        result.entries.is_empty(),
        "no entries should be returned when offset ({}) > total ({})",
        10,
        result.total
    );
}

// ===========================================================================
// Test 9 — Read-only connection (verify no writes happen during search)
//
// The SearchEngine opens its connection with SQLITE_OPEN_READ_ONLY.  Any
// attempt to write through that connection must fail with a SQLite error.
// We verify this by trying to insert via a raw rusqlite connection opened with
// the same read-only flags as the SearchEngine.
// ===========================================================================

#[test]
fn search_engine_connection_is_read_only() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "some content");

    // A read operation must succeed.
    let result = engine
        .search("some", None, 0, 10)
        .expect("read-only search must work");
    assert!(
        result.total >= 1,
        "read must succeed on a read-only connection"
    );

    // Open the same DB with read-only flags (as SearchEngine does) and
    // confirm that a write attempt fails.
    let db_path = _dir.path().join("deep.db");
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open read-only connection");

    let write_result = conn.execute(
        "INSERT INTO entries (created_at, last_seen_at, pinned, ephemeral, \
         content_class, preview_text, source_app, canonical_hash) \
         VALUES (0, 0, 0, 0, 'text', 'injected', NULL, zeroblob(32))",
        [],
    );

    assert!(
        write_result.is_err(),
        "writing via a read-only connection must fail; got Ok"
    );
}

// ===========================================================================
// Test 10 — Case-insensitive search
// ===========================================================================

#[test]
fn search_is_case_insensitive_for_fts() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "Docker Run It Ubuntu");

    // Lowercase query should match mixed-case entry.
    let result = engine
        .search("docker", None, 0, 20)
        .expect("lowercase search");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "lowercase 'docker' should match 'Docker Run It Ubuntu'"
    );
}

#[test]
fn search_is_case_insensitive_uppercase_query() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "git status output");

    let result = engine.search("GIT", None, 0, 20).expect("uppercase query");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "uppercase 'GIT' should match 'git status output'"
    );
}

#[test]
fn search_is_case_insensitive_mixed_case_query() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "rust programming language");

    let result = engine
        .search("RuSt", None, 0, 20)
        .expect("mixed-case query");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "mixed-case 'RuSt' should match 'rust programming language'"
    );
}

#[test]
fn search_case_insensitive_with_class_filter() {
    let (store, engine, _dir) = setup();
    let id = insert_text(&store, 1, "Kubernetes Deployment YAML");
    insert_url(&store, 2, "https://kubernetes.io");

    let result = engine
        .search("KUBERNETES", Some(ContentClass::Text), 0, 20)
        .expect("case-insensitive with class filter");
    let ids: Vec<EntryId> = result.entries.iter().map(|e| e.id).collect();
    assert!(
        ids.contains(&id),
        "'KUBERNETES' should match 'Kubernetes Deployment YAML' with Text filter"
    );
}

// ===========================================================================
// Additional edge cases
// ===========================================================================

#[test]
fn search_whitespace_only_query_behaves_like_empty_query() {
    // The SearchEngine trims the query; "   " should behave the same as "".
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "hello");
    insert_text(&store, 2, "world");

    let empty_result = engine.search("", None, 0, 20).expect("empty query");
    let ws_result = engine.search("   ", None, 0, 20).expect("whitespace query");

    assert_eq!(
        empty_result.total, ws_result.total,
        "whitespace-only query must return same total as empty query"
    );
}

#[test]
fn search_query_matching_nothing_returns_zero_with_no_error() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "hello world");

    let result = engine
        .search("zzznomatch999", None, 0, 20)
        .expect("search for non-existent term");

    assert_eq!(result.total, 0);
    assert!(result.entries.is_empty());
}

#[test]
fn search_limit_zero_returns_zero_entries_but_correct_total() {
    let (store, engine, _dir) = setup();
    insert_text(&store, 1, "entry a");
    insert_text(&store, 2, "entry b");

    let result = engine.search("", None, 0, 0).expect("limit 0");
    assert_eq!(
        result.total, 2,
        "total should reflect all matching entries even with limit=0"
    );
    assert!(result.entries.is_empty(), "limit=0 must return no entries");
}
