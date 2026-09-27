//! `tribler-test-support` — helpers de tests partages.
//!
//! Propriete unique des helpers de tests d'integration cross-crates
//! (fixtures de torrents/canaux, pairs IPv8 en boucle locale, daemon
//! jetable pour les tests d'API). A utiliser en `dev-dependency`
//! uniquement — jamais en dependance de production. Cf. la table
//! "Anti-duplication" de `AGENTS.md` : toute fixture partagee par au
//! moins deux crates doit vivre ici, pas etre dupliquee.
//!
//! Etat : squelette (etape 0). Peuple au fil des etapes, en meme temps
//! que les besoins de tests d'integration apparaissent.

/// Marqueur de disponibilite du crate de support de tests (permet aux
/// autres crates de verifier qu'ils importent bien la bonne version).
pub const SUPPORT_CRATE_READY: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_crate_de_support_est_marque_pret() {
        let ready = std::hint::black_box(SUPPORT_CRATE_READY);
        assert!(ready);
    }
}
