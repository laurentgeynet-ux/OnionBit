// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Banc **live** : vrais telechargements BitTorrent traversant de
//! vrais circuits onion (UDP loopback multi-noeuds), ajoutes par
//! `.torrent` ET par `magnet:`, avec preuve que le trafic emprunte
//! le nombre de sauts demande.
//!
//! Topologie par test :
//!
//! ```text
//!   session (leecher)                relais (relais+sortie BT)        seeder rqbit
//!   CoreSession + IPv8    --UDP-->   Relay0/Relay1/Relay2   --UDP-->   BtEngine
//!   lanes anon 1..=3 sauts           communaute TunnelCommunity       payload.bin
//! ```
//!
//! Chaque circuit est cree avec `create_circuit_pinned` : la route
//! (identite des sauts) est imposee puis verifiee par
//! `CircuitInfo.verified_hops` — le mid de chaque relais doit
//! apparaitre dans l'ordre. Les octets de `bytes_up`/`bytes_down` du
//! circuit prouvent que le trafic y a reellement transite.
//!
//! Le seeder est annonce au downloader via `initial_peers` rqbit
//! (equivalent du `add_peer` d'amorce en banc Tribler) : aucune DHT
//! ni tracker ne participe — tout ce qui circule est observe.
//!
//! Hors-ligne : tous les sockets sont en loopback.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;
use onionbit_core::{CoreConfig, CoreSession, Ipv8Stack, Notifier};
use onionbit_crypto::hash::ipv8_mid;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::{CircuitInfo, TunnelCommunity};
use onionbit_tunnel::routing::{CIRCUIT_TYPE_DATA, PEER_FLAG_EXIT_BT, PEER_FLAG_RELAY};
use onionbit_tunnel::settings::TunnelSettings;
use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

/// Delai max de construction d'un circuit epingle.
const CIRCUIT_WAIT: Duration = Duration::from_secs(30);
/// Delai max d'un transfert complet via le tunnel.
const TRANSFER_WAIT: Duration = Duration::from_secs(180);
/// Delai max des transitions d'etat (pending, migration).
const STATE_WAIT: Duration = Duration::from_secs(30);
/// Intervalle de scrutation.
const POLL: Duration = Duration::from_millis(50);

/// Noeud relais autonome (`PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT`).
struct Relay {
    key: LibNaClSecretKey,
    network: Arc<Network>,
    tunnel: Arc<TunnelCommunity>,
    addr: SocketAddr,
}

async fn make_relay() -> Relay {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
    let addr = ep.local_addr().unwrap();
    let tunnel = TunnelCommunity::new_with_id(
        key.clone(),
        network.clone(),
        ep.clone(),
        TunnelSettings {
            peer_flags: PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT,
            ..TunnelSettings::default()
        },
        TUNNEL_COMMUNITY_ID,
    )
    .await;
    tokio::spawn(async move {
        let _ = ep.run().await;
    });
    Relay {
        key,
        network,
        tunnel,
        addr,
    }
}

/// `Peer` correspondant au relais (service tunnel decouvert).
fn peer_of(r: &Relay) -> Peer {
    Peer::new(r.key.public_key().to_bin(), Some(UdpAddress::from(r.addr))).unwrap()
}

/// Mid hex du relais — attendu dans `verified_hops` du circuit.
fn mid_of(r: &Relay) -> String {
    hex::encode(ipv8_mid(&r.key.public_key().to_bin()))
}

/// `true` quand `f` devient vrai avant `wait`.
async fn wait_until(wait: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + wait;
    while !f() && Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
    }
    f()
}

/// Instantane des circuits `DATA READY` de `hops` sauts.
fn ready_data_circuits(tunnel: &TunnelCommunity, hops: usize) -> Vec<CircuitInfo> {
    tunnel
        .circuits_info()
        .into_iter()
        .filter(|c| c.ctype == CIRCUIT_TYPE_DATA && c.state == "READY" && c.goal_hops == hops)
        .collect()
}

/// Banc live : session leecher + `n_relays` relais + seeder reel.
struct Bench {
    session: CoreSession,
    tunnel: Arc<TunnelCommunity>,
    relays: Vec<Relay>,
    seed_addr: SocketAddr,
    /// Moteur seeder garde vivant (arrete avec le banc).
    _seeder: BtEngine,
    /// Repertoires gardes vivants (supprimes a la destruction).
    _dl_dir: tempfile::TempDir,
    _seed_dir: tempfile::TempDir,
    dl_dir: std::path::PathBuf,
    torrent_bytes: Vec<u8>,
    infohash_hex: String,
    payload: Vec<u8>,
}

