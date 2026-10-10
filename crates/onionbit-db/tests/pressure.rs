// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Banc de pression SQLite : reproduit la charge reelle d'un
//! telechargement (tick de stats + flush du ledger + lectures de la
//! file) sur une base fichier en WAL, et mesure l'effet du
//! regroupement en transaction et de la contention disque.
//!
//! Volontairement `#[ignore]` — execution a la demande :
//!
//! ```powershell
//! cargo test -p onionbit-db --test pressure --release -- --ignored --nocapture
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_db::models::DownloadRow;
use onionbit_db::Database;

/// Nombre de telechargements dans la base du banc (ordre de grandeur
/// d'une session reelle).
const N_DOWNLOADS: usize = 20;
/// Pairs « dirty » par flush du ledger (le tick reql des tunnels en
/// persiste une a une — voir `peer_stats.rs::flush`).
const N_PEERS: usize = 200;
/// Duree de chaque scenario.
const DURATION: Duration = Duration::from_secs(4);

fn dl_row(i: usize) -> DownloadRow {
    let mut infohash = vec![0u8; 20];
    infohash[..8].copy_from_slice(&(i as u64).to_be_bytes());
    DownloadRow {
        rowid: 0,
        infohash,
        name: Some(format!("contenu-{i}")),
        source_uri: String::new(),
        torrent_data: None,
        output_dir: "downloads".to_string(),
        added_on: 0,
        paused: false,
        finished: false,
        anon_hops: 0,
        safe_seeding: false,
        user_stopped: false,
        upload_limit: 0,
        download_limit: 0,
        seeding_ratio: None,
        auto_managed: true,
        queue_position: i as i64,
        completed_dir: None,
        storage_area: "public".to_string(),
        origin: "user".to_string(),
        selected_files: None,
        file_priorities: None,
        extra_trackers: Vec::new(),
        removed_trackers: Vec::new(),
        time_finished: 0,
        channel_download: false,
        add_download_to_channel: false,
        total_uploaded: 0,
        total_downloaded: 0,
    }
}

fn seed(db: &Database) -> Vec<Vec<u8>> {
    db.with(|c| {
        for i in 0..N_DOWNLOADS {
            onionbit_db::downloads::upsert(c, &dl_row(i))?;
        }
        Ok(())
    })
    .expect("seed");
    (0..N_DOWNLOADS).map(|i| dl_row(i).infohash).collect()
}

/// Latences d'une rafale d'operations (echantillonnees).
#[derive(Default)]
struct Samples {
    us: Mutex<Vec<u64>>,
}

impl Samples {
    fn push(&self, d: Duration) {
        self.us.lock().unwrap().push(d.as_micros() as u64);
    }

    fn rapport(&self, nom: &str) {
        let mut v = std::mem::take(&mut *self.us.lock().unwrap());
        if v.is_empty() {
            println!("{nom}: aucune operation");
            return;
        }
        v.sort_unstable();
        let pct = |p: usize| v[(v.len() * p / 100).min(v.len() - 1)];
        let total: u64 = v.iter().sum();
        println!(
            "{nom}: {} ops  p50={}us p95={}us p99={}us max={}us  moy={}us",
            v.len(),
            pct(50),
            pct(95),
            pct(99),
            v[v.len() - 1],
            total / v.len() as u64,
        );
    }
}

