//! Integration de `tribler-network-policy` dans le moteur :
//! - `socks5_proxy` non loopback refuse au demarrage (proxy guard) ;
//! - kill switch : sonde du proxy, `add`/`resume` refuses tant qu'il
//!   est engage.
//!
//! Tout est en loopback — aucun trafic externe.

use std::time::Duration;

use tribler_bittorrent::config::EngineConfig;
use tribler_bittorrent::engine::BtEngine;
use tribler_bittorrent::error::BtError;

/// Attente max pour que le watchdog sonde le proxy (1er tick
/// immediat, timeout de sonde 2 s).
const ENGAGE_TIMEOUT: Duration = Duration::from_secs(8);

/// Un proxy SOCKS5 distant doit faire echouer `start` — jamais de
/// repli silencieux vers une connexion directe.
#[tokio::test]
async fn remote_socks5_proxy_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = EngineConfig::offline(dir.path().join("dl"));
    cfg.socks5_proxy = Some("socks5://8.8.8.8:1080".into());
    let res = BtEngine::start(cfg).await;
    assert!(
        matches!(res, Err(BtError::Policy(_))),
        "proxy distant accepte : {res:?}"
    );
}

/// Proxy loopback valide : la session demarre et le kill switch
/// s'engage tant que le port n'ecoute pas -> `add_uri` refuse.
#[tokio::test]
async fn kill_switch_blocks_add_while_proxy_down() {
    let dir = tempfile::tempdir().unwrap();
    // Port loopback ferme : le watchdog doit engager le kill switch.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let mut cfg = EngineConfig::offline(dir.path().join("dl"));
    cfg.socks5_proxy = Some(format!("socks5://{addr}"));
    let engine = BtEngine::start(cfg).await.unwrap();
    let ks = engine
        .kill_switch()
        .expect("kill switch absent avec proxy configure");

    tokio::time::timeout(ENGAGE_TIMEOUT, async {
        while !ks.is_engaged() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("kill switch jamais engage");

    let res = engine
        .add_uri("magnet:?xt=urn:btih:0000000000000000000000000000000000000000")
        .await;
    assert!(
        matches!(res, Err(BtError::Policy(_))),
        "add accepte malgre le kill switch : {res:?}"
    );

    // Le proxy revient : le watchdog desengage.
    let listener = std::net::TcpListener::bind(addr).unwrap();
    tokio::time::timeout(ENGAGE_TIMEOUT, async {
        while ks.is_engaged() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("kill switch jamais desengage");
    drop(listener);
    engine.stop().await;
}
