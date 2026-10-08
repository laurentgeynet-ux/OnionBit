// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cles de session des tunnels IPv8 et crypto des cellules.
//!
//! Reference de verite :
//! `docs/reference_tribler/ipv8_rust_tunnels/dh.rs` (`generate_session_keys`,
//! `SessionKeys::encrypt_str`/`decrypt_str`, `crypto_auth`).
//!
//! - `generate_session_keys` : HKDF-SHA256 (modele "expand only",
//!   info = `"key_generation"`) sur le secret partage de
//!   [`crypto_box_beforenm`](super::dh::crypto_box_beforenm) -> 72 octets
//!   = `key_backward` (32) + `key_forward` (32) + `salt_backward` (4) +
//!   `salt_forward` (4) ;
//! - `encrypt_str`/`decrypt_str` : ChaCha20-Poly1305, nonce = sel (4o) +
//!   compteur (8o, big-endian), sortie = `compteur || ciphertext || tag` ;
//! - `crypto_auth` : HMAC-SHA512 tronque a 32 octets.

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};

use crate::error::CryptoError;

/// Info HKDF utilisee par IPv8 pour deriver les cles de session.
pub const HKDF_INFO_KEY_GENERATION: &[u8] = b"key_generation";
/// Taille du secret derive HKDF (72 octets).
const SESSION_KEY_MATERIAL_LEN: usize = 72;
/// Taille du sel (4 octets) et du compteur explicite (8 octets).
const SALT_LEN: usize = 4;
const COUNTER_LEN: usize = 8;
/// Taille du tag Poly1305.
const TAG_LEN: usize = 16;
/// Longueur minimale d'une cellule chiffree (compteur + tag).
const MIN_ENCRYPTED_LEN: usize = COUNTER_LEN + TAG_LEN;

/// Direction d'une cellule dans un circuit (vue de chaque noeud).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Sens "aller" : de l'initiateur vers la sortie.
    Forward,
    /// Sens "retour" : de la sortie vers l'initiateur.
    Backward,
}

/// Cles de session d'un saut de circuit.
#[derive(Debug, Clone)]
pub struct SessionKeys {
    /// Cle de chiffrement direction aller (32 octets).
    pub key_forward: [u8; 32],
    /// Cle de chiffrement direction retour (32 octets).
    pub key_backward: [u8; 32],
    /// Sel du nonce, direction aller (4 octets).
    pub salt_forward: [u8; 4],
    /// Sel du nonce, direction retour (4 octets).
    pub salt_backward: [u8; 4],
    /// Compteur explicite direction aller (demarre a 1, incremente avant usage).
    salt_explicit_forward: u64,
    /// Compteur explicite direction retour.
    salt_explicit_backward: u64,
}

/// Derive les cles de session depuis un secret partage
/// ([`crypto_box_beforenm`](super::dh::crypto_box_beforenm)).
///
/// Equivalent de `generate_session_keys` cote pyipv8.
pub fn generate_session_keys(shared_secret: &[u8]) -> Result<SessionKeys, CryptoError> {
    // HKDF "EXPAND_ONLY" de la reference (`ipv8_rust_tunnels` :
    // `set_hkdf_mode(EXPAND_ONLY)` + `set_hkdf_key(shared_secret)`) : le
    // secret partage est la PRK telle quelle, sans etape extract.
    // `Hkdf::from_prk` reproduit exactement cela.
    let hkdf = Hkdf::<sha2::Sha256>::from_prk(shared_secret)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF prk: {e}")))?;
    let mut key = [0u8; SESSION_KEY_MATERIAL_LEN];
    hkdf.expand(HKDF_INFO_KEY_GENERATION, &mut key)
        .map_err(|e| CryptoError::KeyDerivation(format!("HKDF expand: {e}")))?;

    let mut keys = SessionKeys {
        key_backward: [0u8; 32],
        key_forward: [0u8; 32],
        salt_backward: [0u8; 4],
        salt_forward: [0u8; 4],
        salt_explicit_backward: 1,
        salt_explicit_forward: 1,
    };
    keys.key_backward.copy_from_slice(&key[0..32]);
    keys.key_forward.copy_from_slice(&key[32..64]);
    keys.salt_backward.copy_from_slice(&key[64..68]);
    keys.salt_forward.copy_from_slice(&key[68..72]);
    Ok(keys)
}

fn chacha(key: &[u8; 32], salt: &[u8; 4], counter: u64) -> (ChaCha20Poly1305, Nonce) {
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..SALT_LEN].copy_from_slice(salt);
    nonce_bytes[SALT_LEN..].copy_from_slice(&counter.to_be_bytes());
    let key: &Key = key.into();
    let nonce: Nonce = nonce_bytes.into();
    (ChaCha20Poly1305::new(key), nonce)
}

