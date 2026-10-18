// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

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

use crate::{migrations, DbError, DbKey, Result};

/// Duree au-dela de laquelle une operation SQLite est loggee en
/// `warn!` — diagnostic perf. 1 s : en dessous, la contention usuelle
/// du mutex de connexion (rafales d'acces simultanes) n'a rien
/// d'anormal et ne doit pas produire de warn.
const SLOW_QUERY_WARN: std::time::Duration = std::time::Duration::from_secs(1);

/// Cadence max des warns « operation sqlite lente » : une contention
/// prolongee produit une rafale d'operations lentes au meme instant —
/// les warns supprimes sont comptes et rapportes au warn suivant
/// (`suppressed`) plutot que d'inonder le journal.
const SLOW_WARN_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Handle de base partageable.
pub struct Database {
    conn: Mutex<Connection>,
    /// Instant du dernier warn « operation lente » emis (`None` =
    /// aucun) — borne la cadence a [`SLOW_WARN_MIN_INTERVAL`].
    last_slow_warn: Mutex<Option<std::time::Instant>>,
    /// Warns « lente » supprimes depuis le dernier emis — rappeles au
    /// warn suivant pour ne pas perdre le volume de la rafale.
    suppressed_slow: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

impl Database {
    /// Ouvre (ou cree) la base a `path` et applique les migrations.
    ///
    /// `key` (ADR-0031) : `Some` = codec sqlite3mc `chacha20` —
    /// `PRAGMA cipher` puis `PRAGMA key` sont poses **avant tout
    /// acces page** (`configure()` pose `journal_mode` qui lit deja
    /// des pages). La detection a froid distingue trois cas :
    ///
    /// - fichier absent/vide → creation chiffree directe ;
    /// - magic `SQLite format 3\0` present → base legacy en clair :
    ///   ouverture sans clef (migration differee, etape 138) ;
    /// - sinon → base chiffree : clef exigee, une premiere lecture
    ///   qui echoue est rapportee [`DbError::Encrypted`] — mauvaise
    ///   clef ou corruption de la premiere page, jamais `Corrupt`.
    pub fn open(path: &Path, key: Option<&DbKey>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let plaintext = sniff_plaintext(path)?;
        let mut conn = Connection::open(path)?;
        if let Some(k) = key.filter(|_| plaintext != Some(true)) {
            // Ordre ADR-0031 §2 : `cipher` n'agit qu'a la creation/
            // rekey ; le poser aussi a la reouverture est inoffensif
            // et couvre les bases d'un ancien default.
            conn.execute_batch("PRAGMA cipher='chacha20'")?;
            let pragma = k.pragma();
            conn.execute_batch(pragma.as_str())?;
        }
        // Premiere lecture reelle : sur base chiffree un echec ici
        // = clef absente/invalide → erreur typee, pas `Corrupt`.
        if plaintext == Some(false) {
            probe_schema(&conn)?;
        }
        if plaintext == Some(true) && key.is_some() {
            tracing::warn!(path = %path.display(),
                "base legacy en clair — migration chiffree differee (ADR-0031 etape 138)");
        }
        configure(&conn)?;
        migrations::migrate(&mut conn)?;
        optimize(&conn);
        tracing::info!(path = %path.display(), "base sqlite ouverte");
        Ok(Self {
            conn: Mutex::new(conn),
            last_slow_warn: Mutex::new(None),
            suppressed_slow: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Base en memoire (tests).
    pub fn memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            last_slow_warn: Mutex::new(None),
            suppressed_slow: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Execute `f` avec la connexion verrouillee.
    ///
    /// Log un `warn!` si l'operation (attente du mutex incluse) depasse
    /// [`SLOW_QUERY_WARN`], cadence bornee a [`SLOW_WARN_MIN_INTERVAL`]
    /// — diagnostic des acces disque lents sans rafale de logs.
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
            self.warn_slow(op, caller, elapsed);
        }
        out
    }

    /// Warn « operation sqlite lente » emis au plus une fois par
    /// [`SLOW_WARN_MIN_INTERVAL`] ; les occurrences intermediaires sont
    /// comptees et rapportees au warn suivant (`suppressed`).
    fn warn_slow(
        &self,
        op: &'static str,
        caller: &'static std::panic::Location<'static>,
        elapsed: std::time::Duration,
    ) {
        // Mutex de diagnostic recupere meme empoisonne : une panique
        // sous verrou ne doit pas masquer la metrique.
        let mut last = self
            .last_slow_warn
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < SLOW_WARN_MIN_INTERVAL) {
            self.suppressed_slow
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let suppressed = self
            .suppressed_slow
            .swap(0, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!(
            elapsed_ms = elapsed.as_millis() as u64,
            op,
            caller = %caller,
            suppressed,
            "operation sqlite lente"
        );
        *last = Some(now);
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

    /// Checkpoint WAL final (`TRUNCATE`) : rejoue le journal dans le
    /// fichier principal puis le vide. A appeler a l'arret de la
    /// session — evite de trainer un WAL de plusieurs Mio entre deux
    /// sessions et accelere le prochain open.
    pub fn checkpoint(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
            Ok(())
        })
    }
}

/// Reglages de connexion communs (journal WAL, foreign keys,
/// pragmas de performance).
/// Detection du format a froid (ADR-0031 §3) : `Some(true)` = magic
/// `SQLite format 3\0` present → base legacy en clair ; `Some(false)`
/// = magic absent → base chiffree sqlite3mc (la page 1 entiere est
/// chiffree, en-tete compris) ; `None` = fichier absent ou vide →
/// creation. La lecture est volontairement hors `Connection` : poser
/// `key` sur une base claire la rendrait illisible.
fn sniff_plaintext(path: &Path) -> Result<Option<bool>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut head = [0u8; 16];
    let n = {
        use std::io::Read;
        f.read(&mut head)?
    };
    Ok(match n {
        // Fichier vide = creation ; <16 octets = ni clair ni
        // chiffre valide → traite comme chiffre (probe_decide).
        0 => None,
        _ => Some(n == 16 && &head == b"SQLite format 3\0"),
    })
}

/// Premiere lecture reelle du schema — sur base chiffree, echec =
/// clef absente ou invalide (la page 1 ne s'authentifie pas) →
/// [`DbError::Encrypted`], distinct d'une corruption en clair.
fn probe_schema(conn: &Connection) -> Result<()> {
    conn.query_one("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|_| DbError::Encrypted)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    // WAL n'a de sens que pour une base fichier ; en memoire c'est un
    // no-op controle par rusqlite. Permet aux lecteurs de ne pas
    // attendre l'ecrivain.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    // Durabilite relachee mais sure en WAL : un checkpoint reste
    // atomique, on evite le fsync a chaque transaction.
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // Checkpoint auto du WAL : le defaut (1000 pages ≈ 4 Mio)
    // declenche le checkpoint en pleine rafale d'ecriture — on le
    // decale a ~32 Mio ; le checkpoint TRUNCATE a l'arret borne le
    // journal entre deux sessions.
    let _ = conn.pragma_update(None, "wal_autocheckpoint", 8_000i64);
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
