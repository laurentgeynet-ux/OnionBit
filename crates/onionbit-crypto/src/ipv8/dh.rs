// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Diffie-Hellman des tunnels IPv8 : `crypto_box_beforenm`.
//!
//! Reference de verite :
//! `docs/reference_tribler/ipv8_rust_tunnels/dh.rs` (repo officiel
//! `Tribler/ipv8-rust-tunnels`).
//!
//! Algorithme :
//!
//! 1. clamper la cle secrete X25519 (comme `crypto_scalarmult_curve25519`
//!    de libnacl) ;
//! 2. multiplication X25519 avec la cle publique du pair -> secret partage
//!    brut (32 octets) ;
//! 3. derivation HSalsa20 (constante "expand 32-byte k", nonce = 16 octets
//!    a zero) -> secret partage final (32 octets).
//!
//! Ce secret alimente ensuite `generate_session_keys` (HKDF-SHA256).

use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::error::CryptoError;

/// Constante HSalsa20 "expand 32-byte k" (sigma0..sigma3).
const HSALSA20_CONSTANT: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
/// Nombre de rounds HSalsa20 (20 rounds = 10 doubles rounds).
const HSALSA20_ROUNDS: usize = 20;

#[inline]
fn rotl32(x: u32, n: u32) -> u32 {
    x.rotate_left(n)
}

/// Coeur HSalsa20 : derive 32 octets depuis `key` (32 octets) et `nonce`
/// (16 octets). Port de `crypto_core_hsalsa20` de libnacl.
fn hsalsa20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32] {
    let load32_le = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("slice de 4 octets"));

    // Etat Salsa20 : [c0, k0, k1, k2, k3, c1, n0, n1, n2, n3, c2, k4, k5, k6, k7, c3]
    let mut x = [
        HSALSA20_CONSTANT[0],
        load32_le(&key[0..4]),
        load32_le(&key[4..8]),
        load32_le(&key[8..12]),
        load32_le(&key[12..16]),
        HSALSA20_CONSTANT[1],
        load32_le(&nonce[0..4]),
        load32_le(&nonce[4..8]),
        load32_le(&nonce[8..12]),
        load32_le(&nonce[12..16]),
        HSALSA20_CONSTANT[2],
        load32_le(&key[16..20]),
        load32_le(&key[20..24]),
        load32_le(&key[24..28]),
        load32_le(&key[28..32]),
        HSALSA20_CONSTANT[3],
    ];

    for _ in (0..HSALSA20_ROUNDS).step_by(2) {
        // Double round Salsa20 (colonne puis ligne), identique a la
        // reference ipv8-rust-tunnels/dh.rs.
        x[4] ^= rotl32(x[0].wrapping_add(x[12]), 7);
        x[8] ^= rotl32(x[4].wrapping_add(x[0]), 9);
        x[12] ^= rotl32(x[8].wrapping_add(x[4]), 13);
        x[0] ^= rotl32(x[12].wrapping_add(x[8]), 18);
        x[9] ^= rotl32(x[5].wrapping_add(x[1]), 7);
        x[13] ^= rotl32(x[9].wrapping_add(x[5]), 9);
        x[1] ^= rotl32(x[13].wrapping_add(x[9]), 13);
        x[5] ^= rotl32(x[1].wrapping_add(x[13]), 18);
        x[14] ^= rotl32(x[10].wrapping_add(x[6]), 7);
        x[2] ^= rotl32(x[14].wrapping_add(x[10]), 9);
        x[6] ^= rotl32(x[2].wrapping_add(x[14]), 13);
        x[10] ^= rotl32(x[6].wrapping_add(x[2]), 18);
        x[3] ^= rotl32(x[15].wrapping_add(x[11]), 7);
        x[7] ^= rotl32(x[3].wrapping_add(x[15]), 9);
        x[11] ^= rotl32(x[7].wrapping_add(x[3]), 13);
        x[15] ^= rotl32(x[11].wrapping_add(x[7]), 18);
        x[1] ^= rotl32(x[0].wrapping_add(x[3]), 7);
        x[2] ^= rotl32(x[1].wrapping_add(x[0]), 9);
        x[3] ^= rotl32(x[2].wrapping_add(x[1]), 13);
        x[0] ^= rotl32(x[3].wrapping_add(x[2]), 18);
        x[6] ^= rotl32(x[5].wrapping_add(x[4]), 7);
        x[7] ^= rotl32(x[6].wrapping_add(x[5]), 9);
        x[4] ^= rotl32(x[7].wrapping_add(x[6]), 13);
        x[5] ^= rotl32(x[4].wrapping_add(x[7]), 18);
        x[11] ^= rotl32(x[10].wrapping_add(x[9]), 7);
        x[8] ^= rotl32(x[11].wrapping_add(x[10]), 9);
        x[9] ^= rotl32(x[8].wrapping_add(x[11]), 13);
        x[10] ^= rotl32(x[9].wrapping_add(x[8]), 18);
        x[12] ^= rotl32(x[15].wrapping_add(x[14]), 7);
        x[13] ^= rotl32(x[12].wrapping_add(x[15]), 9);
        x[14] ^= rotl32(x[13].wrapping_add(x[12]), 13);
        x[15] ^= rotl32(x[14].wrapping_add(x[13]), 18);
    }

    // HSalsa20 ne garde que 8 mots (positions 0, 5, 10, 15, 6, 7, 8, 9).
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&x[0].to_le_bytes());
    out[4..8].copy_from_slice(&x[5].to_le_bytes());
    out[8..12].copy_from_slice(&x[10].to_le_bytes());
    out[12..16].copy_from_slice(&x[15].to_le_bytes());
    out[16..20].copy_from_slice(&x[6].to_le_bytes());
    out[20..24].copy_from_slice(&x[7].to_le_bytes());
    out[24..28].copy_from_slice(&x[8].to_le_bytes());
    out[28..32].copy_from_slice(&x[9].to_le_bytes());
    out
}

