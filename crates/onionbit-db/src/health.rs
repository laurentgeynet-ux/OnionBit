//! Acces a `torrent_state` / `tracker_state` (sante des essaims,
//! trackers connus). Equivalents des acces Pony de
//! `torrent_checker`/`health.py`.

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{TorrentStateRow, TrackerStateRow};
use crate::Result;

fn torrent_state_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TorrentStateRow> {
    Ok(TorrentStateRow {
        rowid: r.get("rowid")?,
        infohash: r.get("infohash")?,
        seeders: r.get("seeders")?,
        leechers: r.get("leechers")?,
        last_check: r.get("last_check")?,
        self_checked: r.get::<_, i64>("self_checked")? != 0,
        has_data: r.get::<_, i64>("has_data")? != 0,
        tracker_id: r.get("tracker_id")?,
    })
}

const TS_COLS: &str =
    "rowid, infohash, seeders, leechers, last_check, self_checked, has_data, tracker_id";

/// Insere ou retourne l'etat d'essaim pour `infohash`
/// (equivalent de `TorrentState.get_for_update` + `TorrentState()`).
pub fn upsert_torrent_state(conn: &Connection, infohash: &[u8]) -> Result<TorrentStateRow> {
    conn.execute(
        "INSERT INTO torrent_state(infohash) VALUES (?1)
         ON CONFLICT(infohash) DO NOTHING",
        params![infohash],
    )?;
    get_torrent_state(conn, infohash)?
        .ok_or_else(|| crate::DbError::Corrupt("torrent_state absent apres upsert".into()))
}

/// Liste tous les etats d'essaim connus — evite le N+1 de
/// `GET /api/downloads` (une requete puis indexation memoire).
pub fn list_torrent_states(conn: &Connection) -> Result<Vec<TorrentStateRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {TS_COLS} FROM torrent_state"))?;
    let rows = stmt.query_map([], torrent_state_from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Lit l'etat d'un essaim par info-hash.
pub fn get_torrent_state(conn: &Connection, infohash: &[u8]) -> Result<Option<TorrentStateRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {TS_COLS} FROM torrent_state WHERE infohash = ?1"),
            params![infohash],
            torrent_state_from_row,
        )
        .optional()?)
}

/// Met a jour les compteurs de sante d'un essaim.
pub fn update_torrent_health(
    conn: &Connection,
    infohash: &[u8],
    seeders: i64,
    leechers: i64,
    last_check: i64,
    self_checked: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE torrent_state
         SET seeders = ?2, leechers = ?3, last_check = ?4, self_checked = ?5
         WHERE infohash = ?1",
        params![infohash, seeders, leechers, last_check, self_checked as i64],
    )?;
    Ok(())
}

/// Insere ou retourne le tracker pour `url`
/// (`TrackerState.get_for_update` + `TrackerState()`).
pub fn upsert_tracker(conn: &Connection, url: &str) -> Result<TrackerStateRow> {
    conn.execute(
        "INSERT INTO tracker_state(url) VALUES (?1)
         ON CONFLICT(url) DO NOTHING",
        params![url],
    )?;
    get_tracker(conn, url)?
        .ok_or_else(|| crate::DbError::Corrupt("tracker_state absent apres upsert".into()))
}

/// Lit un tracker par URL.
pub fn get_tracker(conn: &Connection, url: &str) -> Result<Option<TrackerStateRow>> {
    Ok(conn
        .query_row(
            "SELECT rowid, url, last_check, alive, failures
             FROM tracker_state WHERE url = ?1",
            params![url],
            |r| {
                Ok(TrackerStateRow {
                    rowid: r.get("rowid")?,
                    url: r.get("url")?,
                    last_check: r.get("last_check")?,
                    alive: r.get::<_, i64>("alive")? != 0,
                    failures: r.get("failures")?,
                })
            },
        )
        .optional()?)
}

/// Met a jour l'etat d'un tracker apres un controle.
pub fn update_tracker(
    conn: &Connection,
    url: &str,
    alive: bool,
    last_check: i64,
    failures: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE tracker_state
         SET alive = ?2, last_check = ?3, failures = ?4
         WHERE url = ?1",
        params![url, alive as i64, last_check, failures],
    )?;
    Ok(())
}

/// Lie un essaim a un tracker (equivalent de `health.trackers.add`).
pub fn link_tracker(conn: &Connection, infohash: &[u8], tracker_url: &str) -> Result<()> {
    let ts = upsert_torrent_state(conn, infohash)?;
    let tr = upsert_tracker(conn, tracker_url)?;
    conn.execute(
        "INSERT INTO torrent_state_tracker(torrent_state_rowid, tracker_state_rowid)
         VALUES (?1, ?2)
         ON CONFLICT DO NOTHING",
        params![ts.rowid, tr.rowid],
    )?;
    Ok(())
}

/// Liste les URLs de trackers lies a un essaim.
pub fn trackers_of(conn: &Connection, infohash: &[u8]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT tr.url FROM tracker_state tr
         JOIN torrent_state_tracker tt ON tt.tracker_state_rowid = tr.rowid
         JOIN torrent_state ts ON ts.rowid = tt.torrent_state_rowid
         WHERE ts.infohash = ?1",
    )?;
    let rows = stmt.query_map(params![infohash], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
