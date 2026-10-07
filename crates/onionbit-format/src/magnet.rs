// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Liens magnet (BEP 9) : `magnet:?xt=urn:btih:<hash>&dn=...&tr=...`.
//!
//! Supports :
//!
//! - `xt=urn:btih:<hex40>` — info-hash v1 en hexadecimal ;
//! - `xt=urn:btih:<base32 32 chars>` — info-hash v1 en base32 ;
//! - `xt=urn:btmh:<hex64>` — info-hash v2 (multihash `0x12 0x20` + SHA-256) ;
//! - `dn` (nom), `tr` (trackers, repetable), `ws` (webseeds), `x.pe`
//!   (pairs directs).

use onionbit_crypto::hash::{InfoHashV1, InfoHashV2};

use crate::error::{FormatError, Result};
use crate::limits;

/// Prefixe canonique d'un lien magnet.
pub const MAGNET_PREFIX: &str = "magnet:?";
/// Multihash SHA-256 prefixe (0x12 = sha2-256, 0x20 = 32 octets) utilise
/// par `urn:btmh:` pour les info-hash v2.
const BTMH_SHA256_PREFIX: [u8; 2] = [0x12, 0x20];

/// Lien magnet parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MagnetLink {
    /// Info-hash v1 (si `urn:btih` present).
    pub info_hash_v1: Option<InfoHashV1>,
    /// Info-hash v2 (si `urn:btmh` sha2-256 present).
    pub info_hash_v2: Option<InfoHashV2>,
    /// Nom d'affichage (`dn`).
    pub display_name: Option<String>,
    /// Trackers (`tr`, ordre conserve).
    pub trackers: Vec<String>,
    /// Sources web (`ws`).
    pub webseeds: Vec<String>,
    /// Pairs directs (`x.pe`, format "host:port").
    pub peers: Vec<String>,
}

impl MagnetLink {
    /// Parse un lien `magnet:?...`.
    pub fn parse(uri: &str) -> Result<Self> {
        if uri.len() > limits::MAX_MAGNET_LEN {
            return Err(FormatError::BadMagnet("lien magnet trop long".into()));
        }
        let query = uri
            .strip_prefix(MAGNET_PREFIX)
            .ok_or_else(|| FormatError::BadMagnet("prefixe magnet:? absent".into()))?;

        let mut link = MagnetLink::default();
        for param in query.split('&') {
            // Tolerance aux liens du web/RSS : `&` terminal, `&&` et
            // flags sans valeur (`&fl`) sont ignores plutot que de
            // rejeter tout le magnet (comportement libtorrent).
            if param.is_empty() {
                continue;
            }
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            let value = percent_decode(value);
            match key {
                "xt" => match value.strip_prefix("urn:btih:") {
                    Some(h) => link.info_hash_v1 = Some(parse_btih(h)?),
                    None => {
                        if let Some(h) = value.strip_prefix("urn:btmh:") {
                            link.info_hash_v2 = Some(parse_btmh(h)?);
                        }
                        // Autres xt (ex. urn:sha1:) ignores.
                    }
                },
                "dn" => link.display_name = Some(value),
                "tr" => link.trackers.push(value),
                "ws" => link.webseeds.push(value),
                "x.pe" => link.peers.push(value),
                _ => {} // kt, xs, as, etc. : ignores
            }
        }
        if link.info_hash_v1.is_none() && link.info_hash_v2.is_none() {
            return Err(FormatError::BadMagnet(
                "aucun xt=urn:btih: ou urn:btmh: present".into(),
            ));
        }
        Ok(link)
    }

    /// Info-hash d'affichage (v1 prioritaire, hex).
    pub fn info_hash_hex(&self) -> String {
        if let Some(h) = &self.info_hash_v1 {
            onionbit_crypto::hash::to_hex(h)
        } else if let Some(h) = &self.info_hash_v2 {
            onionbit_crypto::hash::to_hex(h)
        } else {
            String::new()
        }
    }
}

/// Reconstruit un lien magnet sans aucun parametre `tr` (les autres
/// parametres — `xt`, `dn`, `ws`, `x.pe`, inconnus — sont recopies
/// verbatim). librqbit fusionne `magnet.trackers` avec
/// `AddTorrentOptions::trackers` : retirer les `tr` de l'URI est le
/// seul moyen de supprimer durablement un tracker de la source.
pub fn strip_trackers(uri: &str) -> String {
    let Some(query) = uri.strip_prefix(MAGNET_PREFIX) else {
        return uri.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| p.split('=').next() != Some("tr"))
        .collect();
    format!("{MAGNET_PREFIX}{}", kept.join("&"))
}

