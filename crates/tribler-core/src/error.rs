//! Erreurs de la couche domaine/orchestration.

/// Resultat des operations de ce crate.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Erreurs de `tribler-core`.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Erreur du moteur BitTorrent.
    #[error("bittorrent: {0}")]
    Bt(#[from] tribler_bittorrent::BtError),
    /// Erreur de persistance.
    #[error("db: {0}")]
    Db(#[from] tribler_db::DbError),
    /// Erreur de format (magnet/.torrent invalide).
    #[error("format: {0}")]
    Format(#[from] tribler_format::FormatError),
    /// Erreur d'E/S.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Session deja demarree ou deja arretee.
    #[error("etat de session invalide: {0}")]
    InvalidState(&'static str),
    /// Erreur d'etat avec message dynamique (services, stack ipv8).
    #[error("etat: {0}")]
    State(String),
    /// Erreur cryptographique (cles IPv8).
    #[error("crypto: {0}")]
    Crypto(#[from] tribler_crypto::CryptoError),
    /// Refus impose par une politique reseau (anti-SSRF).
    #[error("politique reseau: {0}")]
    Policy(#[from] tribler_network_policy::PolicyError),
}
