// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sonde `TorProxyServer` hors daemon (ADR-0030, spike etape 129) :
//! bootstrappe `TorRuntime` embedded puis sert le proxy forward
//! loopback — permet les preuves `curl` sur reseau reel.
//!
//! Usage : `cargo run -p onionbit-tor --features tor --example
//! proxy_probe -- <state_dir> [listen]`
//!
//! Puis :
//!   curl -x http://127.0.0.1:<port> http://<miroir>.onion/
//!   curl --proxytunnel -x http://127.0.0.1:<port> https://<hote>/
//!   curl -x http://127.0.0.1:<port> http://example.com/   (→ 403)

#[cfg(feature = "tor")]
fn main() {
    use onionbit_tor::{proxy, ProxyConfig, TorRuntime, TorSettings, TorState};
    use std::time::Duration;

    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("state_dir");
    let listen: std::net::SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:0".to_string())
        .parse()
        .expect("listen addr");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime tokio");

    rt.block_on(async move {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        // Debug : erreurs de connexion proxy visibles (jamais d'URL/
        // hote au-dessus de debug — ADR-0028 §5).
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
        let runtime = TorRuntime::new(TorSettings {
            enabled: true,
            state_dir: dir.into(),
            request_timeout: Duration::from_secs(60),
            bootstrap_timeout: Duration::from_secs(600),
            ..TorSettings::default()
        });
        runtime.bootstrap().expect("bootstrap lance");
        loop {
            match runtime.state() {
                TorState::Ready => break,
                TorState::Bootstrapping { progress } => {
                    println!("==> bootstrap {progress}%");
                }
                TorState::Error(e) => panic!("bootstrap : {e}"),
                _ => {}
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let addr = proxy::serve(runtime, listen, ProxyConfig::default())
            .await
            .expect("serve");
        println!("==> PROXY PRET http://{addr}");
        println!("    curl -x http://{addr} http://<miroir>.onion/");
        println!("    curl --proxytunnel -x http://{addr} https://<hote>/");
        std::future::pending::<()>().await;
    });
}

#[cfg(not(feature = "tor"))]
fn main() {
    eprintln!("compiler avec --features tor");
    std::process::exit(1);
}
