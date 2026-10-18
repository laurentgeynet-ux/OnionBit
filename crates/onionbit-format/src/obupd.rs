// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Format `OBUP` — artefact de mise a jour signe (ADR-0032).
//!
//! Layout :
//! ```text
//! "OBUP" (4o) | version u8 | key_id u64 LE | sig ed25519 (64o) |
//! manifest bencode canonique
//! ```
//!
//! Le manifest est un dict bencode (cles triees — `BTreeMap` :
//! l'encodage est canonique par construction) :
//!
//! - `v` : semver de la release (`"1.2.0"`) ;
//! - `target` : cible (`"windows-x64"`) ;
//! - `ts` : timestamp unix de creation ;
//! - `files` : liste `[{p: "windows/OnionBit.exe",
//!   sha256: <32 octets>, size: int}, ...]` — chemins **relatifs a
//!   la racine du bundle**, `/` normalise, `..`/absolu/`\` interdits ;
//! - optionnels : `min_supported` (semver), `rollback` (int != 0),
//!   `next_key` (32o — clef de release successeur), `notes` (str).
//!
//! Borne : un blob `OBUP` est *petit* (le payload est a cote, pas
//! dedans) — [`MAX_OBUP_SIZE`] est applique **avant** tout parse.

use std::collections::BTreeMap;

use crate::bencode::{self, BValue};
use crate::FormatError;

/// Magic d'un artefact de release signee.
pub const OBUP_MAGIC: &[u8; 4] = b"OBUP";

/// Version du format `OBUP`.
pub const OBUP_VERSION: u8 = 1;

/// Taille maximale d'un blob `OBUP` — le manifest ne decrit que des
/// chemins+hashes ; un blob plus gros est refuse **avant lecture**
/// (l'endpoint d'import est une surface DoS).
pub const MAX_OBUP_SIZE: usize = 1024 * 1024;

/// Nombre maximal de fichiers listes dans le manifest.
pub const MAX_OBUP_FILES: usize = 1024;

/// Longueur max d'un chemin relatif dans le manifest.
pub const MAX_OBUP_PATH: usize = 512;

/// Erreur d'artefact `OBUP` — distinguée de la corruption generique
/// (une mauvaise signature n'est pas un fichier corrompu).
#[derive(Debug, thiserror::Error)]
pub enum ObupdError {
    /// Magic ou version incorrects.
    #[error("artefact OBUP invalide: {0}")]
    Malformed(String),
    /// `key_id` ne correspond a aucune clef acceptee.
    #[error("clef de release inconnue (key_id {0:#x})")]
    UnknownKeyId(u64),
    /// Signature Ed25519 invalide sous la clef resolue.
    #[error("signature OBUP invalide")]
    SigInvalid,
    /// Chemin de fichier interdit dans le manifest.
    #[error("chemin interdit dans le manifest: {0}")]
    BadPath(String),
    /// Erreur de format sous-jacente (bencode, borne).
    #[error(transparent)]
    Format(#[from] FormatError),
}

/// Une entree `files` du manifest : chemin relatif + empreinte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObupFile {
    /// Chemin relatif au bundle, `/` normalise.
    pub path: String,
    /// SHA-256 du contenu du fichier.
    pub sha256: [u8; 32],
    /// Taille en octets.
    pub size: u64,
}

/// Manifest decode et valide.
#[derive(Debug, Clone)]
pub struct UpdateManifest {
    /// Version semver de la release.
    pub version: String,
    /// Cible (`"windows-x64"`).
    pub target: String,
    /// Timestamp de creation.
    pub ts: i64,
    /// Fichiers du payload (ordonnes par chemin).
    pub files: Vec<ObupFile>,
    /// Version minimale exigee pour appliquer la mise a jour.
    pub min_supported: Option<String>,
    /// Downgrade autorise (signe).
    pub rollback: bool,
    /// Clef de release successeur (rotation signee).
    pub next_key: Option<[u8; 32]>,
    /// Notes libres affichees avant confirmation.
    pub notes: Option<String>,
    /// `key_id` du signataire (en-tete du blob).
    pub key_id: u64,
}

