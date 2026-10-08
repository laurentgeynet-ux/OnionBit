// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Codec de trame messagerie — unique point d'entree du parseur
//! (ADR-0011, « format canonique versionne » ; v2 : ADR-0019).
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
//! Trame v2 = v1 + cle `conv` (16 octets) **au niveau dict**, sous
//! la signature (routable avant dechiffrement) + types
//! `gctl`/`attach` :
//!
//! ```text
//! { "body": <corps chiffre>,  "conv": <16 octets conv_id>,
//!   "id":   <16 octets>,      "seq":  <u64>,
//!   "sig":  <Ed25519 64 o>,   "ts":   <u64>,
//!   "type": <…|gctl|attach>,  "v":    2 }
//! ```
//!
//! - `body` = ChaCha20-Poly1305 du corps applicatif, cle
//!   [`crate::keys::MessagingKeys`], nonce = `"omsg" || seq(BE)` —
//!   `seq` monotone par direction garantit l'unicite du nonce.
//! - `sig` = signature Ed25519 de la forme canonique du dictionnaire
//!   **sans** `sig` (encrypt-then-sign : la signature authentifie le
//!   corps chiffre et `conv`) ; verification contre la `pk` du
//!   contact, qui est aussi celle dont derive le swarm.
//! - `v:2` **exige** `conv` ; `gctl`/`attach` exigent `v:2`. Le
//!   texte 1:1 reste emis en v1 tant que `CAP_MSG_V2` n'est pas
//!   confirme cote pair (compat Phase 8 : un pair v1 prefiltre les
//!   trames v2 en `UnknownVersion`).

use std::collections::BTreeMap;

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use onionbit_crypto::error::CryptoError;
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey, SIGNATURE_LENGTH};
use onionbit_format::bencode::{decode, BValue};

use crate::config::MessagingConfig;
use crate::conv::{ConvId, CONV_ID_LEN};
use crate::error::MessagingError;

/// Version du format de trame v1 (emission par defaut — compat).
pub const PROTO_VERSION: i64 = 1;
/// Version du format de trame v2 (`conv` + `gctl`/`attach`, ADR-0019).
pub const PROTO_VERSION_V2: i64 = 2;
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
    /// Controle de groupe (v2 — corps [`crate::gctl::Gctl`]).
    Gctl,
    /// Descripteur de piece jointe (v2 — corps
    /// [`crate::attach::AttachDesc`]).
    Attach,
}

impl MsgKind {
    /// `true` si le type exige une trame v2 (avec `conv`).
    pub fn is_v2_only(self) -> bool {
        matches!(self, MsgKind::Gctl | MsgKind::Attach)
    }

    fn as_bytes(self) -> &'static [u8] {
        match self {
            MsgKind::Hello => b"hello",
            MsgKind::Accept => b"accept",
            MsgKind::Reject => b"reject",
            MsgKind::Msg => b"msg",
            MsgKind::Ack => b"ack",
            MsgKind::Gctl => b"gctl",
            MsgKind::Attach => b"attach",
        }
    }

    fn from_bytes(b: &[u8]) -> Result<Self, MessagingError> {
        match b {
            b"hello" => Ok(MsgKind::Hello),
            b"accept" => Ok(MsgKind::Accept),
            b"reject" => Ok(MsgKind::Reject),
            b"msg" => Ok(MsgKind::Msg),
            b"ack" => Ok(MsgKind::Ack),
            b"gctl" => Ok(MsgKind::Gctl),
            b"attach" => Ok(MsgKind::Attach),
            _ => Err(MessagingError::UnknownKind(
                String::from_utf8_lossy(b).into_owned(),
            )),
        }
    }
}

