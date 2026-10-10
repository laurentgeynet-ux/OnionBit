// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests des services secondaires (etape 14) : watch folder,
//! torrent checker (BEP-15 UDP loopback), RSS (serveur HTTP
//! loopback). Aucun trafic sortant.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_core::config::CoreConfig;
use onionbit_core::notifier::{Notification, Notifier};
use onionbit_core::services::rss::RssManager;
use onionbit_core::services::torrent_checker::TorrentChecker;
use onionbit_core::services::watch_folder::WatchFolderService;
use onionbit_core::session::CoreSession;
use onionbit_db::Database;

const WAIT: Duration = Duration::from_secs(8);

/// `.torrent` minimal valide (info dict mono-fichier).
fn torrent_bytes() -> Vec<u8> {
    let mut pieces = vec![0u8; 20];
    pieces[0] = 0xAB;
    let mut out =
        b"d8:announce0:4:infod6:lengthi64e4:name8:test.bin12:piece lengthi16384e6:pieces20:"
            .to_vec();
    out.extend_from_slice(&pieces);
    out.extend_from_slice(b"ee");
    out
}

async fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while !f() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    f()
}

/// Serveur HTTP minimal loopback : `routes` = chemin -> corps
/// (`{port}` est substitue par le port reel dans les corps).
fn spawn_http(routes: Vec<(String, Vec<u8>)>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("/")
                .to_string();
            let body = routes
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, b)| b.clone());
            let (status, body) = match body {
                Some(b) => (
                    "200 OK",
                    // Substitution uniquement sur les corps texte (les
                    // octets du .torrent restent intacts).
                    match String::from_utf8(b.clone()) {
                        Ok(t) => t.replace("{port}", &port.to_string()).into_bytes(),
                        Err(_) => b,
                    },
                ),
                None => ("404 Not Found", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    port
}

/// Watch folder : un `.torrent` pose dans le repertoire est importe.
#[tokio::test(flavor = "multi_thread")]
async fn watch_folder_imports_torrent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CoreConfig::offline(dir.path().join("state"));
    let session = CoreSession::start_offline(cfg, Notifier::new())
        .await
        .unwrap();
    let watch_dir = dir.path().join("watch");
    std::fs::create_dir_all(&watch_dir).unwrap();
    let svc = WatchFolderService::new(session.clone(), watch_dir.clone(), Duration::from_secs(60));

    std::fs::write(watch_dir.join("a.torrent"), torrent_bytes()).unwrap();
    let n = svc.check().await;
    assert_eq!(n, 1, "un fichier traite");
    let ok = wait_for(|| !session.downloads().is_empty()).await;
    assert!(ok, "telechargement non ajoute");
    svc.stop();
    session.stop().await;
}

