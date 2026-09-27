//! `tribler-network-policy` — garde-fous de securite reseau.
//!
//! Responsabilite unique : centraliser les regles de securite non
//! negociables du daemon, pour eviter qu'elles ne soient dupliquees ou
//! affaiblies localement dans un autre crate :
//!
//! - protection anti-SSRF sur toute requete HTTP declenchee par du
//!   contenu distant (trackers, RSS, listes de canaux) ;
//! - politique des noeuds de sortie de `tribler-tunnel` (quelles
//!   destinations un exit node est autorise a relayer) ;
//! - kill switch atomique : coupe tout trafic reseau si l'etat
//!   d'anonymisation attendu ne peut plus etre garanti ;
//! - validation des connexions au proxy SOCKS5 expose par les tunnels.
//!
//! Ce crate est volontairement sans dependance vers `tribler-ipv8` ou
//! `tribler-bittorrent` : il expose des politiques pures, appliquees par
//! les autres crates.
//!
//! Etat : squelette (etape 0). Implementation a l'etape 13
//! ("Politiques de securite reseau et kill switch").

/// Decision d'une politique reseau : autoriser ou refuser une action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_decisions_de_politique_sont_distinctes() {
        assert_ne!(PolicyDecision::Allow, PolicyDecision::Deny);
    }
}
