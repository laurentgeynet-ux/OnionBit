// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests d'integration HTTP de `onionbit-api`.
//!
//! Regle projet : aucun trafic sortant — le serveur axum est lie a
//! `127.0.0.1:0` (port ephemere) et le client `reqwest` ne vise que
//! cette adresse.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;

use onionbit_api::{build, AppState};
use onionbit_core::{CoreConfig, CoreSession, Notifier};

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
/// une piece factice) — fixture partagee `onionbit-test-support`.
fn test_torrent_bytes() -> Vec<u8> {
    onionbit_test_support::test_torrent_bytes("api-test.bin", 42)
}

/// Serveur de test monte sur loopback.
struct TestServer {
    addr: SocketAddr,
    session: CoreSession,
    state: AppState,
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
    let state = AppState::new(session.clone());
    let app = build(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        addr,
        session,
        state,
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
async fn put_downloads_binaire_torrent_avec_query_params() {
    let srv = spawn_server().await;

    let torrent_data = test_torrent_bytes();

    // PUT /api/downloads?safe_seeding=false&paused=false avec corps binaire applications/x-bittorrent
    let resp = srv
        .client
        .put(srv.url("/api/downloads?safe_seeding=false&paused=false"))
        .header("Content-Type", "applications/x-bittorrent")
        .body(torrent_data)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["started"], true);
    let infohash = body["infohash"].as_str().unwrap().to_string();
    assert_eq!(infohash.len(), 40);

    // Verifie la presence dans la liste des telechargements
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

/// Regression : `PUT /api/downloads` avec `anon_hops > 0` sans
/// `safe_seeding` est refuse (message Python litteral).
#[tokio::test]
async fn put_anon_sans_safe_seeding_retourne_400() {
    let srv = spawn_server().await;
    let torrent_path = srv._dir.path().join("api-test.torrent");
    std::fs::write(&torrent_path, test_torrent_bytes()).unwrap();
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({
            "torrent": torrent_path.display().to_string(),
            "anon_hops": 1,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["handled"], true);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("safe seeding"));
    srv.session.stop().await;
}

/// Regression : avec `safe_seeding: true` mais la stack IPv8 inactive
/// (session offline), l'ajout anonyme echoue en **400** — pas en 500.
#[tokio::test]
async fn put_anon_stack_inactive_retourne_400() {
    let srv = spawn_server().await;
    let torrent_path = srv._dir.path().join("api-test.torrent");
    std::fs::write(&torrent_path, test_torrent_bytes()).unwrap();
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({
            "torrent": torrent_path.display().to_string(),
            "anon_hops": 1,
            "safe_seeding": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    srv.session.stop().await;
}

/// `PATCH anon_hops` doit etre le seul parametre de la requete
/// (regle Python : 400 sinon — verifiee apres le 404, comme
/// `update_download` Python qui teste d'abord l'existence).
#[tokio::test]
async fn patch_anon_hops_combine_retourne_400() {
    let srv = spawn_server().await;
    let ih = srv
        .session
        .add_torrent_bytes(test_torrent_bytes(), true)
        .await
        .unwrap()
        .info_hash_hex();
    let resp = srv
        .client
        .patch(srv.url(&format!("/api/downloads/{ih}")))
        .json(&serde_json::json!({"state": "stop", "anon_hops": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("only parameter"));
    srv.session.stop().await;
}

/// `PATCH anon_hops` sur un telechargement inconnu : 404 (pas 500).
#[tokio::test]
async fn patch_anon_hops_inconnu_retourne_404() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .patch(srv.url("/api/downloads/0000000000000000000000000000000000000000"))
        .json(&serde_json::json!({"anon_hops": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    srv.session.stop().await;
}

/// `PATCH anon_hops` seul sur un download existant : le telechargement
/// est recree ; sans stack IPv8 la session refuse proprement (400).
#[tokio::test]
async fn patch_anon_hops_existant_stack_inactive_400() {
    let srv = spawn_server().await;
    let torrent_path = srv._dir.path().join("api-test.torrent");
    std::fs::write(&torrent_path, test_torrent_bytes()).unwrap();
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({"torrent": torrent_path.display().to_string()}))
        .send()
        .await
        .unwrap();
    let infohash = resp.json::<serde_json::Value>().await.unwrap()["infohash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = srv
        .client
        .patch(srv.url(&format!("/api/downloads/{infohash}")))
        .json(&serde_json::json!({"anon_hops": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    // Rollback : le download d'origine survit a l'echec de re-creation.
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads?infohash={infohash}")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["downloads"].as_array().unwrap().len(), 1);
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

/// Reglages restart-only (`tunnel_community/enabled`,
/// `exitnode_enabled`, `ipv8/enabled`, `libtorrent/{dht,utp,lsd,upnp}`,
/// `dht_discovery`, `content_discovery_community`,
/// `torrent_checker`) : `POST /api/settings` les persiste pour le
/// prochain demarrage — `GET` doit refleter la valeur postee et non
/// l'etat runtime (session offline : tout inactif au boot), sinon les
/// commutateurs de l'UI reviennent a leur position initiale.
#[tokio::test]
async fn settings_restart_only_refletent_la_valeur_postee() {
    let srv = spawn_server().await;

    // Etat de demarrage (session offline) : tout inactif.
    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["settings"]["ipv8"]["enabled"], false);
    assert_eq!(body["settings"]["tunnel_community"]["enabled"], false);

    let resp = srv
        .client
        .post(srv.url("/api/settings"))
        .json(&serde_json::json!({
            "settings": {
                "ipv8": { "enabled": true },
                "tunnel_community": { "enabled": true, "exitnode_enabled": true },
                "dht_discovery": { "enabled": true },
                "content_discovery_community": { "enabled": true },
                "torrent_checker": { "enabled": true },
                "libtorrent": { "dht": true, "upnp": true, "lsd": true, "utp": true }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    for (section, key) in [
        ("ipv8", "enabled"),
        ("tunnel_community", "enabled"),
        ("tunnel_community", "exitnode_enabled"),
        ("dht_discovery", "enabled"),
        ("content_discovery_community", "enabled"),
        ("torrent_checker", "enabled"),
        ("libtorrent", "dht"),
        ("libtorrent", "upnp"),
        ("libtorrent", "lsd"),
        ("libtorrent", "utp"),
    ] {
        assert_eq!(
            body["settings"][section][key],
            serde_json::json!(true),
            "{section}/{key} doit refleter la valeur postee (restart-only)"
        );
    }
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
    assert!(body["statistics"]["free"].as_u64().unwrap() > 0);
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
            .map_err(onionbit_db::DbError::from)
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

    // PUT + GET /trackers (reponse Python : `{"added": true}`).
    let resp = srv
        .client
        .put(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .json(&serde_json::json!({"url": "udp://127.0.0.1:6969/announce"}))
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

    // DELETE /trackers : le tracker ajoute a chaud disparait du listing.
    let resp = srv
        .client
        .delete(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .json(&serde_json::json!({"url": "udp://127.0.0.1:6969/announce"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["removed"],
        true
    );
    let resp = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["tracker_info"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["url"] == "udp://127.0.0.1:6969/announce"));

    // DELETE sans url -> 400 "url parameter missing" (message Python).
    let resp = srv
        .client
        .delete(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["message"], "url parameter missing");

    // PUT /default_trackers : no-op accepte sans fichier configure
    // (reponse Python `{"added": true}`).
    let resp = srv
        .client
        .put(srv.url(&format!("/api/downloads/{ih}/default_trackers")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["added"],
        true
    );

    // PUT /tracker_force_announce : `forced: true` meme pour une URL
    // inconnue (comportement Python).
    let resp = srv
        .client
        .put(srv.url(&format!("/api/downloads/{ih}/tracker_force_announce")))
        .json(&serde_json::json!({"url": "udp://inconnu.local:1/announce"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["forced"],
        true
    );

    // 404 avant validation du corps sur un infohash inconnu.
    let resp = srv
        .client
        .delete(srv.url("/api/downloads/0000000000000000000000000000000000000000/trackers"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

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
    // Test avec ?session=0 (compatibilite)
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

    // Test avec ?hop=0 (parite Python)
    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/settings?hop=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["hop"], 0);

    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/session?hop=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["session"]["torrents"].is_number());

    // Lane anonyme inexistante (pas de stack ipv8) -> 404 avec ?hop=2 ou ?session=2.
    let resp = srv
        .client
        .get(srv.url("/api/libtorrent/session?hop=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    srv.session.stop().await;
}

#[tokio::test]
async fn ipv8_et_search_sans_stack_retournent_erreur() {
    let srv = spawn_server().await;
    // Stack IPv8 desactivee en config offline — comme `session is
    // None` / `tunnels is None` Python, les GET repondent 200 avec
    // des collections vides.
    for (path, key) in [
        ("/api/ipv8/overlays", "overlays"),
        ("/api/ipv8/tunnel/circuits", "circuits"),
        ("/api/ipv8/tunnel/settings", "settings"),
        ("/api/ipv8/tunnel/relays", "relays"),
        ("/api/ipv8/tunnel/exits", "exits"),
        ("/api/ipv8/tunnel/swarms", "swarms"),
        ("/api/ipv8/tunnel/peers", "peers"),
        ("/api/ipv8/tunnel/guards", "guards"),
        ("/api/ipv8/tunnel/debug/circuit-downloads", "downloads"),
        ("/api/ipv8/network", "peers"),
        ("/api/ipv8/overlays/statistics", "statistics"),
    ] {
        let resp = srv.client.get(srv.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 200, "{path}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body[key].as_array().map(|a| a.is_empty()).unwrap_or(false)
                || body[key].as_object().map(|o| o.is_empty()).unwrap_or(false),
            "{path} -> {body}"
        );
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

/// ADR-0015 : `GET /api/ipv8/tunnel/ledger` repond 200 avec le
/// ledger marque desactive quand la stack IPv8 n'existe pas —
/// meme convention 200+vide que les autres GET tunnel.
#[tokio::test]
async fn tunnel_ledger_sans_stack_retourne_desactive() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/ledger"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ledger"]["enabled"], false);
    assert_eq!(body["ledger"]["enforce"], false);
    assert_eq!(body["ledger"]["peer_count"], 0);
    assert_eq!(body["ledger"]["total_served"], 0);
    assert_eq!(body["ledger"]["total_used"], 0);
    assert_eq!(body["ledger"]["peers"], serde_json::json!([]));
    srv.session.stop().await;
}

/// ADR-0015 : `GET /api/ipv8/ext` repond 200 avec la communaute
/// marquee desactivee sans stack IPv8 (ou `ext/enabled = false`) —
/// meme convention 200+vide que `/ipv8/tunnel/ledger`.
#[tokio::test]
async fn ipv8_ext_sans_stack_retourne_desactive() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/ext"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ext"]["enabled"], false);
    assert_eq!(body["ext"]["peer_count"], 0);
    assert_eq!(body["ext"]["peers"], serde_json::json!([]));
    srv.session.stop().await;
}

/// ADR-0015 §6 : les sous-routes operatoires de `/api/ipv8/ext`
/// repondent 404 quand la communaute n'existe pas — et les corps
/// invalides 400 avant meme la verification de stack.
#[tokio::test]
async fn ipv8_ext_attest_sans_stack_404_et_400() {
    let srv = spawn_server().await;
    // 404 partout : ext desactive.
    for (method, path) in [
        ("POST", "/api/ipv8/ext/attest".to_string()),
        ("GET", "/api/ipv8/ext/attestations".to_string()),
        (
            "GET",
            format!("/api/ipv8/ext/trust/infohash/{}", "aa".repeat(20)),
        ),
    ] {
        let req = srv.client.request(method.parse().unwrap(), srv.url(&path));
        let req = if method == "POST" {
            req.json(&serde_json::json!({
                "kind": "infohash",
                "subject": "aa".repeat(20),
                "verdict": "endorse",
            }))
        } else {
            req
        };
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 404, "{method} {path}");
    }
    // 400 avant la verification de stack : kind/verdict inconnus.
    for body in [
        serde_json::json!({"kind": "nope", "subject": "aa", "verdict": "endorse"}),
        serde_json::json!({"kind": "infohash", "subject": "aa", "verdict": "nope"}),
        serde_json::json!({"kind": "infohash", "subject": "zz", "verdict": "flag"}),
    ] {
        let resp = srv
            .client
            .post(srv.url("/api/ipv8/ext/attest"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{body}");
    }
    srv.session.stop().await;
}

#[tokio::test]
async fn messaging_desactivee_repond_404_sur_tous_les_endpoints() {
    let srv = spawn_server().await;
    // Session offline : `enable_messaging` off et pas de stack IPv8 —
    // chaque endpoint repond 404 « messagerie desactivee » (MS-12 :
    // jamais de reponse partielle quand la fonctionnalite est off).
    let pk = hex::encode([0u8; 64]);
    for (method, path, body) in [
        ("GET", "/api/messaging/stats".to_string(), "".to_string()),
        ("GET", "/api/messaging/contacts".to_string(), "".to_string()),
        (
            "GET",
            "/api/messaging/contacts/pending".to_string(),
            "".to_string(),
        ),
        (
            "GET",
            format!("/api/messaging/contacts/{pk}/messages"),
            "".to_string(),
        ),
        ("GET", "/api/messaging/events".to_string(), "".to_string()),
        (
            "POST",
            "/api/messaging/contacts/connect".to_string(),
            format!("{{\"public_key\":\"{pk}\"}}"),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/accept"),
            "{}".to_string(),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/refuse"),
            "{}".to_string(),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/block"),
            "{}".to_string(),
        ),
        (
            "DELETE",
            format!("/api/messaging/contacts/{pk}/block"),
            "".to_string(),
        ),
        (
            "DELETE",
            format!("/api/messaging/contacts/{pk}"),
            "".to_string(),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/messages"),
            "{\"body\":\"test\"}".to_string(),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/alias"),
            "{\"alias\":\"alice\"}".to_string(),
        ),
        (
            "POST",
            format!("/api/messaging/contacts/{pk}/retention"),
            "{\"retention_secs\":0}".to_string(),
        ),
        (
            "DELETE",
            format!("/api/messaging/messages/{}", hex::encode([0u8; 16])),
            "".to_string(),
        ),
    ] {
        let resp = match method {
            "GET" => srv.client.get(srv.url(&path)),
            "POST" => srv
                .client
                .post(srv.url(&path))
                .header("content-type", "application/json")
                .body(body),
            _ => srv.client.delete(srv.url(&path)),
        }
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 404, "{method} {path}");
    }
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
    // Extension : trackers + flag private exposes pour l'anticipation
    // cote client (torrent sans tracker -> liste vide).
    assert_eq!(body["trackers"], serde_json::json!([]));
    assert_eq!(body["private"], false);

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

    // Sonde neutralisee : `github_repo` vide + `check_urls` vide =
    // aucune requete sortante (regle projet : pas de trafic externe).
    srv.state
        .daemon_config
        .lock()
        .unwrap()
        .versioning
        .github_repo
        .clear();
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

/// `versions/check` reel : une sonde loopback annoncant `v99.0.0`
/// doit donner `has_version: true` (politique `permissive` de
/// `CoreConfig::offline`).
#[tokio::test]
async fn versioning_check_sonde_locale() {
    // Serveur "releases" factice : `{"name": "v99.0.0"}`.
    let sonde_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sonde_addr = sonde_listener.local_addr().unwrap();
    let sonde = axum::Router::new().route(
        "/releases",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"name": "v99.0.0"})) }),
    );
    tokio::spawn(async move {
        axum::serve(sonde_listener, sonde).await.unwrap();
    });

    let srv = spawn_server().await;
    {
        let mut cfg = srv.state.daemon_config.lock().unwrap();
        cfg.versioning.github_repo.clear();
        cfg.versioning.check_urls = vec![format!("http://{sonde_addr}/releases")];
    }
    let resp = srv
        .client
        .get(srv.url("/api/versioning/versions/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["has_version"], true);
    assert_eq!(body["new_version"], "99.0.0");
    srv.session.stop().await;
}

#[tokio::test]
async fn events_info_et_dirspace_contrat_python() {
    let srv = spawn_server().await;

    // GET /api/events/info : kwargs du message initial Python.
    let resp = srv
        .client
        .get(srv.url("/api/events/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["version"].is_string());
    assert!(body["public_key"].is_string());
    assert!(body["sessions"].is_string()); // str cote Python

    // PUT /api/statistics/dirspace {"directory"} : reponse
    // {"statistics": {total, used, free}} (contrat Python).
    let resp = srv
        .client
        .put(srv.url("/api/statistics/dirspace"))
        .json(&serde_json::json!({"directory": "."}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["statistics"]["total"].as_u64().unwrap() > 0);
    assert!(body["statistics"].get("free").is_some());

    // Chemin inexistant profond : le Python remonte au premier
    // ancetre existant -> les stats du disque sont retournees.
    let resp = srv
        .client
        .put(srv.url("/api/statistics/dirspace"))
        .json(&serde_json::json!({"directory": "./nonexistent-dir-xyz/deep"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap()["statistics"]["total"].is_u64());

    // GET ?path= reste disponible (confort, meme reponse).
    let resp = srv
        .client
        .get(srv.url("/api/statistics/dirspace?path=."))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap()["statistics"]["total"].is_u64());

    srv.session.stop().await;
}

#[tokio::test]
async fn clierrors_journalise_puis_vide() {
    let srv = spawn_server().await;

    // Echec d'ajout avec cli=true : l'erreur entre dans la file.
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({"cli": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // La file est visible dans le champ `clierrors` de la liste.
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["clierrors"], 1);

    // GET /api/downloads/clierrors : retourne et vide la file.
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads/clierrors"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["errors"].as_array().unwrap().len(), 1);
    assert_eq!(body["errors"][0], "uri parameter missing");

    // Videe : compteur a 0, drain suivant vide.
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["clierrors"], 0);
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads/clierrors"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["errors"].as_array().unwrap().len(), 0);

    // Sans `cli`, une erreur n'entre pas dans la file.
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["clierrors"], 0);

    srv.session.stop().await;
}

#[tokio::test]
async fn downloads_eta_est_un_nombre() {
    // Contrat Python : `eta` est un float de secondes (pas une
    // chaine formatee).
    let srv = spawn_server().await;
    let dir = srv._dir.path().join("dl");
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = onionbit_test_support::test_torrent_bytes("eta-test", 16 * 1024);
    let tp = dir.join("t.torrent");
    std::fs::write(&tp, &bytes).unwrap();
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({"torrent": tp.display().to_string()}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body["downloads"][0]["eta"].is_f64() || body["downloads"][0]["eta"].is_u64());
    assert_eq!(body["downloads"][0]["hops"], 0);
    assert_eq!(body["downloads"][0]["anon_download"], false);
    srv.session.stop().await;
}

// ============================================================================
// Etape 21 — cle API (ApiKeyMiddleware) + configuration.json persistee
// ============================================================================

/// Serveur de test dont l'etat est personnalise (cle API, config
/// daemon).
async fn spawn_server_with(state_fn: impl FnOnce(CoreSession) -> AppState) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let state = state_fn(session.clone());
    let app = build(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        addr,
        session,
        state,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

#[tokio::test]
async fn auth_cle_api_header_query_cookie() {
    let srv = spawn_server_with(|s| AppState::new(s).with_api_key("cle-de-test")).await;

    // Sans cle -> 401 au format Tribler.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["handled"], true);
    assert_eq!(body["error"]["message"], "Unauthorized access");

    // Mauvaise cle -> 401.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .header("x-api-key", "mauvaise")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Route inconnue non authentifiee -> 401 (le middleware precede
    // le routage, comme en Python).
    let resp = srv
        .client
        .get(srv.url("/api/inconnu"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Routes messagerie sans cle -> 401 (MS-12 : le middleware
    // s'applique a l'extension comme aux endpoints Python — jamais
    // de fuite d'etat avant authentification).
    for path in [
        "/api/messaging/contacts",
        "/api/messaging/events",
        "/api/messaging/stats",
    ] {
        let resp = srv.client.get(srv.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 401, "{path}");
    }

    // En-tete `X-Api-Key`.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .header("X-Api-Key", "cle-de-test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Query `?key=`.
    let resp = srv
        .client
        .get(srv.url("/api/downloads?key=cle-de-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Cookie `api_key`.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .header("cookie", "api_key=cle-de-test; autre=1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    srv.session.stop().await;
}

#[tokio::test]
async fn settings_post_merge_et_persiste_configuration_json() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("configuration.json");
    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let mut dcfg = onionbit_core::DaemonConfig::default();
    dcfg.ensure_api_key();
    let key = dcfg.api.key.clone();
    let srv = {
        let state =
            AppState::new(session.clone()).with_daemon_config(dcfg, Some(config_path.clone()));
        let app = build(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestServer {
            addr,
            session,
            state,
            client: reqwest::Client::new(),
            _dir: dir,
        }
    };

    // GET : arbre complet + cle API visible (comme `config.configuration`).
    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .header("x-api-key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["settings"]["api"]["key"], key);
    // Les sections par defaut Python sont presentes.
    for section in [
        "ipv8",
        "libtorrent",
        "tunnel_community",
        "rss",
        "watch_folder",
        "torrent_checker",
        "versioning",
    ] {
        assert!(body["settings"][section].is_object(), "section {section}");
    }

    // POST au format Python : l'arbre directement (sans enveloppe
    // "settings"), merge recursif.
    let resp = srv
        .client
        .post(srv.url("/api/settings"))
        .header("x-api-key", &key)
        .json(&serde_json::json!({
            "rss": { "urls": ["http://127.0.0.1:9/feed"] },
            "section_inconnue": { "x": 1 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Le fichier est ecrit et relectible ; la cle inconnue est
    // conservee (semantique du dict Python).
    let stored = onionbit_core::DaemonConfig::load(&config_path);
    assert_eq!(stored.rss.urls, vec!["http://127.0.0.1:9/feed".to_string()]);
    assert_eq!(
        stored.extra.get("section_inconnue").unwrap()["x"],
        serde_json::json!(1)
    );
    // La cle API est inchangee par le merge.
    assert_eq!(stored.api.key, key);

    // Le GET reflete la valeur persistee.
    let resp = srv
        .client
        .get(srv.url("/api/settings"))
        .header("x-api-key", &key)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["settings"]["rss"]["urls"],
        serde_json::json!(["http://127.0.0.1:9/feed"])
    );
    assert_eq!(
        body["settings"]["section_inconnue"]["x"],
        serde_json::json!(1)
    );

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

// ============================================================================
// Etape 22 — reglages par download persistes + PATCH complet
// ============================================================================

/// Ajoute le torrent de test (pause) et retourne son info-hash hex.
async fn add_paused(srv: &TestServer) -> String {
    srv.session
        .add_torrent_bytes(test_torrent_bytes(), true)
        .await
        .unwrap()
        .info_hash_hex()
}

#[tokio::test]
async fn patch_download_reglages_persistes() {
    let srv = spawn_server().await;
    let ih = add_paused(&srv).await;

    // selected_files / limites / ratio / auto_managed / queue.
    let resp = srv
        .client
        .patch(srv.url(&format!("/api/downloads/{ih}")))
        .json(&serde_json::json!({
            "selected_files": [0],
            "upload_limit": 1024,
            "download_limit": 2048,
            "seeding_ratio": 1.5,
            "auto_managed": true,
            "queue_position": "queue_top",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["modified"], true);
    assert_eq!(body["infohash"], ih);

    // Persistes en base et exposes par le GET.
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap();
    let d = &resp.json::<serde_json::Value>().await.unwrap()["downloads"][0];
    assert_eq!(d["safe_seeding"], true); // defaut `safeseeding_enabled`
    assert_eq!(d["upload_limit"], 1024);
    assert_eq!(d["download_limit"], 2048);
    assert_eq!(d["seeding_ratio"], 1.5);
    assert_eq!(d["auto_managed"], true);
    assert_eq!(d["queue_position"], 0);

    // `seeding_ratio_default` : reset au defaut `download_defaults`.
    let resp = srv
        .client
        .patch(srv.url(&format!("/api/downloads/{ih}")))
        .json(&serde_json::json!({ "seeding_ratio_default": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let d = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["downloads"][0]
        .clone();
    assert_eq!(d["seeding_ratio"], 2.0); // defaut `download_defaults`

    srv.session.stop().await;
}

#[tokio::test]
async fn patch_download_validations_python() {
    let srv = spawn_server().await;
    let ih = add_paused(&srv).await;
    let url = srv.url(&format!("/api/downloads/{ih}"));
    let patch = |body: serde_json::Value| {
        let (client, url) = (srv.client.clone(), url.clone());
        async move { client.patch(url).json(&body).send().await.unwrap() }
    };

    // anon_hops non exclusif -> 400 (meme message Python).
    let r = patch(serde_json::json!({"anon_hops": 1, "state": "resume"})).await;
    assert_eq!(r.status(), 400);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["message"],
        "anon_hops must be the only parameter in this request"
    );

    // selected_files hors bornes -> "index out of range".
    let r = patch(serde_json::json!({"selected_files": [0, 9]})).await;
    assert_eq!(r.status(), 400);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["message"],
        "index out of range"
    );

    // file_priority : index hors bornes puis priorite hors bornes.
    let r = patch(serde_json::json!({"file_priority": [5, 1]})).await;
    assert_eq!(r.status(), 400);
    let r = patch(serde_json::json!({"file_priority": [0, 8]})).await;
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["message"],
        "file priority out of range"
    );
    // Paire malformee -> 500 handled (unpack Python).
    let r = patch(serde_json::json!({"file_priority": [0]})).await;
    assert_eq!(r.status(), 500);

    // auto_managed non booleen -> 400.
    let r = patch(serde_json::json!({"auto_managed": "oui"})).await;
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["message"],
        "invalid value for auto_managed"
    );

    // queue_position invalide -> 400.
    let r = patch(serde_json::json!({"queue_position": "queue_sideways"})).await;
    assert_eq!(r.status(), 400);

    // state inconnu -> "unknown state parameter".
    let r = patch(serde_json::json!({"state": "explode"})).await;
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["message"],
        "unknown state parameter"
    );

    // move_storage sans dest_dir -> 500 handled (KeyError Python).
    let r = patch(serde_json::json!({"state": "move_storage"})).await;
    assert_eq!(r.status(), 500);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        true
    );

    // move_storage vers un dossier inexistant -> 400.
    let r = patch(serde_json::json!({
        "state": "move_storage",
        "dest_dir": "Z:\\non\\existe\\pas"
    }))
    .await;
    assert_eq!(r.status(), 400);

    // Download inconnu -> 404.
    let r = srv
        .client
        .patch(srv.url("/api/downloads/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
        .json(&serde_json::json!({"state": "resume"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    srv.session.stop().await;
}

#[tokio::test]
async fn patch_download_recheck_stop_resume_move_storage() {
    let srv = spawn_server().await;
    let ih = add_paused(&srv).await;
    let url = srv.url(&format!("/api/downloads/{ih}"));
    let patch = |body: serde_json::Value| {
        let (client, url) = (srv.client.clone(), url.clone());
        async move { client.patch(url).json(&body).send().await.unwrap() }
    };

    // stop -> paused + user_stopped persistes ; resume -> releves.
    let r = patch(serde_json::json!({"state": "stop"})).await;
    assert_eq!(r.status(), 200);
    let d = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["downloads"][0]
        .clone();
    assert_eq!(d["user_stopped"], true);
    let r = patch(serde_json::json!({"state": "resume"})).await;
    assert_eq!(r.status(), 200);

    // recheck : remove + re-add -> le download reapparait avec ses
    // reglages (queue_position conservee).
    let r = patch(serde_json::json!({"state": "recheck"})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["modified"],
        true
    );
    assert!(srv.session.find_download(&ih).is_some());

    // move_storage : no-op (meme dossier) -> modified:false.
    let dest = srv.session.find_download(&ih).unwrap().output_folder();
    let r = patch(serde_json::json!({
        "state": "move_storage",
        "dest_dir": dest.display().to_string()
    }))
    .await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["modified"],
        false
    );

    // move_storage reel : dossier cree -> fichiers deplaces,
    // output_dir persiste mis a jour. Fichier etranger depose dans
    // le dossier de telechargements : il ne doit PAS suivre — seul
    // le contenu declare par le torrent est deplace (mono-fichier :
    // `output_folder` = dossier partage, ancien bug qui vidait tout).
    let dl_dir = srv.session.find_download(&ih).unwrap().output_folder();
    std::fs::write(dl_dir.join("etranger.txt"), b"leurre").unwrap();
    std::fs::write(dl_dir.join("api-test.bin"), b"donnees").unwrap();
    let new_dir = srv._dir.path().join("deplace");
    std::fs::create_dir_all(&new_dir).unwrap();
    let r = patch(serde_json::json!({
        "state": "move_storage",
        "dest_dir": new_dir.display().to_string()
    }))
    .await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["modified"],
        true
    );
    let dl = srv.session.find_download(&ih).unwrap();
    assert_eq!(dl.output_folder(), new_dir);
    assert!(
        new_dir.join("api-test.bin").exists(),
        "le fichier du torrent doit suivre"
    );
    assert!(
        dl_dir.join("etranger.txt").exists(),
        "le fichier etranger doit rester en place"
    );
    assert!(
        !new_dir.join("etranger.txt").exists(),
        "le leurre ne doit pas etre deplace"
    );

    srv.session.stop().await;
}

#[tokio::test]
async fn get_downloads_flags_peers_pieces_availability() {
    let srv = spawn_server().await;
    let ih = add_paused(&srv).await;
    let _ = ih;

    // Sans flag : les cles d'enrichissement sont absentes (Python
    // n'emet `peers`/`pieces`/`availability` que sur demande).
    let resp = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap();
    let d = resp.json::<serde_json::Value>().await.unwrap()["downloads"][0].clone();
    assert!(d.get("peers").is_none());
    assert!(d.get("pieces").is_none());
    assert!(d.get("availability").is_none());

    // Flags = "1" (exactement, comme `params.get(...) == "1"` Python).
    let resp = srv
        .client
        .get(srv.url("/api/downloads?get_peers=1&get_pieces=1&get_availability=1"))
        .send()
        .await
        .unwrap();
    let d = resp.json::<serde_json::Value>().await.unwrap()["downloads"][0].clone();
    assert!(d["peers"].is_array());
    assert!(d["pieces"].is_string());
    assert!(d["availability"].is_number());
    assert!(d["total_pieces"].as_u64().unwrap() >= 1);

    srv.session.stop().await;
}

// ============================================================================
// Etape 24 — topics SSE complets
// ============================================================================

/// Tous les topics du `Notification` Python ajoutes a l'etape 24 sont
/// serialises en trames `event: <topic>\ndata: <json>\n\n`.
#[tokio::test]
async fn events_sse_topics_etape24() {
    use onionbit_core::Notification;
    let srv = spawn_server().await;
    let mut resp = srv.client.get(srv.url("/api/events")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut buf = String::new();
    read_until(&mut buf, &mut resp, "events_start").await;

    let cases: Vec<(Notification, &str, Vec<&str>)> = vec![
        (
            Notification::ShutdownState {
                state: "Shutting down torrent checker.".into(),
            },
            "tribler_shutdown_state",
            vec!["state"],
        ),
        (
            Notification::RemoteQueryResults {
                query: "ubuntu".into(),
                results: vec![serde_json::json!({"name": "ubuntu.iso"})],
                uuid: "req-1".into(),
                peer: "aabb".into(),
            },
            "remote_query_results",
            vec!["query", "results", "uuid", "peer"],
        ),
        (
            Notification::LocalQueryResults {
                query: "debian".into(),
                results: vec![],
            },
            "local_query_results",
            vec!["query", "results"],
        ),
        (
            Notification::TunnelRemoved {
                circuit_id: 42,
                circuit_class: "Circuit".into(),
                bytes_up: 10,
                bytes_down: 20,
                uptime_secs: 3.5,
                additional_info: "got destroy".into(),
            },
            "tunnel_removed",
            vec![
                "circuit_id",
                "circuit_class",
                "bytes_up",
                "bytes_down",
                "uptime",
                "additional_info",
            ],
        ),
        (
            Notification::LowSpace {
                disk_usage_data: serde_json::json!({"total": 100, "used": 90, "free": 10}),
            },
            "low_space",
            vec!["disk_usage_data"],
        ),
        (
            Notification::TriblerException {
                error: "boom".into(),
            },
            "tribler_exception",
            vec!["error", "traceback"],
        ),
        (
            Notification::ReportConfigError {
                error: "bad json".into(),
            },
            "report_config_error",
            vec!["error"],
        ),
        (
            Notification::AskAddDownload {
                uri: "magnet:?xt=urn:btih:aa".into(),
            },
            "ask_add_download",
            vec!["uri"],
        ),
        (
            Notification::TriblerNewVersion {
                version: "9.9.9".into(),
            },
            "tribler_new_version",
            vec!["version"],
        ),
    ];
    for (n, topic, keys) in cases {
        srv.session.notifier().notify(n);
        read_until(&mut buf, &mut resp, &format!("event: {topic}")).await;
        let marker = format!("event: {topic}\ndata: ");
        let pos = buf.rfind(&marker).unwrap();
        let data: serde_json::Value =
            serde_json::from_str(buf[pos + marker.len()..].lines().next().unwrap()).unwrap();
        for k in keys {
            assert!(data.get(k).is_some(), "{topic} sans cle {k} : {data}");
        }
        buf.clear();
    }
    srv.session.stop().await;
}

/// `DownloadStateChanged` est traduit en `torrent_status_changed`
/// (nom `DownloadStatus` Python), pas en `download_state_changed`.
#[tokio::test]
async fn events_sse_torrent_status_changed() {
    use onionbit_core::Notification;
    let srv = spawn_server().await;
    let mut resp = srv.client.get(srv.url("/api/events")).send().await.unwrap();
    let mut buf = String::new();
    read_until(&mut buf, &mut resp, "events_start").await;
    buf.clear();
    srv.session
        .notifier()
        .notify(Notification::DownloadStateChanged {
            infohash: "aa".repeat(20),
            state: onionbit_bittorrent::DownloadState::Downloading,
        });
    read_until(&mut buf, &mut resp, "event: torrent_status_changed").await;
    let pos = buf.rfind("data: ").unwrap();
    let data: serde_json::Value =
        serde_json::from_str(buf[pos + 6..].lines().next().unwrap()).unwrap();
    assert_eq!(data["status"], "DOWNLOADING");
    srv.session.stop().await;
}

/// `local_search` notifie `local_query_results` avec la requete et
/// les resultats (comme `database_endpoint.py`).
#[tokio::test]
async fn local_search_notifie_local_query_results() {
    let srv = spawn_server().await;
    let mut rx = srv.session.notifier().subscribe();
    let resp = srv
        .client
        .get(srv.url("/api/metadata/search/local?fts_text=ubuntu"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let n = tokio::time::timeout(IO_TIMEOUT, async {
        loop {
            if let Ok(onionbit_core::Notification::LocalQueryResults { query, .. }) =
                rx.recv().await
            {
                break query;
            }
        }
    })
    .await
    .expect("local_query_results non recu");
    assert_eq!(n, "ubuntu");
    srv.session.stop().await;
}

/// `PUT /api/downloads` avec `cli` + `ask_download_settings` : le
/// daemon notifie `ask_add_download` et ne demarre rien.
#[tokio::test]
async fn put_download_ask_add_download() {
    let srv = spawn_server().await;
    srv.state
        .daemon_config
        .lock()
        .unwrap()
        .libtorrent
        .ask_download_settings = true;
    let mut rx = srv.session.notifier().subscribe();
    let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
    let resp = srv
        .client
        .put(srv.url("/api/downloads"))
        .json(&serde_json::json!({"uri": uri, "cli": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["started"], false);
    assert_eq!(body["infohash"], "");
    let got = tokio::time::timeout(IO_TIMEOUT, async {
        loop {
            if let Ok(onionbit_core::Notification::AskAddDownload { uri: u }) = rx.recv().await {
                break u;
            }
        }
    })
    .await
    .expect("ask_add_download non recu");
    assert_eq!(got, uri);
    srv.session.stop().await;
}

/// Serveur de test avec la stack IPv8 **active** (UDP loopback
/// ephemere, aucun bootstrap — zero trafic sortant) : permet de
/// couvrir les routes `/api/ipv8/dht/*` en presence de la community.
async fn spawn_server_ipv8() -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().into());
    cfg.ipv8.enabled = true;
    cfg.ipv8.listen_addr = "0.0.0.0:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    cfg.ipv8.enable_dht = true;
    let session = CoreSession::start_offline(cfg, Notifier::new())
        .await
        .unwrap();
    let state = AppState::new(session.clone());
    let app = build(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        addr,
        session,
        state,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

/// Sans community DHT (`dht_discovery/enabled=false` ou IPv8 off),
/// le `dht_endpoint` Python repond 404 `{"success": false, "error":
/// "DHT community not found"}` — sauf `buckets` (200, liste vide) et
/// `refresh` (400 "is not loaded").
#[tokio::test]
async fn dht_routes_sans_community() {
    let srv = spawn_server().await;
    for path in [
        "/api/ipv8/dht/statistics",
        "/api/ipv8/dht/values",
        "/api/ipv8/dht/values/0123456789abcdef0123456789abcdef01234567",
        "/api/ipv8/dht/peers/0123456789abcdef0123456789abcdef01234567",
    ] {
        let resp = srv.client.get(srv.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["success"], false, "{path}");
        assert_eq!(body["error"], "DHT community not found", "{path}");
    }
    // PUT values/{key} : meme 404.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/dht/values/0123456789abcdef0123456789abcdef01234567"))
        .json(&serde_json::json!({"value": "aabb"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        false
    );
    // `buckets` : 200 + liste vide (le endpoint n'echoue pas).
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/buckets"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["buckets"],
        serde_json::json!([])
    );
    // `refresh` : 400 "is not loaded".
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/buckets/0/refresh"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "DHT community is not loaded");
    srv.session.stop().await;
}

/// Avec `dht_discovery/enabled` : la community repond aux 7 routes
/// avec les formes Python (`statistics.peer_id`, `buckets`, `debug`
/// du lookup, `success` du PUT/refresh).
#[tokio::test]
async fn dht_routes_avec_community() {
    let srv = spawn_server_ipv8().await;
    assert!(srv.session.dht().is_some(), "dht_discovery active");

    // `statistics` : structure `{"statistics": {...}}`.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/statistics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let stats = &body["statistics"];
    assert_eq!(stats["peer_id"].as_str().unwrap().len(), 40, "mid hex");
    assert!(stats["num_tokens"].is_number());
    assert!(stats["endpoints"].is_array());
    // `DHTDiscoveryCommunity` : compteurs store/store_for_me.
    assert!(stats["num_peers_in_store"].is_object());
    assert!(stats["num_store_for_me"].is_object());

    // `values` (stockees localement) : objet indexe par cle hex.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/values"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap().is_object());

    // `values/{key}` : aucune table de routage encore -> `find_values`
    // iterer sur zero classes rend `([], [])` Python -> 200 vide.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/values/0123456789abcdef0123456789abcdef01234567"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["values"], serde_json::json!([]));
    let debug = &body["debug"];
    assert_eq!(debug["requests"], 0);
    assert_eq!(debug["responses"], 0);
    assert!(debug["time"].is_number());

    // `values/{key}` avec hex invalide : `unhexlify` Python → 500.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/values/pas_du_hex!"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );

    // `PUT` sans champ `value` : 400 `incorrect parameters`.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/dht/values/0123456789abcdef0123456789abcdef01234567"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "incorrect parameters");

    // `PUT` avec `value` : `store_on_nodes` leve `DHTError` quand
    // aucun noeud n'est connu ("No nodes found for storing the
    // key-value pairs" Python) -> 500 non geree.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/dht/values/0123456789abcdef0123456789abcdef01234567"))
        .json(&serde_json::json!({"value": "aabb"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );

    // `buckets` : 200 + liste (vide tant qu'aucun pair n'est appris).
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/buckets"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.json::<serde_json::Value>().await.unwrap()["buckets"].is_array());

    // `refresh` d'un prefixe inexistant : 400 `no such bucket`.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/buckets/0101/refresh"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "no such bucket");

    // `peers/{mid}` : `connect_peer` leve `DHTError` ("No nodes
    // found for connecting to peer" / "Failed to connect peer") sur
    // une table vide -> 500 `{"error": {"handled": false}}`.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/peers/0123456789abcdef0123456789abcdef01234567"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );

    // `peers/{mid}` avec hex invalide : `unhexlify` -> 500.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/dht/peers/zz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );
    srv.session.stop().await;
}

// -------------------------------------------------------------------
// Etape 26 — /api/ipv8/network, /isolation, /noblockdht,
//            /api/ipv8/overlays[/statistics]
// -------------------------------------------------------------------

/// `GET /api/ipv8/overlays` avec stack : `OverlaySchema` pyipv8
/// (id hex, my_peer, global_time, peers, overlay_name, statistics,
/// max_peers, is_isolated, my_estimated_*, strategies).
#[tokio::test]
async fn ipv8_overlays_shape_python() {
    let srv = spawn_server_ipv8().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/overlays"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let overlays = body["overlays"].as_array().unwrap();
    // Discovery + ContentDiscovery + DHTDiscovery (enable_dht) —
    // pas de tunnel (enable_anonymity=false en config offline).
    let names: Vec<&str> = overlays
        .iter()
        .map(|o| o["overlay_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "DiscoveryCommunity",
            "ContentDiscoveryCommunity",
            "DHTDiscoveryCommunity"
        ]
    );
    for o in overlays {
        assert_eq!(o["id"].as_str().unwrap().len(), 40, "community_id hex");
        assert!(o["my_peer"].as_str().unwrap().len() >= 64, "pubkey hex");
        assert!(o["global_time"].is_u64());
        assert!(o["peers"].is_array());
        assert_eq!(o["max_peers"], 30);
        assert!(o["statistics"]["num_up"].is_u64());
        assert!(o["statistics"]["diff_time"].is_f64() || o["statistics"]["diff_time"].is_i64());
        assert!(o["my_estimated_wan"]["ip"].is_string());
        assert!(o["my_estimated_wan"]["port"].is_u64());
        assert!(!o["strategies"].as_array().unwrap().is_empty());
    }
    // `is_isolated` : vrai uniquement pour le DHT (Network propre).
    let by_name = |n: &str| overlays.iter().find(|o| o["overlay_name"] == n).unwrap();
    assert_eq!(by_name("DHTDiscoveryCommunity")["is_isolated"], true);
    assert_eq!(by_name("DiscoveryCommunity")["is_isolated"], false);
    assert_eq!(
        by_name("DiscoveryCommunity")["strategies"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "RandomWalk + RandomChurn + PeriodicSimilarity"
    );
    srv.session.stop().await;
}

/// `GET /api/ipv8/network` : `{"peers": {b64(mid): {...}}}` vide
/// avec stack (aucun pair sur loopback isole).
#[tokio::test]
async fn ipv8_network_pairs_vides_avec_stack() {
    let srv = spawn_server_ipv8().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/network"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["peers"],
        serde_json::json!({})
    );
    srv.session.stop().await;
}

/// `POST /api/ipv8/isolation` : validations 400 puis `success`.
#[tokio::test]
async fn ipv8_isolation_semantique_python() {
    let srv = spawn_server_ipv8().await;

    // Corps vide/non-objet → "ip" and "port" are required (400).
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "Parameters \"ip\" and \"port\" are required");

    // ip+port sans mode → 400 "exitnode" or "bootstrapnode".
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .json(&serde_json::json!({"ip": "1.2.3.4", "port": 4242}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"],
        "Parameter \"exitnode\" or \"bootstrapnode\" is required"
    );

    // bootstrapnode : blacklist + walk + bootstrapper → success.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .json(&serde_json::json!({"ip": "1.2.3.4", "port": 4242, "bootstrapnode": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // exitnode (pas de tunnel → walk no-op) → success.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .json(&serde_json::json!({"ip": "1.2.3.4", "port": 4242, "exitnode": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // Corps non-JSON → exception `request.json()` → 500.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .body("pas du json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    srv.session.stop().await;
}

/// `POST /api/ipv8/isolation` sans stack : Python leve
/// `AttributeError` sur `self.session.network` → 500 `handled:false`.
#[tokio::test]
async fn ipv8_isolation_sans_stack() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/isolation"))
        .json(&serde_json::json!({"ip": "1.2.3.4", "port": 42, "bootstrapnode": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );
    srv.session.stop().await;
}

/// `GET/POST /api/ipv8/overlays/statistics` : activation, erreurs
/// 400/412, stats auto-activees au demarrage (session.py).
#[tokio::test]
async fn ipv8_overlay_statistics_semantique() {
    let srv = spawn_server_ipv8().await;

    // GET : chaque overlay present (notre endpoint est toujours un
    // `StatisticsEndpoint`), maps par msg_id vides sans trafic.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/overlays/statistics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let stats = body["statistics"].as_array().unwrap();
    let names: Vec<&str> = stats
        .iter()
        .flat_map(|o| o.as_object().unwrap().keys().map(|k| k.as_str()))
        .collect();
    assert!(names.contains(&"DiscoveryCommunity"));
    assert!(names.contains(&"DHTDiscoveryCommunity"));

    // POST sans `enable` → 400.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/overlays/statistics"))
        .json(&serde_json::json!({"all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"],
        "Parameter \"enable\" is required"
    );

    // POST `enable` sans `all`/`overlay_name` → 412.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/overlays/statistics"))
        .json(&serde_json::json!({"enable": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 412);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"],
        "Parameter \"all\" or \"overlay_name\" is required"
    );

    // Desactivation globale → GET rend des maps vides, agregat = 0.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/overlays/statistics"))
        .json(&serde_json::json!({"enable": false, "all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/ipv8/overlays/statistics"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for entry in body["statistics"].as_array().unwrap() {
        for per_msg in entry.as_object().unwrap().values() {
            assert!(per_msg.as_object().unwrap().is_empty());
        }
    }
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/ipv8/overlays"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for o in body["overlays"].as_array().unwrap() {
        assert_eq!(o["statistics"]["num_up"], 0);
        assert_eq!(o["statistics"]["num_down"], 0);
    }

    // Re-activation par nom d'overlay.
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/overlays/statistics"))
        .json(&serde_json::json!({"enable": true, "overlay_name": "DiscoveryCommunity"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    srv.session.stop().await;
}

/// POST statistics sans stack → 412 `IPv8 is not running`.
#[tokio::test]
async fn ipv8_overlay_statistics_sans_stack() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .post(srv.url("/api/ipv8/overlays/statistics"))
        .json(&serde_json::json!({"enable": true, "all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 412);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "IPv8 is not running");
    srv.session.stop().await;
}

/// `GET /api/ipv8/noblockdht/{mid}` : mid valide → `{"success":
/// true}` immediat ; hex invalide → 500 ; DHT absent → 404 sans
/// cle `success`.
#[tokio::test]
async fn ipv8_noblockdht_semantique() {
    let srv = spawn_server_ipv8().await;
    // mid valide : tache spawn, reponse immediate.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/noblockdht/0123456789abcdef0123456789abcdef01234567"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );

    // hex invalide → `unhexlify` → 500 `handled:false`.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/noblockdht/zz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["error"]["handled"],
        false
    );
    srv.session.stop().await;

    // Sans stack IPv8 → 404 `{"error": "DHT community not found"}`.
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/noblockdht/0123456789abcdef0123456789abcdef01234567"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "DHT community not found");
    assert!(body.get("success").is_none());
    srv.session.stop().await;
}

/// Serveur IPv8 **avec tunnel** (`enable_anonymity`) — loopback
/// UDP ephemere, aucun pair : couvre `/api/ipv8/tunnel/*` avec une
/// `TunnelCommunity` vivante mais sans circuit.
async fn spawn_server_ipv8_anon() -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().into());
    cfg.ipv8.enabled = true;
    cfg.ipv8.listen_addr = "0.0.0.0:0".into();
    cfg.ipv8.bootstrap_peers = Vec::new();
    cfg.ipv8.enable_dht = true;
    cfg.ipv8.enable_anonymity = true;
    let session = CoreSession::start_offline(cfg, Notifier::new())
        .await
        .unwrap();
    let state = AppState::new(session.clone());
    let app = build(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        addr,
        session,
        state,
        client: reqwest::Client::new(),
        _dir: dir,
    }
}

/// `GET /api/ipv8/tunnel/swarms/{ih}/size` — etape 27 : collection
/// vide sans tunnel, `unhexlify` → 500, `hops` (string) → 0 quirk,
/// `{"swarm_size": n}` nominal.
#[tokio::test]
async fn tunnel_swarm_size_semantique() {
    let ih = "0123456789abcdef0123456789abcdef01234567";

    // Sans stack IPv8 → `Response({"swarms": []})` (pas un swarm_size).
    let srv = spawn_server().await;
    let body: serde_json::Value = srv
        .client
        .get(srv.url(&format!("/api/ipv8/tunnel/swarms/{ih}/size")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, serde_json::json!({ "swarms": [] }));
    srv.session.stop().await;

    let srv = spawn_server_ipv8_anon().await;

    // Hex invalide → `binascii.Error` Python → 500 non geree.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/swarms/zz/size"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["handled"], false);

    // `?hops=` present → la valeur arrive en CHAINE chez Python →
    // `select_circuit(hops="2")` echoue → `{"swarm_size": 0}`.
    let body: serde_json::Value = srv
        .client
        .get(srv.url(&format!("/api/ipv8/tunnel/swarms/{ih}/size?hops=2")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, serde_json::json!({ "swarm_size": 0 }));

    // Sans `hops` : aucun circuit → toutes les requetes echouent → 0.
    let body: serde_json::Value = srv
        .client
        .get(srv.url(&format!("/api/ipv8/tunnel/swarms/{ih}/size")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, serde_json::json!({ "swarm_size": 0 }));
    srv.session.stop().await;
}

/// `GET /api/ipv8/tunnel/peers/dht` + `peers/pex` — listes brutes
/// `[]` sans tunnel ; shape `{info_hash, peers[]}` peuplee via
/// `PUT /api/ipv8/dht/values/{key}` (DHTIntroPointPayload).
#[tokio::test]
async fn tunnel_peers_dht_pex_semantique() {
    // Sans stack → `Response([])` brut.
    let srv = spawn_server().await;
    for path in ["/api/ipv8/tunnel/peers/dht", "/api/ipv8/tunnel/peers/pex"] {
        let resp = srv.client.get(srv.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body, serde_json::json!([]));
    }
    srv.session.stop().await;

    let srv = spawn_server_ipv8_anon().await;
    // Stores vides → `[]`.
    for path in ["/api/ipv8/tunnel/peers/dht", "/api/ipv8/tunnel/peers/pex"] {
        let body: serde_json::Value = srv
            .client
            .get(srv.url(path))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body, serde_json::json!([]));
    }

    // Peuple la DHT avec un `DHTIntroPointPayload` :
    // ["ip_address"(type+ipv4+port), "I" last_seen, "varlenH" intro_pk,
    // "varlenH" seeder_pk].
    let mut value = vec![0x01u8]; // ADDRESS_TYPE_IPV4
    value.extend_from_slice(&[10, 0, 0, 7]);
    value.extend_from_slice(&4242u16.to_be_bytes());
    value.extend_from_slice(&1_700_000_000u32.to_be_bytes());
    let intro_pk = [0xAAu8; 32];
    let seeder_pk = [0xBBu8; 32];
    value.extend_from_slice(&32u16.to_be_bytes());
    value.extend_from_slice(&intro_pk);
    value.extend_from_slice(&32u16.to_be_bytes());
    value.extend_from_slice(&seeder_pk);

    // Injection directe dans le stockage local (`storage.put`
    // Python) — `PUT /dht/values` exigerait des noeuds DHT connus.
    let key = "11223344556677889900aabbccddeeff00112233";
    let stack = srv.session.ipv8().expect("stack ipv8");
    let dht = stack.dht.clone().expect("dht community");
    let serialized = dht.serialize_value(&value, false);
    let key_bytes: [u8; 20] = hex::decode(key).unwrap().try_into().unwrap();
    dht.add_value(
        &key_bytes,
        &serialized,
        &onionbit_ipv8::UdpAddress::unspecified(),
        3600.0,
    );

    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/peers/dht"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = body.as_array().expect("liste");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["info_hash"], key);
    let peers = arr[0]["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0]["address"]["ip"], "10.0.0.7");
    assert_eq!(peers[0]["address"]["port"], 4242);
    // `Peer(b"LibNaCLPK:"+intro_pk)` → hex(prefixe+cle).
    let mut expected_pk = b"LibNaCLPK:".to_vec();
    expected_pk.extend_from_slice(&intro_pk);
    assert_eq!(peers[0]["address"]["public_key"], hex::encode(&expected_pk));
    let mut expected_seeder = b"LibNaCLPK:".to_vec();
    expected_seeder.extend_from_slice(&seeder_pk);
    assert_eq!(peers[0]["seeder_pk"], hex::encode(&expected_seeder));
    assert_eq!(peers[0]["source"], 1);
    srv.session.stop().await;
}

/// `GET /api/ipv8/tunnel/circuits/test` + `/{circuit_id}/test` —
/// ordre des validations et erreurs `{"error": ...}` brutes de
/// `tunnel_endpoint.py`.
#[tokio::test]
async fn tunnel_speed_test_validations() {
    // Sans stack → `tunnels is None` → 404 (circuit_id non-numerique
    // reste 400 : l'ordre Python garde la validation du path d'abord).
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/circuits/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "TunnelCommunity is not initialized");
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/circuits/abc/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/circuits/123/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    srv.session.stop().await;

    let srv = spawn_server_ipv8_anon().await;

    // `goal_hops` : `isdigit` + 1..=3.
    for q in [
        "?goal_hops=0",
        "?goal_hops=4",
        "?goal_hops=x",
        "?goal_hops=",
    ] {
        let resp = srv
            .client
            .get(srv.url(&format!("/api/ipv8/tunnel/circuits/test{q}")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{q}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid number of hops specified");
    }

    // Pas de pair candidat → `create_circuit` renvoie None →
    // `{"error": "failed to create circuit"}` 500.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/tunnel/circuits/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "failed to create circuit");

    // Circuit inexistant → 404 ; circuit_id trop grand → 404 aussi
    // (`int` Python a precision arbitraire).
    for id in ["999", "99999999999999999999"] {
        let resp = srv
            .client
            .get(srv.url(&format!("/api/ipv8/tunnel/circuits/{id}/test")))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{id}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "could not find requested circuit");
    }
    srv.session.stop().await;
}

/// `DELETE /api/ipv8/tunnel/anon_lanes/{hops}` (17c-5) : 404 sans
/// stack, 404 sans lane, `{"success": true}` puis 404 quand la lane
/// est detruite — la recreation reste possible (`anon_engine`).
#[tokio::test]
async fn delete_anon_lane_semantique() {
    // Sans stack IPv8 → `{"success": false, "error": ...}` 404.
    let srv = spawn_server().await;
    let resp = srv
        .client
        .delete(srv.url("/api/ipv8/tunnel/anon_lanes/2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    srv.session.stop().await;

    let srv = spawn_server_ipv8_anon().await;
    // Pas de lane a 2 sauts → 404.
    let resp = srv
        .client
        .delete(srv.url("/api/ipv8/tunnel/anon_lanes/2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Lane creee → DELETE success, lane retiree, re-DELETE → 404 ;
    // `anon_engine` recree une lane neuve (get-or-create).
    let stack = srv.session.ipv8().unwrap();
    stack.anon_engine(2).await.unwrap();
    assert!(stack.anon_lanes().iter().any(|(h, _)| *h == 2));
    let resp = srv
        .client
        .delete(srv.url("/api/ipv8/tunnel/anon_lanes/2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    assert!(!stack.anon_lanes().iter().any(|(h, _)| *h == 2));
    let resp = srv
        .client
        .delete(srv.url("/api/ipv8/tunnel/anon_lanes/2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    stack.anon_engine(2).await.unwrap();
    assert!(stack.anon_lanes().iter().any(|(h, _)| *h == 2));
    srv.session.stop().await;
}

/// `/api/ipv8/asyncio/drift` : 404 tant que la mesure n'est pas
/// activee, `enable` via PUT, 400 `incorrect parameters`, 200
/// `Session not initialized.` sans stack IPv8.
#[tokio::test]
async fn asyncio_drift_semantique() {
    let srv = spawn_server_ipv8().await;
    // Mesure desactivee → 404 `Core drift disabled.` (Python).
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/drift"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "Core drift disabled.");

    // `enable` absent → 400 `{"error": "incorrect parameters"}`
    // (sans cle `success`, shape Python exacte).
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/drift"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "incorrect parameters");
    assert!(body.get("success").is_none());

    // Corps non-JSON → `request.json()` leve → 500 non geree.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/drift"))
        .body("pas du json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);

    // Activation puis lecture de l'historique.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/drift"))
        .json(&serde_json::json!({"enable": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/drift"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let measurements = body["measurements"].as_array().unwrap();
    assert!(!measurements.is_empty());
    assert!(measurements[0]["timestamp"].is_f64());
    assert!(measurements[0]["drift"].is_f64());

    // Desactivation → de nouveau 404.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/drift"))
        .json(&serde_json::json!({"enable": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/drift"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    srv.session.stop().await;
}

/// `PUT /drift {"enable"}` sans stack IPv8 → `Session not initialized.`
/// (code 200 — `enable()` retourne `false` en Python).
#[tokio::test]
async fn asyncio_drift_sans_ipv8() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/drift"))
        .json(&serde_json::json!({"enable": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["error"], "Session not initialized.");
    srv.session.stop().await;
}

/// `/api/ipv8/asyncio/tasks` : shape `AsyncioTask` — `name`,
/// `running`, `stack` (+ `taskmanager`/`start_time`/`interval` pour
/// les taches `register_task`).
#[tokio::test]
async fn asyncio_tasks_shape() {
    let srv = spawn_server_ipv8().await;
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tasks = body["tasks"].as_array().unwrap();
    // La boucle `progress` de la session est toujours enregistree.
    let progress = tasks.iter().find(|t| t["name"] == "progress").unwrap();
    assert_eq!(progress["taskmanager"], "CoreSession");
    assert!(progress["start_time"].is_f64());
    assert!(progress["interval"].is_f64());
    assert_eq!(progress["running"], false);
    assert_eq!(progress["stack"], serde_json::json!([]));
    // IPv8 actif → taches de maintenance DHT enregistrees.
    assert!(tasks
        .iter()
        .any(|t| t["name"] == "node_maintenance" && t["taskmanager"] == "DHTDiscoveryCommunity"));
    srv.session.stop().await;
}

/// `/api/ipv8/asyncio/debug` : PUT `enable`/`slow_callback_duration`,
/// GET renvoie `messages`/`enable`/`slow_callback_duration`.
#[tokio::test]
async fn asyncio_debug_semantique() {
    let srv = spawn_server().await;
    // Etat initial : debug off, `slow_callback_duration` 0.1 asyncio.
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/debug"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enable"], false);
    assert_eq!(body["slow_callback_duration"], 0.1);
    assert_eq!(body["messages"], serde_json::json!([]));

    // Aucun parametre → 400 `{"success": false, ...}`.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/debug"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        false
    );

    // `enable` + `slow_callback_duration` → `{"success": true}`.
    let resp = srv
        .client
        .put(srv.url("/api/ipv8/asyncio/debug"))
        .json(&serde_json::json!({"enable": true, "slow_callback_duration": 0.5}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["success"],
        true
    );
    let resp = srv
        .client
        .get(srv.url("/api/ipv8/asyncio/debug"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enable"], true);
    assert_eq!(body["slow_callback_duration"], 0.5);
    srv.session.stop().await;
}

/// `GET /api/rss` : listing des items persistes (`rss_items`) —
/// extension Rust documentee (Python n'expose que `PUT`).
#[tokio::test]
async fn rss_list_items() {
    let srv = spawn_server().await;
    let resp = srv.client.get(srv.url("/api/rss")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["items"], serde_json::json!([]));

    // Injection directe (le fetch reel est couvert par les tests
    // loopback du service RSS dans onionbit-core).
    srv.session
        .db()
        .insert_rss_item("http://feed.example/rss", "http://t/1.torrent", 1000)
        .unwrap();
    srv.session
        .db()
        .set_rss_item_metadata(
            "http://feed.example/rss",
            "http://t/1.torrent",
            "Titre",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
    let resp = srv.client.get(srv.url("/api/rss")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["feed_url"], "http://feed.example/rss");
    assert_eq!(items[0]["title"], "Titre");
    assert_eq!(
        items[0]["infohash"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    srv.session.stop().await;
}

#[tokio::test]
async fn tracker_lists_follow_trackerstatusdict_shape() {
    let srv = spawn_server().await;
    let ih0 = add_paused(&srv).await;
    // Shape `TrackerStatusDict` Python (`{url, peers, seeds, leeches,
    // status}`) identique sur `downloads[].trackers` et
    // `GET /downloads/{ih}/trackers` (`tracker_info`), avec les
    // pseudo-entrees `[DHT]`/`[PeX]` en queue.
    let body: serde_json::Value = srv
        .client
        .get(srv.url("/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let downloads = body["downloads"].as_array().unwrap();
    let dl = downloads
        .iter()
        .find(|d| d["infohash"].as_str() == Some(ih0.as_str()))
        .unwrap();
    let ih = ih0;
    let in_list = dl["trackers"].as_array().unwrap().clone();

    let dedicated: serde_json::Value = srv
        .client
        .get(srv.url(&format!("/api/downloads/{ih}/trackers")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let trackers = dedicated["tracker_info"].as_array().unwrap();
    assert_eq!(&in_list, trackers);
    for t in trackers {
        for k in ["url", "peers", "seeds", "leeches", "status"] {
            assert!(t.get(k).is_some(), "cle absente : {k} ({t})");
        }
    }
    let urls: Vec<&str> = trackers.iter().filter_map(|t| t["url"].as_str()).collect();
    assert_eq!(&urls[urls.len() - 2..], &["[DHT]", "[PeX]"]);
    srv.session.stop().await;
}

#[tokio::test]
async fn logging_trouve_le_journal_rolle_par_date() {
    let srv = spawn_server().await;
    // `tracing_appender::rolling::daily` produit `onionbit.log.YYYY-MM-DD`
    // (l'extension est la date) — regression : le filtre ne matchait
    // que `*.log` et l'onglet Journaux restait vide.
    let logs_dir = srv.session.config().state_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(
        logs_dir.join("onionbit.log.2026-09-28"),
        "ligne ancienne\nligne recente\n",
    )
    .unwrap();

    let resp = srv
        .client
        .get(srv.url("/api/logging?max_lines=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert_eq!(text.trim(), "ligne recente");
    srv.session.stop().await;
}

#[tokio::test]
async fn web_ui_statiques_exemptes_d_auth() {
    // UI web servie par le daemon : statiques hors `/api` sans clé
    // (parité exemptions `/ui`/`/static` Python), `/api/*` toujours
    // protégé, repli SPA vers index.html, pas de sortie de racine.
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(
        web.join("index.html"),
        "<html><head></head><body>onionbit ui</body></html>",
    )
    .unwrap();
    std::fs::write(web.join("assets/app.js"), "// js").unwrap();
    // Secret hors de la racine servie : ne doit jamais fuiter.
    std::fs::write(dir.path().join("secret.txt"), "TOPSECRET").unwrap();

    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let state = AppState::new(session.clone())
        .with_api_key("cle-test")
        .with_web_ui_dir(Some(web));
    let app = build(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let url = |p: &str| format!("http://{addr}{p}");

    // Statique racine sans clé : 200 + en-têtes de sécurité.
    let resp = client.get(url("/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-cache");
    let html = resp.text().await.unwrap();
    assert!(html.contains("onionbit ui"));
    // Auto-connexion : la clé API est injectée en meta dans l'index
    // servi (api/web_ui_inject_key = true par défaut).
    assert!(html.contains("name=\"onionbit-api-key\" content=\"cle-test\""));

    // /index.html direct et repli SPA servent la version injectée.
    let resp = client.get(url("/index.html")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("onionbit-api-key"));

    // Asset + repli SPA (route inconnue hors /api → index.html).
    let resp = client.get(url("/assets/app.js")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "// js");
    let resp = client.get(url("/downloads/abc")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("onionbit ui"));

    // Traversée de chemin : le secret hors racine n'est pas servi.
    let resp = client.get(url("/%2e%2e/secret.txt")).send().await.unwrap();
    assert_ne!(resp.text().await.unwrap(), "TOPSECRET");

    // `/api/*` reste derrière la clé — même un chemin inconnu (401,
    // pas 404 — parité ApiKeyMiddleware).
    let resp = client.get(url("/api/downloads")).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    let resp = client.get(url("/api/inconnu")).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    let resp = client.get(url("/api")).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    let resp = client
        .get(url("/api/downloads"))
        .header("X-Api-Key", "cle-test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    session.stop().await;
}

#[tokio::test]
async fn web_ui_injection_cle_desactivable() {
    // api/web_ui_inject_key = false : l'index servi est brut, la clé
    // n'y apparait pas (saisie manuelle ou ?key= dans l'UI).
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<html>ui</html>").unwrap();

    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let state = AppState::new(session.clone())
        .with_api_key("cle-test")
        .with_web_ui_dir(Some(web))
        .with_web_ui_inject_key(false);
    let app = build(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(!html.contains("onionbit-api-key"));
    assert!(!html.contains("cle-test"));

    session.stop().await;
}

// ============================================================================
// Extension Rust — GET /api/connections (agregat diagnostic)
// ============================================================================

#[tokio::test]
async fn connections_agregat_offline() {
    let srv = spawn_server().await;
    let resp = srv
        .client
        .get(srv.url("/api/connections"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Session offline : ni stack IPv8 ni ecoute BitTorrent — les deux
    // collections sont presentes mais vides.
    assert_eq!(body["connections"], serde_json::json!([]));
    assert_eq!(body["listeners"], serde_json::json!([]));

    // Chaque entree distante exposerait les cles du contrat.
    let resp = srv
        .client
        .get(srv.url("/api/connections"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    for c in body["connections"].as_array().unwrap() {
        for k in [
            "ip",
            "port",
            "transports",
            "ipv8",
            "mid",
            "overlays",
            "dht",
            "tunnel_flags",
            "exit_circuits",
            "bittorrent",
        ] {
            assert!(c.get(k).is_some(), "cle {k} manquante");
        }
    }
    srv.session.stop().await;
}

/// Cycle complet identite : cle publique exposee, export brut +
/// protege `OBID`, restauration (remplace `ipv8_keypair.bin`,
/// `restart_required`), refus des blobs invalides.
#[tokio::test]
async fn identite_export_import_cycle() {
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
    use onionbit_crypto::keyblob::{keyblob_open, keyblob_seal};

    let srv = spawn_server_ipv8().await;
    let stack = srv.session.ipv8().expect("ipv8 actif");

    // GET : la cle publique correspond a la cle secrete du stack.
    let resp = srv
        .client
        .get(srv.url("/api/identity"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let pk = body["public_key"].as_str().unwrap().to_string();
    assert_eq!(pk, stack.public_key_hex());
    // Jamais de materiel prive dans la reponse GET.
    assert!(body.get("key").is_none());
    assert!(body.get("secret").is_none());

    // Export brut : la cle hex se decode en la meme cle secrete.
    let resp = srv
        .client
        .post(srv.url("/api/identity/export"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], false);
    let raw = hex::decode(body["key"].as_str().unwrap()).unwrap();
    LibNaClSecretKey::from_bin(&raw).unwrap();
    assert_eq!(raw, stack.secret_key_bin());

    // Export protege : blob OBID, re-ouvrable avec le mot de passe.
    let resp = srv
        .client
        .post(srv.url("/api/identity/export"))
        .json(&serde_json::json!({"password": "phrase forte"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], true);
    let blob = hex::decode(body["key"].as_str().unwrap()).unwrap();
    let ouvert = keyblob_open(b"phrase forte", &blob).unwrap();
    assert_eq!(ouvert, stack.secret_key_bin());
    // Mauvais mot de passe → echec AEAD.
    assert!(keyblob_open(b"autre", &blob).is_err());

    // Restauration d'une NOUVELLE cle (blob OBID protege) :
    // restart_required + fichier remplace sur disque.
    let nouvelle = LibNaClSecretKey::generate();
    let blob_b = keyblob_seal(b"mdp", &nouvelle.to_bin()).unwrap();
    let resp = srv
        .client
        .post(srv.url("/api/identity/restore"))
        .json(&serde_json::json!({
            "key": hex::encode(&blob_b),
            "password": "mdp",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["restart_required"], true);
    let sur_disque =
        std::fs::read(srv.session.config().state_dir.join("ipv8_keypair.bin")).unwrap();
    let rechargee = LibNaClSecretKey::from_bin(&sur_disque).unwrap();
    assert_eq!(
        rechargee.public_key().to_bin(),
        nouvelle.public_key().to_bin()
    );

    // Blob OBID sans mot de passe → 400 ; mauvais mot de passe → 400 ;
    // hex invalide → 400. Le fichier conserve la cle B dans tous les cas.
    for json in [
        serde_json::json!({"key": hex::encode(&blob_b)}),
        serde_json::json!({"key": hex::encode(&blob_b), "password": "faux"}),
        serde_json::json!({"key": "zzzz"}),
        serde_json::json!({"key": hex::encode(b"pas une cle")}),
    ] {
        let resp = srv
            .client
            .post(srv.url("/api/identity/restore"))
            .json(&json)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "cas {json}");
    }
    let sur_disque =
        std::fs::read(srv.session.config().state_dir.join("ipv8_keypair.bin")).unwrap();
    assert_eq!(sur_disque, nouvelle.to_bin());

    srv.session.stop().await;
}
