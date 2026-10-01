// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Noeud d'interop DHT Rust pour l'etape 10 : prouve
//! `find`/`store`/`find` aller-retour avec un vrai `DHTCommunity`
//! pyipv8, dont acceptation + refus des tokens apres rotation des
//! secrets cote Python.
//!
//! Choreographie (le Python est pilote par `py_dht_node.py`,
//! cf. `scripts/interop_dht.ps1`) :
//! 1. attend que le noeud Python soit connu (ping -> table de routage)
//! 2. `find_values(K_RUST)`            -> RUST_FIND_OK
//! 3. `store_value(K_RUST, v)`         -> RUST_STORE_OK (token Python accepte)
//!    — cote Python ce store declenche la rotation des secrets
//! 4. `store_on_nodes` avec le token   -> RUST_STALE_REJECTED attendu
//!    desormais evince (sans re-find)
//! 5. `find_nodes` + `store_on_nodes`  -> RUST_REFRESHED_STORE_OK
//!    (token frais apres rotation)
//! 6. sert les requetes Python jusqu'a la fin.
//!
//! Usage :
//! `dht_interop_node --port P --py-addr 127.0.0.1:Q --py-key-file F
//!                  --key-file F --duration S`

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use onionbit_crypto::hash::sha1;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::dht::routing::Node;
use onionbit_ipv8::dht::DhtCommunity;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::UdpAddress;

/// Cible partagee avec `py_dht_node.py` (K_RUST = b"rs" + 18*0x00).
const K_RUST: [u8; 20] = {
    let mut k = [0u8; 20];
    k[0] = b'r';
    k[1] = b's';
    k
};
/// Seconde cible pour le store post-rotation.
const K_RUST2: [u8; 20] = {
    let mut k = [0u8; 20];
    k[0] = b'r';
    k[1] = b'2';
    k
};
/// Valeur stockee puis relue par le Python (`RUST_VALUE`).
const RUST_VALUE: &[u8] = b"rust-value";

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let mut port = "0".to_string();
    let mut py_addr_s = None;
    let mut py_key_file = None;
    let mut key_file = None;
    let mut duration = 28u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().unwrap(),
            "--py-addr" => py_addr_s = args.next(),
            "--py-key-file" => py_key_file = args.next(),
            "--key-file" => key_file = args.next(),
            "--duration" => duration = args.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }
    let deadline = Instant::now() + Duration::from_secs(duration);

    let ep = UdpEndpoint::bind(&format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    // Tap brut : chaque datagramme recu est reloge en hex sur stderr
    // (diagnostic interop).
    let mut tap_rx = ep.set_tap().await;
    tokio::spawn(async move {
        while let Ok((dir, addr, bytes)) = tap_rx.recv().await {
            let d = match dir {
                onionbit_ipv8::endpoint::TapDir::Rx => "RX",
                onionbit_ipv8::endpoint::TapDir::Tx => "TX",
            };
            eprintln!("TAP|{d}|{addr}|{}", hex::encode(&bytes));
        }
    });
    let local = ep.local_addr().unwrap();
    let wan = UdpAddress::from(local);
    let lan = UdpAddress::from("127.0.0.1:0".parse::<SocketAddr>().unwrap());

    let key = LibNaClSecretKey::generate();
    if let Some(f) = &key_file {
        std::fs::write(f, hex::encode(key.public_key().to_bin())).unwrap();
    }

    let community = DhtCommunity::new(key, wan, lan, ep.clone()).await;
    let ep2 = ep.clone();
    tokio::spawn(async move {
        let _ = ep2.run().await;
    });

    // 1. Attendre que le Python nous ait pinges (table de routage).
    let mut known = false;
    while Instant::now() < deadline && !known {
        known = community.node_count() > 0;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("RUST_NODES|{}", community.node_count());

    // 2. find_values : obtient un token Python (et prouve le decodage
    // find-response : token + values + nodes).
    match community.find_values(&K_RUST, 0).await {
        Ok(v) => eprintln!("RUST_FIND_OK|{}", v.len()),
        Err(e) => eprintln!("RUST_FIND_FAIL|{e}"),
    }

    // 3. store_value **signe** : le token Python doit etre accepte et
    // la signature Rust verifiable par `deserialize_value` Python.
    match community.store_value(&K_RUST, RUST_VALUE, true).await {
        Ok(n) => eprintln!("RUST_STORE_OK|{}", n.len()),
        Err(e) => eprintln!("RUST_STORE_FAIL|{e}"),
    }

    // 4. Store avec le token mis en cache AVANT la rotation Python —
    // le secret ayant produit ce token a ete evince : le store doit
    // etre rejete silencieusement (timeout de la requete).
    if let (Some(addr_s), Some(kf)) = (py_addr_s, py_key_file) {
        let hex_key = std::fs::read_to_string(kf).unwrap().trim().to_string();
        let py_key = hex::decode(hex_key).unwrap();
        let py_addr = UdpAddress::from(addr_s.parse::<SocketAddr>().unwrap());
        let mid = sha1(&py_key);
        let py_node = Node::new(py_key, mid, py_addr);

        match community
            .store_on_nodes(
                &K_RUST2,
                vec![b"rust-value-2".to_vec()],
                std::slice::from_ref(&py_node),
            )
            .await
        {
            Ok(_) => eprintln!("RUST_STALE_ACCEPTED"),
            Err(e) => eprintln!("RUST_STALE_REJECTED|{e}"),
        }

        // 5. Re-find -> token frais post-rotation -> store accepte.
        let _ = community.find_nodes(&K_RUST2).await;
        match community
            .store_on_nodes(&K_RUST2, vec![b"rust-value-2".to_vec()], &[py_node])
            .await
        {
            Ok(n) => eprintln!("RUST_REFRESHED_STORE_OK|{}", n.len()),
            Err(e) => eprintln!("RUST_REFRESHED_STORE_FAIL|{e}"),
        }
    }

    // 6. Sert les requetes Python (find/store) jusqu'a l'echeance.
    let remain = deadline.saturating_duration_since(Instant::now());
    tokio::time::sleep(remain).await;
}
