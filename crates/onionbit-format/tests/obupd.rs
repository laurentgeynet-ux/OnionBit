// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Etape 141 (ADR-0032) — spike `OBUP` : round-trip signature,
//! traversal Windows, doublons, `key_id` inconnu vs mauvaise clef,
//! borne de taille, chaine `next_key`, rejeu cross-domaine.

use ed25519_dalek::SigningKey;
use onionbit_crypto::release::{release_key_id, sign_update_manifest};
use onionbit_format::obupd::{self, ObupFile, ObupdError, UpdateManifest};

fn sk(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn pk(k: &SigningKey) -> [u8; 32] {
    k.verifying_key().to_bytes()
}

fn manifest() -> UpdateManifest {
    UpdateManifest {
        version: "1.2.0".into(),
        target: "windows-x64".into(),
        ts: 1_700_000_000,
        files: vec![
            ObupFile {
                path: "windows/OnionBit.exe".into(),
                sha256: [0xAA; 32],
                size: 1024,
            },
            ObupFile {
                path: "windows/onionbit-daemon.exe".into(),
                sha256: [0xBB; 32],
                size: 2048,
            },
        ],
        min_supported: None,
        rollback: false,
        next_key: None,
        notes: None,
        key_id: 0,
    }
}

fn signed(m: &UpdateManifest, k: &SigningKey) -> Vec<u8> {
    let enc = obupd::encode_manifest(m);
    let sig = sign_update_manifest(k, &enc);
    obupd::build(m, release_key_id(&pk(k)), &sig)
}

/// Round-trip : blob signe → parse → verify → manifest identique.
#[test]
fn round_trip_signe_verifie() {
    let k = sk(0x01);
    let blob = signed(&manifest(), &k);
    let m = obupd::verify(&blob, &[pk(&k)]).expect("verify");
    assert_eq!(m.version, "1.2.0");
    assert_eq!(m.files.len(), 2);
    assert_eq!(m.files[0].path, "windows/OnionBit.exe");
    assert_eq!(m.key_id, release_key_id(&pk(&k)));
}

/// Signature sous une autre clef que celle du `key_id` declare →
/// `SigInvalid` ; `key_id` inconnu → `UnknownKeyId` — distincts.
#[test]
fn mauvaise_clef_et_key_id_inconnu_distingues() {
    let k1 = sk(0x01);
    let k2 = sk(0x02);
    // key_id present dans la liste mais signature d'une autre clef :
    // on fabrique un blob dont key_id=k1 mais sig=k2.
    let enc = obupd::encode_manifest(&manifest());
    let sig2 = sign_update_manifest(&k2, &enc);
    let forged = obupd::build(&manifest(), release_key_id(&pk(&k1)), &sig2);
    let err = obupd::verify(&forged, &[pk(&k1), pk(&k2)]).expect_err("forge");
    assert!(matches!(err, ObupdError::SigInvalid), "recu {err:?}");
    // key_id inconnu : clef absente de l'ensemble.
    let blob3 = signed(&manifest(), &sk(0x03));
    let err = obupd::verify(&blob3, &[pk(&k1)]).expect_err("key3");
    assert!(matches!(err, ObupdError::UnknownKeyId(_)), "recu {err:?}");
}

/// Traversal Windows : `..\`, absolu `C:\`, UNC `\\?\`, `a//b`,
/// `a/./b`, `/abs` — tous refuses avant tout acces fichier.
#[test]
fn chemins_traversal_rejetes() {
    let k = sk(0x01);
    for bad in [
        "..\\evil",
        "windows\\x.exe",
        "C:\\Windows\\x.dll",
        "\\\\?\\C:\\x",
        "a//b",
        "a/./b",
        "/abs/x",
        "../up.exe",
        "windows/../escape",
        "",
    ] {
        let mut m = manifest();
        m.files = vec![ObupFile {
            path: bad.into(),
            sha256: [1; 32],
            size: 1,
        }];
        let blob = signed(&m, &k);
        let err = obupd::verify(&blob, &[pk(&k)]).expect_err(bad);
        assert!(
            matches!(err, ObupdError::BadPath(_) | ObupdError::Malformed(_)),
            "{bad:?} → {err:?}"
        );
    }
}

/// Deux entrees `files` au meme chemin → rejet (la seconde ecraserait
/// silencieusement la premiere a l'extraction).
#[test]
fn chemin_duplique_rejete() {
    let k = sk(0x01);
    let mut m = manifest();
    m.files.push(ObupFile {
        path: "windows/OnionBit.exe".into(),
        sha256: [0xFF; 32],
        size: 99,
    });
    let blob = signed(&m, &k);
    let err = obupd::verify(&blob, &[pk(&k)]).expect_err("dup");
    assert!(matches!(err, ObupdError::Malformed(_)), "recu {err:?}");
}

/// Borne : un blob > `MAX_OBUP_SIZE` est refuse **avant** lecture —
/// l'endpoint d'import ne lit jamais un fichier geant.
#[test]
fn blob_trop_gros_borne_avant_parse() {
    let mut blob = signed(&manifest(), &sk(0x01));
    blob.resize(obupd::MAX_OBUP_SIZE + 1, 0);
    let err = obupd::verify(&blob, &[pk(&sk(0x01))]).expect_err("gros");
    assert!(matches!(err, ObupdError::Format(_)), "recu {err:?}");
    // Et un blob tronque est refuse proprement.
    for cut in [0usize, 4, 40, 76] {
        assert!(obupd::verify(&blob[..cut], &[pk(&sk(0x01))]).is_err());
    }
}

/// Chaine `next_key` : release A signee clef1 declare clef2 → clef2
/// acceptee pour la suivante ; une release signee clef3 (non chainee)
/// est rejetee. Le stockage `state/release_key.next` est le relais.
#[test]
fn next_key_rotation_chainee() {
    let k1 = sk(0x01);
    let k2 = sk(0x02);
    let k3 = sk(0x03);
    // Release A : signee k1, emporte next_key=k2.
    let mut a = manifest();
    a.next_key = Some(pk(&k2));
    let blob_a = signed(&a, &k1);
    let ma = obupd::verify(&blob_a, &[pk(&k1)]).expect("A");
    assert_eq!(ma.next_key, Some(pk(&k2)));
    // Simule la persistance post-swap : clef acceptee = {k1, k2}.
    let keys = [pk(&k1), pk(&k2)];
    // Release B signee k2 → acceptee.
    let blob_b = signed(&manifest(), &k2);
    assert!(obupd::verify(&blob_b, &keys).is_ok());
    // Release C signee k3 non chainee → UnknownKeyId.
    let blob_c = signed(&manifest(), &k3);
    let err = obupd::verify(&blob_c, &keys).expect_err("C");
    assert!(matches!(err, ObupdError::UnknownKeyId(_)), "recu {err:?}");
}

/// Une signature produite sous un autre domaine (`onion-bind` sur la
/// meme pk) ne peut pas etre reutilisee comme signature OBUP — le
/// domaine isole les usages.
#[test]
fn signature_cross_domaine_rejetee() {
    let k = sk(0x01);
    let m = manifest();
    let enc = obupd::encode_manifest(&m);
    // Signe le manifest brut SANS le domaine update — doit echouer
    // sous verify_update_manifest (domaine ajoute a la verification).
    let raw_sig = ed25519_dalek::Signer::sign(&k, &enc).to_bytes();
    let blob = obupd::build(&m, release_key_id(&pk(&k)), &raw_sig);
    let err = obupd::verify(&blob, &[pk(&k)]).expect_err("cross");
    assert!(matches!(err, ObupdError::SigInvalid), "recu {err:?}");
}

/// Le manifest est parse une fois la signature validee : meme contenu
/// signe mais manifest altere apres signature → echec.
#[test]
fn manifest_altere_apres_signature_rejete() {
    let k = sk(0x01);
    let mut blob = signed(&manifest(), &k);
    // Flippe un octet du manifest (apres l'en-tete 77o).
    let n = blob.len();
    blob[n - 4] ^= 0xFF;
    let err = obupd::verify(&blob, &[pk(&k)]).expect_err("altere");
    assert!(matches!(err, ObupdError::SigInvalid), "recu {err:?}");
}
