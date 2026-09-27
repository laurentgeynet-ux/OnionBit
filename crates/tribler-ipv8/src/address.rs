//! Adresses de pairs IPv8 (equivalent de
//! `messaging/interfaces/udp/endpoint.py` : `UDPv4Address`,
//! `UDPv6Address`, `DomainAddress`).

use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

/// Adresse d'un pair : IPv4, IPv6 ou nom de domaine.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UdpAddress {
    /// `UDPv4Address`.
    Ipv4(SocketAddrV4),
    /// `UDPv6Address`.
    Ipv6(SocketAddrV6),
    /// `DomainAddress`.
    Domain(String, u16),
}

impl UdpAddress {
    /// Convertit en `SocketAddr` si l'adresse est numerique.
    pub fn to_socket_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Ipv4(a) => Some(SocketAddr::V4(*a)),
            Self::Ipv6(a) => Some(SocketAddr::V6(*a)),
            Self::Domain(..) => None,
        }
    }

    /// `true` si c'est une adresse IPv4 "0.0.0.0:0" (placeholder des
    /// messages d'introduction).
    pub fn is_unspecified(&self) -> bool {
        match self {
            Self::Ipv4(a) => a.ip().is_unspecified() || a.port() == 0,
            Self::Ipv6(a) => a.ip().is_unspecified() || a.port() == 0,
            Self::Domain(h, p) => h.is_empty() || *p == 0,
        }
    }
}

impl From<SocketAddr> for UdpAddress {
    fn from(sa: SocketAddr) -> Self {
        match sa {
            SocketAddr::V4(a) => Self::Ipv4(a),
            SocketAddr::V6(a) => Self::Ipv6(a),
        }
    }
}
