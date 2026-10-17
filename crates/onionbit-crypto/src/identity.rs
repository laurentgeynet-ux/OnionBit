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
//!   (ADR-0017) — la phrase sauvegarde aussi les liens d'invitation ;
//! - `onionbit/identity/tor-hs/v1` → seed Ed25519 du service `.onion`
//!   d'identite (ADR-0029) — jamais `ipv8-sign` direct : deux plans
//!   non liables.

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
const INFO_TOR_HS: &[u8] = b"onionbit/identity/tor-hs/v1";

/// Domaine de la signature liant une adresse `.onion` a une identite
/// IPv8 — personas/canaux *publics* uniquement, opt-in explicite
/// (ADR-0029 §2) : l'adresse auto-certifiee prouve que le detenteur
/// de la cle de canal controle aussi le service cache.
const ONION_BIND_DOMAIN: &[u8] = b"onionbit/onion-bind/v1";

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

    /// Seed Ed25519 du service `.onion` d'identite (ADR-0029 etape
    /// 122) — domaine separe de `ipv8-sign` : l'adresse `.onion` ne
    /// revele ni n'implique la cle publique du plan OnionBit.
    pub fn derive_tor_hs(&self) -> [u8; 32] {
        derive_tor_hs_seed(self.as_bytes())
    }
}

/// Derivation `tor-hs/v1` sur une racine arbitraire — en `Seeded` la
/// racine est la graine ; en `Legacy` c'est `SHA-256(keypair)` (le
/// `store_root` de [`IdentityMaterial`] cote core), ce qui donne une
/// adresse `.onion` stable meme aux installations anterieures a
/// ADR-0016.
pub fn derive_tor_hs_seed(root: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<sha2::Sha256>::from_prk(root)
        .expect("32 octets > HKDF min")
        .expand(INFO_TOR_HS, &mut out)
        .expect("32 octets < limite HKDF");
    out
}

/// Message signe pour la liaison identite → `.onion` : domaine +
/// hote (sans scheme ni port — l'adresse v3 est l'identite).
fn onion_bind_message(onion_host: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(ONION_BIND_DOMAIN.len() + 1 + onion_host.len());
    msg.extend_from_slice(ONION_BIND_DOMAIN);
    msg.push(0);
    msg.extend_from_slice(onion_host.as_bytes());
    msg
}

/// Signe la liaison `cle IPv8 → adresse .onion` — a exposer dans les
/// personas/canaux publics (opt-in ; jamais de liaison implicite).
pub fn sign_onion_address(kp: &LibNaClSecretKey, onion_host: &str) -> [u8; 64] {
    kp.sign(&onion_bind_message(onion_host))
}

/// Verifie une liaison `sign_onion_address` produite par la pk d'un
/// persona/canal public.
pub fn verify_onion_address(
    pk: &crate::ipv8::keys::LibNaClPublicKey,
    onion_host: &str,
    signature: &[u8],
) -> bool {
    pk.verify(&onion_bind_message(onion_host), signature)
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
        let t = seed.derive_tor_hs();
        assert_ne!(c, s);
        assert_ne!(c, b);
        assert_ne!(s, b);
        assert_ne!(t, s);
        assert_ne!(t, c);
        assert_ne!(t, b);
    }

    #[test]
    fn tor_hs_deterministe_et_racine_libre() {
        let seed = IdentitySeed::generate();
        let a = seed.derive_tor_hs();
        let b = IdentitySeed::from_bytes(seed.as_bytes())
            .unwrap()
            .derive_tor_hs();
        assert_eq!(a, b, "meme graine -> meme service .onion");
        // La variante a racine libre (legacy : SHA-256(keypair))
        // partage le meme domaine — jamais la valeur de la racine.
        let root = [9u8; 32];
        let d = derive_tor_hs_seed(&root);
        assert_ne!(d, root);
        assert_eq!(d, derive_tor_hs_seed(&root));
    }

    #[test]
    fn onion_binding_signe_et_verifie() {
        let seed = IdentitySeed::generate();
        let kp = seed.derive_keypair();
        let pk = LibNaClPublicKey::from_bin(&kp.public_key().to_bin()).unwrap();
        let host = "wzftlyphv4tl35qbnw7t2bjjvbd75utmpo5fgjkdivizqirco6vvsvyd.onion";
        let sig = sign_onion_address(&kp, host);
        assert!(verify_onion_address(&pk, host, &sig));
        // Autre hote, autre cle, troncation → refus.
        assert!(!verify_onion_address(
            &pk,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.onion",
            &sig
        ));
        let other = IdentitySeed::generate().derive_keypair();
        let pk2 = LibNaClPublicKey::from_bin(&other.public_key().to_bin()).unwrap();
        assert!(!verify_onion_address(&pk2, host, &sig));
        assert!(!verify_onion_address(&pk, host, &sig[..32]));
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
