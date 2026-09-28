//! Test d'integration bout en bout : binaire `tribler-cli` reel
//! contre un serveur `tribler-api` monte sur `127.0.0.1:0`
//! (loopback uniquement, aucune sortie reseau).

use std::net::SocketAddr;

use tokio::net::TcpListener;

use tribler_api::{build, AppState};
use tribler_core::{CoreConfig, CoreSession, Notifier};

/// .torrent minimal valide (fixture partagee `tribler-test-support`).
fn test_torrent_bytes() -> Vec<u8> {
    tribler_test_support::test_torrent_bytes("cli-test.bin", 42)
}

/// Montre le serveur API sur loopback, retourne l'URL de base.
async fn spawn_api() -> (String, CoreSession, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let app = build(AppState::new(session.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), session, dir)
}

/// Execute le binaire `tribler-cli` compile par cargo (async : un
/// `std::process::Command` bloquerait le runtime mono-thread du test
/// et empecherait le serveur de repondre).
async fn run_cli(api: &str, args: &[&str]) -> std::process::Output {
    let mut full = vec!["--api".to_string(), api.to_string()];
    full.extend(args.iter().map(|s| s.to_string()));
    run_cli_args(&full).await
}

/// Execute `tribler-cli` avec des arguments bruts (sans `--api`
/// injecte — pour tester la decouverte via `configuration.json`).
async fn run_cli_args(args: &[String]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_tribler-cli");
    tokio::process::Command::new(bin)
        .args(args)
        .output()
        .await
        .expect("lancement tribler-cli")
}

#[tokio::test]
async fn cli_status_list_add_remove_loopback() {
    let (api, session, dir) = spawn_api().await;

    // `status` sur un daemon joignable.
    let out = run_cli(&api, &["status"]).await;
    assert!(
        out.status.success(),
        "status={:?} stdout={} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("daemon OK"), "stdout={stdout}");

    // `list` vide.
    let out = run_cli(&api, &["list"]).await;
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("aucun telechargement"));

    // `add` avec un .torrent local.
    let torrent_path = dir.path().join("cli-test.torrent");
    std::fs::write(&torrent_path, test_torrent_bytes()).unwrap();
    let out = run_cli(&api, &["add", &torrent_path.display().to_string()]).await;
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("ajoute"));

    // `list` affiche maintenant l'entree.
    let out = run_cli(&api, &["list"]).await;
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("cli-test.bin"), "stdout={stdout}");
    let infohash = stdout
        .lines()
        .find(|l| l.contains("cli-test.bin"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap()
        .to_string();

    // `pause` puis `resume`.
    for sub in ["pause", "resume"] {
        let out = run_cli(&api, &[sub, &infohash]).await;
        assert!(out.status.success(), "{sub} : {:?}", out);
    }

    // `remove`.
    let out = run_cli(&api, &["remove", &infohash]).await;
    assert!(out.status.success());

    // `status` sur un port ou rien n'ecoute -> echec propre.
    let out = run_cli("http://127.0.0.1:1", &["status"]).await;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("injoignable"));

    session.stop().await;
}

/// Etape 21 : le daemon exige une cle API — le CLI l'envoie via
/// `--api-key` ou la lit dans `<state-dir>/configuration.json`
/// (avec `http_port_running` pour trouver l'URL).
#[tokio::test]
async fn cli_cle_api_explicite_et_decouverte_state_dir() {
    // Serveur avec cle API activee.
    let dir = tempfile::tempdir().unwrap();
    let session =
        CoreSession::start_offline(CoreConfig::offline(dir.path().into()), Notifier::new())
            .await
            .unwrap();
    let app = build(AppState::new(session.clone()).with_api_key("cle-cli-test"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let api = format!("http://{addr}");

    // Sans cle : 401 en erreur propre.
    let out = run_cli(&api, &["status"]).await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("401"),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // --api-key explicite.
    let out = run_cli(&api, &["--api-key", "cle-cli-test", "status"]).await;
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Decouverte via --state-dir : configuration.json fournit la cle
    // ET le port (comme api/http_port_running Python).
    let state_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        state_dir.path().join("configuration.json"),
        serde_json::json!({
            "api": {
                "key": "cle-cli-test",
                "http_host": "127.0.0.1",
                "http_port_running": addr.port()
            }
        })
        .to_string(),
    )
    .unwrap();
    let out = run_cli_args(&[
        "--state-dir".into(),
        state_dir.path().display().to_string(),
        "status".into(),
    ])
    .await;
    assert!(
        out.status.success(),
        "decouverte via configuration.json — stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    session.stop().await;
}
