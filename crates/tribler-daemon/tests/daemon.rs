//! Test e2e du binaire `tribler-daemon` : demarrage en `--offline`
//! (aucun trafic sortant), creation de `configuration.json` (cle API
//! generee + `http_port_running` publie), reponse de l'API sur
//! loopback authentifiee par `X-Api-Key`, arret.

use std::time::Duration;

/// Cherche un port loopback libre — fixture partagee
/// `tribler-test-support` (fenetre de course acceptable en test).
fn free_port() -> u16 {
    tribler_test_support::free_port()
}

/// Attend que `path` existe et le parse en JSON (le daemon ecrit
/// `configuration.json` au tout debut du demarrage).
async fn read_config(path: &std::path::Path) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v.pointer("/api/key").and_then(|k| k.as_str()).is_some() {
                    return v;
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "configuration.json jamais ecrit dans {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn daemon_offline_repond_a_l_api_avec_cle_puis_s_arrete() {
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
        // Pas d'icone systray pendant les tests.
        .arg("--no-tray")
        .kill_on_drop(true)
        .spawn()
        .expect("lancement tribler-daemon");

    // Le daemon ecrit configuration.json avec la cle API generee.
    let config_path = dir.path().join("configuration.json");
    let config = read_config(&config_path).await;
    let key = config
        .pointer("/api/key")
        .and_then(|k| k.as_str())
        .expect("api.key absente")
        .to_string();
    assert_eq!(key.len(), 32, "cle API hex attendue : {key}");

    // Sans cle -> 401 {error:{handled:true}} ; avec la cle -> 200.
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut up = false;
    while std::time::Instant::now() < deadline {
        match client.get(format!("{base}/api/downloads")).send().await {
            Ok(r) if r.status() == 401 => {
                let body: serde_json::Value = r.json().await.unwrap();
                assert_eq!(body["error"]["handled"], true);
                assert_eq!(body["error"]["message"], "Unauthorized access");
                up = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    assert!(up, "le daemon n'a pas expose l'API protegee sur {base}");

    // L'API repond : le bind a eu lieu et le port reel a ete publie
    // (`api/http_port_running`, comme Python). On relit le fichier.
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        config
            .pointer("/api/http_port_running")
            .and_then(|v| v.as_u64()),
        Some(port as u64),
        "http_port_running devrait valoir {port}"
    );

    let body: serde_json::Value = client
        .get(format!("{base}/api/downloads"))
        .header("x-api-key", &key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["downloads"], serde_json::json!([]));

    // Variante query `?key=` (ApiKeyMiddleware Python).
    let resp = client
        .get(format!("{base}/api/downloads?key={key}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Arret du processus (kill_on_drop assure le nettoyage en plus).
    child.kill().await.unwrap();
    let _ = child.wait().await;
}
