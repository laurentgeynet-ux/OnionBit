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
