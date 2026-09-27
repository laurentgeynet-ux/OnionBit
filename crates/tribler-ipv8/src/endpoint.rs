//! Endpoint UDP (equivalent de
//! `messaging/interfaces/udp/endpoint.py`) : socket UDP liee,
//! dispatch des datagrammes vers les communities par prefixe de 22
//! octets, envoi.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::Mutex;

use crate::address::UdpAddress;
use crate::error::Ipv8Error;
use crate::packet::{Packet, PREFIX_LEN};

/// Taille max d'un datagramme IPv8 lu (borne defensive ; les paquets
/// pyipv8 tiennent largement sous 64 Ko).
const MAX_DGRAM: usize = 65535;

/// Handler appele pour chaque paquet decode d'une community.
///
/// Args : (adresse source, paquet verifie).
pub type PacketHandler = Arc<dyn Fn(SocketAddr, Packet) -> Result<(), Ipv8Error> + Send + Sync>;

/// Endpoint UDP : dispatch par prefixe (community_id + version).
///
/// Equivalent de `endpoint.add_prefix_listener` : chaque community
/// enregistre son prefixe de 22 octets et recoit les datagrammes
/// correspondants.
pub struct UdpEndpoint {
    socket: Arc<UdpSocket>,
    /// Listeners par prefixe de 22 octets.
    listeners: Mutex<HashMap<[u8; PREFIX_LEN], PacketHandler>>,
}

impl UdpEndpoint {
    /// Lie un socket UDP sur `bind` (ex. `"0.0.0.0:0"` ou
    /// `"127.0.0.1:0"` pour les tests).
    pub async fn bind(bind: &str) -> Result<Arc<Self>, Ipv8Error> {
        let socket = UdpSocket::bind(bind).await?;
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            listeners: Mutex::new(HashMap::new()),
        }))
    }

    /// Adresse locale du socket.
    pub fn local_addr(&self) -> Result<SocketAddr, Ipv8Error> {
        Ok(self.socket.local_addr()?)
    }

    /// Enregistre un listener pour un prefixe de community.
    pub async fn add_prefix_listener(&self, prefix: [u8; PREFIX_LEN], handler: PacketHandler) {
        self.listeners.lock().await.insert(prefix, handler);
    }

    /// Envoie des octets bruts a une adresse (UDP numerique
    /// uniquement ; les noms de domaine sont resolus par l'appelant
    /// ou ignores en mode offline).
    pub async fn send_to(&self, addr: &UdpAddress, data: &[u8]) -> Result<(), Ipv8Error> {
        match addr.to_socket_addr() {
            Some(sa) => {
                self.socket.send_to(data, sa).await?;
                Ok(())
            }
            // Comme le Python : on ne peut pas envoyer a un nom de
            // domaine non resolu -> no-op silencieux (log debug).
            None => {
                tracing::debug!(?addr, "envoi ignore : adresse non resoluble en SocketAddr");
                Ok(())
            }
        }
    }

    /// Boucle de reception : dispatch par prefixe vers les listeners.
    /// Bloquante — a lancer dans une tache tokio.
    pub async fn run(self: &Arc<Self>) -> Result<(), Ipv8Error> {
        let mut buf = vec![0u8; MAX_DGRAM];
        loop {
            let (n, src) = self.socket.recv_from(&mut buf).await?;
            let data = &buf[..n];
            if data.len() < PREFIX_LEN {
                continue;
            }
            let mut prefix = [0u8; PREFIX_LEN];
            prefix.copy_from_slice(&data[..PREFIX_LEN]);
            let handler = { self.listeners.lock().await.get(&prefix).cloned() };
            if let Some(h) = handler {
                match Packet::parse(data, None) {
                    Ok(pkt) => {
                        if let Err(e) = h(src, pkt) {
                            tracing::debug!(error = %e, "handler de community en erreur");
                        }
                    }
                    Err(e) => {
                        tracing::trace!(error = %e, "paquet IPv8 rejete");
                    }
                }
            }
        }
    }
}
