// This file is part of OnionBit.
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
use onionbit_crypto::keyblob::{seedblob_is_sealed, seedblob_open, seedblob_seal};

use crate::error::{CoreError, Result};
use crate::ipv8_stack::write_identity_key;

/// Nom du fichier graine racine — 32 octets en clair (`0600`) ou
/// blob `OBSK` quand `identity.at_rest` est actif.
pub const IDENTITY_SEED_FILE: &str = "identity_seed.bin";
/// Nom du fichier de la cle maitresse IPv8 derivee (cache en mode
/// seede, secret maitre en mode legacy).
pub const IPV8_KEY_FILE: &str = "ipv8_keypair.bin";
/// Nom du fichier du secret statique du pont stealth (ADR-0017).
pub const STEALTH_BRIDGE_KEY_FILE: &str = "stealth_bridge.key";

/// Repertoire des fichiers d'identite (ADR-0018, etape 58) :
/// `state/identity/` dans le layout cible, `state_dir` plat tant que
/// la migration legacy n'a pas deplace les fichiers.
///
/// Regle de compat lecture — la destination gagne des qu'elle
/// contient la graine ou le keypair ; sinon le plat historique gagne
/// s'il en contient un ; a defaut (install vierge) le sous-dossier
/// est choisi quand le layout l'a deja cree au boot.
pub fn identity_dir(state_dir: &Path) -> PathBuf {
    let dir = state_dir.join("identity");
    if dir.join(IDENTITY_SEED_FILE).exists() || dir.join(IPV8_KEY_FILE).exists() {
        return dir;
    }
    if state_dir.join(IDENTITY_SEED_FILE).exists() || state_dir.join(IPV8_KEY_FILE).exists() {
        return state_dir.to_path_buf();
    }
    if dir.is_dir() {
        dir
    } else {
        state_dir.to_path_buf()
    }
}

/// Etat de l'identite sur disque au boot.
pub enum IdentityState {
    /// Graine racine presente en clair — derivation autoritaire.
    Seeded { seed: IdentitySeed },
    /// Graine racine chiffree `OBSK` (`identity.at_rest`) — le boot
    /// entre en mode `locked` ; la cle n'est derivee qu'apres
    /// `POST /api/identity/unlock`.
    Sealed { blob: Vec<u8> },
    /// Cle maitresse seule (install anterieure a ADR-0016) — le
    /// fichier est la source de verite, jamais regenere.
    Legacy { keypair: Box<LibNaClSecretKey> },
    /// Rien sur disque — a resoudre par le gate de premier boot ou
    /// l'auto-generation (etape 48d).
    Absent,
}

/// Materiel identitaire resolu pour le demarrage — entierement en
/// memoire, rien n'est relu depuis le disque par la suite.
pub struct IdentityMaterial {
    /// Cle maitresse IPv8 effective (derivee ou legacy).
    pub keypair: LibNaClSecretKey,
    /// Secret statique du pont stealth (ADR-0017) — derive de la
    /// graine en mode seede, lu du fichier legacy sinon.
    pub bridge_sk: [u8; 32],
    /// Etat sur disque (apres resolution).
    pub kind: IdentityKind,
    /// Session invitee : aucun fichier identite ne doit etre ecrit ;
    /// l'identite meurt avec le processus.
    pub guest: bool,
    /// Racine `K_store` de la zone privee (ADR-0018) : la graine elle-
    /// meme en mode `Seeded`/`guest` (deterministe entre runs,
    /// protegee par `OBSK` quand l'at-rest est actif) et
    /// `SHA-256(keypair)` en `Legacy` — liee a l'identite, jamais
    /// persistee ni derivee d'un fichier mutable comme `bridge_sk`.
    pub store_root: [u8; 32],
}

impl IdentityMaterial {
    /// Materiel ephemere pour une session invitee — aucune
    /// persistance (`guest`), identite impossible a rejouer.
    pub fn guest() -> Self {
        Self::from_seed(&IdentitySeed::generate(), true)
    }

