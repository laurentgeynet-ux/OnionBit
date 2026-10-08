// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Manifest prive `manifest.obm` — catalogue JSON chiffre des
//! telechargements prives (ADR-0018, etape 62).
//!
//! La base publique `downloads` ne conserve des lignes privees que
//! sous cle opaque (`HMAC(K_names, infohash)`) avec `name`,
//! `source_uri` et `torrent_data` vides : les metadonnees reelles
//! vivent ici, sous AEAD `K_manifest = HKDF(K_store, "manifest")`.
//!
//! Layout : `nonce(12) ‖ AEAD(K_manifest, nonce, aad="obm/manifest")
//! 〔"OBM" ‖ version(1) ‖ json〕` — convention blobs portables
//! (magic + version + nonce aleatoire + borne a l'ouverture).
//!
//! Persistance atomique (`store_manifest`) : ecriture en `.obm.tmp`,
//! rotation du courant vers `.obm.bak`, renommage, puis fsync du
//! dossier — a tout instant il existe un manifest exploitable. A
//! l'ouverture (`load_manifest`) : le courant d'abord, sinon `.bak`.
//! La reconstruction depuis les sceaux `scan_ct` des `.obd` orphelins
//! est la responsabilite du core (etape 62) — ce module reste un
//! codec byte-level sans connaissance du contenu JSON.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::Nonce;
use rand::Rng;
use thiserror::Error;

use crate::obdfile::{PrivateStoreKeys, NONCE_LEN, TAG_LEN};

/// Magic + version du clair avant AEAD.
const MAGIC: &[u8; 3] = b"OBM";
const VERSION: u8 = 1;
/// Donnees authentifiees liees a la fonction (un `hdr_ct` de fichier
/// `OBD` colle ici echoue au decrypt).
const AAD_MANIFEST: &[u8] = b"obm/manifest";

/// Borne du catalogue en clair a l'ouverture — un manifest corrompu
/// ou ennemi ne doit pas forcer une allocation demesuree (convention
/// `OB*` : borne de taille a l'ouverture). 8 Mio ≈ des dizaines de
/// milliers d'entrees privees, largement au-dessus des usages.
const MAX_PLAIN_LEN: usize = 8 << 20;
/// Borne du fichier sur disque (nonce ‖ en-tete clair ‖ ct).
const MAX_FILE_LEN: u64 = (NONCE_LEN + 4 + MAX_PLAIN_LEN + TAG_LEN) as u64;

/// Erreur du manifest prive.
#[derive(Debug, Error)]
pub enum ObmError {
    /// Ouverture refusee — uniforme (mauvaise cle, corruption, troncature,
    /// depassement de borne) : aucun oracle différencie.
    #[error("ouverture OBM refusee (cle, format ou troncature)")]
    Open,
    /// E/S sous-jacente.
    #[error("E/S OBM: {0}")]
    Io(#[from] io::Error),
}

/// Scelle le clair JSON du manifest : `nonce ‖ AEAD〔"OBM"‖v‖plain〕`.
pub fn seal_manifest(keys: &PrivateStoreKeys, plain: &[u8]) -> Vec<u8> {
    let cipher = keys.manifest_cipher();
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let mut body = Vec::with_capacity(4 + plain.len());
    body.extend_from_slice(MAGIC);
    body.push(VERSION);
    body.extend_from_slice(plain);
    let n = Nonce::from(nonce);
    let ct = cipher
        .encrypt(
            &n,
            Payload {
                msg: &body,
                aad: AAD_MANIFEST,
            },
        )
        .expect("AEAD encrypt n'echoue pas");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// Ouvre un manifest scelle et retourne le clair JSON (sans le
/// prefixe `"OBM"‖v`). `Open` uniforme sur toute corruption.
pub fn open_manifest(keys: &PrivateStoreKeys, blob: &[u8]) -> Result<Vec<u8>, ObmError> {
    if blob.len() < NONCE_LEN + 4 + TAG_LEN || blob.len() as u64 > MAX_FILE_LEN {
        return Err(ObmError::Open);
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let mut n = [0u8; NONCE_LEN];
    n.copy_from_slice(nonce);
    let plain = keys
        .manifest_cipher()
        .decrypt(
            &Nonce::from(n),
            Payload {
                msg: ct,
                aad: AAD_MANIFEST,
            },
        )
        .map_err(|_| ObmError::Open)?;
    if plain.len() < 4 || &plain[..3] != MAGIC || plain[3] != VERSION {
        return Err(ObmError::Open);
    }
    Ok(plain[4..].to_vec())
}

/// Chemin `.bak` associe a un manifest.
fn bak_path(path: &Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".bak");
    p.into()
}

/// Chemin temporaire de l'ecriture atomique.
fn tmp_path(path: &Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".tmp");
    p.into()
}

/// fsync du dossier parent (durabilite des renommages sous
/// Linux/macOS ; no-op silencieux ailleurs).
fn fsync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Reecriture atomique du manifest : `tmp` → rotation vers `.bak` →
/// renommage → fsync du dossier. A chaque instant, `manifest.obm` ou
/// `.bak` est un blob complet et valide.
pub fn store_manifest(path: &Path, keys: &PrivateStoreKeys, plain: &[u8]) -> Result<(), ObmError> {
    let tmp = tmp_path(path);
    let bak = bak_path(path);
    // fsync du fichier avant renommage : un crash entre rename et
    // sync ne doit pas laisser un `.obm` page-cache fantome.
    // (`sync_all` exige un handle ouvert en ecriture sous Windows.)
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&seal_manifest(keys, plain))?;
    f.sync_all()?;
    drop(f);
    if path.exists() {
        fs::rename(path, &bak)?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => {}
        Err(e) => {
            // Renommage final impossible : on restaure le courant
            // depuis `.bak` pour ne jamais laisser le dossier sans
            // manifest.
            if bak.exists() {
                let _ = fs::rename(&bak, path);
            }
            return Err(e.into());
        }
    }
    if let Some(dir) = path.parent() {
        fsync_dir(dir);
    }
    Ok(())
}

