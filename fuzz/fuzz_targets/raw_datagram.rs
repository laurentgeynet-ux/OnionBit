// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Datagramme brut contre le dispatcher complet `on_raw_datagram`
//! d'un noeud reel (endpoint loopback, aucun circuit) : exercice le
//! chemin `is_cell -> process_cell -> parse/decrypt/dispatch` tel
//! qu'un pair hostile l'atteint.

#![no_main]

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::Network;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::PEER_FLAG_RELAY;

fn node() -> &'static (tokio::runtime::Runtime, Arc<TunnelCommunity>) {
    static NODE: OnceLock<(tokio::runtime::Runtime, Arc<TunnelCommunity>)> = OnceLock::new();
    NODE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let tunnel = rt.block_on(async {
            let ep = UdpEndpoint::bind("127.0.0.1:0").await.expect("bind");
            TunnelCommunity::new(
                LibNaClSecretKey::generate(),
                Arc::new(Network::default()),
                ep,
                PEER_FLAG_RELAY,
            )
            .await
        });
        (rt, tunnel)
    })
}

fuzz_target!(|data: &[u8]| {
    let (rt, tunnel) = node();
    let src: SocketAddr = "127.0.0.1:4242".parse().unwrap();
    rt.block_on(async {
        tunnel.on_raw_datagram(src, data);
        // Donne un tick aux taches spawnnes par le dispatch.
        tokio::task::yield_now().await;
    });
});
