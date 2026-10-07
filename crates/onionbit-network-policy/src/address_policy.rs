// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Politique d'adresses IP — socle anti-SSRF et garde-fou SOCKS5.
//!
//! Toute destination IP atteignable a cause d'une entree controlee
//! par un tiers (URL de tracker, flux RSS, liste de canaux, cible
//! `CONNECT` d'un client SOCKS5) doit passer par [`IpPolicy::check`]
//! avant d'ouvrir la moindre connexion.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::error::{PolicyError, Result};
use crate::PolicyDecision;

/// Politique d'adresses : quelles classes d'IP sont joignables.
///
/// Les sept categories refusables couvrent les plages speciales
/// IPv4/IPv6 usuelles ; `allowed_ports` borne en plus le port (ex.
/// n'accepter que 80/443 pour du fetch de metadonnees).
#[derive(Debug, Clone)]
pub struct IpPolicy {
    /// Autorise loopback (`127.0.0.0/8`, `::1`). Desactive en
    /// production ; les tests locaux l'activent.
    pub allow_loopback: bool,
    /// Autorise les plages privees RFC1918 + CGNAT + ULA
    /// (`10/8`, `172.16/12`, `192.168/16`, `100.64/10`, `fc00::/7`).
    pub allow_private: bool,
    /// Autorise link-local (`169.254/16`, `fe80::/10`) et l'adresse
    /// de documentation/reservee.
    pub allow_link_local: bool,
    /// Autorise multicast (`224.0.0.0/4`, `ff00::/8`).
    pub allow_multicast: bool,
    /// Autorise unspecified (`0.0.0.0`, `::`).
    pub allow_unspecified: bool,
    /// Autorise les plages "reserved"/documentation et broadcast
    /// (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`,
    /// `240/4`, `255.255.255.255`, `2001:db8::/32`...).
    pub allow_reserved: bool,
    /// Ports autorises (`None` = tous).
    pub allowed_ports: Option<std::ops::RangeInclusive<u16>>,
}

impl IpPolicy {
    /// Politique stricte de production : uniquement des adresses
    /// publiques routables.
    pub fn strict() -> Self {
        Self {
            allow_loopback: false,
            allow_private: false,
            allow_link_local: false,
            allow_multicast: false,
            allow_unspecified: false,
            allow_reserved: false,
            allowed_ports: None,
        }
    }

    /// Politique de test : tout autorise (loopback compris) — pour les
    /// tests d'integration sur `127.0.0.1`.
    pub fn permissive() -> Self {
        Self {
            allow_loopback: true,
            allow_private: true,
            allow_link_local: true,
            allow_multicast: true,
            allow_unspecified: true,
            allow_reserved: true,
            allowed_ports: None,
        }
    }

    /// Evalue une adresse IP : `Allow` ou `Deny`.
    pub fn decide(&self, ip: &IpAddr) -> PolicyDecision {
        match self.deny_reason(ip) {
            Some(_) => PolicyDecision::Deny,
            None => PolicyDecision::Allow,
        }
    }