/// Charge le manifest : le courant d'abord, sinon la rotation `.bak`.
/// `Ok(None)` = aucun manifest exploitable (absent ou double perte —
/// le core bascule alors sur le balayage `scan_ct`).
pub fn load_manifest(path: &Path, keys: &PrivateStoreKeys) -> Result<Option<Vec<u8>>, ObmError> {
    match fs::read(path) {
        Ok(blob) => match open_manifest(keys, &blob) {
            Ok(plain) => return Ok(Some(plain)),
            Err(ObmError::Open) => {
                tracing::warn!("manifest.obm corrompu — tentative .bak");
            }
            Err(e) => return Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let bak = bak_path(path);
    match fs::read(&bak) {
        Ok(blob) => match open_manifest(keys, &blob) {
            Ok(plain) => {
                tracing::warn!("manifest restaure depuis manifest.obm.bak");
                Ok(Some(plain))
            }
            Err(ObmError::Open) => Ok(None),
            Err(e) => Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> PrivateStoreKeys {
        PrivateStoreKeys::from_root(&[7u8; 32])
    }

    #[test]
    fn seal_open_roundtrip() {
        let blob = seal_manifest(&keys(), br#"{"entries":[]}"#);
        assert_eq!(open_manifest(&keys(), &blob).unwrap(), br#"{"entries":[]}"#);
    }

    #[test]
    fn open_rejette_mauvaise_cle_et_corruption() {
        let blob = seal_manifest(&keys(), b"{}");
        // Mauvaise cle.
        let other = PrivateStoreKeys::from_root(&[9u8; 32]);
        assert!(matches!(open_manifest(&other, &blob), Err(ObmError::Open)));
        // Octet corrompu dans le ct.
        let mut bad = blob.clone();
        let n = bad.len();
        bad[n - 1] ^= 0x55;
        assert!(matches!(open_manifest(&keys(), &bad), Err(ObmError::Open)));
        // Troncature.
        assert!(matches!(
            open_manifest(&keys(), &blob[..8]),
            Err(ObmError::Open)
        ));
    }

    #[test]
    fn store_atomic_et_bak() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.obm");
        store_manifest(&path, &keys(), b"v1").unwrap();
        store_manifest(&path, &keys(), b"v2").unwrap();
        assert_eq!(load_manifest(&path, &keys()).unwrap().unwrap(), b"v2");
        // Perte du courant : `.bak` sert le precedent.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(load_manifest(&path, &keys()).unwrap().unwrap(), b"v1");
        // Double perte : None (le core balaiera les scan_ct).
        std::fs::remove_file(dir.path().join("manifest.obm.bak")).unwrap();
        assert_eq!(load_manifest(&path, &keys()).unwrap(), None);
    }
}