/// Encode le manifest en bencode canonique (cles triees par
/// `BTreeMap`) — c'est ces octets qui sont signes.
pub fn encode_manifest(m: &UpdateManifest) -> Vec<u8> {
    let mut files = Vec::with_capacity(m.files.len());
    for f in &m.files {
        let mut d = BTreeMap::new();
        d.insert(b"p".to_vec(), BValue::Bytes(f.path.clone().into_bytes()));
        d.insert(b"sha256".to_vec(), BValue::Bytes(f.sha256.to_vec()));
        d.insert(b"size".to_vec(), BValue::Int(f.size as i64));
        files.push(BValue::Dict(d));
    }
    let mut root = BTreeMap::new();
    root.insert(b"v".to_vec(), BValue::Bytes(m.version.clone().into_bytes()));
    root.insert(
        b"target".to_vec(),
        BValue::Bytes(m.target.clone().into_bytes()),
    );
    root.insert(b"ts".to_vec(), BValue::Int(m.ts));
    root.insert(b"files".to_vec(), BValue::List(files));
    if let Some(ms) = &m.min_supported {
        root.insert(
            b"min_supported".to_vec(),
            BValue::Bytes(ms.clone().into_bytes()),
        );
    }
    if m.rollback {
        root.insert(b"rollback".to_vec(), BValue::Int(1));
    }
    if let Some(nk) = &m.next_key {
        root.insert(b"next_key".to_vec(), BValue::Bytes(nk.to_vec()));
    }
    if let Some(n) = &m.notes {
        root.insert(b"notes".to_vec(), BValue::Bytes(n.clone().into_bytes()));
    }
    bencode::encode(&BValue::Dict(root))
}

/// Emballe `manifest` + signature dans un blob `OBUP`.
pub fn build(m: &UpdateManifest, key_id: u64, sig: &[u8; 64]) -> Vec<u8> {
    let manifest = encode_manifest(m);
    let mut out = Vec::with_capacity(4 + 1 + 8 + 64 + manifest.len());
    out.extend_from_slice(OBUP_MAGIC);
    out.push(OBUP_VERSION);
    out.extend_from_slice(&key_id.to_le_bytes());
    out.extend_from_slice(sig);
    out.extend_from_slice(&manifest);
    out
}

/// Valide un chemin relatif de payload : interdit absolu, `..`,
/// `\`, `:`, `//`, composants vides, et borne sa longueur — couvre
/// `..\evil`, `C:\x`, `\\?\x`, `a//b`, slashs mixtes.
fn check_path(p: &str) -> std::result::Result<(), ObupdError> {
    if p.is_empty() {
        return Err(ObupdError::BadPath("<vide>".into()));
    }
    if p.len() > MAX_OBUP_PATH {
        return Err(FormatError::SizeExceeded {
            actual: p.len(),
            max: MAX_OBUP_PATH,
        }
        .into());
    }
    if p.starts_with('/')
        || p.starts_with('\\')
        || p.contains('\\')
        || p.contains(':')
        || p.contains("//")
    {
        return Err(ObupdError::BadPath(p.to_string()));
    }
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(ObupdError::BadPath(p.to_string()));
        }
    }
    Ok(())
}

/// Decode + valide le manifest bencode. Le chemin de chaque fichier
/// est controle avant d'etre retourne (aucun `..`/absolu ne sort).
fn decode_manifest(data: &[u8], key_id: u64) -> std::result::Result<UpdateManifest, ObupdError> {
    let root = match bencode::decode(data)? {
        BValue::Dict(d) => d,
        _ => {
            return Err(ObupdError::Malformed("manifest non-dict".into()));
        }
    };
    let get = |k: &[u8]| root.get(k);
    let version = get(b"v")
        .and_then(BValue::as_str)
        .ok_or(FormatError::MissingField("v"))?
        .to_string();
    let target = get(b"target")
        .and_then(BValue::as_str)
        .ok_or(FormatError::MissingField("target"))?
        .to_string();
    let ts = get(b"ts")
        .and_then(BValue::as_int)
        .ok_or(FormatError::MissingField("ts"))?;

    let files = match get(b"files") {
        Some(BValue::List(l)) => l,
        _ => return Err(FormatError::MissingField("files").into()),
    };
    if files.len() > MAX_OBUP_FILES {
        return Err(FormatError::SizeExceeded {
            actual: files.len(),
            max: MAX_OBUP_FILES,
        }
        .into());
    }
    let mut out_files = Vec::with_capacity(files.len());
    let mut seen = std::collections::BTreeSet::new();
    for f in files {
        let d = f
            .as_dict()
            .ok_or(ObupdError::Malformed("entree files non-dict".into()))?;
        let path = d
            .get(b"p".as_slice())
            .and_then(|v| v.as_str())
            .ok_or(FormatError::MissingField("p"))?
            .to_string();
        check_path(&path)?;
        if !seen.insert(path.clone()) {
            return Err(ObupdError::Malformed(format!("chemin duplique : {path}")));
        }
        let sha_bytes = d
            .get(b"sha256".as_slice())
            .and_then(|v| v.as_bytes())
            .ok_or(FormatError::MissingField("sha256"))?;
        let sha256: [u8; 32] = sha_bytes
            .try_into()
            .map_err(|_| ObupdError::Malformed("sha256 != 32o".into()))?;
        let size = d
            .get(b"size".as_slice())
            .and_then(|v| v.as_int())
            .ok_or(FormatError::MissingField("size"))?;
        if size < 0 {
            return Err(ObupdError::Malformed("size negative".into()));
        }
        out_files.push(ObupFile {
            path,
            sha256,
            size: size as u64,
        });
    }
    out_files.sort_by(|a, b| a.path.cmp(&b.path));

    let min_supported = get(b"min_supported")
        .and_then(BValue::as_str)
        .map(str::to_string);
    let rollback = get(b"rollback").and_then(BValue::as_int).unwrap_or(0) != 0;
    let next_key = match get(b"next_key").and_then(BValue::as_bytes) {
        Some(b) => Some(
            b.try_into()
                .map_err(|_| ObupdError::Malformed("next_key != 32o".into()))?,
        ),
        None => None,
    };
    let notes = get(b"notes").and_then(BValue::as_str).map(str::to_string);

    Ok(UpdateManifest {
        version,
        target,
        ts,
        files: out_files,
        min_supported,
        rollback,
        next_key,
        notes,
        key_id,
    })
}