/// Trame dechiffree mais **pas encore authentifiee** — utile cote
/// repondant, qui ne sait pas quelle cle publique verifier avant
/// la premiere trame d'un circuit (`hello` declare l'expediteur ;
/// sinon on essaie les contacts connus).
///
/// INVARIANT : `body` est accessible mais la trame n'a prouve ni
/// son emetteur ni son integrite — ne jamais la livrer a
/// l'application avant [`RawFrame::verify`].
#[derive(Debug)]
pub struct RawFrame {
    /// Type de trame.
    pub kind: MsgKind,
    /// Conversation (`v2` — routable avant verification de l'emet-
    /// teur : le prefiltre peut jauger `conv` avant `verify`).
    pub conv: Option<ConvId>,
    /// Identifiant (dedup).
    pub id: [u8; MSG_ID_LEN],
    /// `seq` — anti-replay applique APRES verification.
    pub seq: u64,
    /// Horodatage emetteur.
    pub ts: u64,
    /// Corps dechiffre (non authentifie).
    pub body: Vec<u8>,
    /// Forme canonique signee (chiffre inclus) — entree de `verify`.
    unsigned: Vec<u8>,
    /// Signature Ed25519 lue sur le fil.
    sig: [u8; SIGNATURE_LENGTH],
}

impl RawFrame {
    /// Version filaire de la trame lue (1 ou 2).
    pub fn version(&self) -> i64 {
        if self.conv.is_some() {
            PROTO_VERSION_V2
        } else {
            PROTO_VERSION
        }
    }

    /// Codec + dechiffrement sans verification d'emetteur : borne,
    /// bencode, ensemble de cles, champs, puis AEAD sous `recv_key`.
    /// Une signature invalide ou un corps non dechiffrable sont
    /// rejetes comme `open` (`Decrypt` couvre aussi une `recv_key`
    /// fausse — impossible de distinguer avant la cle).
    pub fn parse(
        data: &[u8],
        recv_key: &[u8; 32],
        cfg: &MessagingConfig,
    ) -> Result<Self, MessagingError> {
        let f = parse_fields(data, cfg)?;
        let unsigned = unsigned_form(f.kind, f.conv.as_ref(), &f.id, f.seq, f.ts, &f.body_ct);
        let (cipher, nonce) = chacha(recv_key, f.seq);
        let body = cipher
            .decrypt(&nonce, f.body_ct.as_slice())
            .map_err(|_| MessagingError::Decrypt)?;
        let mut sig = [0u8; SIGNATURE_LENGTH];
        sig.copy_from_slice(&f.sig);
        Ok(Self {
            kind: f.kind,
            conv: f.conv,
            id: f.id,
            seq: f.seq,
            ts: f.ts,
            body,
            unsigned,
            sig,
        })
    }

    /// Verifie la signature contre `peer` et livre la trame
    /// authentifiee. `BadSignature` si la preuve echoue.
    pub fn verify(&self, peer: &LibNaClPublicKey) -> Result<Frame, MessagingError> {
        if !peer.verify(&self.unsigned, &self.sig) {
            return Err(MessagingError::BadSignature);
        }
        Ok(Frame {
            kind: self.kind,
            conv: self.conv,
            id: self.id,
            seq: self.seq,
            ts: self.ts,
            body: self.body.clone(),
        })
    }
}

/// Prefiltre du demux (ADR-0011, etape 38) : verifie la borne de
/// taille et la version **avant** tout parse bencode — le seul cout
/// par datagramme hostile ecarte ici est constant.
///
/// La forme canonique trie les cles, donc `v` est la derniere et la
/// trame se termine par `1:vi<ver>ee` (`conv` trie avant `id` : la
/// position de `v` ne change pas en v2). Une entree qui ne revele
/// pas ce suffixe n'est pas une trame canonique : rejet `Malformed`.
/// Les versions admises sont `{1, 2}` — un pair v1 rejetterait ici
/// toute trame v2 en `UnknownVersion` (degradation propre).
pub fn preflight(data: &[u8], cfg: &MessagingConfig) -> Result<(), MessagingError> {
    if data.len() > cfg.max_frame_len {
        return Err(MessagingError::FrameTooLarge(data.len(), cfg.max_frame_len));
    }
    if data.first() != Some(&b'd') {
        return Err(MessagingError::Malformed(
            "la trame n'est pas un dictionnaire",
        ));
    }
    // Cherche `1:vi` dans les derniers octets, puis exige
    // `<ver>` entier suivi exactement de `ee` (fin du dict).
    let tail = &data[data.len().saturating_sub(16)..];
    let Some(pos) = tail.windows(4).position(|w| w == b"1:vi") else {
        return Err(MessagingError::Malformed("suffixe de version absent"));
    };
    let rest = &tail[pos + 4..];
    let Some(end) = rest.iter().position(|b| *b == b'e') else {
        return Err(MessagingError::Malformed("version non terminee"));
    };
    let v: i64 = std::str::from_utf8(&rest[..end])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(MessagingError::Malformed("version non entiere"))?;
    // L'`e` terminant l'entier doit etre suivi du `e` final du dict.
    if rest.len() != end + 2 || rest[end + 1] != b'e' {
        return Err(MessagingError::Malformed("version non terminale"));
    }
    if v != PROTO_VERSION && v != PROTO_VERSION_V2 {
        return Err(MessagingError::UnknownVersion(v));
    }
    Ok(())
}

