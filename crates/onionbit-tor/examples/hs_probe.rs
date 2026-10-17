// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Spike ADR-0029 etape 120 : hebergement d'un service onion v3 via
//! `tor-hsservice` (arti embarque). Evalue : bootstrap + lancement,
//! obtention de l'adresse `.onion`, persistance de la cle au reboot
//! (relancer = meme adresse), cout de tenue des intro points.
//!
//! Usage : `cargo run -p onionbit-tor --features hs --example
//! hs_probe -- <state_dir> [timeout_s]`. Relancer avec le meme
//! <state_dir> : l'adresse .onion doit etre identique (cle persistee
//! dans le keystore de state_dir).

#[cfg(feature = "hs")]
fn main() {
    use arti_client::config::TorClientConfigBuilder;
    use arti_client::{BootstrapBehavior, TorClient};
    use futures_util::StreamExt;
    use tor_hsservice::config::OnionServiceConfigBuilder;
    use tor_hsservice::HsNickname;

    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "arti-hs-probe-state".to_string());
    let timeout_secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime tokio");

    rt.block_on(async move {
        tracing_subscriber::fmt::init();
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let config = TorClientConfigBuilder::from_directories(
            format!("{dir}/state"),
            format!("{dir}/cache"),
        )
        .build()
        .expect("TorClientConfig");
        let client = TorClient::builder()
            .config(config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await
            .expect("client");
        let mut events = client.bootstrap_events();
        tokio::spawn(async move {
            while let Some(s) = events.next().await {
                eprintln!("==> {:.0}%", s.as_frac() * 100.0);
            }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            client.bootstrap(),
        )
        .await
        .expect("timeout bootstrap")
        .expect("bootstrap");
        eprintln!("==> READY");

        // Service onion minimal : nickname fixe → cle derivee du
        // keystore persistant sous state_dir ; la meme adresse doit
        // ressortir a chaque lancement sur le meme state_dir.
        let nickname = HsNickname::new("onionbit-probe".to_string()).expect("nickname");
        let svc_config = OnionServiceConfigBuilder::default()
            .nickname(nickname)
            .build()
            .expect("OnionServiceConfig");
        let (svc, _rend_requests) = client
            .launch_onion_service(svc_config)
            .expect("launch_onion_service")
            .expect("service actif");
        match svc.onion_address() {
            Some(addr) => {
                use safelog::DisplayRedacted;
                eprintln!("==> ONION http://{}", addr.display_unredacted());
            }
            None => eprintln!("!! pas d'adresse onion"),
        }
        let mut status = svc.status_events();
        let dl = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        loop {
            let Ok(Some(st)) = tokio::time::timeout_at(dl, status.next()).await else {
                break;
            };
            eprintln!("==> status: {:?}", st.state());
            if st.state().is_fully_reachable() {
                eprintln!("==> SERVICE REACHABLE (descripteur publie)");
                break;
            }
        }
        eprintln!("==> DONE — relancer sur le meme state_dir : meme adresse attendue");
    });
}

#[cfg(not(feature = "hs"))]
fn main() {
    eprintln!("compile avec `--features hs` (spike ADR-0029 etape 120)");
    std::process::exit(2);
}
