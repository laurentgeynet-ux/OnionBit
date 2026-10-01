// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Proxy SOCKS5 minimal (UDP ASSOCIATE) : le point d'entree local pour
//! router du trafic applicatif (ex. BitTorrent UDP) a travers les
//! circuits de `TunnelCommunity` — equivalent de
//! `messaging/anonymization/tunnel/socks5/` cote pyipv8 et de
//! `socks5.rs` cote `ipv8-rust-tunnels`.
//!
//! Protocole implemente :
//! - negociation sans auth (`0x05 0x00`) ;
//! - `UDP ASSOCIATE` (0x03) : cree une socket UDP de relais, les
//!   datagrammes SOCKS5 UDP (`RSV(2) FRAG ATYP ADDR PORT DATA`) sont
//!   decapsules puis envoyes en cellules `data` sur un circuit `READY`
//!   du bon nombre de sauts (sticky destination -> circuit) ;
//! - chemin retour : les `CircuitData` de la community sont
//!   reencapsules en frames SOCKS5 UDP vers le client ;
//! - `CONNECT` : lit la requete HTTP brute du client et l'envoie en
//!   cellule `http-request` (msg 28) sur un circuit dont la sortie
//!   annonce `PEER_FLAG_EXIT_HTTP` ; la reponse (chunks msg 29) est
//!   recollee puis reecrite sur la connexion (`ipv8-rust-tunnels`
//!   `socks5.rs` `perform_http_request`) ;
//! - `BIND` : repond `CommandNotSupported`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::{Arc, Mutex};

use onionbit_ipv8::{Ipv8Error, UdpAddress};
use rand::seq::SliceRandom;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use crate::community::{CircuitData, TunnelCommunity};

/// Version SOCKS5.
const SOCKS5_VER: u8 = 5;
/// Methode "no authentication required".
const SOCKS5_NO_AUTH: u8 = 0;
/// Commande CONNECT.
const CMD_CONNECT: u8 = 1;
/// Commande BIND.
const CMD_BIND: u8 = 2;
/// Commande UDP ASSOCIATE.
const CMD_UDP_ASSOCIATE: u8 = 3;
/// ATYP IPv4.
const ATYP_IPV4: u8 = 1;
/// ATYP nom de domaine.
const ATYP_DOMAIN: u8 = 3;
/// ATYP IPv6.
const ATYP_IPV6: u8 = 4;
/// Reponse : succes.
const REP_SUCCEEDED: u8 = 0;
/// Reponse : commande non supportee.
const REP_CMD_UNSUPPORTED: u8 = 7;
/// Taille du buffer de datagrammes de l'association UDP.
const UDP_BUF: usize = 65535;

/// Serveur SOCKS5 adosse a une `TunnelCommunity`.
pub struct Socks5Server {
    /// Community tunnels (envoi de cellules `data`, receveur de
    /// retour).
    tunnel: Arc<TunnelCommunity>,
    /// Nombre de sauts requis pour les circuits utilises
    /// (`self.hops` des tunnels Rust).
    hops: usize,
    /// `circuit_id -> (socket d'association, adresse client)` : pour
    /// le chemin retour.
    return_map: Mutex<HashMap<u32, (Arc<UdpSocket>, SocketAddr)>>,
}

impl Socks5Server {
    /// Nouveau serveur.
    pub fn new(tunnel: Arc<TunnelCommunity>, hops: usize) -> Arc<Self> {
        Arc::new(Self {
            tunnel,
            hops,
            return_map: Mutex::new(HashMap::new()),
        })
    }