/// Fil « tick de stats » : `add_transferred` par torrent.
/// `batched=true` regroupe les N ecritures en UNE acquisition du mutex
/// (une transaction) — le modele corrige ; `false` reproduit le
/// `.with` par torrent actuel.
fn tick_writer(db: &Arc<Database>, ihs: &[Vec<u8>], batched: bool, s: &Samples, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        if batched {
            let _ = db.with(|c| {
                let tx = c.unchecked_transaction()?;
                for ih in ihs {
                    onionbit_db::downloads::add_transferred(&tx, ih, 16_384, 1_048_576)?;
                }
                tx.commit()?;
                Ok(())
            });
        } else {
            for ih in ihs {
                let _ =
                    db.with(|c| onionbit_db::downloads::add_transferred(c, ih, 16_384, 1_048_576));
            }
        }
        s.push(t0.elapsed());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Fil « flush ledger » : upserts `peer_stats` — a l'unite (modele
/// actuel) ou en transaction unique.
fn peers_writer(db: &Arc<Database>, batched: bool, s: &Samples, stop: &AtomicBool) {
    let rows: Vec<onionbit_db::peer_stats::PeerStatRow> = (0..N_PEERS)
        .map(|i| {
            let mut public_key = vec![0u8; 32];
            public_key[..8].copy_from_slice(&(i as u64).to_be_bytes());
            onionbit_db::peer_stats::PeerStatRow {
                public_key,
                bytes_served: 1_000_000,
                bytes_used: 500_000,
                circuits_served: 10,
                circuits_used: 5,
                first_seen: 1,
                last_seen: 2,
            }
        })
        .collect();
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        if batched {
            let _ = db.with(|c| {
                let tx = c.unchecked_transaction()?;
                for r in &rows {
                    onionbit_db::peer_stats::upsert(&tx, r)?;
                }
                tx.commit()?;
                Ok(())
            });
        } else {
            for r in &rows {
                let _ = db.with(|c| onionbit_db::peer_stats::upsert(c, r));
            }
        }
        s.push(t0.elapsed());
    }
}

/// Fil « lecteur » : `downloads::list` comme `enforce_queue_limits`
/// a chaque tick — les lectures doivent rester rapides meme sous
/// pression d'ecriture.
fn list_reader(db: &Arc<Database>, s: &Samples, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        let _ = db.with(onionbit_db::downloads::list);
        s.push(t0.elapsed());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Fil « disque » : ecriture sequentielle d'un fichier de 256 Mio en
/// boucle sur le meme volume — approxime la contention entre les
/// ecritures torrent et le WAL SQLite.
fn disk_hammer(dir: &std::path::Path, stop: &AtomicBool) {
    let path = dir.join("hammer.bin");
    let buf = vec![0xabu8; 1024 * 1024];
    while !stop.load(Ordering::Relaxed) {
        let mut f = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        use std::io::Write;
        for _ in 0..256 {
            if stop.load(Ordering::Relaxed) || f.write_all(&buf).is_err() {
                return;
            }
        }
        let _ = f.sync_all();
    }
}

fn scenario(nom: &str, batched: bool, hammer: bool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Database::open(&dir.path().join("bench.db")).expect("open"));
    let ihs = seed(&db);
    let stop = AtomicBool::new(false);
    let tick_s = Samples::default();
    let peers_s = Samples::default();
    let list_s = Samples::default();

    std::thread::scope(|scope| {
        scope.spawn(|| tick_writer(&db, &ihs, batched, &tick_s, &stop));
        scope.spawn(|| peers_writer(&db, batched, &peers_s, &stop));
        scope.spawn(|| list_reader(&db, &list_s, &stop));
        if hammer {
            scope.spawn(|| disk_hammer(dir.path(), &stop));
        }
        std::thread::sleep(DURATION);
        stop.store(true, Ordering::Relaxed);
    });

    println!("--- {nom} ---");
    tick_s.rapport("tick   (20x add_transferred)");
    peers_s.rapport("ledger (200x peer_stats upsert)");
    list_s.rapport("lecture downloads::list        ");
}

/// Mesure le cout structurel du modele « un `.with` par operation »
/// versus la transaction groupee, puis la sensibilite a la
/// contention disque — c'est le couplage des deux qui produisait les
/// stalls multi-secondes observes en session.
#[test]
#[ignore = "banc de pression — cargo test -p onionbit-db --test pressure --release -- --ignored --nocapture"]
fn bench_pression_sqlite() {
    scenario("par operation (modele actuel)", false, false);
    scenario("transaction groupee            ", true, false);
    scenario("par operation + disque sature  ", false, true);
    scenario("transaction groupee + disque   ", true, true);
}
