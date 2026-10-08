// This file is part of OnionBit.
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

/// Traduction des codes de retour d'erreur RFC 6886.
fn rfc6886_result_desc(code: u16) -> &'static str {
    match code {
        0 => "Succès",
        1 => "Version protocole non supportée",
        2 => "Non autorisé / refusé par la passerelle",
        3 => "Échec réseau de la passerelle",
        4 => "Ressources épuisées sur la passerelle",
        5 => "Opcode non supporté",
        _ => "Code retour inconnu",
    }
}

/// Parametres d'une requete de mapping (`opcode` : 1 = UDP, 2 = TCP).
#[derive(Debug, Clone, Copy)]
struct MappingSpec {
    opcode: u8,
    internal_port: u16,
    suggested_ext_port: u16,
    lifetime: u32,
}

/// Envoie une requête de mapping de port selon RFC 6886.
async fn send_mapping_request(
    socket: &UdpSocket,
    gateway: Ipv4Addr,
    dest_port: u16,
    spec: MappingSpec,
    timeout: Duration,
) -> Option<NatPmpMappingResult> {
    let proto = if spec.opcode == 1 { "UDP" } else { "TCP" };
    let mut packet = [0u8; 12];
    packet[0] = 0; // Version 0 (RFC 6886)
    packet[1] = spec.opcode; // 1 = UDP, 2 = TCP
    packet[2] = 0; // Réservé
    packet[3] = 0;
    packet[4..6].copy_from_slice(&spec.internal_port.to_be_bytes());
    packet[6..8].copy_from_slice(&spec.suggested_ext_port.to_be_bytes());
    packet[8..12].copy_from_slice(&spec.lifetime.to_be_bytes());

    let dest = SocketAddr::V4(SocketAddrV4::new(gateway, dest_port));
    if let Err(e) = socket.send_to(&packet, dest).await {
        tracing::warn!(gateway = %gateway, port = spec.internal_port, proto, error = %e, "NAT-PMP : échec de l'envoi du paquet UDP vers la passerelle");
        return None;
    }

    // Boucle a deadline : un datagramme parasite ou une reponse
    // tardive d'une sonde precedente ne doit pas abandonner
    // l'attente — on l'ignore et on consomme le reste du delai.
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = [0u8; 64];
    loop {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remain.is_zero() {
            tracing::debug!(
                gateway = %gateway,
                proto,
                internal_port = spec.internal_port,
                timeout_secs = timeout.as_secs(),
                "NAT-PMP : pas de réponse sur UDP 5351 (délai dépassé / passerelle muette)"
            );
            return None;
        }
        match tokio::time::timeout(remain, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, src)))
                if len >= 16 && src.ip() == IpAddr::V4(gateway) && buf[1] == 128 + spec.opcode =>
            {
                // Verdict de la passerelle pour CETTE sonde (pas de
                // nonce RFC 6886 : source + opcode est le seul
                // moyen de la correler).
                let result_code = u16::from_be_bytes([buf[2], buf[3]]);
                if result_code == 0 {
                    let resp_int = u16::from_be_bytes([buf[8], buf[9]]);
                    let resp_ext = u16::from_be_bytes([buf[10], buf[11]]);
                    let resp_lifetime = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
                    tracing::info!(
                        gateway = %gateway,
                        proto,
                        internal_port = resp_int,
                        mapped_external_port = resp_ext,
                        lifetime_secs = resp_lifetime,
                        "NAT-PMP : port mappé avec succès sur la passerelle (code 0: Succès)"
                    );
                    return Some(NatPmpMappingResult {
                        internal_port: resp_int,
                        mapped_external_port: resp_ext,
                        lifetime_secs: resp_lifetime,
                    });
                }
                tracing::warn!(
                    gateway = %gateway,
                    proto,
                    internal_port = spec.internal_port,
                    result_code,
                    description = rfc6886_result_desc(result_code),
                    "NAT-PMP : la passerelle a refusé le mappage de port"
                );
                return None;
            }
            Ok(Ok((len, src))) => {
                // Source etrangere, taille insuffisante ou opcode
                // d'une sonde precedente — on consomme le delai.
                tracing::debug!(
                    gateway = %gateway,
                    from = %src,
                    len,
                    "NAT-PMP : paquet ignore, attente poursuivie"
                );
            }
            Ok(Err(e)) => {
                tracing::warn!(gateway = %gateway, proto, error = %e, "NAT-PMP : erreur de réception sur socket");
                return None;
            }
            Err(_) => {
                tracing::debug!(
                    gateway = %gateway,
                    proto,
                    internal_port = spec.internal_port,
                    timeout_secs = timeout.as_secs(),
                    "NAT-PMP : pas de réponse sur UDP 5351 (délai dépassé / passerelle muette)"
                );
                return None;
            }
        }
    }
}

