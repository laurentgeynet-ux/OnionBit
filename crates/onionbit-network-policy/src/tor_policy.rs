// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Politique des cibles Tor (ADR-0028 §3) — frontiere de la seule
//! surface de fetch ajoutee par la connectivite Tor.
//!
//! `TorTargetPolicy` borne les destinations a des hotes `.onion` v3
//! valides ET declares en allowlist (ports 80/443). Le fetcher Tor
//! n'est jamais un proxy generique : ni clearnet ni onion non
//! declares, ni autre port — et **jamais de sortie clearnet via un
//! exit** (`api_via_exit` a ete retire : l'anonymat d'un appel qui
//! sort du reseau est degrade par construction, pas un compromis
//! acceptable). Toute entree invalide est rejetee a l'insertion
//! (le chargeur de config warn + ignore).

use std::collections::BTreeSet;

use onionbit_format::onion::{onion_host, OnionAddr};

use crate::error::{PolicyError, Result};

/// Ports servis par le fetcher onion — HTTP/HTTPS d'un service
/// cache (pas de port exotique : la politique borne les canaux de
/// fuite et les usages non prevus).
pub const ALLOWED_ONION_PORTS: [u16; 2] = [80, 443];

/// Allowlist de cibles `.onion` joignables par le fetcher Tor.
#[derive(Debug, Clone, Default)]
pub struct TorTargetPolicy {
    /// Hotes allowlistes, forme normalisee `"<56 chars>.onion"`.
    allowed_hosts: BTreeSet<String>,
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

    /// `true` si `host` est une adresse v3 valide et allowlistee.
    pub fn is_allowed(&self, host: &str) -> bool {
        match OnionAddr::parse(host) {
            Ok(a) => self.allowed_hosts.contains(a.host()),
            Err(_) => false,
        }
    }

    /// Retire une entree de l'allowlist (`true` si elle existait —
    /// formes acceptees comme [`insert`]). Utilisee pour revoquer un
    /// pair dont l'adresse `.onion` associee est oubliee (ADR-0029).
    pub fn remove(&mut self, entry: &str) -> bool {
        let Ok(addr) = onion_host(entry) else {
            return false;
        };
        self.allowed_hosts.remove(addr.host())
    }

    /// Frontiere dure appliquee avant tout `connect` : `.onion` v3
    /// valide ET allowliste (ports 80/443) — rien d'autre.
    pub fn check(&self, host: &str, port: u16) -> Result<()> {
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

    /// Nombre d'hotes onion allowlistes.
    pub fn len(&self) -> usize {
        self.allowed_hosts.len()
    }

    /// `true` si la politique est vide (tout fetch refuse).
    pub fn is_empty(&self) -> bool {
        self.allowed_hosts.is_empty()
    }
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

    /// Clearnet jamais joignable — pas de sortie via un exit Tor
    /// (`api_via_exit` retire : anonymat degrade par construction).
    #[test]
    fn clearnet_toujours_refuse() {
        let mut p = TorTargetPolicy::new();
        p.insert(TPB).unwrap();
        for (host, port) in [
            ("apibay.org", 80),
            ("thepiratebay.org", 443),
            ("127.0.0.1", 80),
            ("localhost", 80),
        ] {
            assert!(p.check(host, port).is_err(), "{host}:{port} refuse");
        }
    }
}
