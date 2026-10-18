// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adresses de services caches Tor v3 (`.onion`) — extension
//! OnionBit (ADR-0028), sans equivalent dans Tribler.
//!
//! Forme (tor-rend-spec-v3) : `base32(pubkey ed25519 ‖ checksum ‖
//! version)` — 56 caracteres `[a-z2-7]` plus le suffixe `.onion`,
//! soit `base32` de 35 octets (32 + 2 + 1). Le checksum est
//! `SHA3-256(b".onion checksum" ‖ pubkey ‖ version)[0..2]` : une
//! adresse v3 est **auto-authentifiee** — la cle publique du service
//! est dans l'adresse elle-meme, ni DNS ni autorite de
//! certification n'interviennent.
//!
//! Vecteurs de test : annuaire curate `greyhat-academy/lists.d`
//! (`onion.domains.list.tsv`) — adresses reelles verifiees.

use std::fmt;
use std::str::FromStr;

use sha3::Digest;

use crate::error::{FormatError, Result};

/// Longueur du corps base32 d'une adresse v3 (hors suffixe).
pub const ONION_V3_BODY_LEN: usize = 56;

/// Suffixe canonique d'un service cache.
pub const ONION_SUFFIX: &str = ".onion";

/// Octet de version des adresses v3 (le seul supporte).
const VERSION_V3: u8 = 0x03;

/// Prefixe du checksum (tor-rend-spec-v3).
const CHECKSUM_PREFIX: &[u8] = b".onion checksum";

/// Nombre d'octets decodes : pubkey (32) ‖ checksum (2) ‖ version
/// (1).
const DECODED_LEN: usize = 35;

/// Adresse de service cache Tor v3 validee (checksum verifie).
///
/// `host` est la forme normalisee complete en minuscules
/// (`"<56 chars>.onion"`) ; `pubkey` est la cle ed25519 du service
/// decodee de l'adresse.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OnionAddr {
    host: String,
    pubkey: [u8; 32],
}

impl OnionAddr {
    /// Parse `"<56 chars base32>.onion"` (suffixe exige, casse
    /// tolerante — les adresses circulent en minuscules mais les
    /// copier-coller capitalisent parfois).
    ///
    /// Refuse : mauvaise longueur, alphabet hors `[a-z2-7]`, octet
    /// de version ≠ `0x03`, checksum incorrect — et toute forme
    /// non-`.onion` (un nom clearnet n'est pas une cible Tor).
    pub fn parse(s: &str) -> Result<Self> {
        let bad = |r: &'static str| FormatError::BadOnionAddr(r.into());
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        let body = lower
            .strip_suffix(ONION_SUFFIX)
            .ok_or_else(|| bad("suffixe .onion absent"))?;
        if body.len() != ONION_V3_BODY_LEN {
            return Err(bad("corps != 56 caracteres"));
        }
        let raw = base32_decode(body).ok_or_else(|| bad("alphabet base32 invalide"))?;
        if raw.len() != DECODED_LEN {
            return Err(bad("longueur decodee != 35 octets"));
        }
        if raw[34] != VERSION_V3 {
            return Err(bad("octet de version != 0x03"));
        }
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&raw[..32]);
        let mut h = sha3::Sha3_256::new();
        h.update(CHECKSUM_PREFIX);
        h.update(pubkey);
        h.update([VERSION_V3]);
        let digest = h.finalize();
        if digest[..2] != raw[32..34] {
            return Err(bad("checksum invalide"));
        }
        Ok(Self {
            host: format!("{body}{ONION_SUFFIX}"),
            pubkey,
        })
    }

    /// Parse `*[sous-domaine.]<56 chars>.onion` — la forme **hote**
    /// complete, sous-domaines toleres.
    ///
    /// Tor identifie le service par les **deux derniers** labels et
    /// transmet le nom complet au pair (arti `Host::Onion` →
    /// `StreamInstructions::Hs { hsid, hostname }`) :
    /// `www.bbcnews….onion` — la forme *publiee* par BBC — est une
    /// cible legitime qu'un parse strict casserait. Chaque label
    /// prefixant reste soumis a la grammaire d'hote
    /// (`[a-z0-9-]{1,63}`, jamais de tiret en tete/queue) ; un nom
    /// qui ne finit pas en `.onion` (`x.onion.evil.com`) est refuse
    /// par le suffixe, comme `parse`.
    ///
    /// Retourne `(host complet normalise, adresse du service)` — le
    /// host se transmet au connecteur, l'adresse sert de cle
    /// d'isolation (un sous-domaine ne change pas le service).
    pub fn parse_host(s: &str) -> Result<(String, Self)> {
        let bad = |r: &'static str| FormatError::BadOnionAddr(r.into());
        let lower = s.trim().to_ascii_lowercase();
        if lower.len() > 253 {
            return Err(bad("hote trop long"));
        }
        let body = lower
            .strip_suffix(ONION_SUFFIX)
            .ok_or_else(|| bad("suffixe .onion absent"))?;
        let (subs, base) = body
            .rsplit_once('.')
            .map_or((None, body), |(s, b)| (Some(s), b));
        if let Some(subs) = subs {
            for label in subs.split('.') {
                let ok = (1..=63).contains(&label.len())
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-');
                if !ok {
                    return Err(bad("label de sous-domaine invalide"));
                }
            }
        }
        let addr = Self::parse(&format!("{base}{ONION_SUFFIX}"))?;
        Ok((lower, addr))
    }

    /// Nom d'hote normalise complet (`"<56 chars>.onion"`).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Cle publique ed25519 du service (32 octets).
    pub fn pubkey(&self) -> &[u8; 32] {
        &self.pubkey
    }
}

