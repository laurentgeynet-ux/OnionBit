//! `tribler-api` — plan de controle REST + WebSocket.
//!
//! Responsabilite unique : exposer `tribler-core` au monde exterieur via
//! HTTP (axum), en visant la parite fonctionnelle avec
//! `tribler.core.restapi` (endpoints REST) et son `notifier`
//! (evenements WebSocket) cote Python. C'est la **seule** porte d'entree
//! reseau locale du daemon : `tribler-cli` et la future UI Flutter ne
//! parlent qu'a ce crate, jamais directement a `tribler-core`.
//!
//! Regles :
//! - aucune logique metier ici, uniquement traduction HTTP/JSON <->
//!   appels a `tribler-core` ;
//! - toute route est bindee par defaut sur `127.0.0.1` (isolation
//!   loopback, coherent avec `tribler-network-policy`) ;
//! - le schema JSON expose est documente dans
//!   `docs/reference_tribler/api_rest_mapping.md` au fur et a mesure de
//!   la migration depuis l'API Python.
//!
//! Etat : squelette (etape 0). Implementation a l'etape 6
//! ("API REST + WebSocket minimale").

/// Chemin de base de l'API REST, aligne (autant que possible) sur celui
/// de l'API Python existante pour faciliter la reutilisation ulterieure
/// du client TypeScript/Flutter.
pub const API_BASE_PATH: &str = "/api";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_chemin_de_base_de_l_api_est_defini() {
        assert_eq!(API_BASE_PATH, "/api");
    }
}
