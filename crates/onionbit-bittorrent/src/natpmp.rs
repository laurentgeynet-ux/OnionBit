// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Client NAT-PMP (RFC 6886) pour l'ouverture automatique de ports
//! sur les routeurs supportant NAT-PMP / PCP (parité Tribler `start_natpmp`).
//!
//! Gère l'ouverture et le renouvellement des baux de ports TCP et UDP (uTP)
//! sur la session BitTorrent en clair (`hops == 0`), ainsi que leur libération
//! propre à l'arrêt (`lifetime = 0`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::watch;

/// Port standard du service NAT-PMP sur la passerelle locale (RFC 6886).
pub const NAT_PMP_PORT: u16 = 5351;

/// Configuration du client NAT-PMP.
#[derive(Debug, Clone)]
pub struct NatPmpConfig {
    /// Durée de bail demandée au routeur en secondes (défaut : 3600s = 1h).
    pub lifetime_secs: u32,
    /// Délai d'attente max d'une réponse de la passerelle en secondes.
    pub timeout_secs: u64,
}

impl Default for NatPmpConfig {
    fn default() -> Self {
        Self {
            lifetime_secs: 3600,
            timeout_secs: 3,
        }
    }
}

/// Statut d'un mapping de port retourné par la passerelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NatPmpMappingResult {
    /// Port interne demandé.
    pub internal_port: u16,
    /// Port externe alloué par la passerelle.
    pub mapped_external_port: u16,
    /// Durée du bail accordée en secondes.
    pub lifetime_secs: u32,
}

/// Détermine les adresses candidates de la passerelle par défaut.
fn guess_gateway_candidates() -> Vec<Ipv4Addr> {
    let mut candidates = Vec::new();

    // Détermine l'adresse IP locale utilisée pour router le trafic externe
    if let Ok(dummy) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if dummy.connect("1.1.1.1:80").is_ok() {
            if let Ok(SocketAddr::V4(local)) = dummy.local_addr() {
                let octets = local.ip().octets();
                // Passerelles conventionnelles sur les réseaux privés (/24) :
                // x.x.x.1 et x.x.x.254
                candidates.push(Ipv4Addr::new(octets[0], octets[1], octets[2], 1));
                candidates.push(Ipv4Addr::new(octets[0], octets[1], octets[2], 254));
            }
        }
    }

    // Passerelles locales standard courantes (fallback)
    let standard = [
        Ipv4Addr::new(192, 168, 1, 1),
        Ipv4Addr::new(192, 168, 0, 1),
        Ipv4Addr::new(10, 0, 0, 1),
    ];
    for gw in standard {
        if !candidates.contains(&gw) {
            candidates.push(gw);
        }
    }

    candidates
}

/// Envoie une requête de mapping de port selon RFC 6886.
/// `opcode`: 1 pour UDP, 2 pour TCP.
async fn send_mapping_request(
    socket: &UdpSocket,
    gateway: Ipv4Addr,
    opcode: u8,
    internal_port: u16,
    suggested_ext_port: u16,
    lifetime: u32,
    timeout: Duration,
) -> Option<NatPmpMappingResult> {
    let mut packet = [0u8; 12];
    packet[0] = 0; // Version
    packet[1] = opcode; // 1 = UDP, 2 = TCP
    packet[2] = 0; // Réservé
    packet[3] = 0;
    packet[4..6].copy_from_slice(&internal_port.to_be_bytes());
    packet[6..8].copy_from_slice(&suggested_ext_port.to_be_bytes());
    packet[8..12].copy_from_slice(&lifetime.to_be_bytes());

    let dest = SocketAddr::V4(SocketAddrV4::new(gateway, NAT_PMP_PORT));
    if socket.send_to(&packet, dest).await.is_err() {
        return None;
    }

    let mut buf = [0u8; 64];
    let res = tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await;
    match res {
        Ok(Ok((len, src))) if len >= 16 && src.ip() == IpAddr::V4(gateway) => {
            let resp_op = buf[1];
            let result_code = u16::from_be_bytes([buf[2], buf[3]]);
            if resp_op == 128 + opcode && result_code == 0 {
                let resp_int = u16::from_be_bytes([buf[8], buf[9]]);
                let resp_ext = u16::from_be_bytes([buf[10], buf[11]]);
                let resp_lifetime = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
                return Some(NatPmpMappingResult {
                    internal_port: resp_int,
                    mapped_external_port: resp_ext,
                    lifetime_secs: resp_lifetime,
                });
            }
            None
        }
        _ => None,
    }
}

