// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Couche transport de datagrammes sous `UdpEndpoint` (ADR-0017,
//! etape 49) : `DatagramTransport` isole l'I/O socket du dispatch
//! par prefixe. `RawUdpTransport` est l'implementation legacy —
//! sockets UDP v4/v6 en clair, comportement `pyipv8` inchange ; le
//! mode stealth branchera un transport morphe sur la meme interface.
//!
//! Les compteurs d'octets et le tap vivent ici, a la **frontiere
//! socket** : ils mesurent ce qui part/revient reellement sur le fil
//! (en stealth, les trames morphes — pas le plaintext interne).

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::net::UdpSocket;
use tokio::sync::broadcast;

use crate::error::Ipv8Error;

/// Taille max d'un datagramme UDP lu (borne defensive ; les paquets
/// pyipv8 tiennent largement sous 64 Ko).
pub(crate) const MAX_DGRAM: usize = 65535;

/// Future boxe retournee par les methodes async du trait — object
/// safety sans dependance `async-trait`.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Sens d'un datagramme tapote (enregistrement interop/debug).
#[derive(Debug, Clone, Copy)]
pub enum TapDir {
    /// Datagramme recu.
    Rx,
    /// Datagramme envoye.
    Tx,
}

/// Evenement de tap : (sens, adresse distante, octets filaires).
pub type TapEvent = (TapDir, SocketAddr, Vec<u8>);

/// Handler de reception : le transport remet chaque datagramme
/// applicatif au dispatch de l'endpoint sous la forme
/// `(source, octets)` — en mode stealth, les octets remis sont le
/// plaintext post-demorph ; le tap mesure toujours la forme filaire.
/// Synchrone : le dispatch ne fait pas d'I/O.
pub type RxHandler = Arc<dyn Fn(SocketAddr, &[u8]) + Send + Sync>;

/// Transport de datagrammes sous `UdpEndpoint` : I/O filaire
/// (envoi/reception), compteurs et tap mesures a la frontiere
/// socket.
pub trait DatagramTransport: Send + Sync {
    /// Envoie `data` vers `dst` dans la forme filaire du transport
    /// (morphee en stealth). Une destination v6 sans socket v6 est un
    /// no-op (parite `DispatcherEndpoint` pyipv8).
    fn send_to<'a>(&'a self, dst: SocketAddr, data: &'a [u8]) -> BoxFut<'a, Result<(), Ipv8Error>>;

    /// Boucle de reception bloquante : remet chaque datagramme
    /// applicatif a `on_rx`. A lancer dans une tache tokio.
    fn run(self: Arc<Self>, on_rx: RxHandler) -> BoxFut<'static, Result<(), Ipv8Error>>;

    /// Adresse d'ecoute locale principale (v4).
    fn local_addr(&self) -> Result<SocketAddr, Ipv8Error>;

    /// Adresse d'ecoute du socket v6 secondaire (`None` si non lie).
    fn local_addr_v6(&self) -> Option<Result<SocketAddr, Ipv8Error>>;

    /// Compteurs d'octets filaires `(up, down)` —
    /// `IPv8StatsEndpoint` pyipv8, base de `/api/statistics/ipv8`.
    fn bytes_counters(&self) -> (u64, u64);

    /// Installe le tap de paquets (un seul canal broadcast) : recoit
    /// chaque datagramme filaire rx+tx. Retourne le receveur a
    /// consommer par l'appelant.
    fn set_tap(&self) -> broadcast::Receiver<TapEvent>;
}

/// Transport UDP en clair — implementation legacy : sockets v4 (+ v6
/// secondaire optionnel partageant les listeners), envoi route par
/// famille d'adresse (`DispatcherEndpoint` pyipv8).
pub struct RawUdpTransport {
    socket: Arc<UdpSocket>,
    /// Socket IPv6 secondaire (interfaces `UDPIPv4` et `UDPIPv6`
    /// pyipv8 partagent les memes listeners ; l'envoi choisit le
    /// socket selon la famille de l'adresse — un pair joint en v6
    /// recoit sa reponse en v6).
    socket_v6: Option<Arc<UdpSocket>>,
    /// Tap optionnel : recoit chaque datagramme filaire (rx+tx) pour
    /// l'enregistrement d'echanges (jalon d'interop, debug).
    tap: Mutex<Option<broadcast::Sender<TapEvent>>>,
    /// Octets envoyes (`IPv8StatsEndpoint.bytes_up` Python).
    bytes_up: AtomicU64,
    /// Octets recus (`IPv8StatsEndpoint.bytes_down` Python).
    bytes_down: AtomicU64,
}

