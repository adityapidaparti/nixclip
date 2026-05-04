//! Fuzzy search over clipboard history using FTS5 for candidate retrieval and
//! nucleo-matcher for re-ranking.

use rusqlite::{Connection, OpenFlags};
use tracing::{debug, warn};

use crate::error::Result;
use crate::{ContentClass, EntryMetadata, EntrySummary, QueryResult};

const SEARCH_HEADROOM: u32 = 500;

// ---------------------------------------------------------------------------
// SearchEngine
// ---------------------------------------------------------------------------

#[derive(Clone)]
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
        let normalized_query = normalized_query_text(trimmed);

        if trimmed.is_empty() {
            let total = count_all_candidates(&conn, content_class)?;
            let entries = fetch_all_candidates_page(&conn, content_class, offset, limit)?;
            return Ok(QueryResult { entries, total });
        }

        let candidate_limit = offset
            .saturating_add(limit)
            .saturating_add(SEARCH_HEADROOM)
            .max(limit);

        // Retrieve candidates from the database.
        let (candidates, total) = {
            let fts_result = fetch_fts_candidates(&conn, trimmed, content_class, candidate_limit);
            match fts_result {
                Ok(rows) if !rows.is_empty() => {
                    (rows, count_fts_candidates(&conn, trimmed, content_class)?)
                }
                Ok(_) => {
                    debug!("FTS5 returned no results, falling back to LIKE query");
                    (
                        fetch_like_candidates(&conn, trimmed, content_class, candidate_limit)?,
                        count_like_candidates(&conn, trimmed, content_class)?,
                    )
                }
                Err(e) => {
                    warn!("FTS5 query failed ({}), falling back to LIKE query", e);
                    (
                        fetch_like_candidates(&conn, trimmed, content_class, candidate_limit)?,
                        count_like_candidates(&conn, trimmed, content_class)?,
                    )
                }
            }
        };

        // Re-rank candidates with nucleo-matcher (or fallback scorer).
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut scored = score_candidates(
            candidates,
            normalized_query.as_deref().unwrap_or(trimmed),
            now_ms,
        );

        // Sort by composite score descending.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

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
fn normalize_query_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in raw.chars() {
        if ch.is_alphanumeric() || ch == '\'' || ch == '-' {
            current.push(ch);
        } else if !current.is_empty() {
            let upper = current.to_uppercase();
            if upper != "OR" && upper != "AND" && upper != "NOT" {
                tokens.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }

    if !current.is_empty() {
        let upper = current.to_uppercase();
        if upper != "OR" && upper != "AND" && upper != "NOT" {
            tokens.push(current);
        }
    }

    tokens
}

fn normalized_query_text(raw: &str) -> Option<String> {
    let tokens = normalize_query_tokens(raw);
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

fn sanitize_fts5_query(raw: &str) -> Option<String> {
    // Split into whitespace-separated tokens, sanitise each token, then
    // reassemble as prefix-matching atoms: `token*`.
    let tokens = normalize_query_tokens(raw);

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
        match_ranges: vec![],
        metadata: EntryMetadata::default(),
    })
}

/// Fetch candidates using FTS5 MATCH. May return a rusqlite::Error
/// if the FTS5 query is syntactically invalid (caller should fall back).
fn fetch_fts_candidates(
    conn: &Connection,
    query: &str,
    content_class: Option<ContentClass>,
    limit: u32,
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
                       ORDER BY e.last_seen_at DESC, e.id DESC \
                       LIMIT ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map((fts_expr.as_str(), limit), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        }
        Some(class) => {
            let sql = "SELECT e.id, e.created_at, e.last_seen_at, e.pinned, e.ephemeral, \
                              e.content_class, e.preview_text, e.source_app \
                       FROM entries e \
                       JOIN search_idx s ON e.id = s.rowid \
                       WHERE search_idx MATCH ? \
                         AND e.content_class = ? \
                       ORDER BY e.last_seen_at DESC, e.id DESC \
                       LIMIT ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map((fts_expr.as_str(), class.as_str(), limit), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
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
    limit: u32,
) -> Result<Vec<EntrySummary>> {
    // Build the LIKE pattern. We use `?` binding — never interpolate user text.
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));

    let rows = match content_class {
        None => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE preview_text LIKE ? ESCAPE '\\' \
                          OR source_app LIKE ? ESCAPE '\\' \
                       ORDER BY last_seen_at DESC, id DESC \
                       LIMIT ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map((pattern.as_str(), pattern.as_str(), limit), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        }
        Some(class) => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE (preview_text LIKE ? ESCAPE '\\' \
                          OR source_app LIKE ? ESCAPE '\\') \
                         AND content_class = ? \
                       ORDER BY last_seen_at DESC, id DESC \
                       LIMIT ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map(
                    (pattern.as_str(), pattern.as_str(), class.as_str(), limit),
                    row_to_summary,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        }
    };

    Ok(rows)
}