    /// Derivation en memoire pure d'une graine — utilisee par
    /// `unlock` (le fichier reste `OBSK`) et par le mode invite.
    fn from_seed(seed: &IdentitySeed, guest: bool) -> Self {
        Self {
            keypair: seed.derive_keypair(),
            bridge_sk: seed.derive_bridge_key(),
            kind: IdentityKind::Seeded,
            guest,
            store_root: *seed.as_bytes(),
        }
    }
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
    let dir = identity_dir(state_dir);
    let seed_path = dir.join(IDENTITY_SEED_FILE);
    let key_path = dir.join(IPV8_KEY_FILE);
    if seed_path.exists() {
        let data = std::fs::read(&seed_path)?;
        if seedblob_is_sealed(&data) {
            return Ok(IdentityState::Sealed { blob: data });
        }
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
            let key_path = identity_dir(state_dir).join(IPV8_KEY_FILE);
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
            let bridge_sk = load_or_create_bridge_sk(state_dir, IdentityKind::Seeded)?;
            Ok(IdentityMaterial {
                keypair,
                bridge_sk,
                kind: IdentityKind::Seeded,
                guest: false,
                store_root: *seed.as_bytes(),
            })
        }
        IdentityState::Legacy { keypair } => Ok(IdentityMaterial {
            bridge_sk: load_or_create_bridge_sk(state_dir, IdentityKind::Legacy)?,
            // Zone privee ancree sur la cle maitresse legacy (aucune
            // graine n'existe) — deterministe tant que le fichier
            // `ipv8_keypair.bin` survit, lie a l'identite.
            store_root: onionbit_crypto::hash::sha256(&keypair.to_bin()),
            keypair: *keypair,
            kind: IdentityKind::Legacy,
            guest: false,
        }),
        IdentityState::Absent => {
            let seed = IdentitySeed::generate();
            let dir = identity_dir(state_dir);
            write_identity_key(&dir.join(IDENTITY_SEED_FILE), seed.as_bytes())?;
            let keypair = seed.derive_keypair();
            write_identity_key(&dir.join(IPV8_KEY_FILE), &keypair.to_bin())?;
            let bridge_sk = seed.derive_bridge_key();
            write_identity_key(&dir.join(STEALTH_BRIDGE_KEY_FILE), &bridge_sk)?;
            Ok(IdentityMaterial {
                keypair,
                bridge_sk,
                kind: IdentityKind::Seeded,
                guest: false,
                store_root: *seed.as_bytes(),
            })
        }
        IdentityState::Sealed { .. } => Err(CoreError::InvalidState(
            "identite verrouillee (at-rest) — unlock requis",
        )),
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
    let dir = identity_dir(state_dir);
    let path = dir.join(STEALTH_BRIDGE_KEY_FILE);
    match kind {
        IdentityKind::Seeded => {
            let seed = IdentitySeed::from_bytes(&std::fs::read(dir.join(IDENTITY_SEED_FILE))?)
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

/// Materiel en memoire pour une graine deja connue — resolution
/// d'identite immediate en `identity_pending`/`locked` apres un
/// `restore` par phrase (le fichier est pose par `restore_seed`,
/// le materiel demarre la session sans le relire).
pub fn material_from_seed(seed: &IdentitySeed) -> IdentityMaterial {
    IdentityMaterial::from_seed(seed, false)
}

/// Chemin du fichier graine (pour l'API `recovery_phrase`, etape 48b).
pub fn seed_path(state_dir: &Path) -> PathBuf {
    identity_dir(state_dir).join(IDENTITY_SEED_FILE)
}

/// Installe une graine de restauration (`POST /api/identity/restore`
/// par phrase BIP39) : ecrit `identity_seed.bin` puis regenere
/// immediatement les caches derives pour coherence — la graine est
/// autoritaire des sa pose, sans attendre le redemarrage.
///
/// Ecrase une identite precedente (seedee ou legacy) : c'est le
/// contrat explicite d'un restore — le retour en arriere passe par
/// la phrase de l'identite precedente, pas par le filesystem.
pub fn restore_seed(state_dir: &Path, seed: &IdentitySeed) -> Result<()> {
    write_identity_key(&seed_path(state_dir), seed.as_bytes())?;
    let dir = identity_dir(state_dir);
    let keypair = seed.derive_keypair();
    write_identity_key(&dir.join(IPV8_KEY_FILE), &keypair.to_bin())?;
    write_identity_key(
        &dir.join(STEALTH_BRIDGE_KEY_FILE),
        &seed.derive_bridge_key(),
    )?;
    Ok(())
}

/// Cree une identite neuve depuis le gate `identity_pending`
/// (`POST /api/identity/create`) : graine neuve persistante, ou
/// verrouillee d'emblee si `password` est fourni (at-rest des la
/// creation — aucun cache derive en clair sur disque dans ce cas).
pub fn create_seed(state_dir: &Path, password: Option<&str>) -> Result<IdentityMaterial> {
    if !matches!(detect(state_dir)?, IdentityState::Absent) {
        return Err(CoreError::InvalidState(
            "une identite existe deja — create n'est valable qu'en premier boot",
        ));
    }
    let seed = IdentitySeed::generate();
    match password {
        Some(pw) if !pw.is_empty() => {
            let blob = seedblob_seal(pw.as_bytes(), seed.as_bytes())
                .map_err(|e| CoreError::State(format!("scellement graine: {e}")))?;
            write_identity_key(&seed_path(state_dir), &blob)?;
            Ok(IdentityMaterial::from_seed(&seed, false))
        }
        _ => {
            restore_seed(state_dir, &seed)?;
            let keypair = seed.derive_keypair();
            Ok(IdentityMaterial {
                keypair,
                bridge_sk: seed.derive_bridge_key(),
                kind: IdentityKind::Seeded,
                guest: false,
                store_root: *seed.as_bytes(),
            })
        }
    }
}

/// Ouvre le blob `OBSK` du disque → graine en memoire.
fn open_sealed(state_dir: &Path, password: &[u8]) -> Result<IdentitySeed> {
    let IdentityState::Sealed { blob } = detect(state_dir)? else {
        return Err(CoreError::InvalidState("pas d'identite verrouillee"));
    };
    let raw = seedblob_open(password, &blob)
        .map_err(|_| CoreError::InvalidState("mot de passe incorrect"))?;
    IdentitySeed::from_bytes(&raw)
        .map_err(|_| CoreError::InvalidState("blob OBSK de taille inattendue"))
}

/// Deverrouille une graine `OBSK` (`POST /api/identity/unlock`) :
/// derivation en memoire uniquement — aucun cache clair n'est ecrit
/// (ecrire `ipv8_keypair.bin` annulerait la protection at-rest).
pub fn unlock_seed(state_dir: &Path, password: &[u8]) -> Result<IdentityMaterial> {
    let seed = open_sealed(state_dir, password)?;
    Ok(IdentityMaterial::from_seed(&seed, false))
}

/// Active l'at-rest (`POST /api/identity/at_rest {enabled:true}`) :
/// remplace `identity_seed.bin` par un blob `OBSK` et retire les
/// caches derives — une copie brute du disque ne revele plus rien.
/// Refuse en mode `Legacy` (pas de graine a proteger).
pub fn seal_seed(state_dir: &Path, password: &[u8]) -> Result<()> {
    let IdentityState::Seeded { seed } = detect(state_dir)? else {
        return Err(CoreError::InvalidState(
            "at-rest requiert une identite seedee en clair (legacy : migrer d'abord)",
        ));
    };
    let blob = seedblob_seal(password, seed.as_bytes())
        .map_err(|e| CoreError::State(format!("scellement graine: {e}")))?;
    // Le fichier scelle d'abord, puis les caches retires — un crash
    // entre les deux laisse un etat encore coherent (caches regeneres
    // au prochain unlock).
    write_identity_key(&seed_path(state_dir), &blob)?;
    let dir = identity_dir(state_dir);
    let _ = std::fs::remove_file(dir.join(IPV8_KEY_FILE));
    let _ = std::fs::remove_file(dir.join(STEALTH_BRIDGE_KEY_FILE));
    Ok(())
}

/// Desactive l'at-rest (`enabled:false`) : mot de passe exige pour
/// re-ecrire la graine en clair + regenerer les caches — sans lui,
/// n'importe quel appelant demantelerait la protection.
pub fn unseal_seed(state_dir: &Path, password: &[u8]) -> Result<()> {
    let seed = open_sealed(state_dir, password)?;
    let material = IdentityMaterial::from_seed(&seed, false);
    write_identity_key(&seed_path(state_dir), seed.as_bytes())?;
    let dir = identity_dir(state_dir);
    write_identity_key(&dir.join(IPV8_KEY_FILE), &material.keypair.to_bin())?;
    write_identity_key(&dir.join(STEALTH_BRIDGE_KEY_FILE), &material.bridge_sk)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_dir_compat_plate_et_migree() {
        // ADR-0018 etape 58 : tant que `state/identity/` n'a pas de
        // contenu, le plat historique fait foi ; sinon le sous-dossier.
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        // Rien nulle part, layout non cree : plat (comportement
        // historique des tests directs).
        assert_eq!(identity_dir(state), state.to_path_buf());
        // Layout cree (boot `ensure_tree`), toujours vierge :
        // sous-dossier choisi pour les ecritures a venir.
        std::fs::create_dir_all(state.join("identity")).unwrap();
        assert_eq!(identity_dir(state), state.join("identity"));
        // Fichier plat legacy non migre : le plat reprend la main.
        std::fs::write(state.join(IPV8_KEY_FILE), b"k").unwrap();
        assert_eq!(identity_dir(state), state.to_path_buf());
        // Fichier migre dans le sous-dossier : il gagne.
        std::fs::write(state.join("identity").join(IPV8_KEY_FILE), b"k2").unwrap();
        assert_eq!(identity_dir(state), state.join("identity"));
    }

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

    #[test]
    fn restore_seed_regenere_les_caches() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = load_or_generate(dir.path()).unwrap();
        // Restore d'une autre graine (phrase BIP39 decodee en amont) :
        // les trois fichiers refletent immediatement la nouvelle
        // racine — la coherence ne depend pas du redemarrage.
        let seed2 = IdentitySeed::generate();
        restore_seed(dir.path(), &seed2).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(IDENTITY_SEED_FILE)).unwrap(),
            seed2.as_bytes()
        );
        let m2 = load_or_generate(dir.path()).unwrap();
        assert_eq!(
            m2.keypair.public_key().to_bin(),
            seed2.derive_keypair().public_key().to_bin()
        );
        assert_ne!(
            m2.keypair.public_key().to_bin(),
            m1.keypair.public_key().to_bin()
        );
        assert_eq!(
            std::fs::read(dir.path().join(STEALTH_BRIDGE_KEY_FILE)).unwrap(),
            seed2.derive_bridge_key()
        );
    }

