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

    if version < 1 {
        tracing::info!(
            target_version = schema::SCHEMA_VERSION,
            "applying migration v1: initial schema"
        );
        // init_schema creates the entries table with all current columns,
        // including the metadata fields added after the original schema.
        schema::init_schema(conn)?;

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
