//! Banc DHT publique : telecharge un VRAI torrent a travers le reseau
//! Tribler reel — selection libre des sauts (aucun epinglage, sortie
//! non imposee), DHT mainline routee dans le tunnel.
//!
//! A la difference de `exit_download_interop` (banc controle a sauts
//! epingles), ici :
//! - le premier saut est tire au hasard parmi les pairs decouverts
//!   (ou impose via `--first-hop`),
//! - chaque saut suivant vient des `candidates`/`EXIT_BT|RELAY` du
//!   reseau reel — la route rapportee est celle OBSERVEE
//!   (`verified_hops`), pas une topologie souhaitee,
//! - la sortie est un noeud Tribler reel flagge exit : les datagrammes
//!   DHT/uTP sortent de son adresse publique,
//! - l'integrite du payload est prouvee par la verification des pieces
//!   BitTorrent elle-meme (`progress_bytes` ne compte que les pieces
//!   hachees correctes).
//!
//! Usage :
//!   interop_public_download --bootstrap 127.0.0.1:8090 --magnet URI
//!   interop_public_download --bootstrap A.B.C.D:P --torrent F.torrent
//!     [--hops N] [--first-hop F] [--bind IP] [--walk-seconds S]
//!     [--download-timeout S] [--min-bytes N] [--max-circuits N] [--tap]
//!
//! Verdict : `INTEROP PUBLIC DOWNLOAD OK (octets_verifies=N, route=[…])`
//! ou `ECHEC` — la route affichee est TOUJOURS celle observee.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use onionbit_bittorrent::config::EngineConfig;
use onionbit_bittorrent::engine::BtEngine;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_ipv8::endpoint::UdpEndpoint;
use onionbit_ipv8::peer::{Network, Peer};
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::{PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_IPV8, PEER_FLAG_RELAY};
use onionbit_tunnel::tunnel_udp_socket::TunnelUdpSockets;

/// Bootstrap DHT publics (mainline BEP 5).
const PUBLIC_DHT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
    "dht.aelitis.com:6881",
];

struct TunnelNode {
    key: LibNaClSecretKey,
    network: Arc<Network>,
    tunnel: Arc<TunnelCommunity>,
    endpoint: Arc<UdpEndpoint>,
    addr: SocketAddr,
}

/// Noeud relie au reseau reel : endpoint UDP sur `bind` (0.0.0.0 en
/// banc public — une socket liee a 127.0.0.1 ne peut pas joindre
/// Internet), `my_lan` = IP LAN reelle pour que les pairs nous
/// contactent sur une adresse valide.
async fn make_node(
    community_id: onionbit_ipv8::CommunityId,
    next_hop_timeout: Duration,
    bind: &str,
    lan_ip: IpAddr,
) -> TunnelNode {
    let key = LibNaClSecretKey::generate();
    let network = Arc::new(Network::default());
    let ep = UdpEndpoint::bind(bind).await.unwrap();
    let addr = ep.local_addr().unwrap();
    let lan = UdpAddress::from(SocketAddr::new(lan_ip, addr.port()));
    let tunnel = TunnelCommunity::new_with_id(
        key.clone(),
        network.clone(),
        ep.clone(),
        onionbit_tunnel::settings::TunnelSettings {
            peer_flags: PEER_FLAG_RELAY,
            next_hop_timeout,
            ..Default::default()
        },
        community_id,
    )
    .await;
    let discovery = onionbit_ipv8::discovery::DiscoveryCommunity::new(
        key.clone(),
        network.clone(),
        ep.clone(),
        lan,
    )
    .await;
    // `my_lan`/`my_wan` du tunnel viennent de la discovery (annonces
    // d'introduction) — indispensable hors loopback.
    tunnel.set_discovery(discovery);
    let ep_run = ep.clone();
    tokio::spawn(async move {
        let _ = ep_run.run().await;
    });
    TunnelNode {
        key,
        network,
        tunnel,
        endpoint: ep,
        addr,
    }
}

fn spawn_tap(node: &TunnelNode, tag: &str) {
    let tag = tag.to_string();
    let ep = node.endpoint.clone();
    tokio::spawn(async move {
        let mut rx = ep.set_tap().await;
        while let Ok((dir, src, data)) = rx.recv().await {
            let head = hex::encode(&data[..data.len().min(30)]);
            eprintln!("TAP[{tag}] {dir:?} {src} len={} head={head}", data.len());
        }
    });
}

/// IP LAN reelle : `connect` UDP (sans envoi) revele l'interface de
/// sortie vers une destination publique (192.0.2.1 = TEST-NET-1, aucun
/// trafic reel emis).
fn detect_lan_ip() -> IpAddr {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip())
        .unwrap_or_else(|_| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

/// Tirage pseudo-aleatoire sans dependance `rand` dans le crate :
/// entropie = heure courante + compteur (suffisant pour un banc).
fn pick<T: Clone>(items: &[T], salt: usize) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    Some(items[(nanos ^ salt.wrapping_mul(0x9E3779B9)) % items.len()].clone())
}

