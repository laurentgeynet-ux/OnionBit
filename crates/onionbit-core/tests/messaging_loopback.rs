// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests d'integration de la messagerie e2e (ADR-0011, banc `MS-1`)
//! en loopback : cycle complet presence -> point d'introduction ->
//! liaison e2e -> trames authentifiees dans les deux sens, sur de
//! vraies sockets UDP et des circuits reels.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_core::services::messaging::{MessagingEvent, MessagingService};
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_messaging::{messaging_hash, MessagingConfig, MsgKind};
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::{IntroductionPoint, PEER_FLAG_RELAY, PEER_SOURCE_PEX};
use onionbit_tunnel::settings::TunnelSettings;
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

/// Delai max d'attente d'un evenement de test.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Intervalle de scrutation.
const POLL: Duration = Duration::from_millis(20);

/// Noeud de test : endpoint + network + community tunnels.
struct Node {
    /// Identite.
    key: LibNaClSecretKey,
    /// Annuaire.
    network: Arc<Network>,
    /// Community tunnels.
    tunnel: Arc<TunnelCommunity>,
    /// Adresse d'ecoute.
    addr: SocketAddr,
}

/// Cree un noeud complet sur loopback.
async fn make_node(settings: TunnelSettings) -> Node {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    let tunnel = TunnelCommunity::new_with_id(
        key.clone(),
        network.clone(),
        ep,
        settings,
        TUNNEL_COMMUNITY_ID,
    )
    .await;
    Node {
        key,
        network,
        tunnel,
        addr,
    }
}

/// `Peer` correspondant au noeud (enregistre service tunnel).
fn peer_of(node: &Node) -> Peer {
    Peer::new(
        node.key.public_key().to_bin(),
        Some(UdpAddress::from(node.addr)),
    )
    .unwrap()
}

/// Enregistre `other` dans l'annuaire de `node` (pair verifie).
fn learn(node: &Node, other: &Node) {
    let p = peer_of(other);
    node.network.add_verified(p.clone());
    node.network
        .discover_service(&p.public_key_bin, TUNNEL_COMMUNITY_ID);
}

/// Attend qu'un circuit de `t` soit READY.
async fn wait_ready(t: &TunnelCommunity, cid: u32) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    while Instant::now() < deadline {
        if t.ready_circuits().contains(&cid) {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("circuit {cid} jamais READY");
}

/// Attend le prochain evenement satisfaisant `pred` (borne
/// `TEST_TIMEOUT`) et le retourne.
async fn wait_event(
    rx: &mut tokio::sync::broadcast::Receiver<MessagingEvent>,
    pred: impl Fn(&MessagingEvent) -> bool,
) -> MessagingEvent {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let Ok(ev) = tokio::time::timeout(POLL * 10, rx.recv()).await else {
            assert!(Instant::now() < deadline, "evenement jamais recu");
            continue;
        };
        if let Ok(ev) = ev {
            if pred(&ev) {
                return ev;
            }
        }
    }
}

