// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Canal personnel signe (ADR-0025, etape 98).
//!
//! La racine est un `COLLECTION_NODE` (220) — type filaire reel
//! cote Python (`METADATA_TYPE_TO_PAYLOAD_CLASS` : le 200
//! `CHANNEL_NODE` n'a **aucune** classe de payload, il ne doit
//! jamais etre emis — `UnknownBlobTypeException` tuerait le blob
//! chez un pair Tribler ; le 200 n'existe chez nous que comme
//! placeholder interne jamais servi).
//!
//! Convention OnionBit (auto-coherente avec l'abonnement) : la
//! racine a `origin_id == id_` (son propre identifiant de canal) ;
//! les entrees `CHANNEL_TORRENT` portent `origin_id = id_` de la
//! racine. Retrait = pierre tombale `DELETED` (500) signee dont la
//! `delete_signature` reference la signature de l'entree supprimee
//! — la ligne visee passe a `metadata_type = 500` et est servie
//! re-signee a la volee (cf. `SessionContentProvider::row_to_entry`).
//!
//! Extension OnionBit : Tribler 8.x n'expose plus d'endpoint de
//! publication de canal — le filaire reste compatible (400/220/500
//! sont parses puis ignores par `process_payload` Python, qui ne
//! persiste que `REGULAR_TORRENT`).

use onionbit_crypto::ipv8::keys::{LibNaClSecretKey, SIGNATURE_LENGTH};
use onionbit_format::mdblob::{
    encode_entry, ChannelMetadataPayload, ChannelNodePayload, CollectionNodePayload, MetadataEntry,
    SignedPayloadHeader, TorrentMetadataPayload,
};
use rusqlite::Connection;

use crate::error::{CoreError, Result};

/// `public_key` du canal personnel = cle publique Ed25519 au format
/// filaire (`key_to_bin()[10..]`, 64 octets).
pub fn personal_channel_pk(key: &LibNaClSecretKey) -> Vec<u8> {
    key.public_key().to_bin()[10..].to_vec()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `id_` libre pour `(public_key, id_)` : timestamp milliseconde
/// (meme convention que `id_` Python), incremente en cas de
/// collision — borne a quelques tentatives.
fn fresh_node_id(conn: &Connection, pk: &[u8]) -> Result<i64> {
    let base = (now_unix() * 1000) as i64;
    for id in base..base.saturating_add(1000) {
        if onionbit_db::channel::get_by_pk_id(conn, pk, id)?.is_none() {
            return Ok(id);
        }
    }
    Err(CoreError::InvalidState(
        "canal personnel : plus d'id_ libre",
    ))
}

/// Cree (ou retourne) la racine `COLLECTION_NODE` du canal
/// personnel ; retourne son `id_` (= identifiant de canal).
/// `title` n'est utilise qu'a la creation — `set_title` pour
/// renommer.
fn ensure_root(conn: &Connection, key: &LibNaClSecretKey, title: &str) -> Result<i64> {
    let pk = personal_channel_pk(key);
    if let Some(root) = onionbit_db::channel::personal_root(conn, &pk)? {
        return Ok(root.id_);
    }
    let id = fresh_node_id(conn, &pk)?;
    let pk64: [u8; 64] = pk
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::InvalidState("cle de canal invalide"))?;
    let mut entry = MetadataEntry::CollectionNode(CollectionNodePayload {
        node: ChannelNodePayload {
            header: SignedPayloadHeader::new(
                onionbit_format::mdblob::types::COLLECTION_NODE,
                0,
                pk64,
            ),
            id: id as u64,
            origin_id: id as u64,
            timestamp: now_unix(),
        },
        title: title.to_string(),
        tags: String::new(),
        num_entries: 0,
    });
    let blob = encode_entry(&entry, key)?;
    if let MetadataEntry::CollectionNode(c) = &mut entry {
        c.node.header = c.node.header.clone().with_signature(
            blob[blob.len() - SIGNATURE_LENGTH..]
                .try_into()
                .unwrap_or([0u8; 64]),
        );
    }
    let Some(row) = crate::ipv8_stack::entry_to_row(&entry) else {
        return Err(CoreError::InvalidState("racine de canal non serialisable"));
    };
    onionbit_db::channel::insert(conn, &row)?;
    Ok(id)
}

