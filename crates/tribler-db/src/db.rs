//! Connexion a la base SQLite : ouverture, migrations, acces mutex.
//!
//! `rusqlite::Connection` n'est ni `Send`-partageable ni `Sync` ; le
//! `Database` encapsule la connexion derriere un `Mutex` pour etre
//! partageable entre les services async (`tribler-core`). Les appels
//! restent synchrones — les operations lourdes devront passer par
//! `tokio::task::spawn_blocking` cote appelant.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::{migrations, Result};

/// Handle de base partageable.
pub struct Database {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

impl Database {
    /// Ouvre (ou cree) la base a `path` et applique les migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        configure(&conn)?;
        migrations::migrate(&mut conn)?;
        tracing::info!(path = %path.display(), "base sqlite ouverte");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Base en memoire (tests).
    pub fn memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Execute `f` avec la connexion verrouillee.
    pub fn with<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| crate::DbError::Corrupt("mutex de connexion sqlite empoisonne".into()))?;
        f(&conn)
    }

    /// Version de schema courante (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<i64> {
        self.with(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
    }
}

/// Reglages de connexion communs (journal WAL, foreign keys).
fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    // WAL n'a de sens que pour une base fichier ; en memoire c'est un
    // no-op controle par rusqlite.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    Ok(())
}