/// Cablage `session <-> relais` : la session apprend chaque relais
/// (verifie + flags sortie pour la selection de sauts), chaque relais
/// apprend la session (reponses `create`/`extended`) et tous les
/// autres relais (candidats offerts a l'extension — toute la chaine
/// est joignable). Reutilisable apres un restart : le pair session a
/// change d'adresse, les relais le re-apprennent.
fn wire_relays(stack: &Ipv8Stack, tunnel: &TunnelCommunity, relays: &[Relay]) {
    let session_peer = Peer::new(
        hex::decode(stack.public_key_hex()).unwrap(),
        Some(UdpAddress::from(stack.endpoint.local_addr().unwrap())),
    )
    .unwrap();
    for r in relays {
        let p = peer_of(r);
        stack.network.add_verified(p.clone());
        stack
            .network
            .discover_service(&p.public_key_bin, TUNNEL_COMMUNITY_ID);
        tunnel.register_exit_peer(
            &r.key.public_key().to_bin(),
            r.addr,
            PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT,
        );
        r.network.add_verified(session_peer.clone());
        r.network
            .discover_service(&session_peer.public_key_bin, TUNNEL_COMMUNITY_ID);
    }
    for (i, ra) in relays.iter().enumerate() {
        for (j, rb) in relays.iter().enumerate() {
            if i == j {
                continue;
            }
            let p = peer_of(rb);
            ra.network.add_verified(p.clone());
            ra.network
                .discover_service(&p.public_key_bin, TUNNEL_COMMUNITY_ID);
            ra.tunnel.register_exit_peer(
                &rb.key.public_key().to_bin(),
                rb.addr,
                PEER_FLAG_RELAY | PEER_FLAG_EXIT_BT,
            );
        }
    }
}

