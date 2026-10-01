//! Connexion a la base SQLite : ouverture, migrations, acces mutex.
//!
//! `rusqlite::Connection` n'est ni `Send`-partageable ni `Sync` ; le
//! `Database` encapsule la connexion derriere un `Mutex` pour etre
//! partageable entre les services async (`onionbit-core`). Les appels
//! restent synchrones — les operations lourdes devront passer par
//! `tokio::task::spawn_blocking` cote appelant.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::{migrations, Result};

/// Duree au-dela de laquelle une operation SQLite est loggee en
/// `warn!` — diagnostic perf : les requetes lentes et la contention
/// du mutex de connexion apparaissent ainsi dans les logs.
const SLOW_QUERY_WARN: std::time::Duration = std::time::Duration::from_millis(250);

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
        optimize(&conn);
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
    ///
    /// Log un `warn!` si l'operation (attente du mutex incluse) depasse
    /// [`SLOW_QUERY_WARN`] — diagnostic des acces disque lents.
    /// `caller` identifie le site appelant (`#[track_caller]`) pour
    /// savoir quelle operation sature quand des appels s'empilent.
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        self.with_at(f, "", std::panic::Location::caller())
    }

    /// [`Database::with`] avec identifiant explicite — utilise par
    /// [`Database::call`] pour remonter le vrai appelant au lieu de
    /// `db.rs` (`#[track_caller]` est un no-op sur les `async fn`).
    fn with_at<R>(
        &self,
        f: impl FnOnce(&Connection) -> Result<R>,
        op: &'static str,
        caller: &'static std::panic::Location<'static>,
    ) -> Result<R> {
        let t0 = std::time::Instant::now();
        let conn = self
            .conn
            .lock()
            .map_err(|_| crate::DbError::Corrupt("mutex de connexion sqlite empoisonne".into()))?;
        let out = f(&conn);
        let elapsed = t0.elapsed();
        if elapsed >= SLOW_QUERY_WARN {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis() as u64,
                op,
                caller = %caller,
                "operation sqlite lente"
            );
        }
        out
    }

    /// Version async de [`Database::with`] : le travail SQLite est
    /// deporte sur le pool de threads bloquants de Tokio
    /// (`spawn_blocking`) pour ne pas figer l'executor pendant les
    /// requetes lourdes (recherche FTS, listes longues). A preferer
    /// dans les handlers axum ; `with` reste acceptable pour les
    /// ecritures courtes hors chemin de requete.
    /// `op` nomme l'operation (ex. `"metadata.trackers"`) : le warn
    /// « operation sqlite lente » l'affiche pour identifier les
    /// requetes qui saturent quand des appels s'empilent.
    pub async fn call<R>(
        self: &std::sync::Arc<Self>,
        op: &'static str,
        f: impl FnOnce(&Connection) -> Result<R> + Send + 'static,
    ) -> Result<R>
    where
        R: Send + 'static,
    {
        const CALL_SITE: &std::panic::Location<'static> = std::panic::Location::caller();
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.with_at(f, op, CALL_SITE))
            .await
            .map_err(|e| crate::DbError::Corrupt(format!("tache sqlite interrompue: {e}")))?
    }

    /// Version de schema courante (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<i64> {
        self.with(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
    }
}

/// Reglages de connexion communs (journal WAL, foreign keys,
/// pragmas de performance).
fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    // WAL n'a de sens que pour une base fichier ; en memoire c'est un
    // no-op controle par rusqlite. Permet aux lecteurs de ne pas
    // attendre l'ecrivain.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    // Durabilite relachee mais sure en WAL : un checkpoint reste
    // atomique, on evite le fsync a chaque transaction.
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // La connexion est serialisee par le `Mutex`, mais WAL autorise
    // des lecteurs externes (fichier) : ne jamais rendre SQLITE_BUSY
    // immediatement.
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    // Tables temporaires/tri en memoire (ORDER BY des recherches,
    // CTE de l'augmenteur).
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    // Cache de pages (~20 Mio) : les recherches FTS + jointures
    // `channel_node`/`torrent_state` restent en memoire.
    let _ = conn.pragma_update(None, "cache_size", -20_000i64);
    // Lecture mmap du fichier DB (64 Mio) : scan de `channel_node`
    // sans copie dans le cache de pages. Ignore en memoire.
    let _ = conn.pragma_update(None, "mmap_size", 64_i64 * 1024 * 1024);
    // Prepares statements reutilises (listes/recherches appelees en
    // boucle par les endpoints REST).
    conn.set_prepared_statement_cache_capacity(64);
    // `search_rank` Python : fonction de ranking appelee depuis le
    // SQL (`get_entries_query`, tri de pertinence des `txt_filter`).
    crate::ranks::register_search_rank(conn)?;
    Ok(())
}

/// `PRAGMA optimize` post-migrations : met a jour les statistiques
/// du planner si les tables ont significativement change (cout
/// quasi nul, evite des plans de requete degrades).
fn optimize(conn: &Connection) {
    if let Err(e) = conn.execute_batch("PRAGMA optimize") {
        tracing::warn!(error = %e, "pragma optimize ignore");
    }
}
