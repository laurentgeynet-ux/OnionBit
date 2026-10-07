// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Primitives du transport furtif `stealth` (ADR-0017, etape 50) —
//! domaine `onionbit/stealth/v1`, disjoint de `ext-obf/v1` et des
//! `pairbox` tunnel : aucun blob n'est interchangeable entre
//! domaines.
//!
//! Le handshake suit le patron `ntor`/obfs4 (conception eprouvee) :
//!
//! ```text
//! client                                  pont
//!   |  rep(X') || AEAD(k_hs1, ts || id | pad)   |
//!   |---------------------------------------->|
//!   |        (X' = point torsion-dirty         |
//!   |         Elligator-encodable ; le          |
//!   |         representant seul va sur le fil)  |
//!   |  rep(Y') || AEAD(k_hs2, ts || pad)        |
//!   |<----------------------------------------|
//!   |  session = HKDF(shared_ee || static_dh,   |
//!   |   transcript rep(X') || rep(Y'))          |
//! ```
//!
//! - `static_dh = X25519(x, B) = X25519(b, X')` authentifie le
//!   premier datagramme : seul le detenteur de `b` peut l'ouvrir —
//!   silence absolu sur tout le reste (anti-probing).
//! - `shared_ee = X25519(x, Y') = X25519(y, X')` donne la forward
//!   secrecy de session.
//! - Les points publics voyagent en **representant Elligator2**
//!   (uniforme, 32 octets) — jamais de coordonnee brute
//!   identifiable (residu quadratique exploitable par DPI). La
//!   composante de torsion du point « sale » est annihilee par le
//!   scalaire clampe du pair (`clamp ≡ 0 mod 8`) : le DH est
//!   inchange, l'encodage couvre le groupe entier.
//!
//! Seules des primitives bytes-in/bytes-out vivent ici ; le format
//! filaire, les sessions et les filtres sont dans
//! `onionbit-ipv8::stealth`.

use crate::error::CryptoError;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

/// Domaine racine du protocole stealth — jamais partage avec les
/// autres usages HKDF du projet.
pub const STEALTH_DOMAIN: &[u8] = b"onionbit/stealth/v1";

/// Info HKDF de la cle d'authentification du 1er datagramme client.
const INFO_HS1: &[u8] = b"onionbit/stealth/v1/hs1";
/// Info HKDF de la cle d'authentification de la reponse du pont.
const INFO_HS2: &[u8] = b"onionbit/stealth/v1/hs2";
/// Info HKDF du trousseau de session post-handshake.
const INFO_SESSION: &[u8] = b"onionbit/stealth/v1/session";

/// Cle ephemere « cachee » : point X25519 **torsion-dirty**
/// representable par Elligator2 — le representant est indiscernable
/// d'une chaine uniforme (bits de poids fort randomises inclus),
/// le point est ce qui entre dans le DH et le transcript.
///
/// Le secret est zeroize au drop (delegue a
/// `elligator2::HiddenKey`).
pub struct HiddenEph {
    inner: elligator2::HiddenKey,
}

impl HiddenEph {
    /// Genere une cle ephemere representable (borne interne de 64
    /// essais — un echec signifie un RNG casse, pas de la malchance).
    pub fn generate() -> Self {
        let key = elligator2::generate(&mut rand::rng())
            .expect("generate Elligator2 borne : RNG sain requis");
        Self { inner: key }
    }

    /// Representant uniforme — **seul element publie sur le fil**.
    pub fn representative(&self) -> [u8; 32] {
        *self.inner.representative()
    }

    /// Point public canonique (u-coordonnee dirty) — entre dans le
    /// DH du pair et dans le transcript HKDF.
    pub fn point(&self) -> [u8; 32] {
        *self.inner.point()
    }

    /// Diffie-Hellman `clamp(secret) * peer_point` — le clamp interne
    /// de X25519 annihile la torsion du point dirty du pair.
    pub fn diffie_hellman(&self, peer_point: &[u8; 32]) -> [u8; 32] {
        x25519(self.inner.secret_bytes(), peer_point)
    }
}

impl core::fmt::Debug for HiddenEph {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Le secret ne doit jamais atteindre une ligne de log.
        f.debug_struct("HiddenEph")
            .field("point", &hex::encode(&self.inner.point()[..4]))
            .finish_non_exhaustive()
    }
}

