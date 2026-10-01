//! Modele d'un fichier `.torrent` (BEP 3, support partiel BEP 52 v2).
//!
//! L'info-hash v1 est le SHA-1 de la **serialisation bencode brute** du
//! dictionnaire `info` (pas d'une re-serialisation : les cles doivent
//! rester dans l'ordre exact du fichier source).

use std::collections::BTreeMap;

use onionbit_crypto::hash::{self, InfoHashV1, InfoHashV2};

use crate::bencode::{parser, BValue};
use crate::error::{FormatError, Result};
use crate::limits;

/// Nombre d'octets d'un hash de piece v1 (SHA-1).
const PIECE_HASH_LEN_V1: usize = 20;

/// Un fichier contenu dans un torrent multi-fichiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// Chemin relatif (segments du chemin joins par '/').
    pub path: Vec<String>,
    /// Taille en octets.
    pub length: u64,
}

/// Metadonnees d'un fichier `.torrent`.
#[derive(Debug, Clone)]
pub struct TorrentMeta {
    /// Tracker principal (`announce`).
    pub announce: Option<String>,
    /// Tous les tiers de trackers (`announce-list`, aplati).
    pub announce_list: Vec<Vec<String>>,
    /// Nom du contenu (`info.name` ou `info.name.utf-8`).
    pub name: String,
    /// Taille d'une piece en octets.
    pub piece_length: u64,
    /// Hashes de pieces v1 (SHA-1 par piece).
    pub pieces_v1: Vec<InfoHashV1>,
    /// Fichiers du torrent (un seul pour un torrent mono-fichier).
    pub files: Vec<TorrentFile>,
    /// Taille totale du contenu.
    pub total_size: u64,
    /// Info-hash v1 : SHA-1 du dictionnaire `info` brut.
    pub info_hash: InfoHashV1,
    /// Racine Merkle v2 (`info.pieces root`), si torrent hybride/v2.
    pub pieces_root_v2: Option<InfoHashV2>,
    /// Torrent marque prive (pas de DHT/PEX).
    pub private: bool,
    /// Commentaire optionnel.
    pub comment: Option<String>,
    /// Createur optionnel (`created by`).
    pub created_by: Option<String>,
    /// Date de creation (timestamp Unix).
    pub creation_date: Option<i64>,
    /// Sources web (`url-list`).
    pub webseeds: Vec<String>,
    /// Octets bruts du dictionnaire `info` (a reannoncer pour BEP 9).
    pub raw_info: Vec<u8>,
}

