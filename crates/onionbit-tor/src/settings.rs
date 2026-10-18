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

/// Service `.onion` d'identite (ADR-0029 etape 122) — hebergement
/// opt-in du service cache dont la cle est derivee de la graine
/// (`onionbit/identity/tor-hs/v1`).
#[derive(Debug, Clone, Default)]
pub struct OnionServiceSettings {
    /// Publie le service `.onion` d'identite quand la lane est
    /// `Ready` — **`false` par defaut** : jamais de publication
    /// sans geste explicite (regle ADR-0028 §8 : aucune adresse
    /// affichee tant que rien n'est heberge).
    pub enabled: bool,
    /// Publie le service `.onion` **coffre** (etape 127) — adresse
    /// `tor-hs-vault/v1` dediee, descripteur verrouille par
    /// l'autorisation client x25519 derivee de la meme identite.
    /// **`false` par defaut** (le coffre n'est servi que sur geste).
    pub vault_enabled: bool,
}

/// Reglages du proxy navigateur (ADR-0030, `tor/browser_proxy`) —
/// le `TorProxyServer` vit et meurt avec le runtime (`swap_tor_stack`).
#[derive(Debug, Clone)]
pub struct BrowserProxySettings {
    /// Proxy HTTP forward loopback actif — **`false` par defaut**.
    pub enabled: bool,
    /// Adresse d'ecoute — loopback exige par le proxy lui-meme ;
    /// `:0` = port ephemere choisi par l'OS, publie par
    /// `GET /api/tor/status`.
    pub listen: SocketAddr,
    /// Exige `Proxy-Authorization: Basic` (token de session genere
    /// au demarrage, transmis au navigateur par le dillorc
    /// ephemere — jamais en argv, jamais persisté). **`false` par
    /// defaut** tant que le canal `http_proxypass` n'est pas lu
    /// cote navigateur (etape 133) — le modele loopback seul est le
    /// repli documente ADR-0030 §Securite.
    pub require_auth: bool,
    /// Sortie clearnet vía exit Tor (ADR-0030 §4, tri-state §8.3) —
    /// `Off` par defaut : la politique reste `.onion`-only.
    pub clearnet_exit: onionbit_network_policy::ClearnetExit,
    /// Connexions simultanees max.
    pub max_conns: usize,
    /// En-tetes de requete max avant rejet 400 (octets).
    pub max_header_bytes: usize,
    /// Fermeture d'une connexion inactive.
    pub idle_timeout: Duration,
    /// Entrees max de la map d'isolation par origine.
    pub isolation_max_entries: usize,
}

impl Default for BrowserProxySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            require_auth: false,
            clearnet_exit: onionbit_network_policy::ClearnetExit::Off,
            max_conns: 64,
            max_header_bytes: 16 * 1024,
            idle_timeout: Duration::from_secs(120),
            isolation_max_entries: 256,
        }
    }
}

/// Binaire navigateur lance par `POST /api/browser/launch`
/// (ADR-0030, `tor/browser`) — chemin et args viennent
/// **uniquement** de la config : l'API ne fait que declencher,
/// jamais d'argv pilote par l'appelant.
#[derive(Debug, Clone, Default)]
pub struct BrowserSettings {
    /// Chemin du binaire (`bin/dillo.exe` dans le bundle). Vide =
    /// navigateur non configure → `browser_not_configured`.
    /// Relatif : resolu par rapport au repertoire de l'executable
    /// du daemon (arborescence portable ADR-0018).
    pub binary_path: PathBuf,
    /// Arguments supplementaires fixes (config, jamais d'API) —
    /// l'URL validee est appendee en dernier argument.
    pub extra_args: Vec<String>,
    /// `false` (defaut) : profil ephemere `browser/run-<rand>` supprime
    /// a la fermeture du navigateur — la trace persistante (hsts,
    /// comm_keys, fichiers dpi vivant hors DB) n'existe que le temps
    /// de la session. `true` : profil stable `browser/` conserve —
    /// persistance assumee en clair (ADR-0030 §134).
    pub keep_profile: bool,
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
    /// Backends de recherche externes (`tpb`, …).
    pub backends: BTreeMap<String, OnionBackendSettings>,
    /// Lignes de ponts Tor (relays non listés — contourne le blocage
    /// par IP des gardes publics ; `tor/bridges`, anti-censure). Des
    /// lignes avec transport PT (`obfs4`, `snowflake`…) exigent la
    /// feature `pt-client` — non compilée : elles sont refusées au
    /// chargement. Liste non vide = bootstrap **ponts uniquement**
    /// (jamais de repli silencieux sur les gardes publics — ça
    /// contournerait l'intention de l'utilisateur).
    pub bridges: Vec<String>,
    /// Passerelle de sortie pour les pairs sans Tor.
    pub gateway: TorGatewaySettings,
    /// Service `.onion` d'identite a heberger (ADR-0029).
    pub onion_service: OnionServiceSettings,
    /// Proxy navigateur (`tor/browser_proxy`, ADR-0030).
    pub browser_proxy: BrowserProxySettings,
    /// Binaire navigateur (`tor/browser`, ADR-0030).
    pub browser: BrowserSettings,
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
            backends: BTreeMap::new(),
            bridges: Vec::new(),
            gateway: TorGatewaySettings {
                enabled: false,
                rate_limit_per_circuit: 10,
            },
            onion_service: OnionServiceSettings::default(),
            browser_proxy: BrowserProxySettings::default(),
            browser: BrowserSettings::default(),
        }
    }
}