/// Parse un blob `OBUP` : retourne `(key_id, sig, manifest_brut)`.
/// Borne [`MAX_OBUP_SIZE`] appliquee **avant** tout decodage.
pub fn parse(blob: &[u8]) -> std::result::Result<(u64, [u8; 64], &[u8]), ObupdError> {
    if blob.len() > MAX_OBUP_SIZE {
        return Err(FormatError::SizeExceeded {
            actual: blob.len(),
            max: MAX_OBUP_SIZE,
        }
        .into());
    }
    const HEAD: usize = 4 + 1 + 8 + 64;
    if blob.len() < HEAD + 1 {
        return Err(ObupdError::Malformed("blob tronque".into()));
    }
    if &blob[..4] != OBUP_MAGIC {
        return Err(ObupdError::Malformed("magic absent".into()));
    }
    if blob[4] != OBUP_VERSION {
        return Err(ObupdError::Malformed(format!("version OBUP {}", blob[4])));
    }
    let key_id = u64::from_le_bytes(blob[5..13].try_into().expect("8o"));
    let sig: [u8; 64] = blob[13..77].try_into().expect("64o");
    Ok((key_id, sig, &blob[HEAD..]))
}

/// Compare deux versions semver `X.Y[.Z]` (pre-release/build
/// ignores au-dela du troisieme composant, prefixe `v` toleré).
/// `None` si l'une n'est pas un triplet numerique — le comparatif
/// anti-downgrade refuse alors plutot que de deviner.
pub fn semver_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn parts(v: &str) -> Option<[u64; 3]> {
        let v = v.trim().trim_start_matches('v');
        // Coupe au premier suffixe `-pre`/`+build`.
        let core = v.split(['-', '+']).next()?;
        let mut out = [0u64; 3];
        let mut n = 0;
        for seg in core.split('.') {
            if n >= 3 || seg.is_empty() || !seg.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            out[n] = seg.parse().ok()?;
            n += 1;
        }
        (n >= 2).then_some(out)
    }
    Some(parts(a)?.cmp(&parts(b)?))
}

/// Verifie un blob `OBUP` contre un ensemble de clefs acceptees :
/// `key_id` resout la clef (inconnu → [`ObupdError::UnknownKeyId`]),
/// puis Ed25519 domainee sur le manifest brut. Retourne le manifest
/// decode si tout est valide.
pub fn verify(blob: &[u8], keys: &[[u8; 32]]) -> std::result::Result<UpdateManifest, ObupdError> {
    let (key_id, sig, manifest) = parse(blob)?;
    let pk = keys
        .iter()
        .find(|k| onionbit_crypto::release::release_key_id(k) == key_id)
        .ok_or(ObupdError::UnknownKeyId(key_id))?;
    if !onionbit_crypto::release::verify_update_manifest(pk, manifest, &sig) {
        return Err(ObupdError::SigInvalid);
    }
    decode_manifest(manifest, key_id)
}
