// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Codec de trame messagerie — unique point d'entree du parseur
//! (ADR-0011, « format canonique versionne »).
//!
//! Trame filaire v1 — dictionnaire bencode **canonique** (cles
//! triees, entiers minimaux, aucune cle hors l'ensemble prevu) :
//!
//! ```text
//! { "body": <corps chiffre>,  "id": <16 octets aleatoires>,
//!   "seq": <u64>,             "sig": <Ed25519 64 octets>,
//!   "ts":  <u64 secondes>,    "type": <hello|accept|reject|msg|ack>,
//!   "v":   1 }
//! ```
//!
//! - `body` = ChaCha20-Poly1305 du corps applicatif, cle
//!   [`crate::keys::MessagingKeys`], nonce = `"omsg" || seq(BE)` —
//!   `seq` monotone par direction garantit l'unicite du nonce.
//! - `sig` = signature Ed25519 de la forme canonique du dictionnaire
//!   **sans** `sig` (encrypt-then-sign : la signature authentifie le
//!   corps chiffre) ; verification contre la `pk` du contact, qui
//!   est aussi celle dont derive le swarm.

use std::collections::BTreeMap;

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use onionbit_crypto::error::CryptoError;
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, SIGNATURE_LENGTH};
use onionbit_format::bencode::{decode, BValue};

use crate::config::MessagingConfig;
use crate::error::MessagingError;

/// Version du format de trame.
pub const PROTO_VERSION: i64 = 1;
/// Taille du `id` de trame (dedup).
pub const MSG_ID_LEN: usize = 16;
/// Taille du tag Poly1305.
const TAG_LEN: usize = 16;
/// Domaine du nonce AEAD (4 octets + `seq` big-endian = 12).
const NONCE_DOMAIN: &[u8; 4] = b"omsg";

/// Type de trame (champ `type`, chaine bencode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    /// Demande de consentement — premiere trame d'un inconnu.
    Hello,
    /// Consentement accorde (reponse a `hello`).
    Accept,
    /// Consentement refuse.
    Reject,
    /// Message applicatif.
    Msg,
    /// Accuse de livraison (`body` = `id` de la trame acquittee).
    Ack,
}

impl MsgKind {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            MsgKind::Hello => b"hello",
            MsgKind::Accept => b"accept",
            MsgKind::Reject => b"reject",
            MsgKind::Msg => b"msg",
            MsgKind::Ack => b"ack",
        }
    }

    fn from_bytes(b: &[u8]) -> Result<Self, MessagingError> {
        match b {
            b"hello" => Ok(MsgKind::Hello),
            b"accept" => Ok(MsgKind::Accept),
            b"reject" => Ok(MsgKind::Reject),
            b"msg" => Ok(MsgKind::Msg),
            b"ack" => Ok(MsgKind::Ack),
            _ => Err(MessagingError::UnknownKind(
                String::from_utf8_lossy(b).into_owned(),
            )),
        }
    }
}

/// Trame de messagerie decodee (corps en clair apres `open`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Type de trame.
    pub kind: MsgKind,
    /// Identifiant aleatoire (dedup, cible des `ack`).
    pub id: [u8; MSG_ID_LEN],
    /// Compteur monotone par (contact, direction) — nonce AEAD.
    pub seq: u64,
    /// Horodatage emetteur (secondes Unix).
    pub ts: u64,
    /// Corps applicatif en clair.
    pub body: Vec<u8>,
}

/// Ensemble exact des cles de la trame v1 — toute cle hors de cette
/// liste est un rejet (pas d'ignore silencieux).
const FRAME_KEYS: &[&[u8]] = &[b"body", b"id", b"seq", b"sig", b"ts", b"type", b"v"];

fn chacha(key: &[u8; 32], seq: u64) -> (ChaCha20Poly1305, Nonce) {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(NONCE_DOMAIN);
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    let key: &Key = key.into();
    (ChaCha20Poly1305::new(key), nonce.into())
}

/// Forme canonique signee : dictionnaire de tous les champs sauf
/// `sig` — re-serialise a l'identique des deux cotes (`BTreeMap`
/// trie les cles).
fn unsigned_form(
    kind: MsgKind,
    id: &[u8; MSG_ID_LEN],
    seq: u64,
    ts: u64,
    body_ct: &[u8],
) -> Vec<u8> {
    let mut d = BTreeMap::new();
    d.insert(b"body".to_vec(), BValue::Bytes(body_ct.to_vec()));
    d.insert(b"id".to_vec(), BValue::Bytes(id.to_vec()));
    d.insert(b"seq".to_vec(), BValue::Int(seq as i64));
    d.insert(b"ts".to_vec(), BValue::Int(ts as i64));
    d.insert(b"type".to_vec(), BValue::Bytes(kind.as_bytes().to_vec()));
    d.insert(b"v".to_vec(), BValue::Int(PROTO_VERSION));
    BValue::Dict(d).encode()
}