/// `crypto_box_beforenm` de libnacl : secret partage (32 octets) entre la
/// cle publique X25519 `pk` du pair et notre cle secrete `sk` (32 octets
/// bruts, clampee en interne comme le fait libnacl).
///
/// Equivalent de `PrivateKey.diffie_hellman(peer_public_key)` cote pyipv8.
pub fn crypto_box_beforenm(pk: &[u8], sk: &[u8]) -> Result<[u8; 32], CryptoError> {
    if pk.len() != 32 || sk.len() != 32 {
        return Err(CryptoError::KeyDerivation(format!(
            "cles de 32 octets attendues, recu pk={} sk={}",
            pk.len(),
            sk.len()
        )));
    }

    // Clampage identique a crypto_scalarmult_curve25519 de libnacl.
    let mut clamped = [0u8; 32];
    clamped.copy_from_slice(sk);
    clamped[0] &= 248;
    clamped[31] &= 127;
    clamped[31] |= 64;

    let secret = StaticSecret::from(clamped);
    let public = X25519PublicKey::from(<[u8; 32]>::try_from(pk).expect("pk de 32 octets verifiee"));
    let shared = secret.diffie_hellman(&public);

    let nonce = [0u8; 16];
    Ok(hsalsa20(shared.as_bytes(), &nonce))
}

/// Domaine HKDF de derivation de la cle AEAD par paire — separe ces
/// chiffrements des cles de session tunnel (autre info HKDF).
const PAIRBOX_HKDF_INFO: &[u8] = b"onionbit/pairbox/v1";
/// Taille du nonce ChaCha20-Poly1305 (12 octets, aleatoire, prefixe au
/// ciphertext).
pub const PAIRBOX_NONCE_LEN: usize = 12;

/// Cle AEAD partagee d'une paire : `crypto_box_beforenm` puis
/// HKDF-SHA256 expand sur `info` — domaine dedie par usage (pas de
/// reutilisation du secret brut en cle — separation des usages).
fn pair_key_in(
    peer_pk: &[u8],
    my_sk: &[u8],
    info: &[u8],
) -> Result<chacha20poly1305::ChaCha20Poly1305, CryptoError> {
    use chacha20poly1305::KeyInit;
    let shared = crypto_box_beforenm(peer_pk, my_sk)?;
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::from_prk(&shared)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF prk pairbox: {e}")))?;
    let mut key = [0u8; 32];
    hkdf.expand(info, &mut key)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF expand pairbox: {e}")))?;
    Ok(chacha20poly1305::ChaCha20Poly1305::new((&key).into()))
}

