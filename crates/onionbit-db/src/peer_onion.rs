// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Table `peer_onions` (ADR-0029, etape 123) : adresse `.onion`
//! associee **explicitement** a une cle publique de pair
//! (contact messagerie ou cle de canal — meme espace d'identite
//! LibNaCl). La liaison n'est jamais inferee : posee par
//! l'utilisateur, ou auto-portante quand `bind_sig` est la
//! signature `onionbit/onion-bind/v1` verifiee sous la cle du pair
//! (`sig_checked = 1` — prouvee cryptographiquement).
//!
//! Jamais de traduction inter-plans : la ligne attache les deux
//! adresses d'une meme identite, elle ne convertit rien.

use rusqlite::{params, Connection, OptionalExtension};

use crate::Result;

/// Prefixe filaire `key_to_bin()` pyipv8 — duplique a dessein :
/// `onionbit-db` ne depend pas de `onionbit-crypto` (sens des
/// dependances). La forme stockee est `pk64` = `pk_bin[10..]`
/// (crypt_pk ‖ vk), la meme que `channel_node.public_key` : un
/// contact (pk_bin 74 o) et le canal de son createur (64 o)
/// partagent ainsi **une seule** ligne — meme identite, meme
/// service `.onion`.
const LIBNACL_PK_PREFIX: &[u8] = b"LibNaCLPK:";

/// Normalise une cle de pair vers sa forme canonique 64 octets :
/// accepte `pk_bin` (74 o prefixes `LibNaCLPK:`) ou `pk64` nu.
/// `None` sur toute autre forme.
pub fn key64(pk: &[u8]) -> Option<Vec<u8>> {
    if pk.len() == 64 {
        return Some(pk.to_vec());
    }
    pk.strip_prefix(LIBNACL_PK_PREFIX)
        .filter(|rest| rest.len() == 64)
        .map(|rest| rest.to_vec())
}

/// Ligne de `peer_onions` (horodatages en secondes Unix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerOnionRow {
    /// Cle canonique `pk64` du pair (cle primaire — une adresse
    /// `.onion` par identite, c'est le service d'identite unique).
    pub public_key: Vec<u8>,
    /// Hote `.onion` v3 normalise (`"<56 chars>.onion"`).
    pub onion_host: String,
    /// Signature `onionbit/onion-bind/v1` si fournie (64 octets).
    pub bind_sig: Option<Vec<u8>>,
    /// `true` = `bind_sig` verifiee sous `public_key` a l'insertion
    /// (liaison prouvee par le pair lui-meme) ; `false` =
    /// association manuelle de l'utilisateur.
    pub sig_checked: bool,
    /// Creation de la ligne.
    pub created_at: i64,
    /// Derniere modification.
    pub updated_at: i64,
}

