//! Noeud d'interop discovery Rust pour l'etape 11 : prouve les
//! introductions new-style (234 -> 233) et les punctures
//! (250/232 -> 249/231) contre un vrai `DiscoveryCommunity` pyipv8,
//! avec decode + verification de signature des deux cotes.
//!
//! Choreographie (le Python est pilote par `py_discovery_node.py`,
//! cf. `scripts/interop_discovery.ps1`) :
//! - Python -> Rust : le Python envoie 234 (retries), 232 puis 250 ;
//!   le Rust repond 233, 231, 249 via ses handlers (compteurs
//!   verifies dans `RUST_RECV`).
//! - Rust -> Python :
//!   1. attend le pair Python (envoye par sa propre requete 234) ;
//!   2. `send_introduction_request` -> l'adresse etant marquee
//!      new-style (introduite), on emet un vrai 234 ; le Python
//!      repond 233 -> `RUST_NEW_INTRO_OK` ;
//!   3. `send_puncture_request(new_style=true)` -> 232 ; le Python
//!      repond 231 -> `RUST_NEW_PUNCTURE_OK` ;
//!   4. `send_puncture_request(new_style=false)` -> 250 ; le Python
//!      repond 249 -> `RUST_OLD_PUNCTURE_OK`.
//!
//! Chaque phase a son propre timeout (une phase en echec ne doit pas
//! consommer le budget des suivantes), puis le noeud sert les
//! requetes Python jusqu'a la fin.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tribler_crypto::ipv8::keys::LibNaClSecretKey;
use tribler_ipv8::{DiscoveryCommunity, Network, UdpAddress, UdpEndpoint};

/// Timeout par phase (le deadline global sert de garde-fou).
const PHASE_TIMEOUT: Duration = Duration::from_secs(4);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::DEBUG)
        .init();
    let mut port = "0".to_string();
    let mut py_addr_s = None;
    let mut duration = 16u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().unwrap(),
            "--py-addr" => py_addr_s = args.next(),
            "--duration" => duration = args.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }
    let py_addr: UdpAddress = py_addr_s.unwrap().parse::<SocketAddr>().unwrap().into();

    let ep = UdpEndpoint::bind(&format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    // Tap brut : chaque datagramme en hex pour `verify_packets.py`.
    let mut tap_rx = ep.set_tap().await;
    tokio::spawn(async move {
        while let Ok((dir, addr, bytes)) = tap_rx.recv().await {
            let d = match dir {
                tribler_ipv8::endpoint::TapDir::Rx => "RX",
                tribler_ipv8::endpoint::TapDir::Tx => "TX",
            };
            eprintln!("TAP|{d}|{addr}|{}", hex::encode(&bytes));
        }
    });
    let key = LibNaClSecretKey::generate();
    let lan = UdpAddress::from("127.0.0.1:0".parse::<SocketAddr>().unwrap());
    let community = DiscoveryCommunity::new(
        key,
        std::sync::Arc::new(Network::default()),
        ep.clone(),
        lan,
    )
    .await;
    {
        let ep = ep.clone();
        tokio::spawn(async move {
            let _ = ep.run().await;
        });
    }

    let deadline = Instant::now() + Duration::from_secs(duration);

    // Attend que le Python apparaisse (il envoie sa requete 234).
    let wait = Instant::now() + PHASE_TIMEOUT;
    while Instant::now() < wait
        && community
            .network()
            .get_verified_by_address(&py_addr)
            .is_none()
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    match community.network().get_verified_by_address(&py_addr) {
        Some(p) => eprintln!("RUST_SEES_PY|new_style={}", p.new_style_intro),
        None => eprintln!("RUST_SEES_PY_FAIL"),
    }

    // `is_new_style` n'est vrai que pour une adresse INTRODUITE avec
    // le flag (`_all_addresses` pyipv8 — un pair verifie directement
    // y est inscrit avec new_style=false) : on simule cette
    // introduction a 3 pairs pour exercer le chemin 234 reel.
    if let Some(py_peer) = community.network().get_verified_by_address(&py_addr) {
        community.network().discover_address(
            &py_peer,
            py_addr.clone(),
            Some(tribler_ipv8::DISCOVERY_COMMUNITY_ID),
            true,
        );
    }

    // Rust -> Python : vraie introduction new-style (234) via l'API.
    if community.send_introduction_request(&py_addr).await.is_ok() {
        let base = community.intro_response_count();
        let wait = (Instant::now() + PHASE_TIMEOUT).min(deadline);
        while Instant::now() < wait && community.intro_response_count() == base {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if community.intro_response_count() > base {
            eprintln!("RUST_NEW_INTRO_OK");
        } else {
            eprintln!("RUST_NEW_INTRO_FAIL|timeout");
        }
    } else {
        eprintln!("RUST_NEW_INTRO_FAIL|send");
    }

    // Rust -> Python : puncture-request new-style (232) -> 231 attendu.
    // Loopback : le WAN du Python partage notre IP -> la reponse vise
    // `lan_walker` (regle `on_puncture_request` pyipv8) ; on annonce
    // donc notre propre adresse pour les deux.
    let my = UdpAddress::from(format!("127.0.0.1:{port}").parse::<SocketAddr>().unwrap());
    let base_p = community.puncture_count();
    if community
        .send_puncture_request(&py_addr, &my, &my, true)
        .await
        .is_ok()
    {
        let wait = (Instant::now() + PHASE_TIMEOUT).min(deadline);
        while Instant::now() < wait && community.puncture_count() == base_p {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if community.puncture_count() > base_p {
            eprintln!("RUST_NEW_PUNCTURE_OK");
        } else {
            eprintln!("RUST_NEW_PUNCTURE_FAIL|timeout");
        }
    } else {
        eprintln!("RUST_NEW_PUNCTURE_FAIL|send");
    }

    // Rust -> Python : puncture-request old-style (250) -> 249 attendu.
    let base_p = community.puncture_count();
    if community
        .send_puncture_request(&py_addr, &my, &my, false)
        .await
        .is_ok()
    {
        let wait = (Instant::now() + PHASE_TIMEOUT).min(deadline);
        while Instant::now() < wait && community.puncture_count() == base_p {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if community.puncture_count() > base_p {
            eprintln!("RUST_OLD_PUNCTURE_OK");
        } else {
            eprintln!("RUST_OLD_PUNCTURE_FAIL|timeout");
        }
    } else {
        eprintln!("RUST_OLD_PUNCTURE_FAIL|send");
    }

    // Le Python envoie ses propres requetes apres ses pauses : on
    // reste en ecoute jusqu'a la deadline pour les servir (reponses
    // 233/231/249), puis on rapporte les compteurs de decode.
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!(
        "RUST_RECV|intro_req={}|intro_resp={}|punctures={}",
        community.intro_request_count(),
        community.intro_response_count(),
        community.puncture_count()
    );
}
