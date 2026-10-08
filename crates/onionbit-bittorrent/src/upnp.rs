// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Forwarder UPnP IGD pour le port UDP (uTP) de la session BitTorrent
//! en clair (`hops == 0`) — parite `libtorrent/upnp` Tribler.
//!
//! `enable_upnp_port_forwarding` de librqbit (`UpnpPortForwarder`) ne
//! demande que `<NewProtocol>TCP</NewProtocol>` — le mapping UDP du
//! port d'ecoute (uTP) n'existe pas, alors que libtorrent mappe les
//! deux protocoles. Ce module comble l'ecart : la decouverte SSDP et
//! le parsing IGD sont reutilises de `librqbit-upnp`, seul le SOAP
//! `AddPortMapping`/`DeletePortMapping` en `UDP` est local.
//!
//! Comme le forwarder TCP : mapping externe = interne, bail renouvele
//! periodiquement ; en plus libtorrent relache le mapping a l'arret —
//! `DeletePortMapping` est donc envoye sur le signal de shutdown.

use std::collections::HashSet;
use std::net::IpAddr;
use std::time::Duration;

use network_interface::NetworkInterfaceConfig;

use tokio::sync::watch;
use url::Url;

/// Service IGD ouvrant les mappings de ports (`WANIPConnection:1`).
const WAN_IP_CONNECTION: &str = librqbit_upnp::SSDP_SEARCH_WAN_IPCONNECTION_ST;

/// Description du mapping publiee sur la passerelle.
const MAPPING_DESCRIPTION: &str = "OnionBit";

/// Configuration du forwarder UPnP UDP.
#[derive(Debug, Clone)]
pub struct UpnpForwardConfig {
    /// Duree de bail demandee a la passerelle en secondes
    /// (defaut : 3600 s = 1 h, comme `NatPmpConfig::lifetime_secs`).
    pub lease_secs: u64,
    /// Fenetre d'ecoute des reponses SSDP `M-SEARCH` (s).
    pub discover_timeout_secs: u64,
    /// Delai avant une nouvelle decouverte quand aucune passerelle
    /// n'a repondu ou que tous les mappings ont echoue (s).
    pub discover_interval_secs: u64,
}

impl Default for UpnpForwardConfig {
    fn default() -> Self {
        Self {
            lease_secs: 3600,
            discover_timeout_secs: 3,
            discover_interval_secs: 60,
        }
    }
}

/// Cible de mapping : URL de controle WANIPConnection d'une
/// passerelle + IP locale joignant cette passerelle.
#[derive(Debug)]
struct MappingTarget {
    control_url: Url,
    local_ip: IpAddr,
}

/// Corps SOAP `AddPortMapping` (IGD v1) pour le protocole donne.
fn add_mapping_body(proto: &str, local_ip: IpAddr, port: u16, lease_secs: u64) -> String {
    format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
    s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
    <s:Body>
        <u:AddPortMapping xmlns:u="{WAN_IP_CONNECTION}">
            <NewRemoteHost></NewRemoteHost>
            <NewExternalPort>{port}</NewExternalPort>
            <NewProtocol>{proto}</NewProtocol>
            <NewInternalPort>{port}</NewInternalPort>
            <NewInternalClient>{local_ip}</NewInternalClient>
            <NewEnabled>1</NewEnabled>
            <NewPortMappingDescription>{MAPPING_DESCRIPTION}</NewPortMappingDescription>
            <NewLeaseDuration>{lease_secs}</NewLeaseDuration>
        </u:AddPortMapping>
    </s:Body>
</s:Envelope>"#
    )
}

/// Corps SOAP `DeletePortMapping` (IGD v1) — liberation propre du
/// mapping a l'arret (libtorrent supprime ses mappings en se fermant).
fn delete_mapping_body(proto: &str, port: u16) -> String {
    format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
    s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
    <s:Body>
        <u:DeletePortMapping xmlns:u="{WAN_IP_CONNECTION}">
            <NewRemoteHost></NewRemoteHost>
            <NewExternalPort>{port}</NewExternalPort>
            <NewProtocol>{proto}</NewProtocol>
        </u:DeletePortMapping>
    </s:Body>
</s:Envelope>"#
    )
}

