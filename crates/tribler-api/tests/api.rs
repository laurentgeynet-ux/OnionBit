//! Tests d'integration HTTP de `tribler-api`.
//!
//! Regle projet : aucun trafic sortant — le serveur axum est lie a
//! `127.0.0.1:0` (port ephemere) et le client `reqwest` ne vise que
//! cette adresse.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;

use tribler_api::{build, AppState};
use tribler_core::{CoreConfig, CoreSession, Notifier};

/// Timeout global des lectures reseau en test (loopback : generouse
/// mais borne pour detecter un blocage).
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Accumule les trames SSE jusqu'a ce que `buf` contienne `needle`
/// (borne par `IO_TIMEOUT` pour detecter un blocage).
async fn read_until(buf: &mut String, resp: &mut reqwest::Response, needle: &str) {
    tokio::time::timeout(IO_TIMEOUT, async {
        while !buf.contains(needle) {
            match resp.chunk().await.unwrap() {
                Some(c) => buf.push_str(&String::from_utf8_lossy(&c)),
                None => panic!("flux SSE ferme avant de voir {needle}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timeout en attendant {needle} ; buffer={buf}"));
}

/// .torrent minimal valide (info dict : fichier unique de 42 octets,
/// une piece factice).
fn test_torrent_bytes() -> Vec<u8> {
    use tribler_format::bencode::{encode, BValue};
    let mut info = std::collections::BTreeMap::new();
    info.insert(b"length".to_vec(), BValue::Int(42));
    info.insert(b"name".to_vec(), BValue::Bytes(b"api-test.bin".to_vec()));
    info.insert(b"piece length".to_vec(), BValue::Int(16384));
    info.insert(b"pieces".to_vec(), BValue::Bytes(vec![0u8; 20]));
    let mut root = std::collections::BTreeMap::new();
    root.insert(b"info".to_vec(), BValue::Dict(info));
    encode(&BValue::Dict(root))
}

/// Serveur de test monte sur loopback.
struct TestServer {
    addr: SocketAddr,
    session: CoreSession,
    client: reqwest::Client,
    /// Garde le repertoire temporaire vivant.
    _dir: tempfile::TempDir,
}

async fn spawn_server() -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let app = build(AppState {
        session: session.clone(),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        addr,
        session,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

#[tokio::test]
async fn get_downloads_liste_vide() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["downloads"], serde_json::json!([]));
    assert_eq!(body["checkpoints"]["all_loaded"], true);
    srv.session.stop().await;
}

#[tokio::test]
async fn put_downloads_sans_uri_ou_torrent_retourne_400() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Format d'erreur Tribler : {"error": {"handled": ..., "message": ...}}
    assert_eq!(body["error"]["handled"], true);
    assert!(body["error"]["message"].is_string());
    srv.session.stop().await;
}

#[tokio::test]
async fn cycle_complet_ajout_pause_resume_suppression() {
    let srv = spawn_server().await;

    // Ecrit le .torrent minimal dans le repertoire temporaire.
    let torrent_path = srv._dir.path().join("api-test.torrent");
    std::fs::write(&torrent_path, test_torrent_bytes()).unwrap();

    // PUT /api/downloads {"torrent": <chemin>}
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({"torrent": torrent_path.display().to_string()}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["started"], true);
    let infohash = body["infohash"].as_str().unwrap().to_string();
    assert_eq!(infohash.len(), 40);

    // GET /api/downloads?infohash=<ih> : l'entree est visible.
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads?infohash={infohash}")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let downloads = body["downloads"].as_array().unwrap();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0]["infohash"], infohash);
    assert_eq!(downloads[0]["name"], "api-test.bin");

    // PATCH stop puis resume.
    for state in ["stop", "resume"] {
        let resp = srv
            .client
            .patch(srv.url(&format!("/api/downloads/{infohash}")))
            .json(&serde_json::json!({"state": state}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "PATCH {state}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["modified"], true);
    }

    // DELETE.
    let resp = srv
        .client
        .delete(srv.url(&format!("/api/downloads/{infohash}")))
        .json(&serde_json::json!({"remove_data": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["removed"], true);

    // La liste est de nouveau vide.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["downloads"], serde_json::json!([]));

    srv.session.stop().await;
}

#[tokio::test]
async fn patch_infohash_inconnu_retourne_404() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .patch(srv.url("/api/downloads/0000000000000000000000000000000000000000"))
        .json(&serde_json::json!({"state": "stop"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["handled"], true);
    srv.session.stop().await;
}

#[tokio::test]
async fn events_sse_format_tribler() {
    let srv = spawn_server().await;
    let mut resp = srv.client.get(srv.url("/api/events")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.starts_with("text/event-stream"), "content-type={ct}");

    let mut buf = String::new();

    // Premier evenement : `events_start` au format `event: x\ndata: y\n\n`.
    read_until(&mut buf, &mut resp, "events_start").await;
    let frame_end = buf.find("\n\n").unwrap();
    let first_frame = &buf[..frame_end];
    let mut lines = first_frame.lines();
    assert_eq!(lines.next(), Some("event: events_start"));
    let data = lines.next().unwrap();
    assert!(data.starts_with("data: "));
    let payload: serde_json::Value = serde_json::from_str(&data["data: ".len()..]).unwrap();
    assert!(payload["version"].is_string());

    // Ajoute un torrent : la boucle de progression emet
    // `download_state_changed` en moins d'une periode (1 s).
    srv.session
        .add_torrent_bytes(test_torrent_bytes(), true)
        .await
        .unwrap();
    read_until(&mut buf, &mut resp, "event: download_state_changed").await;

    srv.session.stop().await;
}
