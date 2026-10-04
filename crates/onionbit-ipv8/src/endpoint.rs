// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Endpoint UDP (equivalent de
//! `messaging/interfaces/udp/endpoint.py`) : socket UDP liee,
//! dispatch des datagrammes vers les communities par prefixe de 22
//! octets, envoi.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Fenetre glissante d'echantillons `(instant, bytes_up, bytes_down)`
/// pour le debit instantane `bytes_rates()` (extension Rust : pyipv8
/// n'expose que les compteurs cumules — Tribler derive le debit cote
/// GUI, ce qui rend la mesure dependante du sondage client).
#[derive(Debug, Default)]
struct RateWindow {
    /// Echantillons tries par instant croissant, tronques a `span`.
    samples: VecDeque<(Instant, u64, u64)>,
}

impl RateWindow {
    /// Pousse un releve et expire les echantillons plus vieux que
    /// `span` (au moins un echantillon est toujours conserve : la
    /// baseline du prochain debit).
    fn push(&mut self, at: Instant, up: u64, down: u64, span: Duration) {
        self.samples.push_back((at, up, down));
        while self.samples.len() > 1 {
            let Some((t, ..)) = self.samples.front() else {
                break;
            };
            if at.duration_since(*t) <= span {
                break;
            }
            self.samples.pop_front();
        }
    }

