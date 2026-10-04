// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Derivation du swarm de contact (ADR-0011 SS1) : un contact est
//! identifie par la cle publique LibNaCL de son demon ; le swarm de
//! messagerie derive de cette cle par SHA-1 domaine-separe.

use onionbit_crypto::hash::sha1;
use onionbit_crypto::ipv8::keys::LibNaClPublicKey;

/// Domaine de derivation du swarm de messagerie — disjoint de
/// `tribler anonymous download` (hidden services BitTorrent) pour
/// qu'un swarm messagerie ne puisse jamais entrer en collision avec
/// un swarm de telechargement.
const MESSAGING_DOMAIN: &[u8] = b"onionbit messaging";

/// `messaging_hash(pk) = SHA1("onionbit messaging" || pk_bin)` : le
/// destinataire `join_swarm` ce hash (`IP_SEEDER` + annonce DHT) ;
/// l'expediteur le resout par `peers-request`/DHT puis lie un
/// circuit e2e — le meme flux qu'un telechargement anonyme.
pub fn messaging_hash(pk: &LibNaClPublicKey) -> [u8; 20] {
    let pk_bin = pk.to_bin();
    let mut data = Vec::with_capacity(MESSAGING_DOMAIN.len() + pk_bin.len());
    data.extend_from_slice(MESSAGING_DOMAIN);
    data.extend_from_slice(&pk_bin);
    sha1(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;

    /// La derivation est stable et distincte pour deux cles.
    #[test]
    fn messaging_hash_stable_et_distinct() {
        let sk1 = LibNaClSecretKey::generate();
        let sk2 = LibNaClSecretKey::generate();
        let h1 = messaging_hash(&sk1.public_key());
        assert_eq!(h1, messaging_hash(&sk1.public_key()));
        assert_ne!(h1, messaging_hash(&sk2.public_key()));
    }

    /// Vecteur interne : `SHA1("onionbit messaging" || pk.to_bin())`
    /// telle qu'ecrite — le format ne peut pas bouger
    /// silencieusement (compat inter-demons OnionBit).
    #[test]
    fn messaging_hash_vecteur() {
        let sk = LibNaClSecretKey::generate();
        let pk = sk.public_key();
        let mut expect = Vec::new();
        expect.extend_from_slice(b"onionbit messaging");
        expect.extend_from_slice(&pk.to_bin());
        assert_eq!(messaging_hash(&pk), sha1(&expect));
    }
}