    /// Ecoute TCP SOCKS5 sur `bind` (ex. `"127.0.0.1:0"`) et lance le
    /// dispatch de retour des donnees de circuit. Retourne l'adresse
    /// locale effective.
    pub async fn listen(self: &Arc<Self>, bind: &str) -> Result<SocketAddr, Ipv8Error> {
        let listener = TcpListener::bind(bind).await?;
        let local = listener.local_addr()?;
        self.spawn_return_dispatcher();
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((conn, _)) => {
                        let c = this.clone();
                        tokio::spawn(async move {
                            if let Err(e) = c.handle_connection(conn).await {
                                tracing::debug!(error = %e, "connexion socks5 en erreur");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept socks5");
                    }
                }
            }
        });
        Ok(local)
    }

    /// Tache : les donnees revenant sur nos circuits (`data_rx`) sont
    /// reencapsulees en frames SOCKS5 UDP vers le client associe.
    /// `data_rx` est un broadcast partage entre les lanes : chaque
    /// serveur filtre par sa `return_map` (circuit -> client).
    fn spawn_return_dispatcher(self: &Arc<Self>) {
        let mut rx = self.tunnel.data_rx();
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => this.dispatch_incoming(msg).await,
                    // Retard de lecture : les cellules sautees sont
                    // perdues pour cette lane (comme un drop UDP) —
                    // logue en warn pour distinguer une saturation du
                    // canal d'une panne de circuit dans le diagnostic.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            hops = this.hops,
                            skipped = n,
                            "socks5: retour tunnel en retard, cellules sautees"
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Encapsule une reponse de tunnel en frame SOCKS5 UDP et l'envoie
    /// au client proprietaire du circuit.
    async fn dispatch_incoming(&self, msg: CircuitData) {
        let target = {
            self.return_map
                .lock()
                .unwrap()
                .get(&msg.circuit_id)
                .cloned()
        };
        let Some((socket, client)) = target else {
            return;
        };
        let frame = encode_udp_frame(&msg.origin, &msg.data);
        let _ = socket.send_to(&frame, client).await;
    }

    /// Negociation + commande d'une connexion TCP entrante.
    async fn handle_connection(&self, conn: TcpStream) -> Result<(), Ipv8Error> {
        let mut conn = conn;
        tracing::trace!("socks5: connexion entrante");
        // Greeting : VER NMETHODS METHODS...
        let ver = conn.read_u8().await?;
        let nmethods = conn.read_u8().await?;
        if ver != SOCKS5_VER {
            return Err(Ipv8Error::Malformed("version SOCKS5 != 5"));
        }
        let mut _methods = vec![0u8; nmethods as usize];
        conn.read_exact(&mut _methods).await?;
        conn.write_all(&[SOCKS5_VER, SOCKS5_NO_AUTH]).await?;
        tracing::trace!("socks5: greeting ok");

        // Requete : VER CMD RSV ATYP ...
        let ver = conn.read_u8().await?;
        let cmd = conn.read_u8().await?;
        let _rsv = conn.read_u8().await?;
        tracing::trace!(cmd, "socks5: commande");
        if ver != SOCKS5_VER {
            return Err(Ipv8Error::Malformed("version SOCKS5 != 5"));
        }
        match cmd {
            CMD_UDP_ASSOCIATE => self.handle_associate(conn).await,
            CMD_CONNECT => self.handle_connect(conn).await,
            CMD_BIND => {
                // Reponse negative (adresse nulle) puis fermeture.
                let mut rep = vec![SOCKS5_VER, REP_CMD_UNSUPPORTED, 0, ATYP_IPV4];
                rep.extend_from_slice(&[0; 4]);
                rep.extend_from_slice(&[0; 2]);
                conn.write_all(&rep).await?;
                Ok(())
            }
            _ => Err(Ipv8Error::Malformed("commande SOCKS5 inconnue")),
        }
    }

    /// `CONNECT` (`socks5.rs` `ipv8-rust-tunnels`) : le client
    /// SOCKS5 est libtorrent — la requete lue est HTTP brute, relayee
    /// en cellules `http-request`/`http-response` sur un circuit dont
    /// la sortie porte `PEER_FLAG_EXIT_HTTP`.
    async fn handle_connect(&self, mut conn: TcpStream) -> Result<(), Ipv8Error> {
        let target = read_address(&mut conn).await?;
        // Reponse positive : CONNECT "reussi" cote SOCKS5 (la requete
        // proprement dite part ensuite en cellules).
        let mut rep = vec![SOCKS5_VER, REP_SUCCEEDED, 0, ATYP_IPV4];
        rep.extend_from_slice(&[0; 4]);
        rep.extend_from_slice(&[0; 2]);
        conn.write_all(&rep).await?;

        let mut buf = vec![0u8; crate::http_tunnel::CONNECT_REQUEST_MAX];
        let n = conn.read(&mut buf).await?;
        let cid = self.select_http_circuit()?;
        tracing::trace!(cid, ?target, "socks5: CONNECT -> http-request");
        let response = self
            .tunnel
            .perform_http_request(
                cid,
                &target,
                &buf[..n],
                crate::http_tunnel::HTTP_RESPONSE_TIMEOUT_MS,
            )
            .await?;
        conn.write_all(&response).await?;
        conn.shutdown().await?;
        Ok(())
    }

    /// Circuit `READY` a `self.hops` sauts dont la sortie porte
    /// `PEER_FLAG_EXIT_HTTP` (selection des tunnels Rust).
    fn select_http_circuit(&self) -> Result<u32, Ipv8Error> {
        let mut usable = self
            .tunnel
            .ready_data_circuits_of_hops_flags(self.hops, crate::routing::PEER_FLAG_EXIT_HTTP);
        usable.shuffle(&mut rand::rng());
        usable
            .first()
            .copied()
            .ok_or(Ipv8Error::Malformed("aucun circuit HTTP pret"))
    }

    /// `UDP ASSOCIATE` : socket UDP de relais + boucle de decapsulage.
    async fn handle_associate(&self, mut conn: TcpStream) -> Result<(), Ipv8Error> {
        // Lit (et ignore) l'adresse demandee — le client decouvrira la
        // socket par la reponse.
        // `read_address` consomme `ATYP ADDR PORT` en entier.
        let _dst = read_address(&mut conn).await?;
        tracing::trace!("socks5: associate, socket de relais...");
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let socket = Arc::new(socket);
        let local = socket.local_addr()?;

        // Reponse : 05 00 00 ATYP BND.ADDR BND.PORT.
        let mut rep = vec![SOCKS5_VER, REP_SUCCEEDED, 0];
        match local {
            SocketAddr::V4(a) => {
                rep.push(ATYP_IPV4);
                rep.extend_from_slice(&a.ip().octets());
                rep.extend_from_slice(&a.port().to_be_bytes());
            }
            SocketAddr::V6(a) => {
                rep.push(ATYP_IPV6);
                rep.extend_from_slice(&a.ip().octets());
                rep.extend_from_slice(&a.port().to_be_bytes());
            }
        }
        conn.write_all(&rep).await?;

        // Boucle d'association : frames SOCKS5 UDP -> cellules `data`.
        let mut buf = vec![0u8; UDP_BUF];
        let mut client: Option<SocketAddr> = None;
        let mut addr_to_cid: HashMap<UdpAddress, u32> = HashMap::new();
        loop {
            // Si le TCP controleur se ferme, on termine.
            let mut probe = [0u8; 1];
            tokio::select! {
                r = conn.read(&mut probe) => {
                    match r {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue, // octet parasite : on continue
                    }
                }
                r = socket.recv_from(&mut buf) => {
                    let Ok((n, src)) = r else { break };
                    if client.is_none() {
                        client = Some(src);
                    }
                    if let Err(e) = self
                        .handle_udp_frame(&buf[..n], &mut addr_to_cid, &socket, src)
                        .await
                    {
                        tracing::debug!(error = %e, "frame SOCKS5 UDP ignoree");
                    }
                }
            }
        }
        Ok(())
    }

    /// Une frame SOCKS5 UDP : decapsule la destination et envoie la
    /// donnee sur un circuit.
    async fn handle_udp_frame(
        &self,
        frame: &[u8],
        addr_to_cid: &mut HashMap<UdpAddress, u32>,
        socket: &Arc<UdpSocket>,
        src: SocketAddr,
    ) -> Result<(), Ipv8Error> {
        // RSV(2) FRAG(1) — FRAG doit etre 0.
        if frame.len() < 4 || frame[0] != 0 || frame[1] != 0 || frame[2] != 0 {
            return Err(Ipv8Error::Malformed("frame SOCKS5 UDP invalide"));
        }
        let mut r = onionbit_ipv8::serializer::Reader::new(&frame[3..]);
        let dest = read_udp_address(&mut r)?;
        let data = r.raw().to_vec();

        // `select_circuit` (`ipv8-rust-tunnels` `socks5.rs`) : une IPv4
        // a port `CIRCUIT_ID_PORT` encode directement le circuit e2e
        // (`ip_to_circuit_id`) — le pair cache.
        if let UdpAddress::Ipv4(a) = &dest {
            if a.port() == crate::routing::CIRCUIT_ID_PORT {
                let cid = crate::routing::ip_to_circuit_id(a.ip());
                // Divergence de securite volontaire vs la reference :
                // `ipv8-rust-tunnels` retombe sur la selection d'un
                // circuit DATA quand le cid n'est pas un RP valide —
                // l'IP factice partirait alors comme destination reelle
                // en UDP depuis une sortie. On rejette sans repli.
                if !self.is_ready_rp_circuit(cid) {
                    return Err(Ipv8Error::Malformed(
                        "adresse circuit_id sans circuit RP pret — rejetee",
                    ));
                }
                self.register_return(cid, socket.clone(), src);
                return self
                    .tunnel
                    .send_data(
                        cid,
                        &dest,
                        &UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap()),
                        &data,
                    )
                    .await;
            }
        }

        let cid = match addr_to_cid.get(&dest).copied() {
            Some(cid) if self.circuit_is_usable(cid) => cid,
            _ => {
                let cid = self.select_circuit()?;
                addr_to_cid.insert(dest.clone(), cid);
                cid
            }
        };
        // Lien retour : les donnees revenues sur ce circuit sont
        // renvoyees au client de cette association.
        self.register_return(cid, socket.clone(), src);
        // `send_data` : origine factice (`0.0.0.0:0` comme la reference).
        self.tunnel
            .send_data(
                cid,
                &dest,
                &UdpAddress::from("0.0.0.0:0".parse::<SocketAddr>().unwrap()),
                &data,
            )
            .await
    }

    /// `true` si `cid` est un circuit de rendez-vous (`RP_DOWNLOADER`
    /// ou `RP_SEEDER`) a l'etat `READY` — seule cible legitime de
    /// l'adressage `CIRCUIT_ID_PORT` (`ipv8-rust-tunnels` exige le type
    /// RP et `keys.len() == goal_hops`, couvert ici par l'etat READY).
    /// Public : surface de test pour le garde-fou anti-fuite.
    pub fn is_ready_rp_circuit(&self, cid: u32) -> bool {
        self.tunnel
            .ready_circuits_of_type(crate::routing::CIRCUIT_TYPE_RP_DOWNLOADER)
            .contains(&cid)
            || self
                .tunnel
                .ready_circuits_of_type(crate::routing::CIRCUIT_TYPE_RP_SEEDER)
                .contains(&cid)
    }

    /// Choix d'un circuit `READY` a `self.hops` sauts
    /// (`select_circuit` des tunnels Rust — version simplifiee : choix
    /// aleatoire parmi les circuits prets de la bonne longueur).
    fn select_circuit(&self) -> Result<u32, Ipv8Error> {
        let mut usable = self.tunnel.ready_data_circuits_of_hops(self.hops);
        usable.shuffle(&mut rand::rng());
        usable
            .first()
            .copied()
            .ok_or(Ipv8Error::Malformed("aucun circuit pret"))
    }

    /// `true` si le circuit est encore pret (pour la table sticky).
    fn circuit_is_usable(&self, cid: u32) -> bool {
        self.tunnel.ready_circuits().contains(&cid)
    }

    /// Enregistre le mapping circuit -> (socket, client) apres une
    /// association (appele par `handle_udp_frame` quand une frame est
    /// envoyee avec succes — le premier envoi cree le lien retour).
    pub fn register_return(&self, circuit_id: u32, socket: Arc<UdpSocket>, client: SocketAddr) {
        self.return_map
            .lock()
            .unwrap()
            .insert(circuit_id, (socket, client));
    }
}

