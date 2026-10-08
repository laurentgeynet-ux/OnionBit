// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Zone privee (ADR-0018, etape 61) : telechargement BitTorrent reel
//! en loopback dont les pieces traversent le stockage chiffre `OBD`,
//! le fastresume `.bitv` opaque (nom HMAC), et les oracles « zero
//! fuite » sur l'arborescence produite.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use librqbit_core::Id20;
use onionbit_bittorrent::add_options::AddDownloadOptions;
use onionbit_bittorrent::bitv_opaque::OpaqueBitV;
use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;
use onionbit_bittorrent::storage_private::PrivateStorageFactory;
use onionbit_crypto::obdfile::{ObdFile, PrivateStoreKeys};

/// Delai max du transfert local (uTP loopback).
const TEST_TIMEOUT: Duration = Duration::from_secs(90);
/// `chunk_log2` = 14 → 16 Kio (bloc BitTorrent, borne RMW).
const CHUNK_LOG2: u8 = 14;

/// Tous les fichiers sous `dir`, recursivement.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Cree un torrent reel monofichier + un seeder uTP loopback.
async fn spawn_seeder(
    payload: &[u8],
) -> (tempfile::TempDir, BtEngine, SocketAddr, bytes::Bytes, Id20) {
    let seed_dir = tempfile::tempdir().unwrap();
    std::fs::write(seed_dir.path().join("payload.bin"), payload).unwrap();
    let torrent = librqbit::create_torrent(
        seed_dir.path(),
        librqbit::CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .expect("create_torrent");
    let ih = torrent.info_hash();
    let torrent_bytes = torrent.as_bytes().unwrap();

    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.utp_only = true;
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seed_addr = {
        let a = seeder.listen_addr().expect("ecoute uTP seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };
    (seed_dir, seeder, seed_addr, torrent_bytes, ih)
}

/// Lit un `.obd` en clair (scan → `K_file` → read complet).
fn read_obd(path: &Path, keys: &PrivateStoreKeys) -> (Id20, Vec<u8>, Vec<u8>) {
    let (ih, relpath) = ObdFile::scan_path(path, keys)
        .expect("scan")
        .expect("secours present");
    let obd = ObdFile::open(path, &keys.file_cipher(&ih, &relpath)).expect("open");
    let mut buf = vec![0u8; obd.plain_len() as usize];
    obd.read_range(0, &mut buf).unwrap();
    (Id20::new(ih), relpath, buf)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_prive_loopback_chiffre_et_opacite() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let (_seed_dir, seeder, seed_addr, torrent_bytes, ih) = spawn_seeder(&payload).await;

    let keys = Arc::new(PrivateStoreKeys::from_root(&[0x2A; 32]));
    let guest = Arc::new(AtomicBool::new(false));

    let dl_dir = tempfile::tempdir().unwrap();
    let private_root = tempfile::tempdir().unwrap();
    let rqbit_dir = dl_dir.path().join("rqbit");
    let opaque = OpaqueBitV::new(Some(keys.clone()), guest.clone(), rqbit_dir.clone());

    let downloader = {
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        cfg.utp_only = true;
        cfg.listen_port = Some(0);
        cfg.fastresume = true;
        cfg.persistence_dir = Some(rqbit_dir.clone());
        cfg.opaque_bitv = Some(opaque.clone());
        BtEngine::start(cfg).await.expect("downloader engine")
    };

    let dl = downloader
        .add_torrent_bytes_opts(
            torrent_bytes,
            &AddDownloadOptions {
                storage_factory: Some(
                    PrivateStorageFactory::new(
                        keys.clone(),
                        private_root.path().to_path_buf(),
                        CHUNK_LOG2,
                    )
                    .with_private_hashes(opaque.private_hashes().clone()),
                ),
                initial_peers: vec![seed_addr],
                ..Default::default()
            },
        )
        .await
        .expect("add private download");

    tokio::time::timeout(TEST_TIMEOUT, dl.wait_completed())
        .await
        .expect("telechargement en timeout")
        .expect("wait_completed");

    // --- Oracle 1 : aucun nom/artefact en clair sous la zone privee.
    let ih_hex = format!("{ih:?}");
    let produced = walk(private_root.path());
    assert!(!produced.is_empty(), "aucun .obd produit");
    for p in &produced {
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.ends_with(".obd"), "fichier non opaque: {name}");
        assert!(!name.contains("payload"), "nom reel visible: {name}");
        assert!(!name.contains(&ih_hex), "infohash visible: {name}");
    }

    // --- Oracle 2 : le contenu dechiffre est fidele (SHA-256 via
    // comparaison octet a octet du plain).
    let obd = produced
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "obd"))
        .expect("un .obd attendu");
    let (scan_ih, relpath, plain) = read_obd(obd, &keys);
    assert_eq!(scan_ih, ih, "scan_ct rend le bon infohash");
    assert_eq!(relpath, b"payload.bin", "relpath attendu");
    assert_eq!(plain, payload, "contenu dechiffre identique");

    // --- Oracle 3 : fastresume opaque — `<hmac>.bitv` present,
    // `<ih>.bitv` et l'entree session.json absents.
    let rqbit_files = walk(&rqbit_dir);
    let names: Vec<String> = rqbit_files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let opaque_bitv = format!("{:?}.bitv", Id20::new(keys.bitv_name(&ih.0)));
    assert!(
        names.contains(&opaque_bitv),
        "<hmac>.bitv absent — presents: {names:?}"
    );
    let clear_bitv = format!("{ih:?}.bitv");
    assert!(
        !names.contains(&clear_bitv),
        "<infohash>.bitv en clair present"
    );
    assert!(
        !names.contains(&format!("{ih:?}.torrent")),
        ".torrent en clair present pour un prive"
    );
    if let Ok(session) = std::fs::read_to_string(rqbit_dir.join("session.json")) {
        assert!(
            !session.contains(&ih_hex),
            "infohash persiste dans session.json"
        );
    }
    // --- Oracle 4 : aucun artefact (zone privee OU rqbit) ne contient
    // en clair un marqueur d'identite `LibNaCL`, le nom reel ou
    // l'infohash — y compris dans le CONTENU des fichiers.
    for p in produced.iter().chain(rqbit_files.iter()) {
        let bytes = std::fs::read(p).unwrap_or_default();
        for needle in [b"LibNaCL".as_slice(), b"payload".as_slice()] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "fuite {:?} dans {}",
                String::from_utf8_lossy(needle),
                p.display()
            );
        }
    }
    // Le dossier de sortie du downloader ne doit rien contenir de
    // materiel en clair pour ce telechargement.
    let public_side = walk(dl_dir.path());
    for p in &public_side {
        if p.starts_with(&rqbit_dir) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains("payload"), "fuite nom reel: {name}");
    }

    // --- Oracle 5 : `remove(delete_files)` efface les artefacts
    // opaques que rqbit ne voit pas (prive saute de session.json) :
    // `<hmac>.bitv` supprime, groupe `.obd` supprime.
    downloader
        .remove(&format!("{ih:?}"), true)
        .await
        .expect("remove prive");
    assert!(
        !rqbit_dir.join(&opaque_bitv).exists(),
        "<hmac>.bitv subsiste apres remove"
    );
    assert!(
        walk(private_root.path()).is_empty(),
        "fichiers .obd subsistent apres remove"
    );

    downloader.stop().await;
    seeder.stop().await;
}

