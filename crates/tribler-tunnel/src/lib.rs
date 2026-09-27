//! `tribler-tunnel` — reseau d'anonymisation (port de `TunnelCommunity`).
//!
//! Responsabilite unique : construire et maintenir des circuits en onion
//! routing au-dessus de `tribler-ipv8` pour anonymiser le trafic
//! BitTorrent (telechargement anonyme, hidden seeding). S'appuie sur :
//!
//! - `tribler-ipv8` pour la decouverte de pairs et la communication
//!   overlay signee ;
//! - `tribler-crypto` pour le chiffrement ChaCha20-Poly1305 par saut de
//!   circuit ;
//! - `tribler-network-policy` pour les regles de securite des
//!   noeuds de sortie (exit policy) et le kill switch.
//!
//! Expose un proxy SOCKS5 local que `tribler-bittorrent` peut utiliser
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
pub mod hidden_services;
pub mod http_tunnel;
pub mod payload;
pub mod routing;
pub mod socks5;
pub mod udp_relay;

/// `community_id` de `TunnelCommunity`/`HiddenTunnelCommunity`
/// (`81ded07332bdc775aa5a46f96de9f8f390bbc9f3`, pyipv8).
pub const TUNNEL_COMMUNITY_ID: tribler_ipv8::CommunityId = [
    0x81, 0xde, 0xd0, 0x73, 0x32, 0xbd, 0xc7, 0x75, 0xaa, 0x5a, 0x46, 0xf9, 0x6d, 0xe9, 0xf8, 0xf3,
    0x90, 0xbb, 0xc9, 0xf3,
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
