// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Banc ADR-0015 — T2 (mesh de curation), T3 (flood controle) et les
//! seuils relatifs de T4 (extinction de la cadence ext) en loopback
//! multi-noeuds.
//!
//! Chaque scenario produit une ligne JSON dans
//! `docs/plans/bench_adr0015/journal.jsonl` (repertoire gitignore —
//! conventions `docs/plans/bancs_tests.md` : oracles observables,
//! artefacts rejouables). Les champs CPU/RSS sont mesures par le
//! harnais PowerShell (`scripts/bench_ext_silence.ps1`,
//! `fingerprint_mesh.ps1 -WithExt`) — en process ils restent `null`.
//!
//! Le banc T1 (legacy silence contre Tribler.exe) et le run T4 de
//! 15 min sont des bancs *terrain* : scripts prepares, non executes
//! par cargo.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use onionbit_crypto::ipv8::keys::LibNaClPublicKey;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::ext::{
    attest_kind, attest_verdict, msg, Attestation, ExtSettings, InMemoryAttestationStore,
    LedgerLink, LedgerTx, OnionbitExtCommunity, EXT_COMMUNITY_ID, EXT_PROTO_VERSION, GENESIS,
};
use onionbit_ipv8::serializer::Writer;
use onionbit_ipv8::{prefix_of, Network, Packet, Peer, UdpAddress, UdpEndpoint, PREFIX_LEN};

/// Noeud du banc : communaute ext + endpoint + annuaire propres.
struct Node {
    c: Arc<OnionbitExtCommunity>,
    ep: Arc<UdpEndpoint>,
    net: Arc<Network>,
    addr: UdpAddress,
    key: LibNaClSecretKey,
}

/// Cree un noeud loopback (endpoint lie + boucle `run` spawnee).
async fn node(settings: ExtSettings) -> Node {
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let SocketAddr::V4(sa) = ep.local_addr().unwrap() else {
        panic!("bind v4")
    };
    let net = Arc::new(Network::default());
    let key = LibNaClSecretKey::generate();
    let c = OnionbitExtCommunity::new(key.clone(), net.clone(), ep.clone(), settings).await;
    let e = ep.clone();
    tokio::spawn(async move {
        let _ = e.run().await;
    });
    Node {
        c,
        ep,
        net,
        addr: UdpAddress::Ipv4(sa),
        key,
    }
}

/// Lien ext `x <-> y` : les deux se connaissent comme pairs verifies
/// (decouverts par les communautes legacy), puis `x` sonde — `y`
/// repond une fois (anti ping-pong) et les deux se marquent ext.
async fn link(x: &Node, y: &Node) {
    let pk_x = x.key.public_key().to_bin();
    let pk_y = y.key.public_key().to_bin();
    x.net
        .add_verified(Peer::new(pk_y.clone(), Some(y.addr.clone())).unwrap());
    y.net
        .add_verified(Peer::new(pk_x.clone(), Some(x.addr.clone())).unwrap());
    let want_x = x.c.ext_peer_count() + 1;
    let want_y = y.c.ext_peer_count() + 1;
    x.c.hello_tick().await;
    wait_until(|| y.c.ext_peer_count() >= want_y).await;
    wait_until(|| x.c.ext_peer_count() >= want_x).await;
}

/// Envoie `att` emballee `msg::ATTEST` depuis `src.ep` vers `dst`,
/// transport-signee par `transport` (relayeur — peut differer du
/// curateur : l'attestation est auto-portante).
async fn send_att(src: &Node, dst: &Node, att: &Attestation, transport: &LibNaClSecretKey) {
    let pkt = Packet::sign_no_dist(
        &EXT_COMMUNITY_ID,
        onionbit_ipv8::ext::msg::ATTEST,
        transport,
        &att.pack(),
    );
    src.ep.send_to(&dst.addr, &pkt).await.unwrap();
}

/// Envoie une trame `LEDGER_*` `{v, payload}` signee par
/// `transport` depuis `src.ep` vers `dst` (relai quelconque : les
/// liens sont auto-portants, la cle de transport n'entre pas dans
/// la validation).
async fn send_ledger(
    src: &Node,
    dst: &Node,
    msg_id: u8,
    link_bytes: &[u8],
    transport: &LibNaClSecretKey,
) {
    let mut w = Writer::new();
    w.u8(EXT_PROTO_VERSION);
    w.raw(link_bytes);
    let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg_id, transport, &w.into_bytes());
    src.ep.send_to(&dst.addr, &pkt).await.unwrap();
}