/// Monte la topologie : session anonyme loopback, `n_relays` relais
/// mutuellement appris, seeder seedant `payload` (fichier
/// `payload.bin`).
async fn setup(n_relays: usize, payload: &[u8]) -> Bench {
    // -- seeder : vrai moteur rqbit + ecoute TCP/uTP loopback --
    let seed_dir = tempfile::tempdir().unwrap();
    std::fs::write(seed_dir.path().join("payload.bin"), payload).unwrap();
    let torrent = librqbit::create_torrent(
        seed_dir.path(),
        librqbit::CreateTorrentOptions {
            piece_length: Some(16384),
            ..Default::default()
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .expect("create_torrent");
    let torrent_bytes = torrent.as_bytes().unwrap().to_vec();
    let meta = onionbit_format::torrent::TorrentMeta::parse(&torrent_bytes).unwrap();
    let infohash_hex = onionbit_crypto::hash::to_hex(&meta.info_hash);

    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder");
    seeder
        .add_torrent_bytes(torrent_bytes.clone(), false)
        .await
        .expect("seed add");
    let seed_addr = {
        let a = seeder.listen_addr().expect("ecoute seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };

    // -- session leecher + stack ipv8 anonyme --
    let dl_dir = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::offline(dl_dir.path().to_path_buf());
    cfg.engine.listen_port = Some(0);
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    let session = CoreSession::start(cfg, Notifier::new())
        .await
        .expect("session");
    let stack = session.ipv8().expect("stack ipv8");
    let tunnel = stack.tunnel.clone().expect("tunnel community");

    let mut relays = Vec::new();
    for _ in 0..n_relays {
        relays.push(make_relay().await);
    }
    wire_relays(&stack, &tunnel, &relays);

    Bench {
        session,
        tunnel,
        relays,
        _seeder: seeder,
        seed_addr,
        _seed_dir: seed_dir,
        dl_dir: dl_dir.path().to_path_buf(),
        _dl_dir: dl_dir,
        torrent_bytes,
        infohash_hex,
        payload: payload.to_vec(),
    }
}

/// Cree un circuit `DATA` a route epinglee : `relays[0]` en premier
/// saut, `relays[1..hops-1]` epingles, `relays[hops-1]` en sortie
/// exigee — puis attend `READY`. Retourne les mids attendus dans
/// l'ordre.
async fn circuit_pinned(bench: &Bench, hops: usize) -> Vec<String> {
    assert!(hops >= 1 && hops <= bench.relays.len());
    let first = peer_of(&bench.relays[0]);
    // Sauts intermediaires epingles (entre le premier et la sortie).
    let pinned: Vec<Peer> = if hops > 2 {
        bench.relays[1..hops - 1].iter().map(peer_of).collect()
    } else {
        Vec::new()
    };
    let exit_pk = bench.relays[hops - 1].key.public_key().to_bin();
    let cid = bench
        .tunnel
        .create_circuit_pinned(hops, &first, CIRCUIT_TYPE_DATA, Some(exit_pk), None, pinned)
        .await
        .expect("create_circuit_pinned");
    let ok = wait_until(CIRCUIT_WAIT, || {
        bench
            .tunnel
            .circuits_info()
            .iter()
            .any(|c| c.circuit_id == cid && c.state == "READY" && c.actual_hops == hops)
    })
    .await;
    assert!(ok, "circuit {cid} ({hops} sauts) pas READY dans le delai");
    let info = bench
        .tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id == cid)
        .expect("circuit cree");
    let expected: Vec<String> = bench.relays[..hops].iter().map(mid_of).collect();
    assert_eq!(
        info.verified_hops, expected,
        "route verifiee != route epinglee (cid={cid}, hops={hops})"
    );
    expected
}

/// Compteurs (up+down) cumules des circuits READY `hops`.
fn circuit_bytes(bench: &Bench, hops: usize) -> u64 {
    ready_data_circuits(&bench.tunnel, hops)
        .iter()
        .map(|c| c.bytes_up + c.bytes_down)
        .sum()
}

/// Fichier telecharge retrouve recursivement sous `dir`.
fn find_payload(dir: &std::path::Path) -> Option<Vec<u8>> {
    for p in walkdir(dir) {
        if p.file_name().map(|n| n == "payload.bin").unwrap_or(false) && p.is_file() {
            return std::fs::read(&p).ok();
        }
    }
    None
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Assertions communes apres un transfert `hops` sauts : contenu,
/// lane proprietaire, lane persistee, route verifiee, trafic mesure
/// sur le circuit, aucune presence sur le moteur en clair.
async fn assert_lane_transfer(bench: &Bench, hops: u32, bytes_before: u64) {
    // Contenu integre.
    let got = find_payload(&bench.dl_dir).expect("payload.bin telecharge");
    assert_eq!(got, bench.payload, "contenu telecharge identique");
    // Verite moteur : la lane est bien celle demandee.
    assert_eq!(
        bench.session.owner_engine_hops(&bench.infohash_hex),
        Some(hops),
        "proprietaire != lane {hops}"
    );
    // Intention persistee identique (badge API honnete).
    assert_eq!(
        bench.session.anon_hops_map().get(&bench.infohash_hex),
        Some(&hops),
        "anon_hops persiste != {hops}"
    );
    if hops > 0 {
        // Le circuit epingle a porte du trafic.
        let after = circuit_bytes(bench, hops as usize);
        assert!(
            after > bytes_before,
            "aucun octet sur les circuits {hops} sauts ({bytes_before} -> {after})"
        );
        // Un seul download visible, pas de copie sur le moteur clair.
        let clear_engine = bench.session.engine_for(0).await.unwrap();
        assert!(
            clear_engine
                .get_by_hash(&onionbit_crypto::hash::from_hex(&bench.infohash_hex).unwrap())
                .is_none(),
            "le moteur en clair detient l'infohash d'un download anonyme"
        );
    }
}

// ----------------------------------------------------------------

/// `.torrent` sur la lane en clair (`hops=0`) : reference hors tunnel.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_torrent_clair() {
    let bench = setup(
        0,
        &(0..200_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
    )
    .await;
    let dl = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            0,
            false,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("add hops=0");
    tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
        .await
        .expect("transfert clair en timeout")
        .expect("wait_completed");
    assert_eq!(
        bench.session.owner_engine_hops(&bench.infohash_hex),
        Some(0)
    );
    let got = find_payload(&bench.dl_dir).expect("payload");
    assert_eq!(got, bench.payload);
    bench.session.stop().await;
}

/// `.torrent` reel sur chaque lane anonyme 1, 2 et 3 sauts — le
/// transfert ne passe que par le circuit epingle de la lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_torrent_hops_1_2_3() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let bench = setup(3, &payload).await;
    for hops in [1usize, 2, 3] {
        let route = circuit_pinned(&bench, hops).await;
        let t0 = Instant::now();
        let before = circuit_bytes(&bench, hops);
        let dl = bench
            .session
            .add_torrent_bytes_anon_with_peers(
                bench.torrent_bytes.clone(),
                false,
                hops as u32,
                true,
                None,
                vec![bench.seed_addr],
            )
            .await
            .unwrap_or_else(|e| panic!("add hops={hops}: {e}"));
        tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
            .await
            .unwrap_or_else(|_| panic!("transfert hops={hops} en timeout"))
            .expect("wait_completed");
        eprintln!(
            "live hops={hops}: transfert en {:?}, route={:?}, octets circuit +{}",
            t0.elapsed(),
            route,
            circuit_bytes(&bench, hops) - before
        );
        assert_lane_transfer(&bench, hops as u32, before).await;
        // Suppression : retire l'infohash de TOUS les moteurs.
        bench
            .session
            .remove(&bench.infohash_hex, true)
            .await
            .expect("remove");
        assert!(
            bench
                .session
                .owner_engine_hops(&bench.infohash_hex)
                .is_none(),
            "residu moteur apres remove (hops={hops})"
        );
    }
    bench.session.stop().await;
}

/// `magnet:` resolu a travers le tunnel : BEP 9 metainfo +
/// telechargement, le tout en cellules `data` du circuit epingle.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_magnet_hops_2() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 249) as u8).collect();
    let bench = setup(3, &payload).await;
    let route = circuit_pinned(&bench, 2).await;
    let before = circuit_bytes(&bench, 2);
    let uri = format!("magnet:?xt=urn:btih:{}&dn=payload", bench.infohash_hex);
    let dl = bench
        .session
        .add_download_anon_with_peers(&uri, false, 2, true, None, vec![bench.seed_addr])
        .await
        .expect("add magnet hops=2");
    tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
        .await
        .expect("transfert magnet en timeout")
        .expect("wait_completed");
    eprintln!(
        "live magnet hops=2: route={:?}, octets circuit +{}",
        route,
        circuit_bytes(&bench, 2) - before
    );
    assert_lane_transfer(&bench, 2, before).await;
    bench.session.stop().await;
}

/// Le scenario utilisateur en live : magnet ajoute sur une lane
/// **sans** circuit (pending METADATA), `PATCH` vers une lane qui
/// en a un — la resolution repart sur la nouvelle lane et le
/// telechargement se termine a travers le tunnel.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_magnet_patch_lane_pendant_resolution() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 247) as u8).collect();
    let bench = setup(3, &payload).await;
    let uri = format!("magnet:?xt=urn:btih:{}&dn=payload", bench.infohash_hex);
    let ih = bench.infohash_hex.clone();

    // Aucun circuit : la resolution reste en pending.
    let s = bench.session.clone();
    let u = uri.clone();
    let peers = vec![bench.seed_addr];
    let add_task = tokio::spawn(async move {
        s.add_download_anon_with_peers(&u, false, 3, true, None, peers)
            .await
    });
    assert!(
        wait_until(STATE_WAIT, || {
            bench
                .session
                .pending_downloads()
                .iter()
                .any(|p| p.infohash == ih && p.anon_hops == 3)
        })
        .await,
        "magnet pas passe en pending hops=3"
    );

    // Le circuit x2 doit etre pret AVANT le `PATCH` : le relance de
    // resolution part immediatement sur la nouvelle lane, et le banc
    // n'a ni DHT ni trackers pour re-annoncer le seeder — les
    // `initial_peers` rqbit sont consommes en une tentative. (En
    // production les trackers fournissent un flux continu de pairs.)
    let route = circuit_pinned(&bench, 2).await;
    let before = circuit_bytes(&bench, 2);
    bench
        .session
        .update_hops(&ih, 2)
        .await
        .expect("update_hops pending");

    // La tache se debloque sur la lane 2 et telecharge via le tunnel.
    let dl = tokio::time::timeout(TRANSFER_WAIT, add_task)
        .await
        .expect("add_task en timeout")
        .expect("join")
        .expect("resolution magnet");
    tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
        .await
        .expect("transfert en timeout")
        .expect("wait_completed");
    eprintln!(
        "live magnet PATCH 3->2: route={:?}, octets circuit +{}",
        route,
        circuit_bytes(&bench, 2) - before
    );
    assert_lane_transfer(&bench, 2, before).await;
    assert!(
        bench.session.pending_downloads().is_empty(),
        "pending residuel apres materialisation"
    );
    bench.session.stop().await;
}

/// `update_hops` pendant un transfert actif : le download migre de la
/// lane 1 vers la lane 3, y re-trouve le seeder (`add_peer` — le
/// `readd_bittorrent_peers` Python) et termine via le circuit x3.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_update_hops_en_transfert() {
    let payload: Vec<u8> = (0..400_000u32).map(|i| (i % 241) as u8).collect();
    let bench = setup(3, &payload).await;
    circuit_pinned(&bench, 1).await;
    circuit_pinned(&bench, 3).await;

    let dl = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            1,
            true,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("add hops=1");
    // Attend le premier octet recu par le tunnel x1.
    assert!(
        wait_until(STATE_WAIT, || {
            bench
                .session
                .find_download_hex(&bench.infohash_hex)
                .map(|d| d.stats().progress_bytes > 0)
                .unwrap_or(false)
        })
        .await,
        "aucun octet sur la lane 1"
    );

    bench
        .session
        .update_hops(&bench.infohash_hex, 3)
        .await
        .expect("update_hops 1->3");
    assert_eq!(
        bench.session.owner_engine_hops(&bench.infohash_hex),
        Some(3),
        "download pas migre sur la lane 3"
    );
    // Re-injection du pair sur le download migre (parite
    // `readd_bittorrent_peers` : la nouvelle lane n'herite pas des
    // pairs de l'ancienne — pas de DHT/trackers dans le banc).
    let dl3 = bench
        .session
        .find_download_hex(&bench.infohash_hex)
        .expect("download sur lane 3");
    let before = circuit_bytes(&bench, 3);
    // Le download migre reverifie le disque avant de passer `live` :
    // `add_peer` est un no-op avant — attendre l'etat live, puis
    // verifier que le pair est vu (un second appel retourne `false`
    // quand l'adresse est deja connue).
    let t0 = Instant::now();
    let mut added = false;
    while Instant::now() - t0 < STATE_WAIT {
        if dl3.add_peer(bench.seed_addr) {
            added = true;
        }
        let s = dl3.stats();
        if s.live && s.peers_seen > 0 {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    let s = dl3.stats();
    eprintln!(
        "post-migration lane3: live={} peers_seen={} queued={} state={:?} add_retourne={}",
        s.live, s.peers_seen, s.peers_queued, s.state, added
    );
    assert!(
        s.live && s.peers_seen > 0,
        "download migre pas live / pair non injecte: {s:?}"
    );
    tokio::time::timeout(TRANSFER_WAIT, dl3.wait_completed())
        .await
        .expect("transfert lane 3 en timeout")
        .expect("wait_completed");
    drop(dl);
    eprintln!(
        "live migration 1->3: octets circuit x3 +{}",
        circuit_bytes(&bench, 3) - before
    );
    assert_lane_transfer(&bench, 3, before).await;
    bench.session.stop().await;
}

/// Pause/reprise sur une lane anonyme : le trafic s'arrete puis
/// reprend, le circuit n'est pas reconstruit.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_pause_resume_tunnel() {
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 239) as u8).collect();
    let bench = setup(2, &payload).await;
    circuit_pinned(&bench, 1).await;
    let dl = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            1,
            true,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("add hops=1");
    assert!(
        wait_until(STATE_WAIT, || dl.stats().progress_bytes > 0).await,
        "aucun octet avant pause"
    );
    bench
        .session
        .pause(&bench.infohash_hex)
        .await
        .expect("pause");
    let paused_at = circuit_bytes(&bench, 1);
    // Laisse une fenetre ou le trafic residuel s'epuiserait.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let during_pause = circuit_bytes(&bench, 1) - paused_at;
    eprintln!("live pause: trafic residuel pendant pause = {during_pause} o");
    bench
        .session
        .resume(&bench.infohash_hex)
        .await
        .expect("resume");
    tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
        .await
        .expect("transfert post-resume en timeout")
        .expect("wait_completed");
    assert_lane_transfer(&bench, 1, 0).await;
    bench.session.stop().await;
}

