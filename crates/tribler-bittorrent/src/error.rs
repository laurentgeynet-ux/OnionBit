//! Erreurs du moteur BitTorrent.

/// Resultat des operations de ce crate.
pub type Result<T> = std::result::Result<T, BtError>;

/// Erreurs remontees par l'enveloppe `librqbit`.
#[derive(Debug, thiserror::Error)]
pub enum BtError {
    /// Erreur interne du moteur `librqbit`.
    #[error("moteur bittorrent: {0}")]
    Engine(String),
    /// Le format d'entree est invalide.
    #[error("format: {0}")]
    Format(#[from] tribler_format::FormatError),
    /// URL ou magnet mal forme.
    #[error("url invalide: {0}")]
    BadUrl(String),
    /// Torrent introuvable dans la session.
    #[error("torrent introuvable: {0}")]
    NotFound(String),
    /// Le torrent ajoute n'a pas produit de handle (ex. list_only).
    #[error("pas de handle de torrent")]
    NoHandle,
    /// Erreur d'E/S.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
