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

/// Handler appele pour chaque datagramme brut d'un prefixe.
///
/// Utilise par les communities dont les messages ne sont pas tous des
/// `Packet` IPv8 (ex. `TunnelCommunity` : les cellules `msg_id == 0`
/// ne portent ni cle publique ni signature). Quand un raw listener
/// est enregistre sur un prefixe, il recoit le datagramme **en plus**
/// du listener `PacketHandler` eventuel — c'est a lui de decider si
/// les octets sont une cellule ou un paquet signe (`Packet::parse`).
pub type RawPacketHandler = Arc<dyn Fn(SocketAddr, &[u8]) -> Result<(), Ipv8Error> + Send + Sync>;

/// Sens d'un datagramme tapote (enregistrement interop/debug).
#[derive(Debug, Clone, Copy)]
pub enum TapDir {
    /// Datagramme recu.
    Rx,
    /// Datagramme envoye.
    Tx,
}

/// Evenement de tap : (sens, adresse distante, octets bruts).
pub type TapEvent = (TapDir, SocketAddr, Vec<u8>);

/// Endpoint UDP : dispatch par prefixe (community_id + version).
///
/// Equivalent de `endpoint.add_prefix_listener` : chaque community
/// enregistre son prefixe de 22 octets et recoit les datagrammes
/// correspondants.
pub struct UdpEndpoint {
    socket: Arc<UdpSocket>,
    /// Listeners par prefixe de 22 octets (paquets `Packet` decodes).
    listeners: Mutex<HashMap<[u8; PREFIX_LEN], PacketHandler>>,
    /// Listeners "bruts" par prefixe (datagrammes non interpretes :
    /// cellules de tunnel, protocoles hybrides). Dispatch en plus du
    /// `PacketHandler` si les deux sont enregistres.
    raw_listeners: Mutex<HashMap<[u8; PREFIX_LEN], RawPacketHandler>>,
    /// Tap optionnel : recoit chaque datagramme brut (rx+tx) pour
    /// l'enregistrement d'echanges (jalon d'interop, debug).
    tap: Mutex<Option<tokio::sync::broadcast::Sender<TapEvent>>>,
    /// Octets envoyes (`IPv8StatsEndpoint.bytes_up` Python).
    bytes_up: std::sync::atomic::AtomicU64,
    /// Octets recus (`IPv8StatsEndpoint.bytes_down` Python).
    bytes_down: std::sync::atomic::AtomicU64,
}

impl UdpEndpoint {
    /// Lie un socket UDP sur `bind` (ex. `"0.0.0.0:0"` ou
    /// `"127.0.0.1:0"` pour les tests).
    pub async fn bind(bind: &str) -> Result<Arc<Self>, Ipv8Error> {
        let socket = UdpSocket::bind(bind).await?;
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            listeners: Mutex::new(HashMap::new()),
            raw_listeners: Mutex::new(HashMap::new()),
            tap: Mutex::new(None),
            bytes_up: std::sync::atomic::AtomicU64::new(0),
            bytes_down: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    /// Compteurs d'octets pour `/api/statistics/ipv8`.
    pub fn bytes_counters(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.bytes_up.load(Relaxed), self.bytes_down.load(Relaxed))
    }

    /// Adresse locale du socket.
    pub fn local_addr(&self) -> Result<SocketAddr, Ipv8Error> {
        Ok(self.socket.local_addr()?)
    }

    /// Enregistre un listener pour un prefixe de community.
    pub async fn add_prefix_listener(&self, prefix: [u8; PREFIX_LEN], handler: PacketHandler) {
        self.listeners.lock().await.insert(prefix, handler);
    }

    /// Enregistre un listener brut pour un prefixe de community.
    /// Le handler recoit les octets du datagramme tels quels (apres
    /// le match de prefixe) — a lui de distinguer cellule/paquet.
    pub async fn add_raw_prefix_listener(
        &self,
        prefix: [u8; PREFIX_LEN],
        handler: RawPacketHandler,
    ) {
        self.raw_listeners.lock().await.insert(prefix, handler);
    }

    /// Installe le tap de paquets (un seul canal broadcast).
    /// Retourne le receveur a consommer par l'appelant.
    pub async fn set_tap(&self) -> tokio::sync::broadcast::Receiver<TapEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(1024);
        *self.tap.lock().await = Some(tx);
        rx
    }

    /// Envoie des octets bruts a une adresse (UDP numerique
    /// uniquement ; les noms de domaine sont resolus par l'appelant
    /// ou ignores en mode offline).
    pub async fn send_to(&self, addr: &UdpAddress, data: &[u8]) -> Result<(), Ipv8Error> {
        match addr.to_socket_addr() {
            Some(sa) => {
                self.socket.send_to(data, sa).await?;
                self.bytes_up
                    .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
                if let Some(t) = self.tap.lock().await.as_ref() {
                    let _ = t.send((TapDir::Tx, sa, data.to_vec()));
                }
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
            self.bytes_down
                .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
            let data = &buf[..n];
            if let Some(t) = self.tap.lock().await.as_ref() {
                let _ = t.send((TapDir::Rx, src, data.to_vec()));
            }
            if data.len() < PREFIX_LEN {
                continue;
            }
            let mut prefix = [0u8; PREFIX_LEN];
            prefix.copy_from_slice(&data[..PREFIX_LEN]);
            let raw_handler = { self.raw_listeners.lock().await.get(&prefix).cloned() };
            if let Some(h) = raw_handler {
                if let Err(e) = h(src, data) {
                    tracing::debug!(error = %e, "raw handler de community en erreur");
                }
            }
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
