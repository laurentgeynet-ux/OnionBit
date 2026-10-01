use std::{
    net::{Ipv6Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Context;
use librqbit_dualstack_sockets::{BindOpts, TcpListener};
use librqbit_utp::{BindDevice, UtpSocketUdp, UtpSocketUdpOpts};
use tokio::io::AsyncWrite;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::{
    stream_connect::{ConnectionKind, UtpAcceptor},
    vectored_traits::AsyncReadVectored,
};

/// Socket uTP injectee sous forme concrete pour le accept loop
/// (Tribler-Rust-Torrent vendored patch) — un wrapper plutot que
/// `Arc<dyn UtpAcceptor>` direct pour eviter l'inference de lifetime
/// sur le trait object dans `task_listener<A: Accept>`.
pub(crate) struct InjectedUtpSocket(pub Arc<dyn UtpAcceptor>);

pub(crate) struct ListenResult {
    pub tcp_socket: Option<TcpListener>,
    pub utp_socket: Option<Arc<UtpSocketUdp>>,
    /// Accepteur uTP injecte (`ListenerOptions::utp_socket`) — lanes
    /// anonymes dont la socket n'est pas un bind UDP reel
    /// (Tribler-Rust-Torrent vendored patch).
    pub utp_acceptor: Option<InjectedUtpSocket>,
    pub enable_upnp_port_forwarding: bool,
    pub addr: SocketAddr,
    pub announce_port: Option<u16>,
    pub max_pending_incoming_handshake_checks: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum ListenerMode {
    TcpOnly,
    UtpOnly,
    TcpAndUtp,
}

pub const DEFAULT_MAX_PENDING_INCOMING_HANDSHAKE_CHECKS: usize = 256;

impl ListenerMode {
    pub fn tcp_enabled(&self) -> bool {
        match self {
            ListenerMode::TcpOnly => true,
            ListenerMode::UtpOnly => false,
            ListenerMode::TcpAndUtp => true,
        }
    }

    pub fn utp_enabled(&self) -> bool {
        match self {
            ListenerMode::TcpOnly => false,
            ListenerMode::UtpOnly => true,
            ListenerMode::TcpAndUtp => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenerOptions {
    pub mode: ListenerMode,
    pub listen_addr: SocketAddr,
    pub enable_upnp_port_forwarding: bool,
    pub utp_opts: Option<librqbit_utp::SocketOpts>,
    pub announce_port: Option<u16>,
    pub ipv4_only: bool,
    pub max_pending_incoming_handshake_checks: usize,
    /// Socket uTP injectee pour les connexions entrantes
    /// (Tribler-Rust-Torrent vendored patch) : quand elle est
    /// presente, aucune socket UDP reelle n'est ouverte — `accept()`
    /// tourne sur ce transport. Typiquement la meme `UtpSocket` que
    /// `ConnectionOptions::utp_socket`.
    pub utp_socket: Option<Arc<dyn UtpAcceptor>>,
}

impl Default for ListenerOptions {
    fn default() -> Self {
        Self {
            // TODO: once uTP is stable upgrade default to both
            mode: ListenerMode::TcpOnly,
            listen_addr: (Ipv6Addr::UNSPECIFIED, 0).into(),
            enable_upnp_port_forwarding: false,
            utp_opts: None,
            announce_port: None,
            ipv4_only: false,
            max_pending_incoming_handshake_checks: DEFAULT_MAX_PENDING_INCOMING_HANDSHAKE_CHECKS,
            utp_socket: None,
        }
    }
}

impl ListenerOptions {
    pub(crate) async fn start(
        mut self,
        parent_span: Option<tracing::Id>,
        cancellation_token: CancellationToken,
        bind_device: Option<&BindDevice>,
    ) -> anyhow::Result<ListenResult> {
        let mut utp_opts = self.utp_opts.take().unwrap_or_default();
        utp_opts.cancellation_token = cancellation_token.clone();
        utp_opts.parent_span = parent_span;
        utp_opts.dont_wait_for_lastack = true;

        let mut listen_addr = if self.ipv4_only {
            if self.listen_addr.is_ipv6() && self.listen_addr.ip().is_unspecified() {
                // Force to IPv4 unspecified if IPv6 unspecified was requested but we are v4 only
                SocketAddr::from(([0, 0, 0, 0], self.listen_addr.port()))
            } else {
                self.listen_addr
            }
        } else {
            self.listen_addr
        };

        let tcp_socket = if self.mode.tcp_enabled() {
            let listener = TcpListener::bind_tcp(
                listen_addr,
                BindOpts {
                    request_dualstack: !self.ipv4_only,
                    reuseport: false,
                    device: bind_device,
                },
            )
            .context("error starting TCP listener")?;
            listen_addr = listener.bind_addr();
            info!(
                "Listening on TCP {:?} for incoming peer connections",
                listen_addr
            );
            Some(listener)
        } else {
            None
        };

        // Socket injectee (Tribler-Rust-Torrent vendored patch) : la
        // lane anonyme ecoute le uTP sur son transport tunnelse — pas
        // de bind UDP reel (le trafic pair ne doit jamais sortir en
        // clair).
        let custom_utp = self.utp_socket.take();
        let utp_socket = if self.mode.utp_enabled() && custom_utp.is_none() {
            let bind_result = UtpSocketUdp::new_udp_with_opts(
                listen_addr,
                utp_opts,
                UtpSocketUdpOpts { bind_device },
            )
            .await;
            match bind_result {
                Ok(sock) => {
                    listen_addr = sock.bind_addr();
                    info!(
                        "Listening on UDP {:?} for incoming uTP peer connections",
                        listen_addr
                    );
                    Some(sock)
                }
                Err(e) if tcp_socket.is_some() => {
                    // If we listen over TCP, it's not a fatal error if we can't listen over uTP.
                    tracing::error!("Error listening on UDP {listen_addr:?}: {e:#}");
                    None
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        } else {
            None
        };

        let announce_port = if let Some(p) = self.announce_port {
            Some(p)
        } else if listen_addr.ip().is_loopback() {
            None
        } else {
            Some(listen_addr.port())
        };
        Ok(ListenResult {
            tcp_socket,
            utp_socket,
            utp_acceptor: custom_utp.map(InjectedUtpSocket),
            announce_port,
            addr: listen_addr,
            enable_upnp_port_forwarding: self.enable_upnp_port_forwarding,
            max_pending_incoming_handshake_checks: self.max_pending_incoming_handshake_checks,
        })
    }
}

pub(crate) trait Accept {
    const KIND: ConnectionKind;

    async fn accept(
        &self,
    ) -> anyhow::Result<(
        SocketAddr,
        (
            impl AsyncReadVectored + Send + 'static,
            impl AsyncWrite + Unpin + Send + 'static,
        ),
    )>;
}

impl Accept for TcpListener {
    const KIND: ConnectionKind = ConnectionKind::Tcp;
    async fn accept(
        &self,
    ) -> anyhow::Result<(
        SocketAddr,
        (
            impl AsyncReadVectored + Send + 'static,
            impl AsyncWrite + Send + 'static,
        ),
    )> {
        let (stream, addr) = self.accept().await.context("error accepting TCP")?;
        let (read, write) = stream.into_split();
        Ok((addr, (read, write)))
    }
}

impl Accept for Arc<UtpSocketUdp> {
    const KIND: ConnectionKind = ConnectionKind::Utp;
    async fn accept(
        &self,
    ) -> anyhow::Result<(
        SocketAddr,
        (
            impl AsyncReadVectored + Send + 'static,
            impl AsyncWrite + Unpin + Send + 'static,
        ),
    )> {
        let stream = self.accept().await.context("error accepting uTP")?;
        let addr = stream.remote_addr();
        let (read, write) = stream.split();
        Ok((addr, (read, write)))
    }
}

/// Socket uTP injectee (`ListenerOptions::utp_socket`) — le stream
/// accepte est le meme type `UtpStream` quel que soit le transport
/// (Tribler-Rust-Torrent vendored patch).
impl Accept for InjectedUtpSocket {
    const KIND: ConnectionKind = ConnectionKind::Utp;
    async fn accept(
        &self,
    ) -> anyhow::Result<(
        SocketAddr,
        (
            impl AsyncReadVectored + Send + 'static,
            impl AsyncWrite + Unpin + Send + 'static,
        ),
    )> {
        let stream = UtpAcceptor::accept(self.0.clone())
            .await
            .context("error accepting uTP")?;
        let addr = stream.remote_addr();
        let (read, write) = stream.split();
        Ok((addr, (read, write)))
    }
}
