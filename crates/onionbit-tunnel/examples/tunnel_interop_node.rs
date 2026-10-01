// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Noeud d'interop tunnels : construit un circuit a 1 saut vers un
//! `TunnelCommunity` pyipv8 (relais+exit) puis envoie un datagramme
//! "uTP-compatible" vers l'echo UDP cote Python a travers la sortie.
//! L'echo revient par le circuit — cela valide create/created, la
//! crypto par couches et la sortie bidirectionnelle contre le vrai
//! code Python. Chaque datagramme brut est journalise en hex.
//!
//! Usage :
//!   tunnel_interop_node --port P --keyfile F --echo 127.0.0.1:E \
//!                       --duration S --log L
//! `--keyfile` contient `<pubkey_hex> <port>` ecrit par
//! `py_tunnel_node.py`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::{TapDir, UdpEndpoint};
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::PEER_FLAG_RELAY;

/// Delai max pour que le circuit devienne READY.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Intervalle de scrutation de l'etat du circuit.
const POLL: Duration = Duration::from_millis(20);
/// Delai max pour l'echo retour.
const ECHO_TIMEOUT: Duration = Duration::from_secs(10);

/// Datagramme conforme `DataChecker.could_be_utp` (type ST_SYN=4,
/// version 1, extension 0, >=20 octets) : sinon l'exit pyipv8 refuse de
/// le relayer (`is_allowed`).
fn utp_shaped_payload() -> Vec<u8> {
    // type_version | extension | conn_id | ts | ts_diff | wnd | seq | ack
    let mut p = vec![0x41, 0x00];
    p.extend_from_slice(&[0x12, 0x34]); // connection_id
    p.extend_from_slice(&[0, 0, 0, 1]); // timestamp_microseconds
    p.extend_from_slice(&[0; 4]); // timestamp_difference
    p.extend_from_slice(&[0; 4]); // wnd_size
    p.extend_from_slice(&[0, 1]); // seq_nr
    p.extend_from_slice(&[0; 2]); // ack_nr
    p
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut port = "0".to_string();
    let mut keyfile = None;
    let mut echo = None;
    let mut duration = 10u64;
    let mut log = "tunnel_interop_rust.log".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().unwrap(),
            "--keyfile" => keyfile = args.next(),
            "--echo" => echo = args.next(),
            "--duration" => duration = args.next().unwrap().parse().unwrap(),
            "--log" => log = args.next().unwrap(),
            _ => {}
        }
    }
    let keyfile = keyfile.expect("--keyfile requis");
    let echo = echo.expect("--echo requis");

    // Cle publique + port du noeud Python.
    let kf = std::fs::read_to_string(&keyfile).expect("lecture keyfile");
    let mut parts = kf.split_whitespace();
    let peer_pk_hex = parts.next().expect("pubkey hex");
    let peer_port: u16 = parts.next().expect("port").parse().unwrap();
    let peer_pk = hex::decode(peer_pk_hex).expect("pubkey hex invalide");
    let peer_addr: SocketAddr = format!("127.0.0.1:{peer_port}").parse().unwrap();
    let py_peer = Peer::new(peer_pk, Some(UdpAddress::from(peer_addr)))
        .expect("cle publique pyipv8 invalide");

    let ep = UdpEndpoint::bind(&format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let local = ep.local_addr().unwrap();

    // Tap -> journal hex (RX|TX|addr|hex).
    let mut tap_rx = ep.set_tap().await;
    let log_path = log.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::File::create(&log_path).await.unwrap();
        while let Ok((dir, addr, bytes)) = tap_rx.recv().await {
            let dir = match dir {
                TapDir::Rx => "RX",
                TapDir::Tx => "TX",
            };
            let line = format!("{}|{}|{}\n", dir, addr, hex::encode(&bytes));
            let _ = f.write_all(line.as_bytes()).await;
            let _ = f.flush().await;
        }
    });

    let net = Arc::new(Network::default());
    let key = LibNaClSecretKey::generate();
    let tunnel = TunnelCommunity::new(key, net.clone(), ep.clone(), PEER_FLAG_RELAY).await;
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    net.add_verified(py_peer.clone());

    eprintln!(
        "rust tunnel node sur {} -> peer python {}",
        local, peer_addr
    );

    // Circuit a 1 saut : le noeud Python devient exit (son
    // `join_circuit` cree un TunnelExitSocket).
    let cid = tunnel
        .create_circuit(1, &py_peer)
        .await
        .expect("create_circuit");

    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline && !tunnel.ready_circuits().contains(&cid) {
        tokio::time::sleep(POLL).await;
    }
    if !tunnel.ready_circuits().contains(&cid) {
        eprintln!("ECHEC : circuit {cid} jamais READY");
        std::process::exit(1);
    }
    eprintln!("circuit {cid} READY (create->created OK avec pyipv8)");

    // Donnee "uTP" vers l'echo Python a travers la sortie.
    let echo_addr = UdpAddress::from(echo.parse::<SocketAddr>().unwrap());
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    let payload = utp_shaped_payload();
    tunnel
        .send_data(cid, &echo_addr, &zero, &payload)
        .await
        .expect("send_data");

    let mut rx = tunnel.data_rx();
    let got = tokio::time::timeout(ECHO_TIMEOUT, rx.recv())
        .await
        .expect("pas d'echo retour")
        .expect("canal data");
    assert_eq!(got.data, payload, "echo modifie");
    eprintln!("echo recu via tunnel : {} octets OK", got.data.len());

    let remain = Duration::from_secs(duration)
        .saturating_sub(Instant::now().duration_since(deadline - READY_TIMEOUT));
    tokio::time::sleep(remain.min(Duration::from_secs(2))).await;
    eprintln!("INTEROP TUNNEL OK");
}