/// Stub de tracker UDP BEP-15 loopback : connect puis scrape.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_udp_scrape() {
    let tracker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, src)) = tracker.recv_from(&mut buf).await {
            let data = &buf[..n];
            let action = i32::from_be_bytes(data[8..12].try_into().unwrap());
            let txn = &data[12..16];
            let mut resp = Vec::new();
            if action == 0 {
                // connect -> !i action=0, i txn, q conn_id
                resp.extend_from_slice(&0i32.to_be_bytes());
                resp.extend_from_slice(txn);
                resp.extend_from_slice(&42i64.to_be_bytes());
            } else if action == 2 {
                // scrape -> !i action=2, i txn, !iii par infohash
                resp.extend_from_slice(&2i32.to_be_bytes());
                resp.extend_from_slice(txn);
                let n_ih = (n - 16) / 20;
                for _ in 0..n_ih {
                    resp.extend_from_slice(&7i32.to_be_bytes()); // seeders
                    resp.extend_from_slice(&1i32.to_be_bytes()); // downloaded
                    resp.extend_from_slice(&3i32.to_be_bytes()); // leechers
                }
            }
            let _ = tracker.send_to(&resp, src).await;
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    let notifier = Notifier::new();
    let checker = TorrentChecker::new(
        db,
        notifier,
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    let mut ih = [0u8; 20];
    ih[0] = 0x42;
    let healths = checker
        .check_tracker(&format!("udp://{tracker_addr}"), &[ih])
        .await
        .unwrap();
    assert_eq!(healths.len(), 1);
    assert_eq!(healths[0].seeders, 7);
    assert_eq!(healths[0].leechers, 3);
    assert!(healths[0].self_checked);
}

/// Plus de 32 infohashes : `check_tracker` decoupe en lots de 32 —
/// avant, tout ce qui depassait etait silencieusement tronque.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_udp_scrape_decoupe_en_lots() {
    use std::sync::atomic::Ordering;
    let tracker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    let n_scrapes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let n_scrapes = n_scrapes.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((n, src)) = tracker.recv_from(&mut buf).await {
                let data = &buf[..n];
                let action = i32::from_be_bytes(data[8..12].try_into().unwrap());
                let txn = &data[12..16];
                let mut resp = Vec::new();
                if action == 0 {
                    resp.extend_from_slice(&0i32.to_be_bytes());
                    resp.extend_from_slice(txn);
                    resp.extend_from_slice(&42i64.to_be_bytes());
                } else if action == 2 {
                    n_scrapes.fetch_add(1, Ordering::SeqCst);
                    resp.extend_from_slice(&2i32.to_be_bytes());
                    resp.extend_from_slice(txn);
                    for _ in 0..(n - 16) / 20 {
                        resp.extend_from_slice(&5i32.to_be_bytes());
                        resp.extend_from_slice(&0i32.to_be_bytes());
                        resp.extend_from_slice(&2i32.to_be_bytes());
                    }
                }
                let _ = tracker.send_to(&resp, src).await;
            }
        });
    }

    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    let checker = TorrentChecker::new(
        db,
        Notifier::new(),
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    let ihs: Vec<[u8; 20]> = (0..40u8).map(|i| [i; 20]).collect();
    let healths = checker
        .check_tracker(&format!("udp://{tracker_addr}"), &ihs)
        .await
        .unwrap();
    assert_eq!(healths.len(), 40, "les 40 infohashes sont scrapes");
    assert_eq!(n_scrapes.load(Ordering::SeqCst), 2, "2 lots de 32+8");
}