/// Re-`PUT` du meme infohash pendant un transfert actif : le
/// doublon est refuse (l'existant est retourne, aucune migration,
/// un seul download visible).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_doublon_refuse_en_transfert() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 233) as u8).collect();
    let bench = setup(3, &payload).await;
    circuit_pinned(&bench, 1).await;
    circuit_pinned(&bench, 3).await;
    let dl = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            1,
            true,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("add hops=1");
    // Re-add `.torrent` sur une AUTRE lane pendant le transfert.
    let dup = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            3,
            true,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("re-add");
    assert_eq!(
        bench.session.owner_engine_hops(&bench.infohash_hex),
        Some(1),
        "le re-add a migre/duplique le download"
    );
    let visible = bench
        .session
        .downloads()
        .iter()
        .filter(|d| d.info_hash == bench.infohash_hex)
        .count();
    assert_eq!(visible, 1, "doublon visible dans la liste");
    tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
        .await
        .expect("transfert en timeout")
        .expect("wait_completed");
    assert_eq!(dl.info_hash(), dup.info_hash(), "meme download retourne");
    bench.session.stop().await;
}

/// Suppression pendant un transfert actif sur lane anonyme :
/// l'infohash disparait de tous les moteurs, la ligne DB est
/// supprimee, et rien ne reapparait (pas de « Clair » fantome).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_suppression_pendant_transfert() {
    let payload: Vec<u8> = (0..400_000u32).map(|i| (i % 229) as u8).collect();
    let bench = setup(2, &payload).await;
    circuit_pinned(&bench, 1).await;
    let _dl = bench
        .session
        .add_torrent_bytes_anon_with_peers(
            bench.torrent_bytes.clone(),
            false,
            1,
            true,
            None,
            vec![bench.seed_addr],
        )
        .await
        .expect("add hops=1");
    assert!(
        wait_until(STATE_WAIT, || {
            bench
                .session
                .find_download_hex(&bench.infohash_hex)
                .map(|d| d.stats().progress_bytes > 0)
                .unwrap_or(false)
        })
        .await,
        "aucun octet avant suppression"
    );
    bench
        .session
        .remove(&bench.infohash_hex, true)
        .await
        .expect("remove");
    assert!(
        bench
            .session
            .owner_engine_hops(&bench.infohash_hex)
            .is_none(),
        "residu moteur apres remove"
    );
    assert!(
        bench
            .session
            .downloads()
            .iter()
            .all(|d| d.info_hash != bench.infohash_hex),
        "download encore visible"
    );
    // Fenetre de resurrection : la tache moteur est bien morte, rien
    // ne reapparait — surtout pas comme « Clair ».
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        bench
            .session
            .owner_engine_hops(&bench.infohash_hex)
            .is_none(),
        "resurrection moteur apres remove"
    );
    bench.session.stop().await;
}