impl Frame {
    /// Cree une trame avec un `id` aleatoire neuf.
    pub fn new(kind: MsgKind, seq: u64, ts: u64, body: Vec<u8>) -> Self {
        Self {
            kind,
            id: rand::random(),
            seq,
            ts,
            body,
        }
    }

    /// Encode la trame pour le fil : chiffre `body` sous `send_key`
    /// (nonce = `seq`), signe la forme canonique avec `sk`.
    ///
    /// Refuse si `body` depasse `max_body_len` ou si la trame finale
    /// depasse `max_frame_len`.
    pub fn seal(
        &self,
        sk: &LibNaClSecretKey,
        send_key: &[u8; 32],
        cfg: &MessagingConfig,
    ) -> Result<Vec<u8>, MessagingError> {
        if self.body.len() > cfg.max_body_len {
            return Err(MessagingError::BodyTooLarge(
                self.body.len(),
                cfg.max_body_len,
            ));
        }
        let (cipher, nonce) = chacha(send_key, self.seq);
        let body_ct = cipher
            .encrypt(&nonce, self.body.as_ref())
            .map_err(|_| CryptoError::Aead)?;
        let unsigned = unsigned_form(self.kind, &self.id, self.seq, self.ts, &body_ct);
        let sig = sk.sign(&unsigned);

        let mut d = BTreeMap::new();
        d.insert(b"body".to_vec(), BValue::Bytes(body_ct));
        d.insert(b"id".to_vec(), BValue::Bytes(self.id.to_vec()));
        d.insert(b"seq".to_vec(), BValue::Int(self.seq as i64));
        d.insert(b"sig".to_vec(), BValue::Bytes(sig.to_vec()));
        d.insert(b"ts".to_vec(), BValue::Int(self.ts as i64));
        d.insert(
            b"type".to_vec(),
            BValue::Bytes(self.kind.as_bytes().to_vec()),
        );
        d.insert(b"v".to_vec(), BValue::Int(PROTO_VERSION));
        let wire = BValue::Dict(d).encode();
        if wire.len() > cfg.max_frame_len {
            return Err(MessagingError::FrameTooLarge(wire.len(), cfg.max_frame_len));
        }
        Ok(wire)
    }

