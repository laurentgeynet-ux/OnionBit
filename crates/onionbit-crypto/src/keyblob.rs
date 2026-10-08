// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Blob protege par mot de passe — export/import de secret
//! d'identite (cle IPv8 `LibNaCLSK:`) entre devices.
//!
//! Format `OBID` :
//! ```text
//! magic(4) || version(1) || sel(16) || nonce(12) || ciphertext || tag(16)
//! ```
//!
//! La cle AEAD est `argon2id(mot_de_passe, sel)` aux parametres
//! recommandes RFC 9106 (m = 19 Mio, t = 2, p = 1) — le sel aleatoire
//! et le nonce aleatoire rendent chaque export unique ; le tag
//! Poly1305 authentifie le tout (un mauvais mot de passe echoue au
//! decrypt, un blob tronque est rejete avant tout travail).

use crate::CryptoError;

/// Magic des blobs proteges par mot de passe.
pub const KEYBLOB_MAGIC: &[u8; 4] = b"OBID";
/// Magic du blob de graine verrouillee au repos (ADR-0016) : meme
/// enveloppe argon2id+AEAD, magic distinct pour qu'un `OBID` colle a
/// la place de `identity_seed.bin` soit refuse a l'ouverture.
pub const SEEDBLOB_MAGIC: &[u8; 4] = b"OBSK";
/// Version du format.
const KEYBLOB_VERSION: u8 = 1;
/// Sel argon2id (aleatoire, par blob).
const SALT_LEN: usize = 16;
/// Nonce ChaCha20-Poly1305 (aleatoire, prefixe au ciphertext).
const NONCE_LEN: usize = 12;
/// En-tete en clair : magic + version + sel + nonce.
const HEADER_LEN: usize = 4 + 1 + SALT_LEN + NONCE_LEN;
/// Taille max d'un blob accepte a l'ouverture — une cle privee tient
/// en ~100 octets ; la borne evite de decoder des collages arbitraires.
pub const KEYBLOB_MAX: usize = 64 * 1024;

/// `true` si `blob` commence par le magic `OBID` (permets a
/// l'appelant de distinguer une cle brute d'un blob protege).
pub fn keyblob_is_sealed(blob: &[u8]) -> bool {
    blob.starts_with(KEYBLOB_MAGIC)
}

/// `true` si `blob` commence par le magic `OBSK`.
pub fn seedblob_is_sealed(blob: &[u8]) -> bool {
    blob.starts_with(SEEDBLOB_MAGIC)
}

/// Cle AEAD derivee du mot de passe : argon2id RFC 9106
/// (m = 19 Mio, t = 2, p = 1 — le cout memoire est la vraie barriere
/// contre le brute-force GPU des exports interceptes).
fn kdf(
    password: &[u8],
    salt: &[u8; SALT_LEN],
) -> Result<chacha20poly1305::ChaCha20Poly1305, CryptoError> {
    use argon2::Argon2;
    use chacha20poly1305::KeyInit;
    let params = argon2::Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|e| CryptoError::KeyDerivation(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| CryptoError::KeyDerivation(format!("argon2id: {e}")))?;
    Ok(chacha20poly1305::ChaCha20Poly1305::new((&key).into()))
}

/// Chiffre `plain` sous `password` → blob `OBID…`.
/// `password` vide = refuse (un export non protege est le choix de
/// l'appelant, pas un accident de ce helper).
pub fn keyblob_seal(password: &[u8], plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    seal_with(KEYBLOB_MAGIC, password, plain)
}

/// Chiffre la graine d'identite sous `password` → blob `OBSK…`
/// (verrouillage au repos ADR-0016).
pub fn seedblob_seal(password: &[u8], plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    seal_with(SEEDBLOB_MAGIC, password, plain)
}

