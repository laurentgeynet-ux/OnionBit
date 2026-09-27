//! Erreurs typées de `tribler-format`.

use thiserror::Error;

/// Erreur de parsing/serialisation d'un format Tribler.
#[derive(Debug, Error)]
pub enum FormatError {
    /// Entree tronquee (fin de donnees atteinte prematurement).
    #[error("entree tronquee a l'offset {offset}")]
    Truncated { offset: usize },
    /// Bencode invalide.
    #[error("bencode invalide a l'offset {offset}: {reason}")]
    BadBencode { offset: usize, reason: String },
    /// Profondeur d'imbrication maximale depassee (protection DoS).
    #[error("profondeur d'imbrication bencode depassee (max {max})")]
    DepthExceeded { max: usize },
    /// Taille maximale depassee (protection DoS).
    #[error("taille maximale depassee ({actual} > {max} octets)")]
    SizeExceeded { actual: usize, max: usize },
    /// Champ obligatoire absent du .torrent.
    #[error("champ obligatoire absent du torrent: {0}")]
    MissingField(&'static str),
    /// Lien magnet invalide.
    #[error("lien magnet invalide: {0}")]
    BadMagnet(String),
}

/// Alias de `Result` pour ce crate.
pub type Result<T> = core::result::Result<T, FormatError>;