/// Tâche d'arrière-plan gérant l'ouverture et le renouvellement périodique NAT-PMP.
pub async fn run_natpmp_forwarder(
    port: u16,
    config: NatPmpConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await else {
        tracing::debug!("NAT-PMP : impossible d'ouvrir un socket UDP client");
        return;
    };

    let gateways = guess_gateway_candidates();
    let timeout = Duration::from_secs(config.timeout_secs);
    let mut active_gateway: Option<Ipv4Addr> = None;

    // Phase 1 : Détection de la passerelle répondant en NAT-PMP
    for &gw in &gateways {
        if *shutdown.borrow() {
            return;
        }
        // Test sur UDP d'abord
        if let Some(res) =
            send_mapping_request(&socket, gw, 1, port, port, config.lifetime_secs, timeout).await
        {
            tracing::info!(
                gateway = %gw,
                port,
                mapped_port = res.mapped_external_port,
                lifetime = res.lifetime_secs,
                "NAT-PMP UDP : port mappe avec succes"
            );
            // Mappe également TCP
            let _ = send_mapping_request(&socket, gw, 2, port, port, config.lifetime_secs, timeout)
                .await;
            active_gateway = Some(gw);
            break;
        }
    }

    let Some(gw) = active_gateway else {
        tracing::debug!("NAT-PMP : aucune passerelle locale n'a repondu (passerelle incompatible ou desactivee)");
        return;
    };

    // Phase 2 : Renouvellement périodique avant expiration
    let renew_interval = Duration::from_secs((config.lifetime_secs / 2).max(60) as u64);
    loop {
        tokio::select! {
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    // Phase 3 : Libération propre des ports à l'arrêt (lifetime = 0)
                    let _ = send_mapping_request(&socket, gw, 1, port, port, 0, Duration::from_millis(500)).await;
                    let _ = send_mapping_request(&socket, gw, 2, port, port, 0, Duration::from_millis(500)).await;
                    tracing::debug!(gateway = %gw, port, "NAT-PMP : mappings de port liberes a l'arret");
                    return;
                }
            }
            _ = tokio::time::sleep(renew_interval) => {
                // Renouvellement UDP et TCP
                let _ = send_mapping_request(&socket, gw, 1, port, port, config.lifetime_secs, timeout).await;
                let _ = send_mapping_request(&socket, gw, 2, port, port, config.lifetime_secs, timeout).await;
                tracing::debug!(gateway = %gw, port, "NAT-PMP : renouvellement du bail de port effectue");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_passerelle_non_vides() {
        let candidates = guess_gateway_candidates();
        assert!(!candidates.is_empty());
    }

    #[tokio::test]
    async fn simulation_reponse_passerelle_natpmp() {
        // Fausse passerelle simulant le serveur NAT-PMP (RFC 6886)
        let mock_gw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let gw_addr = mock_gw.local_addr().unwrap();

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let task = tokio::spawn(async move {
            let mut req = [0u8; 12];
            let (len, from) = mock_gw.recv_from(&mut req).await.unwrap();
            assert_eq!(len, 12);
            assert_eq!(req[0], 0); // Vers
            assert_eq!(req[1], 1); // Opcode UDP
            let int_port = u16::from_be_bytes([req[4], req[5]]);

            // Reponse NAT-PMP (16 octets)
            let mut resp = [0u8; 16];
            resp[0] = 0; // Vers
            resp[1] = 128 + 1; // Reponse UDP
            resp[2] = 0; // Result Code = 0 (Success)
            resp[3] = 0;
            resp[4..8].copy_from_slice(&12345u32.to_be_bytes()); // Epoch
            resp[8..10].copy_from_slice(&int_port.to_be_bytes()); // Internal port
            resp[10..12].copy_from_slice(&(int_port + 100).to_be_bytes()); // Mapped ext port
            resp[12..16].copy_from_slice(&3600u32.to_be_bytes()); // Lifetime

            mock_gw.send_to(&resp, from).await.unwrap();
        });

        let mut packet = [0u8; 12];
        packet[0] = 0;
        packet[1] = 1;
        packet[4..6].copy_from_slice(&6881u16.to_be_bytes());
        packet[6..8].copy_from_slice(&6881u16.to_be_bytes());
        packet[8..12].copy_from_slice(&3600u32.to_be_bytes());

        client.send_to(&packet, gw_addr).await.unwrap();

        let mut buf = [0u8; 64];
        let (len, _) = client.recv_from(&mut buf).await.unwrap();
        assert_eq!(len, 16);
        assert_eq!(buf[1], 129); // 128 + 1
        let mapped_port = u16::from_be_bytes([buf[10], buf[11]]);
        assert_eq!(mapped_port, 6981);

        task.await.unwrap();
    }
}
