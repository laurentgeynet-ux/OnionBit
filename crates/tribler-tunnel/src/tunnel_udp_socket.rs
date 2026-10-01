//! Socket datagramme adossee aux cellules `data` du tunnel — point
//! d'entree du trafic UDP applicatif (uTP, DHT, trackers UDP) pour les
//! lanes anonymes.
//!
//! En Tribler, la session libtorrent anonyme voit ses datagrammes UDP
//! partir en `UDP ASSOCIATE` vers le SOCKS5 local puis en cellules
//! `data` sur un circuit `READY` ; le noeud de sortie les rejoue en
//! vrai UDP et route les reponses en sens inverse. Ici, rqbit n'a pas
//! de client SOCKS5-UDP : `TunnelUdpSocket` implemente directement les
//! traits attendus par le moteur — `librqbit_utp::Transport` pour la
//! socket uTP et `librqbit_dualstack_sockets::DatagramSocket` pour la
//! DHT et les trackers UDP — au-dessus de
//! [`TunnelCommunity::send_data`]/[`TunnelCommunity::data_rx`].
//!
//! - Envoi : pinning destination -> circuit (`select_circuit` du
//!   SOCKS5 de `ipv8-rust-tunnels`) — un pair/noeud DHT donne reste
//!   sur le meme circuit tant qu'il est `READY`.
//! - Reception : chaque socket souscrit `data_rx` (broadcast par
//!   lane) et ne garde que les datagrammes de sa forme — les
//!   predicats `could_be_*` de `tribler-network-policy` sont les
//!   memes classifieurs que le filtre de sortie (`DataChecker`
//!   pyipv8), sans recouvrement entre les trois formes (octet 0 :
//!   uTP `x1`, DHT `d`=0x64, tracker 0x00).
//!
//! Aucun datagramme ne sort en UDP reel : quand aucun circuit n'est
//! pret (kill switch), les envois sont silencieusement perdus —
//! semantique UDP, les couches uTP/DHT/tracker reessaient.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use librqbit_dualstack_sockets::{DatagramSocket, PollSendToVectored};
use librqbit_utp::Transport;
use tokio::sync::mpsc;
use tribler_ipv8::address::UdpAddress;
use tribler_ipv8::error::Ipv8Error;
use tribler_network_policy::exit_policy::{could_be_dht, could_be_udp_tracker, could_be_utp};

use crate::community::TunnelCommunity;
use crate::routing::PEER_FLAG_EXIT_BT;

/// Capacite de la file d'envoi par socket : au-dela, les datagrammes
/// sont perdus (comportement UDP — la congestion du tunnel est mieux
/// representee par des pertes que par du backpressure, qui
/// bloquerait le dispatcher uTP/DHT).
const OUT_QUEUE_CAP: usize = 1024;
/// Capacite de la file de reception par socket.
const IN_QUEUE_CAP: usize = 512;

/// Origine factice annoncee dans les cellules `data` (`0.0.0.0:0`,
/// convention `tunnel_data` pyipv8 / `socks5.rs` de la reference).
fn zero_address() -> UdpAddress {
    UdpAddress::unspecified()
}

/// Forme de trafic acheminee par une socket : le predicat de
/// reception associe (memes classifieurs que `DataChecker`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelUdpKind {
    /// uTP BEP-29 (connexions pairs anonymes).
    Utp,
    /// DHT mainline BEP-5 (dictionnaires bencode `d…e`).
    Dht,
    /// Protocole tracker UDP (`action` 0..=3).
    UdpTracker,
}

impl TunnelUdpKind {
    fn matches(self, data: &[u8]) -> bool {
        match self {
            Self::Utp => could_be_utp(data),
            Self::Dht => could_be_dht(data),
            Self::UdpTracker => could_be_udp_tracker(data),
        }
    }
}

/// Etat partage d'une [`TunnelUdpSocket`] (les taches d'E/S et les
/// clones du handle y accedent).
struct Inner {
    tunnel: Arc<TunnelCommunity>,
    hops: usize,
    kind: TunnelUdpKind,
    /// Adresse locale factice (loopback du proxy SOCKS5 de la lane) —
    /// purement indicative (`bind_addr` sert au logging upstream).
    bind_addr: SocketAddr,
    /// Pinning destination -> circuit (`select_circuit` reference).
    dest_circuits: Mutex<HashMap<SocketAddr, u32>>,
    /// File de datagrammes entrants (forme `kind`).
    incoming: tokio::sync::Mutex<mpsc::Receiver<(Vec<u8>, SocketAddr)>>,
    /// Producteur de la file entrante — conserve pour
    /// [`TunnelUdpSocket::inject_incoming`] (trafic e2e hidden
    /// services pousse directement, sans passer par `data_rx`).
    in_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    /// File de datagrammes sortants (drainee par la tache d'envoi).
    out_tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
}