impl fmt::Display for OnionAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.host)
    }
}

impl FromStr for OnionAddr {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// Extrait et valide l'hote onion d'une entree d'allowlist ou d'une
/// URL : `"pirate….onion"`, `"http://pirate….onion/path"` ou
/// `"https://pirate….onion"` donnent tous le meme [`OnionAddr`].
/// Rejette les entrees avec identifiants ou port explicite — une
/// allowlist designe des *hotes*, pas des cibles precuites.
pub fn onion_host(entry: &str) -> Result<OnionAddr> {
    let bad = |r: &'static str| FormatError::BadOnionAddr(r.into());
    let e = entry.trim();
    let e = e
        .strip_prefix("http://")
        .or_else(|| e.strip_prefix("https://"))
        .unwrap_or(e);
    if e.contains('@') {
        return Err(bad("identifiants interdits dans une entree onion"));
    }
    let host = e
        .split(['/', '?', '#'])
        .next()
        .ok_or_else(|| bad("entree vide"))?;
    if host.contains(':') {
        return Err(bad("port explicite interdit dans une entree onion"));
    }
    OnionAddr::parse(host)
}

/// Decode du base32 RFC 4648 (alphabet `a-z2-7`, sans padding —
/// forme canonique des adresses onion). Retourne `None` sur
/// caractere hors alphabet.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buf: u64 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'a'..=b'z' => c - b'a',
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Adresses reelles de `greyhat-academy/lists.d`
    /// (`onion.domains.list.tsv`) — toutes doivent valider.
    const VECTORS: &[&str] = &[
        "duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion",
        "dwnewsgngmhlplxy6o2twtfgjnrnjxbegbwqx6wnotdhkzt562tszfid.onion",
        "fpfjxcrmw437h6z2xl3w4czl55kvkmxpapg37bbopsafdu7q454byxid.onion",
        "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion",
        "lldan5gahapx5k7iafb3s4ikijc4ni7gx5iywdflkba5y2ezyg6sjgyd.onion",
        "featherdvtpi7ckdbkb2yxjfwx3oyvr3xjz3oo4rszylfzjdg6pbm3id.onion",
        "xao2lxsmia2edq2n5zxg6uahx6xox2t7bfjw6b5vdzsxi7ezmqob6qid.onion",
        "ueghr2hzndecdntou33mhymbbxj7pir74nwzhqr6drhxpbz3j272p4id.onion",
        "eh5esdnd6fkbkapfc6nuyvkjgbtnzq2is72lmpwbdbxepd2z7zbgzsqd.onion",
        "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid.onion",
        "xmrhfasfg5suueegrnc4gsgyi2tyclcy5oz7f5drnrodmdtob6t2ioyd.onion",
        "s3p666he6q6djb6u3ekjdkmoyd77w63zq6gqf6sde54yg6bdfqukz2qd.onion",
        "c6usaa6obkiahck7rkn2phbffb3strd375pfpxmawuexnmectjizyjad.onion",
        "keybase5wmilwokqirssclfnsqrjdsi7jdir5wy7y7iu3tanwmtp6oid.onion",
        "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion",
        "p53lf57qovyuvwsc6xnrppyply3vtqm7l6pcobkmyqsiofyeznfu5uqd.onion",
        "juhanurmihxlp77nkq76byazcldy2hlmovfu2epvl5ankdibsot4csyd.onion",
        "phobosxilamwcg75xt22id7aywkzol6q6rfl2flipcqoc4e4ahima5id.onion",
        "guardian2zotagl6tmjucg3lrhxdk4dw3lhbqnkvvkywawy3oqfoprid.onion",
        "qubesosfasa4zl44o4tws22di6kepyzfeqv3tg4e3ztknltfxqrymdad.onion",
        "7sk2kov2xwx6cbc32phynrifegg6pklmzs7luwcggtzrnlsolxxuyfyd.onion",
        "yq5jjvr7drkjrelzhut7kgclfuro65jjlivyzfmxiq2kyv5lickrl4qd.onion",
        "kitclu5euns4jdutq2fuusbuqj2bjrwglyxmnvzvwga7wiovbfuoqaqd.onion",
        "kitgayb6n6klaytma4feoblj3akwjpf727cv4hxcs5v3gpwwirsxfvyd.onion",
        "rurcblzhmdk22kttfkel2zduhyu3r6to7knyc7wiorzrx5gw4c3lftad.onion",
        "dwtqmjzvn2c6z2x462mmbd34ugjjrodowtul4jfbkexjuttzaqzcjyad.onion",
        "kycnotmezdiftahfmc34pqbpicxlnx3jbf5p7jypge7gdvduu7i6qjqd.onion",
        "digitazyyxyihwwzudp5syxxyn3qhcd63wqcha2dxpfqiyydmrgdiaad.onion",
        "monerod2jqtblreunnwtzqbjjj4slvsd3cjbhnzt2evwnry5u772l3qd.onion",
        "airvpn3epnw2fnsbx5x2ppzjs6vxtdarldas7wjyqvhscj7x43fxylqd.onion",
    ];

