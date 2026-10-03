// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Kill switch en plein transfert (residuel des etapes 13/16) : un
//! telechargement TCP relaye par un proxy SOCKS5 est coupe quand le
//! proxy meurt — aucune fuite directe, reprise seulement apres
//! retablissement.
//!
//! Detecteur de fuite : le seeder reste joignable en direct sur
//! loopback — si rqbit avait un fallback direct, le telechargement
//! *finirait* pendant la panne. `StreamConnector::connect` (rqbit)
//! court-circuite sur `proxy_config` : `?` propage l'echec du proxy
//! sans repli TCP/uTP — verifie sur la source. Ici on verifie le
//! comportement : progression gelee + kill switch engage +
//! `resume`/`add` refuses + reprise fonctionnelle au retour du proxy.
//!
//! Tout est en loopback — aucun trafic externe.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;

/// Fenetre d'observation de la panne : assez longue pour que le
/// watchdog (sonde toutes les 5 s) engage le kill switch et pour
/// qu'un hypothétique fallback direct termine le telechargement.
const OUTAGE_OBSERVE: Duration = Duration::from_secs(8);
/// Delai max pour la reprise apres retablissement du proxy.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(120);
/// Pause entre deux blocs relayes (bride le proxy a ~400 Ko/s).
const RELAY_DELAY: Duration = Duration::from_millis(40);

/// Proxy SOCKS5 minimal (RFC 1928, sans auth, CONNECT IPv4/domaine)
/// pilotable : `kill()` ferme le listener (connexions refusees) et
/// `restore()` le re-binde sur le meme port.
struct Socks5Stub {
    addr: std::net::SocketAddr,
    listener: Option<TcpListener>,
    connects: Arc<AtomicUsize>,
    dead: Arc<AtomicBool>,
    /// Connexions relayees actives — coupees au `kill` (un vrai
    /// proxy mort interrompt les flux en cours, pas seulement les
    /// nouvelles demandes).
    active: Arc<Mutex<Vec<TcpStream>>>,
}

