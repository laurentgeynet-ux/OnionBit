//! Trackers par telechargement.
//!
//! Deux mecanismes equivalents a ceux de Tribler Python :
//!
//! - **trackers par defaut** : fichier `download_defaults/trackers_file`
//!   (format uTorrent — une URL par ligne, lignes vides ignorees,
//!   cf. `cached_read`), optionnellement synchronise depuis
//!   `trackers_file_sync_url` (`sync_default_trackers_file` : TTL
//!   d'une heure) ;
//! - **ensemble effectif** rejoue a chaque re-add : trackers de la
//!   source (announce/announce-list du `.torrent` ou `tr` du magnet)
//!   ∪ `extra_trackers` ∖ `removed_trackers`. librqbit fusionne les
//!   trackers de la source avec `AddTorrentOptions::trackers` sans
//!   permettre le retrait a chaud (`replace_trackers` Python n'a pas
//!   d'equivalent) : pour honorer un retrait, la source est reecrite
//!   sans ses trackers et l'ensemble effectif complet est passe en
//!   option (cf. `tribler_format::torrent::strip_trackers` et
//!   `tribler_format::magnet::strip_trackers`).

use std::collections::BTreeSet;
use std::path::Path;

use tribler_db::models::DownloadRow;

/// TTL de la synchronisation de `trackers_file` depuis
/// `trackers_file_sync_url` (`Download.LAST_TRACKER_FILE_SYNC` Python :
/// 3600 secondes).
pub const TRACKER_SYNC_TTL_SECS: u64 = 3600;

/// Parse le contenu du fichier de trackers par defaut : lignes non
/// vides trimees (`cached_read` Python — le format uTorrent separe
/// les URLs par des lignes vides).
pub fn parse_trackers_file(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Resout le chemin de `trackers_file` (relatif a `state_dir` si non
/// absolu).
pub fn trackers_file_path(state_dir: &Path, configured: &str) -> Option<std::path::PathBuf> {
    if configured.is_empty() {
        return None;
    }
    let p = Path::new(configured);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        state_dir.join(p)
    })
}

/// Trackers declares par la source persistee : `announce` +
/// `announce-list` du `.torrent`, ou parametres `tr` du magnet.
/// Vide si la source est une URL `http(s)` ou non analysable.
pub fn source_trackers(row: &DownloadRow) -> Vec<String> {
    if let Some(bytes) = &row.torrent_data {
        if let Ok(meta) = tribler_format::torrent::TorrentMeta::parse(bytes) {
            return meta.tracker_urls();
        }
    }
    if row
        .source_uri
        .starts_with(tribler_format::magnet::MAGNET_PREFIX)
    {
        if let Ok(m) = tribler_format::magnet::MagnetLink::parse(&row.source_uri) {
            return m.trackers;
        }
    }
    Vec::new()
}

/// Ensemble effectif de trackers a passer au moteur : union des
/// trackers de la source et des ajouts a chaud, moins les retraits
/// (`replace_trackers` + filtrage de `tdef.atp.trackers` Python).
pub fn effective_trackers(row: &DownloadRow) -> Vec<String> {
    let removed: BTreeSet<&str> = row.removed_trackers.iter().map(String::as_str).collect();
    if removed.is_empty() {
        return row.extra_trackers.clone();
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    source_trackers(row)
        .into_iter()
        .chain(row.extra_trackers.iter().cloned())
        .filter(|u| !removed.contains(u.as_str()))
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Source effective a reinjecter au moteur. Quand des retraits sont
/// enregistres, la source est reecrite **sans** ses trackers propres
/// (sinon librqbit les refusionnerait avec `opts.trackers`) et
/// [`effective_trackers`] porte l'ensemble complet. Sans retrait, la
/// source est inchangee et seuls les `extra_trackers` sont passes.
///
/// Retourne `(torrent_data, source_uri)` potentiellement modifies.
pub fn effective_source(row: &DownloadRow) -> (Option<Vec<u8>>, String) {
    if row.removed_trackers.is_empty() {
        return (row.torrent_data.clone(), row.source_uri.clone());
    }
    let torrent_data = row
        .torrent_data
        .as_ref()
        .map(|b| tribler_format::torrent::strip_trackers(b).unwrap_or_else(|_| b.clone()));
    let source_uri = if row
        .source_uri
        .starts_with(tribler_format::magnet::MAGNET_PREFIX)
    {
        tribler_format::magnet::strip_trackers(&row.source_uri)
    } else {
        row.source_uri.clone()
    };
    (torrent_data, source_uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_row() -> DownloadRow {
        DownloadRow {
            source_uri: "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d\
                         &tr=udp%3A%2F%2Fa.local%3A80&tr=udp%3A%2F%2Fb.local%3A80&dn=x"
                .into(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_fichier_utorrent() {
        let c = "udp://a:80\n\nudp://b:80\r\n\r\n http://c/announce \n";
        assert_eq!(
            parse_trackers_file(c),
            vec!["udp://a:80", "udp://b:80", "http://c/announce"]
        );
    }

    #[test]
    fn effectif_union_moins_retraits() {
        let mut row = base_row();
        row.extra_trackers = vec!["udp://extra:1".into(), "udp://a.local:80".into()];
        row.removed_trackers = vec!["udp://b.local:80".into()];
        assert_eq!(
            effective_trackers(&row),
            vec!["udp://a.local:80", "udp://extra:1"]
        );
    }

    #[test]
    fn effectif_sans_retrait_ne_passe_que_les_extras() {
        let mut row = base_row();
        row.extra_trackers = vec!["udp://extra:1".into()];
        assert_eq!(effective_trackers(&row), vec!["udp://extra:1"]);
    }

    #[test]
    fn source_magnet_purgee_des_tr() {
        let mut row = base_row();
        row.removed_trackers = vec!["udp://b.local:80".into()];
        let (_data, uri) = effective_source(&row);
        assert!(!uri.contains("tr="));
        assert!(uri.contains("xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d"));
        assert!(uri.contains("dn=x"));
    }

    #[test]
    fn source_torrent_purgee_infohash_stable() {
        // announce + announce-list (un tier, une URL) + info (private=1
        // dans le dict info, cles triees canoniquement).
        let raw = b"d8:announce13:udp://t.local13:announce-listll14:udp://t2.localee4:infod6:lengthi42e4:name8:test.bin12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaa7:privatei1eee";
        let meta = tribler_format::torrent::TorrentMeta::parse(raw).expect("torrent valide");
        assert_eq!(meta.tracker_urls(), vec!["udp://t.local", "udp://t2.local"]);

        let stripped = tribler_format::torrent::strip_trackers(raw).expect("strip");
        let meta2 = tribler_format::torrent::TorrentMeta::parse(&stripped).expect("reparse");
        assert!(meta2.tracker_urls().is_empty());
        assert_eq!(meta.info_hash, meta2.info_hash, "infohash preserve");

        let mut row = base_row();
        row.torrent_data = Some(raw.to_vec());
        row.removed_trackers = vec!["udp://t.local".into()];
        row.extra_trackers = vec!["udp://extra:1".into()];
        let (data, _uri) = effective_source(&row);
        let meta3 = tribler_format::torrent::TorrentMeta::parse(&data.unwrap()).unwrap();
        assert!(meta3.tracker_urls().is_empty());
        assert_eq!(
            effective_trackers(&row),
            vec!["udp://t2.local", "udp://extra:1"]
        );
    }
}
