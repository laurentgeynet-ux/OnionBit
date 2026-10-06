// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Attestation` — verdict signe Ed25519 sur un sujet de contenu
//! (ADR-0015 §6, Phase 9d : curation OnionBit-only).
//!
//! Objet **auto-portant** : `curator` + `signature` voyagent dans le
//! payload — l'attestation reste verifiable quand elle est re-emise
//! par un tiers (gossip borne) ou relue depuis le stockage. La
//! signature couvre `SIG_DOMAIN || champs` : le domaine separe ces
//! objets de tout autre materiel signe par la meme cle (un `hello` ou
//! une trame messagerie ne peut pas etre rejoue en attestation).
//!
//! Pas de dependance a la reputation bande passante (ADR-0015 §6) :
//! « une chaine IPTV est une cle publique, pas un portefeuille » — le
//! verdict d'un curateur ne vaut que pour les noeuds qui le suivent
//! (`ext/curators`), le score de confiance est **local**.

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, LIBNACL_PK_BIN_LEN};

use crate::error::Ipv8Error;
use crate::serializer::{Reader, Writer};

/// Domaine de signature des attestations : separe cryptographiquement
/// ces objets de toute autre trame signee par la meme cle (anti-replay
/// croise `hello`/tunnel/messagerie).
const SIG_DOMAIN: &[u8] = b"onionbit/attest/v1";

/// Version du format d'attestation (`{v, ...}`) — v1.
pub const ATTEST_VERSION: u8 = 1;

/// `subject_kind` : nature de la cible du verdict.
pub mod kind {
    /// Torrent — `subject` = info-hash (20 octets).
    pub const INFOHASH: u8 = 1;
    /// Canal — `subject` = cle publique du canal (`LibNaClPK`
    /// binaire). « Une chaine IPTV est une cle publique » (ADR §6).
    pub const CHANNEL: u8 = 2;
    /// Identite d'un pair — `subject` = `pk_bin` du pair
    /// (`LibNaClPK` binaire, `LIBNACL_PK_BIN_LEN` octets — les memes
    /// octets que le champ `curator` et que les cles des contacts
    /// messagerie). Porte la confiance « utilisateur » : un endorse
    /// signe par un curateur suivi vaut `+1` sur la cle, et une
    /// auto-attestation `endorse` constitue la liste d'amis publique
    /// du signataire (re-gossipable vers un nouveau device).
    pub const IDENTITY: u8 = 3;

    /// Longueur attendue de `subject` pour `kind`, `None` si kind
    /// inconnu.
    pub fn subject_len(kind: u8) -> Option<usize> {
        match kind {
            INFOHASH => Some(20),
            // `LibNaClPK` filaire : `LibNaCLPK:` + crypt_pk + vk.
            // (historiquement 42 — le prefixe + une seule cle : un
            // canal reel n'a jamais tenu dans cette borne.)
            CHANNEL | IDENTITY => Some(super::LIBNACL_PK_BIN_LEN),
            _ => None,
        }
    }
}

/// `verdict` porte par une attestation.
pub mod verdict {
    /// Le sujet merite d'etre mis en avant (`+1` au score local).
    pub const ENDORSE: u8 = 1;
    /// Spam / contenu nuisible (`-1` au score local).
    pub const FLAG: u8 = 2;

    /// `true` si `v` est un verdict connu.
    pub fn is_valid(v: u8) -> bool {
        matches!(v, ENDORSE | FLAG)
    }
}