    #[test]
    fn adresses_reelles_valident() {
        for v in VECTORS {
            let a = OnionAddr::parse(v).unwrap_or_else(|e| panic!("{v} : {e}"));
            assert_eq!(a.host(), *v);
            assert_eq!(a.to_string(), *v);
            assert_ne!(a.pubkey(), &[0; 32]);
        }
    }

    #[test]
    fn casse_et_scheme_toleres() {
        let bare = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";
        assert_eq!(
            OnionAddr::parse(&bare.to_uppercase()).unwrap(),
            OnionAddr::parse(bare).unwrap()
        );
        assert_eq!(
            onion_host(&format!("http://{bare}/search")).unwrap().host(),
            bare
        );
        assert_eq!(onion_host(&format!("https://{bare}")).unwrap().host(), bare);
    }

    #[test]
    fn rejette_les_formes_invalides() {
        let good = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";
        for s in [
            // Checksum mute (un caractere change).
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coiX.onion",
            // Corps trop court / trop long.
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coi.onion",
            "apiratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion",
            // v2 (16 chars) et clearnet.
            "3g2upl4pq6kufc4m.onion",
            "piratebay.org",
            // Alphabet hors base32 (0,1,8,9) et suffixe faux.
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coi8.onion",
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onions",
            "",
        ] {
            assert!(OnionAddr::parse(s).is_err(), "{s} devrait etre refuse");
        }
        // Version byte falsifiee : reconstruit une adresse avec le
        // dernier char base32 change (le version 0x03 vit dedans).
        let mut mutated = good[..55].to_string();
        mutated.push_str(if good.ends_with("d.onion") {
            "e.onion"
        } else {
            "d.onion"
        });
        assert!(OnionAddr::parse(&mutated).is_err());
    }

    #[test]
    fn parse_host_sous_domaines_toleres() {
        let base = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";
        // Forme nue : identique a `parse`.
        let (h, a) = OnionAddr::parse_host(base).unwrap();
        assert_eq!(h, base);
        assert_eq!(a.host(), base);
        // Forme publiee par BBC : sous-domaine + base v3.
        let (h, a) = OnionAddr::parse_host(&format!("www.{base}")).unwrap();
        assert_eq!(h, format!("www.{base}"));
        assert_eq!(a.host(), base);
        // Casse normalisee, multi-labels, cle d'isolation = service.
        let (h, a2) = OnionAddr::parse_host(&format!("WWW.x.Y.{base}")).unwrap();
        assert_eq!(h, format!("www.x.y.{base}"));
        assert_eq!(a2, a);
    }

    #[test]
    fn parse_host_rejette_les_formes_invalides() {
        let base = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";
        for s in [
            // Le suffixe `.onion` doit terminer le nom — pas de
            // `.onion.evil.com`.
            "x.onion.evil.com",
            &format!("{base}.evil.com"),
            // Labels vides, tirets mal places, alphabet invalide,
            // label > 63.
            &format!(".{base}"),
            &format!("a..{base}"),
            &format!("-www.{base}"),
            &format!("www-.{base}"),
            &format!("so_us.{base}"),
            &format!("{}.{base}", "a".repeat(64)),
            // La base reste strictement v3 — un sous-domaine ne
            // raccourcit pas l'exigence.
            "www.pas-un.onion",
            "www.3g2upl4pq6kufc4m.onion",
            // Checksum muté sur la base.
            "www.piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coix.onion",
        ] {
            assert!(OnionAddr::parse_host(s).is_err(), "{s} devrait etre refuse");
        }
    }

    #[test]
    fn onion_host_rejette_auth_et_port() {
        for e in [
            "http://user:pw@piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion",
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion:8080",
            "ftp://piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion",
        ] {
            assert!(onion_host(e).is_err(), "{e} devrait etre refuse");
        }
    }
}