/// Champs bruts d'une trame filaire (corps encore chiffre).
struct WireFields {
    kind: MsgKind,
    conv: Option<ConvId>,
    id: [u8; MSG_ID_LEN],
    seq: u64,
    ts: u64,
    body_ct: Vec<u8>,
    sig: Vec<u8>,
}

/// Codec strict de la trame filaire : borne -> bencode -> version
/// -> ensemble de cles exact **par version** -> types/tailles de
/// champs. Ne verifie ni `sig` ni AEAD — c'est le role des phases
/// suivantes.
fn parse_fields(data: &[u8], cfg: &MessagingConfig) -> Result<WireFields, MessagingError> {
    // Borne avant tout parse : le seul cout par trame hostile
    // est lineaire et borne (anti-DoS, ADR-0011).
    if data.len() > cfg.max_frame_len {
        return Err(MessagingError::FrameTooLarge(data.len(), cfg.max_frame_len));
    }
    let value = decode(data)?;
    let dict = value.as_dict().ok_or(MessagingError::Malformed(
        "la trame n'est pas un dictionnaire",
    ))?;
    // La version tranche l'ensemble de cles attendu — `v` est lue
    // d'abord (elle est aussi verifiee par le prefiltre).
    let v = dict
        .get(b"v".as_ref())
        .and_then(BValue::as_int)
        .ok_or(MessagingError::Malformed("v absent ou non entier"))?;
    let (keys, v2) = match v {
        PROTO_VERSION => (FRAME_KEYS, false),
        PROTO_VERSION_V2 => (FRAME_KEYS_V2, true),
        _ => return Err(MessagingError::UnknownVersion(v)),
    };
    // Ensemble de cles strict : ni champ critique absent, ni
    // champ inconnu ignore.
    if dict.len() != keys.len() || !keys.iter().all(|k| dict.contains_key(*k)) {
        return Err(MessagingError::Malformed(
            "ensemble de cles different de la trame attendue",
        ));
    }
    let get = |k: &'static str| dict.get(k.as_bytes()).unwrap();

    let conv = if v2 {
        let raw = get("conv")
            .as_bytes()
            .ok_or(MessagingError::Malformed("conv non chaine"))?;
        if raw.len() != CONV_ID_LEN {
            return Err(MessagingError::Malformed("conv != 16 octets"));
        }
        let mut conv = [0u8; CONV_ID_LEN];
        conv.copy_from_slice(raw);
        Some(conv)
    } else {
        None
    };
    let kind = MsgKind::from_bytes(
        get("type")
            .as_bytes()
            .ok_or(MessagingError::Malformed("type non chaine"))?,
    )?;
    // `gctl`/`attach` exigent v2 (une trame v1 ne peut pas porter
    // `conv` — le service n'aurait rien pour router la trame).
    if !v2 && kind.is_v2_only() {
        return Err(MessagingError::Malformed("type v2 sur trame v1"));
    }
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
    Ok(WireFields {
        kind,
        conv,
        id,
        seq: seq as u64,
        ts: ts as u64,
        body_ct: body_ct.to_vec(),
        sig: sig.to_vec(),
    })
}

