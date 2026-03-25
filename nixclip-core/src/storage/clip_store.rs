use std::collections::HashSet;
use std::path::Path;

use rusqlite::params;

use crate::config::GeneralConfig;
use crate::error::Result;
use crate::{
    ContentClass, EntrySummary, EntryId, NewEntry, PruneStats, Query, QueryResult,
    Representation, StoreStats,
};

use super::blob::BlobStore;
use super::migrations;

/// Inline threshold: representations smaller than this are stored directly in
/// the SQLite `representations.data` column; larger payloads go to the blob
/// store and the relative path is recorded in `blob_path`.
const BLOB_THRESHOLD: usize = 64 * 1024;

/// Main storage façade.  All operations are synchronous; callers are expected
/// to run them inside `tokio::task::spawn_blocking`.
pub struct ClipStore {
    conn: rusqlite::Connection,
    blob_store: BlobStore,
    db_path: std::path::PathBuf,
}

// Safety: `rusqlite::Connection` is `!Send` because SQLite uses thread-local
// error state.  We guarantee single-threaded access by wrapping `ClipStore`
// in `std::sync::Mutex` in the daemon's `AppState`.  All callers acquire the
// mutex before touching the connection, so no two threads use it concurrently.
unsafe impl Send for ClipStore {}
unsafe impl Sync for ClipStore {}

impl ClipStore {
    // -------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------

    /// Open (or create) the database at `db_path` and the blob directory at
    /// `blob_dir`.  Runs any pending schema migrations.
    pub fn open(db_path: &Path, blob_dir: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = rusqlite::Connection::open(db_path)?;

        // Enable pragmas before any migration work.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;

        migrations::run_migrations(&conn)?;

        let blob_store = BlobStore::new(blob_dir)?;

        Ok(Self {
            conn,
            blob_store,
            db_path: db_path.to_path_buf(),
        })
    }

    // -------------------------------------------------------------------
    // Insert
    // -------------------------------------------------------------------

    /// Insert a new clipboard entry.
    ///
    /// **Last-entry deduplication**: if the most recent entry has the same
    /// `canonical_hash`, only its `last_seen_at` timestamp is bumped and `None`
    /// is returned.  Otherwise the entry is fully inserted and `Some(id)` is
    /// returned.
    pub fn insert(&self, entry: NewEntry) -> Result<Option<EntryId>> {
        let now = chrono::Utc::now().timestamp_millis();

        // -- Dedup check against the most recent entry --
        let last_hash: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT canonical_hash FROM entries ORDER BY last_seen_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();

        if let Some(ref h) = last_hash {
            if h.as_slice() == entry.canonical_hash.as_slice() {
                self.conn.execute(
                    "UPDATE entries SET last_seen_at = ?1
                     WHERE id = (SELECT id FROM entries ORDER BY last_seen_at DESC LIMIT 1)",
                    params![now],
                )?;
                return Ok(None);
            }
        }

        // -- Full insert inside a transaction --
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO entries
                 (created_at, last_seen_at, pinned, ephemeral, content_class, preview_text, source_app, canonical_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now,
                now,
                false,
                entry.ephemeral,
                entry.content_class.as_str(),
                entry.preview_text,
                entry.source_app,
                entry.canonical_hash.as_slice(),
            ],
        )?;

        let entry_id: EntryId = tx.last_insert_rowid();

        // Insert each representation.
        for rep in &entry.representations {
            if rep.data.len() < BLOB_THRESHOLD {
                // Inline storage.
                tx.execute(
                    "INSERT INTO representations (entry_id, mime, data, blob_path)
                     VALUES (?1, ?2, ?3, NULL)",
                    params![entry_id, rep.mime, rep.data],
                )?;
            } else {
                // Blob storage.
                let blob_hash = blake3::hash(&rep.data);
                let rel_path =
                    self.blob_store.store(blob_hash.as_bytes(), &rep.data)?;
                tx.execute(
                    "INSERT INTO representations (entry_id, mime, data, blob_path)
                     VALUES (?1, ?2, NULL, ?3)",
                    params![entry_id, rep.mime, rel_path],
                )?;
            }
        }

