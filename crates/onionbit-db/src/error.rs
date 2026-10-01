// This file is part of OnionBit - a Rust port of the Tribler daemon.
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
}