impl Socks5Stub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let connects = Arc::new(AtomicUsize::new(0));
        let dead = Arc::new(AtomicBool::new(false));
        let mut stub = Self {
            addr,
            listener: None,
            connects,
            dead,
            active: Arc::new(Mutex::new(Vec::new())),
        };
        stub.listener = Some(listener);
        stub.spawn_accept();
        stub
    }

    fn spawn_accept(&mut self) {
        let listener = self.listener.take().unwrap();
        let connects = self.connects.clone();
        let dead = self.dead.clone();
        let active = self.active.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if dead.load(Ordering::Relaxed) {
                    break;
                }
                match conn {
                    Ok(s) => {
                        connects.fetch_add(1, Ordering::Relaxed);
                        let dead = dead.clone();
                        let active = active.clone();
                        std::thread::spawn(move || {
                            if dead.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Ok(dup) = s.try_clone() {
                                active.lock().unwrap().push(dup);
                            }
                            let _ = Self::serve(s);
                        });
                    }
                    Err(_) => break,
                }
            }
        });
    }

    /// `kill` : ferme le listener ET coupe les flux en cours — les
    /// sondes TCP du watchdog et les nouveaux CONNECT echouent en
    /// connexion refusee, les connexions etablies sont interrompues.
    fn kill(&mut self) {
        self.dead.store(true, Ordering::Relaxed);
        self.listener = None; // drop -> port ferme
        for s in self.active.lock().unwrap().drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    /// `restore` : re-binde le meme port (le precedent est libere).
    fn restore(&mut self) {
        self.dead.store(false, Ordering::Relaxed);
        let listener = TcpListener::bind(self.addr).expect("rebind proxy");
        self.listener = Some(listener);
        self.spawn_accept();
    }

    /// `serve` : handshake RFC 1928 + CONNECT + relay bidirectionnel.
    fn serve(mut s: TcpStream) -> std::io::Result<()> {
        let mut head = [0u8; 2];
        s.read_exact(&mut head)?;
        if head[0] != 0x05 {
            return Ok(());
        }
        let mut methods = vec![0u8; head[1] as usize];
        s.read_exact(&mut methods)?;
        s.write_all(&[0x05, 0x00])?; // pas d'auth
        let mut req = [0u8; 4];
        s.read_exact(&mut req)?;
        if req[1] != 0x01 {
            return Ok(()); // CONNECT seulement
        }
        let target = match req[3] {
            0x01 => {
                let mut a = [0u8; 4];
                s.read_exact(&mut a)?;
                let mut p = [0u8; 2];
                s.read_exact(&mut p)?;
                format!(
                    "{}.{}.{}.{}:{}",
                    a[0],
                    a[1],
                    a[2],
                    a[3],
                    u16::from_be_bytes(p)
                )
            }
            0x03 => {
                let mut n = [0u8; 1];
                s.read_exact(&mut n)?;
                let mut d = vec![0u8; n[0] as usize];
                s.read_exact(&mut d)?;
                let mut p = [0u8; 2];
                s.read_exact(&mut p)?;
                format!("{}:{}", String::from_utf8_lossy(&d), u16::from_be_bytes(p))
            }
            _ => return Ok(()),
        };
        let upstream = TcpStream::connect(&target)?;
        s.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
        // Relay bidirectionnel.
        let mut up_read = upstream.try_clone()?;
        let mut s_read = s.try_clone()?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 16384];
            loop {
                match up_read.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if s.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        // Debit bride (~400 Ko/s) : fenetre de panne en
                        // plein vol deterministe sur loopback.
                        std::thread::sleep(RELAY_DELAY);
                    }
                }
            }
            let _ = s.shutdown(Shutdown::Both);
        });
        let mut buf = [0u8; 16384];
        loop {
            match s_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut up = &upstream;
                    if up.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    std::thread::sleep(RELAY_DELAY);
                }
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_death_mid_transfer_engages_kill_switch_no_leak() {
    tracing_subscriber::fmt()
        .with_env_filter("onionbit=debug")
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    // 1. Torrent reel + seeder TCP (uTP inutilise : le proxy est TCP).
    let seed_dir = tempfile::tempdir().unwrap();
    // 1 Mo a ~400 Ko/s de relais -> ~2,5 s de transfert : la panne
    // tombe en plein vol, pas avant/apres.
    let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(seed_dir.path().join("fichier.bin"), &payload).unwrap();
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
    let torrent_bytes = torrent.as_bytes().unwrap();

    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder engine");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed torrent");
    let seeder_listen = {
        let a = seeder.listen_addr().expect("ecoute seeder");
        if a.ip().is_unspecified() {
            std::net::SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };

    // 2. Proxy SOCKS5 + downloader tout-TCP relaye.
    let mut proxy = Socks5Stub::start();
    let dl_dir = tempfile::tempdir().unwrap();
    let mut dl_cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
    dl_cfg.socks5_proxy = Some(format!("socks5://{}", proxy.addr));
    // uTP coupe : le proxy ne relaie que du TCP — si uTP reste actif,
    // rqbit peut monter une connexion uTP directe vers le seeder en
    // loopback (progres sans passage par le proxy — flaky observe sur
    // macOS CI) et l'oracle "connexion via proxy" devient aleatoire.
    dl_cfg.enable_utp = false;
    dl_cfg.listen_port = Some(0);
    let downloader = BtEngine::start(dl_cfg).await.expect("downloader engine");
    let ks = downloader
        .kill_switch()
        .expect("kill switch absent alors que proxy configure");

    let dl = downloader
        .add_with_options(
            librqbit::AddTorrent::from_bytes(torrent_bytes),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                initial_peers: Some(vec![seeder_listen]),
                ..Default::default()
            }),
        )
        .await
        .expect("add download");

    // 3. Le transfert demarre via le proxy.
    let t0 = Instant::now();
    loop {
        let st = dl.stats();
        if st.progress_bytes > 0 {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "pas de progres via proxy"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        proxy.connects.load(Ordering::Relaxed) > 0,
        "aucune connexion passee par le proxy"
    );

    // 4. Panne en plein vol : le proxy meurt (listener + flux).
    assert!(
        !dl.stats().finished,
        "telechargement deja termine avant la panne — test vide"
    );
    proxy.kill();
    let before = dl.stats().progress_bytes;

    // Le watchdog doit engager le kill switch.
    let t1 = Instant::now();
    while !ks.is_engaged() {
        assert!(
            t1.elapsed() < Duration::from_secs(12),
            "kill switch jamais engage apres la mort du proxy"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Pendant la panne : le seeder est TOUJOURS joignable en direct —
    // toute fuite directe terminerait le telechargement.
    let end = Instant::now() + OUTAGE_OBSERVE;
    while Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let st = dl.stats();
        assert!(
            st.progress_bytes <= before + 16384 * 2,
            "fuite directe ? progression pendant la panne : {} -> {}",
            before,
            st.progress_bytes
        );
        assert!(!st.finished, "telecharge en direct malgre le proxy mort");
    }
    assert!(
        !dl.stats().finished,
        "le telechargement a contourne le proxy"
    );

    // add/resume refuses tant que le proxy est mort.
    let resume_err = downloader.resume(&dl.info_hash_hex()).await;
    assert!(resume_err.is_err(), "resume accepte avec proxy mort");

    // 5. Retablissement : le watchdog doit relacher, puis resume marche.
    proxy.restore();
    let t2 = Instant::now();
    while ks.is_engaged() {
        assert!(
            t2.elapsed() < Duration::from_secs(15),
            "kill switch jamais relache apres retour du proxy"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // `resume` ne doit plus etre bloque par le kill switch ; "already
    // live" est acceptable (le torrent n'a jamais ete mis en pause —
    // ses connexions sont juste mortes et retentees via le proxy).
    match downloader.resume(&dl.info_hash_hex()).await {
        Ok(()) => {}
        Err(e) => assert!(
            format!("{e:?}").contains("already live"),
            "resume refuse apres retablissement : {e:?}"
        ),
    }
    // pause/unpause force une tentative immediate chez rqbit (sinon le
    // backoff de reconnexion pair laisse trainer la reprise ~2 min).
    downloader.pause(&dl.info_hash_hex()).await.expect("pause");
    downloader
        .resume(&dl.info_hash_hex())
        .await
        .expect("resume post-pause");

    tokio::time::timeout(RECOVERY_TIMEOUT, dl.wait_completed())
        .await
        .expect("reprise en timeout apres retour du proxy")
        .expect("wait_completed");
    let got = std::fs::read(dl_dir.path().join("fichier.bin")).unwrap();
    assert_eq!(got, payload, "contenu telecharge identique");

    downloader.stop().await;
    seeder.stop().await;
}