impl TorrentMeta {
    /// Parse un fichier `.torrent` (octets bruts).
    ///
    /// Borne par [`limits::MAX_TORRENT_FILE_SIZE`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() > limits::MAX_TORRENT_FILE_SIZE {
            return Err(FormatError::SizeExceeded {
                actual: data.len(),
                max: limits::MAX_TORRENT_FILE_SIZE,
            });
        }
        let root = parser::decode(data)?;
        let dict = root
            .as_dict()
            .ok_or(FormatError::MissingField("racine non-dictionnaire"))?;

        // L'info-hash se calcule sur les octets *bruts* du dict `info`.
        let raw_info = extract_raw_info(data)?;
        let info_hash = hash::sha1(raw_info);

        let info = dict
            .get(b"info" as &[u8])
            .and_then(BValue::as_dict)
            .ok_or(FormatError::MissingField("info"))?;

        let name: String = dict_str(info, b"name.utf-8")
            .or_else(|| dict_str(info, b"name"))
            .map(String::from)
            .ok_or(FormatError::MissingField("info.name"))?;

        let piece_length = dict_int(info, b"piece length")
            .ok_or(FormatError::MissingField("info.piece length"))?
            as u64;

        let pieces_raw = dict_bytes(info, b"pieces").unwrap_or(&[]);
        if !pieces_raw.is_empty() && !pieces_raw.len().is_multiple_of(PIECE_HASH_LEN_V1) {
            return Err(FormatError::BadBencode {
                offset: 0,
                reason: format!(
                    "info.pieces n'est pas un multiple de {PIECE_HASH_LEN_V1} octets ({})",
                    pieces_raw.len()
                ),
            });
        }
        let pieces_v1: Vec<InfoHashV1> = pieces_raw.as_chunks::<PIECE_HASH_LEN_V1>().0.to_vec();

        let pieces_root_v2 = dict_bytes(info, b"pieces root").map(|b| {
            let mut h = [0u8; 32];
            h.copy_from_slice(&b[..32.min(b.len())]);
            h
        });

        let private = dict_int(info, b"private") == Some(1);

        // Mode mono-fichier (info.length) ou multi-fichiers (info.files).
        let (files, total_size) = if let Some(len) = dict_int(info, b"length") {
            (
                vec![TorrentFile {
                    path: vec![name.clone()],
                    length: len as u64,
                }],
                len as u64,
            )
        } else if let Some(files) = info.get(b"files" as &[u8]).and_then(BValue::as_list) {
            let mut out = Vec::with_capacity(files.len());
            let mut total = 0u64;
            for f in files {
                let fd = f
                    .as_dict()
                    .ok_or(FormatError::MissingField("info.files[] non-dictionnaire"))?;
                let length =
                    dict_int(fd, b"length").ok_or(FormatError::MissingField("file.length"))? as u64;
                let path = dict_get(fd, b"path.utf-8")
                    .or_else(|| dict_get(fd, b"path"))
                    .and_then(BValue::as_list)
                    .ok_or(FormatError::MissingField("file.path"))?
                    .iter()
                    .map(|seg| seg.as_str().unwrap_or_default().to_string())
                    .collect();
                total += length;
                out.push(TorrentFile { path, length });
            }
            (out, total)
        } else {
            // Torrent v2 pur : la taille se deduit de l'arbre de fichiers.
            let mut files = Vec::new();
            if let Some(tree) = dict_get(info, b"file tree").and_then(BValue::as_dict) {
                collect_v2_files(tree, &mut Vec::new(), &mut files)?;
            }
            let total = files.iter().map(|f| f.length).sum();
            (files, total)
        };

        Ok(Self {
            announce: dict_str(dict, b"announce").map(String::from),
            announce_list: dict
                .get(b"announce-list" as &[u8])
                .and_then(BValue::as_list)
                .map(|tiers| {
                    tiers
                        .iter()
                        .filter_map(|t| {
                            t.as_list().map(|urls| {
                                urls.iter()
                                    .filter_map(|u| u.as_str().map(String::from))
                                    .collect()
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            name,
            piece_length,
            pieces_v1,
            files,
            total_size,
            info_hash,
            pieces_root_v2,
            private,
            comment: dict_str(dict, b"comment.utf-8")
                .or_else(|| dict_str(dict, b"comment"))
                .map(String::from),
            created_by: dict_str(dict, b"created by").map(String::from),
            creation_date: dict_int(dict, b"creation date"),
            webseeds: dict
                .get(b"url-list" as &[u8])
                .map(|v| match v {
                    BValue::Bytes(b) => vec![String::from_utf8_lossy(b).into_owned()],
                    BValue::List(l) => l
                        .iter()
                        .filter_map(|u| u.as_str().map(String::from))
                        .collect(),
                    _ => Vec::new(),
                })
                .unwrap_or_default(),
            raw_info: raw_info.to_vec(),
        })
    }

    /// Info-hash formate en hexadecimal (identifiant usuel d'un torrent).
    pub fn info_hash_hex(&self) -> String {
        hash::to_hex(&self.info_hash)
    }

    /// URLs de trackers du torrent : `announce` puis chaque tier de
    /// `announce-list` aplati, dedoublonne en conservant l'ordre —
    /// equivalent de `tdef.atp.trackers` Python.
    pub fn tracker_urls(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(a) = &self.announce {
            out.push(a.clone());
        }
        for tier in &self.announce_list {
            for url in tier {
                if !out.iter().any(|u| u == url) {
                    out.push(url.clone());
                }
            }
        }
        out
    }
}

/// Retire `announce` et `announce-list` d'un fichier `.torrent` brut.
///
/// Chirurgical : les entrees du dictionnaire racine sont localisees par
/// leurs spans d'octets et decoupees — le sous-arbre `info` est recopie
/// **verbatim** (l'info-hash est preserve meme si l'encodage source
/// n'est pas canonique). Equivalent de vider `tdef.atp.trackers` avant
/// re-add : librqbit fusionne toujours les trackers de la source avec
/// ceux de `AddTorrentOptions::trackers`, il faut donc les retirer de
/// la source pour qu'une suppression persiste.
pub fn strip_trackers(data: &[u8]) -> Result<Vec<u8>> {
    if data.first() != Some(&b'd') {
        return Err(FormatError::BadBencode {
            offset: 0,
            reason: "un .torrent doit commencer par un dictionnaire".into(),
        });
    }
    let mut spans_to_cut: Vec<(usize, usize)> = Vec::new();
    let mut pos = 1;
    while pos < data.len() && data[pos] != b'e' {
        let entry_start = pos;
        let key = parser::decode_at(data, pos, 1)?;
        let key_bytes = key
            .value
            .as_bytes()
            .ok_or(FormatError::BadBencode {
                offset: pos,
                reason: "cle de dictionnaire non-binaire".into(),
            })?
            .to_vec();
        let val = parser::decode_at(data, key.end, 1)?;
        pos = val.end;
        if key_bytes == b"announce" || key_bytes == b"announce-list" {
            spans_to_cut.push((entry_start, val.end));
        }
    }
    if pos >= data.len() {
        return Err(FormatError::Truncated { offset: pos });
    }
    if spans_to_cut.is_empty() {
        return Ok(data.to_vec());
    }
    let mut out = Vec::with_capacity(data.len());
    let mut cursor = 0;
    for (start, end) in spans_to_cut {
        out.extend_from_slice(&data[cursor..start]);
        cursor = end;
    }
    out.extend_from_slice(&data[cursor..]);
    Ok(out)
}

/// Cree une copie publique d'un `.torrent` prive : retire le flag
/// `private` du dictionnaire `info` (=> nouvel info-hash, DHT/PEX
/// reactives) ainsi que `announce`/`announce-list` — l'URL du
/// tracker prive ne doit pas figurer dans le jumeau, le partage se
/// fait par la DHT/PEX a travers les circuits.
///
/// Chirurgical comme [`strip_trackers`] : seules les entrees
/// concernees sont decoupees, le reste (dont `pieces`) est recopie
/// verbatim — pas de re-hash des donnees, la copie seede les memes
/// fichiers instantanement.
pub fn to_public(data: &[u8]) -> Result<Vec<u8>> {
    // 1. Les trackers (prives) hors de la copie publique.
    let data = strip_trackers(data)?;
    // 2. `private` a l'interieur du dict `info` — son retrait change
    //    l'info-hash (l'info est re-serialisee sans la cle).
    let (info_start, info_end) = raw_info_span(&data)?;
    let mut pos = info_start + 1; // saute le 'd' du dict info
    let mut cut = None;
    while pos < info_end && data[pos] != b'e' {
        let entry_start = pos;
        let key = parser::decode_at(&data, pos, 1)?;
        let val = parser::decode_at(&data, key.end, 1)?;
        pos = val.end;
        if matches!(&key.value, BValue::Bytes(b) if b == b"private") {
            cut = Some((entry_start, val.end));
            break;
        }
    }
    if let Some((s, e)) = cut {
        let mut out = Vec::with_capacity(data.len());
        out.extend_from_slice(&data[..s]);
        out.extend_from_slice(&data[e..]);
        Ok(out)
    } else {
        Ok(data)
    }
}

/// Extrait le sous-arbre "file tree" d'un torrent v2 en liste de fichiers.
fn collect_v2_files(
    tree: &BTreeMap<Vec<u8>, BValue>,
    path: &mut Vec<String>,
    out: &mut Vec<TorrentFile>,
) -> Result<()> {
    for (name, node) in tree {
        if let Some(dir) = node.as_dict() {
            if let Some(leaf) = dir.get(b"" as &[u8]).and_then(BValue::as_dict) {
                let length = dict_int(leaf, b"length").unwrap_or(0) as u64;
                let mut p = path.clone();
                p.push(String::from_utf8_lossy(name).into_owned());
                out.push(TorrentFile { path: p, length });
            } else {
                path.push(String::from_utf8_lossy(name).into_owned());
                collect_v2_files(dir, path, out)?;
                path.pop();
            }
        }
    }
    Ok(())
}

/// Cherche la valeur `info` dans le dictionnaire racine *en suivant les
/// offsets bruts*, pour recuperer sa serialisation d'origine (necessaire
/// a l'info-hash SHA-1 : la re-serialisation canonique pourrait differer
/// si le fichier source n'etait pas canonique).
fn extract_raw_info(data: &[u8]) -> Result<&[u8]> {
    let (start, end) = raw_info_span(data)?;
    Ok(&data[start..end])
}

/// Offsets `[start, end)` de la valeur `info` dans le dictionnaire
/// racine (meme marche d'offsets bruts que [`extract_raw_info`]).
fn raw_info_span(data: &[u8]) -> Result<(usize, usize)> {
    if data.first() != Some(&b'd') {
        return Err(FormatError::MissingField("racine non-dictionnaire"));
    }
    let mut pos = 1;
    loop {
        match data.get(pos) {
            None | Some(b'e') => return Err(FormatError::MissingField("info")),
            Some(b'0'..=b'9') => {
                let key = parser::decode_at(data, pos, 0)?;
                pos = key.end;
                let is_info = matches!(&key.value, BValue::Bytes(b) if b == b"info");
                let val = parser::decode_at(data, pos, 0)?;
                if is_info {
                    return Ok((pos, val.end));
                }
                pos = val.end;
            }
            Some(_) => {
                return Err(FormatError::BadBencode {
                    offset: pos,
                    reason: "cle de dictionnaire racine non-chaine".into(),
                })
            }
        }
    }
}

fn dict_get<'a>(dict: &'a BTreeMap<Vec<u8>, BValue>, key: &[u8]) -> Option<&'a BValue> {
    dict.get(key)
}

fn dict_str<'a>(dict: &'a BTreeMap<Vec<u8>, BValue>, key: &[u8]) -> Option<&'a str> {
    dict.get(key).and_then(BValue::as_str)
}