    /// Evalue `ip` et renvoie la raison du refus (`None` si autorise).
    pub fn deny_reason(&self, ip: &IpAddr) -> Option<&'static str> {
        match ip {
            IpAddr::V4(v4) => self.deny_reason_v4(v4),
            IpAddr::V6(v6) => self.deny_reason_v6(v6),
        }
    }

    fn deny_reason_v4(&self, ip: &Ipv4Addr) -> Option<&'static str> {
        let o = ip.octets();
        if !self.allow_unspecified && ip.is_unspecified() {
            return Some("adresse non specifiee");
        }
        if !self.allow_loopback && ip.is_loopback() {
            return Some("loopback refuse");
        }
        if !self.allow_multicast && ip.is_multicast() {
            return Some("multicast refuse");
        }
        if !self.allow_link_local && ip.is_link_local() {
            return Some("link-local refuse");
        }
        if !self.allow_private
            && (ip.is_private()
            // CGNAT 100.64.0.0/10
            || (o[0] == 100 && (o[1] & 0xC0) == 64))
        {
            return Some("plage privee/CGNAT refusee");
        }
        if !self.allow_reserved
            && (
                // documentation RFC5737 + IETF
                (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                // "this network" 0.0.0.0/8 (hors unspecified deja vu)
                || o[0] == 0
                // reserved 240.0.0.0/4 + broadcast
                || o[0] >= 240
            )
        {
            return Some("plage reservee refusee");
        }
        None
    }

    fn deny_reason_v6(&self, ip: &Ipv6Addr) -> Option<&'static str> {
        // IPv4-mapped : on applique la politique IPv4.
        if let Some(v4) = ip.to_ipv4_mapped() {
            return self.deny_reason_v4(&v4);
        }
        let s = ip.segments();
        if !self.allow_unspecified && ip.is_unspecified() {
            return Some("adresse non specifiee");
        }
        if !self.allow_loopback && ip.is_loopback() {
            return Some("loopback refuse");
        }
        if !self.allow_multicast && ip.is_multicast() {
            return Some("multicast refuse");
        }
        // IPv4-compatible `::/96` (deprecie RFC4291, mais encore
        // acceptee par certaines piles/clients HTTP) : sans ce cas,
        // `::127.0.0.1` franchissait tous les filtres v6 — on applique
        // la politique IPv4 aux 32 derniers bits. `::` et `::1` sont
        // deja traites par les checks unspecified/loopback plus haut
        // (leurs raisons propres sont conservees).
        if s[0] == 0
            && s[1] == 0
            && s[2] == 0
            && s[3] == 0
            && s[4] == 0
            && s[5] == 0
            && !ip.is_unspecified()
            && !ip.is_loopback()
        {
            let v4 = Ipv4Addr::new(
                (s[6] >> 8) as u8,
                (s[6] & 0xff) as u8,
                (s[7] >> 8) as u8,
                (s[7] & 0xff) as u8,
            );
            return self.deny_reason_v4(&v4);
        }
        if !self.allow_private && (s[0] & 0xfe00) == 0xfc00 {
            return Some("ULA fc00::/7 refusee");
        }
        if !self.allow_link_local && (s[0] & 0xffc0) == 0xfe80 {
            return Some("link-local fe80::/10 refuse");
        }
        if !self.allow_reserved
            && (
                // documentation 2001:db8::/32
                (s[0] == 0x2001 && s[1] == 0x0db8)
                // discards 100::/64
                || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0)
                // 6to4/teredo
                || s[0] == 0x2002
                || s[0] == 0x2001 && s[1] == 0
            )
        {
            return Some("plage reservee refusee");
        }
        None
    }

    /// Valide une destination `host:port` complete (IP + port).
    pub fn check(&self, addr: &SocketAddr) -> Result<()> {
        if let Some(reason) = self.deny_reason(&addr.ip()) {
            return Err(PolicyError::DeniedDestination(reason));
        }
        if let Some(ports) = &self.allowed_ports {
            if !ports.contains(&addr.port()) {
                return Err(PolicyError::DeniedDestination("port hors politique"));
            }
        }
        Ok(())
    }

    /// Equivalent [`Self::check`] pour un `IpAddr` sans port.
    pub fn check_ip(&self, ip: &IpAddr) -> Result<()> {
        match self.deny_reason(ip) {
            Some(reason) => Err(PolicyError::DeniedDestination(reason)),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_refuse_les_plages_speciales_ipv4() {
        let p = IpPolicy::strict();
        for s in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "192.0.2.1",
            "240.1.2.3",
            "255.255.255.255",
        ] {
            let ip: IpAddr = s.parse().unwrap();
            assert_eq!(
                p.decide(&ip),
                PolicyDecision::Deny,
                "{s} devrait etre refuse"
            );
        }
        assert_eq!(p.decide(&"8.8.8.8".parse().unwrap()), PolicyDecision::Allow);
    }

    #[test]
    fn strict_refuse_ipv6_special_et_mappe() {
        let p = IpPolicy::strict();
        for s in [
            "::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            // IPv4-compatible ::/96 : la politique v4 s'applique aux
            // 32 derniers bits (loopback, prive, reserve refusees).
            "::127.0.0.1",
            "::10.0.0.1",
            "::0.0.0.1",
            "2001:db8::1",
        ] {
            let ip: IpAddr = s.parse().unwrap();
            assert_eq!(
                p.decide(&ip),
                PolicyDecision::Deny,
                "{s} devrait etre refuse"
            );
        }
        assert_eq!(
            p.decide(&"2001:4860:4860::8888".parse().unwrap()),
            PolicyDecision::Allow
        );
        // `::8.8.8.8` = IPv4-compatible publique → traitee comme v4.
        assert_eq!(
            p.decide(&"::8.8.8.8".parse().unwrap()),
            PolicyDecision::Allow
        );
        // En mode permissif `::1` reste autorise (loopback explicite)
        // alors que `::127.0.0.1` suit le v4 loopback — meme verdict.
        let permissive = IpPolicy::permissive();
        assert_eq!(
            permissive.decide(&"::1".parse().unwrap()),
            PolicyDecision::Allow
        );
        assert_eq!(
            permissive.decide(&"::127.0.0.1".parse().unwrap()),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn check_valide_ip_et_port() {
        let mut p = IpPolicy::strict();
        p.allowed_ports = Some(80..=443);
        assert!(p.check(&"8.8.8.8:443".parse().unwrap()).is_ok());
        assert!(p.check(&"8.8.8.8:444".parse().unwrap()).is_err());
        assert!(p.check(&"127.0.0.1:80".parse().unwrap()).is_err());
    }

    #[test]
    fn permissive_accepte_loopback() {
        let p = IpPolicy::permissive();
        assert_eq!(
            p.decide(&"127.0.0.1".parse().unwrap()),
            PolicyDecision::Allow
        );
    }
}