    /// Debit `(up, down)` en octets/s entre le premier et le dernier
    /// echantillon. `(0, 0)` si moins de deux instants distincts.
    /// `saturating_sub` : un compteur remis a zero ne produit pas de
    /// debit negatif.
    fn rates(&self) -> (u64, u64) {
        let (Some(&(t0, up0, down0)), Some(&(t1, up1, down1))) =
            (self.samples.front(), self.samples.back())
        else {
            return (0, 0);
        };
        let secs = t1.duration_since(t0).as_secs_f64();
        if secs <= 0.0 {
            return (0, 0);
        }
        (
            (up1.saturating_sub(up0) as f64 / secs) as u64,
            (down1.saturating_sub(down0) as f64 / secs) as u64,
        )
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
    /// Fenetre glissante de compteurs alimentee par
    /// `run_rate_sampler` — sert `bytes_rates()`.
    rate_window: Mutex<RateWindow>,
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
            rate_window: Mutex::new(RateWindow::default()),
            statistics: Mutex::new(HashMap::new()),
        }))
    }

    /// Compteurs d'octets pour `/api/statistics/ipv8`.
    pub fn bytes_counters(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.bytes_up.load(Relaxed), self.bytes_down.load(Relaxed))
    }

    /// Debit instantane `(up, down)` en octets/s, mesure sur la
    /// fenetre glissante (`run_rate_sampler`) — champ `rate_up` /
    /// `rate_down` de `/api/statistics/ipv8`. `(0, 0)` tant que le
    /// sampler n'a pas pose deux releves distincts.
    pub async fn bytes_rates(&self) -> (u64, u64) {
        self.rate_window.lock().await.rates()
    }

    /// Tache d'echantillonnage des compteurs alimentant
    /// `bytes_rates()` : un releve toutes les `interval`, tronque a
    /// `span`. Tache dediee plutot qu'une diff a la lecture : la
    /// mesure ne depend ni du nombre ni de la cadence des clients
    /// HTTP (desktop et web sondent differemment).
    pub async fn run_rate_sampler(self: &Arc<Self>, interval: Duration, span: Duration) {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let (up, down) = self.bytes_counters();
            self.rate_window
                .lock()
                .await
                .push(Instant::now(), up, down, span);
        }
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

    /// Nombre maximal de tentatives d'incrementation de port en cas de
    /// collision (fidele a `create_socket_with_retry` de Tribler).
    pub const MAX_PORT_RETRY_ATTEMPTS: u16 = 1000;

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
            rate_window: Mutex::new(RateWindow::default()),
            statistics: Mutex::new(HashMap::new()),
        }))
    }

    /// Lie un socket UDP IPv4 (et optionnellement IPv6) avec incrementation
    /// sequentielle de port en cas de conflit (`create_socket_with_retry` Tribler :
    /// tente `port, port + 1, ...` jusqu'a `max_attempts`).
    /// Si le port initial vaut 0 ou si l'adresse n'est pas un `SocketAddr`, bind direct.
    /// Si le bind IPv6 echoue pour toute raison, il est ignore avec repli IPv4 seul.
    /// Si toutes les tentatives echouent, repli ultime sur `"0.0.0.0:0"` (port ephemere).
    pub async fn bind_dual_with_retry(
        bind: &str,
        bind_v6: Option<&str>,
        max_attempts: u16,
    ) -> Result<Arc<Self>, Ipv8Error> {
        let parsed_v4 = bind.parse::<SocketAddr>().ok();
        let parsed_v6 = bind_v6.and_then(|s| s.parse::<SocketAddr>().ok());

        // Si le port de depart est 0 ou adresse non parseable, bind direct sans boucle.
        let (mut addr_v4, mut addr_v6) = match parsed_v4 {
            Some(v4) if v4.port() != 0 => (Some(v4), parsed_v6),
            _ => {
                return Self::bind_dual(bind, bind_v6).await;
            }
        };

        let attempts = max_attempts.max(1);
        let mut last_err = None;

        for _ in 0..attempts {
            let v4_str = addr_v4
                .map(|a| a.to_string())
                .unwrap_or_else(|| bind.to_string());
            let v6_str = addr_v6.map(|a| a.to_string());

            // Tente dual-stack si v6 configure, sinon IPv4 seul
            let res = match v6_str.as_deref() {
                Some(v6) => match Self::bind_dual(&v4_str, Some(v6)).await {
                    Ok(ep) => return Ok(ep),
                    Err(e) => {
                        // Si l'echec est du au v6 indisponible sur l'hote, tente v4 seul sur ce port
                        match Self::bind(&v4_str).await {
                            Ok(ep) => {
                                tracing::debug!(
                                    listen_v4 = %v4_str,
                                    listen_v6 = %v6,
                                    "bind UDP IPv8 v6 echoue, repli IPv4 seul retenu"
                                );
                                return Ok(ep);
                            }
                            Err(_) => Err(e),
                        }
                    }
                },
                None => Self::bind(&v4_str).await,
            };

            match res {
                Ok(ep) => return Ok(ep),
                Err(e) => {
                    last_err = Some(e);
                    if let Some(ref mut a4) = addr_v4 {
                        a4.set_port(a4.port().saturating_add(1));
                    }
                    if let Some(ref mut a6) = addr_v6 {
                        a6.set_port(a6.port().saturating_add(1));
                    }
                }
            }
        }

        if let Some(e) = last_err {
            tracing::warn!(
                error = %e,
                bind,
                attempts,
                "echec des tentatives d'incrementation de port UDP IPv8, repli sur port ephemere 0.0.0.0:0"
            );
        }
        Self::bind("0.0.0.0:0").await
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
                // `catch_unwind` : un panic dans un handler de community
                // (ex. bug dans `process_cell`) ne doit PAS tuer la
                // boucle de reception — sinon le noeud devient sourd
                // definitivement tout en continuant a emettre.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(src, data)));
                match res {
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "raw handler de community en erreur");
                    }
                    Err(_) => {
                        tracing::error!(%src, "raw handler de community a panicke — paquet ignore");
                    }
                    _ => {}
                }
            }
            let handler = { self.listeners.lock().await.get(&prefix).cloned() };
            if let Some((h, policy)) = handler {
                match Packet::parse(data, None, &policy) {
                    Ok(pkt) => {
                        let msg_id = pkt.msg_id;
                        let res =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                                h(src, pkt)
                            }));
                        match res {
                            Ok(Err(e)) => {
                                tracing::debug!(
                                    error = %e,
                                    msg_id,
                                    prefix = hex::encode(&prefix[..8]),
                                    %src,
                                    "handler de community en erreur"
                                );
                            }
                            Err(_) => {
                                tracing::error!(
                                    msg_id,
                                    %src,
                                    "handler de community a panicke — paquet ignore"
                                );
                            }
                            _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `RateWindow` : debit mesure entre premier et dernier
    /// echantillon, expiration hors `span`, compteur remis a zero
    /// borne a 0.
    #[test]
    fn rate_window_rates() {
        let span = Duration::from_secs(6);
        let t0 = Instant::now();
        let mut w = RateWindow::default();

        // Vide ou echantillon unique : pas de mesure.
        assert_eq!(w.rates(), (0, 0));
        w.push(t0, 1_000, 2_000, span);
        assert_eq!(w.rates(), (0, 0));

        // +10 Ko up / +30 Ko down en 2 s -> 5 Ko/s / 15 Ko/s.
        w.push(t0 + Duration::from_secs(2), 11_000, 32_000, span);
        assert_eq!(w.rates(), (5_000, 15_000));

        // Compteur remis a zero (redemarrage) : delta negatif borne
        // a 0 plutot qu'en debit negatif geant.
        w.push(t0 + Duration::from_secs(4), 10, 20, span);
        assert_eq!(w.rates(), (0, 0));

        // Expiration : les releves plus vieux que `span` sortent de
        // la mesure (t0+4 expire a t0+12 — 8 s > 6 s).
        w.push(t0 + Duration::from_secs(12), 610, 620, span);
        assert_eq!(w.rates(), (0, 0));
        w.push(t0 + Duration::from_secs(14), 2_610, 4_620, span);
        assert_eq!(w.rates(), (1_000, 2_000));
    }

    /// Verifie que `bind_dual_with_retry` incrémente le port séquentiellement
    /// quand le port initial est déjà pris par un autre socket.
    #[tokio::test]
    async fn bind_retry_incremente_le_port_si_deja_pris() {
        let occupied = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();

        let bind_v4 = format!("127.0.0.1:{port}");
        let ep = UdpEndpoint::bind_dual_with_retry(&bind_v4, None, 10)
            .await
            .expect("bind_dual_with_retry doit reussir sur le port suivant");

        let bound_port = ep.local_addr().unwrap().port();
        assert!(
            bound_port > port && bound_port <= port + 10,
            "L'endpoint aurait du sauter le port occupé ({port}) et se lier à un port incrémenté (reçu: {bound_port})"
        );
    }
}