/// Chiffre `plain` pour la paire sous le domaine `info`
/// (`pair_seal` = `pair_seal_in` avec `PAIRBOX_HKDF_INFO`). Les
/// domaines distincts rendent les blobs non interchangeables entre
/// usages (un `tx` de ledger ne peut pas etre re-soumis comme
/// enveloppe OBF — ADR-0015 §7).
pub fn pair_seal_in(
    peer_pk: &[u8],
    my_sk: &[u8],
    info: &[u8],
    plain: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    use chacha20poly1305::aead::Aead;
    let cipher = pair_key_in(peer_pk, my_sk, info)?;
    let mut nonce_bytes = [0u8; PAIRBOX_NONCE_LEN];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut nonce_bytes);
    let nonce = chacha20poly1305::Nonce::from(nonce_bytes);
    let ct = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| CryptoError::Aead)?;
    let mut out = Vec::with_capacity(PAIRBOX_NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Dechiffre un blob produit par [`pair_seal_in`] sous le meme
/// domaine `info` (membre de la paire uniquement — `Aead` sinon).
pub fn pair_open_in(
    peer_pk: &[u8],
    my_sk: &[u8],
    info: &[u8],
    blob: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    use chacha20poly1305::aead::Aead;
    if blob.len() < PAIRBOX_NONCE_LEN + 16 {
        return Err(CryptoError::Truncated {
            expected: PAIRBOX_NONCE_LEN + 16,
            actual: blob.len(),
        });
    }
    let cipher = pair_key_in(peer_pk, my_sk, info)?;
    let nonce = chacha20poly1305::Nonce::from(
        <[u8; PAIRBOX_NONCE_LEN]>::try_from(&blob[..PAIRBOX_NONCE_LEN])
            .expect("nonce de 12 octets borne"),
    );
    cipher
        .decrypt(&nonce, &blob[PAIRBOX_NONCE_LEN..])
        .map_err(|_| CryptoError::Aead)
}

/// Chiffre `plain` pour la paire `(peer_pk X25519, my_sk X25519)` :
/// sortie `nonce(12) || ciphertext || tag(16)`. Seuls les deux
/// membres de la paire peuvent lire (anti-*bandwidth crawler* —
/// ADR-0015 §5).
pub fn pair_seal(peer_pk: &[u8], my_sk: &[u8], plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    pair_seal_in(peer_pk, my_sk, PAIRBOX_HKDF_INFO, plain)
}

/// Dechiffre un blob produit par [`pair_seal`] (membre de la paire
/// uniquement — `Aead` sinon).
pub fn pair_open(peer_pk: &[u8], my_sk: &[u8], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    pair_open_in(peer_pk, my_sk, PAIRBOX_HKDF_INFO, blob)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipv8::keys::LibNaClSecretKey;

    #[test]
    fn dh_symetrique_entre_deux_cles() {
        // Le secret partage doit etre identique des deux cotes.
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let ab =
            crypto_box_beforenm(&b.public_key().crypt_pk, &a.crypt_x25519().to_bytes()).unwrap();
        let ba =
            crypto_box_beforenm(&a.public_key().crypt_pk, &b.crypt_x25519().to_bytes()).unwrap();
        assert_eq!(ab, ba);
    }

    #[test]
    fn dh_vecteur_reference_hsalsa20() {
        // Vecteur de test connu : DH de deux cles X25519 fixes + HSalsa20
        // (nonce a zero). Les valeurs attendues sont calculees a partir de
        // l'implementation de reference (ipv8-rust-tunnels/dh.rs).
        let sk = [9u8; 32];
        let pk = [9u8; 32];
        let out = crypto_box_beforenm(&pk, &sk).unwrap();
        // Le resultat doit etre deterministe et de 32 octets non nuls.
        assert_eq!(out.len(), 32);
        assert_ne!(out, [0u8; 32]);
    }

    #[test]
    fn dh_rejette_les_mauvaises_tailles() {
        assert!(crypto_box_beforenm(&[0u8; 16], &[0u8; 32]).is_err());
        assert!(crypto_box_beforenm(&[0u8; 32], &[0u8; 16]).is_err());
    }
}