fn seal_with(magic: &[u8; 4], password: &[u8], plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use chacha20poly1305::aead::Aead;
    if password.is_empty() {
        return Err(CryptoError::BadKey("mot de passe vide".into()));
    }
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut salt);
    rand::Rng::fill_bytes(&mut rand::rng(), &mut nonce_bytes);
    let cipher = kdf(password, &salt)?;
    let nonce = chacha20poly1305::Nonce::from(nonce_bytes);
    let ct = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| CryptoError::Aead)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(magic);
    out.push(KEYBLOB_VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Dechiffre un blob `OBID…` sous `password` — `Truncated` si trop
/// court, `BadKey` si magic/version inconnus ou blob hors borne,
/// `Aead` si mot de passe faux ou contenu altere.
pub fn keyblob_open(password: &[u8], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    open_with(KEYBLOB_MAGIC, "OBID", password, blob)
}

/// Dechiffre un blob `OBSK…` sous `password` — memes erreurs que
/// [`keyblob_open`].
pub fn seedblob_open(password: &[u8], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    open_with(SEEDBLOB_MAGIC, "OBSK", password, blob)
}

fn open_with(
    magic: &[u8; 4],
    name: &str,
    password: &[u8],
    blob: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    use chacha20poly1305::aead::Aead;
    if blob.len() > KEYBLOB_MAX {
        return Err(CryptoError::BadKey("blob hors borne".into()));
    }
    if blob.len() < HEADER_LEN + 16 {
        return Err(CryptoError::Truncated {
            expected: HEADER_LEN + 16,
            actual: blob.len(),
        });
    }
    if !blob.starts_with(magic) {
        return Err(CryptoError::BadKey(format!("magic {name} absent")));
    }
    if blob[4] != KEYBLOB_VERSION {
        return Err(CryptoError::BadKey(format!("version {name} inconnue")));
    }
    let salt: &[u8; SALT_LEN] = blob[5..5 + SALT_LEN]
        .try_into()
        .map_err(|_| CryptoError::BadKey("sel invalide".into()))?;
    let nonce = chacha20poly1305::Nonce::from(
        <[u8; NONCE_LEN]>::try_from(&blob[5 + SALT_LEN..HEADER_LEN])
            .map_err(|_| CryptoError::BadKey("nonce invalide".into()))?,
    );
    kdf(password, salt)?
        .decrypt(&nonce, &blob[HEADER_LEN..])
        .map_err(|_| CryptoError::Aead)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour_et_mauvais_mot_de_passe() {
        let blob = keyblob_seal(b"phrase secrete", b"LibNaCLSK:...").unwrap();
        assert!(keyblob_is_sealed(&blob));
        assert_eq!(
            keyblob_open(b"phrase secrete", &blob).unwrap(),
            b"LibNaCLSK:..."
        );
        // Mauvais mot de passe : echec AEAD, jamais de plaintext.
        assert!(keyblob_open(b"autre", &blob).is_err());
        // Deux exports du meme plaintext = blobs distincts (sel + nonce).
        let blob2 = keyblob_seal(b"phrase secrete", b"LibNaCLSK:...").unwrap();
        assert_ne!(blob, blob2);
    }

    #[test]
    fn formes_rejetees() {
        // Magic absent, tronque, version inconnue, hors borne, mdp vide.
        assert!(keyblob_seal(b"", b"x").is_err());
        assert!(keyblob_open(b"x", b"XXXX....").is_err());
        assert!(keyblob_open(b"x", b"OBID").is_err());
        let mut v1 = keyblob_seal(b"p", b"data").unwrap();
        v1[4] = 9;
        assert!(keyblob_open(b"p", &v1).is_err());
        let huge = vec![b'O', b'B', b'I', b'D']
            .into_iter()
            .chain(std::iter::repeat_n(0u8, KEYBLOB_MAX))
            .collect::<Vec<_>>();
        assert!(keyblob_open(b"p", &huge).is_err());
        // Corruption d'un octet du ciphertext → tag Poly1305 invalide.
        let mut corrupt = keyblob_seal(b"p", b"data").unwrap();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(keyblob_open(b"p", &corrupt).is_err());
    }

    #[test]
    fn obsk_aller_retour() {
        let seed = [0xABu8; 32];
        let blob = seedblob_seal(b"mot de passe", &seed).unwrap();
        assert!(seedblob_is_sealed(&blob));
        assert!(!keyblob_is_sealed(&blob));
        assert_eq!(seedblob_open(b"mot de passe", &blob).unwrap(), seed);
        // Mauvais mot de passe : echec AEAD uniforme, jamais de clair.
        assert!(seedblob_open(b"autre", &blob).is_err());
        // Deux scellements = sel + nonce distincts.
        let blob2 = seedblob_seal(b"mot de passe", &seed).unwrap();
        assert_ne!(blob, blob2);
    }

    #[test]
    fn obsk_rejets_hostiles() {
        let blob = seedblob_seal(b"p", &[0x42; 32]).unwrap();
        // Un OBID pose a la place d'un OBSK : magic refuse.
        let obid = keyblob_seal(b"p", &[0x42; 32]).unwrap();
        assert!(seedblob_open(b"p", &obid).is_err());
        assert!(keyblob_open(b"p", &blob).is_err());
        // Tronque, version inconnue, hors borne, corruption, mdp vide.
        assert!(seedblob_open(b"p", &blob[..10]).is_err());
        let mut v9 = blob.clone();
        v9[4] = 9;
        assert!(seedblob_open(b"p", &v9).is_err());
        let huge = vec![b'O', b'S', b'K']
            .into_iter()
            .chain(std::iter::repeat_n(0u8, KEYBLOB_MAX))
            .collect::<Vec<_>>();
        assert!(seedblob_open(b"p", &huge).is_err());
        let mut corrupt = blob.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(seedblob_open(b"p", &corrupt).is_err());
        assert!(seedblob_seal(b"", &[0x42; 32]).is_err());
    }
}
