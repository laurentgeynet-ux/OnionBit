// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Cycle de vie de l'identite sur disque (ADR-0016, etape 48a).
//!
//! Trois etats possibles dans `state_dir` :
//!
//! - **`Seeded`** : `identity_seed.bin` present — racine de verite ;
//!   les fichiers derives (`ipv8_keypair.bin`, `stealth_bridge.key`)
//!   sont des caches regeneres par HKDF ;
//! - **`Legacy`** : `ipv8_keypair.bin` seul (install anterieure) —
//!   fichier maitre, jamais touche par la derivation ;
//! - **`Absent`** : rien — le gate de premier boot ou l'auto-generation
//!   decident de la suite (etape 48d).

use std::path::{Path, PathBuf};

use onionbit_crypto::identity::IdentitySeed;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;

use crate::error::{CoreError, Result};
use crate::ipv8_stack::write_identity_key;

/// Nom du fichier graine racine (32 octets, `0600`).
pub const IDENTITY_SEED_FILE: &str = "identity_seed.bin";
/// Nom du fichier de la cle maitresse IPv8 derivee (cache en mode
/// seede, secret maitre en mode legacy).
pub const IPV8_KEY_FILE: &str = "ipv8_keypair.bin";
/// Nom du fichier du secret statique du pont stealth (ADR-0017).
pub const STEALTH_BRIDGE_KEY_FILE: &str = "stealth_bridge.key";

/// Etat de l'identite sur disque au boot.
pub enum IdentityState {
    /// Graine racine presente — derivation autoritaire.
    Seeded { seed: IdentitySeed },
    /// Cle maitresse seule (install anterieure a ADR-0016) — le
    /// fichier est la source de verite, jamais regenere.
    Legacy { keypair: Box<LibNaClSecretKey> },
    /// Rien sur disque — a resoudre par le gate de premier boot ou
    /// l'auto-generation (etape 48d).
    Absent,
}

/// Materiel identitaire resolu pour le demarrage.
pub struct IdentityMaterial {
    /// Cle maitresse IPv8 effective (derivee ou legacy).
    pub keypair: LibNaClSecretKey,
    /// Etat sur disque (apres resolution).
    pub kind: IdentityKind,
}

/// Classe d'identite (exposee a l'API : `seeded` dans `GET
/// /api/identity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// Identite racinee sur `identity_seed.bin`.
    Seeded,
    /// Identite fichier seul (pas de phrase possible).
    Legacy,
}

/// Charge l'etat identitaire de `state_dir` **sans rien ecrire**.
///
/// - graine presente et valide → `Seeded` ;
/// - graine corrompue → `Err` (jamais de regeneration silencieuse :
///   ecrire une nouvelle graine detruirait l'identite sans retour) ;
/// - keypair seul → `Legacy` (corrompu → `Err` via `from_bin`) ;
/// - rien → `Absent`.
pub fn detect(state_dir: &Path) -> Result<IdentityState> {
    let seed_path = state_dir.join(IDENTITY_SEED_FILE);
    let key_path = state_dir.join(IPV8_KEY_FILE);
    if seed_path.exists() {
        let data = std::fs::read(&seed_path)?;
        let seed = IdentitySeed::from_bytes(&data)
            .map_err(|_| CoreError::InvalidState("identity_seed.bin corrompu (taille != 32)"))?;
        return Ok(IdentityState::Seeded { seed });
    }
    if key_path.exists() {
        let data = std::fs::read(&key_path)?;
        let keypair = LibNaClSecretKey::from_bin(&data).map_err(CoreError::from)?;
        return Ok(IdentityState::Legacy {
            keypair: Box::new(keypair),
        });
    }
    Ok(IdentityState::Absent)
}

/// Resout l'identite pour le boot : `Seeded` et `Legacy` se chargent,
/// `Absent` genere une graine neuve (auto-generation headless — le
/// gate `identity_pending` intercepte avant, etape 48d).
///
/// Mode `Seeded` : derivation autoritaire — `ipv8_keypair.bin` est
/// reecrit quand il diverge de la graine (cache regenere), `warn`
/// trace. Mode `Legacy` : le fichier n'est jamais modifie.
pub fn load_or_generate(state_dir: &Path) -> Result<IdentityMaterial> {
    match detect(state_dir)? {
        IdentityState::Seeded { seed } => {
            let keypair = seed.derive_keypair();
            let key_path = state_dir.join(IPV8_KEY_FILE);
            let expected = keypair.to_bin();
            let divergent = std::fs::read(&key_path)
                .map(|d| d != expected)
                .unwrap_or(true);
            if divergent {
                if key_path.exists() {
                    tracing::warn!(
                        "ipv8_keypair.bin diverge de la graine — regenere (cache derive)"
                    );
                }
                write_identity_key(&key_path, &expected)?;
            }
            Ok(IdentityMaterial {
                keypair,
                kind: IdentityKind::Seeded,
            })
        }
        IdentityState::Legacy { keypair } => Ok(IdentityMaterial {
            keypair: *keypair,
            kind: IdentityKind::Legacy,
        }),
        IdentityState::Absent => {
            let seed = IdentitySeed::generate();
            write_identity_key(&state_dir.join(IDENTITY_SEED_FILE), seed.as_bytes())?;
            let keypair = seed.derive_keypair();
            write_identity_key(&state_dir.join(IPV8_KEY_FILE), &keypair.to_bin())?;
            Ok(IdentityMaterial {
                keypair,
                kind: IdentityKind::Seeded,
            })
        }
    }
}