impl RawUdpTransport {
    /// Lie un socket UDP sur `bind` (ex. `"0.0.0.0:0"` ou
    /// `"127.0.0.1:0"` pour les tests).
    pub async fn bind(bind: &str) -> Result<Arc<Self>, Ipv8Error> {
        let socket = UdpSocket::bind(bind).await?;
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            socket_v6: None,
            tap: Mutex::new(None),
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
        }))
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
            tap: Mutex::new(None),
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
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
                    Ok(t) => return Ok(t),
                    Err(e) => {
                        // Si l'echec est du au v6 indisponible sur l'hote, tente v4 seul sur ce port
                        match Self::bind(&v4_str).await {
                            Ok(t) => {
                                tracing::debug!(
                                    listen_v4 = %v4_str,
                                    listen_v6 = %v6,
                                    "bind UDP IPv8 v6 echoue, repli IPv4 seul retenu"
                                );
                                return Ok(t);
                            }
                            Err(_) => Err(e),
                        }
                    }
                },
                None => Self::bind(&v4_str).await,
            };

            match res {
                Ok(t) => return Ok(t),
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

    /// Boucle de reception d'un socket : remet chaque datagramme a
    /// `on_rx` (partagee entre v4 et v6).
    ///
    /// Les erreurs de `recv_from` (ex. `WSAECONNRESET` Windows quand
    /// un ICMP « port injoignable » revient d'un envoi vers un pair
    /// mort) ne doivent **pas** tuer la boucle — le socket reste
    /// utilisable.
    async fn recv_loop(self: &Arc<Self>, socket: Arc<UdpSocket>, on_rx: RxHandler) {
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
            self.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
            let data = &buf[..n];
            if let Some(t) = self.tap.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = t.send((TapDir::Rx, src, data.to_vec()));
            }
            on_rx(src, data);
            // Rendement cooperatif par datagramme : le dispatch est
            // synchrone — sans yield, un flot continu drainerait le
            // buffer noyau entier sans re-ordonnancer les autres
            // taches (famine sous flood sur runtime mono-thread).
            // Parite avec le modele pyipv8 : un callback asyncio par
            // datagramme, l'event-loop avance entre chacun.
            tokio::task::yield_now().await;
        }
    }
}

impl DatagramTransport for RawUdpTransport {
    fn send_to<'a>(&'a self, dst: SocketAddr, data: &'a [u8]) -> BoxFut<'a, Result<(), Ipv8Error>> {
        Box::pin(async move {
            // `DispatcherEndpoint.send` pyipv8 : famille d'adresse
            // -> interface. Sans socket v6, un envoi v6 est un
            // no-op (comme un domaine non resolu).
            let socket = if dst.is_ipv6() {
                match &self.socket_v6 {
                    Some(s) => s.clone(),
                    None => return Ok(()),
                }
            } else {
                self.socket.clone()
            };
            socket.send_to(data, dst).await?;
            self.bytes_up
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            if let Some(t) = self.tap.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = t.send((TapDir::Tx, dst, data.to_vec()));
            }
            Ok(())
        })
    }

    fn run(self: Arc<Self>, on_rx: RxHandler) -> BoxFut<'static, Result<(), Ipv8Error>> {
        Box::pin(async move {
            // Socket IPv6 secondaire : sa boucle de reception partage
            // les memes listeners (`DispatcherEndpoint` pyipv8 — la
            // reception est accrochee directement au sous-endpoint).
            let v6_task = self.socket_v6.clone().map(|sock| {
                let me = self.clone();
                let on_rx = on_rx.clone();
                tokio::spawn(async move { me.recv_loop(sock, on_rx).await })
            });
            self.recv_loop(self.socket.clone(), on_rx).await;
            if let Some(t) = v6_task {
                t.abort();
            }
            Ok(())
        })
    }

    fn local_addr(&self) -> Result<SocketAddr, Ipv8Error> {
        Ok(self.socket.local_addr()?)
    }

    fn local_addr_v6(&self) -> Option<Result<SocketAddr, Ipv8Error>> {
        self.socket_v6.as_ref().map(|s| Ok(s.local_addr()?))
    }

    fn bytes_counters(&self) -> (u64, u64) {
        (
            self.bytes_up.load(Ordering::Relaxed),
            self.bytes_down.load(Ordering::Relaxed),
        )
    }

    fn set_tap(&self) -> broadcast::Receiver<TapEvent> {
        let (tx, rx) = broadcast::channel(1024);
        *self.tap.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        rx
    }
}