/// Reprise apres restart : nouveau moteur, memes dossiers — le
/// `.bitv` opaque est relu sans re-telechargement (le seeder est
/// eteint : seule la validation locale peut terminer le torrent).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_prive_reprise_sans_reseau() {
    let payload: Vec<u8> = (0..120_000u32).map(|i| (i % 249) as u8).collect();
    let (_seed_dir, seeder, seed_addr, torrent_bytes, _ih) = spawn_seeder(&payload).await;

    let keys = Arc::new(PrivateStoreKeys::from_root(&[0x7B; 32]));
    let guest = Arc::new(AtomicBool::new(false));

    let dl_dir = tempfile::tempdir().unwrap();
    let private_root = tempfile::tempdir().unwrap();
    let rqbit_dir = dl_dir.path().join("rqbit");
    let opaque = OpaqueBitV::new(Some(keys.clone()), guest.clone(), rqbit_dir.clone());

    let downloader = {
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        cfg.utp_only = true;
        cfg.listen_port = Some(0);
        cfg.fastresume = true;
        cfg.persistence_dir = Some(rqbit_dir.clone());
        cfg.opaque_bitv = Some(opaque.clone());
        BtEngine::start(cfg).await.expect("downloader engine")
    };
    // `with_private_hashes` : `create` inscrit l'infohash dans le set
    // partage avant le `bitv.load` — meme apres un restart ou le set
    // memoire est vide (c'est le manifeste OBM qui re-ajoute).
    let factory =
        PrivateStorageFactory::new(keys.clone(), private_root.path().to_path_buf(), CHUNK_LOG2)
            .with_private_hashes(opaque.private_hashes().clone());
    let dl = downloader
        .add_torrent_bytes_opts(
            torrent_bytes.clone(),
            &AddDownloadOptions {
                storage_factory: Some(factory.clone()),
                initial_peers: vec![seed_addr],
                ..Default::default()
            },
        )
        .await
        .expect("add private");
    tokio::time::timeout(TEST_TIMEOUT, dl.wait_completed())
        .await
        .expect("timeout phase 1")
        .expect("phase 1");

    // Restart : nouveau moteur, seeder eteint — seule la relecture
    // locale (.bitv + re-hash des .obd) peut conclure.
    downloader.stop().await;
    seeder.stop().await;

    let downloader2 = {
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        cfg.utp_only = true;
        cfg.listen_port = Some(0);
        cfg.fastresume = true;
        cfg.persistence_dir = Some(rqbit_dir.clone());
        cfg.opaque_bitv = Some(opaque);
        BtEngine::start(cfg).await.expect("downloader engine 2")
    };
    let dl2 = downloader2
        .add_torrent_bytes_opts(
            torrent_bytes,
            &AddDownloadOptions {
                storage_factory: Some(factory),
                ..Default::default()
            },
        )
        .await
        .expect("re-add private");
    tokio::time::timeout(TEST_TIMEOUT, dl2.wait_completed())
        .await
        .expect("timeout reprise")
        .expect("reprise sans reseau");

    // Le contenu relu en clair reste fidele.
    let obd = walk(private_root.path())
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "obd"))
        .expect("un .obd");
    let (_, _, plain) = read_obd(&obd, &keys);
    assert_eq!(plain, payload, "contenu integre apres restart");

    downloader2.stop().await;
}
