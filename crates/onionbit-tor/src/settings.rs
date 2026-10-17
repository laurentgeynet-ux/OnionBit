// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reglages runtime de la connectivite Tor — produits par
//! `DaemonConfig::to_core_config` depuis la section `tor`
//! (`configuration.json`, ADR-0028 §2).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use onionbit_network_policy::TorTargetPolicy;

/// Transport Tor utilise par le fetcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorMode {
    /// arti embarque (Tor officiel en Rust) — autonomie, chemin
    /// nominal.
    Embedded,
    /// Tor systeme via son SOCKS5 loopback (`socks5h:` — la
    /// « resolution » d'un `.onion` se fait cote Tor de toute
    /// facon, jamais en DNS local).
    External(SocketAddr),
}

/// Backend de recherche externe (un service cache joignable).
#[derive(Debug, Clone)]
pub struct OnionBackendSettings {
    /// Miroirs `.onion` interchangeables, dans l'ordre de
    /// preference — les adresses onion tombent ou changent, le
    /// backend bascule avant de rendre erreur. Tous sont
    /// implicitement allowlistes (inclus dans `target_policy`).
    pub mirrors: Vec<String>,
    /// Template d'URL de recherche (`{host}` et `{q}` substitues —
    /// ex. `"http://{host}/q.php?q={q}"`).
    pub url_template: String,
    /// Borne de resultats remontes par recherche — une page hostile
    /// ne doit pas gonfler l'UI (`tor.search.backends.<id>.max_results`).
    pub max_results: usize,
    /// Tentatives supplementaires par miroir sur echec de transport
    /// ou statut injoignable (`tor.search.backends.<id>.retries`) —
    /// un rendez-vous onion rate souvent de facon transitoire.
    pub retries: usize,
    /// Delai entre deux tentatives (`…retry_delay_ms`) — laisse
    /// arti construire un nouveau circuit de rendez-vous.
    pub retry_delay: Duration,
}

/// Passerelle de sortie onion pour les pairs sans Tor
/// (`PEER_FLAG_EXIT_TOR`, ADR-0028 §7 — phase 2).
#[derive(Debug, Clone)]
pub struct TorGatewaySettings {
    /// Sert les requetes `.onion` recues via les cellules
    /// `http-request` : la passerelle voit la requete en clair
    /// (comme tout exit) — opt-in explicite.
    pub enabled: bool,
    /// Requetes simultanees max par circuit entrant (par-dessus le
    /// semaphore `http_tunnel` existant).
    pub rate_limit_per_circuit: u32,
}

/// Reglages complets de la connectivite Tor.
#[derive(Debug, Clone)]
pub struct TorSettings {
    /// Connectivite active — `false` par defaut : jamais de
    /// bootstrap Tor silencieux (visible du FAI, censure).
    pub enabled: bool,
    /// Transport choisi.
    pub mode: TorMode,
    /// Repertoire d'etat arti (consensus, gardes, cache) — resolu
    /// `<state_dir>/tor` par `tor_settings()` (ADR-0028 §1, meme
    /// discipline d'arborescence portable qu'ADR-0018). Vide =
    /// bootstrap `embedded` impossible (jamais de fuite hors
    /// `state_dir` — un runtime mal configure refuse de demarrer).
    pub state_dir: PathBuf,
    /// Frontieres des cibles joignables (allowlist `.onion` —
    /// union de `allowed_hosts` et des miroirs des backends).
    pub target_policy: TorTargetPolicy,
    /// Borne d'une reponse onion (octets).
    pub max_response_bytes: usize,
    /// Timeout d'une requete.
    pub request_timeout: Duration,
    /// Timeout du bootstrap (premier consensus + gardes).
    pub bootstrap_timeout: Duration,
    /// Bornes de redirections 30x suivies (vers un hote
    /// allowliste seulement).
    pub max_redirects: u32,
    /// Backend `apibay` clearnet via un exit Tor (`tor.api_via_exit`)
    /// — desactive par defaut : anonymat degrade par rapport au
    /// fetch 100 % onion.
    pub api_via_exit: bool,
    /// Backends de recherche externes (`tpb`, …).
    pub backends: BTreeMap<String, OnionBackendSettings>,
    /// Passerelle de sortie pour les pairs sans Tor.
    pub gateway: TorGatewaySettings,
}

impl Default for TorSettings {
    /// Connectivite inactive : politique vide (tout fetch refuse),
    /// mode embarque, aucun backend.
    fn default() -> Self {
        Self {
            enabled: false,
            mode: TorMode::Embedded,
            state_dir: PathBuf::new(),
            target_policy: TorTargetPolicy::new(),
            max_response_bytes: 512 * 1024,
            request_timeout: Duration::from_secs(30),
            bootstrap_timeout: Duration::from_secs(120),
            max_redirects: 3,
            api_via_exit: false,
            backends: BTreeMap::new(),
            gateway: TorGatewaySettings {
                enabled: false,
                rate_limit_per_circuit: 10,
            },
        }
    }
}
