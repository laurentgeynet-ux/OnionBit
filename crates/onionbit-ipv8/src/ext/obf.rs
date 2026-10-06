// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Enveloppe opaque `OBF` — obfuscation de transport negociee
//! (ADR-0015 §7, Phase 9e).
//!
//! Principe : un message ext (`ATTEST`, `LEDGER_*`) destine a un pair
//! ayant annonce `CAP_OBF_V1` dans son `hello` est glisse dans une
//! enveloppe `{v, blob}` ou `blob` = `pair_seal_in` de la paire
//! (X25519 des deux cles -> ChaCha20-Poly1305, domaine HKDF dedie).
//! L'observateur ne voit qu'un datagramme ext opaque ; le `msg_id`
//! interne et le padding restent dans le plaintext AEAD — ni le type
//! ni la taille reelle du message ne sont lisibles.
//!
//! Bornes : seuls les messages **post-hello** sont enveloppes (le
//! `hello` negocie la capacite — il ne peut pas s'envelopper lui-
//! meme). Le padding par classes de taille normalise la longueur du
//! datagramme : `attest` (~250 B) et `hello` (9 B) tombent dans la
//! meme classe. Le prefixe `community_id` reste visible — l'activite
//! ext est une metadonnee assumee (ADR §Consequences) ; OBF masque
//! le *contenu* (type, taille, donnees), pas l'existence.
//!
//! Jamais avec un pair legacy : un pair qui n'a pas annonce le bit
//! `CAP_OBF_V1` recoit les trames en clair comme avant — un pair
//! Tribler ne recoit de toute facon rien (il n'est pas dans
//! `ext_peers`).

use onionbit_crypto::ipv8::dh::{pair_open_in, pair_seal_in};
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use rand::Rng;

use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};

/// Domaine HKDF de l'enveloppe OBF — separe de `pairbox` (le `tx`
/// chiffre du ledger) : un blob OBF ne peut pas etre re-soumis
/// comme `tx_enc` et inversement.
pub const OBF_HKDF_INFO: &[u8] = b"onionbit/ext-obf/v1";

/// Classe de padding par defaut (octets) : le plaintext AEAD est
/// complete d'octets aleatoires jusqu'au multiple superieur —
/// `hello` (9 B) et `attest` (~250 B) produisent des datagrammes de
/// taille identique.
pub const OBF_PAD_BUCKET: usize = 256;

/// Taille max du payload interne accepte a l'ouverture — borne le
/// travail d'un blob OBF valide dont l'inner serait gonfle. Les
/// messages ext reels tiennent dans quelques centaines d'octets.
pub const OBF_INNER_MAX: usize = 64 * 1024;

/// Bit de capacite `hello.caps` : le pair sait ouvrir les trames
/// `OBF`. Reserve bit 0 du bitmap extensible.
pub const CAP_OBF_V1: u64 = 1 << 0;

/// Version de l'enveloppe OBF.
const OBF_VERSION: u8 = 1;

/// Enveloppe `{inner_msg_id || len16 || payload || pad}` chiffree
/// pour la paire et serailisee `{v, blob}` — valeur `payload` d'un
/// `msg::OBF`. `bucket` = classe de padding (multiple).
///
/// Erreur `Ipv8Error::Malformed` si `payload` depasse
/// `OBF_INNER_MAX` (les messages ext n'ont pas vocation a etre gros).
pub fn seal(
    peer_pk: &LibNaClPublicKey,
    my_key: &LibNaClSecretKey,
    inner_msg_id: u8,
    payload: &[u8],
    bucket: usize,
) -> Result<Vec<u8>, Ipv8Error> {
    if payload.len() > OBF_INNER_MAX || payload.len() > u16::MAX as usize {
        return Err(Ipv8Error::Malformed("obf payload trop grand"));
    }
    // inner = msg_id(1) || len(2) || payload || pad — arrondi au
    // multiple de `bucket` superieur (jamais 0 : 3 octets d'en-tete).
    let bucket = bucket.max(16);
    let inner_len = 3 + payload.len();
    let padded = inner_len.div_ceil(bucket) * bucket;
    let mut inner = Vec::with_capacity(padded);
    inner.push(inner_msg_id);
    inner.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    inner.extend_from_slice(payload);
    inner.resize(padded, 0u8);
    rand::rng().fill_bytes(&mut inner[inner_len..]);

    let blob = pair_seal_in(
        &peer_pk.crypt_pk,
        &my_key.crypt_x25519().to_bytes(),
        OBF_HKDF_INFO,
        &inner,
    )
    .map_err(Ipv8Error::Crypto)?;

    let mut w = Writer::new();
    w.u8(OBF_VERSION);
    w.varlen_h(&blob);
    Ok(w.into_bytes())
}