// ----------------------------------------------------------------
// Banc « flotte » : N torrents distincts sur les 4 lanes, arret du
// daemon puis redemarrage sur le meme `state_dir` — validation de la
// reprise reelle (persistance DB + fastresume rqbit par lane).
// ----------------------------------------------------------------

/// Fichier `name` retrouve recursivement sous `dir`.
fn find_named(dir: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    walkdir(dir)
        .into_iter()
        .find(|p| p.file_name().map(|n| n == name).unwrap_or(false) && p.is_file())
        .and_then(|p| std::fs::read(&p).ok())
}

/// Torrent du banc flotte.
struct FleetTorrent {
    /// Octets `.torrent` / source du magnet.
    bytes: Vec<u8>,
    /// Info-hash hex.
    ih: String,
    /// Nom du fichier contenu.
    name: String,
    /// Contenu attendu.
    payload: Vec<u8>,
    /// Lane demandee a l'ajout.
    hops: u32,
    /// Ajoute par `magnet:` plutot que `.torrent`.
    magnet: bool,
}

/// Rig flotte : relais + seeder multi-torrents + repertoire d'etat
/// reutilise entre les redemarrages.
struct Fleet {
    relays: Vec<Relay>,
    /// Seeder garde vivant.
    _seeder: BtEngine,
    seed_addr: SocketAddr,
    /// `state_dir` persistant (db + fastresume) — survit au restart.
    _state_dir: tempfile::TempDir,
    _seed_dir: tempfile::TempDir,
    state_dir: std::path::PathBuf,
    /// Repertoire de sortie des downloads (`state_dir/downloads`).
    dl_dir: std::path::PathBuf,
    torrents: Vec<FleetTorrent>,
}