/// Invariant de non-fuite (docs/security/threat_model.md) : un
/// infohash en telechargement/seeding anonyme (`downloads.anon_hops >
/// 0`) ne doit JAMAIS etre scrape en clair — le tracker apprendrait
/// IP reelle <-> contenu. `check_tracker` ignore l'infohash et
/// `check_oldest` saute la ligne : zero datagramme vers le tracker.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_never_scrapes_anonymous_infohash() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Tracker BEP-15 espion : compte tout datagramme recu.
    let tracker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    let received = Arc::new(AtomicUsize::new(0));
    {
        let received = received.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while tracker.recv_from(&mut buf).await.is_ok() {
                received.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    let mut ih = [0u8; 20];
    ih[0] = 0x42;
    let tracker_url = format!("udp://{tracker_addr}");
    // Download anonyme persiste + entree de sante (comme si le
    // torrent avait ete vu dans un catalogue) + lien tracker.
    db.with(|c| {
        onionbit_db::downloads::upsert(
            c,
            &onionbit_db::models::DownloadRow {
                infohash: ih.to_vec(),
                name: Some("secret.bin".to_string()),
                anon_hops: 2,
                ..Default::default()
            },
        )?;
        onionbit_db::health::link_tracker(c, &ih, &tracker_url)
    })
    .unwrap();

    let checker = TorrentChecker::new(
        db,
        Notifier::new(),
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    // Chemin API (`GET /metadata/torrents/{ih}/health?refresh=1`).
    let healths = checker.check_tracker(&tracker_url, &[ih]).await.unwrap();
    assert!(healths.is_empty(), "un swarm anonyme n'est pas scrape");

    // Chemin periodique : la ligne anonyme est sautee.
    let checked = checker.check_oldest().await.unwrap();
    assert_eq!(checked, 0, "la rotation ne doit pas retenir l'anonyme");

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        received.load(Ordering::SeqCst),
        0,
        "le tracker a recu un datagramme — fuite IP <-> infohash"
    );
}

/// Rotation : un torrent ajoute par `.torrent`/magnet n'a aucun
/// tracker lie (`torrent_state_tracker` n'est rempli qu'au retour
/// d'un scrape) — `check_oldest` retombe sur les trackers propres de
/// la ligne `downloads` (`tr` du magnet, announce du `.torrent`) et
/// le scrape les lie ensuite.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_replie_sur_trackers_propres() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let tracker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap();
    let received = Arc::new(AtomicUsize::new(0));
    {
        let received = received.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((n, src)) = tracker.recv_from(&mut buf).await {
                received.fetch_add(1, Ordering::SeqCst);
                let data = &buf[..n];
                let action = i32::from_be_bytes(data[8..12].try_into().unwrap());
                let txn = &data[12..16];
                let mut resp = Vec::new();
                if action == 0 {
                    resp.extend_from_slice(&0i32.to_be_bytes());
                    resp.extend_from_slice(txn);
                    resp.extend_from_slice(&42i64.to_be_bytes());
                } else if action == 2 {
                    resp.extend_from_slice(&2i32.to_be_bytes());
                    resp.extend_from_slice(txn);
                    let n_ih = (n - 16) / 20;
                    for _ in 0..n_ih {
                        resp.extend_from_slice(&9i32.to_be_bytes()); // seeders
                        resp.extend_from_slice(&1i32.to_be_bytes()); // downloaded
                        resp.extend_from_slice(&4i32.to_be_bytes()); // leechers
                    }
                }
                let _ = tracker.send_to(&resp, src).await;
            }
        });
    }

    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    let mut ih = [0u8; 20];
    ih[0] = 0x43;
    // Download en clair dont le magnet porte son propre tracker ;
    // `torrent_state` existe (catalogue) mais aucun tracker lie.
    db.with(|c| {
        onionbit_db::downloads::upsert(
            c,
            &onionbit_db::models::DownloadRow {
                infohash: ih.to_vec(),
                name: Some("manuel.bin".to_string()),
                source_uri: format!(
                    "magnet:?xt=urn:btih:{}&tr=udp%3A%2F%2F127.0.0.1%3A{}",
                    hex::encode(ih),
                    tracker_addr.port()
                ),
                ..Default::default()
            },
        )?;
        onionbit_db::health::upsert_torrent_state(c, &ih)?;
        Ok(())
    })
    .unwrap();

    let checker = TorrentChecker::new(
        db.clone(),
        Notifier::new(),
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    let checked = checker.check_oldest().await.unwrap();
    assert_eq!(checked, 1, "le tracker propre du torrent est scrape");
    assert!(
        received.load(Ordering::SeqCst) >= 2,
        "connect + scrape emis vers le tracker du magnet"
    );

    // Sante enregistree et tracker desormais lie : les prochains
    // passages de la rotation utiliseront `torrent_state_tracker`.
    let (seeders, linked) = db
        .with(|c| {
            let st = onionbit_db::health::get_torrent_state(c, &ih)?.unwrap();
            Ok((st.seeders, onionbit_db::health::trackers_of(c, &ih)?))
        })
        .unwrap();
    assert_eq!(seeders, 9);
    assert_eq!(linked, vec![format!("udp://{tracker_addr}")]);
}

/// Une ligne `torrent_state` orpheline (recue par gossip, jamais
/// telechargee, aucun tracker connu) est marquee comme controlee
/// plutot que repick a chaque tick — sinon `last_check=0` la rendait
/// eternellement la plus vieille et affamait toute la rotation.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_consomme_ligne_sans_tracker() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    let mut ih = [0u8; 20];
    ih[0] = 0x44;
    db.with(|c| onionbit_db::health::upsert_torrent_state(c, &ih))
        .unwrap();

    let checker = TorrentChecker::new(
        db.clone(),
        Notifier::new(),
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    assert_eq!(checker.check_oldest().await.unwrap(), 0);
    let last_check = db
        .with(|c| {
            Ok(onionbit_db::health::get_torrent_state(c, &ih)?
                .unwrap()
                .last_check)
        })
        .unwrap();
    assert!(last_check > 0, "la ligne sans tracker est consommee");
    assert_eq!(
        checker.check_oldest().await.unwrap(),
        0,
        "plus rien d'eligible : la rotation n'est pas bloquee"
    );
}