/// MS-1 — cycle complet : B publie sa presence (IP sur `i` via le
/// moniteur du service), A la resout en PEX, lie un circuit e2e,
/// puis les trames `hello`/`msg` passent dans les deux sens,
/// authentifiees Ed25519 par la cle d'identite de chacun.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_cycle_complet_a_vers_b() {
    let relay = TunnelSettings {
        max_intro_points: 0,
        ..TunnelSettings::default()
    };
    let a = make_node(TunnelSettings {
        peer_flags: PEER_FLAG_RELAY,
        max_intro_points: 0,
        ..relay.clone()
    })
    .await;
    let i = make_node(relay.clone()).await;
    let r1 = make_node(relay.clone()).await;
    let r2 = make_node(relay.clone()).await;
    // B epingle son point d'introduction sur `i` : le moniteur de
    // presence du service le cree au premier tick.
    let i_addr_placeholder = i.addr;
    let b = make_node(TunnelSettings {
        peer_flags: PEER_FLAG_RELAY,
        intro_point_peer: Some(UdpAddress::from(i_addr_placeholder)),
        max_intro_points: 1,
        ..relay.clone()
    })
    .await;
    let nodes = [&a, &b, &i, &r1, &r2];
    for x in &nodes {
        for y in &nodes {
            if !std::ptr::eq(*x, *y) {
                learn(x, y);
            }
        }
    }
    // Config de test : re-annonce rapide (le premier tick du
    // moniteur pose immediatement le point d'introduction de B).
    let cfg = MessagingConfig {
        announce_interval: Duration::from_secs(1),
        ..Default::default()
    };
    let b_svc = MessagingService::start(b.tunnel.clone(), b.key.clone(), cfg.clone(), 0);
    let a_svc = MessagingService::start(a.tunnel.clone(), a.key.clone(), cfg, 1);
    let mut b_events = b_svc.subscribe();
    let mut a_events = a_svc.subscribe();

    // Le moniteur de B pose son IP sur `i` (attente de la
    // publication : `intro_point_for` peuple cote `i`).
    let b_pk = b.key.public_key().to_bin();
    let a_pk = a.key.public_key().to_bin();
    let mh_b = messaging_hash(&b.key.public_key());
    // Circuit de donnees 1 saut d'A (requis par `create-e2e`).
    let data_cid = a
        .tunnel
        .create_circuit(1, &peer_of(&r1))
        .await
        .expect("circuit data A");
    wait_ready(&a.tunnel, data_cid).await;

    // A resout l'IP de B en PEX via `i` (chemin `Some(target)` du
    // peers-request : pas de DHT en loopback).
    let intro_i = IntroductionPoint {
        address: UdpAddress::from(i.addr),
        peer_key: i.key.public_key().to_bin(),
        seeder_pk: b_pk.clone(),
        source: PEER_SOURCE_PEX,
        last_seen_secs: 0,
    };
    let mut intro_point = None;
    for _ in 0..50 {
        match a.tunnel.send_peers_request(mh_b, Some(&intro_i), 1).await {
            Ok(ips) if !ips.is_empty() => {
                intro_point = ips.into_iter().next();
                break;
            }
            _ => tokio::time::sleep(POLL * 10).await,
        }
    }
    let ip = intro_point.expect("point d'introduction de B decouvert");
    // Le seeder annonce est la cle d'identite de B — le handshake
    // e2e authentifie le destinataire au niveau transport.
    assert_eq!(ip.seeder_pk, b_pk);

    // Liaison e2e : `connected` attend le relais `e2e_ready`.
    let cid = a_svc
        .connect(&b_pk, &ip)
        .await
        .expect("liaison e2e messagerie");

    // A voit `Bound(B)` ; B voit `Pending` puis `Bound(A)` apres le
    // `hello` envoye par A au premier `send`.
    let ev = wait_event(
        &mut a_events,
        |e| matches!(e, MessagingEvent::Bound { contact, .. } if *contact == b_pk),
    )
    .await;
    assert!(matches!(ev, MessagingEvent::Bound { circuit_id, .. } if circuit_id == cid));

    a_svc
        .send(&b_pk, b"bonjour B".to_vec())
        .await
        .expect("envoi a->b");

    // B : l'expediteur s'identifie par le `hello` (Bound) puis le
    // message arrive authentifie.
    let ev = wait_event(
        &mut b_events,
        |e| matches!(e, MessagingEvent::Bound { contact, .. } if *contact == a_pk),
    )
    .await;
    assert!(matches!(ev, MessagingEvent::Bound { .. }));
    let ev = wait_event(&mut b_events, |e| {
        matches!(
            e,
            MessagingEvent::Frame {
                kind: MsgKind::Msg,
                ..
            }
        )
    })
    .await;
    match ev {
        MessagingEvent::Frame { contact, body, .. } => {
            assert_eq!(contact, a_pk);
            assert_eq!(body, b"bonjour B");
        }
        _ => unreachable!(),
    }

    // Sens retour : B repond sur le meme circuit e2e (repondant ->
    // initiateur).
    b_svc
        .send(&a_pk, b"recu A".to_vec())
        .await
        .expect("envoi b->a");
    let ev = wait_event(&mut a_events, |e| {
        matches!(
            e,
            MessagingEvent::Frame {
                kind: MsgKind::Msg,
                ..
            }
        )
    })
    .await;
    match ev {
        MessagingEvent::Frame { contact, body, .. } => {
            assert_eq!(contact, b_pk);
            assert_eq!(body, b"recu A");
        }
        _ => unreachable!(),
    }

    a_svc.stop();
    b_svc.stop();
}
