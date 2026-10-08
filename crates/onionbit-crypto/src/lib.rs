// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-crypto` — primitives cryptographiques partagees.
//!
//! - [`hash`] : SHA-1/SHA-256 pour les info-hash BitTorrent et les `mid`
//!   IPv8 ;
//! - [`ipv8::keys`] : cles IPv8 "LibNaCL dual" (Ed25519 + X25519) au format
//!   filaire exact de pyipv8 ;
//! - [`ipv8::dh`] : `crypto_box_beforenm` (X25519 + HSalsa20) pour les
//!   echanges de cles de circuit ;
//! - [`ipv8::session`] : cles de session HKDF-SHA256 et chiffrement
//!   ChaCha20-Poly1305 des cellules de tunnel ;
//! - [`error`] : erreurs typees.
//!
//! Reference de verite : `docs/reference_tribler/ipv8_rust_tunnels/`
//! (copie locale des sources Rust officielles du projet
//! `Tribler/ipv8-rust-tunnels`, qui implementent le plan de donnees des
//! tunnels utilise par pyipv8).
//!
//! Aucune logique reseau ou de protocole ici : uniquement des fonctions
//! autour des cles et des hachages.

pub mod error;
pub mod hash;
pub mod ipv8;
/// Blobs proteges par mot de passe (export d'identite portable).
pub mod keyblob;
/// Primitives du transport furtif (ADR-0017) : cles ephemeres
/// Elligator2, DH/HKDF domaine `onionbit/stealth/v1`, AEAD.
pub mod stealth;

pub use error::CryptoError;
