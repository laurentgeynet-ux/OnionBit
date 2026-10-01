//! Acces a la table `misc` (cle/valeur) : `db_version`, reglages
//! persistants du daemon.

use rusqlite::{params, Connection, OptionalExtension};

use crate::Result;

/// Lit une valeur `misc` (`None` si absente).
pub fn get(conn: &Connection, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM misc WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?)
}

/// Ecrit une valeur `misc` (insert ou remplace).
pub fn set(conn: &Connection, name: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO misc(name, value) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
        params![name, value],
    )?;
    Ok(())
}