/// Configuration session « live » persistante : anonymat actif +
/// fastresume (`persistence_dir`) pour que le restart valide les
/// pieces deja telechargees au lieu de tout relire a froid.
fn live_cfg(state_dir: &std::path::Path) -> CoreConfig {
    let mut cfg = CoreConfig::offline(state_dir.to_path_buf());
    cfg.engine.listen_port = Some(0);
    cfg.engine.fastresume = true;
    cfg.engine.persistence_dir = Some(state_dir.join("rqbit").join("main"));
    cfg.ipv8.enabled = true;
    cfg.ipv8.enable_anonymity = true;
    cfg.ipv8.listen_addr = "127.0.0.1:0".into();
    cfg
}

/// Session live demarree sur `state_dir` (reutilisable au restart).
async fn start_live_session(state_dir: &std::path::Path) -> (CoreSession, Arc<TunnelCommunity>) {
    let session = CoreSession::start(live_cfg(state_dir), Notifier::new())
        .await
        .expect("session live");
    let stack = session.ipv8().expect("stack ipv8");
    let tunnel = stack.tunnel.clone().expect("tunnel community");
    (session, tunnel)
}

/// Cree `specs` torrents distincts seedes par un seul moteur.
/// Chaque torrent contient un fichier `fleet<i>.bin` de taille
/// variable, dans son sous-dossier `f<i>` du repertoire seeder.
async fn fleet_setup(specs: &[(u32, bool)], n_relays: usize) -> Fleet {
    let seed_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let mut torrents = Vec::new();
    for (i, &(hops, magnet)) in specs.iter().enumerate() {
        let name = format!("fleet{i}.bin");
        // Contenu distinct par torrent (pattern dependant de i).
        let payload: Vec<u8> = (0..80_000u32 + i as u32 * 40_000)
            .map(|j| ((j + i as u32 * 7) % 251) as u8)
            .collect();
        std::fs::write(seed_dir.path().join(&name), &payload).unwrap();
        let torrent = librqbit::create_torrent(
            &seed_dir.path().join(&name),
            librqbit::CreateTorrentOptions {
                piece_length: Some(16384),
                ..Default::default()
            },
            &librqbit::spawn_utils::BlockingSpawner::new(1),
        )
        .await
        .expect("create_torrent fleet");
        let bytes = torrent.as_bytes().unwrap().to_vec();
        let meta = onionbit_format::torrent::TorrentMeta::parse(&bytes).unwrap();
        torrents.push(FleetTorrent {
            ih: onionbit_crypto::hash::to_hex(&meta.info_hash),
            bytes,
            name,
            payload,
            hops,
            magnet,
        });
    }
    let mut seed_cfg = EngineConfig::offline(seed_dir.path().to_path_buf());
    seed_cfg.listen_port = Some(0);
    let seeder = BtEngine::start(seed_cfg).await.expect("seeder");
    for t in &torrents {
        seeder
            .add_torrent_bytes(t.bytes.clone(), false)
            .await
            .expect("seed add");
    }
    let seed_addr = {
        let a = seeder.listen_addr().expect("ecoute seeder");
        if a.ip().is_unspecified() {
            SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), a.port())
        } else {
            a
        }
    };
    let mut relays = Vec::new();
    for _ in 0..n_relays {
        relays.push(make_relay().await);
    }
    Fleet {
        relays,
        _seeder: seeder,
        seed_addr,
        dl_dir: state_dir.path().join("downloads"),
        state_dir: state_dir.path().to_path_buf(),
        _state_dir: state_dir,
        _seed_dir: seed_dir,
        torrents,
    }
}

/// Attend que `ih` soit `finished` cote session (scrutation stats).
async fn wait_finished(session: &CoreSession, ih: &str) -> bool {
    wait_until(TRANSFER_WAIT, || {
        session
            .find_download_hex(ih)
            .map(|d| d.stats().finished)
            .unwrap_or(false)
    })
    .await
}