/// Socket datagramme routee par les circuits `data` d'une
/// `TunnelCommunity` (clone = meme socket).
#[derive(Clone)]
pub struct TunnelUdpSocket {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TunnelUdpSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelUdpSocket")
            .field("bind_addr", &self.inner.bind_addr)
            .field("hops", &self.inner.hops)
            .field("kind", &self.inner.kind)
            .finish_non_exhaustive()
    }
}

impl TunnelUdpSocket {
    /// Nouvelle socket tunnel pour `kind` sur les circuits `READY` de
    /// `hops` sauts. `bind_addr` est factice (logging uniquement —
    /// typiquement l'adresse SOCKS5 de la lane).
    pub fn new(
        tunnel: Arc<TunnelCommunity>,
        hops: usize,
        kind: TunnelUdpKind,
        bind_addr: SocketAddr,
    ) -> Self {
        Self::with_syn_filter(tunnel, hops, kind, bind_addr, false)
    }

    /// `new` + `drop_wan_syn` : politique Tribler de la lane
    /// BitTorrent anonyme — les `ST_SYN` livres par l'exit via
    /// `data_rx` sont des connexions WAN non sollicitees (l'entrant
    /// anonyme n'existe que via les lanes e2e, `inject_incoming`).
    /// Filtre opt-in : la socket generique reste neutre.
    pub fn with_syn_filter(
        tunnel: Arc<TunnelCommunity>,
        hops: usize,
        kind: TunnelUdpKind,
        bind_addr: SocketAddr,
        drop_wan_syn: bool,
    ) -> Self {
        let (in_tx, in_rx) = mpsc::channel(IN_QUEUE_CAP);
        let (out_tx, mut out_rx) = mpsc::channel(OUT_QUEUE_CAP);
        let socket = Self {
            inner: Arc::new(Inner {
                tunnel: tunnel.clone(),
                hops,
                kind,
                bind_addr,
                dest_circuits: Mutex::new(HashMap::new()),
                incoming: tokio::sync::Mutex::new(in_rx),
                in_tx: in_tx.clone(),
                out_tx,
            }),
        };

        // Reception : broadcast `data_rx` -> filtrage par forme. La
        // tache meurt quand la socket est droppee (`in_tx` ferme).
        {
            let mut rx = tunnel.data_rx();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(msg) => {
                            if !kind.matches(&msg.data) {
                                continue;
                            }
                            // Les ST_SYN WAN livres par l'exit sont des
                            // connexions entrantes non sollicitees sans
                            // chemin anonyme utile (l'entrant anonyme
                            // n'existe que via les lanes e2e, injectees
                            // par `inject_incoming`). Les laisser
                            // entrer sature la file d'acceptation uTP
                            // du moteur et affame les SYN e2e.
                            if drop_wan_syn
                                && kind == TunnelUdpKind::Utp
                                && tribler_network_policy::exit_policy::is_utp_syn(&msg.data)
                            {
                                continue;
                            }
                            let Some(src) = msg.origin.to_socket_addr() else {
                                continue;
                            };
                            // File pleine : perte UDP.
                            let _ = in_tx.try_send((msg.data, src));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                skipped = n,
                                ?kind,
                                "socket tunnel: cellules sautees (retard de lecture)"
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // Envoi : file -> pinning circuit -> cellule `data`.
        {
            let inner = socket.inner.clone();
            tokio::spawn(async move {
                while let Some((data, target)) = out_rx.recv().await {
                    if let Err(e) = inner.dispatch(target, &data).await {
                        tracing::debug!(
                            %target,
                            ?kind,
                            error = %e,
                            "socket tunnel: datagramme perdu"
                        );
                    }
                }
            });
        }

        socket
    }

    /// Epingle `target` sur `cid` — equivalent du pinning
    /// `select_circuit` mais impose par l'appelant. Cote seeder d'un
    /// hidden service, l'adresse factice `circuit_id_to_ip(cid):1024`
    /// du pair distant est epinglee sur le circuit e2e `RP_SEEDER`
    /// pour que les reponses uTP repartent dans LE circuit lie et non
    /// sur un circuit `DATA` aleatoire (`set_udp_associate_default_
    /// remote` de `TriblerTunnelCommunity`).
    pub fn pin_circuit(&self, target: SocketAddr, cid: u32) {
        self.inner.dest_circuits.lock().unwrap().insert(target, cid);
    }

    /// Injecte un datagramme comme s'il arrivait du circuit (forme
    /// `kind` deja verifiee par l'appelant). File pleine : perte UDP.
    /// Cote seeder, les cellules `data` d'un circuit e2e lie sont
    /// poussees ici avec l'adresse factice du pair comme `src` —
    /// role du `serve` de `udp_relay` quand le service est une
    /// `TunnelUdpSocket` et non une vraie socket UDP.
    pub fn inject_incoming(&self, data: Vec<u8>, src: SocketAddr) {
        let _ = self.inner.in_tx.try_send((data, src));
    }

    /// Surface de test : circuit epingle pour `target`, si encore
    /// valide dans la table.
    #[doc(hidden)]
    pub fn pinned_circuit_for(&self, target: SocketAddr) -> Option<u32> {
        self.inner
            .dest_circuits
            .lock()
            .unwrap()
            .get(&target)
            .copied()
    }

    /// Datagramme sortant : enfile pour la tache d'envoi (`send_data`
    /// etant async, `poll_send_to` ne peut pas l'attendre — la file
    /// preserve l'ordre FIFO). File pleine : perte UDP.
    fn enqueue(&self, data: Vec<u8>, target: SocketAddr) -> std::io::Result<usize> {
        let len = data.len();
        match self.inner.out_tx.try_send((data, target)) {
            Ok(()) => Ok(len),
            Err(mpsc::error::TrySendError::Full(_)) => Ok(len),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "socket tunnel fermee",
            )),
        }
    }
}