/// Cree ou renomme la racine du canal personnel (`PUT
/// /api/channels/personal`) : le renommage re-signe la racine
/// (nouvelle signature — la ligne est remplacee, `id_` conserve).
pub fn set_title(conn: &Connection, key: &LibNaClSecretKey, title: &str) -> Result<i64> {
    let pk = personal_channel_pk(key);
    let existing = onionbit_db::channel::personal_root(conn, &pk)?;
    if let Some(root) = &existing {
        onionbit_db::channel::delete_rowid(conn, root.rowid)?;
    }
    // La racine a ete supprimee : `ensure_root` recree un id_
    // — on force celui d'avant pour preserver les liens
    // `origin_id` des entrees.
    if let Some(root) = existing {
        let pk64: [u8; 64] = pk
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::InvalidState("cle de canal invalide"))?;
        let mut entry = MetadataEntry::CollectionNode(CollectionNodePayload {
            node: ChannelNodePayload {
                header: SignedPayloadHeader::new(
                    onionbit_format::mdblob::types::COLLECTION_NODE,
                    0,
                    pk64,
                ),
                id: root.id_ as u64,
                origin_id: root.id_ as u64,
                timestamp: now_unix(),
            },
            title: title.to_string(),
            tags: String::new(),
            num_entries: 0,
        });
        let blob = encode_entry(&entry, key)?;
        if let MetadataEntry::CollectionNode(c) = &mut entry {
            c.node.header = c.node.header.clone().with_signature(
                blob[blob.len() - SIGNATURE_LENGTH..]
                    .try_into()
                    .unwrap_or([0u8; 64]),
            );
        }
        let row = crate::ipv8_stack::entry_to_row(&entry)
            .ok_or_else(|| CoreError::InvalidState("racine non serialisable"))?;
        onionbit_db::channel::insert(conn, &row)?;
        Ok(root.id_)
    } else {
        ensure_root(conn, key, title)
    }
}

/// Ajoute un torrent au canal personnel (`commit` de
/// `channels_endpoint` Python) : cree la racine si besoin puis
/// insere un `CHANNEL_TORRENT` (400) signe par notre cle,
/// `origin_id` = racine. Les champs proviennent de la ligne
/// `channel_node` deja connue pour cet info-hash.
///
/// Retourne `Some(id_)` de l'entree creee ou deja presente,
/// `None` si l'info-hash est inconnu de la base.
pub fn commit(conn: &Connection, key: &LibNaClSecretKey, infohash: &[u8]) -> Result<Option<i64>> {
    let pk = personal_channel_pk(key);
    if let Some(existing) = onionbit_db::channel::channel_entry_by_infohash(conn, &pk, infohash)? {
        return Ok(Some(existing.id_));
    }
    let Some(src) = onionbit_db::channel::get_by_infohash(conn, infohash)? else {
        return Ok(None);
    };
    let root_id = ensure_root(conn, key, DEFAULT_CHANNEL_TITLE)?;
    let pk64: [u8; 64] = pk
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::InvalidState("cle de canal invalide"))?;
    let mut ih = [0u8; 20];
    if src.infohash.len() != 20 {
        return Ok(None);
    }
    ih.copy_from_slice(&src.infohash);
    let id = fresh_node_id(conn, &pk)?;
    let mut entry = MetadataEntry::ChannelTorrent(ChannelMetadataPayload {
        torrent: TorrentMetadataPayload {
            node: ChannelNodePayload {
                header: SignedPayloadHeader::new(
                    onionbit_format::mdblob::types::CHANNEL_TORRENT,
                    0,
                    pk64,
                ),
                id: id as u64,
                origin_id: root_id as u64,
                timestamp: now_unix(),
            },
            infohash: ih,
            size: src.size.max(0) as u64,
            torrent_date: src.torrent_date.max(0) as u32,
            title: src.title.clone(),
            tags: src.tags.clone(),
            tracker_info: src.tracker_info.clone(),
        },
        num_entries: 0,
        start_timestamp: now_unix(),
    });
    let blob = encode_entry(&entry, key)?;
    if let MetadataEntry::ChannelTorrent(c) = &mut entry {
        c.torrent.node.header = c.torrent.node.header.clone().with_signature(
            blob[blob.len() - SIGNATURE_LENGTH..]
                .try_into()
                .unwrap_or([0u8; 64]),
        );
    }
    let row = crate::ipv8_stack::entry_to_row(&entry)
        .ok_or_else(|| CoreError::InvalidState("entree de canal non serialisable"))?;
    onionbit_db::channel::insert(conn, &row)?;
    Ok(Some(id))
}

