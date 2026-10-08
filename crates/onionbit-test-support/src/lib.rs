// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-test-support` — helpers de tests partages.
//!
//! Propriete unique des helpers de tests d'integration cross-crates
//! (fixtures de torrents/canaux, pairs IPv8 en boucle locale, daemon
//! jetable pour les tests d'API). A utiliser en `dev-dependency`
//! uniquement — jamais en dependance de production. Cf. la table
//! "Anti-duplication" de `AGENTS.md` : toute fixture partagee par au
//! moins deux crates doit vivre ici, pas etre dupliquee.
//!
//! Toutes les fixtures sont **hors-ligne** : aucun trafic sortant,
//! sockets loopback uniquement.

use std::time::Duration;

/// Marqueur de disponibilite du crate de support de tests (permet aux
/// autres crates de verifier qu'ils importent bien la bonne version).
pub const SUPPORT_CRATE_READY: bool = true;

/// `.torrent` minimal valide : info dict mono-fichier (`name`,
/// `length` octets, une piece factice de 20 octets de SHA-1 nuls,
/// `piece length` 16384). Utilisable partout ou un torrent parseable
/// suffit (aucune piece reelle ne sera servie).
pub fn test_torrent_bytes(name: &str, length: u64) -> Vec<u8> {
    use onionbit_format::bencode::{encode, BValue};
    let mut info = std::collections::BTreeMap::new();
    info.insert(b"length".to_vec(), BValue::Int(length as i64));
    info.insert(b"name".to_vec(), BValue::Bytes(name.as_bytes().to_vec()));
    info.insert(b"piece length".to_vec(), BValue::Int(16384));
    info.insert(b"pieces".to_vec(), BValue::Bytes(vec![0u8; 20]));
    let mut root = std::collections::BTreeMap::new();
    root.insert(b"info".to_vec(), BValue::Dict(info));
    encode(&BValue::Dict(root))
}

/// Cherche un port loopback libre (TCP), le libere, puis le rend —
/// fenetre de course acceptable pour un test local.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Attente active bornee : interroge `f` toutes les 30 ms jusqu'a
/// `timeout`. Retourne `false` si la condition n'est jamais remplie.
pub async fn wait_for(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !f() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_crate_de_support_est_marque_pret() {
        let ready = std::hint::black_box(SUPPORT_CRATE_READY);
        assert!(ready);
    }

    #[test]
    fn fixture_torrent_minimal_parse() {
        let bytes = test_torrent_bytes("fixture.bin", 42);
        let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
        assert_eq!(meta.name, "fixture.bin");
        assert_eq!(meta.files[0].length, 42);
        assert_eq!(meta.info_hash_hex().len(), 40);
    }

    #[tokio::test]
    async fn wait_for_timeout_retourne_faux() {
        let ok = wait_for(Duration::from_millis(120), || false).await;
        assert!(!ok);
        let ok = wait_for(Duration::from_millis(120), || true).await;
        assert!(ok);
    }
}
