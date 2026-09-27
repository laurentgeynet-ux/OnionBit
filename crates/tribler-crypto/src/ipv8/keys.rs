//! Cles IPv8 au format "LibNaCL dual" (courbe `curve25519` de pyipv8).
//!
//! Reference de verite :
//! `docs/reference_tribler/ipv8_rust_tunnels/keys.rs` (copie locale du
//! repo officiel `Tribler/ipv8-rust-tunnels`, qui fournit le code Rust
//! utilise par le `OpenSSLPK`/`OpenSSLSK` de pyipv8).
//!
//! Format filaire :
//!
//! - cle publique : `"LibNaCLPK:"` (10 octets) + `crypt_pk` X25519 (32
//!   octets) + `vk` Ed25519 (32 octets) = 74 octets ;
//! - cle privee : `"LibNaCLSK:"` (10 octets) + `crypt_sk` X25519 (32
//!   octets) + `seed` Ed25519 (32 octets) = 74 octets ;
//! - signature : Ed25519 pure (64 octets) ;
//! - `mid` d'un pair : SHA-1 de `key_to_bin()` de sa cle publique.

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use crate::error::CryptoError;
use crate::hash;

/// Prefixe des cles publiques LibNaCL.
pub const LIBNACL_PK_PREFIX: &[u8] = b"LibNaCLPK:";
/// Prefixe des cles privees LibNaCL.
pub const LIBNACL_SK_PREFIX: &[u8] = b"LibNaCLSK:";
/// Taille totale d'une cle publique serialisee (prefixe + 2 x 32 octets).
pub const LIBNACL_PK_BIN_LEN: usize = 10 + 32 + 32;
/// Taille totale d'une cle privee serialisee (prefixe + 2 x 32 octets).
pub const LIBNACL_SK_BIN_LEN: usize = 10 + 32 + 32;
/// Taille d'une signature Ed25519.
pub const SIGNATURE_LENGTH: usize = 64;

/// Cle publique IPv8 (curve25519 : crypt_pk X25519 + vk Ed25519).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibNaClPublicKey {
    /// Cle publique X25519 (utilisee pour le chiffrement / DH des tunnels).
    pub crypt_pk: [u8; 32],
    /// Cle de verification Ed25519 (utilisee pour les signatures).
    pub vk: [u8; 32],
}

impl LibNaClPublicKey {
    /// Parse une cle publique depuis sa forme binaire IPv8.
    pub fn from_bin(data: &[u8]) -> Result<Self, CryptoError> {
        if !data.starts_with(LIBNACL_PK_PREFIX) || data.len() < LIBNACL_PK_BIN_LEN {
            return Err(CryptoError::BadKey(format!(
                "cle publique LibNaCL attendue ({} octets, prefixe LibNaCLPK:), recu {} octets",
                LIBNACL_PK_BIN_LEN,
                data.len()
            )));
        }
        let mut crypt_pk = [0u8; 32];
        let mut vk = [0u8; 32];
        crypt_pk.copy_from_slice(&data[10..42]);
        vk.copy_from_slice(&data[42..74]);
        Ok(Self { crypt_pk, vk })
    }

    /// Serialise la cle au format binaire IPv8 (`LibNaCLPK:` + pk + vk).
    pub fn to_bin(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LIBNACL_PK_BIN_LEN);
        out.extend_from_slice(LIBNACL_PK_PREFIX);
        out.extend_from_slice(&self.crypt_pk);
        out.extend_from_slice(&self.vk);
        out
    }

    /// Verifie une signature Ed25519 sur `msg`.
    pub fn verify(&self, msg: &[u8], signature: &[u8]) -> bool {
        let Ok(sig) = <&[u8; 64]>::try_from(signature).map(ed25519_dalek::Signature::from_bytes)
        else {
            return false;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&self.vk) else {
            return false;
        };
        vk.verify(msg, &sig).is_ok()
    }

    /// `mid` du pair : SHA-1 de la cle serialisee.
    pub fn mid(&self) -> [u8; 20] {
        hash::ipv8_mid(&self.to_bin())
    }

    /// Cle publique X25519 brute (pour `diffie_hellman` cote pair).
    pub fn crypt_x25519(&self) -> X25519PublicKey {
        X25519PublicKey::from(self.crypt_pk)
    }

    /// Longueur de signature pour ce type de cle (64 octets, Ed25519).
    pub fn signature_length(&self) -> usize {
        SIGNATURE_LENGTH
    }
}

/// Cle privee IPv8 (curve25519 : crypt_sk X25519 + seed Ed25519).
pub struct LibNaClSecretKey {
    crypt_sk: StaticSecret,
    sign: SigningKey,
}

