//! `tribler-crypto` — primitives cryptographiques partagees.
//!
//! Responsabilite unique :
//!
//! - hachage de pieces/fichiers BitTorrent (SHA-1 pour BEP 3, SHA-256 pour
//!   BEP 52 "v2") ;
//! - gestion des cles IPv8 (Ed25519 pour la signature des messages
//!   overlay, X25519 pour l'echange de cles des circuits de tunnel),
//!   avec les trois niveaux de taille de cle historiques de pyipv8
//!   (`low`/`medium`/`high`) ;
//! - chiffrement/dechiffrement des donnees de tunnel (AES-GCM par saut de
//!   circuit).
//!
//! Aucune logique reseau ou de protocole ici : uniquement des fonctions
//! pures/deterministes autour des cles et des hachages. Cf. ADR-0003 pour
//! la justification du choix des algorithmes face aux equivalents pyipv8
//! (libnacl/curve25519).
//!
//! Etat : squelette (etape 0). Implementation a l'etape 2 ("Crypto de
//! base") et etape 9 ("Cles et crypto IPv8/tunnel").

/// Tailles de cle IPv8 historiques (pyipv8 `LibNaCLSK`), a reimplementer a
/// l'identique pour l'interoperabilite avec les pairs IPv8 existants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv8KeySize {
    Low,
    Medium,
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_trois_tailles_de_cle_sont_distinctes() {
        assert_ne!(Ipv8KeySize::Low, Ipv8KeySize::Medium);
        assert_ne!(Ipv8KeySize::Medium, Ipv8KeySize::High);
    }
}
