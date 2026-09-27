//! `tribler-tunnel` — reseau d'anonymisation (port de `TunnelCommunity`).
//!
//! Responsabilite unique : construire et maintenir des circuits en onion
//! routing au-dessus de `tribler-ipv8` pour anonymiser le trafic
//! BitTorrent (telechargement anonyme, hidden seeding). S'appuie sur :
//!
//! - `tribler-ipv8` pour la decouverte de pairs et la communication
//!   overlay signee ;
//! - `tribler-crypto` pour le chiffrement AES-GCM par saut de circuit ;
//! - `tribler-network-policy` pour les regles de securite des
//!   noeuds de sortie (exit policy) et le kill switch.
//!
//! Expose un proxy SOCKS5 local que `tribler-bittorrent` peut utiliser
//! pour router le trafic peer d'un telechargement a travers un circuit.
//!
//! Complexite/risque : eleve (protocole de circuit proprietaire, gestion
//! des noeuds de sortie, resistance aux attaques Sybil). Cf. ADR-0002.
//!
//! Etat : squelette (etape 0). Implementation a l'etape 12
//! ("TunnelCommunity : circuits et hidden seeding").

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