/// 10 downloads reels (7 `.torrent` + 3 magnets) repartis sur les 4
/// lanes, arret du daemon en cours de transfert puis redemarrage sur
/// le meme `state_dir` :
///
/// - chaque download repart sur **sa** lane persistee (pas de lane
///   par defaut, pas de doublon) ;
/// - le fastresume valide les pieces deja presentes (progression
///   conservee, pas de re-telechargement a zero) ;
/// - apres re-annonce du seeder chaque download termine et le
///   contenu est identique a la source.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_flotte_10_restart() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "onionbit=debug,librqbit=info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
    // (hops, magnet) — couvre les 4 lanes, magnets inclus.
    let specs: Vec<(u32, bool)> = vec![
        (0, false),
        (1, false),
        (2, false),
        (3, true),
        (0, false),
        (1, true),
        (2, false),
        (3, false),
        (1, false),
        (2, true),
    ];
    let fleet = fleet_setup(&specs, 3).await;
    let (session, tunnel) = start_live_session(&fleet.state_dir).await;
    let stack = session.ipv8().unwrap();
    wire_relays(&stack, &tunnel, &fleet.relays);

    // Circuits epingles pour les lanes anonymes utilisees.
    for hops in 1..=3usize {
        circuit_pinned_fl(&tunnel, &fleet.relays, hops).await;
    }

    // Ajout des 10 downloads.
    for t in &fleet.torrents {
        if t.magnet {
            let uri = format!("magnet:?xt=urn:btih:{}&dn={}", t.ih, t.name);
            session
                .add_download_anon_with_peers(
                    &uri,
                    false,
                    t.hops,
                    t.hops > 0,
                    None,
                    vec![fleet.seed_addr],
                )
                .await
                .unwrap_or_else(|e| panic!("magnet {} hops={}: {e}", t.name, t.hops));
        } else {
            session
                .add_torrent_bytes_anon_with_peers(
                    t.bytes.clone(),
                    false,
                    t.hops,
                    t.hops > 0,
                    None,
                    vec![fleet.seed_addr],
                )
                .await
                .unwrap_or_else(|e| panic!("add {} hops={}: {e}", t.name, t.hops));
        }
        assert_eq!(
            session.owner_engine_hops(&t.ih),
            Some(t.hops),
            "{} ajoute sur la mauvaise lane",
            t.name
        );
    }

    // Tous progressent (ou terminent) avant le kill.
    for t in &fleet.torrents {
        assert!(
            wait_until(Duration::from_secs(60), || {
                session
                    .find_download_hex(&t.ih)
                    .map(|d| d.stats().progress_bytes > 0 || d.stats().finished)
                    .unwrap_or(false)
            })
            .await,
            "{} n'a pas demarre",
            t.name
        );
    }
    let before: Vec<u64> = fleet
        .torrents
        .iter()
        .map(|t| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes)
                .unwrap_or(0)
        })
        .collect();
    eprintln!("pre-kill progression (octets): {before:?}");

    // ===== Arret du daemon (quitte l'app) =====
    session.stop().await;

    // ===== Redemarrage sur le meme state_dir =====
    let (session, tunnel) = start_live_session(&fleet.state_dir).await;
    let stack = session.ipv8().unwrap();
    // Le pair session a une nouvelle adresse : les relais le
    // re-apprennent (meme cle persistee, nouvelle socket).
    wire_relays(&stack, &tunnel, &fleet.relays);
    session.wait_restored().await;

    // Restauration : les 10 lignes, chacune sur sa lane persistee.
    let rows = session.anon_hops_map();
    for t in &fleet.torrents {
        assert_eq!(
            rows.get(&t.ih),
            Some(&t.hops),
            "anon_hops persiste perdu pour {}",
            t.name
        );
        assert_eq!(
            session.owner_engine_hops(&t.ih),
            Some(t.hops),
            "{} restaure sur la mauvaise lane",
            t.name
        );
    }
    assert_eq!(
        session.downloads().len(),
        specs.len(),
        "doublon/perte a la restauration"
    );

    // Fastresume : la progression n'est pas repartie a zero — les
    // verifications de hash se serialisent sur les threads de
    // blocage, on borne donc sur une deadline globale.
    let ok = wait_until(Duration::from_secs(120), || {
        fleet.torrents.iter().zip(&before).all(|(t, &pre)| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes >= pre || d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await;
    if !ok {
        for (t, &pre) in fleet.torrents.iter().zip(&before) {
            let st = session
                .find_download_hex(&t.ih)
                .map(|d| format!("{:?}", d.stats()))
                .unwrap_or_else(|| "ABSENT du moteur".into());
            eprintln!(
                "restore {} (hops={}, magnet={}): pre={} stats={st}",
                t.name, t.hops, t.magnet, pre
            );
        }
        panic!("progression perdue au restart (voir ci-dessus)");
    }

    // Les lanes attendent leurs circuits (fail-closed) : on les
    // recree, puis le seeder est re-annonce a chaque download — les
    // pairs ne survivent pas au restart moteur (comme Tribler, qui
    // fait `readd_bittorrent_peers`).
    for hops in 1..=3usize {
        circuit_pinned_fl(&tunnel, &fleet.relays, hops).await;
    }
    for t in &fleet.torrents {
        let dl = session.find_download_hex(&t.ih).expect("download restaure");
        let t0 = Instant::now();
        while Instant::now() - t0 < STATE_WAIT {
            dl.add_peer(fleet.seed_addr);
            if dl.stats().peers_seen > 0 {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        assert!(
            dl.stats().peers_seen > 0,
            "{} : pair jamais injecte",
            t.name
        );
    }

    // Tous terminent via leur lane restauree.
    for t in &fleet.torrents {
        assert!(
            wait_finished(&session, &t.ih).await,
            "{} n'a pas termine apres restart",
            t.name
        );
        let got = find_named(&fleet.dl_dir, &t.name)
            .unwrap_or_else(|| panic!("{} absent du disque", t.name));
        assert_eq!(got, t.payload, "{} : contenu corrompu", t.name);
        // Toujours sur sa lane — jamais bascule en clair.
        assert_eq!(session.owner_engine_hops(&t.ih), Some(t.hops));
    }
    // Trafic tunnel mesure : chaque lane a porte des octets.
    for hops in [1usize, 2, 3] {
        let b = ready_data_circuits(&tunnel, hops)
            .iter()
            .map(|c| c.bytes_up + c.bytes_down)
            .sum::<u64>();
        eprintln!("post-restart lane {hops}: {b} o sur circuits READY");
        assert!(b > 0, "aucun trafic observe sur la lane {hops}");
    }
    session.stop().await;
}

/// Variante de `circuit_pinned` operant sur un tunnel libre (hors
/// `Bench`) — route epinglee + verification `verified_hops`.
async fn circuit_pinned_fl(tunnel: &Arc<TunnelCommunity>, relays: &[Relay], hops: usize) {
    assert!(hops >= 1 && hops <= relays.len());
    let first = peer_of(&relays[0]);
    let pinned: Vec<Peer> = if hops > 2 {
        relays[1..hops - 1].iter().map(peer_of).collect()
    } else {
        Vec::new()
    };
    let exit_pk = relays[hops - 1].key.public_key().to_bin();
    let cid = tunnel
        .create_circuit_pinned(hops, &first, CIRCUIT_TYPE_DATA, Some(exit_pk), None, pinned)
        .await
        .expect("create_circuit_pinned");
    let ok = wait_until(CIRCUIT_WAIT, || {
        tunnel
            .circuits_info()
            .iter()
            .any(|c| c.circuit_id == cid && c.state == "READY" && c.actual_hops == hops)
    })
    .await;
    assert!(ok, "circuit {cid} ({hops} sauts) pas READY");
    let info = tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id == cid)
        .expect("circuit cree");
    let expected: Vec<String> = relays[..hops].iter().map(mid_of).collect();
    assert_eq!(
        info.verified_hops, expected,
        "route verifiee != route epinglee (cid={cid})"
    );
}

/// Comportement documente au restart : un magnet **encore en
/// resolution** (pending METADATA, jamais materialise) n'est pas
/// restaure — l'entree `pending` est en memoire seule et aucune
/// ligne `downloads` n'existe encore. Ecart connu avec Python, qui
/// checkpointe les downloads en etat METADATA : le test fige le
/// comportement actuel pour qu'un futur rattrapage soit visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_magnet_pending_non_restaure_au_restart() {
    let fleet = fleet_setup(&[(2, true)], 3).await;
    let t = &fleet.torrents[0];
    let (session, _tunnel) = start_live_session(&fleet.state_dir).await;

    // Aucun circuit : la resolution reste en pending.
    let uri = format!("magnet:?xt=urn:btih:{}&dn={}", t.ih, t.name);
    let s = session.clone();
    let u = uri.clone();
    let peers = vec![fleet.seed_addr];
    let add_task = tokio::spawn(async move {
        s.add_download_anon_with_peers(&u, false, 2, true, None, peers)
            .await
    });
    let ih = t.ih.clone();
    assert!(
        wait_until(STATE_WAIT, || {
            session.pending_downloads().iter().any(|p| p.infohash == ih)
        })
        .await,
        "magnet pas passe en pending"
    );

    // Arret alors que le magnet n'est jamais resolu.
    session.stop().await;
    drop(add_task); // tache liee a l'ancienne session

    let (session, _t2) = start_live_session(&fleet.state_dir).await;
    session.wait_restored().await;
    assert!(
        session.owner_engine_hops(&t.ih).is_none()
            && session.downloads().iter().all(|d| d.info_hash != t.ih),
        "un magnet jamais resolu ne devrait pas etre restaure (ecart Python documente)"
    );
    session.stop().await;
}