impl core::fmt::Debug for LibNaClSecretKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LibNaClSecretKey").finish_non_exhaustive()
    }
}

impl LibNaClSecretKey {
    /// Genere une nouvelle paire (equivalent de `generate_key("curve25519")`
    /// cote pyipv8 : Ed25519 + X25519 independants).
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        Self {
            crypt_sk: StaticSecret::random_from_rng(OsRng),
            sign: SigningKey::from_bytes(&seed),
        }
    }

    /// Parse une cle privee depuis sa forme binaire IPv8.
    pub fn from_bin(data: &[u8]) -> Result<Self, CryptoError> {
        if !data.starts_with(LIBNACL_SK_PREFIX) || data.len() < LIBNACL_SK_BIN_LEN {
            return Err(CryptoError::BadKey(format!(
                "cle privee LibNaCL attendue ({} octets, prefixe LibNaCLSK:), recu {} octets",
                LIBNACL_SK_BIN_LEN,
                data.len()
            )));
        }
        let mut crypt_sk = [0u8; 32];
        let mut seed = [0u8; 32];
        crypt_sk.copy_from_slice(&data[10..42]);
        seed.copy_from_slice(&data[42..74]);
        Ok(Self {
            crypt_sk: StaticSecret::from(crypt_sk),
            sign: SigningKey::from_bytes(&seed),
        })
    }

    /// Serialise la cle privee au format binaire IPv8 (`LibNaCLSK:` + sk + seed).
    pub fn to_bin(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LIBNACL_SK_BIN_LEN);
        out.extend_from_slice(LIBNACL_SK_PREFIX);
        out.extend_from_slice(&self.crypt_sk.to_bytes());
        out.extend_from_slice(&self.sign.to_bytes());
        out
    }

    /// Cle publique correspondante.
    pub fn public_key(&self) -> LibNaClPublicKey {
        LibNaClPublicKey {
            crypt_pk: X25519PublicKey::from(&self.crypt_sk).to_bytes(),
            vk: self.sign.verifying_key().to_bytes(),
        }
    }

    /// Signe `msg` avec la cle Ed25519 (64 octets de signature).
    pub fn sign(&self, msg: &[u8]) -> [u8; SIGNATURE_LENGTH] {
        self.sign.sign(msg).to_bytes()
    }

    /// Cle secrete X25519 brute (pour le DH des tunnels).
    pub fn crypt_x25519(&self) -> &StaticSecret {
        &self.crypt_sk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_serialization_cle_privee() {
        let sk = LibNaClSecretKey::generate();
        let bin = sk.to_bin();
        assert_eq!(bin.len(), LIBNACL_SK_BIN_LEN);
        assert!(bin.starts_with(LIBNACL_SK_PREFIX));
        let sk2 = LibNaClSecretKey::from_bin(&bin).unwrap();
        assert_eq!(sk2.to_bin(), bin);
        assert_eq!(sk2.public_key(), sk.public_key());
    }

    #[test]
    fn generation_serialization_cle_publique() {
        let sk = LibNaClSecretKey::generate();
        let pk = sk.public_key();
        let bin = pk.to_bin();
        assert_eq!(bin.len(), LIBNACL_PK_BIN_LEN);
        assert!(bin.starts_with(LIBNACL_PK_PREFIX));
        let pk2 = LibNaClPublicKey::from_bin(&bin).unwrap();
        assert_eq!(pk2, pk);
    }

    #[test]
    fn signature_verification_aller_retour() {
        let sk = LibNaClSecretKey::generate();
        let msg = b"paquet ipv8 de test";
        let sig = sk.sign(msg);
        assert_eq!(sig.len(), SIGNATURE_LENGTH);
        let pk = sk.public_key();
        assert!(pk.verify(msg, &sig));
        assert!(!pk.verify(b"autre message", &sig));
    }

    #[test]
    fn mid_est_sha1_de_la_cle_publique() {
        let sk = LibNaClSecretKey::generate();
        let pk = sk.public_key();
        assert_eq!(pk.mid(), hash::sha1(&pk.to_bin()));
    }

    #[test]
    fn from_bin_rejette_les_mauvaises_cles() {
        assert!(LibNaClPublicKey::from_bin(b"trop-court").is_err());
        assert!(LibNaClSecretKey::from_bin(b"LibNaCLSK:trop-court").is_err());
        let mut bad = vec![0u8; LIBNACL_PK_BIN_LEN];
        bad[..10].copy_from_slice(b"BadPrefix:");
        assert!(LibNaClPublicKey::from_bin(&bad).is_err());
    }
}
