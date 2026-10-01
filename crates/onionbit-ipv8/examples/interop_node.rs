//! Noeud d'interop IPv8 (DiscoveryCommunity) pour le jalon
//! Rust↔pyipv8 : enregistre chaque datagramme brut (rx+tx) dans un
//! journal hex et envoie des introduction-request a une cible.
//!
//! Usage :
//! `interop_node --port N --target 127.0.0.1:P --duration S --log FILE`

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::discovery::DiscoveryCommunity;
use onionbit_ipv8::endpoint::{TapDir, UdpEndpoint};
use onionbit_ipv8::peer::Network;
use onionbit_ipv8::UdpAddress;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut port = "0".to_string();
    let mut target = None;
    let mut duration = 8u64;
    let mut log = "interop_rust.log".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().unwrap(),
            "--target" => target = args.next(),
            "--duration" => duration = args.next().unwrap().parse().unwrap(),
            "--log" => log = args.next().unwrap(),
            _ => {}
        }
    }

    let ep = UdpEndpoint::bind(&format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let local = ep.local_addr().unwrap();
    let lan = UdpAddress::from("127.0.0.1:0".parse::<SocketAddr>().unwrap());

    // Tap -> journal hex (RX|TX|addr|hex).
    let mut rx = ep.set_tap().await;
    let log_path = log.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::File::create(&log_path).await.unwrap();
        while let Ok((dir, addr, bytes)) = rx.recv().await {
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
    let community = DiscoveryCommunity::new(key, net.clone(), ep.clone(), lan).await;
    let ep2 = ep.clone();
    tokio::spawn(async move {
        let _ = ep2.run().await;
    });

    eprintln!("rust node sur {}", local);
    let deadline = Instant::now() + Duration::from_secs(duration);
    if let Some(t) = target {
        let target_addr = UdpAddress::from(t.parse::<SocketAddr>().unwrap());
        // Envoie des introduction-request tant que B n'est pas verifie.
        while Instant::now() < deadline {
            if net.get_verified_by_address(&target_addr).is_some() && community.peer_count() >= 2 {
                break;
            }
            let _ = community.send_introduction_request(&target_addr).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    // Laisse tourner pour recevoir les reponses/marches.
    let remain = deadline.saturating_duration_since(Instant::now());
    tokio::time::sleep(remain).await;
    eprintln!("peers verifies : {}", community.peer_count());
}