/// POST SOAP vers l'URL de controle IGD ; `Err` sur statut non-2xx.
async fn soap_post(
    client: &reqwest::Client,
    control_url: &Url,
    action: &str,
    body: String,
) -> crate::Result<()> {
    let response = client
        .post(control_url.clone())
        .header("Content-Type", "text/xml")
        .header("SOAPAction", format!("\"{WAN_IP_CONNECTION}#{action}\""))
        .body(body)
        .send()
        .await
        .map_err(|e| crate::BtError::Engine(format!("upnp {action} : {e}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    tracing::trace!(%status, %text, "reponse SOAP {action}");
    if !status.is_success() {
        return Err(crate::BtError::Engine(format!(
            "upnp {action} refuse par la passerelle : {status}"
        )));
    }
    Ok(())
}

/// `AddPortMapping` UDP — ouvre ou renouvelle le bail du port.
async fn add_udp_mapping(
    client: &reqwest::Client,
    target: &MappingTarget,
    port: u16,
    lease_secs: u64,
) -> crate::Result<()> {
    soap_post(
        client,
        &target.control_url,
        "AddPortMapping",
        add_mapping_body("UDP", target.local_ip, port, lease_secs),
    )
    .await
}

/// `DeletePortMapping` UDP — libere le mapping a l'arret.
async fn delete_udp_mapping(client: &reqwest::Client, control_url: &Url, port: u16) {
    let _ = soap_post(
        client,
        control_url,
        "DeletePortMapping",
        delete_mapping_body("UDP", port),
    )
    .await;
}

/// URLs de controle `WANIPConnection` d'un root device IGD (equivalent
/// du `get_wan_ip_control_urls` prive de `librqbit-upnp` — le device
/// IGD peut imbriquer `WANDevice`/`WANConnectionDevice`).
fn wan_ip_control_urls(desc: &librqbit_upnp::RootDesc, location: &Url) -> Vec<Url> {
    desc.devices
        .iter()
        .flat_map(|d| d.iter_services(tracing::Span::none()))
        .filter(|(_, s)| s.service_type == WAN_IP_CONNECTION)
        .filter_map(|(_, s)| location.join(&s.control_url).ok())
        .collect()
}

/// Decouverte SSDP + resolution des cibles de mapping (URL de
/// controle + IP locale associee), dedoublonnees par URL.
async fn discover_targets(timeout: Duration) -> Vec<MappingTarget> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    if let Err(e) = librqbit_upnp::discover_once(&tx, WAN_IP_CONNECTION, timeout, None).await {
        tracing::warn!(error = %e, "UPnP : decouverte SSDP impossible");
        return Vec::new();
    }
    drop(tx);

    let Ok(nics) = network_interface::NetworkInterface::show() else {
        tracing::warn!("UPnP : enumeration des interfaces reseau impossible");
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    while let Ok(resp) = rx.try_recv() {
        let services = match librqbit_upnp::discover_services(resp.location.clone()).await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(location = %resp.location, error = %e, "UPnP : description IGD illisible");
                continue;
            }
        };
        let local_ip = match librqbit_upnp::get_local_ip_relative_to(resp.received_from, &nics) {
            Ok(ip) => ip,
            Err(e) => {
                tracing::debug!(from = %resp.received_from, error = %e, "UPnP : IP locale introuvable");
                continue;
            }
        };
        for control_url in wan_ip_control_urls(&services, &resp.location) {
            if seen.insert(control_url.clone()) {
                targets.push(MappingTarget {
                    control_url,
                    local_ip,
                });
            }
        }
    }
    targets
}

/// Tache d'arriere-plan : mapping UPnP UDP du port d'ecoute
/// BitTorrent, renouvellement du bail et liberation a l'arret.
/// Symetrique a [`crate::natpmp::run_natpmp_forwarder`].
pub async fn run_upnp_udp_forwarder(
    port: u16,
    config: UpnpForwardConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    // Pas de proxy pour les appels IGD : la passerelle est toujours
    // locale (un proxy HTTP d'entreprise casserait le SOAP).
    // Timeout borne : un IGD gele (miniupnpd crashe) bloquait la
    // tache forwarder indefiniment sur le POST SOAP.
    let client = match reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(config.discover_timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "UPnP : client HTTP impossible");
            return;
        }
    };
    let discover_timeout = Duration::from_secs(config.discover_timeout_secs);
    let discover_interval = Duration::from_secs(config.discover_interval_secs);
    let renew_interval = Duration::from_secs((config.lease_secs / 2).max(60));
    let mut targets: Vec<MappingTarget> = Vec::new();

    loop {
        if targets.is_empty() {
            // (Re)decouverte : aucune passerelle connue ou tous les
            // renouvellements ont echoue (changement de passerelle,
            // reboot de la box).
            for target in discover_targets(discover_timeout).await {
                match add_udp_mapping(&client, &target, port, config.lease_secs).await {
                    Ok(()) => {
                        tracing::info!(
                            gateway = %target.control_url,
                            port,
                            "UPnP : mapping UDP ouvert"
                        );
                        targets.push(target);
                    }
                    Err(e) => {
                        tracing::warn!(
                            gateway = %target.control_url,
                            port,
                            error = %e,
                            "UPnP : AddPortMapping UDP refuse"
                        );
                    }
                }
            }
        } else {
            // Renouvellement : un AddPortMapping identique prolonge le
            // bail. Une cible qui refuse tombe ; si toutes tombent on
            // repart en decouverte au prochain tour.
            let mut kept = Vec::with_capacity(targets.len());
            for target in std::mem::take(&mut targets) {
                if add_udp_mapping(&client, &target, port, config.lease_secs)
                    .await
                    .is_ok()
                {
                    kept.push(target);
                } else {
                    tracing::warn!(
                        gateway = %target.control_url,
                        port,
                        "UPnP : renouvellement du mapping UDP refuse"
                    );
                }
            }
            targets = kept;
        }

        let delay = if targets.is_empty() {
            discover_interval
        } else {
            renew_interval
        };
        tokio::select! {
            _ = shutdown.changed() => {
                for target in &targets {
                    delete_udp_mapping(&client, &target.control_url, port).await;
                }
                tracing::debug!(port, "UPnP : mapping UDP libere a l'arret");
                return;
            }
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serveur HTTP factice : capture la requete SOAP brute et repond
    /// 200. Retourne l'URL de controle et le corps recu.
    async fn fake_control_server() -> (Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = conn.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let marker = b"\r\n\r\n";
                if let Some(pos) = buf.windows(marker.len()).position(|w| w == marker) {
                    let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                    let len: usize = headers
                        .split("\r\n")
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if buf.len() >= pos + marker.len() + len {
                        break;
                    }
                }
            }
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        });
        (Url::parse(&format!("http://{addr}/ctl")).unwrap(), handle)
    }

    /// `AddPortMapping` UDP : le corps SOAP porte `NewProtocol` UDP et
    /// le meme port externe/interne (parite libtorrent).
    #[tokio::test]
    async fn add_mapping_en_udp() {
        let (url, server) = fake_control_server().await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let target = MappingTarget {
            control_url: url,
            local_ip: IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 42)),
        };
        add_udp_mapping(&client, &target, 6881, 3600).await.unwrap();
        let body = server.await.unwrap();
        assert!(body.contains("<NewProtocol>UDP</NewProtocol>"), "{body}");
        assert!(
            body.contains("<NewExternalPort>6881</NewExternalPort>"),
            "{body}"
        );
        assert!(
            body.contains("<NewInternalPort>6881</NewInternalPort>"),
            "{body}"
        );
        assert!(
            body.contains("<NewInternalClient>192.168.1.42</NewInternalClient>"),
            "{body}"
        );
        assert!(body.contains("AddPortMapping"), "{body}");
    }

    /// `DeletePortMapping` UDP : liberation du mapping a l'arret.
    #[tokio::test]
    async fn delete_mapping_en_udp() {
        let (url, server) = fake_control_server().await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        delete_udp_mapping(&client, &url, 6881).await;
        let body = server.await.unwrap();
        assert!(body.contains("DeletePortMapping"), "{body}");
        assert!(body.contains("<NewProtocol>UDP</NewProtocol>"), "{body}");
        assert!(
            body.contains("<NewExternalPort>6881</NewExternalPort>"),
            "{body}"
        );
    }

    /// Un statut non-2xx de la passerelle est une erreur (pas de
    /// mapping pretendu).
    #[tokio::test]
    async fn mapping_refuse_sur_500() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = conn.read(&mut buf).await;
            conn.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let target = MappingTarget {
            control_url: Url::parse(&format!("http://{addr}/ctl")).unwrap(),
            local_ip: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        };
        assert!(add_udp_mapping(&client, &target, 6881, 60).await.is_err());
    }

    /// Extraction des URLs de controle `WANIPConnection` dans un
    /// device IGD imbrique (WANDevice/WANConnectionDevice).
    #[test]
    fn controle_urls_device_imbrique() {
        use librqbit_upnp::{Device, DeviceList, RootDesc, Service, ServiceList};
        let svc = |st: &str, ctl: &str| Service {
            service_type: st.to_string(),
            control_url: ctl.to_string(),
            scpd_url: String::new(),
            event_sub_url: None,
        };
        let desc = RootDesc {
            devices: vec![Device {
                device_type: "urn:schemas-upnp-org:device:InternetGatewayDevice:1".into(),
                friendly_name: "box".into(),
                service_list: ServiceList {
                    services: vec![svc(
                        "urn:schemas-upnp-org:service:Layer3Forwarding:1",
                        "/l3f",
                    )],
                },
                device_list: DeviceList {
                    devices: vec![Device {
                        device_type: "urn:schemas-upnp-org:device:WANDevice:1".into(),
                        friendly_name: String::new(),
                        service_list: ServiceList::default(),
                        device_list: DeviceList {
                            devices: vec![Device {
                                device_type: "urn:schemas-upnp-org:device:WANConnectionDevice:1"
                                    .into(),
                                friendly_name: String::new(),
                                service_list: ServiceList {
                                    services: vec![svc(
                                        "urn:schemas-upnp-org:service:WANIPConnection:1",
                                        "/upnp/control/WANIPConnection0",
                                    )],
                                },
                                device_list: DeviceList::default(),
                            }],
                        },
                    }],
                },
            }],
        };
        let location = Url::parse("http://192.168.1.1:5000/rootdesc.xml").unwrap();
        let urls = wan_ip_control_urls(&desc, &location);
        assert_eq!(
            urls,
            vec![Url::parse("http://192.168.1.1:5000/upnp/control/WANIPConnection0").unwrap()]
        );
    }
}