/// Decode un representant en point canonique — la map est **totale**
/// (toute chaine de 32 octets a une image) : un observateur ne peut
/// pas filtrer sur « decode ou pas ».
pub fn representative_to_point(rep: &[u8; 32]) -> [u8; 32] {
    elligator2::from_representative(rep)
}

/// Le test de classification lui-meme : vrai si `bytes` est une
/// u-coordonnee Montgomery plausible (residu quadratique) — ce qu'un
/// DPI executerait sur une cle X25519 brute. Reserve aux tests : un
/// *representant* ne doit **pas** etre detectable ainsi plus que le
/// hasard.
pub fn is_montgomery_u(bytes: &[u8; 32]) -> bool {
    elligator2::is_montgomery_u(bytes)
}

/// X25519 nu : `clamp(sk) * pk`. Le clampage du scalaire a lieu dans
/// `x25519-dalek` — `sk` peut etre un scalaire brut non clampe
/// (`HiddenEph::secret_bytes` l'est, a l'image de `crypto_scalarmult`
/// libnacl).
pub fn x25519(sk: &[u8; 32], pk: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*sk);
    let public = X25519PublicKey::from(*pk);
    *secret.diffie_hellman(&public).as_bytes()
}

/// HKDF-SHA256 `extract(salt=domaine, ikm)` puis `expand(info)` —
/// 32 octets. `info` concatene le sous-domaine et le transcript qui
/// lie la cle au contexte exact du handshake.
fn hkdf32(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    hkdf.expand(info, &mut out)
        .expect("HKDF expand borne a 32 octets");
    out
}

/// Cle d'authentification du 1er datagramme (`hs1`) : prouve la
/// connaissance du secret statique du pont `static_dh =
/// X25519(x, B) = X25519(b, X')`. Le transcript `rep_x || bridge_pk`
/// lie la cle a ce pair precis — un hs1 fabrique pour un autre pont
/// ou un autre ephémère ne s'ouvre pas.
pub fn hs1_key(bridge_pk: &[u8; 32], rep_x: &[u8; 32], static_dh: &[u8; 32]) -> [u8; 32] {
    let mut info = Vec::with_capacity(INFO_HS1.len() + 64);
    info.extend_from_slice(INFO_HS1);
    info.extend_from_slice(rep_x);
    info.extend_from_slice(bridge_pk);
    hkdf32(STEALTH_DOMAIN, static_dh, &info)
}

/// Cle d'authentification de la reponse du pont (`hs2`) : prouve la
/// connaissance de `shared_ee` (eph-eph) en plus de `static_dh` — un
/// attaquant qui a vole `bridge_pk` ne peut pas forger de reponse
/// sans le secret `y`.
pub fn hs2_key(
    rep_x: &[u8; 32],
    rep_y: &[u8; 32],
    shared_ee: &[u8; 32],
    static_dh: &[u8; 32],
) -> [u8; 32] {
    let mut info = Vec::with_capacity(INFO_HS2.len() + 64);
    info.extend_from_slice(INFO_HS2);
    info.extend_from_slice(rep_x);
    info.extend_from_slice(rep_y);
    let mut ikm = Vec::with_capacity(64);
    ikm.extend_from_slice(shared_ee);
    ikm.extend_from_slice(static_dh);
    hkdf32(STEALTH_DOMAIN, &ikm, &info)
}

/// Trousseau de session post-handshake : cles directionnelles +
/// masques d'obfuscation du compteur de trame.
pub struct SessionKeys {
    /// Cle AEAD client -> pont.
    pub c2b: [u8; 32],
    /// Cle AEAD pont -> client.
    pub b2c: [u8; 32],
    /// Masque XOR du compteur filaire, sens client -> pont.
    pub mask_c2b: u64,
    /// Masque XOR du compteur filaire, sens pont -> client.
    pub mask_b2c: u64,
}

