// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adresse multi-plan d'un pair (ADR-0029 etape 123) — extension
//! OnionBit, sans equivalent dans Tribler.
//!
//! Un contact ou un canal designe **la meme identite** joignable
//! sur un ou deux plans, completement separes :
//!
//! - `onionbit:<cle>` — cle publique LibNaCl de l'identite IPv8
//!   (64 octets, 128 hex) : plan overlay OnionBit (circuits
//!   multi-sauts UDP) ;
//! - `onion:<hote>` — adresse de service cache Tor v3 : plan Tor
//!   (rendez-vous `.onion` sur circuits TCP arti).
//!
//! Forme canonique texte : segments separes par `,` dans l'ordre
//! `onionbit` puis `onion` — `"onionbit:ab12…,onion:x…y.onion"`.
//! Un seul segment suffit (`"onion:…"`). Les formes nues sont
//! tolerées a la lecture : une adresse `.onion` ou 128 hex seuls
//! deviennent le plan evident — le copier-coller reste ergonomique.
//!
//! **Jamais de traduction inter-plans** : le type conserve les deux
//! plans comme fournis ; rien ne derive une adresse `.onion` d'une
//! cle IPv8 ni l'inverse (la liaison est explicite, ADR-0029 §8 Q3).

use std::fmt;
use std::str::FromStr;

use crate::error::{FormatError, Result};
use crate::onion::OnionAddr;

/// Prefixe du plan overlay OnionBit dans la forme texte.
pub const PLAN_ONIONBIT_PREFIX: &str = "onionbit:";
/// Prefixe du plan Tor dans la forme texte.
pub const PLAN_ONION_PREFIX: &str = "onion:";

/// Longueur hex d'une cle publique LibNaCl (64 octets).
const PK_HEX_LEN: usize = 128;

/// Libelle stable d'un plan — remonte a l'UI (`planes` des DTO).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// Overlay OnionBit — circuits IPv8 multi-sauts.
    Onionbit,
    /// Service cache Tor — rendez-vous `.onion` (en ligne requis).
    Onion,
}

impl Plane {
    /// Libelle API (`"onionbit"` | `"onion"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onionbit => "onionbit",
            Self::Onion => "onion",
        }
    }
}

/// Adresse d'un pair : `onionbit:<cle>` et/ou `onion:<hote>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerAddress {
    /// Cle publique IPv8 du pair (64 octets) — plan OnionBit.
    pub onionbit: Option<Vec<u8>>,
    /// Service cache Tor du pair — plan `.onion`.
    pub onion: Option<OnionAddr>,
}

impl PeerAddress {
    /// Parse la forme texte (`onionbit:`/`onion:`, segments `,` —
    /// voir l'en-tete du module).
    pub fn parse(s: &str) -> Result<Self> {
        let bad = |r: &str| FormatError::BadPeerAddr(r.into());
        let mut addr = Self::default();
        let mut seen = false;
        for seg in s.trim().split(',') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            seen = true;
            if let Some(hex_pk) = seg.strip_prefix(PLAN_ONIONBIT_PREFIX) {
                if addr.onionbit.is_some() {
                    return Err(bad("segment onionbit duplique"));
                }
                addr.onionbit = Some(parse_pk(hex_pk.trim())?);
            } else if let Some(host) = seg.strip_prefix(PLAN_ONION_PREFIX) {
                if addr.onion.is_some() {
                    return Err(bad("segment onion duplique"));
                }
                addr.onion = Some(parse_onion(host.trim())?);
            } else if let Ok(pk) = parse_pk(seg) {
                if addr.onionbit.is_some() {
                    return Err(bad("segment onionbit duplique"));
                }
                addr.onionbit = Some(pk);
            } else if let Ok(o) = parse_onion(seg) {
                if addr.onion.is_some() {
                    return Err(bad("segment onion duplique"));
                }
                addr.onion = Some(o);
            } else {
                return Err(bad("segment ni `onionbit:` ni `.onion` valide"));
            }
        }
        if !seen || addr.planes().is_empty() {
            return Err(bad("adresse vide"));
        }
        Ok(addr)
    }

    /// Plans disponibles, ordre canonique `onionbit` puis `onion`.
    pub fn planes(&self) -> Vec<Plane> {
        let mut v = Vec::with_capacity(2);
        if self.onionbit.is_some() {
            v.push(Plane::Onionbit);
        }
        if self.onion.is_some() {
            v.push(Plane::Onion);
        }
        v
    }

    /// `true` si le plan `.onion` est renseigne.
    pub fn has_onion(&self) -> bool {
        self.onion.is_some()
    }
}