/// Trame de messagerie decodee (corps en clair apres `open`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Type de trame.
    pub kind: MsgKind,
    /// Conversation — `Some` ⇒ trame v2 (conv directe derivee ou
    /// identifiant de groupe) ; `None` ⇒ v1 (compat Phase 8).
    pub conv: Option<ConvId>,
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

/// Ensemble exact des cles de la trame v2 = v1 + `conv`.
const FRAME_KEYS_V2: &[&[u8]] = &[
    b"body", b"conv", b"id", b"seq", b"sig", b"ts", b"type", b"v",
];

fn chacha(key: &[u8; 32], seq: u64) -> (ChaCha20Poly1305, Nonce) {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(NONCE_DOMAIN);
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    let key: &Key = key.into();
    (ChaCha20Poly1305::new(key), nonce.into())
}

/// Forme canonique signee : dictionnaire de tous les champs sauf
/// `sig` — re-serialise a l'identique des deux cotes (`BTreeMap`
/// trie les cles). `conv` fait partie de la forme signee en v2 :
/// la signature authentifie la conversation routee.
fn unsigned_form(
    kind: MsgKind,
    conv: Option<&ConvId>,
    id: &[u8; MSG_ID_LEN],
    seq: u64,
    ts: u64,
    body_ct: &[u8],
) -> Vec<u8> {
    let mut d = BTreeMap::new();
    d.insert(b"body".to_vec(), BValue::Bytes(body_ct.to_vec()));
    if let Some(conv) = conv {
        d.insert(b"conv".to_vec(), BValue::Bytes(conv.to_vec()));
    }
    d.insert(b"id".to_vec(), BValue::Bytes(id.to_vec()));
    d.insert(b"seq".to_vec(), BValue::Int(seq as i64));
    d.insert(b"ts".to_vec(), BValue::Int(ts as i64));
    d.insert(b"type".to_vec(), BValue::Bytes(kind.as_bytes().to_vec()));
    d.insert(
        b"v".to_vec(),
        BValue::Int(if conv.is_some() {
            PROTO_VERSION_V2
        } else {
            PROTO_VERSION
        }),
    );
    BValue::Dict(d).encode()
}

impl Frame {
    /// Cree une trame v1 avec un `id` aleatoire neuf.
    pub fn new(kind: MsgKind, seq: u64, ts: u64, body: Vec<u8>) -> Self {
        Self {
            kind,
            conv: None,
            id: rand::random(),
            seq,
            ts,
            body,
        }
    }

    /// Cree une trame v2 dans une conversation (`conv` non nul —
    /// directe derivee ou groupe).
    pub fn new_in_conv(kind: MsgKind, conv: ConvId, seq: u64, ts: u64, body: Vec<u8>) -> Self {
        Self {
            kind,
            conv: Some(conv),
            id: rand::random(),
            seq,
            ts,
            body,
        }
    }

    /// Version filaire emise par [`Frame::seal`] (2 si `conv`
    /// present, 1 sinon).
    pub fn version(&self) -> i64 {
        if self.conv.is_some() {
            PROTO_VERSION_V2
        } else {
            PROTO_VERSION
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
        // `gctl`/`attach` exigent `conv` : sans conversation la
        // trame est inroutable cote recepteur.
        if self.kind.is_v2_only() && self.conv.is_none() {
            return Err(MessagingError::Malformed(
                "gctl/attach exigent une trame v2 (conv)",
            ));
        }
        let (cipher, nonce) = chacha(send_key, self.seq);
        let body_ct = cipher
            .encrypt(&nonce, self.body.as_ref())
            .map_err(|_| CryptoError::Aead)?;
        let unsigned = unsigned_form(
            self.kind,
            self.conv.as_ref(),
            &self.id,
            self.seq,
            self.ts,
            &body_ct,
        );
        let sig = sk.sign(&unsigned);

        let mut d = BTreeMap::new();
        d.insert(b"body".to_vec(), BValue::Bytes(body_ct));
        if let Some(conv) = &self.conv {
            d.insert(b"conv".to_vec(), BValue::Bytes(conv.to_vec()));
        }
        d.insert(b"id".to_vec(), BValue::Bytes(self.id.to_vec()));
        d.insert(b"seq".to_vec(), BValue::Int(self.seq as i64));
        d.insert(b"sig".to_vec(), BValue::Bytes(sig.to_vec()));
        d.insert(b"ts".to_vec(), BValue::Int(self.ts as i64));
        d.insert(
            b"type".to_vec(),
            BValue::Bytes(self.kind.as_bytes().to_vec()),
        );
        d.insert(b"v".to_vec(), BValue::Int(self.version()));
        let wire = BValue::Dict(d).encode();
        if wire.len() > cfg.max_frame_len {
            return Err(MessagingError::FrameTooLarge(wire.len(), cfg.max_frame_len));
        }
        Ok(wire)
    }

    /// Parse une trame recue : borne de taille **avant** tout parse,
    /// ensemble de cles strict, dechiffrement de `body` sous
    /// `recv_key` puis verification de signature contre la `pk` du
    /// contact.
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
        RawFrame::parse(data, recv_key, cfg)?.verify(peer)
    }
}