impl Inner {
    /// Resout/pin le circuit pour `target` puis envoie la cellule.
    async fn dispatch(&self, target: SocketAddr, data: &[u8]) -> Result<(), Ipv8Error> {
        let dest = UdpAddress::from(target);
        let pinned = self.dest_circuits.lock().unwrap().get(&target).copied();
        let cid = { pinned.filter(|cid| self.tunnel.ready_circuits().contains(cid)) };
        let cid = match cid {
            Some(cid) => cid,
            None => {
                if let Some(stale) = pinned {
                    // Le pin pointait sur un circuit mort : on le
                    // depile avant de retomber sur `select_circuit`
                    // (sinon datagrammes perpetuallement perdus sur le
                    // pin stale sans trace).
                    tracing::debug!(
                        %target,
                        stale_cid = stale,
                        ?self.kind,
                        "socket tunnel: pin stale -> reselection"
                    );
                    self.dest_circuits.lock().unwrap().remove(&target);
                }
                let cid = self.select_circuit()?;
                self.dest_circuits.lock().unwrap().insert(target, cid);
                cid
            }
        };
        // Entete uTP : seq_nr/ack_nr (offsets 16 et 18) — l'ack_nr des
        // STATE sortants revele si le vsock a consomme le DATA entrant.
        let (seq_nr, ack_nr) = if self.kind == TunnelUdpKind::Utp && data.len() >= 20 {
            (
                u16::from_be_bytes([data[16], data[17]]),
                u16::from_be_bytes([data[18], data[19]]),
            )
        } else {
            (0, 0)
        };
        tracing::debug!(
            %target,
            circuit_id = cid,
            len = data.len(),
            head = %hex::encode(&data[..data.len().min(8)]),
            seq_nr,
            ack_nr,
            ?self.kind,
            "socket tunnel -> cellule data"
        );
        match self
            .tunnel
            .send_data(cid, &dest, &zero_address(), data)
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                // Le circuit a peut-etre ete detruit entre-temps :
                // on depile le pin pour forcer une reselection.
                self.dest_circuits.lock().unwrap().remove(&target);
                Err(e)
            }
        }
    }

    /// Choix d'un circuit `READY` a `self.hops` sauts, en preferant
    /// ceux dont la sortie annonce `PEER_FLAG_EXIT_BT` (la politique
    /// de sortie `is_exit_data_allowed` exige ce flag pour le trafic
    /// BT) puis en retombant sur n'importe quel circuit pret — un
    /// exit qui n'annonce pas ses flags peut encore accepter selon
    /// sa politique.
    fn select_circuit(&self) -> Result<u32, Ipv8Error> {
        use rand::seq::SliceRandom;
        let mut usable = self
            .tunnel
            .ready_data_circuits_of_hops_flags(self.hops, PEER_FLAG_EXIT_BT);
        if usable.is_empty() {
            usable = self.tunnel.ready_data_circuits_of_hops(self.hops);
        }
        usable.shuffle(&mut rand::thread_rng());
        usable.first().copied().ok_or(Ipv8Error::Malformed(
            "aucun circuit pret pour la socket tunnel",
        ))
    }
}