/// Retire un torrent du canal personnel : la ligne passe a
/// `metadata_type = 500` (pierre tombale conservee, re-signee a
/// la volee quand elle est servie — les abonnes apprennent le
/// retrait a la prochaine sync).
///
/// Le blob `DELETED` emis porte `delete_signature` = signature de
/// l'entree supprimee (colonne `signature` — cf.
/// `DeletedMetadataPayload.delete_signature` Python).
pub fn remove(conn: &Connection, key: &LibNaClSecretKey, infohash: &[u8]) -> Result<bool> {
    let pk = personal_channel_pk(key);
    let Some(entry) = onionbit_db::channel::channel_entry_by_infohash(conn, &pk, infohash)? else {
        return Ok(false);
    };
    let Some(sig) = entry.signature.as_deref() else {
        return Ok(false);
    };
    if sig.len() != SIGNATURE_LENGTH {
        return Ok(false);
    }
    onionbit_db::channel::mark_deleted_by_signature(conn, &pk, sig)?;
    Ok(true)
}

/// Titre par defaut de la racine creee implicitement au premier
/// `commit` (renommable via `set_title`).
const DEFAULT_CHANNEL_TITLE: &str = "OnionBit channel";

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_db::models::ChannelNodeRow;

    #[test]
    fn commit_puis_remove() {
        let db = onionbit_db::Database::memory().unwrap();
        db.with(|c| {
            let key = LibNaClSecretKey::generate();
            let pk = personal_channel_pk(&key);
            // Source : un torrent deja connu.
            let mut src = ChannelNodeRow {
                infohash: vec![7u8; 20],
                title: "mon torrent".into(),
                size: 42,
                torrent_date: 1_700_000_000,
                metadata_type: 300,
                public_key: vec![9u8; 64],
                id_: 1,
                signature: Some(vec![1u8; 64]),
                ..Default::default()
            };
            onionbit_db::channel::insert(c, &src)?;
            src.infohash = vec![8u8; 20];
            src.id_ = 2;
            src.signature = Some(vec![2u8; 64]);
            onionbit_db::channel::insert(c, &src)?;

            // Commit : racine 220 + entree 400 signees par notre cle.
            let id = commit(c, &key, &[7u8; 20]).unwrap().unwrap();
            let root = onionbit_db::channel::personal_root(c, &pk)?.unwrap();
            assert_eq!(root.metadata_type, 220);
            let e = onionbit_db::channel::channel_entry_by_infohash(c, &pk, &[7u8; 20])?.unwrap();
            assert_eq!(e.origin_id, root.id_);
            assert_eq!(e.metadata_type, 400);
            assert_eq!(e.id_, id);
            // Idempotent.
            assert_eq!(commit(c, &key, &[7u8; 20]).unwrap().unwrap(), id);
            // Info-hash inconnu : None.
            assert!(commit(c, &key, &[9u8; 20]).unwrap().is_none());

            // Remove : la ligne devient pierre tombale 500.
            assert!(remove(c, &key, &[7u8; 20]).unwrap());
            let e = onionbit_db::channel::channel_entry_by_infohash(c, &pk, &[7u8; 20])?;
            assert!(e.is_none());
            assert!(!remove(c, &key, &[7u8; 20]).unwrap());
            Ok(())
        })
        .unwrap();
    }
}