/// Decode la cle publique `"<128 hex>"` → 64 octets.
fn parse_pk(hex_pk: &str) -> Result<Vec<u8>> {
    let raw =
        hex::decode(hex_pk).map_err(|_| FormatError::BadPeerAddr("cle onionbit non-hex".into()))?;
    if raw.len() != PK_HEX_LEN / 2 {
        return Err(FormatError::BadPeerAddr("cle onionbit != 64 octets".into()));
    }
    Ok(raw)
}

/// Parse le segment `onion` (`OnionAddr` valide le checksum v3).
fn parse_onion(host: &str) -> Result<OnionAddr> {
    OnionAddr::parse(host).map_err(|e| FormatError::BadPeerAddr(e.to_string()))
}

impl fmt::Display for PeerAddress {
    /// Forme canonique `onionbit:<hex>,onion:<hote>` (segments
    /// absents omis).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut segs = Vec::with_capacity(2);
        if let Some(pk) = &self.onionbit {
            segs.push(format!("{PLAN_ONIONBIT_PREFIX}{}", hex::encode(pk)));
        }
        if let Some(o) = &self.onion {
            segs.push(format!("{PLAN_ONION_PREFIX}{o}"));
        }
        f.write_str(&segs.join(","))
    }
}

impl FromStr for PeerAddress {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONION: &str = "duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion";

    fn pk() -> String {
        "ab".repeat(64)
    }

    #[test]
    fn plans_simples_et_combines() {
        let pk = pk();
        // Plan seul.
        let a = PeerAddress::parse(&format!("onion:{ONION}")).unwrap();
        assert_eq!(a.planes(), vec![Plane::Onion]);
        let b = PeerAddress::parse(&format!("onionbit:{pk}")).unwrap();
        assert_eq!(b.planes(), vec![Plane::Onionbit]);
        // Combine, ordre canonique quel que soit l'ordre fourni.
        let c = PeerAddress::parse(&format!("onion:{ONION},onionbit:{pk}")).unwrap();
        assert_eq!(c.planes(), vec![Plane::Onionbit, Plane::Onion]);
        assert_eq!(c.to_string(), format!("onionbit:{pk},onion:{ONION}"));
        // Formes nues tolerees.
        assert_eq!(
            PeerAddress::parse(ONION).unwrap().planes(),
            vec![Plane::Onion]
        );
        assert_eq!(
            PeerAddress::parse(&pk).unwrap().planes(),
            vec![Plane::Onionbit]
        );
    }

    #[test]
    fn rejette_les_formes_invalidees() {
        for bad in [
            "",
            "  ",
            "onion:",
            "onionbit:",
            "onion:pas-une-adresse.onion",
            "onion:3g2upl4pq6kufc4m.onion", // v2
            "onionbit:zz",                  // non-hex
            "onionbit:ab",                  // trop court
            "clearnet.org",
            "tor:<x>", // prefixe inconnu
        ] {
            assert!(PeerAddress::parse(bad).is_err(), "{bad:?} refuse");
        }
        // Doublons de segment.
        assert!(PeerAddress::parse(&format!("onion:{ONION},onion:{ONION}")).is_err());
        assert!(PeerAddress::parse(&format!("onionbit:{0},onionbit:{0}", pk())).is_err());
        // Host onion altere (checksum v3 casse).
        let mut alt = ONION.to_string();
        alt.replace_range(0..1, "a");
        assert!(PeerAddress::parse(&format!("onion:{alt}")).is_err());
    }
}
