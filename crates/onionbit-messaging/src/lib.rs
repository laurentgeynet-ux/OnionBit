// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Messagerie anonyme sur circuits e2e — ADR-0011, Phase 8.
//!
//! Ce crate est le **proprietaire unique** de la couche protocole de
//! la messagerie : trames canoniques, signature de l'emetteur, cles
//! applicatives et anti-replay. Le service d'orchestration
//! (contacts, consentement, persistance, API) vit dans
//! `onionbit-core` et consomme [`frame::Frame`].
//!
//! Proprietes visees (bornees aux tests — cf.
//! `docs/plans/bancs_tests.md` SS4.11, `MS-*`) :
//!
//! - **confidentialite du contenu e2e** : `body` chiffre
//!   ChaCha20-Poly1305 sous une cle applicative HKDF
//!   (`keys::derive_messaging_keys`), distincte des
//!   `hs_session_keys` de transport ;
//! - **authentification de l'emetteur** : chaque trame est signee
//!   Ed25519 par la cle IPv8 de l'expediteur, verifiee contre la `pk`
//!   dont derive le swarm de contact ([`hash::messaging_hash`]) ;
//! - **anti-replay / ordre** : `seq` monotone + fenetre de reception
//!   + dedup `id` ([`replay::RecvWindow`]).
//!
//! Non-claims explicites (ADR-0011) : pas de forward secrecy (v1
//! sans ratchet), pas de resistance a la correlation de trafic, aux
//! points d'introduction malveillants, ni a la compromission
//! d'endpoint.

pub mod attach;
pub mod config;
pub mod conv;
pub mod error;
pub mod frame;
pub mod gctl;
pub mod gmsg;
pub mod hash;
pub mod hello;
pub mod keys;
pub mod obox;
pub mod replay;

pub use attach::{AttachDesc, IH_LEN, MID_LEN};
pub use config::MessagingConfig;
pub use conv::{direct_conv, random_conv, ConvId, CONV_ID_LEN};
pub use error::MessagingError;
pub use frame::{preflight, Frame, MsgKind, RawFrame, PROTO_VERSION_V2};
pub use gctl::{Gctl, RosterEntry, PK_LEN};
pub use hash::messaging_hash;
pub use hello::{decode_hello, encode_hello_v1, encode_hello_v2, HELLO_CAP_GROUPS};
pub use keys::{derive_messaging_keys, MessagingKeys, HKDF_INFO_MESSAGING};
pub use replay::RecvWindow;
