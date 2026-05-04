use crate::Result;

use super::schema;

/// Run any pending schema migrations.  The `schema_version` table is created if
/// it does not exist yet, and the current version is read.  Each migration is
/// applied in order until the schema is up-to-date.
pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
    // Ensure the version-tracking table exists.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
             version INTEGER NOT NULL
         );",
    )?;

    let current: Option<u32> = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();

    let version = current.unwrap_or(0);
    let entries_exists = table_exists(conn, "entries")?;

    if version < 1 {
        tracing::info!(
            target_version = schema::SCHEMA_VERSION,
            "applying migration v1: initial schema"
        );
        schema::init_schema(conn)?;
        if entries_exists {
            ensure_entries_metadata_columns(conn)?;
        }

        if current.is_none() {
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [schema::SCHEMA_VERSION],
            )?;
        } else {
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                [schema::SCHEMA_VERSION],
            )?;
        }
    } else if version < 2 {
        // Existing database created with v1 schema — add the metadata columns.
        tracing::info!("applying migration v2: add metadata columns to entries");
        conn.execute_batch(
            "ALTER TABLE entries ADD COLUMN image_width  INTEGER;
             ALTER TABLE entries ADD COLUMN image_height INTEGER;
             ALTER TABLE entries ADD COLUMN file_count   INTEGER;
             ALTER TABLE entries ADD COLUMN url_domain   TEXT;",
        )?;
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            [schema::SCHEMA_VERSION],
        )?;
    }

    Ok(())
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn column_exists(conn: &rusqlite::Connection, table: &str, column: &str) -> Result<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn ensure_entries_metadata_columns(conn: &rusqlite::Connection) -> Result<()> {
    let missing = [
        (
            "image_width",
            "ALTER TABLE entries ADD COLUMN image_width INTEGER",
        ),
        (
            "image_height",
            "ALTER TABLE entries ADD COLUMN image_height INTEGER",
        ),
        (
            "file_count",
            "ALTER TABLE entries ADD COLUMN file_count INTEGER",
        ),
        (
            "url_domain",
            "ALTER TABLE entries ADD COLUMN url_domain TEXT",
        ),
    ];

    for (column, sql) in missing {
        if !column_exists(conn, "entries", column)? {
            conn.execute_batch(sql)?;
        }
    }

    Ok(())
}
