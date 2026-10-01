// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-tunnel` — reseau d'anonymisation (port de `TunnelCommunity`).
//!
//! Responsabilite unique : construire et maintenir des circuits en onion
//! routing au-dessus de `onionbit-ipv8` pour anonymiser le trafic
//! BitTorrent (telechargement anonyme, hidden seeding). S'appuie sur :
//!
//! - `onionbit-ipv8` pour la decouverte de pairs et la communication
//!   overlay signee ;
//! - `onionbit-crypto` pour le chiffrement ChaCha20-Poly1305 par saut de
//!   circuit ;
//! - `onionbit-network-policy` pour les regles de securite des
//!   noeuds de sortie (exit policy) et le kill switch.
//!
//! Expose un proxy SOCKS5 local que `onionbit-bittorrent` peut utiliser
//! pour router le trafic peer d'un telechargement a travers un circuit.
//!
//! Complexite/risque : eleve (protocole de circuit proprietaire, gestion
//! des noeuds de sortie, resistance aux attaques Sybil). Cf. ADR-0002.
//!
//! Etat : etape 12 en cours — cellules, payloads, routage et
//! `TunnelCommunity` (create/created/extend/extended, relais, sortie
//! UDP, destroy, ping/pong) implementes ; SOCKS5 et hidden services
//! en cours.

pub mod cell;
pub mod community;
pub mod guards;
pub mod hidden_services;
pub mod http_tunnel;
pub mod payload;
pub(crate) mod pex;
pub mod routing;
pub mod settings;
pub mod socks5;
pub mod speedtest;
pub mod tunnel_udp_socket;
pub mod udp_relay;

/// `community_id` de `TunnelCommunity`/`HiddenTunnelCommunity`
/// (`81ded07332bdc775aa5a46f96de9f8f390bbc9f3`, pyipv8).
pub const TUNNEL_COMMUNITY_ID: onionbit_ipv8::CommunityId = [
    0x81, 0xde, 0xd0, 0x73, 0x32, 0xbd, 0xc7, 0x75, 0xaa, 0x5a, 0x46, 0xf9, 0x6d, 0xe9, 0xf8, 0xf3,
    0x90, 0xbb, 0xc9, 0xf3,
];

/// `community_id` de `TriblerTunnelCommunity` (Tribler >= 7.x —
/// `a3591a6bd89bbaca0974062a1287afcfbc6fd6bc`). Utilisable via
/// `TunnelCommunity::new_with_id` pour l'interop avec le client
/// Tribler installe (dont le prefixe differe de celui de pyipv8).
pub const TRIBLER_TUNNEL_COMMUNITY_ID: onionbit_ipv8::CommunityId = [
    0xa3, 0x59, 0x1a, 0x6b, 0xd8, 0x9b, 0xba, 0xca, 0x09, 0x74, 0x06, 0x2a, 0x12, 0x87, 0xaf, 0xcf,
    0xbc, 0x6f, 0xd6, 0xbc,
];

/// Niveau d'anonymisation demande pour un telechargement, aligne sur les
/// options historiques de Tribler (nombre de sauts du circuit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopCount {
    Zero,
    One,
    Two,
    Three,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_niveaux_de_sauts_sont_distincts() {
        assert_ne!(HopCount::Zero, HopCount::Three);
    }
}
