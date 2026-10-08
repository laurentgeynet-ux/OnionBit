// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adds concurrents de torrents distincts sur le meme moteur avec
//! persistance — regression du TOCTOU `next_id` de
//! `Session::add_torrent_internal` : `persistence.next_id()` lisait
//! `max+1` sans le reserver, deux adds paralleles obtenaient le meme
//! id et le second etait classe `AlreadyManaged` du torrent voisin
//! (`*eid == id`) — jamais insere dans la map, `get_by_hash` muet.
//! C'est le flake CI `live_flotte_10_restart` (`.bin` et magnet
//! spawnes sur la meme lane, assert `owner_engine_hops` -> `None`).

use std::sync::Arc;

use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;
use onionbit_crypto::hash::InfoHashV1;

/// Assez de torrents pour que plusieurs `next_id`/`store` se
/// chevauchent meme sur une machine rapide.
const N_TORRENTS: usize = 32;
/// Rounds repetes : la course reste probabiliste, on la rejoue.
const ROUNDS: usize = 3;

async fn make_torrent(seed_dir: &std::path::Path, i: usize) -> (Vec<u8>, InfoHashV1) {
    let payload: Vec<u8> = (0..2_000_000u32)
        .map(|j| ((j + i as u32 * 7) % 251) as u8)
        .collect();
    let path = seed_dir.join(format!("f{i}.bin"));
    std::fs::write(&path, &payload).unwrap();
    let torrent = librqbit::create_torrent(
        &path,
        librqbit::CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .expect("create_torrent");
    let bytes = torrent.as_bytes().unwrap().to_vec();
    let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
    (bytes, meta.info_hash)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn adds_concurrents_ids_uniques() {
    for round in 0..ROUNDS {
        let seed_dir = tempfile::tempdir().unwrap();
        let dl_dir = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
        // La persistance est la condition du bug : sans elle `next_id`
        // atomique servait deja les ids. Elle n'est cablee que si
        // `fastresume` est actif (`offline` le coupe) — les deux
        // reglages sont requis.
        cfg.fastresume = true;
        cfg.persistence_dir = Some(dl_dir.path().join("rqbit"));
        let engine = BtEngine::start(cfg).await.expect("engine");

        let mut torrents = Vec::new();
        for i in 0..N_TORRENTS {
            torrents.push(make_torrent(seed_dir.path(), i).await);
        }

        // Barriere commune : tous les adds entrent dans
        // `add_torrent_internal` au meme instant — la fenetre entre
        // `next_id()` et `store()` se recouvre au maximum.
        let barrier = Arc::new(tokio::sync::Barrier::new(N_TORRENTS));
        let mut set = tokio::task::JoinSet::new();
        for (bytes, ih) in &torrents {
            let e = engine.clone();
            let b = bytes.clone();
            let ih = *ih;
            let bar = barrier.clone();
            set.spawn(async move {
                bar.wait().await;
                (ih, e.add_torrent_bytes(b, false).await)
            });
        }
        let mut results = Vec::new();
        while let Some(r) = set.join_next().await {
            results.push(r.expect("tache add"));
        }

        for (ih, res) in &results {
            let dl = res
                .as_ref()
                .unwrap_or_else(|e| panic!("round {round}: add {ih:?} en erreur: {e}"));
            assert_eq!(
                &dl.info_hash(),
                ih,
                "round {round}: AlreadyManaged du torrent voisin \
                 (le handle retourne n'est pas celui du torrent ajoute)"
            );
        }
        for (_, ih) in &torrents {
            let dl = engine.get_by_hash(ih).unwrap_or_else(|| {
                panic!("round {round}: {ih:?} absent du moteur — jamais insere")
            });
            assert_eq!(&dl.info_hash(), ih, "round {round}: hash incoherent");
        }
        engine.stop().await;
    }
}