fn dict_int(dict: &BTreeMap<Vec<u8>, BValue>, key: &[u8]) -> Option<i64> {
    dict.get(key).and_then(BValue::as_int)
}

fn dict_bytes<'a>(dict: &'a BTreeMap<Vec<u8>, BValue>, key: &[u8]) -> Option<&'a [u8]> {
    dict.get(key).and_then(BValue::as_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construit un .torrent minimal en bencode.
    fn build_torrent() -> Vec<u8> {
        // d8:announce14:udp://t.local4:infod6:lengthi42e4:name8:test.bin12:piece lengthi16384e6:pieces20:<20 zeros>ee
        let mut pieces = String::from("6:pieces20:");
        for _ in 0..20 {
            pieces.push('\0');
        }
        let info = format!(
            "d6:lengthi42e4:name8:test.bin12:piece lengthi16384e{}e",
            pieces
        );
        let s = format!("d8:announce13:udp://t.local4:info{}e", info);
        s.into_bytes()
    }

    #[test]
    fn parse_torrent_mono_fichier() {
        let data = build_torrent();
        let t = TorrentMeta::parse(&data).unwrap();
        assert_eq!(t.name, "test.bin");
        assert_eq!(t.piece_length, 16384);
        assert_eq!(t.total_size, 42);
        assert_eq!(t.files.len(), 1);
        assert_eq!(t.announce.as_deref(), Some("udp://t.local"));
        assert_eq!(t.pieces_v1.len(), 1);
        assert_eq!(t.info_hash.len(), 20);
    }

    #[test]
    fn info_hash_correspond_au_sha1_du_dict_info_brut() {
        let data = build_torrent();
        let t = TorrentMeta::parse(&data).unwrap();
        // L'info-hash doit etre SHA-1 des octets bruts du dict `info`.
        assert_eq!(t.info_hash, hash::sha1(&t.raw_info));
    }

    #[test]
    fn parse_rejette_un_fichier_trop_gros() {
        let big = vec![b'd'; limits::MAX_TORRENT_FILE_SIZE + 1];
        assert!(TorrentMeta::parse(&big).is_err());
    }

    #[test]
    fn parse_rejette_un_torrent_sans_info() {
        assert!(TorrentMeta::parse(b"d1:ai1ee").is_err());
    }

    /// `.torrent` prive minimal (announce + private=1 dans info).
    fn build_private_torrent() -> Vec<u8> {
        let mut pieces = String::from("6:pieces20:");
        for _ in 0..20 {
            pieces.push('\0');
        }
        let info = format!(
            "d6:lengthi42e4:name8:test.bin12:piece lengthi16384e{}7:privatei1ee",
            pieces
        );
        format!(
            "d8:announce13:udp://t.local13:announce-listll14:udp://t2.localee4:info{}e",
            info
        )
        .into_bytes()
    }

    #[test]
    fn to_public_retire_private_et_trackers() {
        let data = build_private_torrent();
        let priv_meta = TorrentMeta::parse(&data).unwrap();
        assert!(priv_meta.private);

        let public = to_public(&data).unwrap();
        let pub_meta = TorrentMeta::parse(&public).unwrap();
        assert!(!pub_meta.private);
        // Nouvel info-hash (le retrait de `private` modifie `info`).
        assert_ne!(pub_meta.info_hash, priv_meta.info_hash);
        // Trackers prives retires de la copie.
        assert!(pub_meta.announce.is_none());
        assert!(pub_meta.announce_list.is_empty());
        // Contenu (pieces, fichiers) inchange — reseeding immediat.
        assert_eq!(pub_meta.pieces_v1, priv_meta.pieces_v1);
        assert_eq!(pub_meta.files, priv_meta.files);
    }

    #[test]
    fn to_public_idempotent_sur_torrent_public() {
        let data = build_torrent();
        let public = to_public(&data).unwrap();
        let meta = TorrentMeta::parse(&public).unwrap();
        assert!(!meta.private);
        assert!(meta.announce.is_none());
    }
}