/// Selection `check_selected` (ADR-0025 etape 99, `torrents_to_check`
/// Python) : `pool_size` lignes **perimees** sont consommees par tick ;
/// les lignes fraiches et les swarms anonymes restent hors selection.
#[tokio::test(flavor = "multi_thread")]
async fn torrent_checker_selection_consomme_pool_borne() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("t.db")).unwrap());
    // 6 lignes perimees (last_check = 0), 1 fraiche, 1 perimee mais
    // anonyme (`anon_hops > 0` — jamais scrapee, jamais touchee).
    let mut stale_ihs = Vec::new();
    for i in 0..6u8 {
        let mut ih = [0u8; 20];
        ih[0] = 0x50 + i;
        stale_ihs.push(ih);
        db.with(move |c| onionbit_db::health::upsert_torrent_state(c, &ih))
            .unwrap();
    }
    let mut fresh_ih = [0u8; 20];
    fresh_ih[0] = 0x60;
    let mut anon_ih = [0u8; 20];
    anon_ih[0] = 0x61;
    db.with(move |c| {
        onionbit_db::health::upsert_torrent_state(c, &fresh_ih)?;
        c.execute(
            "UPDATE torrent_state SET last_check = ?1 WHERE infohash = ?2",
            rusqlite::params![999_999_999i64, fresh_ih.as_slice()],
        )?;
        onionbit_db::downloads::upsert(
            c,
            &onionbit_db::models::DownloadRow {
                infohash: anon_ih.to_vec(),
                name: Some("anon.bin".to_string()),
                anon_hops: 1,
                ..Default::default()
            },
        )?;
        onionbit_db::health::upsert_torrent_state(c, &anon_ih)
    })
    .unwrap();

    let checker = TorrentChecker::new(
        db.clone(),
        Notifier::new(),
        onionbit_network_policy::IpPolicy::permissive(),
    )
    .await
    .unwrap();

    // Aucun tracker connu : chaque selectionnee est « touchee »
    // (last_check maj) sans scrape — 3 sur 6 consommees.
    let checked = checker.check_selected(3, 3600).await.unwrap();
    assert_eq!(checked, 0, "pas de tracker : rien de scrape");
    let (remaining, fresh_check, anon_check) = db
        .with(|c| {
            let mut n_stale = 0usize;
            for ih in &stale_ihs {
                let st = onionbit_db::health::get_torrent_state(c, ih)?.unwrap();
                if st.last_check == 0 {
                    n_stale += 1;
                }
            }
            Ok((
                n_stale,
                onionbit_db::health::get_torrent_state(c, &fresh_ih)?
                    .unwrap()
                    .last_check,
                onionbit_db::health::get_torrent_state(c, &anon_ih)?
                    .unwrap()
                    .last_check,
            ))
        })
        .unwrap();
    assert_eq!(remaining, 3, "pool_size = 3 : 3 lignes consommees");
    assert_eq!(
        fresh_check, 999_999_999,
        "la ligne fraiche n'est pas selectionnee"
    );
    assert_eq!(anon_check, 0, "le swarm anonyme n'est jamais touche");
}

/// RSS : un flux annoncant un `.torrent` -> `TorrentMetadataCreated`.
#[tokio::test(flavor = "multi_thread")]
async fn rss_discovers_torrent_and_notifies() {
    // Le flux pointe vers le meme serveur (`{port}` substitue par le
    // serveur lui-meme).
    let feed = "<?xml version=\"1.0\"?><rss><channel><item>\
        <link>http://127.0.0.1:{port}/x.torrent</link></item></channel></rss>"
        .as_bytes()
        .to_vec();
    let port = spawn_http(vec![
        ("/feed".to_string(), feed),
        ("/x.torrent".to_string(), torrent_bytes()),
    ]);

    let notifier = Notifier::new();
    let mut rx = notifier.subscribe();
    let mgr = RssManager::new(
        notifier,
        onionbit_network_policy::IpPolicy::permissive(),
        Arc::new(Database::memory().unwrap()),
        onionbit_core::asyncio::TaskRegistry::default(),
    );
    mgr.update(&[format!("http://127.0.0.1:{port}/feed")]);

    let ok = tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(Notification::TorrentMetadataCreated { title, .. }) => {
                    break title == "test.bin";
                }
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    mgr.stop();
    assert!(ok, "metadonnee de torrent non notifiee");
}