/// `HKDF(shared_ee || static_dh, "session" || rep_x || rep_y)` — le
/// transcript complet lie la session au handshake exact (rep inclus :
/// un rep choisi par l'attaquant ne peut pas pivoter les cles vers
/// un contexte autre).
pub fn session_keys(
    shared_ee: &[u8; 32],
    static_dh: &[u8; 32],
    rep_x: &[u8; 32],
    rep_y: &[u8; 32],
) -> SessionKeys {
    let mut ikm = Vec::with_capacity(64);
    ikm.extend_from_slice(shared_ee);
    ikm.extend_from_slice(static_dh);
    let mut info = Vec::with_capacity(INFO_SESSION.len() + 64);
    info.extend_from_slice(INFO_SESSION);
    info.extend_from_slice(rep_x);
    info.extend_from_slice(rep_y);
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(STEALTH_DOMAIN), &ikm);
    let mut block = [0u8; 80];
    hkdf.expand(&info, &mut block)
        .expect("HKDF expand borne a 80 octets");
    let mut c2b = [0u8; 32];
    let mut b2c = [0u8; 32];
    c2b.copy_from_slice(&block[..32]);
    b2c.copy_from_slice(&block[32..64]);
    SessionKeys {
        c2b,
        b2c,
        mask_c2b: u64::from_le_bytes(block[64..72].try_into().expect("8 octets")),
        mask_b2c: u64::from_le_bytes(block[72..80].try_into().expect("8 octets")),
    }
}

/// ChaCha20-Poly1305 `encrypt(nonce, aad ‖ pt)` — le compteur de
/// trame sert de nonce (12 octets : 4 zeros ‖ compteur BE), jamais
/// reutilise sous une meme cle de session.
pub fn aead_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.into());
    cipher
        .encrypt(
            nonce.into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("ChaCha20-Poly1305 encrypt sans allocation bornee")
}

/// Ouverture AEAD symetrique de [`aead_seal`]. Echec homogene :
/// `CryptoError::Aead` quelle que soit la cause.
pub fn aead_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            nonce.into(),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Aead)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_eph_roundtrip_et_dh_symetrique() {
        let x = HiddenEph::generate();
        let y = HiddenEph::generate();
        // representant -> point exact
        assert_eq!(representative_to_point(&x.representative()), x.point());
        // DH symetrique malgre les points dirty
        assert_eq!(x.diffie_hellman(&y.point()), y.diffie_hellman(&x.point()));
    }

    #[test]
    fn representant_indistinguable_du_test_montgomery() {
        // Oracle de classification brut : une pk X25519 brute est
        // toujours u-coordonnee valide ; un representant ne doit pas
        // l'etre significativement plus que le hasard (~1/2).
        let pk = x25519_dalek::PublicKey::from(&StaticSecret::random());
        assert!(is_montgomery_u(pk.as_bytes()));
        let mut hits = 0u32;
        for _ in 0..64 {
            let e = HiddenEph::generate();
            hits += u32::from(is_montgomery_u(&e.representative()));
        }
        // ~32 attendu si uniforme ; borne large anti-flake (>55 = signal).
        assert!(
            hits <= 55,
            "representants trop souvent u-valides: {hits}/64"
        );
    }

    #[test]
    fn domaines_distincts_cles_different() {
        let e1 = HiddenEph::generate();
        let e2 = HiddenEph::generate();
        let b = x25519_dalek::StaticSecret::random();
        let b_pk = X25519PublicKey::from(&b);
        let sdh_c = x25519(e1.inner.secret_bytes(), b_pk.as_bytes());
        let sdh_b = x25519(b.as_bytes(), &e1.point());
        assert_eq!(sdh_c, sdh_b);
        let ee_c = e1.diffie_hellman(&e2.point());
        let ee_b = e2.diffie_hellman(&e1.point());
        assert_eq!(ee_c, ee_b);
        let k1 = hs1_key(b_pk.as_bytes(), &e1.representative(), &sdh_c);
        let k2 = hs2_key(&e1.representative(), &e2.representative(), &ee_c, &sdh_c);
        let ks = session_keys(&ee_c, &sdh_c, &e1.representative(), &e2.representative());
        assert_ne!(k1, k2);
        assert_ne!(k1, ks.c2b);
        assert_ne!(ks.c2b, ks.b2c);
        assert_ne!(ks.mask_c2b, ks.mask_b2c);
    }

    #[test]
    fn aead_ouvert_et_rejet_homogene() {
        let k = [7u8; 32];
        let n = [3u8; 12];
        let ct = aead_seal(&k, &n, b"aad", b"payload");
        assert_eq!(aead_open(&k, &n, b"aad", &ct).unwrap(), b"payload");
        assert!(aead_open(&k, &n, b"autre-aad", &ct).is_err());
        assert!(aead_open(&[8u8; 32], &n, b"aad", &ct).is_err());
        assert!(aead_open(&k, &[4u8; 12], b"aad", &ct).is_err());
        assert!(aead_open(&k, &n, b"aad", &ct[..ct.len() - 3]).is_err());
    }
}