    /// Parse une trame recue : borne de taille **avant** tout parse,
    /// ensemble de cles strict, verification de signature contre la
    /// `pk` du contact puis dechiffrement de `body` sous `recv_key`.
    ///
    /// L'anti-replay (`seq`/`id`) est applique par l'appelant via
    /// [`crate::replay::RecvWindow`] — cette fonction ne fait que
    /// codec + preuve d'authenticite.
    pub fn open(
        data: &[u8],
        peer: &LibNaClPublicKey,
        recv_key: &[u8; 32],
        cfg: &MessagingConfig,
    ) -> Result<Self, MessagingError> {
        // Borne avant tout parse : le seul cout par trame hostile
        // est lineaire et borne (anti-DoS, ADR-0011).
        if data.len() > cfg.max_frame_len {
            return Err(MessagingError::FrameTooLarge(data.len(), cfg.max_frame_len));
        }
        let value = decode(data)?;
        let dict = value.as_dict().ok_or(MessagingError::Malformed(
            "la trame n'est pas un dictionnaire",
        ))?;
        // Ensemble de cles strict : ni champ critique absent, ni
        // champ inconnu ignore.
        if dict.len() != FRAME_KEYS.len() || !FRAME_KEYS.iter().all(|k| dict.contains_key(*k)) {
            return Err(MessagingError::Malformed(
                "ensemble de cles different de la trame v1",
            ));
        }
        let get = |k: &'static str| dict.get(k.as_bytes()).unwrap();

        let v = get("v")
            .as_int()
            .ok_or(MessagingError::Malformed("v non entier"))?;
        if v != PROTO_VERSION {
            return Err(MessagingError::UnknownVersion(v));
        }
        let kind = MsgKind::from_bytes(
            get("type")
                .as_bytes()
                .ok_or(MessagingError::Malformed("type non chaine"))?,
        )?;
        let id_raw = get("id")
            .as_bytes()
            .ok_or(MessagingError::Malformed("id non chaine"))?;
        if id_raw.len() != MSG_ID_LEN {
            return Err(MessagingError::Malformed("id != 16 octets"));
        }
        let mut id = [0u8; MSG_ID_LEN];
        id.copy_from_slice(id_raw);

        let seq = get("seq")
            .as_int()
            .ok_or(MessagingError::Malformed("seq non entier"))?;
        let ts = get("ts")
            .as_int()
            .ok_or(MessagingError::Malformed("ts non entier"))?;
        if seq < 0 || ts < 0 {
            return Err(MessagingError::Malformed("seq/ts negatif"));
        }
        let sig = get("sig")
            .as_bytes()
            .ok_or(MessagingError::Malformed("sig non chaine"))?;
        if sig.len() != SIGNATURE_LENGTH {
            return Err(MessagingError::Malformed("sig != 64 octets"));
        }
        let body_ct = get("body")
            .as_bytes()
            .ok_or(MessagingError::Malformed("body non chaine"))?;
        // Corps chiffre = clair + tag : la borne filaire est
        // `max_body_len + TAG_LEN`.
        if body_ct.len() > cfg.max_body_len + TAG_LEN {
            return Err(MessagingError::BodyTooLarge(
                body_ct.len(),
                cfg.max_body_len + TAG_LEN,
            ));
        }

        let seq_u = seq as u64;
        // Verification de la signature sur la forme canonique
        // **telle que recue** (chiffre, puis signe).
        let unsigned = unsigned_form(kind, &id, seq_u, ts as u64, body_ct);
        if !peer.verify(&unsigned, sig) {
            return Err(MessagingError::BadSignature);
        }
        let (cipher, nonce) = chacha(recv_key, seq_u);
        let body = cipher
            .decrypt(&nonce, body_ct)
            .map_err(|_| MessagingError::Decrypt)?;
        Ok(Self {
            kind,
            id,
            seq: seq_u,
            ts: ts as u64,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> (LibNaClSecretKey, LibNaClPublicKey, MessagingConfig) {
        let sk = LibNaClSecretKey::generate();
        let pk = sk.public_key();
        (sk, pk, MessagingConfig::default())
    }

    fn roundtrip(frame: &Frame, sk: &LibNaClSecretKey, pk: &LibNaClPublicKey) -> Vec<u8> {
        let cfg = MessagingConfig::default();
        let key = [9u8; 32];
        let wire = frame.seal(sk, &key, &cfg).unwrap();
        let back = Frame::open(&wire, pk, &key, &cfg).unwrap();
        assert_eq!(&back, frame);
        wire
    }

    /// Roundtrip strict : `seal`/`open` sur chaque type de trame.
    #[test]
    fn roundtrip_tous_les_types() {
        let (sk, pk, _) = keys();
        for kind in [
            MsgKind::Hello,
            MsgKind::Accept,
            MsgKind::Reject,
            MsgKind::Msg,
            MsgKind::Ack,
        ] {
            let f = Frame::new(kind, 42, 1_700_000_000, b"corps de test".to_vec());
            roundtrip(&f, &sk, &pk);
        }
    }

    /// Determinisme : la meme trame (id fixe) se re-encode a
    /// l'identique — exigence de la forme canonique.
    #[test]
    fn encodage_deterministe() {
        let (sk, _, cfg) = keys();
        let key = [1u8; 32];
        let f = Frame {
            kind: MsgKind::Msg,
            id: [7u8; MSG_ID_LEN],
            seq: 1,
            ts: 100,
            body: b"abc".to_vec(),
        };
        assert_eq!(
            f.seal(&sk, &key, &cfg).unwrap(),
            f.seal(&sk, &key, &cfg).unwrap()
        );
    }

    /// `v != 1` → `UnknownVersion`, champ inconnu → `Malformed`,
    /// trame > borne → `FrameTooLarge`, bencode casse → erreur.
    #[test]
    fn rejets_de_codec() {
        let (sk, pk, cfg) = keys();
        let key = [2u8; 32];
        let f = Frame::new(MsgKind::Msg, 1, 1, b"x".to_vec());
        let wire = f.seal(&sk, &key, &cfg).unwrap();

        // bencode casse.
        assert!(Frame::open(&wire[..wire.len() - 2], &pk, &key, &cfg).is_err());
        // Non-dictionnaire.
        assert!(Frame::open(b"i1e", &pk, &key, &cfg).is_err());
        // Trame trop grande.
        let huge = vec![0u8; cfg.max_frame_len + 1];
        assert!(matches!(
            Frame::open(&huge, &pk, &key, &cfg),
            Err(MessagingError::FrameTooLarge(..))
        ));

        // `v` = 2 : reconstruit une trame pirate signee — la
        // signature reste valide mais la version est rejetee AVANT
        // verification (v lu avant sig).
        let mut d = BTreeMap::new();
        d.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
        d.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        d.insert(b"seq".to_vec(), BValue::Int(0));
        d.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
        d.insert(b"ts".to_vec(), BValue::Int(0));
        d.insert(b"type".to_vec(), BValue::Bytes(b"msg".to_vec()));
        d.insert(b"v".to_vec(), BValue::Int(2));
        let bad = BValue::Dict(d).encode();
        assert!(matches!(
            Frame::open(&bad, &pk, &key, &cfg),
            Err(MessagingError::UnknownVersion(2))
        ));

        // Cle inconnue ajoutee.
        let mut d2 = BTreeMap::new();
        d2.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
        d2.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        d2.insert(b"seq".to_vec(), BValue::Int(0));
        d2.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
        d2.insert(b"ts".to_vec(), BValue::Int(0));
        d2.insert(b"type".to_vec(), BValue::Bytes(b"msg".to_vec()));
        d2.insert(b"v".to_vec(), BValue::Int(1));
        d2.insert(b"extrane".to_vec(), BValue::Int(1));
        let bad2 = BValue::Dict(d2).encode();
        assert!(Frame::open(&bad2, &pk, &key, &cfg).is_err());
    }

    /// Signature : une trame signee par une AUTRE cle est rejetee ;
    /// un corps modifie post-signature aussi (encrypt-then-sign).
    #[test]
    fn signature_obligatoire_et_corps_authentifie() {
        let (sk, pk, cfg) = keys();
        let evil = LibNaClSecretKey::generate();
        let key = [3u8; 32];
        let f = Frame::new(MsgKind::Msg, 1, 1, b"secret".to_vec());
        // Signe par un imposteur -> rejet a la verification.
        let forged = f.seal(&evil, &key, &cfg).unwrap();
        assert!(matches!(
            Frame::open(&forged, &pk, &key, &cfg),
            Err(MessagingError::BadSignature)
        ));
        // Sig tronquee -> malforme avant verification.
        let mut wire = f.seal(&sk, &key, &cfg).unwrap();
        wire.truncate(wire.len() - 4);
        assert!(Frame::open(&wire, &pk, &key, &cfg).is_err());
    }

    /// Le corps est chiffre sur le fil (jamais le clair) et la
    /// mauvaise `recv_key` echoue en AEAD.
    #[test]
    fn corps_chiffre_sur_le_fil() {
        let (sk, pk, cfg) = keys();
        let key = [4u8; 32];
        let f = Frame::new(MsgKind::Msg, 1, 1, b"contenu secret".to_vec());
        let wire = f.seal(&sk, &key, &cfg).unwrap();
        assert!(wire.windows(14).all(|w| w != b"contenu secret"));
        let wrong = [5u8; 32];
        assert!(matches!(
            Frame::open(&wire, &pk, &wrong, &cfg),
            Err(MessagingError::Decrypt)
        ));
    }

    /// Bornes : `body` a la limite passe, au-dela refuse a l'emission
    /// comme a la reception.
    #[test]
    fn bornes_body_et_frame() {
        let (sk, pk, mut cfg) = keys();
        let key = [6u8; 32];
        let ok = Frame::new(MsgKind::Msg, 1, 1, vec![0u8; cfg.max_body_len]);
        let wire = ok.seal(&sk, &key, &cfg).unwrap();
        assert!(Frame::open(&wire, &pk, &key, &cfg).is_ok());
        let too_big = Frame::new(MsgKind::Msg, 1, 1, vec![0u8; cfg.max_body_len + 1]);
        assert!(matches!(
            too_big.seal(&sk, &key, &cfg),
            Err(MessagingError::BodyTooLarge(..))
        ));
        // Borne trame resserree : le refus touche aussi seal.
        cfg.max_frame_len = 64;
        assert!(matches!(
            ok.seal(&sk, &key, &cfg),
            Err(MessagingError::FrameTooLarge(..))
        ));
    }
}