    #[test]
    fn restore_cle_brute_refusee_sur_seedee_sauf_force_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = load_or_generate(dir.path()).unwrap();
        let cle = LibNaClSecretKey::generate();
        // Sans confirmation explicite : refus, graine intacte.
        assert!(crate::ipv8_stack::restore_identity_key(dir.path(), &cle.to_bin(), false).is_err());
        assert!(seed_path(dir.path()).exists());
        // Avec force_legacy : la graine est retiree, l'install
        // devient legacy avec la cle restauree.
        crate::ipv8_stack::restore_identity_key(dir.path(), &cle.to_bin(), true).unwrap();
        assert!(!seed_path(dir.path()).exists());
        let m = load_or_generate(dir.path()).unwrap();
        assert_eq!(m.kind, IdentityKind::Legacy);
        assert_eq!(m.keypair.public_key().to_bin(), cle.public_key().to_bin());
        assert_ne!(
            m.keypair.public_key().to_bin(),
            m1.keypair.public_key().to_bin()
        );
    }

    #[test]
    fn scellement_detect_sealed_unlock_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = load_or_generate(dir.path()).unwrap();
        let pk = m1.keypair.public_key().to_bin();
        seal_seed(dir.path(), b"mot de passe").unwrap();
        // Le disque ne contient plus ni graine claire ni caches.
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Sealed { .. }
        ));
        assert!(!dir.path().join(IPV8_KEY_FILE).exists());
        assert!(!dir.path().join(STEALTH_BRIDGE_KEY_FILE).exists());
        // `load_or_generate` refuse : l'identite attend l'unlock.
        assert!(load_or_generate(dir.path()).is_err());
        // Mauvais mot de passe : echec uniforme.
        assert!(unlock_seed(dir.path(), b"faux").is_err());
        // Bon mot de passe : meme identite, derivee en memoire.
        let m2 = unlock_seed(dir.path(), b"mot de passe").unwrap();
        assert_eq!(m2.keypair.public_key().to_bin(), pk);
        assert!(!m2.guest);
        // Le fichier reste OBSK, aucun cache clair n'est reecrit.
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Sealed { .. }
        ));
        assert!(!dir.path().join(IPV8_KEY_FILE).exists());
    }

    #[test]
    fn unseal_reecrit_graine_et_caches() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = load_or_generate(dir.path()).unwrap();
        seal_seed(dir.path(), b"pw").unwrap();
        unseal_seed(dir.path(), b"pw").unwrap();
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Seeded { .. }
        ));
        assert!(dir.path().join(IPV8_KEY_FILE).exists());
        let m2 = load_or_generate(dir.path()).unwrap();
        assert_eq!(m2.keypair.to_bin(), m1.keypair.to_bin());
        // Mauvais mot de passe sur unseal : rien n'est modifie.
        seal_seed(dir.path(), b"pw").unwrap();
        assert!(unseal_seed(dir.path(), b"faux").is_err());
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Sealed { .. }
        ));
    }

    #[test]
    fn create_seed_gate_premier_boot() {
        let dir = tempfile::tempdir().unwrap();
        // Sans mot de passe : graine claire + caches.
        let m = create_seed(dir.path(), None).unwrap();
        assert_eq!(m.kind, IdentityKind::Seeded);
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Seeded { .. }
        ));
        assert!(dir.path().join(IPV8_KEY_FILE).exists());
        // Second create sur une install seedee : refus.
        assert!(create_seed(dir.path(), None).is_err());
    }

    #[test]
    fn create_seed_avec_mot_de_passe_scelle() {
        let dir = tempfile::tempdir().unwrap();
        let m = create_seed(dir.path(), Some("pw")).unwrap();
        // Graine scellee d'emblee : aucun cache clair sur disque.
        assert!(matches!(
            detect(dir.path()).unwrap(),
            IdentityState::Sealed { .. }
        ));
        assert!(!dir.path().join(IPV8_KEY_FILE).exists());
        // Le materiel retourne est celui de la graine scellee.
        let m2 = unlock_seed(dir.path(), b"pw").unwrap();
        assert_eq!(
            m2.keypair.public_key().to_bin(),
            m.keypair.public_key().to_bin()
        );
    }

    #[test]
    fn invite_ephemere_et_distinct() {
        let g1 = IdentityMaterial::guest();
        let g2 = IdentityMaterial::guest();
        assert!(g1.guest && g2.guest);
        assert_eq!(g1.kind, IdentityKind::Seeded);
        // Deux sessions invitees = deux cles publiques distinctes.
        assert_ne!(
            g1.keypair.public_key().to_bin(),
            g2.keypair.public_key().to_bin()
        );
    }

    #[test]
    fn seal_refuse_en_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let kp = LibNaClSecretKey::generate();
        write_identity_key(&dir.path().join(IPV8_KEY_FILE), &kp.to_bin()).unwrap();
        assert!(seal_seed(dir.path(), b"pw").is_err());
    }
}
