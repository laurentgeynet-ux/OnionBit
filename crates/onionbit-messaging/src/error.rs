// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs de la couche messagerie.

/// Erreurs de codec, de verification et d'anti-replay.
#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    /// Trame recue plus grande que la borne filaire.
    #[error("trame trop grande : {0} octets (borne {1})")]
    FrameTooLarge(usize, usize),
    /// Corps applicatif au-dela de la borne `body`.
    #[error("corps trop grand : {0} octets (borne {1})")]
    BodyTooLarge(usize, usize),
    /// Erreur du decodeur bencode borne.
    #[error("bencode invalide : {0}")]
    Bencode(#[from] onionbit_format::error::FormatError),
    /// Forme de trame non conforme (types/tailles de champs).
    #[error("trame malformee : {0}")]
    Malformed(&'static str),
    /// `v` different de la version supportee.
    #[error("version de trame inconnue : {0}")]
    UnknownVersion(i64),
    /// `type` hors ensemble connu de la v1.
    #[error("type de trame inconnu : {0}")]
    UnknownKind(String),
    /// Signature Ed25519 non verifiee contre la cle du contact.
    #[error("signature de trame invalide")]
    BadSignature,
    /// Echec AEAD sur le corps chiffre.
    #[error("dechiffrement du corps impossible")]
    Decrypt,
    /// Erreur de derivation/chiffrement sous-jacente.
    #[error("erreur cryptographique : {0}")]
    Crypto(#[from] onionbit_crypto::error::CryptoError),
    /// `seq` deja accepte pour cette conversation.
    #[error("sequence deja vue (rejeu)")]
    Replayed,
    /// `seq` en dessous de la fenetre de reception.
    #[error("sequence trop ancienne")]
    TooOld,
    /// `id` de trame deja vu (dedup).
    #[error("identifiant de trame duplique")]
    DuplicateId,
}
