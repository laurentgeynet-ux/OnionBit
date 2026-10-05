// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Instantanes REST `/api/ipv8/overlays` (`OverlaySchema` pyipv8) :
//! description d'une community active et nommage des `msg_id`
//! (`decode_map` → `"id:on_handler"`).

use crate::packet::{prefix_of, PREFIX_LEN};
use crate::{CommunityId, UdpAddress};

/// `max_peers` par defaut (`DEFAULT_MAX_PEERS` pyipv8).
pub const DEFAULT_MAX_PEERS: u32 = 30;

/// Pair `OverlaySchema.peers[]` — `{"ip","port","public_key"}` (hex).
#[derive(Debug, Clone)]
pub struct OverlayPeer {
    /// `peer.address[0]`.
    pub ip: String,
    /// `peer.address[1]`.
    pub port: u16,
    /// `peer.public_key.key_to_bin()` en hex.
    pub public_key_hex: String,
}

/// `(ip, port)` d'une `UdpAddress` (`("0.0.0.0", 0)` pour une adresse
/// absente — `peer.address` Python est toujours definie sur un pair
/// verifie ; le repli est un garde-fou).
pub fn addr_parts(addr: Option<&UdpAddress>) -> (String, u16) {
    match addr {
        Some(UdpAddress::Ipv4(a)) => (a.ip().to_string(), a.port()),
        Some(UdpAddress::Ipv6(a)) => (a.ip().to_string(), a.port()),
        Some(UdpAddress::Domain(h, p)) => (h.clone(), *p),
        None => ("0.0.0.0".to_string(), 0),
    }
}

/// `OverlayPeer` depuis un `Peer` de l'annuaire.
pub fn overlay_peer(peer: &crate::peer::Peer) -> OverlayPeer {
    let (ip, port) = addr_parts(peer.address.as_ref());
    OverlayPeer {
        ip,
        port,
        public_key_hex: hex::encode(&peer.public_key_bin),
    }
}

/// `strategies[]` — `{"name","target_peers"}` (`-1` = toujours actif).
#[derive(Debug, Clone)]
pub struct OverlayStrategy {
    /// `strategy.__class__.__name__`.
    pub name: &'static str,
    /// `target_peers` (`session.strategies` Python).
    pub target_peers: i32,
}

/// Instantane `OverlaySchema` pyipv8 (un element de
/// `GET /api/ipv8/overlays`).
#[derive(Debug, Clone)]
pub struct OverlayInfo {
    /// `community_id` (20 octets, serialise en hex).
    pub community_id: CommunityId,
    /// `my_peer.public_key.key_to_bin()` en hex.
    pub my_peer_hex: String,
    /// `global_time` (horloge de Lamport locale).
    pub global_time: u64,
    /// `get_peers()` du service de la community.
    pub peers: Vec<OverlayPeer>,
    /// `overlay.__class__.__name__`.
    pub overlay_name: &'static str,
    /// `max_peers` (`settings.max_peers`, defaut 30).
    pub max_peers: u32,
    /// `session.network != overlay.network` — la DHT discovery a sa
    /// propre instance `Network` (isolee), comme pyipv8.
    pub is_isolated: bool,
    /// `my_estimated_wan` (non estime → `0.0.0.0:0`).
    pub my_estimated_wan: UdpAddress,
    /// `my_estimated_lan`.
    pub my_estimated_lan: UdpAddress,
    /// `session.strategies` filtrees sur cet overlay.
    pub strategies: Vec<OverlayStrategy>,
    /// `decode_map` de la community : `msg_id` → nom de handler.
    pub decode: fn(u8) -> Option<&'static str>,
}

impl OverlayInfo {
    /// `overlay.get_prefix()` : `0x00` + `0x00` + `community_id`.
    pub fn prefix(&self) -> [u8; PREFIX_LEN] {
        prefix_of(&self.community_id)
    }

    /// `overlay.decode_map[msg_id].__name__` : nom du handler, ou
    /// `None` → le REST serialise `"{id}:unknown"`.
    pub fn msg_name(&self, msg_id: u8) -> Option<&'static str> {
        (self.decode)(msg_id)
    }
}

/// Nom de handler du `decode_map` de `Community` pyipv8 — partage par
/// toutes les communities (y compris `onionbit-tunnel`, qui ne peut pas
/// dependre de `onionbit-ipv8::overlays` pour sa propre map).
pub fn base_community_msg_name(msg_id: u8) -> Option<&'static str> {
    Some(match msg_id {
        234 => "on_new_introduction_request",
        233 => "on_new_introduction_response",
        246 => "on_old_introduction_request",
        245 => "on_old_introduction_response",
        232 => "on_new_puncture_request",
        250 => "on_old_puncture_request",
        231 => "on_new_puncture",
        249 => "on_puncture",
        235..=244 | 247 | 248 | 251..=255 => "on_deprecated_message",
        _ => return None,
    })
}

/// `decode_map` de `DiscoveryCommunity` (`SimilarityRequest`/`Response`,
/// `Ping`/`Pong` + map de base).
pub fn discovery_msg_name(msg_id: u8) -> Option<&'static str> {
    match msg_id {
        1 => Some("on_similarity_request"),
        2 => Some("on_similarity_response"),
        3 => Some("on_ping"),
        4 => Some("on_pong"),
        _ => base_community_msg_name(msg_id),
    }
}

/// `decode_map` de `ContentDiscoveryCommunity` Tribler.
pub fn content_discovery_msg_name(msg_id: u8) -> Option<&'static str> {
    match msg_id {
        3 => Some("on_health_request"),
        4 => Some("on_health"),
        101 => Some("on_version_request"),
        102 => Some("on_version_response"),
        201 => Some("on_remote_select"),
        202 => Some("on_remote_select_response"),
        // `add_message_handler(1|2|209, on_deprecated_message)`.
        1 | 2 | 209 => Some("on_deprecated_message"),
        _ => base_community_msg_name(msg_id),
    }
}

/// `decode_map` de `OnionbitExtCommunity` (ADR-0015 — extension
/// OnionBit-only ; les `msg_id` futurs `ledger_*`/`attest_*` s'y
/// ajouteront, les inconnus ne remontent pas de nom).
pub fn ext_msg_name(msg_id: u8) -> Option<&'static str> {
    match msg_id {
        1 => Some("on_hello"),
        _ => None,
    }
}

/// `decode_map` de `DHTDiscoveryCommunity` pyipv8.
pub fn dht_msg_name(msg_id: u8) -> Option<&'static str> {
    match msg_id {
        1 => Some("on_ping_request"),
        2 => Some("on_ping_response"),
        3 => Some("on_store_request"),
        4 => Some("on_store_response"),
        5 => Some("on_find_request"),
        6 => Some("on_find_response"),
        7 => Some("on_store_peer_request"),
        8 => Some("on_store_peer_response"),
        9 => Some("on_connect_peer_request"),
        10 => Some("on_connect_peer_response"),
        _ => base_community_msg_name(msg_id),
    }
}