fn count_fts_candidates(
    conn: &Connection,
    query: &str,
    content_class: Option<ContentClass>,
) -> Result<u32> {
    let fts_expr = match sanitize_fts5_query(query) {
        Some(expr) => expr,
        None => return Ok(0),
    };

    let total = match content_class {
        None => {
            let sql = "SELECT COUNT(*) \
                       FROM entries e \
                       JOIN search_idx s ON e.id = s.rowid \
                       WHERE search_idx MATCH ?";
            let mut stmt = conn.prepare(sql)?;
            stmt.query_row([fts_expr.as_str()], |row| row.get(0))?
        }
        Some(class) => {
            let sql = "SELECT COUNT(*) \
                       FROM entries e \
                       JOIN search_idx s ON e.id = s.rowid \
                       WHERE search_idx MATCH ? \
                         AND e.content_class = ?";
            let mut stmt = conn.prepare(sql)?;
            stmt.query_row((fts_expr.as_str(), class.as_str()), |row| row.get(0))?
        }
    };

    Ok(total)
}

fn count_like_candidates(
    conn: &Connection,
    query: &str,
    content_class: Option<ContentClass>,
) -> Result<u32> {
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));

    let total = match content_class {
        None => {
            let sql = "SELECT COUNT(*) \
                       FROM entries \
                       WHERE preview_text LIKE ? ESCAPE '\\' \
                          OR source_app LIKE ? ESCAPE '\\'";
            let mut stmt = conn.prepare(sql)?;
            stmt.query_row((pattern.as_str(), pattern.as_str()), |row| row.get(0))?
        }
        Some(class) => {
            let sql = "SELECT COUNT(*) \
                       FROM entries \
                       WHERE (preview_text LIKE ? ESCAPE '\\' \
                          OR source_app LIKE ? ESCAPE '\\') \
                         AND content_class = ?";
            let mut stmt = conn.prepare(sql)?;
            stmt.query_row(
                (pattern.as_str(), pattern.as_str(), class.as_str()),
                |row| row.get(0),
            )?
        }
    };

    Ok(total)
}

fn count_all_candidates(conn: &Connection, content_class: Option<ContentClass>) -> Result<u32> {
    let total = match content_class {
        None => {
            let mut stmt = conn.prepare("SELECT COUNT(*) FROM entries")?;
            stmt.query_row([], |row| row.get(0))?
        }
        Some(class) => {
            let mut stmt = conn.prepare("SELECT COUNT(*) FROM entries WHERE content_class = ?")?;
            stmt.query_row([class.as_str()], |row| row.get(0))?
        }
    };

    Ok(total)
}

