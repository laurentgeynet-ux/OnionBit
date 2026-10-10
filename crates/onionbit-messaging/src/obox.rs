// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Trame offline de boite aux lettres (ADR-0026, `MAILBOX_PUT` /
//! `MAILBOX_PULL`) : un [`Frame`] **identique au filaire** — le corps
//! est chiffre AEAD + la forme canonique signee Ed25519 — dont la
//! `send_key` est derivee du DH **statique de paire**
//! (`crypt_pk` du destinataire x `crypt_sk` de l'emetteur) au lieu du
//! secret de circuit e2e. Le pont relais qui heberge la boite ne
//! voit que des octets opaques ; l'authentification est la signature
//! de trame existante.
//!
//! Blob = `{f: <trame_filiaire>, p: <pk_expediteur>}` (dictionnaire
//! bencode, ensemble de cles strict comme [`crate::frame`]). La cle
//! publique voyage hors du chiffrement : le destinataire en a
//! besoin pour deriver la cle de lecture (DH symetrique). Le pont
//! apprend deja la cle signataire de la trame `ENCAP` de depot —
//! aucune fuite supplementaire.
//!
//! Seule la trame `msg` a un sens hors ligne (le `hello` de
//! liaison et les `ack` supposent un circuit vivant) ; [`seal`]
//! refuse tout autre kind.

use std::collections::BTreeMap;

use onionbit_crypto::ipv8::dh::crypto_box_beforenm;
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use onionbit_format::bencode::{decode, BValue};

use crate::config::MessagingConfig;
use crate::error::MessagingError;
use crate::frame::{Frame, MsgKind};
use crate::keys::derive_messaging_keys;

/// Scelle `frame` (kind [`MsgKind::Msg`]) pour `recipient` :
/// cle statique de paire -> [`derive_messaging_keys`] role
/// initiateur -> [`Frame::seal`] -> blob `{varlen pk, wire}`.
pub fn seal(
    frame: &Frame,
    sk: &LibNaClSecretKey,
    recipient: &LibNaClPublicKey,
    cfg: &MessagingConfig,
) -> Result<Vec<u8>, MessagingError> {
    if frame.kind != MsgKind::Msg {
        return Err(MessagingError::Malformed(
            "obox : seule la trame msg voyage hors ligne",
        ));
    }
    let shared = crypto_box_beforenm(&recipient.crypt_pk, &sk.crypt_x25519().to_bytes())?;
    let keys = derive_messaging_keys(&shared, true)?;
    let wire = frame.seal(sk, &keys.send, cfg)?;
    let mut d = BTreeMap::new();
    d.insert(b"f".to_vec(), BValue::Bytes(wire));
    d.insert(b"p".to_vec(), BValue::Bytes(sk.public_key().to_bin()));
    Ok(BValue::Dict(d).encode())
}

/// Ouvre un blob offline : parse `{f: wire, p: pk}` (ensemble de
/// cles strict), derive la cle de lecture (DH de paire, role
/// repondant) puis [`Frame::open`] — verification de signature +
/// AEAD identiques a une trame de circuit. Retourne
/// `(pk_expediteur, trame)`.
pub fn open(
    blob: &[u8],
    own_sk: &LibNaClSecretKey,
    cfg: &MessagingConfig,
) -> Result<(LibNaClPublicKey, Frame), MessagingError> {
    let value = decode(blob).map_err(|_| MessagingError::Malformed("obox : bencode"))?;
    let dict = value.as_dict().ok_or(MessagingError::Malformed(
        "obox : n'est pas un dictionnaire",
    ))?;
    if dict.len() != 2 {
        return Err(MessagingError::Malformed("obox : cles"));
    }
    let pk_bin = dict
        .get(b"p".as_ref())
        .and_then(BValue::as_bytes)
        .ok_or(MessagingError::Malformed("obox : pk absente"))?;
    let wire = dict
        .get(b"f".as_ref())
        .and_then(BValue::as_bytes)
        .ok_or(MessagingError::Malformed("obox : trame absente"))?;
    let sender = LibNaClPublicKey::from_bin(pk_bin)
        .map_err(|_| MessagingError::Malformed("obox : pk invalide"))?;
    let shared = crypto_box_beforenm(&sender.crypt_pk, &own_sk.crypt_x25519().to_bytes())?;
    let keys = derive_messaging_keys(&shared, false)?;
    let frame = Frame::open(wire, &sender, &keys.recv, cfg)?;
    Ok((sender, frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roundtrip : le destinataire dechiffre et verifie la trame
    /// via le pipeline standard (`Frame::open`).
    #[test]
    fn obox_roundtrip() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let cfg = MessagingConfig::default();
        let f = Frame::new(MsgKind::Msg, 7, 1_700_000_000, b"coucou".to_vec());
        let blob = seal(&f, &a, &b.public_key(), &cfg).unwrap();
        let (sender, got) = open(&blob, &b, &cfg).unwrap();
        assert_eq!(sender, a.public_key());
        assert_eq!(got.body, b"coucou");
        assert_eq!(got.id, f.id);
    }

    /// Une boite scellee pour B ne s'ouvre pas pour C (AEAD).
    #[test]
    fn obox_mauvais_destinataire() {
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let c = LibNaClSecretKey::generate();
        let cfg = MessagingConfig::default();
        let f = Frame::new(MsgKind::Msg, 1, 1, b"secret".to_vec());
        let blob = seal(&f, &a, &b.public_key(), &cfg).unwrap();
        assert!(open(&blob, &c, &cfg).is_err());
    }

    /// Un blob tronque/forge est refuse au parse ou a la
    /// verification — jamais de panique.
    #[test]
    fn obox_blobs_hostiles() {
        let cfg = MessagingConfig::default();
        let b = LibNaClSecretKey::generate();
        assert!(open(b"", &b, &cfg).is_err());
        assert!(open(&[0xff; 8], &b, &cfg).is_err());
        let a = LibNaClSecretKey::generate();
        let f = Frame::new(MsgKind::Msg, 1, 1, b"x".to_vec());
        let mut blob = seal(&f, &a, &b.public_key(), &cfg).unwrap();
        blob.truncate(blob.len() - 3);
        assert!(open(&blob, &b, &cfg).is_err());
    }
}
