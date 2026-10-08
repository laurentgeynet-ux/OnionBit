// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handler `GET /api/connections` — **extension Rust** sans
//! equivalent `tribler.core.restapi` (le debug GUI Tribler agrege ces
//! donnees cote client depuis plusieurs endpoints).
//!
//! Vue agregee par adresse distante `ip:port` : pour chaque endpoint
//! connu du daemon, les roles/protocoles observes
//! (`"transports": ["udp","tcp",…]`, overlays IPv8, flags tunnel,
//! presence en bucket DHT, circuits de sortie l'ayant contactee,
//! connexions BitTorrent tcp/uTP/SOCKS) — plus `listeners`, les
//! sockets d'ecoute locales (endpoint IPv8 UDP, BitTorrent, SOCKS5
//! des lanes, sockets de sortie).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;

use onionbit_ipv8::overlays::addr_parts;

use crate::state::AppState;

/// Agregat mutable d'une adresse distante, rempli par les sondes
/// ci-dessous puis serialise.
#[derive(Default)]
struct ConnEntry {
    /// Pair verifie du `Network` IPv8 principal.
    ipv8: bool,
    /// `hexlify(peer.mid)` si pair IPv8.
    mid: Option<String>,
    /// Noms des overlays dont le pair est membre.
    overlays: BTreeSet<&'static str>,
    /// Noeud de la table de routage DHT.
    dht: bool,
    /// `PEER_FLAG_*` annonces (pair tunnel connu du flag registry).
    tunnel_flags: Vec<i32>,
    /// `circuit_id` des sockets de sortie ayant contacte cette
    /// adresse WAN (conntrack `contacted_sources`).
    exit_circuits: Vec<u32>,
    /// Connexions BitTorrent vers ce pair (une par torrent).
    bittorrent: Vec<serde_json::Value>,
}

impl ConnEntry {
    fn into_json(self, ip: String, port: u16) -> serde_json::Value {
        // Transports reels : UDP pour tout ce qui transite par
        // l'endpoint IPv8 / les sockets de sortie ; `conn_kind` rqbit
        // pour BitTorrent (`"socks"` = TCP vers le proxy local).
        let mut transports = BTreeSet::new();
        if self.ipv8 || self.dht || !self.tunnel_flags.is_empty() || !self.exit_circuits.is_empty()
        {
            transports.insert("udp");
        }
        for bt in &self.bittorrent {
            transports.insert(match bt["conn_kind"].as_str().unwrap_or("") {
                "tcp" => "tcp",
                // uTP est encapsule en datagrammes (y compris a travers
                // le tunnel sur les lanes anonymes).
                "uTP" => "udp",
                _ => "tcp",
            });
        }
        serde_json::json!({
            "ip": ip,
            "port": port,
            "transports": transports,
            "ipv8": self.ipv8,
            "mid": self.mid,
            "overlays": self.overlays,
            "dht": self.dht,
            "tunnel_flags": self.tunnel_flags,
            "exit_circuits": self.exit_circuits,
            "bittorrent": self.bittorrent,
        })
    }
}

/// Entree d'agregat d'une adresse distante, `None` pour les adresses
/// non resolues/absentes (`0.0.0.0:0` de `addr_parts`) — sans interet
/// comme endpoint distant.
fn conn_entry(
    conns: &mut BTreeMap<(String, u16), ConnEntry>,
    ip: String,
    port: u16,
) -> Option<&mut ConnEntry> {
    if port == 0 {
        return None;
    }
    Some(conns.entry((ip, port)).or_default())
}