/// Attestation signee : verdict d'un curateur sur un sujet.
/// `(curator, kind, subject)` est la cle de deduplication — seule la
/// plus recente (`ts` max) compte dans le score local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// Version du format (`ATTEST_VERSION`).
    pub version: u8,
    /// `subject_kind`.
    pub kind: u8,
    /// Verdict (`verdict::ENDORSE`/`FLAG`).
    pub verdict: u8,
    /// Horodatage createur (secondes Unix — anti-replay inter-version :
    /// seule la plus recente attestation `(curator, kind, subject)`
    /// est conservee par les stockeurs).
    pub ts: u64,
    /// Sujet vise (info-hash 20 B ou `LibNaClPK` binaire — canal ou
    /// identite).
    pub subject: Vec<u8>,
    /// Cle publique du curateur (`LibNaClPK` binaire).
    pub curator: Vec<u8>,
    /// Signature Ed25519 de `SIG_DOMAIN || champs`.
    pub signature: [u8; 64],
}

impl Attestation {
    /// Cree et signe une attestation v1 avec `key` comme curateur.
    /// `kind`/`subject`/`verdict` sont supposes valides par l'appelant
    /// (`sign` re-verifie la forme — defensif).
    pub fn sign(
        key: &LibNaClSecretKey,
        kind: u8,
        subject: &[u8],
        verdict: u8,
        ts: u64,
    ) -> Result<Self, Ipv8Error> {
        let mut att = Self {
            version: ATTEST_VERSION,
            kind,
            verdict,
            ts,
            subject: subject.to_vec(),
            curator: key.public_key().to_bin(),
            signature: [0; 64],
        };
        att.check_shape()?;
        att.signature = key.sign(&att.signed_bytes());
        Ok(att)
    }

