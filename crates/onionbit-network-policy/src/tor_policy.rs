// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Politique des cibles Tor (ADR-0028 §3) — frontiere de la seule
//! surface de fetch ajoutee par la connectivite Tor.
//!
//! `TorTargetPolicy` borne les destinations a des hotes `.onion` v3
//! valides ET declares en allowlist (ports 80/443), plus — opt-in
//! `api_via_exit` — des noms DNS clearnet declares joignables via
//! un noeud de sortie (port 80 seulement). Le fetcher Tor n'est
//! jamais un proxy generique : ni clearnet ni onion non declares,
//! ni autre port. Toute entree invalide est rejetee a l'insertion
//! (le chargeur de config warn + ignore).

use std::collections::BTreeSet;
use std::net::IpAddr;

use onionbit_format::onion::{onion_host, OnionAddr};

use crate::error::{PolicyError, Result};

/// Ports servis par le fetcher onion — HTTP/HTTPS d'un service
/// cache (pas de port exotique : la politique borne les canaux de
/// fuite et les usages non prevus).
pub const ALLOWED_ONION_PORTS: [u16; 2] = [80, 443];

/// Ports des hotes clearnet de sortie (`api_via_exit`) — HTTP/HTTPS
/// seulement (TLS est implemente par-dessus le flux Tor : un exit
/// ne peut ni lire ni reecrire la reponse).
pub const ALLOWED_EXIT_PORTS: [u16; 2] = [80, 443];

/// Allowlist de cibles `.onion` joignables par le fetcher Tor.
#[derive(Debug, Clone, Default)]
pub struct TorTargetPolicy {
    /// Hotes allowlistes, forme normalisee `"<56 chars>.onion"`.
    allowed_hosts: BTreeSet<String>,
    /// Hotes clearnet joignables **via un exit Tor** — la famille
    /// `api_via_exit` (ADR-0028 §5, opt-in : la requete traverse un
    /// noeud de sortie qui la voit en clair, anonymat degrade par
    /// rapport au fetch 100 % onion). Distincte de
    /// `allowed_hosts` : jamais d'onion ici, jamais d'IP litterale
    /// — uniquement des noms DNS publics sur :80.
    exit_hosts: BTreeSet<String>,
}

impl TorTargetPolicy {
    /// Politique vide — rien n'est joignable tant qu'aucun hote
    /// n'est declare.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insere une entree d'allowlist (`"pirate….onion"` ou URL
    /// `"http://pirate….onion/path"` — le host est extrait et valide).
    /// `Err` si l'entree n'est pas une adresse onion v3 valide.
    pub fn insert(&mut self, entry: &str) -> Result<()> {
        let addr = onion_host(entry).map_err(|_| {
            PolicyError::DeniedDestination("entree allowlist non-onion ou invalide")
        })?;
        self.allowed_hosts.insert(addr.host().to_string());
        Ok(())
    }

    /// Insere un hote clearnet de sortie (`api_via_exit`) —
    /// `"apibay.org"` ou `"http://apibay.org/path"` (le host est
    /// extrait et valide). `Err` sur `.onion`, IP litterale ou nom
    /// DNS invalide : la sortie ne designe que des hotes publics.
    pub fn insert_exit_host(&mut self, entry: &str) -> Result<()> {
        let host = exit_host(entry).ok_or(PolicyError::DeniedDestination(
            "hote de sortie invalide (nom DNS public attendu)",
        ))?;
        self.exit_hosts.insert(host);
        Ok(())
    }

    /// `true` si `host` est une adresse v3 valide et allowlistee.
    pub fn is_allowed(&self, host: &str) -> bool {
        match OnionAddr::parse(host) {
            Ok(a) => self.allowed_hosts.contains(a.host()),
            Err(_) => false,
        }
    }

    /// `true` si `host` est un hote clearnet de sortie declare.
    pub fn is_exit_allowed(&self, host: &str) -> bool {
        self.exit_hosts.contains(&host.to_ascii_lowercase())
    }

    /// Frontiere dure appliquee avant tout `connect` : `.onion` v3
    /// valide ET allowliste (ports 80/443), ou hote clearnet de
    /// sortie declare (port 80 seulement).
    pub fn check(&self, host: &str, port: u16) -> Result<()> {
        if self.exit_hosts.contains(host) {
            return if ALLOWED_EXIT_PORTS.contains(&port) {
                Ok(())
            } else {
                Err(PolicyError::DeniedDestination(
                    "port de sortie hors politique (80/443)",
                ))
            };
        }
        if !ALLOWED_ONION_PORTS.contains(&port) {
            return Err(PolicyError::DeniedDestination(
                "port onion hors politique (80/443)",
            ));
        }
        let addr = OnionAddr::parse(host)
            .map_err(|_| PolicyError::DeniedDestination("hote n'est pas une adresse onion v3"))?;
        if !self.allowed_hosts.contains(addr.host()) {
            return Err(PolicyError::DeniedDestination(
                "hote onion absent de l'allowlist",
            ));
        }
        Ok(())
    }

