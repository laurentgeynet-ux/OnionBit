// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Graine racine d'identite (ADR-0016, etape 48a).
//!
//! `IdentitySeed` : 32 octets aleatoires persistes dans
//! `identity_seed.bin` — racine unique de l'identite. Toutes les cles
//! du noeud sont derivees par HKDF-SHA256 a domaines separes, ce qui
//! rend les fichiers de cles derives (`ipv8_keypair.bin`,
//! `stealth_bridge.key`) de simples caches regenerables et permet la
//! phrase de recuperation BIP39 (etape 48b).
//!
//! Domaines de derivation :
//!
//! - `onionbit/identity/ipv8-crypt/v1` → `crypt_sk` X25519 ;
//! - `onionbit/identity/ipv8-sign/v1` → seed Ed25519 ;
//! - `onionbit/identity/bridge/v1` → secret statique du pont stealth
//!   (ADR-0017) — la phrase sauvegarde aussi les liens d'invitation.

use hkdf::Hkdf;
use rand::Rng;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::CryptoError;
use crate::ipv8::keys::LibNaClSecretKey;

/// Taille de la graine racine (octets).
pub const IDENTITY_SEED_LEN: usize = 32;

const INFO_IPV8_CRYPT: &[u8] = b"onionbit/identity/ipv8-crypt/v1";
const INFO_IPV8_SIGN: &[u8] = b"onionbit/identity/ipv8-sign/v1";
const INFO_BRIDGE: &[u8] = b"onionbit/identity/bridge/v1";

/// Graine racine de l'identite — zeroisee au drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct IdentitySeed {
    seed: [u8; IDENTITY_SEED_LEN],
}

impl IdentitySeed {
    /// Tire une graine aleatoire (RNG OS).
    pub fn generate() -> Self {
        let mut seed = [0u8; IDENTITY_SEED_LEN];
        rand::rng().fill_bytes(&mut seed);
        Self { seed }
    }

    /// Reconstruit depuis une forme binaire (fichier, phrase decodee).
    /// Toute autre taille que 32 octets est refusee.
    pub fn from_bytes(data: &[u8]) -> Result<Self, CryptoError> {
        if data.len() != IDENTITY_SEED_LEN {
            return Err(CryptoError::BadKey(format!(
                "graine d'identite : {IDENTITY_SEED_LEN} octets attendus, recu {}",
                data.len()
            )));
        }
        let mut seed = [0u8; IDENTITY_SEED_LEN];
        seed.copy_from_slice(data);
        Ok(Self { seed })
    }

    /// La graine brute — pour l'ecriture du fichier / l'encodage BIP39.
    pub fn as_bytes(&self) -> &[u8; IDENTITY_SEED_LEN] {
        &self.seed
    }

    /// Derive `len` octets sous le domaine `info` (HKDF-SHA256 expand).
    fn expand(&self, info: &[u8], out: &mut [u8]) -> Result<(), CryptoError> {
        Hkdf::<sha2::Sha256>::from_prk(&self.seed)
            .map_err(|_| CryptoError::KeyDerivation("hkdf seed".into()))?
            .expand(info, out)
            .map_err(|_| CryptoError::KeyDerivation("hkdf expand".into()))
    }

    /// Materiel X25519 de la cle maitresse IPv8 (`crypt_sk`).
    pub fn derive_ipv8_crypt(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.expand(INFO_IPV8_CRYPT, &mut out)
            .expect("32 octets < limite HKDF");
        out
    }

    /// Materiel Ed25519 de la cle maitresse IPv8 (seed de signature).
    pub fn derive_ipv8_sign(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.expand(INFO_IPV8_SIGN, &mut out)
            .expect("32 octets < limite HKDF");
        out
    }

    /// Cle maitresse IPv8 complete (`LibNaClSK:` = crypt_sk + sign seed).
    pub fn derive_keypair(&self) -> LibNaClSecretKey {
        LibNaClSecretKey::from_parts(self.derive_ipv8_crypt(), self.derive_ipv8_sign())
    }

    /// Secret statique du pont stealth (ADR-0017).
    pub fn derive_bridge_key(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.expand(INFO_BRIDGE, &mut out)
            .expect("32 octets < limite HKDF");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipv8::keys::LibNaClPublicKey;

    #[test]
    fn derivation_deterministe() {
        let seed = IdentitySeed::generate();
        let k1 = seed.derive_keypair().to_bin();
        let k2 = IdentitySeed::from_bytes(seed.as_bytes())
            .unwrap()
            .derive_keypair()
            .to_bin();
        assert_eq!(k1, k2, "meme graine -> meme keypair bit-exact");
    }

    #[test]
    fn domaines_distincts_materiel_distinct() {
        let seed = IdentitySeed::generate();
        let c = seed.derive_ipv8_crypt();
        let s = seed.derive_ipv8_sign();
        let b = seed.derive_bridge_key();
        assert_ne!(c, s);
        assert_ne!(c, b);
        assert_ne!(s, b);
    }

    #[test]
    fn keypair_derivee_cohere_cle_publique() {
        let seed = IdentitySeed::generate();
        let kp = seed.derive_keypair();
        // pk reconstruite depuis le bin prive = pk publique directe.
        let pk1 = kp.public_key();
        let kp2 = LibNaClSecretKey::from_bin(&kp.to_bin()).unwrap();
        assert_eq!(pk1.to_bin(), kp2.public_key().to_bin());
        // La signature du keypair derive verifie avec sa propre pk.
        let sig = kp.sign(b"test");
        assert!(LibNaClPublicKey::from_bin(&pk1.to_bin())
            .unwrap()
            .verify(b"test", &sig));
    }

    #[test]
    fn from_bytes_rejette_taille_invalide() {
        assert!(IdentitySeed::from_bytes(&[0u8; 31]).is_err());
        assert!(IdentitySeed::from_bytes(&[0u8; 33]).is_err());
        assert!(IdentitySeed::from_bytes(&[]).is_err());
    }
}
