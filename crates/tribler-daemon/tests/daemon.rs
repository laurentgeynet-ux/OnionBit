//! Test e2e du binaire `tribler-daemon` : demarrage en `--offline`
//! (aucun trafic sortant), reponse de l'API sur loopback, arret.

use std::net::TcpListener as StdListener;
use std::time::Duration;

/// Cherche un port loopback libre, le libere, puis le confie au
/// daemon (fenetre de course acceptable pour un test local).
fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn daemon_offline_repond_a_l_api_puis_s_arrete() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let listen = format!("127.0.0.1:{port}");
    let base = format!("http://{listen}");

    let bin = env!("CARGO_BIN_EXE_tribler-daemon");
    let mut child = tokio::process::Command::new(bin)
        .arg("--listen")
        .arg(&listen)
        .arg("--state-dir")
        .arg(dir.path())
        .arg("--offline")
        .kill_on_drop(true)
        .spawn()
        .expect("lancement tribler-daemon");

    // Attend que l'API reponde (le daemon met un peu de temps a
    // demarrer : ouverture DB + moteur).
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut up = false;
    while std::time::Instant::now() < deadline {
        match client.get(format!("{base}/api/downloads")).send().await {
            Ok(r) if r.status().is_success() => {
                up = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    assert!(up, "le daemon n'a pas expose l'API sur {base}");

    let body: serde_json::Value = client
        .get(format!("{base}/api/downloads"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["downloads"], serde_json::json!([]));

    // Arret du processus (kill_on_drop assure le nettoyage en plus).
    child.kill().await.unwrap();
    let _ = child.wait().await;
}