    /// Nombre d'hotes joignables (onion allowlistes + sorties
    /// clearnet declarees).
    pub fn len(&self) -> usize {
        self.allowed_hosts.len() + self.exit_hosts.len()
    }

    /// `true` si la politique est vide (tout fetch refuse).
    pub fn is_empty(&self) -> bool {
        self.allowed_hosts.is_empty() && self.exit_hosts.is_empty()
    }
}

/// Extrait et valide un hote clearnet de sortie — meme discipline
/// d'extraction que [`onion_host`] (scheme optionnel, pas
/// d'identifiants ni de port explicite, chemin ignore). Un hote de
/// sortie est un nom DNS **public** : jamais d'`.onion` (canal
/// dedie), jamais d'IP litterale, jamais de nom simple non
/// qualifie (`localhost`…).
fn exit_host(entry: &str) -> Option<String> {
    let e = entry.trim();
    let e = e
        .strip_prefix("http://")
        .or_else(|| e.strip_prefix("https://"))
        .unwrap_or(e);
    if e.contains('@') {
        return None;
    }
    let host = e.split(['/', '?', '#']).next()?;
    if host.contains(':') {
        return None;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.ends_with(".onion")
        || !host.contains('.')
        || host.parse::<IpAddr>().is_ok()
    {
        return None;
    }
    let valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    valid.then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPB: &str = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";
    const AHMIA: &str = "juhanurmihxlp77nkq76byazcldy2hlmovfu2epvl5ankdibsot4csyd.onion";

    #[test]
    fn check_exige_v3_allowliste_et_port_http() {
        let mut p = TorTargetPolicy::new();
        p.insert(TPB).unwrap();
        assert!(p.check(TPB, 80).is_ok());
        assert!(p.check(TPB, 443).is_ok());
        // Port hors politique.
        assert!(p.check(TPB, 8080).is_err());
        // Onion valide mais non declare.
        assert!(p.check(AHMIA, 80).is_err());
        // Clearnet — jamais joignable par le fetcher.
        assert!(p.check("thepiratebay.org", 80).is_err());
        assert!(p.check("127.0.0.1", 80).is_err());
        // Formes invalides.
        assert!(p.check("pas-une-adresse.onion", 80).is_err());
        assert!(p.check("", 80).is_err());
    }

    #[test]
    fn insert_valide_et_normalise() {
        let mut p = TorTargetPolicy::new();
        // URL -> host extrait ; doublon deduplique par le set.
        p.insert(&format!("http://{TPB}/search.php")).unwrap();
        p.insert(&TPB.to_uppercase()).unwrap();
        assert_eq!(p.len(), 1);
        assert!(p.is_allowed(TPB));
        // Entrees rejetees.
        for bad in [
            "thepiratebay.org",
            "3g2upl4pq6kufc4m.onion", // v2
            "http://user:pw@x.onion",
            "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coiX.onion",
            "",
        ] {
            assert!(p.insert(bad).is_err(), "{bad} devrait etre refuse");
        }
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn politique_vide_refuse_tout() {
        let p = TorTargetPolicy::new();
        assert!(!p.is_allowed(TPB));
        assert!(p.check(TPB, 80).is_err());
        assert!(p.is_empty());
    }

    #[test]
    fn sortie_clearnet_declaree_ports_web() {
        let mut p = TorTargetPolicy::new();
        p.insert_exit_host("apibay.org").unwrap();
        p.insert_exit_host("http://APIBAY.org/q.php").unwrap();
        assert_eq!(p.len(), 1);
        assert!(p.is_exit_allowed("apibay.org"));
        assert!(p.check("apibay.org", 80).is_ok());
        assert!(p.check("apibay.org", 443).is_ok());
        // Un hote de sortie n'ouvre rien d'autre.
        assert!(p.check("apibay.org", 8080).is_err());
        assert!(p.check("autre.org", 80).is_err());
        // Un clearnet non declare reste refuse.
        assert!(p.check("thepiratebay.org", 80).is_err());
        // Et un onion declare n'est pas une sortie.
        p.insert(TPB).unwrap();
        assert!(!p.is_exit_allowed(TPB));
        assert!(p.check(TPB, 80).is_ok());
    }

    #[test]
    fn sortie_clearnet_rejette_onion_ip_et_noms_internes() {
        let mut p = TorTargetPolicy::new();
        for bad in [
            TPB,                    // onion → allowlist dediee
            "127.0.0.1",            // IPv4 litterale
            "10.0.0.1",             // prive
            "::1",                  // IPv6 litterale
            "localhost",            // nom non qualifie
            "intranet",             // label unique
            "http://user:pw@x.org", // identifiants
            "apibay.org:8080",      // port explicite
            "a..b.org",             // label vide
            "-bad-.org",            // tirets extremes
            "sous_score.org",       // caractere hors DNS
            "",                     // vide
        ] {
            assert!(
                p.insert_exit_host(bad).is_err(),
                "{bad} devrait etre refuse"
            );
        }
        assert!(p.is_empty());
    }
}
