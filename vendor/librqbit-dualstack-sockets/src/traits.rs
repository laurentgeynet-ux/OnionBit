use std::{
    net::SocketAddr,
    task::{Context, Poll},
};

use socket2::SockRef;

pub trait PollSendToVectored {
    fn poll_send_to_vectored(
        &self,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
        target: SocketAddr,
    ) -> Poll<std::io::Result<usize>>;
}

impl PollSendToVectored for tokio::net::UdpSocket {
    fn poll_send_to_vectored(
        &self,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
        target: SocketAddr,
    ) -> Poll<std::io::Result<usize>> {
        let sref = SockRef::from(self);
        loop {
            match sref.send_to_vectored(bufs, &target.into()) {
                Ok(sz) => return Poll::Ready(Ok(sz)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::task::ready!(self.poll_send_ready(cx))?;
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }
}

impl PollSendToVectored for crate::UdpSocket {
    fn poll_send_to_vectored(
        &self,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
        target: SocketAddr,
    ) -> Poll<std::io::Result<usize>> {
        let target = self.convert_addr_for_send(target);
        self.socket().poll_send_to_vectored(cx, bufs, target)
    }
}

/// Object-safe datagram socket abstraction — the subset of `UdpSocket`
/// used by the DHT, UDP tracker clients and uTP transports. Allows
/// injecting a custom datagram transport (e.g. one routed through an
/// anonymity tunnel) instead of a real UDP socket.
///
/// Tribler-Rust-Torrent vendored patch.
pub trait DatagramSocket: Send + Sync + std::fmt::Debug + 'static {
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        target: SocketAddr,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<usize>> + Send + Sync + 'a>,
    >;

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<(usize, SocketAddr)>> + Send + Sync + 'a,
        >,
    >;

    /// The local address the transport is bound to. Used only for logging.
    fn bind_addr(&self) -> SocketAddr;
}

impl DatagramSocket for crate::UdpSocket {
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        target: SocketAddr,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<usize>> + Send + Sync + 'a>,
    > {
        Box::pin(crate::UdpSocket::send_to(self, buf, target))
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<(usize, SocketAddr)>> + Send + Sync + 'a,
        >,
    > {
        Box::pin(crate::UdpSocket::recv_from(self, buf))
    }

    fn bind_addr(&self) -> SocketAddr {
        crate::UdpSocket::bind_addr(self)
    }
}
