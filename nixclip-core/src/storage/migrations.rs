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
        tracing::info!("applying migration v1: initial schema");
        schema::init_schema(conn)?;

        if current.is_none() {
            conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [1u32])?;
        } else {
            conn.execute(
                "UPDATE schema_version SET version = ?1",
                [1u32],
            )?;
        }
    }

    Ok(())
}