    /// Octets couverts par la signature : `SIG_DOMAIN || champs`.
    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SIG_DOMAIN.len() + 96);
        out.extend_from_slice(SIG_DOMAIN);
        let mut w = Writer::new();
        self.pack_fields(&mut w);
        out.extend_from_slice(&w.into_bytes());
        out
    }

    /// Champs signes : `{v, kind, verdict, ts, varlenH(subject),
    /// varlenH(curator)}` — l'ordre est fige par `signed_bytes`.
    fn pack_fields(&self, w: &mut Writer) {
        w.u8(self.version);
        w.u8(self.kind);
        w.u8(self.verdict);
        w.u64(self.ts);
        w.varlen_h(&self.subject);
        w.varlen_h(&self.curator);
    }

    /// Forme minimale exigible : version v1, kind/verdict connus,
    /// longueur du sujet conforme au kind, curateur en `LibNaClPK`.
    fn check_shape(&self) -> Result<(), Ipv8Error> {
        if self.version != ATTEST_VERSION {
            return Err(Ipv8Error::Malformed("version d'attestation inconnue"));
        }
        let Some(want) = kind::subject_len(self.kind) else {
            return Err(Ipv8Error::Malformed("subject_kind d'attestation inconnu"));
        };
        if self.subject.len() != want {
            return Err(Ipv8Error::Malformed(
                "longueur de sujet incoherente avec kind",
            ));
        }
        if !verdict::is_valid(self.verdict) {
            return Err(Ipv8Error::Malformed("verdict d'attestation inconnu"));
        }
        LibNaClPublicKey::from_bin(&self.curator)
            .map_err(|_| Ipv8Error::Malformed("curateur n'est pas une LibNaClPK"))?;
        Ok(())
    }

    /// Verifie la signature Ed25519 (`curator` signe `signed_bytes`).
    pub fn verify(&self) -> bool {
        let Ok(pk) = LibNaClPublicKey::from_bin(&self.curator) else {
            return false;
        };
        pk.verify(&self.signed_bytes(), &self.signature)
    }

    /// Payload `msg::ATTEST` : champs + signature (64 derniers octets).
    pub fn pack(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.pack_fields(&mut w);
        w.raw(&self.signature);
        w.into_bytes()
    }

    /// Deserialise et valide la **forme** (la signature doit encore
    /// etre verifiee par l'appelant via `verify` — les deux etapes
    /// sont separees pour fuzzer `unpack` seul). Trailing toleré.
    pub fn unpack(r: &mut Reader) -> Result<Self, Ipv8Error> {
        let att = Self {
            version: r.u8()?,
            kind: r.u8()?,
            verdict: r.u8()?,
            ts: r.u64()?,
            subject: r.varlen_h()?.to_vec(),
            curator: r.varlen_h()?.to_vec(),
            signature: {
                let mut sig = [0u8; 64];
                sig.copy_from_slice(r.take(64)?);
                sig
            },
        };
        att.check_shape()?;
        Ok(att)
    }

    /// `mid` hex du curateur (affichage API — jamais d'adresse).
    pub fn curator_mid(&self) -> String {
        hex::encode(onionbit_crypto::hash::ipv8_mid(&self.curator))
    }

    /// `mid` hex du sujet quand `kind == CHANNEL` (le sujet est une
    /// `LibNaClPK` — son `mid` est l'identite du canal).
    pub fn subject_mid(&self) -> Option<String> {
        (self.kind == kind::CHANNEL)
            .then(|| hex::encode(onionbit_crypto::hash::ipv8_mid(&self.subject)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let key = LibNaClSecretKey::generate();
        let att = Attestation::sign(&key, kind::INFOHASH, &[0x42; 20], verdict::ENDORSE, 1000)
            .expect("sign");
        assert!(att.verify());
        let packed = att.pack();
        let mut r = Reader::new(&packed);
        let got = Attestation::unpack(&mut r).expect("unpack");
        assert_eq!(got, att);
        assert!(got.verify());
    }

    #[test]
    fn signature_sur_domaine_separe() {
        // Une signature valide sur les champs SANS le domaine ne doit
        // pas verifier — protection anti-replay croise.
        let key = LibNaClSecretKey::generate();
        let att = Attestation::sign(
            &key,
            kind::CHANNEL,
            &[0x24; LIBNACL_PK_BIN_LEN],
            verdict::FLAG,
            7,
        )
        .unwrap();
        let mut w = Writer::new();
        att.pack_fields(&mut w); // sans SIG_DOMAIN
        let pk = LibNaClPublicKey::from_bin(&att.curator).unwrap();
        assert!(!pk.verify(&w.into_bytes(), &att.signature));
    }

    #[test]
    fn kind_identity_accepte_pk_bin() {
        // `IDENTITY` : sujet = `pk_bin` du pair vise (meme filaire
        // que `curator`) — porte la confiance utilisateur et la
        // liste d'amis auto-signee.
        let key = LibNaClSecretKey::generate();
        let ami = LibNaClSecretKey::generate();
        let att = Attestation::sign(
            &key,
            kind::IDENTITY,
            &ami.public_key().to_bin(),
            verdict::ENDORSE,
            42,
        )
        .expect("sign");
        assert!(att.verify());
        assert_eq!(kind::subject_len(kind::IDENTITY), Some(LIBNACL_PK_BIN_LEN));
        // Mauvaise longueur rejetee comme pour les autres kinds.
        assert!(Attestation::sign(&key, kind::IDENTITY, &[0; 32], verdict::ENDORSE, 0).is_err());
    }

    #[test]
    fn formes_rejetees() {
        let key = LibNaClSecretKey::generate();
        // kind inconnu.
        assert!(Attestation::sign(&key, 9, &[0; 20], verdict::ENDORSE, 0).is_err());
        // sujet de mauvaise longueur pour le kind.
        assert!(Attestation::sign(&key, kind::INFOHASH, &[0; 19], verdict::ENDORSE, 0).is_err());
        // verdict inconnu.
        assert!(Attestation::sign(&key, kind::INFOHASH, &[0; 20], 9, 0).is_err());
        // tronquee : unpack echoue sans panic.
        let att = Attestation::sign(&key, kind::INFOHASH, &[0; 20], verdict::FLAG, 1).unwrap();
        let packed = att.pack();
        for n in [0, 1, 8, 20, 40, packed.len() - 1] {
            let mut r = Reader::new(&packed[..n]);
            assert!(Attestation::unpack(&mut r).is_err());
        }
    }
}