/// Ouvre une enveloppe `{v, blob}` : retourne `(inner_msg_id,
/// payload)` si le blob se dechiffre pour la paire. Erreur sur
/// version inconnue, blob tronque, AEAD invalide ou inner malforme
/// — le receveur droppe silencieusement (`dropped`++).
pub fn open(
    peer_pk: &LibNaClPublicKey,
    my_key: &LibNaClSecretKey,
    payload: &[u8],
) -> Result<(u8, Vec<u8>), Ipv8Error> {
    let mut r = Reader::new(payload);
    let v = r.u8()?;
    if v != OBF_VERSION {
        return Err(Ipv8Error::Malformed("obf version inconnue"));
    }
    let blob = r.varlen_h()?;
    let inner = pair_open_in(
        &peer_pk.crypt_pk,
        &my_key.crypt_x25519().to_bytes(),
        OBF_HKDF_INFO,
        blob,
    )
    .map_err(Ipv8Error::Crypto)?;
    if inner.len() < 3 {
        return Err(Ipv8Error::Malformed("obf inner borne"));
    }
    let mut ir = Reader::new(&inner);
    let inner_msg_id = ir.u8()?;
    let len = ir.u16()? as usize;
    let body = ir.take(len)?.to_vec();
    Ok((inner_msg_id, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;

    #[test]
    fn seal_open_allez_retour_et_padding() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        for payload in [b"x".to_vec(), vec![0u8; 250], vec![7u8; 500]] {
            let env = seal(&b.public_key(), &a, 42, &payload, OBF_PAD_BUCKET).unwrap();
            let (msg_id, back) = open(&a.public_key(), &b, &env).unwrap();
            assert_eq!(msg_id, 42);
            assert_eq!(back, payload);
        }
        // Classes de taille : deux payloads d'une meme classe
        // produisent des enveloppes de taille identique.
        let e1 = seal(&b.public_key(), &a, 1, b"petit", OBF_PAD_BUCKET).unwrap();
        let e2 = seal(&b.public_key(), &a, 2, &[9u8; 240], OBF_PAD_BUCKET).unwrap();
        assert_eq!(e1.len(), e2.len());
    }

    #[test]
    fn open_rejette_mauvaise_cle_tronque_et_version() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let c = LibNaClSecretKey::generate();
        let env = seal(&b.public_key(), &a, 1, b"secret", OBF_PAD_BUCKET).unwrap();
        // Mauvaise paire : AEAD refuse.
        assert!(open(&c.public_key(), &b, &env).is_err());
        assert!(open(&b.public_key(), &c, &env).is_err());
        // Tronque.
        assert!(open(&a.public_key(), &b, &env[..env.len() - 4]).is_err());
        assert!(open(&a.public_key(), &b, &env[..3]).is_err());
        // Version inconnue.
        let mut bad = env.clone();
        bad[0] = 99;
        assert!(open(&a.public_key(), &b, &bad).is_err());
        // Enveloppe trop courte.
        assert!(open(&a.public_key(), &b, &[1]).is_err());
    }

    #[test]
    fn seal_rejette_payload_geant() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        assert!(seal(&b.public_key(), &a, 1, &[0u8; 70_000], OBF_PAD_BUCKET).is_err());
    }
}
