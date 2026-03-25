//! Fuzzy search over clipboard history using FTS5 for candidate retrieval and
//! nucleo-matcher for re-ranking.

use rusqlite::{Connection, OpenFlags};
use tracing::{debug, warn};

use crate::{ContentClass, EntryMetadata, EntrySummary, QueryResult};
use crate::error::Result;

// ---------------------------------------------------------------------------
// SearchEngine
// ---------------------------------------------------------------------------

pub struct SearchEngine {
    db_path: std::path::PathBuf,
}

impl SearchEngine {
    pub fn new(db_path: std::path::PathBuf) -> Self {
        Self { db_path }
    }

    /// Search clipboard entries with fuzzy matching.
    ///
    /// Uses FTS5 for initial candidate retrieval, then nucleo for re-ranking.
    /// The connection is opened read-only; callers should wrap this in
    /// `tokio::task::spawn_blocking` if calling from async context.
    pub fn search(
        &self,
        text: &str,
        content_class: Option<ContentClass>,
        offset: u32,
        limit: u32,
    ) -> Result<QueryResult> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let trimmed = text.trim();

        // Retrieve candidates from the database (up to 500).
        let candidates = if trimmed.is_empty() {
            fetch_all_candidates(&conn, content_class)?
        } else {
            let fts_result = fetch_fts_candidates(&conn, trimmed, content_class);
            match fts_result {
                Ok(rows) if !rows.is_empty() => rows,
                Ok(_) => {
                    // FTS5 returned nothing — try LIKE fallback.
                    debug!("FTS5 returned no results, falling back to LIKE query");
                    fetch_like_candidates(&conn, trimmed, content_class)?
                }
                Err(e) => {
                    // FTS5 query syntax error or other FTS error — fall back.
                    warn!("FTS5 query failed ({}), falling back to LIKE query", e);
                    fetch_like_candidates(&conn, trimmed, content_class)?
                }
            }
        };

        // Re-rank candidates with nucleo-matcher (or fallback scorer).
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut scored: Vec<(EntrySummary, f64)> = if trimmed.is_empty() {
            // No query — only recency + pin scoring.
            candidates
                .into_iter()
                .map(|entry| {
                    let composite = composite_score(0.0, entry.last_seen_at, entry.pinned, now_ms);
                    (entry, composite)
                })
                .collect()
        } else {
            score_candidates(candidates, trimmed, now_ms)
        };

        // Sort by composite score descending.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let total = scored.len() as u32;

        let entries: Vec<EntrySummary> = scored
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(entry, _)| entry)
            .collect();

        Ok(QueryResult { entries, total })
    }
}

// ---------------------------------------------------------------------------
// FTS5 sanitisation
// ---------------------------------------------------------------------------

/// Sanitize user-supplied text for safe use in an FTS5 MATCH expression.
///
/// FTS5 special characters that need escaping or stripping:
///   `"` — starts a phrase; escaped by doubling inside a phrase.
///   `*` — prefix operator; strip from user input (we add our own).
///   `^` — initial-token operator; strip.
///   `-` — negation operator (at word start); strip from word starts.
///   `(`, `)` — grouping; strip.
///   `OR`, `AND`, `NOT` — boolean operators; left as-is (treated as literals
///     when they appear as full words they would act as operators, so strip).
///
/// We adopt a conservative strategy: keep only alphanumeric characters and
/// safe punctuation (hyphens inside words, apostrophes). Everything else is
/// dropped.  Returns `None` if the sanitized result is empty.
fn sanitize_fts5_query(raw: &str) -> Option<String> {
    // Split into whitespace-separated tokens, sanitise each token, then
    // reassemble as prefix-matching atoms: `token*`.
    let tokens: Vec<String> = raw
        .split_whitespace()
        .filter_map(|token| {
            // Strip any character that has FTS5 operator meaning.
            let cleaned: String = token
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '-')
                .collect();

            // Drop tokens that are FTS5 boolean keywords (case-insensitive).
            if cleaned.is_empty() {
                return None;
            }
            let upper = cleaned.to_uppercase();
            if upper == "OR" || upper == "AND" || upper == "NOT" {
                return None;
            }
            Some(cleaned)
        })
        .collect();

    if tokens.is_empty() {
        return None;
    }

    // Build: token1* token2* … (each token is prefix-matched).
    // Wrap each token in double-quotes to ensure it is treated as a phrase
    // literal (protects against any remaining special chars), then append *.
    let expr = tokens
        .iter()
        .map(|t| {
            // Escape any double-quotes within the token by doubling them
            // (FTS5 phrase quoting rule).
            let escaped = t.replace('"', "\"\"");
            format!("\"{}\"*", escaped)
        })
        .collect::<Vec<_>>()
        .join(" ");

    Some(expr)
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// Shared row-to-EntrySummary mapping for rusqlite.
fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntrySummary> {
    let content_class_str: String = row.get(5)?;
    let content_class = content_class_str
        .parse::<ContentClass>()
        .unwrap_or(ContentClass::Text);

    Ok(EntrySummary {
        id: row.get(0)?,
        created_at: row.get(1)?,
        last_seen_at: row.get(2)?,
        pinned: row.get::<_, i32>(3)? != 0,
        ephemeral: row.get::<_, i32>(4)? != 0,
        content_class,
        preview_text: row.get(6)?,
        source_app: row.get(7)?,
        // thumbnail is not stored in the entries table; set to None.
        thumbnail: None,
        metadata: EntryMetadata::default(),
    })
}

