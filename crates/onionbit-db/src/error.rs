// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs de la couche persistance.

/// Resultat des operations de ce crate.
pub type Result<T> = std::result::Result<T, DbError>;

/// Erreurs de la base SQLite.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Erreur SQLite.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Erreur d'E/S (creation du repertoire parent, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Le schema de la base est plus recent que ce que ce binaire
    /// comprend (downgrade non supporte).
    #[error("version de schema trop recente : {found} > {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
    /// Champ obligatoire absent ou de type inattendu.
    #[error("donnee corrompue : {0}")]
    Corrupt(String),
    /// Base chiffree sqlite3mc illisible a l'ouverture : clef
    /// absente ou ne correspond pas (jamais confondu avec
    /// `Corrupt` — l'UI peut dire « mauvaise identite »).
    #[error("base chiffree illisible (clef absente ou invalide)")]
    Encrypted,
    /// La bibliotheque `sqlite3` liee n'est pas sqlite3mc :
    /// `PRAGMA cipher_version` repond vide (SQLite vanilla ignore
    /// les pragmas inconnus). Cause typique : une feature de
    /// dependance a impose une autre liaison (`libsqlite3-sys/
    /// bundled`, `arti-client/static-sqlite` — incident ADR-0031,
    /// etape 132). Refus d'ouvrir : une base « chiffree » serait
    /// ecrite en clair en silence.
    #[error("codec sqlite3mc absent de la bibliotheque liee (PRAGMA cipher_version vide)")]
    CodecAbsent,
}
