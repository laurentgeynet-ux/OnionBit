//! `tribler-db` — persistance locale.
//!
//! Responsabilite unique : stocker et interroger, via SQLite
//! (`rusqlite`, mode "bundled" pour ne pas dependre d'une lib systeme —
//! important pour le multi-plateforme), les donnees que Tribler
//! persistait via Pony ORM :
//!
//! - metadonnees de torrents connus (nom, taille, info-hash, trackers) ;
//! - canaux de contenu et leurs entrees (equivalent
//!   `tribler.core.database.orm_bindings`) ;
//! - votes/popularite (equivalent `ranks.py`) ;
//! - parametres utilisateur persistants.
//!
//! Aucune logique metier ici (cf. `tribler-core` pour l'orchestration) :
//! uniquement des requetes et des migrations de schema versionnees.
//!
//! Etat : squelette (etape 0). Implementation a l'etape 4
//! ("Schema SQLite et migrations").

/// Version de schema courante geree par ce crate (les migrations futures
/// incrementeront cette valeur).
pub const SCHEMA_VERSION: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_version_de_schema_initiale_est_zero() {
        assert_eq!(SCHEMA_VERSION, 0);
    }
}
