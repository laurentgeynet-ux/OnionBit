// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Invariants de socket du mode stealth (ADR-0017, etape 53) :
//!
//! - `stealth.enabled` × `ipv8.enabled` refuse fermement, sur chaque
//!   chemin de demarrage ;
//! - validation stricte : role inconnu, lien mal forme, client sans
//!   pont → `Err` ;
//! - overlays legacy absents (`discovery`, `dht`, `content_discovery`)
//!   quand le transport morphe est actif ;
//! - **aucun marqueur legacy sur le fil** : le tap socket (frontiere
//!   UDP, octets morphes) ne doit jamais montrer `LibNaCLPK:`, un
//!   prefixe de communaute connu, ni le plaintext applicatif ;
//! - `anon_hops = 0` refuse en client/pont, autorise en gateway.

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use onionbit_core::daemon_config::StealthFileConfig;
use onionbit_core::{CoreConfig, CoreSession, Notifier};
use onionbit_ipv8::endpoint::TapDir;
use onionbit_ipv8::packet::PREFIX_LEN;
use onionbit_ipv8::stealth_transport::BRIDGE_LINK_SCHEME;
use onionbit_ipv8::UdpAddress;

/// Config de session stealth loopback (offline + transport morphe).
fn stealth_cfg(state_dir: std::path::PathBuf, role: &str, bridges: Vec<String>) -> CoreConfig {
    let mut cfg = CoreConfig::offline(state_dir);
    cfg.ipv8.enabled = false;
    cfg.ipv8.stealth = Some(StealthFileConfig {
        enabled: true,
        role: role.into(),
        bridges,
        ..Default::default()
    });
    cfg
}

/// `stealth.enabled` × `ipv8.enabled` → refus ferme (fail closed).
#[tokio::test]
async fn stealth_x_ipv8_legacy_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dir.path().to_path_buf());
    cfg.ipv8.enabled = true;
    cfg.ipv8.stealth = Some(StealthFileConfig {
        enabled: true,
        role: "client".into(),
        bridges: vec!["onionbit-bridge://127.0.0.1:1#0000000000000000000000000000000000000000000000000000000000000000".into()],
        ..Default::default()
    });
    let r = CoreSession::start_offline(cfg, Notifier::new()).await;
    assert!(r.is_err(), "la combinaison stealth × ipv8 doit echouer");
}

/// Validation stricte de la section `stealth` : tout est refuse au
/// demarrage, jamais degrade en silence.
#[tokio::test]
async fn stealth_validation_fail_closed() {
    let link_ok =
        "onionbit-bridge://127.0.0.1:1#0000000000000000000000000000000000000000000000000000000000000000"
            .to_string();
    type Mutate = Box<dyn Fn(&mut StealthFileConfig)>;
    let cases: Vec<(&str, Mutate)> = vec![
        ("role inconnu", Box::new(|sc| sc.role = "relai".into())),
        (
            "lien mal forme",
            Box::new(|sc| sc.bridges = vec!["onionbit-bridge://pas-une-addr".into()]),
        ),
        ("client sans pont", Box::new(|sc| sc.bridges = vec![])),
        (
            "allowlist non-hex",
            Box::new(|sc| sc.client_allowlist = vec!["zz".into()]),
        ),
    ];
    for (name, mutate) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = stealth_cfg(dir.path().to_path_buf(), "client", vec![link_ok.clone()]);
        let sc = cfg.ipv8.stealth.as_mut().unwrap();
        mutate(sc);
        let r = CoreSession::start_offline(cfg, Notifier::new()).await;
        assert!(r.is_err(), "config invalide acceptee : {name}");
    }
}

/// Le pont sert de serveur : son lien d'invitation porte sa pk
/// statique (persistee, jamais la cle maitresse).
#[tokio::test]
async fn stealth_pont_lien_invitation() {
    let dir = tempfile::tempdir().unwrap();
    let session = CoreSession::start_offline(
        stealth_cfg(dir.path().to_path_buf(), "bridge", vec![]),
        Notifier::new(),
    )
    .await
    .expect("start bridge");
    let stack = session.ipv8().expect("stack demarree en stealth");
    let transport = stack.stealth_transport.clone().expect("transport stealth");
    let link = transport.bridge_link().expect("lien de pont present");
    assert!(link.starts_with(BRIDGE_LINK_SCHEME));
    let entry = onionbit_ipv8::stealth_transport::BridgeEntry::parse_link(&link).unwrap();
    assert_eq!(
        entry.addr.port(),
        stack.endpoint.local_addr().unwrap().port()
    );
    // Redemarrage : le meme secret recharge — la pk du lien est stable.
    session.stop().await;
    let session2 = CoreSession::start_offline(
        stealth_cfg(dir.path().to_path_buf(), "bridge", vec![]),
        Notifier::new(),
    )
    .await
    .expect("restart bridge");
    let link2 = session2
        .ipv8()
        .unwrap()
        .stealth_transport
        .as_ref()
        .unwrap()
        .bridge_link()
        .unwrap();
    let entry2 = onionbit_ipv8::stealth_transport::BridgeEntry::parse_link(&link2).unwrap();
    assert_eq!(entry.pk, entry2.pk, "la pk de pont doit persister");
    session2.stop().await;
}

