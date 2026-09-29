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
/// `NetworkStat` pyipv8 (`to_dict`) : compteurs d'un `msg_id` d'un
/// prefixe de community active.
#[derive(Debug, Clone, Default)]
pub struct NetworkStat {
    /// `msg_id`.
    pub identifier: u8,
    /// Messages envoyes.
    pub num_up: u64,
    /// Messages recus.
    pub num_down: u64,
    /// Octets envoyes.
    pub bytes_up: u64,
    /// Octets recus.
    pub bytes_down: u64,
    /// Premier envoi (epoch sec, 0 si jamais).
    pub first_measured_up: f64,
    /// Premiere reception.
    pub first_measured_down: f64,
    /// Dernier envoi.
    pub last_measured_up: f64,
    /// Derniere reception.
    pub last_measured_down: f64,
}

impl NetworkStat {
    /// `add_sent_stat`.
    fn sent(&mut self, ts: f64, bytes: usize) {
        self.num_up += 1;
        self.bytes_up += bytes as u64;
        self.last_measured_up = ts;
        if self.first_measured_up == 0.0 {
            self.first_measured_up = ts;
        }
    }

    /// `add_received_stat`.
    fn received(&mut self, ts: f64, bytes: usize) {
        self.num_down += 1;
        self.bytes_down += bytes as u64;
        self.last_measured_down = ts;
        if self.first_measured_down == 0.0 {
            self.first_measured_down = ts;
        }
    }
}

/// `get_aggregate_statistics(prefix)` pyipv8.
#[derive(Debug, Clone, Copy, Default)]
pub struct AggregateStats {
    /// Messages envoyes.
    pub num_up: u64,
    /// Messages recus.
    pub num_down: u64,
    /// Octets envoyes.
    pub bytes_up: u64,
    /// Octets recus.
    pub bytes_down: u64,
    /// `last_measured - first_measured` (0 si inconnu).
    pub diff_time: f64,
}

/// Secondes epoch flottantes (timestamps `NetworkStat` Python).
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Equivalent de `endpoint.add_prefix_listener` : chaque community
/// enregistre son prefixe de 22 octets et recoit les datagrammes
/// correspondants.
pub struct UdpEndpoint {
    socket: Arc<UdpSocket>,
    /// Socket IPv6 secondaire (`DispatcherEndpoint` pyipv8 : les
    /// interfaces `UDPIPv4` et `UDPIPv6` partagent les memes
    /// listeners ; l'envoi choisit le socket selon la famille de
    /// l'adresse — un pair joint en v6 recoit sa reponse en v6).
    socket_v6: Option<Arc<UdpSocket>>,
    /// Listeners par prefixe de 22 octets (paquets `Packet` decodes) —
    /// chaque entree emporte la `WirePolicy` de sa community (le layout
    /// `auth`/`dist` depend du handler Python, pas du `msg_id` seul).
    listeners: Mutex<HashMap<[u8; PREFIX_LEN], (PacketHandler, crate::packet::WirePolicy)>>,
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
    /// `StatisticsEndpoint.statistics` : prefixes actives ->
    /// `msg_id` -> compteurs. Un prefixe absent n'est pas compte
    /// (`enable_community_statistics` Python).
    statistics: Mutex<HashMap<[u8; PREFIX_LEN], HashMap<u8, NetworkStat>>>,
}