/// Resout le secret statique du pont stealth (ADR-0017).
///
/// - `Seeded` : **derivation autoritaire** — le secret vient de la
///   graine ; un fichier divergent est remplace avec `warn` (la
///   phrase de recuperation doit reproduire exactement la meme cle
///   de pont, sinon les liens `onionbit-bridge://` distribues
///   survivraient mal a une restauration).
/// - `Legacy` : le fichier est maitre — jamais regenere
///   silencieusement ; absent → aleatoire (comportement actuel).
pub fn load_or_create_bridge_sk(state_dir: &Path, kind: IdentityKind) -> Result<[u8; 32]> {
    let path = state_dir.join(STEALTH_BRIDGE_KEY_FILE);
    match kind {
        IdentityKind::Seeded => {
            let seed =
                IdentitySeed::from_bytes(&std::fs::read(state_dir.join(IDENTITY_SEED_FILE))?)
                    .map_err(|_| CoreError::InvalidState("identity_seed.bin corrompu"))?;
            let derived = seed.derive_bridge_key();
            let divergent = std::fs::read(&path)
                .map(|d| d.as_slice() != derived.as_slice())
                .unwrap_or(true);
            if divergent {
                if path.exists() {
                    tracing::warn!(
                        "stealth_bridge.key diverge de la graine — regenere (cache derive) ; \
                         les liens d'invitation precedents deviennent invalides"
                    );
                }
                write_identity_key(&path, &derived)?;
            }
            Ok(derived)
        }
        IdentityKind::Legacy => match std::fs::read(&path) {
            Ok(data) => <[u8; 32]>::try_from(data.as_slice())
                .map_err(|_| CoreError::InvalidState("stealth_bridge.key corrompu (taille != 32)")),
            Err(_) => {
                let (sk, _pk) = onionbit_crypto::stealth::generate_bridge_keypair();
                write_identity_key(&path, &sk)?;
                Ok(sk)
            }
        },
    }
}

/// Chemin du fichier graine (pour l'API `recovery_phrase`, etape 48b).
pub fn seed_path(state_dir: &Path) -> PathBuf {
    state_dir.join(IDENTITY_SEED_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_puis_seeded_deterministe() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = load_or_generate(dir.path()).unwrap();
        assert_eq!(m1.kind, IdentityKind::Seeded);
        let pk1 = m1.keypair.public_key().to_bin();
        // Second boot : meme identite (derivee de la meme graine).
        let m2 = load_or_generate(dir.path()).unwrap();
        assert_eq!(m2.keypair.public_key().to_bin(), pk1);
        // Le fichier keypair est un cache fidele de la graine.
        let on_disk = std::fs::read(dir.path().join(IPV8_KEY_FILE)).unwrap();
        assert_eq!(on_disk, m2.keypair.to_bin());
    }

    #[test]
    fn legacy_jamais_touche() {
        let dir = tempfile::tempdir().unwrap();
        let kp = LibNaClSecretKey::generate();
        write_identity_key(&dir.path().join(IPV8_KEY_FILE), &kp.to_bin()).unwrap();
        let m = load_or_generate(dir.path()).unwrap();
        assert_eq!(m.kind, IdentityKind::Legacy);
        assert_eq!(m.keypair.to_bin(), kp.to_bin());
        // Pas de graine creee en legacy.
        assert!(!dir.path().join(IDENTITY_SEED_FILE).exists());
    }

    #[test]
    fn seeded_reecrit_keypair_divergent() {
        let dir = tempfile::tempdir().unwrap();
        let m = load_or_generate(dir.path()).unwrap();
        // Sabotage du cache : keypair etranger.
        let other = LibNaClSecretKey::generate();
        write_identity_key(&dir.path().join(IPV8_KEY_FILE), &other.to_bin()).unwrap();
        let m2 = load_or_generate(dir.path()).unwrap();
        assert_eq!(
            m2.keypair.public_key().to_bin(),
            m.keypair.public_key().to_bin(),
            "la graine doit gagner sur le cache divergent"
        );
    }

    #[test]
    fn graine_corrompue_refusee() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(IDENTITY_SEED_FILE), [0u8; 17]).unwrap();
        assert!(detect(dir.path()).is_err());
        assert!(load_or_generate(dir.path()).is_err());
    }

    #[test]
    fn bridge_sk_legacy_preserve_seeded_derive() {
        let dir = tempfile::tempdir().unwrap();
        // Legacy : fichier aleatoire conserve tel quel.
        let kp = LibNaClSecretKey::generate();
        write_identity_key(&dir.path().join(IPV8_KEY_FILE), &kp.to_bin()).unwrap();
        let (sk, _) = onionbit_crypto::stealth::generate_bridge_keypair();
        write_identity_key(&dir.path().join(STEALTH_BRIDGE_KEY_FILE), &sk).unwrap();
        let got = load_or_create_bridge_sk(dir.path(), IdentityKind::Legacy).unwrap();
        assert_eq!(got, sk, "legacy : le fichier pont est maitre");

        // Seeded : derivation autoritaire.
        let dir2 = tempfile::tempdir().unwrap();
        load_or_generate(dir2.path()).unwrap();
        let (other, _) = onionbit_crypto::stealth::generate_bridge_keypair();
        write_identity_key(&dir2.path().join(STEALTH_BRIDGE_KEY_FILE), &other).unwrap();
        let got2 = load_or_create_bridge_sk(dir2.path(), IdentityKind::Seeded).unwrap();
        assert_ne!(got2, other, "seeded : la graine gagne sur le fichier");
        let seed =
            IdentitySeed::from_bytes(&std::fs::read(dir2.path().join(IDENTITY_SEED_FILE)).unwrap())
                .unwrap();
        assert_eq!(got2, seed.derive_bridge_key());
    }
}