/// Oracle socket : tout ce qui sort de la socket du client stealth
/// est morphe — aucun marqueur `LibNaCLPK:`, prefixe de communaute,
/// `ez_send`, ni le plaintext applicatif lui-meme.
#[tokio::test]
async fn stealth_aucun_marqueur_legacy_sur_le_fil() {
    let dir_b = tempfile::tempdir().unwrap();
    let dir_c = tempfile::tempdir().unwrap();

    let bridge = CoreSession::start_offline(
        stealth_cfg(dir_b.path().to_path_buf(), "bridge", vec![]),
        Notifier::new(),
    )
    .await
    .expect("start bridge");
    let b_stack = bridge.ipv8().unwrap();
    let b_transport = b_stack.stealth_transport.clone().unwrap();
    // Le pont ecoute `0.0.0.0` — non dialable : le lien de test
    // pointe sur loopback (l'UI publique substituerait l'adresse WAN).
    let mut b_entry = onionbit_ipv8::stealth_transport::BridgeEntry::parse_link(
        &b_transport.bridge_link().unwrap(),
    )
    .unwrap();
    b_entry
        .addr
        .set_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let link = b_entry.to_link();
    let b_addr = b_entry.addr;

    // Listener brut cote pont : reçoit le plaintext *demorphe* —
    // preuve que la chaine applicative traverse le transport.
    let got_plaintext = Arc::new(AtomicBool::new(false));
    let marker = [0xEEu8; PREFIX_LEN];
    {
        let flag = got_plaintext.clone();
        b_stack
            .endpoint
            .add_raw_prefix_listener(
                marker,
                Arc::new(move |src, data| {
                    let _ = src;
                    if data.windows(9).any(|w| w == b"marqueur!") {
                        flag.store(true, AtomicOrdering::Relaxed);
                    }
                    Ok(())
                }),
            )
            .await;
    }

    let client = CoreSession::start_offline(
        stealth_cfg(dir_c.path().to_path_buf(), "client", vec![link]),
        Notifier::new(),
    )
    .await
    .expect("start client");
    let c_stack = client.ipv8().unwrap();

    // Invariants de structure : aucun overlay legacy n'existe.
    assert!(c_stack.stealth_transport.is_some());
    assert!(c_stack.discovery.is_none(), "discovery cree en stealth");
    assert!(c_stack.dht.is_none(), "dht cree en stealth");
    assert!(
        c_stack.content_discovery.is_none(),
        "content_discovery cree en stealth"
    );

    // Tap = frontiere socket : les octets observes sont ce qui part
    // reellement sur le reseau (forme morphee).
    let mut tap = c_stack.endpoint.set_tap().await;

    // Activite applicative : datagramme brut vers le pont (prefixe
    // factice — peu importe, il traverse morphe).
    let mut payload = marker.to_vec();
    payload.extend_from_slice(b"marqueur!");
    c_stack
        .endpoint
        .send_to(&UdpAddress::from(b_addr), &payload)
        .await
        .unwrap();

    // Collecte pendant le handshake + flush applicatif.
    let mut wire: Vec<Vec<u8>> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), tap.recv()).await {
            Ok(Ok((TapDir::Tx, _addr, bytes))) => wire.push(bytes),
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) if got_plaintext.load(AtomicOrdering::Relaxed) => break,
            Err(_) => {}
        }
    }

    assert!(!wire.is_empty(), "aucun datagramme capte sur le tap");
    // Marqueurs interdits : prefixe IPv8 (`LibNaCLPK:`), communautes
    // connues, nom du protocole, et le plaintext applicatif.
    let forbidden: &[&[u8]] = &[
        b"LibNaCLPK:",
        b"LibNaCLSK:",
        b"marqueur!",
        &onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID,
        &onionbit_ipv8::dht::DHT_COMMUNITY_ID,
        &onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID,
    ];
    for (i, d) in wire.iter().enumerate() {
        for marker in forbidden {
            assert!(
                !d.windows(marker.len()).any(|w| w == *marker),
                "datagramme {i} : marqueur legacy en clair sur le fil ({marker:?})"
            );
        }
        // Une trame morphee fait au moins rep(32)+len(2)+tag(16).
        assert!(d.len() >= 48, "datagramme {i} trop petit pour etre morphe");
        assert!(
            d.len() <= onionbit_ipv8::stealth::STEALTH_MTU,
            "datagramme {i} au-dela de STEALTH_MTU"
        );
    }
    assert!(
        got_plaintext.load(AtomicOrdering::Relaxed),
        "le pont n'a jamais recu le plaintext demorphe"
    );
    client.stop().await;
    bridge.stop().await;
}

