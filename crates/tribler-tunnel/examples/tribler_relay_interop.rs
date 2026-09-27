//! Interop contre le client Tribler installe (8.4.x) : circuit a 2
//! sauts `Rust A -> Tribler (relais) -> Rust B (sortie)` puis echo
//! UDP a travers le tunnel, plus l'echange de `peer_flags` par
//! `introduction-request`/`introduction-response` sur le prefixe
//! `TriblerTunnelCommunity` (`a3591a6b…d6bc` — distinct du prefixe
//! pyipv8 `81ded073…c9f3`).
//!
//! Tribler stocke (`tunnel_community/exitnode_enabled` non exposable
//! par configuration) `peer_flags = {RELAY, SPEED_TEST}` : il relaie
//! mais ne sort pas — le role de sortie revient donc a un second
//! noeud tunnel Rust dans le meme processus.
//!
//! Usage :
//!   tribler_relay_interop --tribler-pem <ec_multichain.pem> \
//!       --tribler-port P --echo 127.0.0.1:E --log L

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::endpoint::{TapDir, UdpEndpoint};
use tribler_ipv8::peer::{Network, Peer};
use tribler_ipv8::UdpAddress;
use tribler_tunnel::community::TunnelCommunity;
use tribler_tunnel::routing::{CIRCUIT_TYPE_DATA, PEER_FLAG_EXIT_BT, PEER_FLAG_RELAY};
use tribler_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID;

/// Delai max pour qu'un circuit devienne READY.
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// Intervalle de scrutation.
const POLL: Duration = Duration::from_millis(20);
/// Delai max pour l'echo retour.
const ECHO_TIMEOUT: Duration = Duration::from_secs(10);
/// Delai max pour apprendre les flags de Tribler par introduction.
const FLAGS_TIMEOUT: Duration = Duration::from_secs(10);

/// Datagramme conforme `DataChecker.could_be_utp` (type ST_SYN=4,
/// version 1, extension 0, >=20 octets).
fn utp_shaped_payload() -> Vec<u8> {
    let mut p = vec![0x41, 0x00];
    p.extend_from_slice(&[0x12, 0x34]); // connection_id
    p.extend_from_slice(&[0, 0, 0, 1]); // timestamp_microseconds
    p.extend_from_slice(&[0; 4]); // timestamp_difference
    p.extend_from_slice(&[0; 4]); // wnd_size
    p.extend_from_slice(&[0, 1]); // seq_nr
    p.extend_from_slice(&[0; 2]); // ack_nr
    p
}