/// `GET /api/connections` — agregat des endpoints distants connus et
/// des sockets d'ecoute locales. `{"connections": [], "listeners": []}`
/// en session sans stack IPv8 (les connexions BitTorrent restent
/// listees : moteur principal toujours actif).
pub async fn get_connections(State(state): State<AppState>) -> Response {
    let mut conns: BTreeMap<(String, u16), ConnEntry> = BTreeMap::new();
    let mut listeners: Vec<serde_json::Value> = Vec::new();

    if let Some(stack) = state.session.ipv8() {
        if let Ok(addr) = stack.endpoint.local_addr() {
            listeners.push(serde_json::json!({
                "protocol": "ipv8-udp", "address": addr.to_string(),
            }));
        }
        if let Some(Ok(addr)) = stack.endpoint.local_addr_v6() {
            listeners.push(serde_json::json!({
                "protocol": "ipv8-udp-v6", "address": addr.to_string(),
            }));
        }
        // Membership par overlay (`get_peers()` de chaque community).
        for info in stack.overlays_info() {
            for p in &info.peers {
                if let Some(e) = conn_entry(&mut conns, p.ip.clone(), p.port) {
                    e.overlays.insert(info.overlay_name);
                }
            }
        }
        // Pairs verifies du `Network` partage (mid + services).
        for peer in stack.network.verified_peers() {
            let (ip, port) = addr_parts(peer.address.as_ref());
            if let Some(e) = conn_entry(&mut conns, ip, port) {
                e.ipv8 = true;
                e.mid = Some(hex::encode(peer.mid));
            }
        }
        if let Some(tunnel) = &stack.tunnel {
            for tp in tunnel.tunnel_peers_info() {
                if let Some(e) = conn_entry(&mut conns, tp.ip, tp.port) {
                    e.tunnel_flags = tp.flags;
                }
            }
            // Destinations WAN contactees par nos sockets de sortie :
            // endpoints UDP reels (autres que des pairs IPv8).
            for (cid, sources) in tunnel.exit_sources() {
                for sa in sources {
                    if let Some(e) = conn_entry(&mut conns, sa.ip().to_string(), sa.port()) {
                        e.exit_circuits.push(cid);
                    }
                }
            }
            for e in tunnel.exits_info() {
                if let Some(port) = e.local_port {
                    listeners.push(serde_json::json!({
                        "protocol": "tunnel-exit-udp",
                        "address": format!("0.0.0.0:{port}"),
                        "circuit_id": e.circuit_id,
                    }));
                }
            }
        }
        if let Some(dht) = &stack.dht {
            for bucket in dht.buckets_snapshot() {
                for node in bucket.peers {
                    if let Some(e) = conn_entry(&mut conns, node.ip, node.port) {
                        e.dht = true;
                    }
                }
            }
        }
        // Proxies SOCKS5 des lanes anonymes (clients = moteurs locaux).
        for (hops, addr) in stack.anon_lanes() {
            listeners.push(serde_json::json!({
                "protocol": "socks5", "address": addr.to_string(), "hops": hops,
            }));
        }
    }

    // Socket d'ecoute BitTorrent du moteur principal (TCP + uTP
    // partagent le port chez librqbit).
    if let Some(addr) = state.session.engine().and_then(|e| e.listen_addr()) {
        listeners.push(serde_json::json!({
            "protocol": "bittorrent", "address": addr.to_string(),
        }));
    }

    // Connexions BitTorrent (tous moteurs : principal + lanes
    // anonymes — `session.peer_stats` fait le dispatch).
    for dl in state.session.downloads() {
        let Some(peers) = state.session.peer_stats(&dl.info_hash) else {
            continue;
        };
        for p in peers {
            if let Some(e) = conn_entry(&mut conns, p.ip, p.port) {
                e.bittorrent.push(serde_json::json!({
                    "infohash": dl.info_hash,
                    "conn_kind": p.connection_kind.unwrap_or_default(),
                    "state": p.state,
                    "incoming": p.incoming,
                    "client": p.client_name,
                    "bytes_up": p.uploaded_bytes,
                    "bytes_down": p.downloaded_bytes,
                }));
            }
        }
    }

    let connections: Vec<_> = conns
        .into_iter()
        .map(|((ip, port), e)| e.into_json(ip, port))
        .collect();
    Json(serde_json::json!({
        "connections": connections,
        "listeners": listeners,
    }))
    .into_response()
}