fn usage() -> ! {
    eprintln!(
        "usage: interop_public_download --bootstrap IP:PORT \
         (--torrent FICHIER | --magnet URI) [--hops N] [--first-hop F] \
         [--bind IP] [--walk-seconds S] [--download-timeout S] \
         [--min-bytes N] [--max-circuits N] [--tap]"
    );
    std::process::exit(2);
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            "interop_public_download=debug,onionbit_tunnel=info,onionbit_bittorrent=info,\
             librqbit=info,librqbit_dht=info",
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let mut bootstrap = None;
    let mut torrent_file = None;
    let mut magnet = None;
    let mut first_hop_file = None;
    let mut hops = 2usize;
    let mut bind_ip = "0.0.0.0".to_string();
    let mut walk_seconds = 20u64;
    let mut download_timeout_s = 300u64;
    let mut min_bytes = 1u64;
    let mut max_circuits = 4usize;
    let mut hop_timeout_ms = 30_000u64;
    let mut tap = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bootstrap" => bootstrap = args.next(),
            "--torrent" => torrent_file = args.next(),
            "--magnet" => magnet = args.next(),
            "--first-hop" => first_hop_file = args.next(),
            "--hops" => hops = args.next().map(|v| v.parse().unwrap()).unwrap(),
            "--bind" => bind_ip = args.next().unwrap(),
            "--walk-seconds" => walk_seconds = args.next().map(|v| v.parse().unwrap()).unwrap(),
            "--download-timeout" => {
                download_timeout_s = args.next().map(|v| v.parse().unwrap()).unwrap()
            }
            "--min-bytes" => min_bytes = args.next().map(|v| v.parse().unwrap()).unwrap(),
            "--max-circuits" => max_circuits = args.next().map(|v| v.parse().unwrap()).unwrap(),
            "--hop-timeout-ms" => hop_timeout_ms = args.next().map(|v| v.parse().unwrap()).unwrap(),
            "--tap" => tap = true,
            _ => usage(),
        }
    }
    let bootstrap_addr: SocketAddr = bootstrap
        .as_deref()
        .map(|s| s.parse().expect("--bootstrap IP:PORT"))
        .unwrap_or_else(|| usage());
    if torrent_file.is_none() && magnet.is_none() {
        usage();
    }
    assert!(hops >= 1, "--hops >= 1");

    // 1. Noeud sur le reseau reel (bind 0.0.0.0 par defaut), marche
    //    aleatoire sur le prefixe TriblerTunnelCommunity.
    let lan_ip = detect_lan_ip();
    let d = make_node(
        onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID,
        Duration::from_millis(hop_timeout_ms),
        &format!("{bind_ip}:0"),
        lan_ip,
    )
    .await;
    eprintln!(
        "noeud downloader : {} (mid {}) — marche via {}",
        d.addr,
        hex::encode(&d.key.public_key().to_bin()[..8]),
        bootstrap_addr
    );
    if tap {
        spawn_tap(&d, "dl");
    }
    let boot = vec![UdpAddress::from(bootstrap_addr)];
    {
        let t = d.tunnel.clone();
        let b = boot.clone();
        tokio::spawn(async move { t.run(b, Duration::from_secs(1)).await });
    }

    // 2. Marche de decouverte : jusqu'a `walk_seconds` ou assez de
    //    pairs avec au moins un exit flagge.
    let deadline = std::time::Instant::now() + Duration::from_secs(walk_seconds);
    loop {
        let known = d
            .network
            .peers_for_service(&onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID);
        let exits = d
            .tunnel
            .get_candidates_subset(&[PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_IPV8]);
        eprintln!(
            "decouverte : {} pair(s) tunnel connus, {} exit(s) flagge(s)",
            known.len(),
            exits.len()
        );
        if (known.len() >= hops + 2 && !exits.is_empty()) || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let candidates = d
        .network
        .peers_for_service(&onionbit_tunnel::TRIBLER_TUNNEL_COMMUNITY_ID);
    let exits = d
        .tunnel
        .get_candidates_subset(&[PEER_FLAG_EXIT_BT, PEER_FLAG_EXIT_IPV8]);
    eprintln!(
        "pool final : {} pair(s), {} exit(s), {} relais",
        candidates.len(),
        exits.len(),
        d.tunnel.get_candidates(PEER_FLAG_RELAY).len()
    );
    if candidates.is_empty() {
        eprintln!("ECHEC : aucun pair tunnel decouvert via le bootstrap");
        std::process::exit(1);
    }

    // Premier saut impose si --first-hop (format `<pubkey_hex> <port>`
    // ou `<pubkey_hex> <ip:port>`).
    let first_hop_forced = first_hop_file.map(|f| {
        let s = std::fs::read_to_string(&f).expect("lecture first-hop keyfile");
        let mut p = s.split_whitespace();
        let pk = hex::decode(p.next().expect("pubkey hex")).expect("hex invalide");
        let addr: SocketAddr = p
            .next()
            .map(|t| {
                if t.contains(':') {
                    t.parse().unwrap()
                } else {
                    format!("127.0.0.1:{t}").parse().unwrap()
                }
            })
            .expect("addr");
        Peer::new(pk, Some(UdpAddress::from(addr))).expect("cle first-hop invalide")
    });

    // 3. Tentatives de circuit : sauts LIBRES (aucun epinglage, pas de
    //    required_exit) — le dernier saut est choisi par le mecanisme
    //    standard parmi les candidats publics.
    let mut ready_cid = None;
    for attempt in 1..=max_circuits {
        let first = first_hop_forced
            .clone()
            .unwrap_or_else(|| pick(&candidates, attempt).expect("candidates vides"));
        eprintln!(
            "essai circuit {attempt}/{max_circuits} : premier saut {} ({:?})",
            first
                .address
                .as_ref()
                .map(|a| format!("{a:?}"))
                .unwrap_or_default(),
            hex::encode(&first.public_key_bin[..8.min(first.public_key_bin.len())])
        );
        match d
            .tunnel
            .create_circuit_typed(
                hops,
                &first,
                onionbit_tunnel::routing::CIRCUIT_TYPE_DATA,
                None,
                None,
            )
            .await
        {
            Ok(cid) => {
                let ready_timeout = hop_timeout_ms * (hops as u64 + 1);
                if d.tunnel
                    .wait_circuit_ready(cid, ready_timeout)
                    .await
                    .is_ok()
                {
                    ready_cid = Some(cid);
                    break;
                }
                eprintln!("essai {attempt} : circuit {cid} jamais READY");
                d.tunnel.remove_circuit(cid, "timeout banc public").await;
            }
            Err(e) => eprintln!("essai {attempt} : create_circuit -> {e:?}"),
        }
    }
    let Some(cid) = ready_cid else {
        eprintln!("ECHEC : aucun circuit READY en {max_circuits} essai(s)");
        std::process::exit(1);
    };

    // Route OBSERVEE (verified_hops) — le verdict du banc rapporte la
    // route reellement construite, pas celle souhaitee.
    let observed: Vec<String> = d
        .tunnel
        .circuits_info()
        .into_iter()
        .find(|c| c.circuit_id == cid)
        .map(|c| c.verified_hops.clone())
        .unwrap_or_default();
    eprintln!("route observee = {observed:?} ({hops} saut(s) demandes)");

    // 4. Telechargement : uTP + DHT + trackers UDP routes dans le
    //    tunnel ; bootstrap DHT = noeuds publics.
    let dl_dir = tempfile::tempdir().unwrap();
    let mut dl_cfg = EngineConfig::offline(dl_dir.path().to_path_buf());
    dl_cfg.utp_only = true;
    dl_cfg.listen_port = Some(0);
    let socks = TunnelUdpSockets::new(d.tunnel.clone(), hops, "127.0.0.1:0".parse().unwrap())
        .expect("tunnel udp sockets");
    dl_cfg.utp_socket = Some(socks.utp.clone());
    dl_cfg.dht_socket = Some(Arc::new(socks.dht.clone()));
    dl_cfg.udp_tracker_socket = Some(Arc::new(socks.tracker.clone()));
    dl_cfg.enable_dht = true;
    dl_cfg.dht_bootstrap_addrs = Some(PUBLIC_DHT_BOOTSTRAP.iter().map(|s| s.to_string()).collect());
    let dl = BtEngine::start(dl_cfg).await.expect("engine downloader");

    let dl_handle = if let Some(m) = &magnet {
        dl.add_uri(m).await
    } else {
        let bytes = std::fs::read(torrent_file.as_deref().unwrap()).expect("lecture .torrent");
        dl.add_torrent_bytes(bytes, false).await
    }
    .expect("ajout torrent");

    // 5. Attente : `progress_bytes` ne compte que des pieces hachees
    //    correctement — l'integrite est celle du torrent lui-meme.
    //    `--min-bytes 0` = exiger la completion du torrent.
    let deadline = std::time::Instant::now() + Duration::from_secs(download_timeout_s);
    let mut last = 0u64;
    loop {
        let s = dl_handle.stats();
        if s.progress_bytes != last {
            eprintln!(
                "progression : {}/{} octets verifies",
                s.progress_bytes, s.total_bytes
            );
            last = s.progress_bytes;
        }
        let target = if min_bytes == 0 {
            s.total_bytes
        } else {
            min_bytes
        };
        if s.total_bytes > 0 && s.progress_bytes >= target {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let s = dl_handle.stats();
    let reached = if min_bytes == 0 {
        s.total_bytes > 0 && s.progress_bytes >= s.total_bytes
    } else {
        s.progress_bytes >= min_bytes
    };
    if reached {
        eprintln!(
            "INTEROP PUBLIC DOWNLOAD OK (octets_verifies={}, route={:?}, {} saut(s))",
            s.progress_bytes, observed, hops
        );
    } else {
        eprintln!(
            "INTEROP PUBLIC DOWNLOAD ECHEC (octets_verifies={} < {} demandes, route={:?})",
            s.progress_bytes, min_bytes, observed
        );
        std::process::exit(1);
    }
}