/// Insere ou remplace l'adresse `.onion` d'un pair — `public_key`
/// est normalisee en `pk64` ([`key64`]) quel que soit l'appelant.
/// `Err(DbError::Corrupt)` si la cle n'est pas une forme connue.
pub fn set_peer_onion(conn: &Connection, row: &PeerOnionRow) -> Result<()> {
    let public_key = key64(&row.public_key)
        .ok_or_else(|| crate::DbError::Corrupt("peer_onions : cle de pair inconnue".into()))?;
    conn.execute(
        "INSERT INTO peer_onions
             (public_key, onion_host, bind_sig, sig_checked,
              created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(public_key) DO UPDATE SET
             onion_host=excluded.onion_host,
             bind_sig=excluded.bind_sig,
             sig_checked=excluded.sig_checked,
             updated_at=excluded.updated_at",
        params![
            public_key,
            row.onion_host,
            row.bind_sig,
            row.sig_checked as i64,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

/// Adresse `.onion` d'un pair (`None` = aucune association ou cle
/// de forme inconnue — [`key64`] normalise `pk_bin` et `pk64`).
pub fn get_peer_onion(conn: &Connection, pk: &[u8]) -> Result<Option<PeerOnionRow>> {
    let Some(pk64) = key64(pk) else {
        return Ok(None);
    };
    conn.query_row(
        "SELECT public_key, onion_host, bind_sig, sig_checked,
                created_at, updated_at
         FROM peer_onions WHERE public_key=?1",
        params![pk64],
        row_to_onion,
    )
    .optional()
    .map_err(Into::into)
}

/// Pair associe a un hote `.onion` (lookup inverse — un service
/// entrant s'identifie par son adresse).
pub fn get_by_host(conn: &Connection, host: &str) -> Result<Option<PeerOnionRow>> {
    conn.query_row(
        "SELECT public_key, onion_host, bind_sig, sig_checked,
                created_at, updated_at
         FROM peer_onions WHERE onion_host=?1",
        params![host],
        row_to_onion,
    )
    .optional()
    .map_err(Into::into)
}

/// Toutes les associations — au demarrage elles re-alimentent
/// l'allowlist etendue `TorTargetPolicy` du runtime Tor courant.
pub fn list_peer_onions(conn: &Connection) -> Result<Vec<PeerOnionRow>> {
    let mut stmt = conn.prepare(
        "SELECT public_key, onion_host, bind_sig, sig_checked,
                created_at, updated_at
         FROM peer_onions ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], row_to_onion)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Retire l'association d'un pair (`true` si une ligne existait —
/// [`key64`] normalise `pk_bin` et `pk64`).
pub fn remove_peer_onion(conn: &Connection, pk: &[u8]) -> Result<bool> {
    let Some(pk64) = key64(pk) else {
        return Ok(false);
    };
    let n = conn.execute("DELETE FROM peer_onions WHERE public_key=?1", params![pk64])?;
    Ok(n > 0)
}

fn row_to_onion(r: &rusqlite::Row) -> rusqlite::Result<PeerOnionRow> {
    Ok(PeerOnionRow {
        public_key: r.get(0)?,
        onion_host: r.get(1)?,
        bind_sig: r.get(2)?,
        sig_checked: r.get::<_, i64>(3)? != 0,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONION: &str = "duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion";

    fn row(pk: u8) -> PeerOnionRow {
        PeerOnionRow {
            public_key: vec![pk; 64],
            onion_host: ONION.into(),
            bind_sig: None,
            sig_checked: false,
            created_at: 100,
            updated_at: 100,
        }
    }

    /// Cycle association : pose, lecture, remplacement, lookup
    /// inverse, retrait.
    #[test]
    fn cycle_adresse_onion_pair() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let pk = vec![7u8; 64];
            let r = row(7);
            set_peer_onion(c, &r)?;
            let got = get_peer_onion(c, &pk)?.unwrap();
            assert_eq!(got.onion_host, ONION);
            assert!(!got.sig_checked);
            // Lookup inverse + liste complete.
            assert_eq!(get_by_host(c, ONION)?.unwrap().public_key, pk);
            assert_eq!(list_peer_onions(c)?.len(), 1);
            // `pk_bin` (74 o prefixes) designe la meme ligne que
            // `pk64` — contact et canal partagent l'association.
            let pk_bin = [b"LibNaCLPK:".as_slice(), pk.as_slice()].concat();
            assert_eq!(
                get_peer_onion(c, &pk_bin)?.unwrap().public_key,
                pk,
                "pk_bin et pk64 designent la meme identite"
            );
            // Remplacement : signature verifiee posee a l'update.
            let mut r2 = r.clone();
            r2.bind_sig = Some(vec![9u8; 64]);
            r2.sig_checked = true;
            set_peer_onion(c, &r2)?;
            let got = get_peer_onion(c, &pk)?.unwrap();
            assert!(got.sig_checked);
            assert_eq!(got.bind_sig, Some(vec![9u8; 64]));
            // Retrait.
            assert!(remove_peer_onion(c, &pk)?);
            assert!(get_peer_onion(c, &pk)?.is_none());
            assert!(!remove_peer_onion(c, &pk)?);
            Ok(())
        })
        .unwrap();
    }
}