/// e2e ADR-0017 etape 54 : la session vers le pont se forme
/// proactivement (tick du transport), le hook `hs1` alimente
/// `Network`, le dialogue ext `hello`+`INTRO` circule morphe et le
/// client apprend des ponts supplementaires via le sink.
#[tokio::test]
async fn stealth_intro_decouverte_ponts() {
    let dir_b = tempfile::tempdir().unwrap();
    let dir_c = tempfile::tempdir().unwrap();

    let mut cfg_b = stealth_cfg(dir_b.path().to_path_buf(), "bridge", vec![]);
    cfg_b.ipv8.ext_enabled = true;
    let bridge = CoreSession::start_offline(cfg_b, Notifier::new())
        .await
        .expect("start bridge");
    let b_stack = bridge.ipv8().unwrap();
    let b_transport = b_stack.stealth_transport.clone().unwrap();
    let b_ext = b_stack.ext.clone().expect("ext actif en stealth");
    // Le pont connait un autre pont (graine) qu'il pourra annoncer.
    b_ext.seed_intro("10.77.0.1:8000".parse().unwrap(), [0x33; 32]);

    let mut b_entry = onionbit_ipv8::stealth_transport::BridgeEntry::parse_link(
        &b_transport.bridge_link().unwrap(),
    )
    .unwrap();
    b_entry
        .addr
        .set_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    let mut cfg_c = stealth_cfg(
        dir_c.path().to_path_buf(),
        "client",
        vec![b_entry.to_link()],
    );
    cfg_c.ipv8.ext_enabled = true;
    let client = CoreSession::start_offline(cfg_c, Notifier::new())
        .await
        .expect("start client");
    let c_stack = client.ipv8().unwrap();
    let c_transport = c_stack.stealth_transport.clone().unwrap();
    let c_ext = c_stack.ext.clone().expect("ext actif cote client");

    // 1. Session proactive : le tick du client emet `hs1` vers le
    //    pont configure — aucun trafic applicatif n'est requis.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while b_transport.session_count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "session jamais etablie (amorcage proactif casse)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // 2. Le hook a enregistre le client dans le `Network` du pont :
    //    le tick `hello` le sonde (l'intervalle par defaut est trop
    //    long pour le test — on force le tick).
    b_ext.hello_tick().await;
    // Sondes de progression (debug du chemin).
    eprintln!(
        "net_b={} extpeers_b={} extpeers_c={}",
        b_stack.network.verified_peers().len(),
        b_ext.ext_peer_count(),
        c_ext.ext_peer_count()
    );
    // 3. Le client marque le pont ext, repond `hello` puis enchaine
    //    `INTRO_REQ` ; le pont sert sa graine -> le client apprend le
    //    pont supplementaire via le sink (`bridge_count` 1 -> 2).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while c_transport.bridge_count() < 2 {
        eprintln!(
            "loop: net_b={} extpeers_b={} extpeers_c={} introd_c={}",
            b_stack.network.verified_peers().len(),
            b_ext.ext_peer_count(),
            c_ext.ext_peer_count(),
            c_ext.intro_table_len()
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "le client n'a appris aucun pont via INTRO"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Le `hello` a pu arriver avant que le client ne soit
        // visible cote pont — on re-tick tant que le dialogue n'a
        // pas eu lieu.
        b_ext.hello_tick().await;
    }
    assert!(c_ext.intro_table_len() >= 1);
    client.stop().await;
    bridge.stop().await;
}

/// `anon_hops = 0` refuse en client/pont, autorise en gateway.

#[tokio::test]
async fn stealth_moteur_direct_refuse_hors_gateway() {
    // Client : le moteur direct est refuse.
    let dir = tempfile::tempdir().unwrap();
    let link = "onionbit-bridge://127.0.0.1:1#0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let client = CoreSession::start_offline(
        stealth_cfg(dir.path().to_path_buf(), "client", vec![link]),
        Notifier::new(),
    )
    .await
    .expect("start client");
    assert!(
        client.engine_for(0).await.is_err(),
        "engine_for(0) doit etre refuse en stealth client"
    );
    client.stop().await;

    // Gateway : la sortie publique reste sa fonction.
    let dir = tempfile::tempdir().unwrap();
    let gw = CoreSession::start_offline(
        stealth_cfg(dir.path().to_path_buf(), "gateway", vec![]),
        Notifier::new(),
    )
    .await
    .expect("start gateway");
    assert!(
        gw.engine_for(0).await.is_ok(),
        "gateway garde son moteur direct"
    );
    gw.stop().await;
}
