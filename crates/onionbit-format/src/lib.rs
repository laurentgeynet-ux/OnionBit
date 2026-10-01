// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-format` — formats de fichiers Tribler.
//!
//! - [`bencode`] : encodage bencode BEP 3 (parseur borne, encodeur
//!   canonique) ;
//! - [`torrent`] : modele `.torrent` (v1/v2, info-hash SHA-1/SHA-256) ;
//! - [`magnet`] : liens magnet BEP 9 (`btih` hex/base32, `btmh` v2) ;
//! - [`mdblob`] : blobs de metadonnees de canaux (format signe pyipv8).
//!
//! Ce crate ne depend d'aucun etat reseau ni stockage persistant
//! (cf. `docs/architecture/architecture.md`).

pub mod bencode;
pub mod error;
pub mod limits;
pub mod magnet;
pub mod mdblob;
pub mod torrent;

pub use bencode::BValue;
pub use error::{FormatError, Result};
pub use magnet::MagnetLink;
pub use torrent::TorrentMeta;