/// Fetch up to 500 candidates using FTS5 MATCH. May return a rusqlite::Error
/// if the FTS5 query is syntactically invalid (caller should fall back).
fn fetch_fts_candidates(
    conn: &Connection,
    query: &str,
    content_class: Option<ContentClass>,
) -> Result<Vec<EntrySummary>> {
    let fts_expr = match sanitize_fts5_query(query) {
        Some(expr) => expr,
        None => return Ok(vec![]),
    };

    let rows = match content_class {
        None => {
            let sql = "SELECT e.id, e.created_at, e.last_seen_at, e.pinned, e.ephemeral, \
                              e.content_class, e.preview_text, e.source_app \
                       FROM entries e \
                       JOIN search_idx s ON e.id = s.rowid \
                       WHERE search_idx MATCH ? \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([&fts_expr], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
        Some(class) => {
            let sql = "SELECT e.id, e.created_at, e.last_seen_at, e.pinned, e.ephemeral, \
                              e.content_class, e.preview_text, e.source_app \
                       FROM entries e \
                       JOIN search_idx s ON e.id = s.rowid \
                       WHERE search_idx MATCH ? \
                         AND e.content_class = ? \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([fts_expr.as_str(), class.as_str()], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
    };

    Ok(rows)
}

/// Fallback: LIKE-based candidate retrieval when FTS5 is unavailable or
/// returns no results.
fn fetch_like_candidates(
    conn: &Connection,
    query: &str,
    content_class: Option<ContentClass>,
) -> Result<Vec<EntrySummary>> {
    // Build the LIKE pattern. We use `?` binding — never interpolate user text.
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));

    let rows = match content_class {
        None => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE preview_text LIKE ? ESCAPE '\\' \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([&pattern], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
        Some(class) => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE preview_text LIKE ? ESCAPE '\\' \
                         AND content_class = ? \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([pattern.as_str(), class.as_str()], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
    };

    Ok(rows)
}

/// Fetch recent entries without text filtering (used when query is empty).
fn fetch_all_candidates(
    conn: &Connection,
    content_class: Option<ContentClass>,
) -> Result<Vec<EntrySummary>> {
    let rows = match content_class {
        None => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       ORDER BY last_seen_at DESC \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
        Some(class) => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE content_class = ? \
                       ORDER BY last_seen_at DESC \
                       LIMIT 500";
            let mut stmt = conn.prepare(sql)?;
            let result = stmt
                .query_map([class.as_str()], row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        }
    };

    Ok(rows)
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Compute the composite score for one entry.
///
/// * `fuzzy_score_normalized` — value in [0.0, 1.0]; 0.0 when there is no
///   query (all entries pass).
/// * `last_seen_ms` — Unix timestamp in milliseconds.
/// * `pinned` — whether the entry is pinned.
/// * `now_ms` — current Unix timestamp in milliseconds.
fn composite_score(
    fuzzy_score_normalized: f64,
    last_seen_ms: i64,
    pinned: bool,
    now_ms: i64,
) -> f64 {
    let hours_since = (now_ms - last_seen_ms).max(0) as f64 / 3_600_000.0;
    let recency_score = (1.0 / (1.0 + hours_since / 24.0)).min(1.0);

    let mut composite = fuzzy_score_normalized * 0.7 + recency_score * 0.3;
    if pinned {
        composite += 1000.0;
    }
    composite
}

/// Score candidates using nucleo-matcher (Pattern API) and build composite
/// scores.  Entries that receive no nucleo score are dropped (they don't
/// fuzzy-match the query at all).
fn score_candidates(
    candidates: Vec<EntrySummary>,
    query: &str,
    now_ms: i64,
) -> Vec<(EntrySummary, f64)> {
    use nucleo_matcher::{Config, Matcher, Utf32Str};
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Pattern};

    let mut matcher = Matcher::new(Config::DEFAULT);
    // Pattern::new splits on whitespace and matches each word as Fuzzy atoms,
    // which is the right behaviour for a search UI.
    let pattern = Pattern::new(query, CaseMatching::Ignore, AtomKind::Fuzzy);

    // Max possible nucleo score for normalisation.  nucleo scores are u32
    // when using Pattern::score.  We discover the max over this candidate set.
    let mut buf = Vec::new();

    // First pass: obtain raw scores and find the maximum.
    let mut raw_scores: Vec<Option<u32>> = Vec::with_capacity(candidates.len());
    let mut max_score: u32 = 1; // avoid division by zero

    for entry in &candidates {
        let haystack_str = entry
            .preview_text
            .as_deref()
            .unwrap_or("");
        let haystack = Utf32Str::new(haystack_str, &mut buf);
        let score = pattern.score(haystack, &mut matcher);
        if let Some(s) = score {
            if s > max_score {
                max_score = s;
            }
        }
        raw_scores.push(score);
    }

    // Second pass: build composite scores, dropping non-matching entries.
    candidates
        .into_iter()
        .zip(raw_scores.into_iter())
        .filter_map(|(entry, raw_score)| {
            let raw = raw_score?; // drop entries with no nucleo match
            let normalized = raw as f64 / max_score as f64;
            let composite =
                composite_score(normalized, entry.last_seen_at, entry.pinned, now_ms);
            Some((entry, composite))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Simple fallback fuzzy scorer (used only if nucleo API is unavailable)
// ---------------------------------------------------------------------------

/// Subsequence fuzzy score with gap penalty.
///
/// Returns a value in [0.0, 100.0].
///
/// ```text
/// score = (matched_chars / query_len) * 100 - (total_gaps * 2)
/// ```
#[allow(dead_code)]
fn simple_fuzzy_score(query: &str, candidate: &str) -> f64 {
    let query_chars: Vec<char> = query.to_lowercase().chars().collect();
    let cand_chars: Vec<char> = candidate.to_lowercase().chars().collect();

    if query_chars.is_empty() {
        return 100.0;
    }

    let mut qi = 0;
    let mut last_match: Option<usize> = None;
    let mut total_gaps: usize = 0;
    let mut matched: usize = 0;

    for (ci, &cc) in cand_chars.iter().enumerate() {
        if qi >= query_chars.len() {
            break;
        }
        if cc == query_chars[qi] {
            if let Some(prev) = last_match {
                let gap = ci - prev - 1;
                total_gaps += gap;
            }
            last_match = Some(ci);
            matched += 1;
            qi += 1;
        }
    }

    let ratio = matched as f64 / query_chars.len() as f64;
    let score = ratio * 100.0 - (total_gaps as f64 * 2.0);
    score.max(0.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        let expr = sanitize_fts5_query("docker run").unwrap();
        assert!(expr.contains("docker"), "should contain docker token");
        assert!(expr.contains("run"), "should contain run token");
        assert!(expr.contains('*'), "should have prefix wildcards");
    }

    #[test]
    fn sanitize_strips_operators() {
        // Boolean keywords should be dropped.
        let expr = sanitize_fts5_query("AND OR NOT").unwrap_or_default();
        assert!(expr.is_empty(), "all tokens stripped, expect empty: {:?}", expr);
    }

    #[test]
    fn sanitize_strips_special_chars() {
        let expr = sanitize_fts5_query("foo* ^bar (baz)").unwrap();
        // Special chars stripped; words retained with prefix wildcard.
        assert!(expr.contains("foo"), "foo retained");
        assert!(expr.contains("bar"), "bar retained");
        assert!(expr.contains("baz"), "baz retained");
        // No unquoted special chars.
        assert!(!expr.contains('^'), "^ stripped");
        assert!(!expr.contains('('), "( stripped");
    }

    #[test]
    fn sanitize_empty_input() {
        assert!(sanitize_fts5_query("").is_none());
        assert!(sanitize_fts5_query("   ").is_none());
    }

    #[test]
    fn simple_fuzzy_score_subsequence() {
        let score = simple_fuzzy_score("ab", "axbx");
        assert!(score > 0.0, "subsequence should match");
    }

    #[test]
    fn simple_fuzzy_score_no_match() {
        let score = simple_fuzzy_score("zz", "abc");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn composite_score_pinned_boost() {
        let unpinned = composite_score(1.0, 0, false, 0);
        let pinned = composite_score(1.0, 0, true, 0);
        assert!(pinned - unpinned > 999.0, "pinned should add ~1000");
    }

    #[test]
    fn composite_score_recency() {
        // Entry seen now vs. entry seen 240 hours ago.
        let now_ms = 1_000_000_000_i64;
        let recent = composite_score(0.5, now_ms, false, now_ms);
        let old = composite_score(0.5, now_ms - 240 * 3_600_000, false, now_ms);
        assert!(recent > old, "more recent entry should score higher");
    }
}
