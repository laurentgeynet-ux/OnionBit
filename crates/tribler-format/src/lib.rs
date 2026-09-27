//! `tribler-format` — formats de fichiers Tribler.
//!
//! Responsabilite unique : parser/serialiser les formats de donnees
//! statiques utilises par Tribler, independamment de tout etat reseau ou
//! disque :
//!
//! - fichiers `.torrent` (bencode, BEP 3) et liens magnet (BEP 9) ;
//! - blobs de metadonnees de canaux `.mdblob` (format proprietaire Tribler
//!   pour la decouverte de contenu decentralisee) ;
//! - identifiants de contenu (info-hash v1/v2, cle de canal).
//!
//! Ce crate ne doit dependre d'aucun etat reseau (voir `tribler-bittorrent`)
//! ni de stockage persistant (voir `tribler-db`). Cf.
//! `docs/architecture/architecture.md` (couche "Formats").
//!
//! Etat : squelette (etape 0 de `docs/plans/roadmap.md`). L'implementation
//! reelle sera ajoutee a l'etape 1 ("Fondations bencode/torrent/magnet").

/// Marqueur de version du format `.mdblob` supporte par ce crate.
///
/// Valeur provisoire tant que le format n'est pas implemente ; sera promue
/// en constante documentee dans un module dedie a l'etape 1.
pub const MDBLOB_FORMAT_VERSION: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_squelette_compile_et_expose_une_constante() {
        assert_eq!(MDBLOB_FORMAT_VERSION, 0);
    }
}
