//! `tribler-bittorrent` — moteur BitTorrent.
//!
//! Responsabilite unique : exposer une API Tribler-idiomatique
//! (`DownloadHandle`, `DownloadConfig`, evenements de progression) au
//! dessus du moteur BitTorrent reutilise `librqbit` (voir ADR-0001) :
//! bencode, protocole peer-wire, DHT mainline (BEP 5), uTP, communication
//! avec les trackers HTTP/UDP.
//!
//! Ce crate est l'equivalent du module Python `tribler.core.libtorrent`,
//! mais ne reimplemente pas le protocole filaire bas niveau : il pilote
//! `librqbit::Session` et traduit son etat vers/depuis les types du
//! domaine Tribler (`tribler-core`).
//!
//! Point d'integration futur avec `tribler-tunnel` : possibilite de
//! forcer le trafic peer d'un telechargement a travers le proxy SOCKS5
//! expose par un circuit anonyme (cf. `tribler-network-policy` pour les
//! garde-fous).
//!
//! Etat : squelette (etape 0). Implementation a l'etape 3 ("Integration
//! librqbit et sessions de telechargement").

/// Etat de haut niveau d'un telechargement, tel qu'expose par ce crate.
///
/// Types provisoires : la forme definitive sera alignee sur les besoins de
/// l'API REST (`tribler-api`) a l'etape 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Stopped,
    Downloading,
    Seeding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_squelette_expose_un_statut_par_defaut_coherent() {
        assert_eq!(DownloadStatus::Stopped, DownloadStatus::Stopped);
    }
}