/// Fetch recent entries without text filtering (used when query is empty).
fn fetch_all_candidates_page(
    conn: &Connection,
    content_class: Option<ContentClass>,
    offset: u32,
    limit: u32,
) -> Result<Vec<EntrySummary>> {
    let rows = match content_class {
        None => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       ORDER BY pinned DESC, last_seen_at DESC, id DESC \
                       LIMIT ? OFFSET ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map((limit, offset), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        }
        Some(class) => {
            let sql = "SELECT id, created_at, last_seen_at, pinned, ephemeral, \
                              content_class, preview_text, source_app \
                       FROM entries \
                       WHERE content_class = ? \
                       ORDER BY pinned DESC, last_seen_at DESC, id DESC \
                       LIMIT ? OFFSET ?";
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map((class.as_str(), limit, offset), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
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
/// scores.
fn score_candidates(
    candidates: Vec<EntrySummary>,
    query: &str,
    now_ms: i64,
) -> Vec<(EntrySummary, f64)> {
    use nucleo_matcher::pattern::{AtomKind, CaseMatching, Pattern};
    use nucleo_matcher::{Config, Matcher, Utf32Str};

    let mut matcher = Matcher::new(Config::DEFAULT);
    // Pattern::new splits on whitespace and matches each word as Fuzzy atoms,
    // which is the right behaviour for a search UI.
    let pattern = Pattern::new(query, CaseMatching::Ignore, AtomKind::Fuzzy);

    // Max possible nucleo score for normalisation.  nucleo scores are u32
    // when using Pattern::score.  We discover the max over this candidate set.
    let mut buf = Vec::new();

    // First pass: obtain raw scores + match indices, find the maximum score.
    let mut raw_scores: Vec<Option<(u32, Vec<u32>)>> = Vec::with_capacity(candidates.len());
    let mut max_score: u32 = 1; // avoid division by zero

    for entry in &candidates {
        let haystack_text = combined_haystack(entry);
        let haystack = Utf32Str::new(&haystack_text, &mut buf);
        let mut indices = Vec::new();
        let score = pattern.indices(haystack, &mut matcher, &mut indices);
        if let Some(s) = score {
            if s > max_score {
                max_score = s;
            }
            raw_scores.push(Some((s, indices)));
        } else {
            raw_scores.push(None);
        }
    }

    // Second pass: build composite scores, dropping non-matching entries.
    candidates
        .into_iter()
        .zip(raw_scores)
        .map(|(mut entry, raw_data)| {
            let (raw, char_indices) = raw_data.unwrap_or_default();
            let normalized = raw as f64 / max_score as f64;
            let composite = composite_score(normalized, entry.last_seen_at, entry.pinned, now_ms);

            // Convert character indices to (start_byte, length_bytes) ranges,
            // collapsing consecutive positions into contiguous ranges.
            let preview = entry.preview_text.as_deref().unwrap_or("");
            let char_to_byte: Vec<usize> = preview.char_indices().map(|(i, _)| i).collect();
            let char_lens: Vec<usize> = preview.chars().map(|c| c.len_utf8()).collect();

            let mut ranges: Vec<(u32, u32)> = Vec::new();
            let mut sorted_indices = char_indices;
            sorted_indices.sort_unstable();
            sorted_indices.dedup();

            let mut i = 0;
            while i < sorted_indices.len() {
                let start_char = sorted_indices[i] as usize;
                if start_char >= char_to_byte.len() {
                    i += 1;
                    continue;
                }
                let start_byte = char_to_byte[start_char];
                let mut end_char = start_char;

                // Extend range while characters are consecutive.
                while i + 1 < sorted_indices.len() && sorted_indices[i + 1] as usize == end_char + 1
                {
                    end_char = sorted_indices[i + 1] as usize;
                    i += 1;
                }

                let end_byte = if end_char < char_lens.len() {
                    char_to_byte[end_char] + char_lens[end_char]
                } else {
                    preview.len()
                };
                ranges.push((start_byte as u32, (end_byte - start_byte) as u32));
                i += 1;
            }

            entry.match_ranges = ranges;
            (entry, composite)
        })
        .collect()
}

fn combined_haystack(entry: &EntrySummary) -> String {
    match (&entry.preview_text, &entry.source_app) {
        (Some(preview), Some(source_app)) => format!("{preview} {source_app}"),
        (Some(preview), None) => preview.clone(),
        (None, Some(source_app)) => source_app.clone(),
        (None, None) => String::new(),
    }
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
        assert!(
            expr.is_empty(),
            "all tokens stripped, expect empty: {:?}",
            expr
        );
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