impl UdpEndpoint {
    /// Lie un socket UDP sur `bind` (ex. `"0.0.0.0:0"` ou
    /// `"127.0.0.1:0"` pour les tests).
    pub async fn bind(bind: &str) -> Result<Arc<Self>, Ipv8Error> {
        let socket = UdpSocket::bind(bind).await?;
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            socket_v6: None,
            listeners: Mutex::new(HashMap::new()),
            raw_listeners: Mutex::new(HashMap::new()),
            tap: Mutex::new(None),
            bytes_up: std::sync::atomic::AtomicU64::new(0),
            bytes_down: std::sync::atomic::AtomicU64::new(0),
            statistics: Mutex::new(HashMap::new()),
        }))
    }

    /// Compteurs d'octets pour `/api/statistics/ipv8`.
    pub fn bytes_counters(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.bytes_up.load(Relaxed), self.bytes_down.load(Relaxed))
    }

    /// `enable_community_statistics` pyipv8 : active/desactive le
    /// comptage par `msg_id` pour un prefixe de community.
    pub async fn enable_community_statistics(&self, prefix: [u8; PREFIX_LEN], enabled: bool) {
        let mut stats = self.statistics.lock().await;
        if enabled {
            stats.entry(prefix).or_default();
        } else {
            stats.remove(&prefix);
        }
    }

    /// `get_statistics(prefix)` pyipv8 : compteurs par `msg_id`
    /// (map vide si le prefixe n'est pas suivi).
    pub async fn get_statistics(&self, prefix: &[u8; PREFIX_LEN]) -> HashMap<u8, NetworkStat> {
        self.statistics
            .lock()
            .await
            .get(prefix)
            .cloned()
            .unwrap_or_default()
    }

    /// `get_aggregate_statistics(prefix)` pyipv8 : somme des
    /// compteurs du prefixe (zeros si non suivi).
    pub async fn get_aggregate_statistics(&self, prefix: &[u8; PREFIX_LEN]) -> AggregateStats {
        let stats = self.statistics.lock().await;
        let mut agg = AggregateStats::default();
        let Some(per_msg) = stats.get(prefix) else {
            return agg;
        };
        // `min_positive` Python : le plus petit timestamp non nul.
        fn min_positive(a: f64, b: f64) -> f64 {
            if b == 0.0 {
                a
            } else if a == 0.0 || b < a {
                b
            } else {
                a
            }
        }
        let mut first_ts = 0.0_f64;
        let mut last_ts = 0.0_f64;
        for s in per_msg.values() {
            agg.num_up += s.num_up;
            agg.num_down += s.num_down;
            agg.bytes_up += s.bytes_up;
            agg.bytes_down += s.bytes_down;
            first_ts = min_positive(
                first_ts,
                min_positive(s.first_measured_up, s.first_measured_down),
            );
            last_ts = last_ts.max(s.last_measured_up).max(s.last_measured_down);
        }
        agg.diff_time = last_ts - first_ts;
        agg
    }

    /// `add_sent_stat` pyipv8 (interne : prefixe deja extrait).
    async fn add_sent_stat(&self, prefix: &[u8; PREFIX_LEN], msg_id: u8, bytes: usize) {
        if let Some(per_msg) = self.statistics.lock().await.get_mut(prefix) {
            per_msg
                .entry(msg_id)
                .or_insert_with(|| NetworkStat {
                    identifier: msg_id,
                    ..NetworkStat::default()
                })
                .sent(now_secs(), bytes);
        }
    }

    /// `add_received_stat` pyipv8 (interne).
    async fn add_received_stat(&self, prefix: &[u8; PREFIX_LEN], msg_id: u8, bytes: usize) {
        if let Some(per_msg) = self.statistics.lock().await.get_mut(prefix) {
            per_msg
                .entry(msg_id)
                .or_insert_with(|| NetworkStat {
                    identifier: msg_id,
                    ..NetworkStat::default()
                })
                .received(now_secs(), bytes);
        }
    }

    /// `bind` + socket IPv6 secondaire (`ipv8/interfaces[UDPIPv6]`
    /// pyipv8 — meme keypair, memes listeners, envoi route par
    /// famille d'adresse). Erreur de bind v6 propagee ; l'appelant
    /// decide du repli IPv4-seul.
    pub async fn bind_dual(bind: &str, bind_v6: Option<&str>) -> Result<Arc<Self>, Ipv8Error> {
        let socket = UdpSocket::bind(bind).await?;
        let socket_v6 = match bind_v6 {
            Some(addr) => Some(Arc::new(UdpSocket::bind(addr).await?)),
            None => None,
        };
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            socket_v6,
            listeners: Mutex::new(HashMap::new()),
            raw_listeners: Mutex::new(HashMap::new()),
            tap: Mutex::new(None),
            bytes_up: std::sync::atomic::AtomicU64::new(0),
            bytes_down: std::sync::atomic::AtomicU64::new(0),
            statistics: Mutex::new(HashMap::new()),
        }))
    }

    /// Adresse locale du socket.
    pub fn local_addr(&self) -> Result<SocketAddr, Ipv8Error> {
        Ok(self.socket.local_addr()?)
    }

    /// Adresse locale du socket IPv6 secondaire (`None` si non lie).
    pub fn local_addr_v6(&self) -> Option<Result<SocketAddr, Ipv8Error>> {
        self.socket_v6.as_ref().map(|s| Ok(s.local_addr()?))
    }

    /// Enregistre un listener pour un prefixe de community. `policy`
    /// decrit le layout filaire de la community (`WirePolicy` — quels
    /// `msg_id` sont non signes / portent `GlobalTimeDistributionPayload`).
    pub async fn add_prefix_listener(
        &self,
        prefix: [u8; PREFIX_LEN],
        handler: PacketHandler,
        policy: crate::packet::WirePolicy,
    ) {
        self.listeners
            .lock()
            .await
            .insert(prefix, (handler, policy));
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
                // `DispatcherEndpoint.send` pyipv8 : famille d'adresse
                // -> interface. Sans socket v6, un envoi v6 est un
                // no-op (comme un domaine non resolu).
                let socket = if sa.is_ipv6() {
                    match &self.socket_v6 {
                        Some(s) => s.clone(),
                        None => return Ok(()),
                    }
                } else {
                    self.socket.clone()
                };
                socket.send_to(data, sa).await?;
                self.bytes_up
                    .fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
                // `StatisticsEndpoint.send` : le msg_id suit le
                // prefixe de 22 octets (paquet IPv8 = >= 23 octets).
                if data.len() > PREFIX_LEN {
                    let mut prefix = [0u8; PREFIX_LEN];
                    prefix.copy_from_slice(&data[..PREFIX_LEN]);
                    self.add_sent_stat(&prefix, data[PREFIX_LEN], data.len())
                        .await;
                }
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
    ///
    /// Les erreurs de `recv_from` (ex. `WSAECONNRESET` Windows quand un
    /// ICMP « port injoignable » revient d'un envoi vers un pair mort)
    /// ne doivent **pas** tuer la boucle — le socket reste utilisable.
    pub async fn run(self: &Arc<Self>) -> Result<(), Ipv8Error> {
        // Socket IPv6 secondaire : sa boucle de reception partage les
        // memes listeners (`DispatcherEndpoint` pyipv8 — la reception
        // est accrochee directement au sous-endpoint).
        let v6_task = self.socket_v6.clone().map(|sock| {
            let me = self.clone();
            tokio::spawn(async move { me.recv_loop(sock).await })
        });
        self.recv_loop(self.socket.clone()).await;
        if let Some(t) = v6_task {
            t.abort();
        }
        Ok(())
    }

    /// Boucle de reception d'un socket : dispatch par prefixe vers
    /// les listeners (partagee entre v4 et v6).
    async fn recv_loop(self: &Arc<Self>, socket: Arc<UdpSocket>) {
        let mut buf = vec![0u8; MAX_DGRAM];
        loop {
            let (n, src) = match socket.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "recv_from en erreur — ecoute poursuivie");
                    // Petite pause : evite un busy-loop si l'erreur est
                    // persistante (interface down, etc.).
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
            };
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
            // `StatisticsEndpoint.on_packet` : compte la reception si
            // le prefixe est suivi (avant tout dispatch).
            if data.len() > PREFIX_LEN {
                self.add_received_stat(&prefix, data[PREFIX_LEN], n).await;
            }
            let raw_handler = { self.raw_listeners.lock().await.get(&prefix).cloned() };
            if let Some(h) = raw_handler {
                if let Err(e) = h(src, data) {
                    tracing::debug!(error = %e, "raw handler de community en erreur");
                }
            }
            let handler = { self.listeners.lock().await.get(&prefix).cloned() };
            if let Some((h, policy)) = handler {
                match Packet::parse(data, None, &policy) {
                    Ok(pkt) => {
                        let msg_id = pkt.msg_id;
                        if let Err(e) = h(src, pkt) {
                            tracing::debug!(error = %e, msg_id, "handler de community en erreur");
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