/// Extrait `public_key_bin` (`LibNaCLPK:` + crypt_pk + vk) du fichier
/// PEM Tribler (`LibNaCLSK:` + crypt_sk(32) + seed(32)) — reutilise
/// `LibNaClSecretKey::from_bin` qui derive les deux publiques.
fn tribler_public_key(pem_path: &str) -> Vec<u8> {
    let raw = std::fs::read(pem_path).expect("lecture ec_multichain.pem");
    let marker = b"LibNaCLSK:";
    let pos = raw
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("marqueur LibNaCLSK absent du PEM");
    LibNaClSecretKey::from_bin(&raw[pos..])
        .expect("cle Tribler invalide")
        .public_key()
        .to_bin()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut pem = None;
    let mut tribler_port = 0u16;
    let mut echo = None;
    let mut log = "tribler_interop_rust.log".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tribler-pem" => pem = args.next(),
            "--tribler-port" => tribler_port = args.next().unwrap().parse().unwrap(),
            "--echo" => echo = args.next(),
            "--log" => log = args.next().unwrap(),
            _ => {}
        }
    }
    let pem = pem.expect("--tribler-pem requis");
    let echo = echo.expect("--echo requis");
    assert!(tribler_port > 0, "--tribler-port requis");

    let tribler_pk = tribler_public_key(&pem);
    let tribler_addr: SocketAddr = format!("127.0.0.1:{tribler_port}").parse().unwrap();
    let tribler_peer = Peer::new(tribler_pk.clone(), Some(UdpAddress::from(tribler_addr)))
        .expect("cle publique Tribler invalide");

    // Deux noeuds tunnel Rust sur le prefixe `TriblerTunnelCommunity` :
    // A (initiateur/relais) et B (sortie).
    let ep_a = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let ep_b = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr_a = ep_a.local_addr().unwrap();
    let addr_b = ep_b.local_addr().unwrap();

    let mut tap_rx = ep_a.set_tap().await;
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

    let net_a = Arc::new(Network::default());
    let net_b = Arc::new(Network::default());
    let key_a = LibNaClSecretKey::generate();
    let key_b = LibNaClSecretKey::generate();
    let tunnel_a = TunnelCommunity::new_with_id(
        key_a,
        net_a.clone(),
        ep_a.clone(),
        PEER_FLAG_RELAY,
        TRIBLER_TUNNEL_COMMUNITY_ID,
    )
    .await;
    let _tunnel_b = TunnelCommunity::new_with_id(
        key_b.clone(),
        net_b.clone(),
        ep_b.clone(),
        PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT,
        TRIBLER_TUNNEL_COMMUNITY_ID,
    )
    .await;
    for ep in [ep_a, ep_b] {
        tokio::spawn(async move {
            let _ = ep.run().await;
        });
    }

    net_a.add_verified(tribler_peer.clone());
    net_a.discover_service(&tribler_pk, TRIBLER_TUNNEL_COMMUNITY_ID);
    let b_peer = Peer::new(key_b.public_key().to_bin(), Some(UdpAddress::from(addr_b))).unwrap();
    net_a.add_verified(b_peer.clone());
    net_a.discover_service(&key_b.public_key().to_bin(), TRIBLER_TUNNEL_COMMUNITY_ID);

    eprintln!(
        "rust A {} | rust B {} | tribler {}",
        addr_a, addr_b, tribler_addr
    );

    // 1) Introduction sur le prefixe tunnel : Tribler doit repondre
    //    avec ses flags ({RELAY, SPEED_TEST} = 9 en bitmask).
    tunnel_a
        .send_introduction_request(&UdpAddress::from(tribler_addr))
        .await
        .expect("send_introduction_request");
    let deadline = Instant::now() + FLAGS_TIMEOUT;
    let mut tribler_flags = 0;
    while Instant::now() < deadline {
        tribler_flags = tunnel_a.peer_flags_of(&tribler_pk);
        if tribler_flags != 0 {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    if tribler_flags == 0 {
        eprintln!("ECHEC : pas d'introduction-response de Tribler (flags non appris)");
        std::process::exit(1);
    }
    eprintln!("flags Tribler appris par introduction : {tribler_flags}");
    assert_eq!(tribler_flags & PEER_FLAG_RELAY, PEER_FLAG_RELAY);

    // 2) Circuit a 2 sauts : premier hop = Tribler (relais), dernier
    //    hop = B (required_exit -> adresse resolue via `node_addr`).
    let cid = tunnel_a
        .create_circuit_typed(
            2,
            &tribler_peer,
            CIRCUIT_TYPE_DATA,
            Some(key_b.public_key().to_bin()),
            None,
        )
        .await
        .expect("create_circuit_typed");

    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline && !tunnel_a.ready_circuits().contains(&cid) {
        tokio::time::sleep(POLL).await;
    }
    if !tunnel_a.ready_circuits().contains(&cid) {
        eprintln!("ECHEC : circuit {cid} jamais READY (relais Tribler)");
        std::process::exit(1);
    }
    eprintln!("circuit {cid} READY via relais Tribler");

    // 3) Donnee uTP -> B sort -> echo -> retour par le tunnel.
    let echo_addr = UdpAddress::from(echo.parse::<SocketAddr>().unwrap());
    let zero = UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap());
    let payload = utp_shaped_payload();
    tunnel_a
        .send_data(cid, &echo_addr, &zero, &payload)
        .await
        .expect("send_data");

    let mut rx = tunnel_a.data_rx();
    let got = tokio::time::timeout(ECHO_TIMEOUT, rx.recv())
        .await
        .expect("pas d'echo retour via Tribler")
        .expect("canal data");
    assert_eq!(got.data, payload, "echo modifie");
    eprintln!(
        "echo recu via circuit Rust->Tribler->Rust : {} octets OK",
        got.data.len()
    );
    eprintln!("INTEROP TRIBLER OK");
}
