// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-network-policy` — garde-fous de securite reseau.
//!
//! Responsabilite unique : centraliser les regles de securite non
//! negociables du daemon, pour eviter qu'elles ne soient dupliquees ou
//! affaiblies localement dans un autre crate :
//!
//! - protection anti-SSRF sur toute requete HTTP declenchee par du
//!   contenu distant (trackers, RSS, listes de canaux) —
//!   [`address_policy::IpPolicy`] ;
//! - politique des noeuds de sortie de `onionbit-tunnel` (quelles
//!   donnees un exit node est autorise a relayer — port fidele de
//!   `DataChecker`/`is_allowed` pyipv8) — [`exit_policy`] ;
//! - kill switch atomique : coupe tout trafic conditionne par
//!   l'anonymat quand il ne peut plus etre garanti —
//!   [`kill_switch::KillSwitch`] ;
//! - validation du proxy SOCKS5 expose par les tunnels —
//!   [`proxy_guard::validate_local_socks5_url`].
//!
//! Ce crate est volontairement sans dependance vers `onionbit-ipv8` ou
//! `onionbit-bittorrent` : il expose des politiques pures, appliquees
//! par les autres crates. Les constantes `PEER_FLAG_*` du protocole
//! de tunnels vivent ici ([`exit_policy`]) et sont re-exportees par
//! `onionbit-tunnel::routing`.

pub mod address_policy;
pub mod error;
pub mod exit_policy;
pub mod kill_switch;
pub mod proxy_guard;

pub use address_policy::IpPolicy;
pub use error::{PolicyError, Result};
pub use kill_switch::KillSwitch;

/// Decision d'une politique reseau : autoriser ou refuser une action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}