/// Lit `ATYP ADDR PORT` sur un flux TCP (requete SOCKS5).
async fn read_address(conn: &mut TcpStream) -> Result<UdpAddress, Ipv8Error> {
    let atyp = conn.read_u8().await?;
    let addr = match atyp {
        ATYP_IPV4 => {
            let mut o = [0u8; 4];
            conn.read_exact(&mut o).await?;
            let port = conn.read_u16().await?;
            return Ok(UdpAddress::from(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::from(o),
                port,
            ))));
        }
        ATYP_IPV6 => {
            let mut o = [0u8; 16];
            conn.read_exact(&mut o).await?;
            let port = conn.read_u16().await?;
            return Ok(UdpAddress::from(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(o),
                port,
                0,
                0,
            ))));
        }
        ATYP_DOMAIN => {
            let len = conn.read_u8().await? as usize;
            let mut h = vec![0u8; len];
            conn.read_exact(&mut h).await?;
            let port = conn.read_u16().await?;
            UdpAddress::Domain(String::from_utf8_lossy(&h).to_string(), port)
        }
        _ => return Err(Ipv8Error::Malformed("ATYP inconnu")),
    };
    Ok(addr)
}

/// Lit `ATYP ADDR PORT` depuis un reader binaire (frame UDP).
fn read_udp_address(
    r: &mut onionbit_ipv8::serializer::Reader<'_>,
) -> Result<UdpAddress, Ipv8Error> {
    let atyp = r.u8()?;
    let addr = match atyp {
        ATYP_IPV4 => {
            let o = r.take(4)?;
            let port = r.u16()?;
            UdpAddress::from(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(o[0], o[1], o[2], o[3]),
                port,
            )))
        }
        ATYP_IPV6 => {
            let o = r.take(16)?;
            let port = r.u16()?;
            let mut a = [0u8; 16];
            a.copy_from_slice(o);
            UdpAddress::from(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(a),
                port,
                0,
                0,
            )))
        }
        ATYP_DOMAIN => {
            let len = r.u8()? as usize;
            let h = r.take(len)?;
            let port = r.u16()?;
            UdpAddress::Domain(String::from_utf8_lossy(h).to_string(), port)
        }
        _ => return Err(Ipv8Error::Malformed("ATYP inconnu")),
    };
    Ok(addr)
}

/// Encode une frame SOCKS5 UDP (`RSV FRAG ATYP ADDR PORT DATA`).
fn encode_udp_frame(src: &UdpAddress, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8, 0, 0];
    match src {
        UdpAddress::Ipv4(a) => {
            out.push(ATYP_IPV4);
            out.extend_from_slice(&a.ip().octets());
            out.extend_from_slice(&a.port().to_be_bytes());
        }
        UdpAddress::Ipv6(a) => {
            out.push(ATYP_IPV6);
            out.extend_from_slice(&a.ip().octets());
            out.extend_from_slice(&a.port().to_be_bytes());
        }
        UdpAddress::Domain(h, p) => {
            out.push(ATYP_DOMAIN);
            out.push(h.len() as u8);
            out.extend_from_slice(h.as_bytes());
            out.extend_from_slice(&p.to_be_bytes());
        }
    }
    out.extend_from_slice(data);
    out
}
