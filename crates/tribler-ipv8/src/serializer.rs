//! Serialiseur binaire pyipv8 (equivalent de `messaging/serialization.py`).
//!
//! Formats implementes (noms identiques au Python) :
//! - `B`, `H`, `I`, `Q`, `?` : entiers big-endian via `struct` ;
//! - `20s`, `32s`, `64s`, `c20s` : chaines d'octets fixes ;
//! - `varlenH`, `varlenI` : longueur `>H`/`>I` + donnees ;
//! - `varlenHx20` : `varlenH` dont la longueur est en multiples de 20 ;
//! - `ipv4` : `>4sH` (adresse IPv4 + port, 6 octets) ;
//! - `ip_address` : type (`0x01`=IPv4, `0x03`=IPv6, `0x02`=domaine) +
//!   donnees ;
//! - `bits` : 8 booleens dans 1 octet ;
//! - `raw` : le reste du buffer.

use std::net::{Ipv4Addr, Ipv6Addr};

use crate::address::UdpAddress;
use crate::error::Ipv8Error;

/// Type d'adresse `ip_address` : IPv4 (cf. `ADDRESS_TYPE_IPV4`).
pub const ADDRESS_TYPE_IPV4: u8 = 0x01;
/// Domaine.
pub const ADDRESS_TYPE_DOMAIN_NAME: u8 = 0x02;
/// IPv6.
pub const ADDRESS_TYPE_IPV6: u8 = 0x03;

/// Lecteur borne sur un buffer (equivalent des `Packer.unpack`).
pub struct Reader<'a> {
    data: &'a [u8],
    /// Position courante.
    pub offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Octets restants.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    /// Lit `n` octets bruts.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Ipv8Error> {
        if self.remaining() < n {
            return Err(Ipv8Error::Truncated {
                need: n,
                have: self.remaining(),
            });
        }
        let s = &self.data[self.offset..self.offset + n];
        self.offset += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, Ipv8Error> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Ipv8Error> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32, Ipv8Error> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64, Ipv8Error> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// `varlenH` : `>H` + donnees.
    pub fn varlen_h(&mut self) -> Result<&'a [u8], Ipv8Error> {
        let n = self.u16()? as usize;
        self.take(n)
    }

    /// `varlenHx20` : longueur en multiples de 20 octets.
    pub fn varlen_h_x20(&mut self) -> Result<Vec<[u8; 20]>, Ipv8Error> {
        let raw = self.varlen_h()?;
        if !raw.len().is_multiple_of(20) {
            return Err(Ipv8Error::Malformed("varlenHx20 non multiple de 20"));
        }
        Ok(raw.as_chunks::<20>().0.iter().copied().collect())
    }

    /// `ipv4` : `>4sH` (6 octets).
    pub fn ipv4(&mut self) -> Result<UdpAddress, Ipv8Error> {
        let ip = Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).unwrap());
        let port = self.u16()?;
        Ok(UdpAddress::Ipv4(std::net::SocketAddrV4::new(ip, port)))
    }

    /// `ip_address` : type + donnees (IPv4/IPv6/domaine).
    pub fn ip_address(&mut self) -> Result<UdpAddress, Ipv8Error> {
        match self.u8()? {
            ADDRESS_TYPE_IPV4 => {
                let ip = Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).unwrap());
                let port = self.u16()?;
                Ok(UdpAddress::Ipv4(std::net::SocketAddrV4::new(ip, port)))
            }
            ADDRESS_TYPE_IPV6 => {
                let ip = Ipv6Addr::from(<[u8; 16]>::try_from(self.take(16)?).unwrap());
                let port = self.u16()?;
                Ok(UdpAddress::Ipv6(std::net::SocketAddrV6::new(
                    ip, port, 0, 0,
                )))
            }
            ADDRESS_TYPE_DOMAIN_NAME => {
                let len = self.u16()? as usize;
                let host = std::str::from_utf8(self.take(len)?)
                    .map_err(|_| Ipv8Error::Malformed("nom de domaine non-UTF8"))?
                    .to_string();
                let port = self.u16()?;
                Ok(UdpAddress::Domain(host, port))
            }
            _ => Err(Ipv8Error::Malformed("type d'adresse inconnu")),
        }
    }

    /// `bits` : 8 booleens dans 1 octet (MSB first).
    pub fn bits(&mut self) -> Result<[bool; 8], Ipv8Error> {
        let b = self.u8()?;
        Ok([
            b & 0x80 != 0,
            b & 0x40 != 0,
            b & 0x20 != 0,
            b & 0x10 != 0,
            b & 0x08 != 0,
            b & 0x04 != 0,
            b & 0x02 != 0,
            b & 0x01 != 0,
        ])
    }

    /// `raw` : le reste.
    pub fn raw(&mut self) -> &'a [u8] {
        let s = &self.data[self.offset..];
        self.offset = self.data.len();
        s
    }
}

/// Ecrivain (equivalent des `Packer.pack`).
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn bytes(&mut self, data: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(data);
        self
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_be_bytes())
    }

    /// `varlenH`.
    pub fn varlen_h(&mut self, data: &[u8]) -> &mut Self {
        self.u16(data.len() as u16);
        self.bytes(data)
    }

    /// `varlenHx20` (longueur en unites de 20 octets).
    pub fn varlen_h_x20(&mut self, items: &[[u8; 20]]) -> &mut Self {
        self.u16(items.len() as u16);
        for it in items {
            self.bytes(it);
        }
        self
    }

    /// `ipv4` : `>4sH` (echoue silencieusement si non-IPv4 — comme le
    /// Python `ip_only`, on n'emet ce format que pour IPv4).
    pub fn ipv4(&mut self, addr: &UdpAddress) -> Result<&mut Self, Ipv8Error> {
        match addr {
            UdpAddress::Ipv4(sa) => {
                self.bytes(&sa.ip().octets());
                self.u16(sa.port());
                Ok(self)
            }
            _ => Err(Ipv8Error::Malformed(
                "format ipv4 utilise pour une adresse non-IPv4",
            )),
        }
    }

    /// `ip_address` : type + donnees.
    pub fn ip_address(&mut self, addr: &UdpAddress) -> Result<&mut Self, Ipv8Error> {
        match addr {
            UdpAddress::Ipv4(sa) => {
                self.u8(ADDRESS_TYPE_IPV4);
                self.bytes(&sa.ip().octets());
                self.u16(sa.port());
            }
            UdpAddress::Ipv6(sa) => {
                self.u8(ADDRESS_TYPE_IPV6);
                self.bytes(&sa.ip().octets());
                self.u16(sa.port());
            }
            UdpAddress::Domain(host, port) => {
                self.u8(ADDRESS_TYPE_DOMAIN_NAME);
                self.u16(host.len() as u16);
                self.bytes(host.as_bytes());
                self.u16(*port);
            }
        }
        Ok(self)
    }

    /// `bits` : 8 booleens MSB-first.
    pub fn bits(&mut self, bits: [bool; 8]) -> &mut Self {
        let mut b = 0u8;
        for (i, bit) in bits.iter().enumerate() {
            if *bit {
                b |= 0x80 >> i;
            }
        }
        self.u8(b)
    }

    /// `raw`.
    pub fn raw(&mut self, data: &[u8]) -> &mut Self {
        self.bytes(data)
    }
}
