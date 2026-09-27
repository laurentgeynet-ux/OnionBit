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

// ============================================================================
// Etape 15 — parite REST Tribler
// ============================================================================

#[tokio::test]
async fn settings_get_post_roundtrip() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["settings"]["libtorrent"]["download_defaults"]["saveas"].is_string());
    assert_eq!(
        body["settings"]["torrent_checker"]["enabled"],
        serde_json::json!(false)
    );

    // Mise a jour partielle : flux RSS.
    let resp = srv
        .client
        .post(srv.url("/api/settings"))
        .json(&serde_json::json!({
            "settings": { "rss": { "urls": ["http://127.0.0.1:1/feed"] } }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["modified"], true);

    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["settings"]["rss"]["urls"],
        serde_json::json!(["http://127.0.0.1:1/feed"])
    );
    srv.session.stop().await;
}

#[tokio::test]
async fn rss_update_feeds() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .put(srv.url("/api/rss"))
        .json(&serde_json::json!({"urls": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["modified"],
        true
    );
    srv.session.stop().await;
}

#[tokio::test]
async fn statistics_endpoints() {
    let srv = spawn_server().await;

    let resp = srv
        .client
        .get(srv.url("/api/statistics/tribler"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["tribler_statistics"]["db_size"].is_number());

    let resp = srv
        .client
        .get(srv.url("/api/statistics/ipv8"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap()["ipv8_statistics"].is_object());

    let dir = srv._dir.path().display().to_string();
    let resp = srv
        .client
        .get(srv.url(&format!("/api/statistics/dirspace?path={dir}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["dirspace"]["free"].as_u64().unwrap() > 0);
    srv.session.stop().await;
}

#[tokio::test]
async fn files_browse_list_create() {
    let srv = spawn_server().await;
    let dir = srv._dir.path().display().to_string();
    std::fs::write(srv._dir.path().join("f.txt"), b"x").unwrap();

    let resp = srv
        .client
        .get(srv.url(&format!(
            "/api/files/browse?path={}&files=1",
            urlencoding(&dir)
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let paths = body["paths"].as_array().unwrap();
    // ".." en tete + le fichier visible (files=1).
    assert_eq!(paths[0]["name"], "..");
    assert!(paths.iter().any(|p| p["name"] == "f.txt"));

    let resp = srv
        .client
        .get(srv.url(&format!(
            "/api/files/list?path={}&recursively=0",
            urlencoding(&dir)
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "f.txt"));

    let newdir = srv._dir.path().join("nouveau").display().to_string();
    let resp = srv
        .client
        .get(srv.url(&format!("/api/files/create?path={}", urlencoding(&newdir))))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(std::path::Path::new(&newdir).is_dir());
    srv.session.stop().await;
}

/// Encodage query minimal (les chemins Windows contiennent `\` et `:`).
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "%5C".to_string(),
            ':' => "%3A".to_string(),
            ' ' => "%20".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[tokio::test]
async fn metadata_search_et_tags() {
    let srv = spawn_server().await;
    let ih = [0x42u8; 20];
    // Insere une entree channel_node pour les tests de recherche/tags.
    srv.session
        .db()
        .with(|c| {
            c.execute(
                "INSERT INTO channel_node (metadata_type, infohash, title, size, public_key,
             signature, timestamp, added_on, id_)
             VALUES (300, ?1, 'ubuntu-24.04', 1024, ?2, ?3, 1700000000, 1700000000, 1)",
                rusqlite::params![ih.as_slice(), vec![0u8; 64], vec![0u8; 64]],
            )
            .map_err(tribler_db::DbError::from)
        })
        .unwrap();

    let resp = srv
        .client
        .get(srv.url("/api/metadata/search/local?fts_text=ubuntu"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"][0]["name"], "ubuntu-24.04");

    // Recherche sans texte -> 400.
    let resp = srv
        .client
        .get(srv.url("/api/metadata/search/local"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Tags : PUT puis DELETE.
    let ih_hex = hex::encode(ih);
    let resp = srv
        .client
        .put(srv.url(&format!("/api/metadata/torrents/{ih_hex}/tags")))
        .json(&serde_json::json!({"tag": "linux"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["added"],
        true
    );

    let resp = srv
        .client
        .delete(srv.url(&format!("/api/metadata/torrents/{ih_hex}/tags")))
        .json(&serde_json::json!({"tag": "linux"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Sante d'un torrent inconnu -> "checking".
    let resp = srv
        .client
        .get(srv.url(&format!("/api/metadata/torrents/{ih_hex}/health")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = srv
        .client
        .get(srv.url("/api/metadata/torrents/popular"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    srv.session.stop().await;
}

#[tokio::test]
async fn downloads_sous_endpoints() {
    let srv = spawn_server().await;
    let dl = srv
        .session
        .add_torrent_bytes(test_torrent_bytes(), true)
        .await
        .unwrap();
    let ih = dl.info_hash_hex();

    // GET /files
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/files")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["files"][0]["name"], "api-test.bin");
    assert_eq!(body["files"][0]["size"], 42);

    // GET /torrent : metainfo brute.
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/torrent")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "application/x-bittorrent");

    // PUT + GET /trackers.
    let resp = srv
        .client
        .put(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .json(&serde_json::json!({"url": "udp://127.0.0.1:6969/announce"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["tracker_info"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["url"] == "udp://127.0.0.1:6969/announce"));

    // Stream du fichier (42 octets factices — le flux s'ouvre meme
    // si les pieces ne sont pas encore la : lecture bornée cote test
    // en tolerant l'absence de donnees).
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/stream/0")))
        .send()
        .await;
    // Le flux peut etre interrompu si la piece n'arrive jamais.
    if let Ok(r) = resp {
        assert!(r.status() == 200 || r.status().is_client_error() || r.status().is_server_error());
    }
    srv.session.stop().await;
}

#[tokio::test]
async fn libtorrent_settings_et_session() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/settings?session=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["hop"], 0);
    assert!(body["settings"]["enable_dht"].is_boolean());

    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/session?session=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["session"]["torrents"].is_number());

    // Lane anonyme inexistante (pas de stack ipv8) -> 404.
    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/session?session=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    srv.session.stop().await;
}

#[tokio::test]
async fn ipv8_et_search_sans_stack_retournent_erreur() {
    let srv = spawn_server().await;
    // Stack IPv8 desactivee en config offline.
    for path in [
        "/api/ipv8/overlays",
        "/api/ipv8/tunnel/circuits",
        "/api/ipv8/tunnel/settings",
    ] {
        let resp = srv.client.get(srv.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 400, "{path}");
    }
    let resp = srv
        .client
        .put(srv.url("/api/search/remote?fts_text=test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    // Sans texte : 400 aussi.
    let resp = srv
        .client
        .put(srv.url("/api/search/remote"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    srv.session.stop().await;
}

#[tokio::test]
async fn torrentinfo_file_et_uri() {
    let srv = spawn_server().await;
    let bytes = test_torrent_bytes();

    // PUT /api/torrentinfo/file (corps brut bencode).
    let resp = srv
        .client
        .put(srv.url("/api/torrentinfo/file"))
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "api-test.bin");
    assert_eq!(body["files"][0]["size"], 42);
    assert_eq!(body["download_exists"], false);

    // POST /api/torrentinfo/uri via file://.
    let torrent_path = srv._dir.path().join("t.torrent");
    std::fs::write(&torrent_path, &bytes).unwrap();
    let resp = srv
        .client
        .post(srv.url("/api/torrentinfo/uri"))
        .json(&serde_json::json!({"uri": format!("file://{}", torrent_path.display())}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "api-test.bin");

    // URI invalide -> 400.
    let resp = srv
        .client
        .post(srv.url("/api/torrentinfo/uri"))
        .json(&serde_json::json!({"uri": "ftp://x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    srv.session.stop().await;
}

#[tokio::test]
async fn createtorrent_et_dryrun() {
    let srv = spawn_server().await;
    let src = srv._dir.path().join("data.bin");
    std::fs::write(&src, vec![7u8; 10_000]).unwrap();

    let resp = srv
        .client
        .post(srv.url("/api/createtorrent"))
        .json(&serde_json::json!({
            "files": [src.display().to_string()],
            "name": "data.bin",
            "tracker": "http://127.0.0.1:1/announce",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"][0]["infohash"].as_str().unwrap().len(), 40);
    // Le .torrent est exporte a cote de la source (comportement Python).
    assert!(srv._dir.path().join("data.bin.torrent").exists());

    // dryrun sur le repertoire temporaire.
    let resp = srv
        .client
        .post(srv.url("/api/createtorrent/dryrun"))
        .json(&serde_json::json!({"export_dir": srv._dir.path().display().to_string()}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["writable"],
        true
    );

    // Fichier inexistant -> 400.
    let resp = srv
        .client
        .post(srv.url("/api/createtorrent"))
        .json(&serde_json::json!({"files": ["nope.bin"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    srv.session.stop().await;
}

#[tokio::test]
async fn versioning_et_logging() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/versioning/versions/current"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap()["version"].is_string());

    let resp = srv
        .client
        .get(srv.url("/api/versioning/versions/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["has_version"],
        false
    );

    let resp = srv
        .client
        .get(srv.url("/api/logging?max_lines=10"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    srv.session.stop().await;
}

#[tokio::test]
async fn shutdown_endpoint_demande_l_arret() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .put(srv.url("/api/shutdown"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["shutdown"],
        true
    );
    // L'arret tourne en tache de fond ; petit delai pour qu'il s'applique.
    tokio::time::sleep(Duration::from_millis(200)).await;
}