/// `Transport` (librqbit-utp) : permet `UtpSocket::new_with_opts` sur
/// ce transport — c'est le point d'injection des connexions uTP
/// anonymes (lane `enable_tcp=false` : les pairs sortants ne peuvent
/// passer que par uTP, donc par ces cellules `data`).
impl Transport for TunnelUdpSocket {
    async fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        let mut rx = self.inner.incoming.lock().await;
        match rx.recv().await {
            Some((data, src)) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                Ok((n, src))
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "socket tunnel fermee",
            )),
        }
    }

    async fn send_to<'a>(&'a self, buf: &'a [u8], target: SocketAddr) -> std::io::Result<usize> {
        self.enqueue(buf.to_vec(), target)
    }

    fn poll_send_to(
        &self,
        _cx: &mut Context<'_>,
        buf: &[u8],
        target: SocketAddr,
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.enqueue(buf.to_vec(), target))
    }

    fn bind_addr(&self) -> SocketAddr {
        self.inner.bind_addr
    }
}

impl PollSendToVectored for TunnelUdpSocket {
    fn poll_send_to_vectored(
        &self,
        _cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
        target: SocketAddr,
    ) -> Poll<std::io::Result<usize>> {
        let data: Vec<u8> = bufs.iter().flat_map(|b| b.iter().copied()).collect();
        Poll::Ready(self.enqueue(data, target))
    }
}

/// `DatagramSocket` (librqbit-dualstack-sockets, patch vendored) :
/// injectable dans la DHT (`DhtConfig::socket`) et le client tracker
/// UDP (`UdpTrackerClient::new_with_socket`).
impl DatagramSocket for TunnelUdpSocket {
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        target: SocketAddr,
    ) -> std::pin::Pin<Box<dyn Future<Output = std::io::Result<usize>> + Send + Sync + 'a>> {
        Box::pin(std::future::ready(self.enqueue(buf.to_vec(), target)))
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> std::pin::Pin<
        Box<dyn Future<Output = std::io::Result<(usize, SocketAddr)>> + Send + Sync + 'a>,
    > {
        Box::pin(async move {
            let mut rx = self.inner.incoming.lock().await;
            match rx.recv().await {
                Some((data, src)) => {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    Ok((n, src))
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "socket tunnel fermee",
                )),
            }
        })
    }

    fn bind_addr(&self) -> SocketAddr {
        self.inner.bind_addr
    }
}

/// Les trois sockets d'une lane anonyme (uTP pour les connexions de
/// pairs, DHT pour la decouverte, tracker UDP pour les annonces
/// `udp://`). Chaque instance filtre sa propre forme de paquet.
#[derive(Debug)]
pub struct TunnelUdpSockets {
    /// Socket uTP prete a injecter dans
    /// `librqbit::ConnectionOptions::utp_socket`.
    pub utp: Arc<librqbit_utp::UtpSocket<TunnelUdpSocket, librqbit_utp::DefaultUtpEnvironment>>,
    /// Transport uTP sous-jacent (`TunnelUdpSocket`) — expose pour
    /// [`TunnelUdpSocket::inject_incoming`]/`pin_circuit` cote
    /// hidden services.
    pub utp_transport: TunnelUdpSocket,
    /// Socket pour `DhtSessionConfig::socket`.
    pub dht: TunnelUdpSocket,
    /// Socket pour `SessionOptions::udp_tracker_socket`.
    pub tracker: TunnelUdpSocket,
}

impl TunnelUdpSockets {
    /// Cree le trio de sockets sur la `TunnelCommunity` de la lane.
    /// `bind_addr` : adresse locale factice affichee dans les logs
    /// (le SOCKS5 de la lane).
    pub fn new(
        tunnel: Arc<TunnelCommunity>,
        hops: usize,
        bind_addr: SocketAddr,
    ) -> Result<Self, librqbit_utp::Error> {
        // Lane moteur (`utp_transport`) : les `ST_SYN` WAN livres par
        // l'exit via `data_rx` sont filtres — l'entrant anonyme n'est
        // legitime que via `inject_incoming` (hidden services e2e).
        // La politique reste opt-in : la socket generique (`new`)
        // l'accepte pour rester neutre hors contexte BitTorrent.
        let utp_transport = TunnelUdpSocket::with_syn_filter(
            tunnel.clone(),
            hops,
            TunnelUdpKind::Utp,
            bind_addr,
            true,
        );
        let utp = librqbit_utp::UtpSocket::new_with_opts(
            utp_transport.clone(),
            librqbit_utp::DefaultUtpEnvironment {},
            librqbit_utp::SocketOpts::default(),
        )?;
        Ok(Self {
            utp,
            utp_transport,
            dht: TunnelUdpSocket::new(tunnel.clone(), hops, TunnelUdpKind::Dht, bind_addr),
            tracker: TunnelUdpSocket::new(tunnel, hops, TunnelUdpKind::UdpTracker, bind_addr),
        })
    }
}