        // Sync the FTS index.
        tx.execute(
            "INSERT INTO search_idx (rowid, preview_text) VALUES (?1, ?2)",
            params![entry_id, entry.preview_text],
        )?;

        tx.commit()?;

        Ok(Some(entry_id))
    }

    // -------------------------------------------------------------------
    // Query
    // -------------------------------------------------------------------

    /// List entries matching the given query.
    pub fn query(&self, q: Query) -> Result<QueryResult> {
        // Build WHERE clause fragments and corresponding bind values.
        let mut conditions: Vec<String> = Vec::new();
        let mut filter_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref text) = q.text {
            if !text.is_empty() {
                conditions.push(
                    "e.id IN (SELECT rowid FROM search_idx WHERE search_idx MATCH ?)"
                        .to_string(),
                );
                // Quote the user text to prevent FTS5 syntax errors.
                filter_values.push(Box::new(format!("\"{}\"", text.replace('"', "\"\""))));
            }
        }

        if let Some(cc) = q.content_class {
            conditions.push("e.content_class = ?".to_string());
            filter_values.push(Box::new(cc.as_str().to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Helper to build a Vec<&dyn ToSql> from the filter values plus optional extras.
        let filter_refs = || -> Vec<&dyn rusqlite::types::ToSql> {
            filter_values.iter().map(|b| b.as_ref()).collect()
        };

        // Total count.
        let count_sql = format!("SELECT COUNT(*) FROM entries e {where_clause}");
        let total: u32 = {
            let mut stmt = self.conn.prepare(&count_sql)?;
            let refs = filter_refs();
            stmt.query_row(refs.as_slice(), |row| row.get(0))?
        };

        // Fetch the requested page.
        let select_sql = format!(
            "SELECT e.id, e.created_at, e.last_seen_at, e.pinned, e.ephemeral,
                    e.content_class, e.preview_text, e.source_app
             FROM entries e
             {where_clause}
             ORDER BY e.pinned DESC, e.last_seen_at DESC
             LIMIT ? OFFSET ?"
        );

        let mut stmt = self.conn.prepare(&select_sql)?;

        // Build the full parameter list: filter params then limit and offset.
        let limit_box: Box<dyn rusqlite::types::ToSql> = Box::new(q.limit);
        let offset_box: Box<dyn rusqlite::types::ToSql> = Box::new(q.offset);
        let mut all_refs: Vec<&dyn rusqlite::types::ToSql> = filter_refs();
        all_refs.push(limit_box.as_ref());
        all_refs.push(offset_box.as_ref());

        let rows = stmt.query_map(all_refs.as_slice(), |row| row_to_summary_row(row))?;

        let mut entries = Vec::new();
        for row_result in rows {
            let mut summary = row_result?;
            summary.thumbnail = self.load_thumbnail(summary.id);
            entries.push(summary);
        }

        Ok(QueryResult { entries, total })
    }

    // -------------------------------------------------------------------
    // Single entry
    // -------------------------------------------------------------------

    /// Get a single entry by id.
    pub fn get_entry(&self, id: EntryId) -> Result<EntrySummary> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, last_seen_at, pinned, ephemeral,
                    content_class, preview_text, source_app
             FROM entries WHERE id = ?1",
        )?;

        let mut summary: EntrySummary = stmt.query_row(params![id], |row| row_to_summary_row(row))?;
        summary.thumbnail = self.load_thumbnail(id);
        Ok(summary)
    }

    // -------------------------------------------------------------------
    // Representations
    // -------------------------------------------------------------------

    /// Get all representations for an entry, resolving blob paths to data.
    pub fn get_representations(&self, id: EntryId) -> Result<Vec<Representation>> {
        let mut stmt = self.conn.prepare(
            "SELECT mime, data, blob_path FROM representations WHERE entry_id = ?1",
        )?;

        let rows = stmt.query_map(params![id], |row| {
            let mime: String = row.get(0)?;
            let inline_data: Option<Vec<u8>> = row.get(1)?;
            let blob_path: Option<String> = row.get(2)?;
            Ok((mime, inline_data, blob_path))
        })?;

        let mut reps = Vec::new();
        for row_result in rows {
            let (mime, inline_data, blob_path) = row_result?;
            let data = if let Some(d) = inline_data {
                d
            } else if let Some(ref p) = blob_path {
                self.blob_store.load(p)?
            } else {
                Vec::new()
            };
            reps.push(Representation { mime, data });
        }

        Ok(reps)
    }

    // -------------------------------------------------------------------
    // Delete
    // -------------------------------------------------------------------

    /// Delete the given entries and their blobs.
    pub fn delete(&self, ids: &[EntryId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let tx = self.conn.unchecked_transaction()?;

        for &id in ids {
            // Collect blob paths before deleting.
            let blob_paths = self.blob_paths_for_entry_tx(&tx, id)?;

            // Fetch preview_text before deleting the row so we can remove
            // the FTS entry with the correct original value.
            let preview_text: Option<String> = tx
                .query_row(
                    "SELECT preview_text FROM entries WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .ok()
                .flatten();

            // Delete FTS entry (must supply the original column values).
            tx.execute(
                "INSERT INTO search_idx (search_idx, rowid, preview_text)
                 VALUES ('delete', ?1, ?2)",
                params![id, preview_text],
            )?;

            // Delete the entry (cascades to representations).
            tx.execute("DELETE FROM entries WHERE id = ?1", params![id])?;

            // Delete blob files.
            for path in blob_paths {
                let _ = self.blob_store.delete(&path);
            }
        }

        tx.commit()?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Pin / unpin
    // -------------------------------------------------------------------

    /// Set or clear the pinned flag on an entry.
    pub fn pin(&self, id: EntryId, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Clear unpinned
    // -------------------------------------------------------------------

    /// Delete all unpinned entries and their blobs, then rebuild the FTS index.
    pub fn clear_unpinned(&self) -> Result<()> {
        let blob_paths = self.collect_blob_paths(
            "SELECT r.blob_path FROM representations r
             JOIN entries e ON r.entry_id = e.id
             WHERE e.pinned = 0 AND r.blob_path IS NOT NULL",
        )?;

        self.conn.execute("DELETE FROM entries WHERE pinned = 0", [])?;

        for path in &blob_paths {
            let _ = self.blob_store.delete(path);
        }

        self.rebuild_fts()?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Prune
    // -------------------------------------------------------------------

    /// Enforce retention and max-entries limits.  Returns statistics about what
    /// was removed.
    pub fn prune(&self, config: &GeneralConfig) -> Result<PruneStats> {
        let mut entries_deleted: u32 = 0;
        let mut blobs_deleted: u32 = 0;
        let mut bytes_freed: u64 = 0;

        // 1. Delete entries past the retention window.
        if let Some(duration) = config.retention.to_duration() {
            let cutoff = chrono::Utc::now().timestamp_millis()
                - duration.num_milliseconds();

            let expired_ids = self.collect_ids(
                "SELECT id FROM entries WHERE last_seen_at < ?1 AND pinned = 0",
                params![cutoff],
            )?;

            if !expired_ids.is_empty() {
                let blob_paths = self.blob_paths_for_ids_tx(&self.conn, &expired_ids)?;
                let count = expired_ids.len() as u32;

                let placeholders = placeholders(expired_ids.len());
                let sql = format!(
                    "DELETE FROM entries WHERE id IN ({placeholders})"
                );
                let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                    expired_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
                self.conn.execute(&sql, param_refs.as_slice())?;

                for path in &blob_paths {
                    if let Ok(meta) = std::fs::metadata(self.blob_store_path(path)) {
                        bytes_freed += meta.len();
                    }
                    let _ = self.blob_store.delete(path);
                    blobs_deleted += 1;
                }

                entries_deleted += count;
            }
        }

        // 2. Enforce max_entries by deleting the oldest non-pinned entries.
        let total_count: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM entries",
            [],
            |row| row.get(0),
        )?;

        if total_count > config.max_entries {
            let excess = total_count - config.max_entries;
            let overflow_ids = self.collect_ids(
                "SELECT id FROM entries WHERE pinned = 0 ORDER BY last_seen_at ASC LIMIT ?1",
                params![excess],
            )?;

            if !overflow_ids.is_empty() {
                let blob_paths = self.blob_paths_for_ids_tx(&self.conn, &overflow_ids)?;
                let count = overflow_ids.len() as u32;

                let placeholders = placeholders(overflow_ids.len());
                let sql = format!(
                    "DELETE FROM entries WHERE id IN ({placeholders})"
                );
                let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                    overflow_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
                self.conn.execute(&sql, param_refs.as_slice())?;

                for path in &blob_paths {
                    if let Ok(meta) = std::fs::metadata(self.blob_store_path(path)) {
                        bytes_freed += meta.len();
                    }
                    let _ = self.blob_store.delete(path);
                    blobs_deleted += 1;
                }

                entries_deleted += count;
            }
        }

        // 3. Garbage-collect orphan blobs.
        let valid = self.all_blob_paths()?;
        let orphan_bytes = self.blob_store.cleanup_orphans(&valid)?;
        bytes_freed += orphan_bytes;

        // Rebuild FTS after bulk deletions.
        if entries_deleted > 0 {
            self.rebuild_fts()?;
        }

        Ok(PruneStats {
            entries_deleted,
            blobs_deleted,
            bytes_freed,
        })
    }

    /// Delete ephemeral entries older than `ttl_hours`.
    ///
    /// Ephemeral entries are clipboard captures that matched a sensitive-content
    /// pattern.  They are kept for a limited time so the user can still paste
    /// them during the current session, but are cleaned up afterwards.
    ///
    /// Returns a [`PruneStats`] describing what was removed.
    pub fn prune_ephemeral(&self, ttl_hours: u32) -> Result<PruneStats> {
        let mut entries_deleted: u32 = 0;
        let mut blobs_deleted: u32 = 0;
        let mut bytes_freed: u64 = 0;

        let cutoff = chrono::Utc::now().timestamp_millis()
            - (ttl_hours as i64) * 3_600_000;

        let expired_ids = self.collect_ids(
            "SELECT id FROM entries WHERE ephemeral = 1 AND created_at < ?1",
            params![cutoff],
        )?;

        if !expired_ids.is_empty() {
            let blob_paths = self.blob_paths_for_ids_tx(&self.conn, &expired_ids)?;
            let count = expired_ids.len() as u32;

            let ph = placeholders(expired_ids.len());
            let sql = format!("DELETE FROM entries WHERE id IN ({ph})");
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                expired_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
            self.conn.execute(&sql, param_refs.as_slice())?;

            for path in &blob_paths {
                if let Ok(meta) = std::fs::metadata(self.blob_store_path(path)) {
                    bytes_freed += meta.len();
                }
                let _ = self.blob_store.delete(path);
                blobs_deleted += 1;
            }

            entries_deleted += count;

            // Rebuild FTS after deletions.
            if entries_deleted > 0 {
                self.rebuild_fts()?;
            }
        }

        Ok(PruneStats {
            entries_deleted,
            blobs_deleted,
            bytes_freed,
        })
    }

    // -------------------------------------------------------------------
    // Stats
    // -------------------------------------------------------------------

    /// Gather high-level storage statistics.
    pub fn stats(&self) -> Result<StoreStats> {
        let entry_count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM entries",
            [],
            |row| row.get(0),
        )?;

        let blob_size_bytes = self.blob_store.total_size()?;

        let db_size_bytes = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(StoreStats {
            entry_count,
            blob_size_bytes,
            db_size_bytes,
        })
    }

    // -------------------------------------------------------------------
    // Integrity
    // -------------------------------------------------------------------

    /// Run SQLite's built-in integrity check and verify that every blob
    /// reference in the database points to an existing file.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut issues = Vec::new();

        // SQLite integrity check.
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |row| {
            let msg: String = row.get(0)?;
            Ok(msg)
        })?;

        for row in rows {
            let msg = row?;
            if msg != "ok" {
                issues.push(format!("sqlite: {msg}"));
            }
        }

        // Blob-reference check.
        let mut stmt = self.conn.prepare(
            "SELECT entry_id, mime, blob_path FROM representations WHERE blob_path IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            let entry_id: EntryId = row.get(0)?;
            let mime: String = row.get(1)?;
            let path: String = row.get(2)?;
            Ok((entry_id, mime, path))
        })?;

        for row in rows {
            let (entry_id, mime, path) = row?;
            if !self.blob_store.exists(&path) {
                issues.push(format!(
                    "missing blob: entry_id={entry_id} mime={mime} path={path}"
                ));
            }
        }

        Ok(issues)
    }

    // -------------------------------------------------------------------
    // FTS rebuild
    // -------------------------------------------------------------------

    /// Rebuild the full-text search index from scratch.
    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute("DELETE FROM search_idx", [])?;
        self.conn.execute(
            "INSERT INTO search_idx (rowid, preview_text)
             SELECT id, preview_text FROM entries",
            [],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------

    /// Load the thumbnail representation for an entry, if one exists.
    fn load_thumbnail(&self, id: EntryId) -> Option<Vec<u8>> {
        let result: std::result::Result<(Option<Vec<u8>>, Option<String>), _> = self.conn.query_row(
            "SELECT data, blob_path FROM representations
             WHERE entry_id = ?1 AND mime = 'image/thumbnail'",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((Some(data), _)) => Some(data),
            Ok((None, Some(ref path))) => self.blob_store.load(path).ok(),
            _ => None,
        }
    }

    /// Collect all blob_path values for a single entry inside a transaction.
    fn blob_paths_for_entry_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        id: EntryId,
    ) -> Result<Vec<String>> {
        let mut stmt = tx.prepare(
            "SELECT blob_path FROM representations WHERE entry_id = ?1 AND blob_path IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![id], |row| row.get(0))?;
        let mut paths = Vec::new();
        for r in rows {
            paths.push(r?);
        }
        Ok(paths)
    }

    /// Collect blob_path values for a set of entry ids.
    fn blob_paths_for_ids_tx(
        &self,
        conn: &rusqlite::Connection,
        ids: &[EntryId],
    ) -> Result<Vec<String>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ph = placeholders(ids.len());
        let sql = format!(
            "SELECT blob_path FROM representations WHERE entry_id IN ({ph}) AND blob_path IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| row.get(0))?;
        let mut paths = Vec::new();
        for r in rows {
            paths.push(r?);
        }
        Ok(paths)
    }

    /// Collect a simple list of blob paths matching a SQL query.
    fn collect_blob_paths(&self, sql: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut paths = Vec::new();
        for r in rows {
            paths.push(r?);
        }
        Ok(paths)
    }

    /// Collect entry IDs from a parameterized query.
    fn collect_ids(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::types::ToSql],
    ) -> Result<Vec<EntryId>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| row.get(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        Ok(ids)
    }

    /// Build the set of all blob paths currently referenced by the database.
    fn all_blob_paths(&self) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT blob_path FROM representations WHERE blob_path IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut set = HashSet::new();
        for r in rows {
            set.insert(r?);
        }
        Ok(set)
    }

    /// Resolve a blob relative path to its absolute path for metadata queries.
    fn blob_store_path(&self, rel: &str) -> std::path::PathBuf {
        self.blob_store_base().join(rel)
    }

    /// Return the blob store base directory.
    fn blob_store_base(&self) -> &Path {
        // The blob_store's base_dir is not exposed publicly, but we can
        // reconstruct it from the db_path sibling.  Instead, just use the
        // blob_store.exists() / delete() methods which already know the path.
        // For metadata, we'll access the field directly since it is crate-local.
        &self.blob_store.base_dir
    }
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

/// Map a rusqlite row (from a SELECT on entries) to an `EntrySummary`.
///
/// Expected column order:
///   0: id, 1: created_at, 2: last_seen_at, 3: pinned, 4: ephemeral,
///   5: content_class, 6: preview_text, 7: source_app
fn row_to_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntrySummary> {
    let class_str: String = row.get(5)?;
    let content_class: ContentClass = class_str
        .parse()
        .unwrap_or(ContentClass::Text);

    Ok(EntrySummary {
        id: row.get(0)?,
        created_at: row.get(1)?,
        last_seen_at: row.get(2)?,
        pinned: row.get(3)?,
        ephemeral: row.get(4)?,
        content_class,
        preview_text: row.get(6)?,
        source_app: row.get(7)?,
        thumbnail: None, // Loaded separately.
    })
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Generate a comma-separated list of `?` placeholders for use in `IN (...)`.
fn placeholders(count: usize) -> String {
    let mut s = String::new();
    for i in 0..count {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('?');
    }
    s
}