impl SessionKeys {
    /// Chiffre `content` dans la direction donnee.
    ///
    /// Sortie : `compteur(8o) || ciphertext || tag(16o)`. Le compteur est
    /// incremente avant chaque appel (comme la reference, qui demarre a 1
    /// puis incremente avant le premier envoi -> premier compteur = 2).
    pub fn encrypt_str(
        &mut self,
        content: &[u8],
        direction: Direction,
    ) -> Result<Vec<u8>, CryptoError> {
        let (key, salt, counter) = match direction {
            Direction::Forward => {
                self.salt_explicit_forward += 1;
                (
                    &self.key_forward,
                    &self.salt_forward,
                    self.salt_explicit_forward,
                )
            }
            Direction::Backward => {
                self.salt_explicit_backward += 1;
                (
                    &self.key_backward,
                    &self.salt_backward,
                    self.salt_explicit_backward,
                )
            }
        };

        let (cipher, nonce) = chacha(key, salt, counter);
        let ciphertext = cipher
            .encrypt(&nonce, content)
            .map_err(|_| CryptoError::Aead)?;

        let mut out = Vec::with_capacity(COUNTER_LEN + ciphertext.len());
        out.extend_from_slice(&counter.to_be_bytes());
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Dechiffre une cellule au format `compteur(8o) || ciphertext || tag`.
    pub fn decrypt_str(
        &self,
        content: &[u8],
        direction: Direction,
    ) -> Result<Vec<u8>, CryptoError> {
        if content.len() < MIN_ENCRYPTED_LEN {
            return Err(CryptoError::Truncated {
                expected: MIN_ENCRYPTED_LEN,
                actual: content.len(),
            });
        }
        let (key, salt) = match direction {
            Direction::Forward => (&self.key_forward, &self.salt_forward),
            Direction::Backward => (&self.key_backward, &self.salt_backward),
        };
        let mut counter = [0u8; COUNTER_LEN];
        counter.copy_from_slice(&content[..COUNTER_LEN]);
        let (cipher, nonce) = chacha(key, salt, u64::from_be_bytes(counter));
        cipher
            .decrypt(&nonce, &content[COUNTER_LEN..])
            .map_err(|_| CryptoError::Aead)
    }
}

/// `crypto_auth` : HMAC-SHA512 tronque a 32 octets.
pub fn crypto_auth(key: &[u8], message: &[u8]) -> Result<[u8; 32], CryptoError> {
    let mut mac = <Hmac<sha2::Sha512> as KeyInit>::new_from_slice(key)
        .map_err(|e| CryptoError::KeyDerivation(format!("HMAC: {e}")))?;
    mac.update(message);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&tag[..32]);
    Ok(out)
}

/// Verification a temps constant d'un tag `crypto_auth`.
pub fn crypto_auth_verify(tag: &[u8], key: &[u8], message: &[u8]) -> bool {
    let Ok(expected) = crypto_auth(key, message) else {
        return false;
    };
    use subtle::ConstantTimeEq;
    expected.ct_eq(tag).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_session_keys_tailles_et_directions() {
        let keys = generate_session_keys(&[42u8; 32]).unwrap();
        assert_eq!(keys.key_forward.len(), 32);
        assert_eq!(keys.key_backward.len(), 32);
        assert_eq!(keys.salt_forward.len(), 4);
        assert_eq!(keys.salt_backward.len(), 4);
        // Forward et backward doivent etre distincts.
        assert_ne!(keys.key_forward, keys.key_backward);
    }

    #[test]
    fn encrypt_decrypt_aller_retour() {
        let mut enc = generate_session_keys(&[7u8; 32]).unwrap();
        let dec = generate_session_keys(&[7u8; 32]).unwrap();
        let msg = b"cellule de tunnel ipv8";
        let ct = enc.encrypt_str(msg, Direction::Forward).unwrap();
        // Format : compteur(8) + ciphertext + tag(16).
        assert_eq!(ct.len(), COUNTER_LEN + msg.len() + TAG_LEN);
        // Premier envoi : compteur = 2 (1 + increment avant usage).
        assert_eq!(&ct[..8], &2u64.to_be_bytes());
        let pt = dec.decrypt_str(&ct, Direction::Forward).unwrap();
        assert_eq!(pt, msg);
    }

    #[test]
    fn decrypt_rejette_le_contenu_trop_court() {
        let dec = generate_session_keys(&[7u8; 32]).unwrap();
        assert!(dec.decrypt_str(b"court", Direction::Forward).is_err());
    }

    #[test]
    fn decrypt_rejette_un_tag_invalide() {
        let mut enc = generate_session_keys(&[7u8; 32]).unwrap();
        let dec = generate_session_keys(&[7u8; 32]).unwrap();
        let mut ct = enc.encrypt_str(b"msg", Direction::Forward).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 1;
        assert!(dec.decrypt_str(&ct, Direction::Forward).is_err());
    }

    #[test]
    fn crypto_auth_vecteur_et_verification() {
        let tag = crypto_auth(b"cle", b"message").unwrap();
        assert_eq!(tag.len(), 32);
        assert!(crypto_auth_verify(&tag, b"cle", b"message"));
        assert!(!crypto_auth_verify(&tag, b"autre-cle", b"message"));
        assert!(!crypto_auth_verify(&tag, b"cle", b"autre-message"));
    }
}