/// Decode `urn:btih:` : 40 caracteres hex ou 32 caracteres base32.
fn parse_btih(s: &str) -> Result<InfoHashV1> {
    match s.len() {
        40 => onionbit_crypto::hash::from_hex(s)
            .and_then(|v| <InfoHashV1>::try_from(v.as_slice()).ok())
            .ok_or_else(|| FormatError::BadMagnet("btih hex invalide".into())),
        32 => base32_decode(s)
            .and_then(|v| <InfoHashV1>::try_from(v.as_slice()).ok())
            .ok_or_else(|| FormatError::BadMagnet("btih base32 invalide".into())),
        _ => Err(FormatError::BadMagnet(format!(
            "btih de {} caracteres (40 hex ou 32 base32 attendus)",
            s.len()
        ))),
    }
}

/// Decode `urn:btmh:` : hex du multihash (doit commencer par 0x12 0x20).
fn parse_btmh(s: &str) -> Result<InfoHashV2> {
    let raw = onionbit_crypto::hash::from_hex(s)
        .ok_or_else(|| FormatError::BadMagnet("btmh hex invalide".into()))?;
    if raw.len() != 2 + 32 || raw[..2] != BTMH_SHA256_PREFIX {
        return Err(FormatError::BadMagnet(
            "btmh n'est pas un multihash sha2-256".into(),
        ));
    }
    let mut h = [0u8; 32];
    h.copy_from_slice(&raw[2..]);
    Ok(h)
}

/// Decode du base32 RFC 4648 (alphabet A-Z2-7, sans padding) — utilise
/// par les vieux liens btih.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buf: u64 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a', // tolerance : certains liens en minuscules
            b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        buf = (buf << 5) | u64::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// Decode le percent-encoding d'un parametre de query.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        // '+' reste litteral dans les magnets (pas d'application/x-www-form-urlencoded)
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encode un parametre de query magnet (`dn`, `tr`…) — pendant de
/// [`percent_decode`]. Sans ca un titre contenant `&` cassait le
/// magnet genere : `split('&')` du parseur voyait un parametre sans
/// `=` et rejetait le lien. Unreserved RFC 3986 conserve, le reste
/// en %XX UTF-8 ; `+` est encode (il est litteral dans les magnets,
/// pas un espace).
pub(crate) fn percent_encode(s: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if UNRESERVED.contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_magnet_hex() {
        let m = MagnetLink::parse(
            "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d&dn=test&tr=udp%3A%2F%2Ft.local%3A80",
        )
        .unwrap();
        assert_eq!(
            m.info_hash_hex(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(m.display_name.as_deref(), Some("test"));
        assert_eq!(m.trackers, vec!["udp://t.local:80"]);
    }

    #[test]
    fn parse_magnet_base32() {
        // base32 du SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        // calcule : a9993e364706816aba3e25717850c26c9cd0d89d en base32.
        let hex =
            onionbit_crypto::hash::from_hex("a9993e364706816aba3e25717850c26c9cd0d89d").unwrap();
        let b32 = base32_encode_for_test(&hex);
        let uri = format!("magnet:?xt=urn:btih:{b32}");
        let m = MagnetLink::parse(&uri).unwrap();
        assert_eq!(
            m.info_hash_hex(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    /// `&` terminal, `&&` et flags sans `=` (`&fl`) sont ignores
    /// plutot que de rejeter tout le lien (comportement libtorrent).
    #[test]
    fn parse_magnet_tolere_params_vides_et_flags() {
        let uri = format!("magnet:?xt=urn:btih:{}&&dn=x&fl&", "a".repeat(40));
        let m = MagnetLink::parse(&uri).unwrap();
        assert_eq!(m.display_name.as_deref(), Some("x"));
        assert!(m.info_hash_v1.is_some());
    }

    #[test]
    fn parse_magnet_v2_btmh() {
        // Multihash sha2-256(0x12 0x20) + SHA-256("abc").
        let m = MagnetLink::parse(
            "magnet:?xt=urn:btmh:1220ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert!(m.info_hash_v1.is_none());
        assert_eq!(
            m.info_hash_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_rejette_sans_xt() {
        assert!(MagnetLink::parse("magnet:?dn=test").is_err());
        assert!(MagnetLink::parse("http://x").is_err());
    }

    /// Petit encodeur base32 pour les tests (reference inverse de
    /// `base32_decode`).
    fn base32_encode_for_test(data: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut out = String::new();
        let mut buf: u64 = 0;
        let mut bits = 0u32;
        for &b in data {
            buf = (buf << 8) | u64::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(ALPHABET[(buf >> bits) as usize & 31] as char);
            }
        }
        if bits > 0 {
            out.push(ALPHABET[(buf << (5 - bits)) as usize & 31] as char);
        }
        out
    }
}