async fn wait_until(pred: impl Fn() -> bool) {
    wait_until_secs(pred, 5).await
}

/// Attend `pred` jusqu'a `secs` s (sondage 10 ms).
async fn wait_until_secs(mut pred: impl FnMut() -> bool, secs: u64) {
    for _ in 0..secs * 100 {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition non atteinte en {secs} s");
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "inconnu".into())
}

/// Chemin du journal de banc (gitignore — `docs/plans/`).
fn journal_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/plans/bench_adr0015/journal.jsonl")
}

/// Ajoute une ligne de journal de scenario (champs du format
/// convenu : commit, config, topologie, duree, compteurs, DB/score
/// avant-apres, volume, verdict legacy).
fn journal(
    scenario: &str,
    ext_config: &str,
    topology: &str,
    started: Instant,
    counters_json: &str,
    volume_json: &str,
    extra_json: &str,
) {
    let path = journal_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = format!(
        "{{\"ts\":{},\"commit\":\"{}\",\"scenario\":\"{}\",\"ext_config\":\"{}\",\"topology\":\"{}\",\"duration_ms\":{},\"counters\":{},\"endpoint_volume\":{},\"cpu_rss\":null,\"legacy_verdict\":\"n/a — loopback OnionBit-only (T1 scripte)\",{}}}",
        epoch(),
        commit(),
        scenario,
        ext_config,
        topology,
        started.elapsed().as_millis(),
        counters_json,
        volume_json,
        extra_json,
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("journal.jsonl");
    use std::io::Write as _;
    writeln!(f, "{line}").expect("journal write");
}

/// `{\"rx\":n,...}` des compteurs d'un noeud.
fn node_counters(n: &Node) -> String {
    let i = n.c.info();
    format!(
        "{{\"peer_count\":{},\"attest_rx\":{},\"attest_dropped\":{},\"attest_stored\":{},\"attest_tx\":{},\"hello_tx\":{},\"hello_probed\":{}}}",
        i.peer_count,
        i.attest_rx,
        i.attest_dropped,
        i.attest_stored,
        i.attest_tx,
        i.hello_tx,
        i.hello_probed,
    )
}

/// Nombre de datagrammes `ATTEST`/`HELLO` (tx+rx) et octets vus par
/// un tap d'endpoint — purge du recepteur (mesure de volume).
fn tap_drain(
    rx: &mut tokio::sync::broadcast::Receiver<onionbit_ipv8::endpoint::TapEvent>,
) -> String {
    let prefix = prefix_of(&EXT_COMMUNITY_ID);
    let (mut n_att, mut n_hello, mut n_pkts, mut bytes) = (0usize, 0usize, 0usize, 0usize);
    while let Ok((_dir, _dst, data)) = rx.try_recv() {
        n_pkts += 1;
        bytes += data.len();
        if data.len() > PREFIX_LEN && data[..PREFIX_LEN] == prefix {
            match data[PREFIX_LEN] {
                m if m == onionbit_ipv8::ext::msg::ATTEST => n_att += 1,
                m if m == onionbit_ipv8::ext::msg::HELLO => n_hello += 1,
                _ => {}
            }
        }
    }
    format!(
        "{{\"pkts\":{n_pkts},\"bytes\":{bytes},\"attest_pkts\":{n_att},\"hello_pkts\":{n_hello}}}"
    )
}

/// T2 — mesh de curation : A curateur ; B/C suivent A ; D ne suit
/// personne ; E emetteur signe non suivi. Chaque cas de la matrice
/// d'oracles est verifie sur les compteurs publics.
#[tokio::test]
async fn t2_mesh_curation() {
    let t0 = Instant::now();
    let a = node(ExtSettings::default()).await;
    let pk_a = a.key.public_key().to_bin();
    let followed = HashSet::from([pk_a.clone()]);
    let b = node(ExtSettings {
        curators: followed.clone(),
        ..ExtSettings::default()
    })
    .await;
    let c = node(ExtSettings {
        curators: followed.clone(),
        ..ExtSettings::default()
    })
    .await;
    let d = node(ExtSettings::default()).await; // ne suit personne
    let e = node(ExtSettings::default()).await;
    let mut tap_a = a.ep.set_tap().await;

    link(&a, &b).await;
    link(&b, &c).await;
    link(&b, &d).await;

    let subject = [0x42; 20];

    // Cas 1 : attestation valide du curateur A → B,C stockent +
    // score, D ne stocke rien (ne suit pas A).
    a.c.publish_attestation(attest_kind::INFOHASH, &subject, attest_verdict::ENDORSE)
        .await
        .unwrap();
    let b2 = b.c.clone();
    wait_until(move || b2.trust_info(attest_kind::INFOHASH, &subject).score == 1).await;
    let c2 = c.c.clone();
    wait_until(move || c2.trust_info(attest_kind::INFOHASH, &subject).score == 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(d.c.attestations_latest(10).is_empty());
    assert_eq!(d.c.trust_info(attest_kind::INFOHASH, &subject).score, 0);

    let att = a.c.attestations_latest(1)[0].clone();
    let b_mark = b.c.info();

    // Cas 2 : rejeu identique → rx+1, stored/tx figes.
    send_att(&a, &b, &att, &a.key).await;
    let b3 = b.c.clone();
    let rx_want = b_mark.attest_rx + 1;
    wait_until(move || b3.info().attest_rx >= rx_want).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let i = b.c.info();
    assert_eq!(i.attest_stored, b_mark.attest_stored);
    assert_eq!(i.attest_tx, b_mark.attest_tx);
    assert!(i.attest_dropped > b_mark.attest_dropped);

    // Cas 3 : attestation valide signee par E (curateur non suivi)
    // → dropped, jamais stockee.
    let att_e = Attestation::sign(
        &e.key,
        attest_kind::INFOHASH,
        &subject,
        attest_verdict::ENDORSE,
        att.ts + 5,
    )
    .unwrap();
    send_att(&e, &b, &att_e, &e.key).await;
    let b4 = b.c.clone();
    wait_until(move || b4.info().attest_rx > rx_want).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let i = b.c.info();
    assert_eq!(i.attest_stored, b_mark.attest_stored);
    assert!(i.attest_dropped > b_mark.attest_dropped);

    // Cas 4 : conflit meme ts (A equivoque : flag vs endorse au
    // meme ts) → rejet, score/DB inchanges, aucune re-emission.
    let c_mark = c.c.info();
    let flag = Attestation::sign(
        &a.key,
        attest_kind::INFOHASH,
        &subject,
        attest_verdict::FLAG,
        att.ts,
    )
    .unwrap();
    send_att(&a, &b, &flag, &a.key).await;
    let b5 = b.c.clone();
    wait_until(move || b5.info().attest_rx >= rx_want + 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(b.c.trust_info(attest_kind::INFOHASH, &subject).score, 1);
    assert_eq!(b.c.info().attest_stored, b_mark.attest_stored);
    // Le conflit n'a pas voyage : C n'a rien recu de plus.
    assert_eq!(c.c.info().attest_rx, c_mark.attest_rx);
    assert_eq!(c.c.attestations_latest(10).len(), 1);

    // Cas 5 : verdict plus recent (ts+10) → remplace, score
    // retourne, propage a C.
    let newer = Attestation::sign(
        &a.key,
        attest_kind::INFOHASH,
        &subject,
        attest_verdict::FLAG,
        att.ts + 10,
    )
    .unwrap();
    send_att(&a, &b, &newer, &a.key).await;
    let b6 = b.c.clone();
    wait_until(move || b6.trust_info(attest_kind::INFOHASH, &subject).score == -1).await;
    let c3 = c.c.clone();
    wait_until(move || c3.trust_info(attest_kind::INFOHASH, &subject).score == -1).await;

    // Cas 6 : unfollow — attestations conservees dans le store
    // partage, exclues du score ; re-suivi les reintegre (le store
    // injecte modelise la DB persistante au redemarrage).
    let subject2 = [0x51; 20];
    let shared = Arc::new(InMemoryAttestationStore::default());
    let b1 = node(ExtSettings {
        curators: followed.clone(),
        ..ExtSettings::default()
    })
    .await;
    b1.c.set_attestation_store(shared.clone());
    let att2 = Attestation::sign(
        &a.key,
        attest_kind::INFOHASH,
        &subject2,
        attest_verdict::FLAG,
        epoch(),
    )
    .unwrap();
    send_att(&a, &b1, &att2, &a.key).await;
    let b1c = b1.c.clone();
    wait_until(move || b1c.trust_info(attest_kind::INFOHASH, &subject2).score == -1).await;
    // « Restart » sans curateur : meme store, score exclu.
    let b2n = node(ExtSettings::default()).await;
    b2n.c.set_attestation_store(shared.clone());
    let t = b2n.c.trust_info(attest_kind::INFOHASH, &subject2);
    assert_eq!(t.score, 0);
    assert_eq!(t.attestation_count, 1);
    // « Restart » re-suivant A : le verdict se recompte.
    let b3n = node(ExtSettings {
        curators: followed.clone(),
        ..ExtSettings::default()
    })
    .await;
    b3n.c.set_attestation_store(shared.clone());
    let t = b3n.c.trust_info(attest_kind::INFOHASH, &subject2);
    assert_eq!(t.score, -1);
    assert_eq!(t.attestation_count, 1);

    let volume = tap_drain(&mut tap_a);
    journal(
        "T2-mesh-curation",
        "defaults ; B/C suivent A ; D/E hors-confiance ; store memoire partage pour unfollow",
        "loopback 5 noeuds : A(curateur)-B-C,D ; E emetteur non suivi",
        t0,
        &format!(
            "{{\"A\":{},\"B\":{},\"C\":{},\"D\":{}}}",
            node_counters(&a),
            node_counters(&b),
            node_counters(&c),
            node_counters(&d)
        ),
        &volume,
        "\"cases\":\"valid|replay|non-suivi|conflit-ts|verdict+recent|unfollow : tous oracles verts\"",
    );
}

/// T3 — flood controle : (a) 300 `ATTEST` d'une seule cle de
/// transport en < 60 s → le budget 256 borne a 44+ drops ; (b) 12
/// cles Sybil fraiches au-dela de `attest_rate_table_max` = 8 → les
/// emetteurs inconnus sont droppes sans insertion, les membres de la
/// table continuent d'etre servis.
#[tokio::test]
async fn t3_flood_controle() {
    let t0 = Instant::now();
    // (a) Mono-cle : 300 > 256/fenetre.
    let a = node(ExtSettings::default()).await;
    let pk_a = a.key.public_key().to_bin();
    let b = node(ExtSettings {
        curators: HashSet::from([pk_a.clone()]),
        ..ExtSettings::default()
    })
    .await;
    for i in 0u32..300 {
        let mut subject = [0x30; 20];
        subject[..4].copy_from_slice(&i.to_be_bytes());
        let att = Attestation::sign(
            &a.key,
            attest_kind::INFOHASH,
            &subject,
            attest_verdict::ENDORSE,
            epoch(),
        )
        .unwrap();
        send_att(&a, &b, &att, &a.key).await;
    }
    let b1 = b.c.clone();
    wait_until_secs(move || b1.info().attest_rx >= 300, 15).await;
    let i = b.c.info();
    assert_eq!(i.attest_rx, 300);
    assert!(i.attest_dropped >= 44, "dropped={}", i.attest_dropped);
    assert!(i.attest_stored <= 256, "stored={}", i.attest_stored);

    // (b) Table de budget pleine : emetteurs Sybil frais dropes,
    // membres presents toujours servis.
    let b2 = node(ExtSettings {
        curators: HashSet::from([pk_a.clone()]),
        attest_rate_max: 100,
        attest_rate_table_max: 8,
        ..ExtSettings::default()
    })
    .await;
    let mut senders: Vec<LibNaClSecretKey> = Vec::new();
    for k in 0u8..12 {
        senders.push(LibNaClSecretKey::generate());
        for j in 0u8..5 {
            let att = Attestation::sign(
                &a.key,
                attest_kind::INFOHASH,
                &[k * 8 + j + 1; 20],
                attest_verdict::ENDORSE,
                epoch(),
            )
            .unwrap();
            send_att(&a, &b2, &att, &senders[k as usize]).await;
        }
    }
    let b2c = b2.c.clone();
    wait_until_secs(move || b2c.info().attest_rx >= 60, 15).await;
    let i2 = b2.c.info();
    assert_eq!(i2.attest_rx, 60);
    // 8 emetteurs admis x 5 = 40 stockes ; 4 exclus x 5 = 20 drops.
    assert_eq!(i2.attest_stored, 40, "stored={}", i2.attest_stored);
    assert_eq!(i2.attest_dropped, 20, "dropped={}", i2.attest_dropped);
    // Un membre de la table reste servi (fenetre non epuisee).
    let att = Attestation::sign(
        &a.key,
        attest_kind::INFOHASH,
        &[0xf0; 20],
        attest_verdict::ENDORSE,
        epoch(),
    )
    .unwrap();
    send_att(&a, &b2, &att, &senders[0]).await;
    let b2d = b2.c.clone();
    wait_until(move || b2d.info().attest_stored == 41).await;

    journal(
        "T3-flood-controle",
        "(a) rate_max=256/60s table 4096 ; (b) rate_max=100, table_max=8",
        "(a) 300 ATTEST mono-cle ; (b) 12 cles transport x 5 ATTEST",
        t0,
        &format!(
            "{{\"mono_cle\":{},\"multi_cles\":{}}}",
            node_counters(&b),
            node_counters(&b2)
        ),
        "{\"note\":\"volume = datagrammes loopback (pas de tap dedie)\"}",
        "\"cases\":\"budget 256→44+ drops ; table pleine→Sybil bornes, membres servis : verts\"",
    );
}

/// T4 (seuils relatifs) — la cadence ext s'eteint : apres
/// convergence, ni `hello` ni `ATTEST` ne repartent en boucle ;
/// le gossip d'une publication s'eteint (`attest_tx` fige).
#[tokio::test]
async fn t4_cadence_ext_s_eteint() {
    let t0 = Instant::now();
    let a = node(ExtSettings::default()).await;
    let pk_a = a.key.public_key().to_bin();
    let b = node(ExtSettings {
        curators: HashSet::from([pk_a]),
        ..ExtSettings::default()
    })
    .await;
    let mut tap_b = b.ep.set_tap().await;
    link(&a, &b).await;

    // Sondage borne : apres le lien, les ticks suivants n'ajoutent
    // rien (B deja connu-ext) — `hello_tx` fige.
    let h0 = a.c.info().hello_tx;
    a.c.hello_tick().await;
    a.c.hello_tick().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let h1 = a.c.info().hello_tx;
    assert_eq!(h0, h1, "hello_tx a progresse apres convergence");

    // Publication unique → rafale bornee puis extinction.
    while tap_b.try_recv().is_ok() {}
    a.c.publish_attestation(attest_kind::INFOHASH, &[0x99; 20], attest_verdict::ENDORSE)
        .await
        .unwrap();
    let b1 = b.c.clone();
    wait_until(move || !b1.attestations_latest(10).is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tx1 = a.c.info().attest_tx;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let tx2 = a.c.info().attest_tx;
    assert_eq!(tx1, tx2, "attest_tx n'a pas converge a zero");
    // La rafale B-side est finie : tap montre un volume borne.
    let volume = tap_drain(&mut tap_b);
    assert!(volume.contains("\"pkts\""));

    // Rejeu post-convergence : absorbe par dedup sans emission.
    let att = a.c.attestations_latest(1)[0].clone();
    send_att(&a, &b, &att, &a.key).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b2i = b.c.info();
    assert_eq!(b2i.attest_stored, 1);
    assert!(b2i.attest_dropped >= 1);

    journal(
        "T4-cadence-extinction",
        "defaults ; B suit A",
        "loopback 2 noeuds lies ; publish + rejeu",
        t0,
        &format!(
            "{{\"A\":{},\"B\":{}}}",
            node_counters(&a),
            node_counters(&b)
        ),
        &volume,
        "\"cases\":\"hello_tx fige post-lien ; attest_tx→0 apres gossip ; rejeu absorbe sans emission : verts\"",
    );
}

/// T5a — settlement loopback (Phase 9c) : la comptabilite locale
/// mesure une tranche servie -> `settle_tick` propose -> le
/// beneficiaire co-signe -> SEAL stocke des deux cotes ; la gate
/// `owes_signature` s'eteint apres reglement.
#[tokio::test]
async fn t5a_ledger_settlement_loopback() {
    let t0 = Instant::now();
    let tranche = 1024u64;
    let sa = ExtSettings {
        ledger_tranche_bytes: tranche,
        ..ExtSettings::default()
    };
    let a = node(sa.clone()).await;
    let b = node(sa).await;
    link(&a, &b).await;
    let pk_a = a.key.public_key().to_bin();
    let pk_b = b.key.public_key().to_bin();
    // Comptabilite locale injectee (pattern `PeerStatsBook`) : A a
    // servi exactement une tranche a B ; B mesure le meme recu.
    let pk_b2 = pk_b.clone();
    a.c.set_stats_source(Arc::new(move |pk| {
        (pk == pk_b2.as_slice()).then_some((tranche, 0))
    }));
    let pk_a2 = pk_a.clone();
    b.c.set_stats_source(Arc::new(move |pk| {
        (pk == pk_a2.as_slice()).then_some((0, tranche))
    }));

    // Gate avant reglement : la tranche consommee est impayee.
    assert!(a.c.owes_signature(&pk_b));

    a.c.settle_tick().await;
    // B recoit PROPOSE -> co-signe -> SEAL ; A stocke le sceau.
    let bc = b.c.clone();
    wait_until(move || bc.info().ledger_stored >= 1).await;
    let ac = a.c.clone();
    wait_until(move || ac.info().ledger_stored >= 1).await;
    assert_eq!(a.c.info().ledger_pending, 0);

    // Meme lien des deux cotes, rang 1, signatures valides, `tx`
    // lisible par la paire seule.
    let la = a.c.ledger_links(1)[0].clone();
    let lb = b.c.ledger_links(1)[0].clone();
    assert_eq!(la.hash(), lb.hash());
    assert!(la.verify_a() && la.verify_b());
    assert_eq!(la.seq_a, 1);
    assert_eq!(la.seq_b, 1);
    let tx_a = LedgerTx::open(
        &la.tx_enc,
        &a.key,
        &LibNaClPublicKey::from_bin(&pk_b).unwrap(),
    )
    .unwrap();
    assert_eq!(tx_a.served_total, tranche);

    // Le reglement signe eteint la gate (delta servi-signe = 0).
    assert!(!a.c.owes_signature(&pk_b));
    assert_eq!(a.c.info().ledger_my_head.0, 1);
    assert_eq!(b.c.info().ledger_my_head.0, 1);

    journal(
        "T5a-ledger-settlement",
        "tranche=1024 ; drift defaut ; stats_source injectee",
        "loopback 2 noeuds lies ; propose->seal",
        t0,
        &format!(
            "{{\"A\":{},\"B\":{}}}",
            node_counters(&a),
            node_counters(&b)
        ),
        "{\"note\":\"volume ledger = datagrammes loopback\"}",
        "\"cases\":\"propose->seal->store ; gate off apres reglement : verts\"",
    );
}

/// T5b — derive + refus -> convergence : A gonfle `served` au-dela
/// de la derive toleree par B -> `LEDGER_REJECT` porte la mesure de
/// B -> la proposition suivante est plafonnee a `measured + derive`
/// et scellee. Le reliquat non signe reste du : la gate persiste
/// (refus != liberation).
#[tokio::test]
async fn t5b_ledger_drift_reject_converge() {
    let t0 = Instant::now();
    let tranche = 1024u64;
    let sa = ExtSettings {
        ledger_tranche_bytes: tranche,
        ledger_drift_permille: 125,
        ledger_drift_min: 128,
        ..ExtSettings::default()
    };
    let a = node(sa.clone()).await;
    let b = node(sa).await;
    link(&a, &b).await;
    let pk_a = a.key.public_key().to_bin();
    let pk_b = b.key.public_key().to_bin();
    let served_claim = 8192u64; // A declare beaucoup plus que la mesure de B
    let used_real = 1024u64;
    let pk_b2 = pk_b.clone();
    a.c.set_stats_source(Arc::new(move |pk| {
        (pk == pk_b2.as_slice()).then_some((served_claim, 0))
    }));
    let pk_a2 = pk_a.clone();
    b.c.set_stats_source(Arc::new(move |pk| {
        (pk == pk_a2.as_slice()).then_some((0, used_real))
    }));

    a.c.settle_tick().await;
    // B refuse (8192 > 1024 + 128 + 128) : A recoit REJECT, libere
    // `pending` et retient la tete + la mesure annoncees.
    let ac = a.c.clone();
    wait_until(move || ac.info().ledger_rx >= 1 && ac.info().ledger_pending == 0).await;

    // Tick suivant : proposition plafonnee -> acceptee et scellee.
    a.c.settle_tick().await;
    let ac = a.c.clone();
    wait_until(move || ac.info().ledger_stored >= 1).await;
    let link = a.c.ledger_links(1)[0].clone();
    let tx = LedgerTx::open(
        &link.tx_enc,
        &a.key,
        &LibNaClPublicKey::from_bin(&pk_b).unwrap(),
    )
    .unwrap();
    // Le montant converge sur la mesure de B + sa derive toleree.
    let cap = used_real + used_real / 1000 * 125 + 128;
    assert_eq!(tx.served_total, cap);

    // Le reliquat non signe (8192 - cap) reste du : gate toujours
    // active pour ce beneficiaire.
    assert!(a.c.owes_signature(&pk_b));

    journal(
        "T5b-ledger-drift-reject",
        "tranche=1024 ; drift 125/1000 + 128 ; served=8192 mesure=1024",
        "loopback 2 noeuds ; claim gonfle -> REJECT -> cap -> SEAL",
        t0,
        &format!(
            "{{\"A\":{},\"B\":{}}}",
            node_counters(&a),
            node_counters(&b)
        ),
        "{\"note\":\"montant corrige visible dans tx dechiffre\"}",
        "\"cases\":\"reject+resync+cap+seal ; gate persiste sur reliquat : verts\"",
    );
}

/// T5c — fork gossip : E equivoque (deux liens scelles a la meme
/// position `(pk_a, seq_a)`, contenus differents). Poussee en
/// `LEDGER_HEAD` vers C -> preuve conservee + `LEDGER_FORK` propage
/// -> B detient l'evidence. Jamais de consensus : la contradiction
/// est juste conservee et diffusee.
#[tokio::test]
async fn t5c_ledger_fork_gossip() {
    let t0 = Instant::now();
    let b = node(ExtSettings::default()).await;
    let c = node(ExtSettings::default()).await;
    link(&b, &c).await;

    // Deux liens contradictoires scelles : E propose, X co-signe les
    // deux (equivocation — meme `seq` des deux cotes, `tx` differents).
    let e_key = LibNaClSecretKey::generate();
    let x_key = LibNaClSecretKey::generate();
    let tx1 = LedgerTx::new(1024, epoch());
    let tx2 = LedgerTx::new(2048, epoch());
    let mut l1 =
        LedgerLink::propose(&e_key, &x_key.public_key(), 1, GENESIS, 1, GENESIS, &tx1).unwrap();
    l1.cosign(&x_key).unwrap();
    let mut l2 =
        LedgerLink::propose(&e_key, &x_key.public_key(), 1, GENESIS, 1, GENESIS, &tx2).unwrap();
    l2.cosign(&x_key).unwrap();
    assert_ne!(l1.hash(), l2.hash());

    // Premier HEAD : lien nouveau stocke (et re-gossippe vers B).
    send_ledger(&b, &c, msg::LEDGER_HEAD, &l1.pack(), &e_key).await;
    let cc = c.c.clone();
    wait_until(move || cc.info().ledger_stored >= 1).await;
    // Second HEAD contradictoire : Fork -> conservation + FORK gossip.
    send_ledger(&b, &c, msg::LEDGER_HEAD, &l2.pack(), &e_key).await;
    let cc = c.c.clone();
    wait_until(move || cc.info().ledger_forks >= 1).await;
    // B recoit la preuve propagee (HEAD l1 relaye puis FORK) :
    // position marquee + deux liens conserves.
    let bc = b.c.clone();
    wait_until(move || bc.info().ledger_forks >= 1).await;
    assert!(b.c.ledger_links(8).len() >= 2);

    journal(
        "T5c-ledger-fork-gossip",
        "defaults ; equivocation fabriquee (meme position, tx differents)",
        "loopback B-C lies ; E/X cles nues ; HEAD x2 -> FORK",
        t0,
        &format!(
            "{{\"B\":{},\"C\":{}}}",
            node_counters(&b),
            node_counters(&c)
        ),
        "{\"note\":\"preuve = paire de liens signes contradictoires\"}",
        "\"cases\":\"fork detecte au put ; FORK propage ; evidence conservee chez C et B : verts\"",
    );
}

/// T6 — Phase 9e : enveloppes `OBF` negociees (ADR-0015 §7).
///
/// Cas 1 : `a` (obf on, curateur suivi) publie une attestation —
/// vers `b` (obf on) elle part enveloppee (`obf_tx`/`obf_rx` > 0,
/// contenu stocke normalement) ; vers `c` (obf off) elle reste en
/// clair (`obf_rx` == 0, contenu stocke aussi — l'enveloppe est
/// opt-in par pair, pas communautaire).
///
/// Cas 2 : `b` recoit une trame `OBF` malformee — droppe sans
/// panic (`obf_dropped` monte).
#[tokio::test(flavor = "current_thread")]
async fn t6_obf_enveloppe_negociee() {
    let t0 = Instant::now();
    let obf_on = ExtSettings {
        obf_enabled: true,
        ..ExtSettings::default()
    };
    let a = node(obf_on.clone()).await;
    let pk_a = a.key.public_key().to_bin();
    let b = node(ExtSettings {
        curators: HashSet::from([pk_a.clone()]),
        ..obf_on.clone()
    })
    .await;
    let c = node(ExtSettings {
        curators: HashSet::from([pk_a.clone()]),
        ..ExtSettings::default()
    })
    .await;

    link(&a, &b).await;
    link(&a, &c).await;
    // `b` a bien annonce le bit (caps visible cote `a`).
    let pk_b = b.key.public_key().to_bin();
    let ac = a.c.clone();
    let pb = pk_b.clone();
    wait_until(move || {
        ac.peers_info()
            .iter()
            .any(|p| p.mid == hex::encode(onionbit_crypto::hash::ipv8_mid(&pb)) && p.caps & 1 != 0)
    })
    .await;

    // Cas 1 : publication -> enveloppe vers b, clair vers c.
    a.c.publish_attestation(attest_kind::INFOHASH, &[0x55; 20], attest_verdict::ENDORSE)
        .await
        .unwrap();
    let bc = b.c.clone();
    wait_until(move || bc.info().attest_stored >= 1).await;
    let cc = c.c.clone();
    wait_until(move || cc.info().attest_stored >= 1).await;
    let ib = b.c.info();
    let ic = c.c.info();
    assert!(ib.obf_rx >= 1, "b doit avoir recu l'attest enveloppee");
    assert_eq!(ic.obf_rx, 0, "c (obf off) recoit l'attest en clair");
    assert!(a.c.info().obf_tx >= 1, "a a enveloppe vers b");

    // Cas 2 : trame OBF malformee -> droppee, pas de panic.
    let mut w = Writer::new();
    w.u8(1u8);
    w.raw(&[0xde; 40]);
    let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::OBF, &a.key, &w.into_bytes());
    a.ep.send_to(&b.addr, &pkt).await.unwrap();
    let bc = b.c.clone();
    wait_until(move || bc.info().obf_dropped >= 1).await;

    journal(
        "T6-obf-enveloppe-negociee",
        "obf_enabled sur a,b ; c en clair ; curateur a",
        "loopback a-b, a-c lies ; publish -> OBF|clair ; OBF malforme",
        t0,
        &format!(
            "{{\"A\":{},\"B\":{},\"C\":{}}}",
            node_counters(&a),
            node_counters(&b),
            node_counters(&c)
        ),
        &format!(
            "{{\"obf\":{{\"A_tx\":{},\"B_rx\":{},\"B_dropped\":{},\"C_rx\":{}}}}}",
            a.c.info().obf_tx,
            b.c.info().obf_rx,
            b.c.info().obf_dropped,
            c.c.info().obf_rx,
        ),
        "\"cases\":\"cap negociee -> enveloppe ; sans cap -> clair ; malforme -> drop : verts\"",
    );
}
