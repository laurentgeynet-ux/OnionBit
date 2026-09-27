//! Acces a `downloads` : telechargements connus du daemon, pour
//! restaurer la session au demarrage.

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::DownloadRow;
use crate::Result;

const COLS: &str = "rowid, infohash, name, source_uri, torrent_data,
                    output_dir, added_on, paused, finished, anon_hops";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRow> {
    Ok(DownloadRow {
        rowid: r.get("rowid")?,
        infohash: r.get("infohash")?,
        name: r.get("name")?,
        source_uri: r.get("source_uri")?,
        torrent_data: r.get("torrent_data")?,
        output_dir: r.get("output_dir")?,
        added_on: r.get("added_on")?,
        paused: r.get::<_, i64>("paused")? != 0,
        finished: r.get::<_, i64>("finished")? != 0,
        anon_hops: r.get("anon_hops")?,
    })
}

/// Insere ou met a jour un telechargement (cle : infohash).
/// Retourne le rowid.
pub fn upsert(conn: &Connection, row: &DownloadRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO downloads(
            infohash, name, source_uri, torrent_data, output_dir,
            added_on, paused, finished, anon_hops
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(infohash) DO UPDATE SET
            name = excluded.name,
            torrent_data = coalesce(excluded.torrent_data, downloads.torrent_data),
            output_dir = excluded.output_dir,
            paused = excluded.paused,
            finished = excluded.finished,
            anon_hops = excluded.anon_hops",
        params![
            row.infohash,
            row.name,
            row.source_uri,
            row.torrent_data,
            row.output_dir,
            row.added_on,
            row.paused as i64,
            row.finished as i64,
            row.anon_hops,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Lit un telechargement par info-hash.
pub fn get(conn: &Connection, infohash: &[u8]) -> Result<Option<DownloadRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM downloads WHERE infohash = ?1"),
            params![infohash],
            from_row,
        )
        .optional()?)
}

/// Liste tous les telechargements connus.
pub fn list(conn: &Connection) -> Result<Vec<DownloadRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM downloads ORDER BY added_on"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Supprime un telechargement connu.
pub fn delete(conn: &Connection, infohash: &[u8]) -> Result<()> {
    conn.execute(
        "DELETE FROM downloads WHERE infohash = ?1",
        params![infohash],
    )?;
    Ok(())
}

/// Marque un telechargement termine.
pub fn set_finished(conn: &Connection, infohash: &[u8], finished: bool) -> Result<()> {
    conn.execute(
        "UPDATE downloads SET finished = ?2 WHERE infohash = ?1",
        params![infohash, finished as i64],
    )?;
    Ok(())
}
