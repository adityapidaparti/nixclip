use crate::Result;

pub const SCHEMA_VERSION: u32 = 1;

/// Set pragmas and create all tables, indexes, and virtual tables if they do not
/// already exist.
pub fn init_schema(conn: &rusqlite::Connection) -> Result<()> {
    // Pragmas
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;

    // Main entries table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             created_at    INTEGER NOT NULL,
             last_seen_at  INTEGER NOT NULL,
             pinned        INTEGER NOT NULL DEFAULT 0,
             ephemeral     INTEGER NOT NULL DEFAULT 0,
             content_class TEXT    NOT NULL,
             preview_text  TEXT,
             source_app    TEXT,
             canonical_hash BLOB  NOT NULL,
             image_width   INTEGER,
             image_height  INTEGER,
             file_count    INTEGER,
             url_domain    TEXT
         );",
    )?;

    // Indexes on entries
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_entries_hash
             ON entries(canonical_hash);
         CREATE INDEX IF NOT EXISTS idx_entries_last_seen
             ON entries(last_seen_at DESC);
         CREATE INDEX IF NOT EXISTS idx_entries_class
             ON entries(content_class);",
    )?;

    // Representations table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS representations (
             entry_id  INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
             mime      TEXT    NOT NULL,
             data      BLOB,
             blob_path TEXT,
             PRIMARY KEY (entry_id, mime)
         );",
    )?;

    // FTS5 full-text search index
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS search_idx
             USING fts5(preview_text, source_app, content=entries, content_rowid=id);",
    )?;

    // Schema version tracking table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
             version INTEGER NOT NULL
         );",
    )?;

    Ok(())
}
