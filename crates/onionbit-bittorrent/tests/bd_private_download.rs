// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Zone privee sur le backend **bitdaemon** (ONIONBIT-1) : le meme
//! oracle « zero fuite » que `private_download` (ADR-0018), porte au
//! `DiskStorage` piece-aligne — telechargement reel en loopback TCP,
//! pieces ecrites en `.obd` chiffres sous noms HMAC.
//!
//! Le seeder reste rqbit (l'interop reseau est le sujet du test) ;
//! le downloader est `EngineBackend::Bitdaemon`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use librqbit_core::Id20;
use onionbit_bittorrent::add_options::AddDownloadOptions;
use onionbit_bittorrent::config::{EngineBackend, EngineConfig};
use onionbit_bittorrent::engine::BtEngine;
use onionbit_bittorrent::storage_private::PrivateStorageFactory;
use onionbit_crypto::obdfile::{ObdFile, PrivateStoreKeys};

/// Delai max du transfert local (TCP loopback).
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

/// Seeder rqbit **TCP** loopback (bitdaemon ne fait pas d'uTP pour
/// l'instant) — le `.torrent` est cree par rqbit, l'interop filaire
/// est celle d'un vrai client.
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
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seed_addr = {
        let a = seeder.listen_addr().expect("ecoute seeder");
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

/// Variante **publique** (pas de `storage_factory`) — isole le
/// stockage chiffre du chemin pair/reseau bitdaemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_public_bd_loopback() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,bitdaemon=debug,onionbit_bittorrent=debug")
        .try_init();
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let (_seed_dir, seeder, seed_addr, torrent_bytes, _ih) = spawn_seeder(&payload).await;

    let dl_dir = tempfile::tempdir().unwrap();
    let downloader = {
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        cfg.backend = EngineBackend::Bitdaemon;
        cfg.listen_port = Some(0);
        BtEngine::start(cfg).await.expect("downloader engine bd")
    };

    let dl = downloader
        .add_torrent_bytes_opts(
            torrent_bytes,
            &AddDownloadOptions {
                output_folder: Some(dl_dir.path().join("out")),
                initial_peers: vec![seed_addr],
                ..Default::default()
            },
        )
        .await
        .expect("add public download bd");

    let deadline = std::time::Instant::now() + TEST_TIMEOUT;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), dl.wait_completed()).await {
            Ok(Ok(())) => break,
            Ok(Err(e)) => panic!("wait_completed: {e}"),
            Err(_) => {
                let st = dl.stats();
                eprintln!(
                    "[probe-pub] progress={} state={:?} peers={}",
                    st.progress_bytes, st.state, st.peers_live
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "telechargement public bd en timeout"
                );
            }
        }
    }
    let written = walk(&dl_dir.path().join("out"))
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "payload.bin"))
        .expect("payload.bin produit");
    assert_eq!(
        std::fs::read(written).expect("read"),
        payload,
        "contenu telecharge identique"
    );

    downloader.stop().await;
    seeder.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_prive_bd_chiffre_et_opacite() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,bitdaemon=debug,onionbit_bittorrent=debug".into()),
        )
        .try_init();
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let (_seed_dir, seeder, seed_addr, torrent_bytes, ih) = spawn_seeder(&payload).await;

    let keys = Arc::new(PrivateStoreKeys::from_root(&[0x2A; 32]));
    let dl_dir = tempfile::tempdir().unwrap();
    let private_root = tempfile::tempdir().unwrap();

    let downloader = {
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        cfg.backend = EngineBackend::Bitdaemon;
        cfg.listen_port = Some(0);
        BtEngine::start(cfg).await.expect("downloader engine bd")
    };

    let dl = downloader
        .add_torrent_bytes_opts(
            torrent_bytes,
            &AddDownloadOptions {
                storage_factory: Some(PrivateStorageFactory::new(
                    keys.clone(),
                    private_root.path().to_path_buf(),
                    CHUNK_LOG2,
                )),
                initial_peers: vec![seed_addr],
                ..Default::default()
            },
        )
        .await
        .expect("add private download bd");

    let deadline = std::time::Instant::now() + TEST_TIMEOUT;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), dl.wait_completed()).await {
            Ok(Ok(())) => break,
            Ok(Err(e)) => panic!("wait_completed: {e}"),
            Err(_) => {
                let st = dl.stats();
                eprintln!(
                    "[probe] progress={} state={:?} peers={}",
                    st.progress_bytes, st.state, st.peers_live
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "telechargement bd en timeout"
                );
            }
        }
    }

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

    // --- Oracle 2 : le contenu dechiffre est fidele.
    let obd = produced
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "obd"))
        .expect("un .obd attendu");
    let (scan_ih, relpath, plain) = read_obd(obd, &keys);
    assert_eq!(scan_ih, ih, "scan_ct rend le bon infohash");
    assert_eq!(relpath, b"payload.bin", "relpath attendu");
    assert_eq!(plain, payload, "contenu dechiffre identique");

    // --- Oracle 3 : aucun cleartext (nom, payload) dans les fichiers
    // produits — le part-file chiffre inclus.
    for p in &produced {
        let bytes = std::fs::read(p).unwrap_or_default();
        for needle in [b"payload".as_slice(), &payload[..4096]] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "fuite de cleartext dans {}",
                p.display()
            );
        }
    }

    // --- Oracle 4 : le dossier de sortie du downloader ne contient
    // rien en clair pour ce telechargement.
    for p in walk(dl_dir.path()) {
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains("payload"), "fuite nom reel: {name}");
        assert!(!name.contains(&ih_hex), "fuite infohash: {name}");
    }

    // --- Oracle 5 : `remove(delete_files)` purge le groupe `.obd`.
    downloader
        .remove(&format!("{ih:?}"), true)
        .await
        .expect("remove prive bd");
    // `delete_files` est un job disque async (upstream
    // `torrent_handle::delete_files` non plus n'est pas synchrone) —
    // on attend la purge plutot que de l'exiger immediate.
    let purge_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !walk(private_root.path()).is_empty() {
        assert!(
            std::time::Instant::now() < purge_deadline,
            "fichiers .obd subsistent apres remove"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    downloader.stop().await;
    seeder.stop().await;
}
