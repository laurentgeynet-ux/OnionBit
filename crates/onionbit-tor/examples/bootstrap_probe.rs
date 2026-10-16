// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sonde de bootstrap arti hors daemon — isole le runtime OnionBit du
//! transport arti pour diagnostiquer les blocages de consensus.
//!
//! Usage : `cargo run -p onionbit-tor --features tor --example
//! bootstrap_probe -- <state_dir>` (ex. `%TEMP%\arti-probe`).

#[cfg(feature = "tor")]
fn main() {
    use arti_client::config::TorClientConfigBuilder;
    use arti_client::{BootstrapBehavior, TorClient};
    use futures_util::StreamExt;

    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "arti-probe-state".to_string());
    let timeout_secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime tokio");

    rt.block_on(async move {
        tracing_subscriber::fmt::init();
        // arti n'embarque aucun provider rustls — le process doit en
        // installer un avant toute connexion TLS.
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
                eprintln!(
                    "==> {:.0}% | {s} | blocked={:?}",
                    s.as_frac() * 100.0,
                    s.blocked()
                );
            }
        });
        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            client.bootstrap(),
        )
        .await
        {
            Ok(Ok(())) => eprintln!("==> READY"),
            Ok(Err(e)) => eprintln!("==> Echec bootstrap : {e}"),
            Err(_) => eprintln!("==> TIMEOUT {timeout_secs}s"),
        }
    });
}

#[cfg(not(feature = "tor"))]
fn main() {
    eprintln!("compiler avec --features tor");
}
