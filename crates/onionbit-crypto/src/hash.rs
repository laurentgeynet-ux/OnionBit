// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hachage BitTorrent : SHA-1 (v1, BEP 3) et SHA-256 (v2, BEP 52).

use sha1::Digest as Sha1Digest;
use sha2::Digest as Sha2Digest;

/// Info-hash BitTorrent v1 (SHA-1, 20 octets).
pub type InfoHashV1 = [u8; 20];

/// Info-hash BitTorrent v2 (SHA-256, 32 octets, racine Merkle).
pub type InfoHashV2 = [u8; 32];

/// Calcule le hachage SHA-1 de `data`.
pub fn sha1(data: &[u8]) -> InfoHashV1 {
    let mut hasher = sha1::Sha1::new();
    Sha1Digest::update(&mut hasher, data);
    Sha1Digest::finalize(hasher).into()
}

/// Calcule le hachage SHA-256 de `data`.
pub fn sha256(data: &[u8]) -> InfoHashV2 {
    let mut hasher = sha2::Sha256::new();
    Sha2Digest::update(&mut hasher, data);
    Sha2Digest::finalize(hasher).into()
}

/// "Member ID" IPv8 : SHA-1 de la cle publique binaire.
///
/// Equivalent de `Peer.mid` cote pyipv8 (`key_to_hash()` = SHA-1 de
/// `key_to_bin()`).
pub fn ipv8_mid(public_key_bin: &[u8]) -> [u8; 20] {
    sha1(public_key_bin)
}

/// Formate un hachage en hexadecimal minuscule (pour les logs/API).
pub fn to_hex(data: &[u8]) -> String {
    hex::encode(data)
}

/// Parse un hachage depuis de l'hexadecimal minuscule/majuscule.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    hex::decode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_abc_vecteur_connu() {
        // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        let h = sha1(b"abc");
        assert_eq!(to_hex(&h), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha256_abc_vecteur_connu() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let h = sha256(b"abc");
        assert_eq!(
            to_hex(&h),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn ipv8_mid_est_sha1_de_la_cle() {
        let key = b"LibNaCLPK:fakefakefakefakefakefakefakefa";
        assert_eq!(ipv8_mid(key), sha1(key));
    }
}
