//! `tribler-ipv8` — moteur overlay IPv8.
//!
//! Portage du coeur du protocole IPv8 (`pyipv8`) : c'est le composant le
//! **plus a risque** du projet (cf. `docs/plans/plan_faisabilite.md`,
//! section "Analyse de risques"), car aucune implementation Rust complete
//! n'existe a ce jour (`ipv8-rust-tunnels` ne couvre que le plan de
//! donnees des tunnels, pas le protocole overlay).
//!
//! Responsabilite unique :
//!
//! - encodage/decodage des messages IPv8 (format binaire `struct`-like de
//!   pyipv8, cf. `ipv8/messaging/`) ;
//! - decouverte de pairs (`ipv8/peerdiscovery/`) : bootstrap via serveurs
//!   connus, marche aleatoire, gestion du churn ;
//! - framework de "communities" (groupes logiques de pairs partageant un
//!   protocole applicatif, ex. `TunnelCommunity`, `DiscoveryCommunity`) ;
//! - DHT overlay IPv8 (distinct du DHT mainline BitTorrent BEP 5) ;
//! - signature/verification Ed25519 de chaque message.
//!
//! Ce crate ne connait ni le protocole BitTorrent (`tribler-bittorrent`)
//! ni les circuits d'anonymisation (`tribler-tunnel`, qui consomme ce
//! crate comme fondation).
//!
//! Etat : squelette (etape 0). Implementation etalee sur les etapes 9 a
//! 11 de `docs/plans/roadmap.md`, en se referant en permanence a
//! `D:\Projet\Tribler_sources\tribler\pyipv8` comme reference de verite
//! protocolaire.

/// Identifiant de community IPv8 (equivalent du `master_peer`/`community_id`
/// pyipv8, 20 octets de hash de cle publique).
pub type CommunityId = [u8; 20];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_type_identifiant_de_community_a_la_bonne_taille() {
        let id: CommunityId = [0u8; 20];
        assert_eq!(id.len(), 20);
    }
}
