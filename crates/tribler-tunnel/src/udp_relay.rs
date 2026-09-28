//! Relais UDP transparent vers les circuits e2e (hidden seeding).
//!
//! rqbit/libtorrent parlent uTP sur une socket UDP concrete — ce
//! pont fait croire a un pair `127.0.0.1:port` ordinaire :
//!
//! - `dial` (cote downloader) : expose un circuit e2e sous une
//!   adresse loopback ; chaque datagramme recu part en cellule `data`
//!   sur le circuit (`dest_address` = l'adresse factice
//!   `circuit_id_to_ip(cid):CIRCUIT_ID_PORT`, comme le SOCKS5 de
//!   `ipv8-rust-tunnels`) ; les donnees entrantes du circuit sont
//!   renvoyees au client UDP local (premiere adresse vue — la socket
//!   uTP unique du moteur).
//! - `serve` (cote seeder) : les donnees entrantes du circuit e2e lie
//!   sont poussees en UDP vers le service local (socket d'ecoute du
//!   moteur BT) ; ses reponses repartent en cellules `data` vers
//!   l'origine.
//!
//! Equivalent fonctionnel du `Socks5Server` + `set_udp_associate_
//! default_remote` de Tribler, sans exiger que le moteur parle
//! SOCKS5 UDP.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tribler_ipv8::address::UdpAddress;
use tribler_ipv8::error::Ipv8Error;

use crate::community::TunnelCommunity;
use crate::routing::{circuit_id_to_ip, CIRCUIT_ID_PORT};

/// Taille du buffer de datagrammes du relais.
const RELAY_BUF: usize = 65535;
/// Adresse factice (`0.0.0.0:0`, convention `tunnel_data` pyipv8).
fn zero_address() -> UdpAddress {
    UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap())
}

/// `dial` : expose `circuit_id` (e2e lie) comme une socket UDP
/// loopback. Retourne l'adresse a donner au moteur comme "adresse du
/// pair cache".
pub async fn dial(tunnel: Arc<TunnelCommunity>, circuit_id: u32) -> Result<SocketAddr, Ipv8Error> {
    // Destination filaire : l'IPv4 factice encodant le circuit
    // (`select_circuit` des tunnels Rust).
    let dest = UdpAddress::from(SocketAddr::V4(std::net::SocketAddrV4::new(
        circuit_id_to_ip(circuit_id),
        CIRCUIT_ID_PORT,
    )));
    dial_to(tunnel, circuit_id, dest).await
}

/// `dial_to` : meme pont que `dial` mais avec une destination filaire
/// explicite — pour un circuit `DATA` ordinaire, `dest` est la socket
/// reelle du service (ex. l'ecoute uTP du seeder) vers laquelle le
/// noeud de sortie relaie les datagrammes.
pub async fn dial_to(
    tunnel: Arc<TunnelCommunity>,
    circuit_id: u32,
    dest: UdpAddress,
) -> Result<SocketAddr, Ipv8Error> {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let local = socket.local_addr()?;
    // Client appris au premier datagramme (socket uTP unique du
    // moteur) — partage entre les deux taches.
    let client = Arc::new(std::sync::Mutex::new(None::<SocketAddr>));

    // Sortant : datagrammes du moteur -> cellules `data`. Le client
    // est fige au premier datagramme (first-seen, comme le
    // `socket.connect` de la reference) : un second emetteur ne peut
    // pas detourner les reponses.
    let out_client = client.clone();
    let out_sock = socket.clone();
    let out_tunnel = tunnel.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF];
        loop {
            match out_sock.recv_from(&mut buf).await {
                Ok((n, src)) => {
                    {
                        let mut c = out_client.lock().unwrap();
                        if c.is_none() {
                            *c = Some(src);
                        } else if *c != Some(src) {
                            tracing::debug!(circuit_id, %src, "relais: datagramme d'un autre client ignore");
                            continue;
                        }
                    }
                    if let Err(e) = out_tunnel
                        .send_data(circuit_id, &dest, &zero_address(), &buf[..n])
                        .await
                    {
                        tracing::warn!(circuit_id, %src, error = %e, "relais: envoi tunnel");
                    }
                }
                Err(e) => {
                    tracing::warn!(circuit_id, error = %e, "relais: recv client");
                    break;
                }
            }
        }
    });

    // Entrant : donnees du circuit -> client UDP appris.
    let mut rx = tunnel.subscribe_circuit_data(circuit_id);
    let in_sock = socket.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let dst = *client.lock().unwrap();
            match dst {
                Some(dst) => {
                    let _ = in_sock.send_to(&msg.data, dst).await;
                }
                None => {
                    tracing::trace!(circuit_id, "relais: donnee avant premier envoi");
                }
            }
        }
    });

    Ok(local)
}

/// `serve` : achemine les donnees entrantes du circuit e2e
/// `circuit_id` vers le service UDP local `service` (socket
/// d'ecoute du moteur BT du seeder) et renvoie ses reponses dans le
/// tunnel. Retourne l'adresse de la socket de relais.
pub async fn serve(
    tunnel: Arc<TunnelCommunity>,
    circuit_id: u32,
    service: SocketAddr,
) -> Result<SocketAddr, Ipv8Error> {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let local = socket.local_addr()?;
    let mut rx = tunnel.subscribe_circuit_data(circuit_id);

    // Entrant : cellules `data` du circuit -> service local.
    let in_sock = socket.clone();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = in_sock.send_to(&msg.data, service).await {
                tracing::warn!(circuit_id, error = %e, "relais: push service");
            }
        }
    });

    // Sortant : reponses du service -> cellules `data` vers
    // `0.0.0.0:0` (l'origine est dans le retour relaye par le RP, les
    // cellules e2e n'utilisent pas org_address cote fil).
    tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, _src)) => {
                    let dest = UdpAddress::from(SocketAddr::V4(std::net::SocketAddrV4::new(
                        circuit_id_to_ip(circuit_id),
                        CIRCUIT_ID_PORT,
                    )));
                    if let Err(e) = tunnel
                        .send_data(circuit_id, &dest, &zero_address(), &buf[..n])
                        .await
                    {
                        tracing::warn!(circuit_id, error = %e, "relais: reponse tunnel");
                    }
                }
                Err(e) => {
                    tracing::warn!(circuit_id, error = %e, "relais: recv service");
                    break;
                }
            }
        }
    });

    Ok(local)
}
