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
            // Relais de banc : `max_joined_circuits` reste la limite
            // protocole Python (100) en prod ; en banc le mini-reseau
            // concentre tout le churn sur 3 noeuds — on la monte pour
            // ne pas mesurer la saturation plutot que le comportement.
            max_joined_circuits: 10_000,
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

/// Logs stderr (`RUST_LOG` ou defaut `onionbit=debug`) — idempotent.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "onionbit=debug,librqbit=info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
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
    init_tracing();
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
        // Borne explicite : un add bloque (moteur lane, persistance)
        // devient un panic localise au lieu d'un hang muet.
        if t.magnet {
            // L'add magnet attend la resolution BEP 9 inline (etat
            // `pending` voulu) : sur lane anonyme elle peut
            // legitiment trainer — on la deporte comme dans le test
            // crash, la convergence reelle est bornee par le
            // `wait_until` de progression par torrent en aval.
            let uri = format!("magnet:?xt=urn:btih:{}&dn={}", t.ih, t.name);
            let s = session.clone();
            let hops = t.hops;
            let peers = vec![fleet.seed_addr];
            tokio::spawn(async move {
                if let Err(e) = s
                    .add_download_anon_with_peers(&uri, false, hops, hops > 0, None, peers)
                    .await
                {
                    tracing::warn!(error = %e, "add magnet flotte echoue");
                }
            });
        } else {
            tokio::time::timeout(
                TRANSFER_WAIT,
                session.add_torrent_bytes_anon_with_peers(
                    t.bytes.clone(),
                    false,
                    t.hops,
                    t.hops > 0,
                    None,
                    vec![fleet.seed_addr],
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("add {} hops={} en timeout", t.name, t.hops))
            .unwrap_or_else(|e| panic!("add {} hops={}: {e}", t.name, t.hops));
        }
        if !t.magnet {
            assert_eq!(
                session.owner_engine_hops(&t.ih),
                Some(t.hops),
                "{} ajoute sur la mauvaise lane",
                t.name
            );
        }
    }
    // Les magnets spawnés ci-dessus materialisent leur download a la
    // resolution : la lane n'est verifiable qu'une fois l'objet moteur
    // cree — le `wait_until` de progression en couvre l'attente.
    for t in fleet.torrents.iter().filter(|t| t.magnet) {
        assert!(
            wait_until(Duration::from_secs(60), || {
                session
                    .owner_engine_hops(&t.ih)
                    .map(|h| h == t.hops)
                    .unwrap_or(false)
            })
            .await,
            "{} : magnet resolu sur la mauvaise lane",
            t.name
        );
    }

    // Tous progressent (ou terminent) avant le kill : le seeder est
    // re-injecte a chaque scrutation. En mode offline (pas de
    // DHT/LSD/trackers) une tentative de connexion initiale perdue —
    // handshake SOCKS/uTP transitoire sur le circuit — n'est jamais
    // retentee par le moteur : meme correction que la re-annonce
    // post-restart (`readd_bittorrent_peers`).
    for t in &fleet.torrents {
        assert!(
            wait_until(Duration::from_secs(60), || {
                if let Some(d) = session.find_download_hex(&t.ih) {
                    d.add_peer(fleet.seed_addr);
                    let st = d.stats();
                    st.progress_bytes > 0 || st.finished
                } else {
                    false
                }
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
    // Marqueurs via tracing (stderr direct) : visibles en direct dans
    // le journal CI meme si le test ne se termine jamais — eprintln
    // serait capture par le harness jusqu'au verdict.
    tracing::info!("flotte: stop() session 1");
    tokio::time::timeout(TRANSFER_WAIT, session.stop())
        .await
        .expect("arret session 1 en timeout");
    tracing::info!("flotte: session 1 arretee, restart");

    // ===== Redemarrage sur le meme state_dir =====
    let (session, tunnel) = start_live_session(&fleet.state_dir).await;
    tracing::info!("flotte: session 2 demarree");
    let stack = session.ipv8().unwrap();
    // Le pair session a une nouvelle adresse : les relais le
    // re-apprennent (meme cle persistee, nouvelle socket).
    wire_relays(&stack, &tunnel, &fleet.relays);
    tokio::time::timeout(TRANSFER_WAIT, session.wait_restored())
        .await
        .expect("restauration en timeout");
    tracing::info!("flotte: restauration terminee");

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

// ----------------------------------------------------------------
// Chaos : cycles de restart repetes, crash pendant resolution.
// ----------------------------------------------------------------

/// Ajoute un `FleetTorrent` a la session (magnet ou `.torrent` selon
/// `t.magnet`) avec le seeder en pair d'amorce.
async fn fleet_add(session: &CoreSession, t: &FleetTorrent, seed_addr: SocketAddr) {
    if t.magnet {
        let uri = format!("magnet:?xt=urn:btih:{}&dn={}", t.ih, t.name);
        session
            .add_download_anon_with_peers(&uri, false, t.hops, t.hops > 0, None, vec![seed_addr])
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
                vec![seed_addr],
            )
            .await
            .unwrap_or_else(|e| panic!("add {} hops={}: {e}", t.name, t.hops));
    }
}

/// Redemarrage complet : `stop()`, nouvelle session sur le meme
/// `state_dir`, rewiring des relais (le pair session a une nouvelle
/// adresse), attente de la restauration, puis circuits epingles
/// 1..=3 — les lanes restent fail-closed tant que leur circuit n'est
/// pas la.
async fn fleet_restart(fleet: &Fleet, session: CoreSession) -> (CoreSession, Arc<TunnelCommunity>) {
    session.stop().await;
    let (session, tunnel) = start_live_session(&fleet.state_dir).await;
    let stack = session.ipv8().unwrap();
    wire_relays(&stack, &tunnel, &fleet.relays);
    session.wait_restored().await;
    for hops in 1..=3usize {
        circuit_pinned_fl(&tunnel, &fleet.relays, hops).await;
    }
    (session, tunnel)
}

/// Re-annonce le seeder a chaque download restaure non pausé et
/// attend que l'injection ait pris (les pairs ne survivent pas au
/// restart moteur — parite `readd_bittorrent_peers` Python).
async fn fleet_reinject(session: &CoreSession, seed_addr: SocketAddr) {
    for d in session.downloads() {
        let dl = match session.find_download_hex(&d.info_hash) {
            Some(d) => d,
            None => continue,
        };
        if dl.is_paused() {
            continue;
        }
        let t0 = Instant::now();
        while Instant::now() - t0 < STATE_WAIT {
            dl.add_peer(seed_addr);
            if dl.stats().peers_seen > 0 {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// Invariants de restauration : chaque torrent attendu est present
/// sur **sa** lane persistee, exactement une fois ; `absent` (si
/// present) ne doit exister nulle part (ni DB, ni moteur) ; `paused`
/// doivent etre restaures en pause.
fn assert_fleet_state(
    session: &CoreSession,
    expected: &[&FleetTorrent],
    absent: Option<&FleetTorrent>,
    paused: &[&FleetTorrent],
) {
    for t in expected {
        assert_eq!(
            session.anon_hops_map().get(&t.ih),
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
        expected.len(),
        "doublon/perte a la restauration"
    );
    if let Some(absent) = absent {
        assert!(
            session.owner_engine_hops(&absent.ih).is_none()
                && session.downloads().iter().all(|d| d.info_hash != absent.ih),
            "{} supprime a reapparu apres restart",
            absent.name
        );
    }
    for t in paused {
        let d = session
            .find_download_hex(&t.ih)
            .unwrap_or_else(|| panic!("{} attendu apres restart", t.name));
        assert!(d.is_paused(), "{} : pause non conservee au restart", t.name);
    }
}

/// Progression non-regression : chaque torrent attendu a conserve au
/// moins sa progression `snap` (ou est `finished`). Deadline globale
/// — les verifications fastresume se serialisent.
async fn fleet_progress_conserved(
    session: &CoreSession,
    expected: &[&FleetTorrent],
    snap: &[u64],
) -> bool {
    wait_until(Duration::from_secs(120), || {
        expected.iter().zip(snap).all(|(t, &pre)| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes >= pre || d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await
}

/// 12 torrents mixtes (`.torrent` + magnets, lanes 0-3), **trois**
/// redemarrages du daemon avec actions entre les cycles :
///
/// - cycle 1 : ajout de la flotte, progression reelle → stop ;
/// - restart 1 : lanes/unicite/progression verifies → pause ciblee
///   (lane 0 + magnet lane 3) + suppression ciblee → stop ;
/// - restart 2 : 11 lignes, supprime absent, pauses conserves →
///   reprise des pauses, progression reprend → stop ;
/// - restart 3 : invariants → tous terminent, contenu identique
///   octet par octet, toujours sur leur lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn live_flotte_restart_3_cycles() {
    init_tracing();
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
        (0, false),
        (3, false),
    ];
    let fleet = fleet_setup(&specs, 3).await;
    let (session, tunnel) = start_live_session(&fleet.state_dir).await;
    let stack = session.ipv8().unwrap();
    wire_relays(&stack, &tunnel, &fleet.relays);
    for hops in 1..=3usize {
        circuit_pinned_fl(&tunnel, &fleet.relays, hops).await;
    }

    // ===== cycle 1 : ajout + progression reelle =====
    for t in &fleet.torrents {
        fleet_add(&session, t, fleet.seed_addr).await;
        assert_eq!(session.owner_engine_hops(&t.ih), Some(t.hops));
    }
    let ok = wait_until(Duration::from_secs(120), || {
        fleet.torrents.iter().all(|t| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes > 0 || d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await;
    assert!(ok, "un download n'a pas demarre (cycle 1)");
    let snap: Vec<u64> = fleet
        .torrents
        .iter()
        .map(|t| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes)
                .unwrap_or(0)
        })
        .collect();
    eprintln!("cycle 1 progression (octets): {snap:?}");

    // ===== restart 1 : etat intact =====
    let (session, _tunnel) = fleet_restart(&fleet, session).await;
    let all: Vec<&FleetTorrent> = fleet.torrents.iter().collect();
    assert_fleet_state(&session, &all, None, &[]);
    assert!(
        fleet_progress_conserved(&session, &all, &snap).await,
        "progression perdue au restart 1"
    );

    // Pause ciblee : t[0] (lane 0) + t[3] (magnet lane 3) — le vrai
    // chemin API : `pause()` moteur + `set_stopped_flag` (persiste
    // `paused`/`user_stopped` dans tribler.db, parite PATCH
    // `state=stop`). Suppression ciblee : t[5] (magnet lane 1).
    let paused_idx = [0usize, 3];
    let removed = &fleet.torrents[5];
    for &i in &paused_idx {
        session
            .pause(&fleet.torrents[i].ih)
            .await
            .expect("pause ciblee");
        session
            .set_stopped_flag(&fleet.torrents[i].ih, true)
            .expect("stopped flag");
    }
    session.remove(&removed.ih, true).await.expect("remove t5");

    // ===== restart 2 : 11 lignes, supprime absent, pauses conserves =====
    let (session, _tunnel) = fleet_restart(&fleet, session).await;
    let survivors: Vec<&FleetTorrent> = fleet
        .torrents
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 5)
        .map(|(_, t)| t)
        .collect();
    let paused: Vec<&FleetTorrent> = paused_idx.iter().map(|&i| &fleet.torrents[i]).collect();
    assert_fleet_state(&session, &survivors, Some(removed), &paused);
    let snap2: Vec<u64> = survivors
        .iter()
        .map(|t| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes)
                .unwrap_or(0)
        })
        .collect();
    // Snap correspondant aux survivants (snap moins l'index 5).
    let snap_surv: Vec<u64> = snap
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 5)
        .map(|(_, &v)| v)
        .collect();
    assert!(
        fleet_progress_conserved(&session, &survivors, &snap_surv).await,
        "progression perdue au restart 2"
    );

    // Reprise des pauses (chemin API : `resume()` + flag) → la
    // progression reprend sur les lanes.
    for t in &paused {
        session.resume(&t.ih).await.expect("resume");
        session.set_stopped_flag(&t.ih, false).expect("resume flag");
    }
    fleet_reinject(&session, fleet.seed_addr).await;
    let ok = wait_until(Duration::from_secs(120), || {
        survivors.iter().zip(&snap2).all(|(t, &pre)| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().progress_bytes > pre || d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await;
    assert!(ok, "progression figee apres restart 2 + resume");

    // ===== restart 3 : invariants puis completion + integrite =====
    let (session, _tunnel) = fleet_restart(&fleet, session).await;
    assert_fleet_state(&session, &survivors, Some(removed), &[]);
    fleet_reinject(&session, fleet.seed_addr).await;
    let ok = wait_until(TRANSFER_WAIT, || {
        survivors.iter().all(|t| {
            session
                .find_download_hex(&t.ih)
                .map(|d| d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await;
    assert!(ok, "un survivant n'a pas termine apres restart 3");
    for t in survivors {
        let got = find_named(&fleet.dl_dir, &t.name).unwrap_or_else(|| panic!("{} absent", t.name));
        assert_eq!(got, t.payload, "{} : contenu corrompu", t.name);
        assert_eq!(session.owner_engine_hops(&t.ih), Some(t.hops));
    }
    session.stop().await;
}

/// Crash brutal pendant la resolution d'un magnet : la session vit
/// dans un **runtime Tokio dedie** sur un thread separe — le runtime
/// est detruit par `shutdown_timeout(0)` sans `stop()` (tasks
/// aborted en vol, pas de flush graceful — equivalent d'un kill de
/// processus : ecritures SQLite/.bitv en vol perdues).
///
/// Avant le kill : 2 downloads `.torrent`+magnet **resolus** sur la
/// lane 0 progressent, 1 magnet lane 2 reste `pending` (aucun
/// circuit → jamais resolu).
///
/// Apres le kill + restart :
/// - les 2 materialises restaurent sur leur lane ;
/// - le pending n'est pas restaure (pas de ligne `downloads`
///   persistee avant resolution — comportement honnete documente) ;
/// - le meme magnet re-ajoute apres restart n'est pas bloque par un
///   `pending` zombie et resout normalement.
#[test]
fn live_crash_pending_magnet_et_restart() {
    init_tracing();
    let main_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .unwrap();
    main_rt.block_on(async {
        // 2 materiels en clair + 1 magnet lane 2 sans circuit
        // (resterait en pending indefiniment — kill garanti pendant
        // la resolution).
        let fleet = fleet_setup(&[(0, false), (0, true), (2, true)], 0).await;
        let state_dir = fleet.state_dir.clone();
        let seed_addr = fleet.seed_addr;
        let add_specs: Vec<(String, Vec<u8>, bool, u32)> = fleet
            .torrents
            .iter()
            .map(|t| (t.ih.clone(), t.bytes.clone(), t.magnet, t.hops))
            .collect();
        let pending_ih = fleet.torrents[2].ih.clone();

        // Signaux child->main / main->child (std mpsc : agnostique au
        // runtime Tokio, utilisable des deux cotes).
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

        let worker = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let (session, _tunnel) = start_live_session(&state_dir).await;
                for (ih, bytes, magnet, hops) in &add_specs {
                    if *magnet {
                        let uri = format!("magnet:?xt=urn:btih:{}&dn=fleet", ih);
                        let s = session.clone();
                        let u = uri.clone();
                        let h = *hops;
                        let peers = vec![seed_addr];
                        // Spawn : le pending reste en cours.
                        tokio::spawn(async move {
                            let _ = s
                                .add_download_anon_with_peers(&u, false, h, h > 0, None, peers)
                                .await;
                        });
                    } else {
                        session
                            .add_torrent_bytes_anon_with_peers(
                                bytes.clone(),
                                false,
                                *hops,
                                *hops > 0,
                                None,
                                vec![seed_addr],
                            )
                            .await
                            .expect("add torrent");
                    }
                }
                // Pret a tuer : les 2 materiels progressent et le
                // magnet lane 2 est en pending.
                let ok = wait_until(Duration::from_secs(60), || {
                    let materialized = session
                        .downloads()
                        .iter()
                        .all(|d| d.progress_bytes > 0 || d.finished)
                        && session.downloads().len() == 2;
                    let pending = session.pending_downloads().iter().any(|p| p.anon_hops == 2);
                    materialized && pending
                })
                .await;
                ready_tx.send(ok).ok();
                // Park jusqu'au kill : le select rend la main au
                // runtime, `block_on` retourne puis le runtime est
                // detruit sans `stop()`.
                let _ = kill_rx.await;
            });
            // Crash : toutes les taches sont aborted a leur point
            // d'attente — pas de `stop()`, pas de flush ordonne.
            rt.shutdown_timeout(Duration::ZERO);
        });

        // Attend que le kill soit legitime (progression + pending
        // reel) — 90 s max.
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(90)),
            Ok(true),
            "la session fille n'a pas atteint l'etat pre-kill"
        );
        kill_tx.send(()).ok();
        worker.join().expect("thread fille");

        // ===== Redemarrage sur le state_dir post-crash =====
        let (session, _tunnel) = start_live_session(&fleet.state_dir).await;
        session.wait_restored().await;

        // Les 2 materialises restaurent sur leur lane (0).
        for t in &fleet.torrents[..2] {
            assert_eq!(
                session.owner_engine_hops(&t.ih),
                Some(0),
                "{} non restaure apres crash",
                t.name
            );
        }
        assert_eq!(session.downloads().len(), 2, "doublon/perte post-crash");
        // Le magnet pending n'a laisse ni ligne ni download.
        assert!(
            session.owner_engine_hops(&pending_ih).is_none()
                && session
                    .downloads()
                    .iter()
                    .all(|d| d.info_hash != pending_ih)
                && session
                    .pending_downloads()
                    .iter()
                    .all(|p| p.infohash != pending_ih),
            "le magnet pending a ete restaure / a laisse un zombie"
        );

        // Comme `readd_bittorrent_peers` Python : les pairs ne
        // survivent pas au restart moteur — on re-annonce le seed
        // sinon les restores restent sans source (le crash a pu
        // emporter un telechargement partiellement recu).
        for t in &fleet.torrents[..2] {
            let dl = session.find_download_hex(&t.ih).expect("download restaure");
            let t0 = Instant::now();
            while Instant::now() - t0 < STATE_WAIT {
                dl.add_peer(fleet.seed_addr);
                if dl.stats().peers_seen > 0 {
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
        }

        // Le meme magnet se re-ajoute et resout sur la lane 0 —
        // aucun blocage par un fantome du crash.
        let uri = format!(
            "magnet:?xt=urn:btih:{}&dn={}",
            pending_ih, fleet.torrents[2].name
        );
        let dl = session
            .add_download_anon_with_peers(&uri, false, 0, false, None, vec![fleet.seed_addr])
            .await
            .expect("re-add magnet post-crash");
        tokio::time::timeout(TRANSFER_WAIT, dl.wait_completed())
            .await
            .expect("resolution/transfert post-crash en timeout")
            .expect("wait_completed");
        // Integrite des 3 contenus.
        for t in &fleet.torrents {
            assert!(
                wait_finished(&session, &t.ih).await,
                "{} n'a pas termine post-crash",
                t.name
            );
            let got =
                find_named(&fleet.dl_dir, &t.name).unwrap_or_else(|| panic!("{} absent", t.name));
            assert_eq!(got, t.payload, "{} : contenu corrompu", t.name);
        }
        session.stop().await;
    });
}

// ----------------------------------------------------------------
// Endurance (nightly, `#[ignore]`) : churn long de la flotte avec
// metriques CSV + seuils de sante.
//
//   LIVE_ENDURANCE_SECS            duree du churn (defaut 300)
//   LIVE_ENDURANCE_CSV             fichier de sortie (defaut
//                                  target/live_endurance.csv)
//   LIVE_ENDURANCE_SEED            graine du PRNG (defaut fixe —
//                                  rejouable)
//   LIVE_ENDURANCE_RESTART_EVERY   actions entre restarts (defaut 30)
//   LIVE_ENDURANCE_MAX_RSS_GROWTH_MB  croissance RSS toleree apres
//                                  warmup (defaut 1024)
//
//   cargo test -p onionbit-core --test live_bench \
//     live_endurance_churn -- --ignored --nocapture
// ----------------------------------------------------------------

/// PRNG xorshift64* — deterministe, sans dependance.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Instantane de sante exporte en CSV a chaque tick.
fn endurance_metrics_line(
    session: &CoreSession,
    tunnel: Option<&TunnelCommunity>,
    sys: &mut sysinfo::System,
    pid: sysinfo::Pid,
    t0: Instant,
    actions: u64,
    restarts: u64,
) -> String {
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    let rss_mb = sys
        .process(pid)
        .map(|p| p.memory() / 1_048_576)
        .unwrap_or(0);
    let alive_tasks = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    let dls = session.downloads();
    let mut init = 0usize;
    let mut down = 0usize;
    let mut seeding = 0usize;
    let mut paused = 0usize;
    let mut err = 0usize;
    for d in &dls {
        match d.state {
            onionbit_bittorrent::DownloadState::Initializing
            | onionbit_bittorrent::DownloadState::Checking => init += 1,
            onionbit_bittorrent::DownloadState::Downloading => down += 1,
            onionbit_bittorrent::DownloadState::Seeding => seeding += 1,
            onionbit_bittorrent::DownloadState::Paused => paused += 1,
            _ => err += 1,
        }
    }
    let pending = session.pending_downloads().len();
    let db = session.anon_hops_map().len();
    let (c_ready, c_total) = tunnel
        .map(|t| {
            let cs = t.circuits_info();
            (cs.iter().filter(|c| c.state == "READY").count(), cs.len())
        })
        .unwrap_or((0, 0));
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        t0.elapsed().as_secs(),
        rss_mb,
        alive_tasks,
        c_ready,
        c_total,
        init,
        down,
        paused,
        seeding,
        err,
        pending,
        db,
        actions,
        restarts,
    )
}

/// Churn long : ajouts/suppressions/pauses/reprises/migrations de
/// lane aleatoires (graine fixe rejouable) + restarts periodiques,
/// avec metriques CSV par tick et seuils de sante a la fin.
///
/// Invariants verifies en fin de run :
/// - chaque survivant est sur sa lane attendue, une seule fois ;
/// - `pending` vide, aucun download coince dans un etat transitoire
///   au-dela de la deadline de drain ;
/// - integrite octet par octet de tous les contenus termines ;
/// - `num_alive_tasks` et RSS reviennent vers leur baseline apres
///   suppression totale (seuil RSS configurable, genereux par defaut
///   pour absorber le bruit de l'allocateur).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "endurance longue — job nightly / manuel"]
async fn live_endurance_churn() {
    init_tracing();
    let secs = env_u64("LIVE_ENDURANCE_SECS", 300);
    let restart_every = env_u64("LIVE_ENDURANCE_RESTART_EVERY", 30).max(1);
    let max_rss_growth_mb = env_u64("LIVE_ENDURANCE_MAX_RSS_GROWTH_MB", 1024);
    let seed = env_u64("LIVE_ENDURANCE_SEED", 0x9E37_79B9_7F4A_7C15);
    let csv_path =
        std::env::var("LIVE_ENDURANCE_CSV").unwrap_or_else(|_| "target/live_endurance.csv".into());
    if let Some(parent) = std::path::Path::new(&csv_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

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
        (0, false),
        (3, false),
    ];
    let fleet = fleet_setup(&specs, 3).await;
    let (mut session, mut tunnel) = start_live_session(&fleet.state_dir).await;
    {
        let stack = session.ipv8().unwrap();
        wire_relays(&stack, &tunnel, &fleet.relays);
        for hops in 1..=3usize {
            circuit_pinned_fl(&tunnel, &fleet.relays, hops).await;
        }
    }

    let mut sys = sysinfo::System::new();
    let pid = sysinfo::get_current_pid().expect("pid");
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(secs);
    let mut rng = Rng(seed.max(1));
    // Presence et lane attendues (la lane bouge apres migration).
    let mut present = vec![false; fleet.torrents.len()];
    let mut lane: Vec<u32> = fleet.torrents.iter().map(|t| t.hops).collect();
    let mut actions = 0u64;
    let mut restarts = 0u64;
    let mut rss_warmup = 0u64;
    let mut csv = String::from(
        "t_s,rss_mb,alive_tasks,circuits_ready,circuits_total,init,downloading,paused,seeding,errors,pending,db_rows,actions,restarts\n",
    );

    // ===== boucle de churn =====
    while Instant::now() < deadline {
        match rng.below(8) {
            // add un torrent absent.
            0 | 1 => {
                let absent: Vec<usize> =
                    (0..fleet.torrents.len()).filter(|&i| !present[i]).collect();
                if !absent.is_empty() {
                    let i = absent[rng.below(absent.len() as u64)];
                    fleet_add(&session, &fleet.torrents[i], fleet.seed_addr).await;
                    present[i] = true;
                    lane[i] = fleet.torrents[i].hops;
                }
            }
            // remove un present.
            2 => {
                let on: Vec<usize> = (0..fleet.torrents.len()).filter(|&i| present[i]).collect();
                if !on.is_empty() {
                    let i = on[rng.below(on.len() as u64)];
                    let _ = session.remove(&fleet.torrents[i].ih, true).await;
                    present[i] = false;
                }
            }
            // pause / resume (chemin API complet).
            3 | 4 => {
                let on: Vec<usize> = (0..fleet.torrents.len()).filter(|&i| present[i]).collect();
                if !on.is_empty() {
                    let i = on[rng.below(on.len() as u64)];
                    let ih = &fleet.torrents[i].ih;
                    if rng.below(2) == 0 {
                        let _ = session.pause(ih).await;
                        let _ = session.set_stopped_flag(ih, true);
                    } else {
                        let _ = session.resume(ih).await;
                        let _ = session.set_stopped_flag(ih, false);
                    }
                }
            }
            // migration de lane aleatoire.
            5 => {
                let on: Vec<usize> = (0..fleet.torrents.len()).filter(|&i| present[i]).collect();
                if !on.is_empty() {
                    let i = on[rng.below(on.len() as u64)];
                    let hops = rng.below(4) as u32;
                    if session
                        .update_hops(&fleet.torrents[i].ih, hops)
                        .await
                        .is_ok()
                    {
                        lane[i] = hops;
                    }
                }
            }
            _ => {}
        }
        actions += 1;
        // Restart periodique.
        if actions.is_multiple_of(restart_every) {
            let (s, t) = fleet_restart(&fleet, session).await;
            session = s;
            tunnel = t;
            restarts += 1;
            fleet_reinject(&session, fleet.seed_addr).await;
        }
        csv.push_str(&endurance_metrics_line(
            &session,
            Some(&tunnel),
            &mut sys,
            pid,
            t0,
            actions,
            restarts,
        ));
        csv.push('\n');
        // Baseline RSS post-warmup : apres ~30 s de churn les
        // structures residentes sont en place.
        if rss_warmup == 0 && t0.elapsed() > Duration::from_secs(30) {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            rss_warmup = sys
                .process(pid)
                .map(|p| p.memory() / 1_048_576)
                .unwrap_or(0);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // ===== drain : tout terminer, verifier l'etat final =====
    let _ = session.resume_all().await;
    for t in &fleet.torrents {
        let _ = session.set_stopped_flag(&t.ih, false);
    }
    fleet_reinject(&session, fleet.seed_addr).await;

    let survivors: Vec<usize> = (0..fleet.torrents.len()).filter(|&i| present[i]).collect();
    let ok = wait_until(TRANSFER_WAIT, || {
        survivors.iter().all(|&i| {
            session
                .find_download_hex(&fleet.torrents[i].ih)
                .map(|d| d.stats().finished)
                .unwrap_or(false)
        })
    })
    .await;
    if !ok {
        for &i in &survivors {
            let st = session
                .find_download_hex(&fleet.torrents[i].ih)
                .map(|d| format!("{:?}", d.stats()))
                .unwrap_or_else(|| "ABSENT".into());
            eprintln!(
                "endurance drain: {} lane={} stats={st}",
                fleet.torrents[i].name, lane[i]
            );
        }
        panic!("des survivants n'ont pas termine (drain)");
    }
    for &i in &survivors {
        let t = &fleet.torrents[i];
        let got = find_named(&fleet.dl_dir, &t.name).unwrap_or_else(|| panic!("{} absent", t.name));
        assert_eq!(got, t.payload, "{} : contenu corrompu", t.name);
        assert_eq!(
            session.owner_engine_hops(&t.ih),
            Some(lane[i]),
            "{} : mauvaise lane finale",
            t.name
        );
    }
    assert_eq!(
        session.downloads().len(),
        survivors.len(),
        "doublon/perte apres churn"
    );
    assert!(
        session.pending_downloads().is_empty(),
        "pending residuel apres churn"
    );

    // ===== seuils ressources : suppression totale puis cooldown =====
    for &i in &survivors {
        let _ = session.remove(&fleet.torrents[i].ih, true).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(
        session.downloads().is_empty(),
        "residu moteur apres suppression totale"
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    let rss_final = sys
        .process(pid)
        .map(|p| p.memory() / 1_048_576)
        .unwrap_or(0);
    let alive_tasks = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    csv.push_str(&endurance_metrics_line(
        &session,
        Some(&tunnel),
        &mut sys,
        pid,
        t0,
        actions,
        restarts,
    ));
    std::fs::write(&csv_path, &csv).expect("ecriture CSV");
    eprintln!(
        "endurance: {actions} actions, {restarts} restarts, \
         rss warmup={rss_warmup} Mo final={rss_final} Mo, \
         tasks vivantes={alive_tasks}, csv={csv_path}"
    );
    if rss_warmup > 0 {
        assert!(
            rss_final <= rss_warmup + max_rss_growth_mb,
            "croissance RSS suspecte : +{} Mo apres warmup (seuil {max_rss_growth_mb})",
            rss_final.saturating_sub(rss_warmup)
        );
    }
    session.stop().await;
}
