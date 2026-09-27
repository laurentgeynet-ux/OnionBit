//! `tribler-core` — domaine et orchestration.
//!
//! Coeur de la "clean architecture" du projet : ce crate ne fait aucune
//! I/O directe (pas de socket, pas de SQL, pas de HTTP) — il definit les
//! entites du domaine et orchestre les crates d'infrastructure au travers
//! de traits (ports), a l'image de `tribler.core.session`/`components.py`
//! cote Python :
//!
//! - `Session` : cycle de vie du daemon, demarrage/arret ordonne des
//!   composants (bittorrent, ipv8, tunnel, db) ;
//! - `Notifier` : bus d'evenements interne (progression de telechargement,
//!   changements d'etat de circuit, etc.), consomme par `tribler-api`
//!   pour alimenter le WebSocket ;
//! - regles metier historiquement dans
//!   `content_discovery/`, `torrent_checker/`, `rss/`, `watch_folder/`.
//!
//! Depend de `tribler-bittorrent`, `tribler-tunnel`, `tribler-db` et
//! `tribler-format` via des traits definis ici (inversion de dependance),
//! jamais l'inverse.
//!
//! Etat : squelette (etape 0). L'orchestration reelle arrive
//! progressivement a partir de l'etape 5 ("Session et Notifier") puis se
//! complete au fil des etapes suivantes.

/// Evenement de domaine emis par la `Session` vers le `Notifier`.
///
/// Enumeration volontairement minimale au stade squelette ; sera etendue
/// au fil des etapes pour couvrir tunnels, canaux, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    SessionStarted,
    SessionStopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_evenements_de_domaine_sont_comparables() {
        assert_eq!(DomainEvent::SessionStarted, DomainEvent::SessionStarted);
        assert_ne!(DomainEvent::SessionStarted, DomainEvent::SessionStopped);
    }
}
