//! Acces a `downloads` : telechargements connus du daemon, pour
//! restaurer la session au demarrage, et reglages par
//! telechargement (equivalent du `DownloadConfig` checkpointe par
//! Tribler dans `dlcheckpoints/<infohash>.conf`).

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::DownloadRow;
use crate::Result;

const COLS: &str = "rowid, infohash, name, source_uri, torrent_data,
                    output_dir, added_on, paused, finished, anon_hops,
                    safe_seeding, user_stopped, upload_limit, download_limit,
                    seeding_ratio, auto_managed, queue_position, completed_dir,
                    selected_files, file_priorities, extra_trackers,
                    time_finished";

/// Encode une liste d'entiers en CSV (`"0,2,3,"` — format du champ
/// `download_defaults/files` des checkpoints Python).
fn encode_csv(list: &[i64]) -> String {
    let mut s = String::new();
    for v in list {
        s.push_str(&v.to_string());
        s.push(',');
    }
    s
}

/// Decode le CSV des checkpoints Python (cellules vides ignorees).
fn decode_csv(text: &str) -> Vec<i64> {
    text.split(',')
        .filter_map(|c| c.trim().parse::<i64>().ok())
        .collect()
}

/// Encode une liste d'URLs (une par ligne — une URL de tracker ne
/// contient jamais de saut de ligne).
fn encode_urls(list: &[String]) -> String {
    list.join("\n")
}

fn decode_urls(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRow> {
    let selected_files: Option<String> = r.get("selected_files")?;
    let file_priorities: Option<String> = r.get("file_priorities")?;
    let extra_trackers: Option<String> = r.get("extra_trackers")?;
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
        safe_seeding: r.get::<_, i64>("safe_seeding")? != 0,
        user_stopped: r.get::<_, i64>("user_stopped")? != 0,
        upload_limit: r.get("upload_limit")?,
        download_limit: r.get("download_limit")?,
        seeding_ratio: r.get("seeding_ratio")?,
        auto_managed: r.get::<_, i64>("auto_managed")? != 0,
        queue_position: r.get("queue_position")?,
        completed_dir: r.get("completed_dir")?,
        selected_files: selected_files.map(|s| decode_csv(&s)),
        file_priorities: file_priorities.map(|s| decode_csv(&s)),
        extra_trackers: extra_trackers.map(|s| decode_urls(&s)).unwrap_or_default(),
        time_finished: r.get("time_finished")?,
    })
}

/// Insere ou met a jour un telechargement (cle : infohash).
/// Retourne le rowid.
///
/// Semantique **remplacement complet** : au conflit, toutes les
/// colonnes sont reecrites depuis `row`. Les flux de re-add internes
/// (update_hops, recheck, move_storage) doivent relire la ligne avant
/// suppression et la reinjecter si les reglages doivent survivre —
/// comme le `checkpoint` Python conserve le `DownloadConfig`.
pub fn upsert(conn: &Connection, row: &DownloadRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO downloads(
            infohash, name, source_uri, torrent_data, output_dir,
            added_on, paused, finished, anon_hops,
            safe_seeding, user_stopped, upload_limit, download_limit,
            seeding_ratio, auto_managed, queue_position, completed_dir,
            selected_files, file_priorities, extra_trackers, time_finished
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)
         ON CONFLICT(infohash) DO UPDATE SET
            name = excluded.name,
            torrent_data = excluded.torrent_data,
            output_dir = excluded.output_dir,
            added_on = excluded.added_on,
            paused = excluded.paused,
            finished = excluded.finished,
            anon_hops = excluded.anon_hops,
            safe_seeding = excluded.safe_seeding,
            user_stopped = excluded.user_stopped,
            upload_limit = excluded.upload_limit,
            download_limit = excluded.download_limit,
            seeding_ratio = excluded.seeding_ratio,
            auto_managed = excluded.auto_managed,
            queue_position = excluded.queue_position,
            completed_dir = excluded.completed_dir,
            selected_files = excluded.selected_files,
            file_priorities = excluded.file_priorities,
            extra_trackers = excluded.extra_trackers,
            time_finished = excluded.time_finished",
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
            row.safe_seeding as i64,
            row.user_stopped as i64,
            row.upload_limit,
            row.download_limit,
            row.seeding_ratio,
            row.auto_managed as i64,
            row.queue_position,
            row.completed_dir,
            row.selected_files.as_deref().map(encode_csv),
            row.file_priorities.as_deref().map(encode_csv),
            (!row.extra_trackers.is_empty()).then(|| encode_urls(&row.extra_trackers)),
            row.time_finished,
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

/// Marque un telechargement termine (`time_finished` = instant de
/// completion, comme `atp.completed_time` Python).
pub fn set_finished(conn: &Connection, infohash: &[u8], finished: bool) -> Result<()> {
    conn.execute(
        "UPDATE downloads SET finished = ?2,
             time_finished = CASE WHEN ?2 <> 0 AND time_finished = 0
                                  THEN unixepoch() ELSE time_finished END
         WHERE infohash = ?1",
        params![infohash, finished as i64],
    )?;
    Ok(())
}

/// Prochaine position de file (`max(queue_position) + 1`).
pub fn next_queue_position(conn: &Connection) -> Result<i64> {
    let pos: Option<i64> =
        conn.query_row("SELECT MAX(queue_position) FROM downloads", [], |r| {
            r.get(0)
        })?;
    Ok(pos.unwrap_or(0) + 1)
}