/// Tâche d'arrière-plan gérant l'ouverture et le renouvellement périodique NAT-PMP.
pub async fn run_natpmp_forwarder(
    port: u16,
    config: NatPmpConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await else {
        tracing::warn!("NAT-PMP : impossible d'ouvrir un socket UDP client");
        return;
    };

    let gateways = guess_gateway_candidates();
    let timeout = Duration::from_secs(config.timeout_secs);
    let mut active_gateway: Option<Ipv4Addr> = None;

    tracing::info!(
        port,
        candidates = ?gateways,
        "NAT-PMP : démarrage du forwarder, recherche d'une passerelle compatible"
    );

    // Phase 1 : Détection de la passerelle répondant en NAT-PMP
    for &gw in &gateways {
        if *shutdown.borrow() {
            return;
        }
        // Test sur UDP d'abord
        let spec = MappingSpec {
            opcode: 1,
            internal_port: port,
            suggested_ext_port: port,
            lifetime: config.lifetime_secs,
        };
        if send_mapping_request(&socket, gw, NAT_PMP_PORT, spec, timeout)
            .await
            .is_some()
        {
            // Mappe également TCP
            let tcp = MappingSpec { opcode: 2, ..spec };
            let _ = send_mapping_request(&socket, gw, NAT_PMP_PORT, tcp, timeout).await;
            active_gateway = Some(gw);
            break;
        }
    }

    let Some(gw) = active_gateway else {
        tracing::info!(
            port,
            "NAT-PMP : aucune passerelle locale n'a répondu sur UDP 5351 (passerelle incompatible ou NAT-PMP désactivé, UPnP prend le relais)"
        );
        return;
    };

    // Phase 2 : Renouvellement périodique avant expiration
    let renew_interval = Duration::from_secs((config.lifetime_secs / 2).max(60) as u64);
    loop {
        tokio::select! {
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    // Phase 3 : Libération propre des ports à l'arrêt (lifetime = 0)
                    let release = MappingSpec {
                        opcode: 1,
                        internal_port: port,
                        suggested_ext_port: port,
                        lifetime: 0,
                    };
                    let short = Duration::from_millis(500);
                    let _ = send_mapping_request(&socket, gw, NAT_PMP_PORT, release, short).await;
                    let tcp = MappingSpec { opcode: 2, ..release };
                    let _ = send_mapping_request(&socket, gw, NAT_PMP_PORT, tcp, short).await;
                    tracing::debug!(gateway = %gw, port, "NAT-PMP : mappings de port liberes a l'arret");
                    return;
                }
            }
            _ = tokio::time::sleep(renew_interval) => {
                // Renouvellement UDP et TCP
                let spec = MappingSpec {
                    opcode: 1,
                    internal_port: port,
                    suggested_ext_port: port,
                    lifetime: config.lifetime_secs,
                };
                let _ = send_mapping_request(&socket, gw, NAT_PMP_PORT, spec, timeout).await;
                let tcp = MappingSpec { opcode: 2, ..spec };
                let _ = send_mapping_request(&socket, gw, NAT_PMP_PORT, tcp, timeout).await;
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

    /// Un datagramme parasite (reponse tardive, autre source) ne doit
    /// pas abandonner l'attente : la deadline court toujours.
    #[tokio::test]
    async fn paquet_parasite_n_interrompt_pas_l_attente() {
        let mock_gw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let gw_addr = mock_gw.local_addr().unwrap();
        let IpAddr::V4(gw_ip) = gw_addr.ip() else {
            panic!("mock gw v4")
        };
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_addr = client.local_addr().unwrap();
        let stray = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let task = tokio::spawn(async move {
            let mut req = [0u8; 12];
            let (len, from) = mock_gw.recv_from(&mut req).await.unwrap();
            assert_eq!(len, 12);
            assert_eq!(req[1], 1);
            // Laisse le parasite arriver avant la vraie reponse.
            tokio::time::sleep(Duration::from_millis(80)).await;
            let int_port = u16::from_be_bytes([req[4], req[5]]);
            let mut resp = [0u8; 16];
            resp[1] = 128 + 1;
            resp[8..10].copy_from_slice(&int_port.to_be_bytes());
            resp[10..12].copy_from_slice(&(int_port + 100).to_be_bytes());
            resp[12..16].copy_from_slice(&3600u32.to_be_bytes());
            mock_gw.send_to(&resp, from).await.unwrap();
        });

        // Le parasite part avant la reponse de la passerelle.
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = stray.send_to(&[0xAAu8; 20], client_addr).await;
        });

        let spec = MappingSpec {
            opcode: 1,
            internal_port: 6881,
            suggested_ext_port: 6881,
            lifetime: 3600,
        };
        let res =
            send_mapping_request(&client, gw_ip, gw_addr.port(), spec, Duration::from_secs(5))
                .await;
        assert_eq!(res.map(|m| m.mapped_external_port), Some(6981));
        task.await.unwrap();
    }
}