impl RawFrame {
    /// [`preflight`] + parse complet — point d'entree du demux.
    pub fn parse_checked(
        data: &[u8],
        recv_key: &[u8; 32],
        cfg: &MessagingConfig,
    ) -> Result<Self, MessagingError> {
        preflight(data, cfg)?;
        Self::parse(data, recv_key, cfg)
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
            conv: None,
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

        // `v` = 3 : reconstruit une trame pirate signee — la
        // signature reste valide mais la version est rejetee AVANT
        // verification (v lu avant sig).
        let mut d = BTreeMap::new();
        d.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
        d.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        d.insert(b"seq".to_vec(), BValue::Int(0));
        d.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
        d.insert(b"ts".to_vec(), BValue::Int(0));
        d.insert(b"type".to_vec(), BValue::Bytes(b"msg".to_vec()));
        d.insert(b"v".to_vec(), BValue::Int(3));
        let bad = BValue::Dict(d).encode();
        assert!(matches!(
            Frame::open(&bad, &pk, &key, &cfg),
            Err(MessagingError::UnknownVersion(3))
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

    /// Roundtrip v2 : `conv` en cle dict top-level, sous la
    /// signature — la trame rouvre avec la meme `conv`.
    #[test]
    fn roundtrip_v2_conv() {
        let (sk, pk, _) = keys();
        let conv = crate::conv::direct_conv(&[1u8; 32], &[2u8; 32]);
        for kind in [MsgKind::Msg, MsgKind::Ack, MsgKind::Gctl, MsgKind::Attach] {
            let f = Frame::new_in_conv(kind, conv, 9, 1_700_000_000, b"corps".to_vec());
            roundtrip(&f, &sk, &pk);
            assert_eq!(f.version(), PROTO_VERSION_V2);
        }
    }

    /// `conv` est signee : une trame v2 dont `conv` est permutee
    /// post-signature est rejetee en `BadSignature`.
    #[test]
    fn v2_conv_signee() {
        let (sk, pk, cfg) = keys();
        let key = [8u8; 32];
        let conv = crate::conv::direct_conv(&[1u8; 32], &[2u8; 32]);
        let f = Frame::new_in_conv(MsgKind::Msg, conv, 1, 1, b"x".to_vec());
        let wire = f.seal(&sk, &key, &cfg).unwrap();
        // `4:conv16:` suivi des 16 octets — permutation des deux
        // premiers octets de la conv sur le fil.
        let pat = b"4:conv16:";
        let pos = wire
            .windows(pat.len())
            .position(|w| w == pat)
            .expect("cle conv absente du fil");
        let mut forged = wire.clone();
        forged.swap(pos + pat.len(), pos + pat.len() + 1);
        assert!(matches!(
            Frame::open(&forged, &pk, &key, &cfg),
            Err(MessagingError::BadSignature)
        ));
    }

    /// `gctl`/`attach` exigent `conv` a l'emission ; une trame v1
    /// portant `type=gctl` est rejetee au codec.
    #[test]
    fn v2_kinds_exigent_conv() {
        let (sk, pk, cfg) = keys();
        let key = [8u8; 32];
        let bare = Frame::new(MsgKind::Gctl, 1, 1, b"{}".to_vec());
        assert!(bare.seal(&sk, &key, &cfg).is_err());
        let bare = Frame::new(MsgKind::Attach, 1, 1, b"{}".to_vec());
        assert!(bare.seal(&sk, &key, &cfg).is_err());
        // v1 filaire avec `type=gctl` → Malformed (pas de conv pour
        // router la trame).
        let mut d = BTreeMap::new();
        d.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
        d.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        d.insert(b"seq".to_vec(), BValue::Int(0));
        d.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
        d.insert(b"ts".to_vec(), BValue::Int(0));
        d.insert(b"type".to_vec(), BValue::Bytes(b"gctl".to_vec()));
        d.insert(b"v".to_vec(), BValue::Int(1));
        assert!(Frame::open(&BValue::Dict(d).encode(), &pk, &key, &cfg).is_err());
        // v2 sans `conv` → ensemble de cles faux.
        let mut d2 = BTreeMap::new();
        d2.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
        d2.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
        d2.insert(b"seq".to_vec(), BValue::Int(0));
        d2.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
        d2.insert(b"ts".to_vec(), BValue::Int(0));
        d2.insert(b"type".to_vec(), BValue::Bytes(b"msg".to_vec()));
        d2.insert(b"v".to_vec(), BValue::Int(2));
        assert!(Frame::open(&BValue::Dict(d2).encode(), &pk, &key, &cfg).is_err());
    }

    /// Compat : v1 et v2 cohabitent sur le meme lien (meme `seq`
    /// space, meme cle de corps) ; le prefiltre accepte les deux.
    #[test]
    fn mixte_v1_v2_meme_lien() {
        let (sk, pk, cfg) = keys();
        let key = [8u8; 32];
        let conv = crate::conv::direct_conv(&[1u8; 32], &[2u8; 32]);
        let v1 = Frame::new(MsgKind::Msg, 1, 1, b"v1".to_vec())
            .seal(&sk, &key, &cfg)
            .unwrap();
        let v2 = Frame::new_in_conv(MsgKind::Msg, conv, 2, 1, b"v2".to_vec())
            .seal(&sk, &key, &cfg)
            .unwrap();
        for wire in [&v1, &v2] {
            preflight(wire, &cfg).unwrap();
        }
        assert_eq!(Frame::open(&v1, &pk, &key, &cfg).unwrap().conv, None);
        assert_eq!(Frame::open(&v2, &pk, &key, &cfg).unwrap().conv, Some(conv));
        // `v` reste la derniere cle : suffixe `1:vi2ee` visible.
        assert!(v2.ends_with(b"1:vi2ee"));
        assert!(v1.ends_with(b"1:vi1ee"));
    }

    /// Une trame v2 avec `conv` de taille fausse ou cle inconnue
    /// est rejetee comme toute trame malformee.
    #[test]
    fn v2_conv_malformee() {
        let (_sk, pk, cfg) = keys();
        let key = [8u8; 32];
        for conv_len in [15usize, 17] {
            let mut d = BTreeMap::new();
            d.insert(b"body".to_vec(), BValue::Bytes(b"ab".to_vec()));
            d.insert(b"conv".to_vec(), BValue::Bytes(vec![0u8; conv_len]));
            d.insert(b"id".to_vec(), BValue::Bytes([0u8; 16].to_vec()));
            d.insert(b"seq".to_vec(), BValue::Int(0));
            d.insert(b"sig".to_vec(), BValue::Bytes([0u8; 64].to_vec()));
            d.insert(b"ts".to_vec(), BValue::Int(0));
            d.insert(b"type".to_vec(), BValue::Bytes(b"msg".to_vec()));
            d.insert(b"v".to_vec(), BValue::Int(2));
            assert!(Frame::open(&BValue::Dict(d).encode(), &pk, &key, &cfg).is_err());
        }
    }
}
