// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sonde de fetch `.onion` hors daemon — passe par `TorRuntime` et
//! `fetch_onion` (le meme chemin borne que l'API, politique incluse)
//! puis dumpe statut + en-tetes + corps dans un fichier, pour
//! inspecter le markup reel d'un backend de recherche (diagnostic
//! `structure_changed`).
//!
//! Usage : `cargo run -p onionbit-tor --features tor --example
//! onion_fetch_probe -- <state_dir> <host.onion> <path> <out.html>`
//! Ex. : `… onionbit-tor-e2e piratebay….onion /q.php?q=test tpb.json`

#[cfg(feature = "tor")]
fn main() {
    use onionbit_network_policy::TorTargetPolicy;
    use onionbit_tor::{TorRuntime, TorSettings, TorState};
    use std::path::PathBuf;
    use std::time::Duration;

    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("state_dir");
    let host = args.next().expect("host.onion");
    let path = args.next().unwrap_or_else(|| "/".to_string());
    let out = args.next().unwrap_or_else(|| "onion-page.html".to_string());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime tokio");

    rt.block_on(async move {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut policy = TorTargetPolicy::new();
        if host.ends_with(".onion") {
            policy.insert(&host).expect("politique cible");
        } else {
            // Hote clearnet — famille `api_via_exit` (sortie Tor).
            policy.insert_exit_host(&host).expect("politique sortie");
        }
        let settings = TorSettings {
            enabled: true,
            state_dir: PathBuf::from(&dir),
            target_policy: policy,
            ..Default::default()
        };
        let runtime = TorRuntime::new(settings);
        runtime.bootstrap().expect("bootstrap");
        loop {
            match runtime.state() {
                TorState::Ready => break,
                TorState::Bootstrapping { progress } => {
                    eprintln!("bootstrap {progress}%");
                }
                TorState::Error(e) => panic!("bootstrap en echec : {e}"),
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let url = format!("http://{host}{path}");
        eprintln!("fetch {url}");
        let resp = runtime.fetch_onion(&url).await.expect("fetch");
        let mut dump = format!("HTTP {}\n", resp.status).into_bytes();
        for (k, v) in &resp.headers {
            dump.extend(format!("{k}: {v}\n").into_bytes());
        }
        dump.extend(b"\n");
        dump.extend(&resp.body);
        std::fs::write(&out, &dump).expect("ecriture");
        eprintln!(
            "==> status {} — {} octets -> {out}",
            resp.status,
            resp.body.len()
        );
    });
}

#[cfg(not(feature = "tor"))]
fn main() {
    eprintln!("compiler avec --features tor");
}
