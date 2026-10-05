// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration persistée du daemon — `state_dir/configuration.json`.
//!
//! Équivalent de `tribler.onionbit_config` (`TriblerConfig` +
//! `TriblerConfigManager`) : arbre de réglages complet avec défauts,
//! chargé au démarrage du daemon, réécrit après chaque `POST
//! /api/settings` (merge récursif, fidèle à
//! `SettingsEndpoint._recursive_merge_settings` qui fait `set("a/b", v)`
//! par feuille — équivalent à une fusion profonde des objets JSON).
//!
//! Les clés inconnues sont préservées (`extra`, serde `flatten`) comme
//! le dict Python les conserve.
//!
//! Écart assumé : `libtorrent/download_defaults/saveas` vide (défaut)
//! est résolu en `<state_dir>/downloads` au lieu de `~/Downloads` —
//! le daemon n'écrit jamais hors de son répertoire d'état sans
//! configuration explicite.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Nom du fichier de configuration dans `state_dir`.
pub const CONFIG_FILENAME: &str = "configuration.json";

/// Version courante du schéma de `configuration.json` — extension
/// Rust (`TriblerConfig` Python n'a pas de marqueur : l'écriture
/// complète du fichier y gèle les défauts de l'époque sans recours).
/// Clé absente → fichier legacy `0` → migrations appliquées au
/// chargement (`migrate_legacy_tree`), puis le fichier est réécrit
/// estampillé. Incrémenter à chaque nouvelle table de migration.
///
/// v1 : migrations de booléens gelés (`guards_enabled`). v2 :
/// écriture **sparse** — le fichier ne persiste que les écarts aux
/// défauts (`DaemonConfig::write` → `deep_diff`) — + réalignement de
/// `tunnel_community/bandwidth/target_delay_ms` (50 → 25 ms).
pub const CURRENT_CONFIG_VERSION: u32 = 2;

/// Merge JSON profond (`_recursive_merge_settings` Python) : les objets
/// se fusionnent clé par clé, toute autre valeur remplace.
fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                deep_merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Diff JSON profond contre les défauts : `None` quand `current`
/// vaut `default` (rien à persister), sinon la sous-arborescence des
/// écarts. Un objet qui ne diffère en rien produit `None` — les clés
/// absentes sont remplies par `#[serde(default)]` au chargement.
fn deep_diff(current: &Value, default: &Value) -> Option<Value> {
    match (current, default) {
        (Value::Object(cur), Value::Object(def)) => {
            let mut out = serde_json::Map::new();
            for (k, v) in cur {
                match def.get(k) {
                    Some(dv) => {
                        if let Some(d) = deep_diff(v, dv) {
                            out.insert(k.clone(), d);
                        }
                    }
                    // Clé inconnue du défaut (section `extra`,
                    // `ui` libre) : choix explicite → persistée.
                    None => {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            (!out.is_empty()).then_some(Value::Object(out))
        }
        (cur, def) if cur == def => None,
        (cur, _) => Some(cur.clone()),
    }
}

/// Section `api` — écoute HTTP(S) du plan de contrôle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Clé API hex (générée au premier run — `ApiKeyMiddleware`).
    pub key: String,
    /// Expose l'API HTTP.
    pub http_enabled: bool,
    /// Hôte d'écoute (loopback uniquement — politique réseau).
    pub http_host: String,
    /// Port demandé (0 = aléatoire, publié dans `http_port_running`).
    /// Défaut 8085 — l'UI tente ce port d'abord (Python utilise 0 car
    /// son GUI lit `http_port_running` dans le fichier de config ;
    /// notre UI découvre le daemon par scan, un port fixe évite le
    /// scan à chaque démarrage). Si le port est pris, repli éphémère.
    pub http_port: u16,
    /// HTTPS désactivé par défaut (comme Python).
    pub https_enabled: bool,
    /// Hôte HTTPS.
    pub https_host: String,
    /// Port HTTPS.
    pub https_port: u16,
    /// Certificat PEM pour HTTPS.
    pub https_certfile: String,
    /// Port HTTP réellement lié (écrit par le daemon au runtime).
    pub http_port_running: u16,
    /// Port HTTPS réellement lié.
    pub https_port_running: u16,
    /// Sert l'interface web Flutter sous `/` (statiques exemptes
    /// d'authentification — parite des exemptions `/ui`/`/static` de
    /// l'`ApiKeyMiddleware` Python ; `/api/*` reste derriere la cle).
    /// Extension Rust : pas d'equivalent dans `TriblerConfig`.
    pub web_ui_enabled: bool,
    /// Repertoire du build web (`index.html` a la racine) ; vide =
    /// detection automatique par le daemon (`<exe>/web`,
    /// `state_dir/web`, puis `app/build/web` du depot en dev).
    pub web_ui_dir: String,
    /// Injecte la cle API dans `index.html` servi (meta
    /// `onionbit-api-key`) : l'UI web se connecte sans saisie, comme
    /// la GUI desktop qui lit `configuration.json`. Sans risque hors
    /// loopback : le daemon ne bind que sur 127.0.0.1 et un autre
    /// site ne peut pas lire la reponse (same-origin policy).
    /// `false` = saisie manuelle de la cle dans l'UI.
    pub web_ui_inject_key: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            key: String::new(),
            http_enabled: true,
            http_host: "127.0.0.1".into(),
            http_port: 8085,
            https_enabled: false,
            https_host: "127.0.0.1".into(),
            https_port: 0,
            https_certfile: "https_certfile".into(),
            http_port_running: 0,
            https_port_running: 0,
            web_ui_enabled: true,
            web_ui_dir: String::new(),
            web_ui_inject_key: true,
        }
    }
}

/// Interface d'écoute IPv8 (`ipv8/interfaces` pyipv8).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8Interface {
    /// Nom de l'interface (`UDPIPv4`, `UDPIPv6`).
    pub interface: String,
    /// Adresse de bind.
    pub ip: String,
    /// Port UDP.
    pub port: u16,
    /// Threads de traitement (optionnel pyipv8 — sans equivalent
    /// Tokio : le runtime gere les workers ; conserve pour
    /// compatibilite du fichier, non lu — comme chez pyipv8 quand
    /// non configure).
    pub worker_threads: Option<u32>,
}

impl Default for Ipv8Interface {
    fn default() -> Self {
        Self {
            interface: "UDPIPv4".into(),
            ip: "0.0.0.0".into(),
            port: crate::ipv8_stack::DEFAULT_IPV8_PORT,
            worker_threads: None,
        }
    }
}

/// Section `ipv8/bootstrap` : `override` remplace la liste d'amorçage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8Bootstrap {
    /// Noeuds d'amorçage explicites (remplace la liste par défaut si
    /// non vide — comme `bootstrap.override` pyipv8).
    #[serde(rename = "override")]
    pub override_peers: Vec<String>,
}

/// Section `ipv8` — moteur overlay (découverte, communities).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ipv8FileConfig {
    /// Active la stack IPv8.
    pub enabled: bool,
    /// Interfaces d'écoute UDP (défaut : IPv4 8090 + IPv6 8091, pyipv8).
    pub interfaces: Vec<Ipv8Interface>,
    /// Liste d'amorçage personnalisée.
    pub bootstrap: Ipv8Bootstrap,
    /// Intervalle des stratégies de découverte (s).
    pub walker_interval: f64,
    /// Niveau de log (DEBUG/INFO/WARNING/ERROR).
    pub logger_level: String,
    /// Estimation WAN forcee `"ip:port"` — extension Rust reservee aux
    /// bancs loopback : une `destination_address` en 127/8 n'est jamais
    /// retenue comme WAN (`address_in_lan_subnets` pyipv8), ce qui
    /// laisserait le DHT muet (`on_node_discovered` refuse tout noeud
    /// tant que `my_estimated_wan` est inconnu). Vide = comportement
    /// normal (WAN appris via introduction-response).
    pub estimated_wan: String,
    /// Clés/communities additionnelles non portées — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for Ipv8FileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interfaces: vec![
                Ipv8Interface::default(),
                Ipv8Interface {
                    interface: "UDPIPv6".into(),
                    ip: "::".into(),
                    port: crate::ipv8_stack::DEFAULT_IPV8_PORT + 1,
                    worker_threads: None,
                },
            ],
            bootstrap: Ipv8Bootstrap::default(),
            walker_interval: 0.5,
            logger_level: "INFO".into(),
            estimated_wan: String::new(),
            extra: serde_json::Map::new(),
        }
    }
}

/// `libtorrent/download_defaults` — réglages par défaut des ajouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadDefaultsConfig {
    /// Anonymat activé par défaut pour les nouveaux téléchargements.
    pub anonymity_enabled: bool,
    /// Nombre de sauts par défaut (0 = non anonyme, max 3).
    pub number_hops: u32,
    /// Seeding sûr — requis si `number_hops > 0`.
    pub safeseeding_enabled: bool,
    /// Dossier de destination (`""` = `<state_dir>/downloads` — écart
    /// assumé vs `~/Downloads` Python, cf. note du module).
    pub saveas: String,
    /// Mode de seed : `forever` / `never` / `ratio` / `time`.
    pub seeding_mode: String,
    /// Ratio cible si `seeding_mode == "ratio"`.
    pub seeding_ratio: f64,
    /// Durée de seed cible (s) si `seeding_mode == "time"`.
    pub seeding_time: f64,
    /// Téléchargement de channel.
    pub channel_download: bool,
    /// Ajoute le téléchargement à un channel.
    pub add_download_to_channel: bool,
    /// Fichier de trackers par défaut.
    pub trackers_file: String,
    /// URL de synchronisation de `trackers_file`.
    pub trackers_file_sync_url: String,
    /// Sauvegarde des .torrent dans ce dossier.
    pub torrent_folder: String,
    /// auto_managed par défaut (file d'attente).
    pub auto_managed: bool,
    /// Déplacement après complétion.
    pub completed_dir: String,
}

impl Default for DownloadDefaultsConfig {
    fn default() -> Self {
        Self {
            anonymity_enabled: true,
            number_hops: 1,
            safeseeding_enabled: true,
            saveas: String::new(),
            seeding_mode: "forever".into(),
            seeding_ratio: 2.0,
            seeding_time: 60.0,
            channel_download: false,
            add_download_to_channel: false,
            trackers_file: String::new(),
            trackers_file_sync_url: String::new(),
            torrent_folder: String::new(),
            auto_managed: false,
            completed_dir: String::new(),
        }
    }
}

/// Section `libtorrent` — réglages de la session BitTorrent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibtorrentConfig {
    /// Ports SOCKS5 par lane d'anonymat (sous `download_defaults` dans
    /// Tribler récent — conservé ici pour compatibilité ascendante).
    pub socks_listen_ports: Vec<u16>,
    /// Interface d'écoute.
    pub listen_interface: String,
    /// Port d'écoute (0 = aléatoire).
    pub port: u16,
    /// Interface IPv6 ("" = désactivée).
    pub listen_interface_v6: String,
    /// Port IPv6.
    pub port_v6: u16,
    /// Type de proxy (enum libtorrent : 0 aucun, 2/3 socks5, 4/5 http).
    pub proxy_type: i64,
    /// Proxy `host:port`.
    pub proxy_server: String,
    /// Auth proxy `user:pass`.
    pub proxy_auth: String,
    /// Connexions max par téléchargement (-1 = illimité).
    pub max_connections_download: i64,
    /// Limite globale download (o/s, 0 = illimité).
    pub max_download_rate: u64,
    /// Limite globale upload (o/s, 0 = illimité).
    pub max_upload_rate: u64,
    /// Transport uTP.
    pub utp: bool,
    /// Plafond du buffer de réception uTP par connexion, en octets
    /// (extension Rust — pas de réglage libtorrent équivalent) : uTP
    /// bufferise en espace utilisateur, ce buffer est aussi la
    /// fenêtre de réception annoncée au pair et donc le débit maximal
    /// d'une connexion (`fenêtre / RTT`). `0` = défaut librqbit-utp
    /// (1 Mio). S'applique à la session en clair et aux lanes
    /// anonymes — cf. `docs/diagnostics/memoire_charge_reelle.md`.
    pub utp_rx_buf_size: u64,
    /// Plafond du buffer d'émission uTP par connexion, en octets
    /// (extension Rust) : borne les données non acquittées stockées
    /// (mémoire TX pire cas par connexion). `0` = défaut
    /// librqbit-utp (croissance 32 Kio → 1 Mio).
    pub utp_tx_buf_max: u64,
    /// DHT mainline BEP 5.
    pub dht: bool,
    /// Timeout de readiness DHT (s).
    pub dht_readiness_timeout: u64,
    /// UPnP.
    pub upnp: bool,
    /// NAT-PMP.
    pub natpmp: bool,
    /// Local service discovery.
    pub lsd: bool,
    /// Annonce à tous les tiers de trackers.
    pub announce_to_all_tiers: bool,
    /// Annonce à tous les trackers.
    pub announce_to_all_trackers: bool,
    /// Annonces HTTP simultanées max.
    pub max_concurrent_http_announces: u64,
    /// Re-hash après complétion.
    pub check_after_complete: bool,
    /// Échantillonnage des pièces à la restauration fastresume
    /// (extension OnionBit, sans équivalent Python —
    /// `fastresume_sampled_check` librqbit) : `true` (défaut) relit
    /// quelques pièces par torrent au démarrage pour détecter des
    /// fichiers modifiés ; `false` fait confiance au `.bitv` tel quel
    /// — démarrage quasi instantané, pas de détection de corruption.
    pub fastresume_check: bool,
    /// File d'attente : téléchargements actifs.
    pub active_downloads: i64,
    /// File d'attente : seeds actifs.
    pub active_seeds: i64,
    /// File d'attente : vérifications actives.
    pub active_checking: i64,
    /// Limite DHT active.
    pub active_dht_limit: i64,
    /// Limite trackers active.
    pub active_tracker_limit: i64,
    /// Limite LSD active.
    pub active_lsd_limit: i64,
    /// Limite globale active.
    pub active_limit: i64,
    /// Demande les réglages à l'ajout (émet `ask_add_download`).
    pub ask_download_settings: bool,
    /// Nettoie les .parts orphelins.
    pub clear_orphaned_parts: bool,
    /// Fichiers mappés mémoire.
    pub allow_mmap: bool,
    /// Défauts des nouveaux téléchargements.
    pub download_defaults: DownloadDefaultsConfig,
    /// Clés libtorrent additionnelles (`advanced_rate_limits`, …).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for LibtorrentConfig {
    fn default() -> Self {
        Self {
            socks_listen_ports: vec![0; 5],
            listen_interface: "0.0.0.0".into(),
            port: 0,
            listen_interface_v6: String::new(),
            port_v6: 0,
            proxy_type: 0,
            proxy_server: String::new(),
            proxy_auth: String::new(),
            max_connections_download: -1,
            max_download_rate: 0,
            max_upload_rate: 0,
            utp: true,
            utp_rx_buf_size: 0,
            utp_tx_buf_max: 0,
            dht: true,
            dht_readiness_timeout: 30,
            upnp: true,
            natpmp: true,
            lsd: true,
            announce_to_all_tiers: false,
            announce_to_all_trackers: false,
            max_concurrent_http_announces: 50,
            check_after_complete: false,
            fastresume_check: true,
            active_downloads: 3,
            active_seeds: 5,
            active_checking: 1,
            active_dht_limit: 88,
            active_tracker_limit: 1600,
            active_lsd_limit: 60,
            active_limit: 500,
            ask_download_settings: false,
            clear_orphaned_parts: false,
            allow_mmap: true,
            download_defaults: DownloadDefaultsConfig::default(),
            extra: serde_json::Map::new(),
        }
    }
}

/// Contrôleur de congestion du débit servi (`tunnel_community/
/// bandwidth`) — extension Rust, famille LEDBAT/USS : le plafond
/// servi en mode `max_relayed_rate = -1` n'est PAS une fraction de
/// capacité mesurée (l'estimation de capacité est impossible sans
/// sonde externe : le pic passif est censuré par son propre
/// plafond). À chaque tick `sample_secs`, on ping `probe_peers`
/// pairs vérifiés ; le minimum des RTT moins la baseline (min sur
/// `base_window_secs`) donne le retard de file d'attente montante —
/// la file d'émission locale étant commune à tous les paquets, le
/// min isole notre congestion de la distance/congestion des pairs.
/// En dessous de `target_delay_ms` le plafond croît de façon
/// additive, au-dessus il décroît multiplicativement — le tunnel
/// occupe l'upload disponible et cède la place dès qu'une autre
/// application (ou le téléchargement local) charge la ligne.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BandwidthConfig {
    /// Plancher du plafond servi (octets/s) : un relais sous ce
    /// débit n'apporte rien au réseau.
    pub floor_bps: u64,
    /// Plafond servi initial, avant le premier échantillon RTT
    /// (octets/s).
    pub fallback_bps: u64,
    /// Borne supérieure absolue du plafond servi (octets/s) —
    /// sécurité, indépendante du contrôleur.
    pub max_bps: u64,
    /// Cadence du tick contrôleur et de la rafale de pings (s).
    pub sample_secs: u64,
    /// Pairs vérifiés pingés par tick (minimum des RTT retenu).
    pub probe_peers: usize,
    /// Attente des pongs avant la décision du tick (ms).
    pub probe_wait_ms: u64,
    /// Plancher de plausibilité d'un échantillon RTT (ms) : un pong
    /// plus rapide a court-circuité la file WAN (auto-ping hairpin,
    /// pair résiduel sur lien local) et empoisonnerait la baseline
    /// — ignoré. `0` = filtre inactif (tests sur réseau local).
    pub probe_min_rtt_ms: u64,
    /// Fenêtre glissante de la baseline RTT (s) — le retard de file
    /// est mesuré par rapport au minimum observé dans cette fenêtre.
    pub base_window_secs: u64,
    /// Retard de file toléré (ms) : le plafond augmente tant que le
    /// dépassement est nul, recule sinon. 25 ms préserve la
    /// réactivité des applications interactives (jeu, visio) tout
    /// en restant au-dessus du jitter naturel du RTT minimum.
    pub target_delay_ms: u64,
    /// Croissance additive : `cap += max(cap / increase_div,
    /// increase_min_bps)`.
    pub increase_div: u64,
    /// Croissance additive minimale par tick (octets/s).
    pub increase_min_bps: u64,
    /// Multiplicateur de réduction en congestion (pourcent gardé :
    /// 75 = cap × 0.75).
    pub decrease_pct: u64,
}

impl Default for BandwidthConfig {
    fn default() -> Self {
        Self {
            floor_bps: 64 * 1024,
            fallback_bps: 512 * 1024,
            max_bps: 32 * 1024 * 1024,
            sample_secs: 5,
            probe_peers: 8,
            probe_wait_ms: 1200,
            probe_min_rtt_ms: 1,
            base_window_secs: 600,
            target_delay_ms: 25,
            increase_div: 8,
            increase_min_bps: 32 * 1024,
            decrease_pct: 75,
        }
    }
}

/// Section `tunnel_community`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TunnelCommunityConfig {
    /// Crée la TunnelCommunity et les lanes anonymes.
    pub enabled: bool,
    /// Circuits minimum maintenus (injecté dans `TunnelSettings`).
    pub min_circuits: u32,
    /// Circuits maximum.
    pub max_circuits: u32,
    /// `max_joined_circuits` Python (défaut 100) : plafond de jambes
    /// de relais + sorties servies simultanément — au-delà les
    /// `create` entrants sont refusés (`should_join_circuit`).
    /// Borne la charge de relais que le réseau impose au nœud ;
    /// pris en compte au redémarrage (settings figés à la
    /// construction de la communauté).
    pub max_joined_circuits: u32,
    /// Extension Rust (sans équivalent pyipv8) : débit max du trafic
    /// servi aux autres pairs — cellules relayées + datagrammes de
    /// sortie — en octets/s. **`-1` = automatique** (défaut : AIMD
    /// sur le retard de file mesuré par ping des pairs vérifiés —
    /// voir `BandwidthConfig`), `0` = illimité
    /// (comportement pyipv8), `>0` = plafond fixe. Le débit servi est
    /// symétrique (1 datagramme relayé = 1 in + 1 out) : le plafond est
    /// basé sur l'upload, toujours le facteur limitant — il borne
    /// automatiquement le download consommé à la même valeur. Appliqué
    /// à chaud via `POST /api/settings` (seau à jetons sur la pompe
    /// d'émission ; l'excédent est perdu en sémantique UDP, lissé par
    /// uTP aux extrémités).
    pub max_relayed_rate: i64,
    /// Paramètres du contrôleur de congestion utilisé quand
    /// `max_relayed_rate = -1` (extension Rust — pyipv8 n'a pas de
    /// plafond de débit servi).
    pub bandwidth: BandwidthConfig,
    /// Accepte d'être noeud de sortie (`exitnode_enabled` Tribler).
    pub exitnode_enabled: bool,
    /// Point d'introduction impose `"ip:port"` — extension Rust
    /// (`required_ip` de `create_introduction_point` pyipv8) : tous les
    /// circuits `IP_SEEDER` terminent sur ce pair. Vide = selection
    /// automatique. Reserve aux bancs controles et au diagnostic.
    pub intro_point_peer: String,
    /// Sortie imposee des circuits `DATA` `"ip:port"` — extension Rust
    /// (`required_exit` de `create_circuit` pyipv8) : tous les circuits
    /// `DATA` terminent sur ce pair, et aucun circuit n'est cree tant
    /// qu'il n'est pas verifie. Vide = selection automatique `EXIT_BT`.
    /// Reserve aux bancs controles (point d'introduction joignable
    /// uniquement en loopback, NAT sans hairpin, ...).
    pub data_exit_peer: String,
    /// Guard nodes (ADR-0010, mesure experimentale de reduction
    /// d'exposition Sybil — pas une garantie d'anonymat) : premiers
    /// sauts persistants bornant la loterie des reconstructions sous
    /// `DESTROY`. `false` = selection pyipv8 exacte. Validee sur le
    /// terrain (matrice interop + download public) — `true` par
    /// defaut ; reste desactivable a chaud via `POST /api/settings`.
    pub guards_enabled: bool,
    /// Extension Rust : plafond de debit de la socket DHT tunnelisee
    /// de chaque lane anonyme, en datagrammes/s sortants (seau a
    /// jetons, rafale bornee a 1 s — l'excedent est perdu en
    /// semantique UDP). `0` = illimite (non recommande : mesure mesh,
    /// un magnet en stall produisait ~6 400 cellules/s). Defaut 30.
    pub anon_dht_rate_pps: u64,
    /// Extension Rust : posture client-only de la DHT anonyme — les
    /// requetes DHT entrantes non sollicitees (reinjectees par la
    /// socket de sortie) sont ecartees au lieu d'etre servies : la
    /// lane interroge la DHT mais ne la sert pas — coupe la boucle
    /// d'amplification requete/reponse a travers le tunnel.
    /// `true` par defaut.
    pub anon_dht_client_only: bool,
    /// Extension Rust : plafond (s) du backoff exponentiel des
    /// re-lookups `get_peers` sans progres (patch librqbit-dht
    /// vendored, jitter ±25 %). `0` = intervalle fixe de 60 s
    /// (comportement librqbit brut). Defaut 900 : un magnet sans
    /// swarm retombe a ~1 vague de requetes par quart d'heure.
    pub anon_dht_backoff_cap_secs: u64,
    /// Extension Rust : TTL (s) des sources WAN "contactees" par une
    /// socket de sortie — la reencapsulation entrante n'accepte que
    /// les datagrammes non-IPv8 provenant d'une destination passee en
    /// sortie dans cette fenetre (semantique conntrack ; le trafic
    /// IPv8 e2e des services caches n'est pas soumis a ce filtre).
    /// `0` desactive le filtre. Defaut 300 : sans lui, le bruit UDP
    /// adresse au port de sortie etait reinjecte puis servi
    /// (amplification mesuree ~6 400 cellules/s en mesh).
    pub exit_inbound_source_ttl_secs: u64,
    /// Extension Rust : borne de la table des sources contactees par
    /// socket de sortie (`exit_inbound_source_ttl_secs`). Defaut 2048.
    pub exit_inbound_max_sources: u64,
    /// Extension Rust (ADR-0011, messagerie anonyme e2e) : demarre le
    /// service messagerie — le demon annonce sa presence sur
    /// `messaging_hash(pk)` et accepte les liaisons de contacts.
    /// `true` par defaut (bancs `MS-*` et parcours inter-daemon
    /// valides) ; sans effet si `enabled = false`.
    pub messaging_enabled: bool,
    /// Extension Rust (ADR-0011) : sauts des circuits messagerie
    /// (`hops` de `join_swarm`, meme echelle que `anon_hops`).
    /// Defaut 1 — augmente l'anonymat de la liaison au prix de
    /// latence (comme les lanes anonymes).
    pub messaging_hops: u32,
    /// Clés tunnel additionnelles — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for TunnelCommunityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_circuits: 3,
            max_circuits: 8,
            max_joined_circuits: 100,
            max_relayed_rate: -1,
            bandwidth: BandwidthConfig::default(),
            exitnode_enabled: false,
            intro_point_peer: String::new(),
            data_exit_peer: String::new(),
            guards_enabled: true,
            anon_dht_rate_pps: crate::ipv8_stack::DEFAULT_ANON_DHT_RATE_PPS,
            anon_dht_client_only: true,
            anon_dht_backoff_cap_secs: crate::ipv8_stack::DEFAULT_ANON_DHT_BACKOFF_CAP_SECS,
            exit_inbound_source_ttl_secs: crate::ipv8_stack::DEFAULT_EXIT_INBOUND_TTL_SECS,
            exit_inbound_max_sources: crate::ipv8_stack::DEFAULT_EXIT_INBOUND_MAX_SOURCES as u64,
            messaging_enabled: true,
            messaging_hops: crate::ipv8_stack::DEFAULT_MESSAGING_HOPS as u32,
            extra: serde_json::Map::new(),
        }
    }
}

/// Section `{enabled: bool}` générique (`database`, `dht_discovery`,
/// `recommender`, `rendezvous`, `torrent_checker`,
/// `content_discovery_community`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnabledSection {
    /// Composant actif.
    pub enabled: bool,
    /// Clés additionnelles — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl EnabledSection {
    /// Section activée (défaut de la plupart des composants Tribler).
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            extra: serde_json::Map::new(),
        }
    }
}

impl Default for EnabledSection {
    fn default() -> Self {
        Self::enabled()
    }
}

/// Section `rss`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RssConfig {
    /// Watchers RSS actifs.
    pub enabled: bool,
    /// Flux surveillés.
    pub urls: Vec<String>,
}

impl Default for RssConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            urls: Vec::new(),
        }
    }
}

/// Section `versioning`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VersioningConfig {
    /// Vérification de version.
    pub enabled: bool,
    /// Accepte les pré-versions.
    pub allow_pre: bool,
    /// Dépôt GitHub `owner/repo` sondé pour les releases (vide = pas
    /// de sonde GitHub).
    pub github_repo: String,
    /// Sondes additionnelles interrogées avant/après GitHub, dans
    /// l'ordre — équivalent de `release.tribler.org` côté Python ;
    /// `{current}` est substitué par la version courante et chaque
    /// sonde doit répondre un JSON `{"name": "x.y.z"}` (ou un tableau
    /// dont le premier élément porte `name`).
    pub check_urls: Vec<String>,
    /// Timeout d'une sonde de version en secondes
    /// (`ClientTimeout(total=5)` Python).
    pub check_timeout_secs: u64,
    /// Clés additionnelles.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for VersioningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_pre: false,
            github_repo: default_github_repo(),
            check_urls: Vec::new(),
            check_timeout_secs: 5,
            extra: serde_json::Map::new(),
        }
    }
}

/// `owner/repo` du projet, dérivé du champ `repository` du paquet
/// (`https://github.com/<owner>/<repo>`) — vide hors GitHub.
fn default_github_repo() -> String {
    option_env!("CARGO_PKG_REPOSITORY")
        .and_then(|u| u.trim_end_matches('/').strip_prefix("https://github.com/"))
        .unwrap_or_default()
        .to_owned()
}

/// Section `watch_folder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchFolderConfig {
    /// Surveillance active (défaut Python : false).
    pub enabled: bool,
    /// Répertoire surveillé.
    pub directory: String,
    /// Intervalle de scan (s).
    pub check_interval: f64,
}

impl Default for WatchFolderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: String::new(),
            check_interval: 10.0,
        }
    }
}

/// Section `logging` — rétention des fichiers de log (extension
/// propre au portage, absente de `TriblerConfig` Python).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// Nombre de fichiers de run precedent conserves en plus du
    /// `onionbit.log` courant (`onionbit.log.1` … `.N` ; `0` = un seul
    /// fichier ecrase a chaque demarrage).
    pub max_files: usize,
    /// Mode debug asyncio persistant (`PUT /api/ipv8/asyncio/debug`
    /// `enable`) : Python ne le persistait pas, mais l'utilisateur
    /// attend que le choix « info/debug » survive au redemarrage —
    /// restaure par `init_tracing` au lancement.
    pub debug: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            max_files: 5,
            debug: false,
        }
    }
}

/// Arbre complet de `configuration.json` (équivalent `TriblerConfig`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Version du schéma persisté (`CURRENT_CONFIG_VERSION`) —
    /// extension Rust. `#[serde(default)]` au niveau champ : une clé
    /// absente (fichier legacy) donne `0` et non le `Default` du
    /// struct — sinon la migration ne s'appliquerait jamais. Jamais
    /// patchable via `POST /api/settings` (`merge` la force).
    #[serde(default)]
    pub config_version: u32,
    /// Section `api`.
    pub api: ApiConfig,
    /// Section `ipv8`.
    pub ipv8: Ipv8FileConfig,
    /// Section `libtorrent`.
    pub libtorrent: LibtorrentConfig,
    /// Section `tunnel_community`.
    pub tunnel_community: TunnelCommunityConfig,
    /// Section `database`.
    pub database: EnabledSection,
    /// Section `dht_discovery`.
    pub dht_discovery: EnabledSection,
    /// Section `content_discovery_community`.
    pub content_discovery_community: EnabledSection,
    /// Section `recommender`.
    pub recommender: EnabledSection,
    /// Section `rendezvous`.
    pub rendezvous: EnabledSection,
    /// Section `rss`.
    pub rss: RssConfig,
    /// Section `torrent_checker`.
    pub torrent_checker: EnabledSection,
    /// Section `versioning`.
    pub versioning: VersioningConfig,
    /// Section `watch_folder`.
    pub watch_folder: WatchFolderConfig,
    /// Section `tray` — icône de zone de notification du daemon
    /// (extension propre au portage, absente de `TriblerConfig` Python ;
    /// `enabled=false` = `--no-tray`).
    pub tray: EnabledSection,
    /// Section `logging` — rétention des fichiers de log.
    pub logging: LoggingConfig,
    /// Mode sans GUI.
    pub headless: bool,
    /// Démarrage minimisé (UI — inerte en daemon).
    pub start_minimized: bool,
    /// Stats IPv8 détaillées (`statistics` Python — cle legacy :
    /// Tribler 8.x ne la lit plus non plus, elle n'est conservee que
    /// par `upgrade_script` ; les stats par community sont activees
    /// inconditionnellement au demarrage, comme Python).
    pub statistics: bool,
    /// Base en mémoire (`:memory:`).
    pub memory_db: bool,
    /// Couleur de l'icône tray (UI — inerte en daemon).
    pub tray_icon_color: String,
    /// Réglages libres de l'UI (sparse).
    pub ui: Value,
    /// Sections/clés inconnues — préservées comme le dict Python.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            api: ApiConfig::default(),
            ipv8: Ipv8FileConfig::default(),
            libtorrent: LibtorrentConfig::default(),
            tunnel_community: TunnelCommunityConfig::default(),
            database: EnabledSection::enabled(),
            dht_discovery: EnabledSection::enabled(),
            content_discovery_community: EnabledSection::enabled(),
            recommender: EnabledSection::enabled(),
            rendezvous: EnabledSection::enabled(),
            rss: RssConfig::default(),
            torrent_checker: EnabledSection::enabled(),
            versioning: VersioningConfig::default(),
            watch_folder: WatchFolderConfig::default(),
            tray: EnabledSection::enabled(),
            logging: LoggingConfig::default(),
            headless: false,
            start_minimized: false,
            statistics: false,
            memory_db: false,
            tray_icon_color: String::new(),
            ui: Value::Object(serde_json::Map::new()),
            extra: serde_json::Map::new(),
        }
    }
}

/// Migrations de défauts gelés par l'ancienne écriture complète du
/// fichier : une valeur **encore égale à l'ancien défaut** est
/// réalignée sur le défaut actuel ; toute autre valeur est un choix
/// explicite, préservé. `(chemin, ancien défaut gelé, défaut actuel)`.
/// Les tables sont indexées par version : une migration ne rejoue
/// jamais sur un fichier qui l'a déjà vue (un choix posé après coup
/// vers l'ancienne valeur est explicite).
fn default_migrations_v0_v1() -> Vec<(&'static [&'static str], Value, Value)> {
    vec![
        // `guards_enabled` est né avec le défaut `false` (feature
        // expérimentale derrière flag) puis validé sur le terrain →
        // `true` : les fichiers écrits entre-temps gardaient `false`
        // indéfiniment.
        (
            &["tunnel_community", "guards_enabled"],
            serde_json::json!(false),
            serde_json::json!(true),
        ),
        // `enabled` (défaut `true`) et `exitnode_enabled` (défaut
        // `false`) n'ont jamais glissé — aucune entrée : une valeur
        // non défaut y est forcément un choix explicite.
    ]
}

fn default_migrations_v1_v2() -> Vec<(&'static [&'static str], Value, Value)> {
    vec![
        // `target_delay_ms` né à 50 ms (seuil LEDBAT mass-transit) →
        // 25 ms : la réactivité des applications interactives prime
        // sur le débit servi aux pairs.
        (
            &["tunnel_community", "bandwidth", "target_delay_ms"],
            serde_json::json!(50),
            serde_json::json!(25),
        ),
    ]
}

/// Descend `path` dans `root` et renvoie le slot terminal mutable
/// (dernier segment), `None` si un maillon est absent ou non objet.
fn path_slot_mut<'a>(
    root: &'a mut serde_json::Map<String, Value>,
    path: &[&str],
) -> Option<&'a mut Value> {
    let (last, parents) = path.split_last()?;
    let mut node = root;
    for key in parents {
        node = node.get_mut(*key)?.as_object_mut()?;
    }
    node.get_mut(*last)
}

/// Applique une table de migrations à l'arbre brut.
fn apply_migrations(
    root: &mut serde_json::Map<String, Value>,
    migrations: &[(&[&str], Value, Value)],
) {
    for (path, legacy, current) in migrations {
        if let Some(slot) = path_slot_mut(root, path) {
            if *slot == *legacy {
                tracing::info!(
                    cle = %path.join("/"),
                    "configuration.json : ancien défaut gelé -> défaut actuel"
                );
                *slot = current.clone();
            }
        }
    }
}

/// Migrations de l'arbre brut, avant remplissage serde : seules les
/// clés explicitement écrites sont candidates (une clé absente prend
/// déjà le défaut actuel). Estampille `config_version`. Retourne
/// `true` si l'arbre a changé — le fichier est alors réécrit.
fn migrate_legacy_tree(tree: &mut Value) -> bool {
    let Some(root) = tree.as_object_mut() else {
        return false;
    };
    let version = root
        .get("config_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    if version >= CURRENT_CONFIG_VERSION {
        return false;
    }
    if version < 1 {
        apply_migrations(root, &default_migrations_v0_v1());
    }
    if version < 2 {
        apply_migrations(root, &default_migrations_v1_v2());
    }
    root.insert(
        "config_version".to_string(),
        Value::from(CURRENT_CONFIG_VERSION),
    );
    true
}

impl DaemonConfig {
    /// Charge `path` ; fichier absent ou corrompu → défauts
    /// (`Failed to load stored configuration. Falling back to
    /// defaults!` Python). La clé API est générée si absente.
    pub fn load(path: &Path) -> Self {
        Self::load_report(path).0
    }

    /// `load` + remontée de l'erreur de parse (`report_config_error`
    /// Python — le daemon peut la notifier sur le bus d'evenements).
    /// Retourne `(config, Some(erreur))` si le fichier existait mais
    /// n'etait pas un JSON valide.
    pub fn load_report(path: &Path) -> (Self, Option<String>) {
        let (mut cfg, error, migrated) = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Value>(&text)
                .map_err(|e| e.to_string())
                .and_then(|mut tree| {
                    let migrated = migrate_legacy_tree(&mut tree);
                    serde_json::from_value::<Self>(tree)
                        .map(|cfg| (cfg, migrated))
                        .map_err(|e| e.to_string())
                }) {
                Ok((cfg, migrated)) => (cfg, None, migrated),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %path.display(),
                        "configuration.json corrompu, repli sur les valeurs par défaut"
                    );
                    (Self::default(), Some(e), false)
                }
            },
            Err(_) => (Self::default(), None, false),
        };
        cfg.ensure_api_key();
        // Migration : `http_port=0` (ancien defaut — port ephemere a
        // chaque demarrage) devient le port fixe 8085 que l'UI tente
        // en premier ; un port configure explicitement est preserve.
        if cfg.api.http_port == 0 {
            cfg.api.http_port = 8085;
        }
        if migrated {
            // Fichier estampillé + valeurs migrées persistées : la
            // migration ne rejoue pas — un choix posé après coup vers
            // l'ancienne valeur est un choix explicite, préservé.
            if let Err(e) = cfg.write(path) {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "réécriture de configuration.json migré impossible"
                );
            } else {
                tracing::info!(
                    path = %path.display(),
                    "configuration.json migré (v{CURRENT_CONFIG_VERSION})"
                );
            }
        }
        (cfg, error)
    }

    /// Réécrit le fichier de configuration (`config.write()` Python)
    /// en **sparse** : seules les valeurs différentes des défauts sont
    /// persistées (+ `config_version`, qui pilote les migrations). Un
    /// défaut corrigé dans une version ultérieure se propage ainsi aux
    /// fichiers existants ; une clé explicitement écrite mais égale au
    /// défaut courant est omise (re-réglable via `POST /api/settings`).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&self.sparse_value())?)
    }

    /// Arbre des seuls écarts par rapport à `Self::default()` —
    /// `config_version` est toujours écrit (migration au chargement)
    /// et `api.key` y figure naturellement (défaut `""` ≠ clé générée).
    fn sparse_value(&self) -> Value {
        let current = serde_json::to_value(self).unwrap_or_default();
        let defaults = serde_json::to_value(Self::default()).unwrap_or_default();
        let mut sparse =
            deep_diff(&current, &defaults).unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if let Some(obj) = sparse.as_object_mut() {
            obj.insert(
                "config_version".to_string(),
                Value::from(CURRENT_CONFIG_VERSION),
            );
        }
        sparse
    }

    /// Génère `api.key` (32 hex) si vide — comme l'installeur Python.
    pub fn ensure_api_key(&mut self) {
        if self.api.key.is_empty() {
            let mut bytes = [0u8; 16];
            rand::RngExt::fill(&mut rand::rng(), &mut bytes);
            self.api.key = hex::encode(bytes);
        }
    }

    /// Clé API attendue (`None` = authentification inactive — le
    /// middleware Python autorise tout quand la clé est vide).
    pub fn api_key(&self) -> Option<&str> {
        (!self.api.key.is_empty()).then_some(self.api.key.as_str())
    }

    /// Merge récursif d'un patch JSON (`POST /api/settings`) : seuls
    /// les champs présents dans `patch` sont écrasés, l'arbre restant
    /// est conservé. Erreur si le résultat ne resérialise pas
    /// (types incohérents → 400 côté handler).
    pub fn merge(&mut self, patch: &Value) -> std::result::Result<(), serde_json::Error> {
        let mut merged = serde_json::to_value(&*self)?;
        deep_merge(&mut merged, patch);
        // `config_version` est gérée par les migrations au chargement —
        // un patch client ne peut pas redéclencher la migration.
        if let Some(obj) = merged.as_object_mut() {
            obj.insert(
                "config_version".to_string(),
                Value::from(CURRENT_CONFIG_VERSION),
            );
        }
        let next: Self = serde_json::from_value(merged)?;
        *self = next;
        Ok(())
    }

    /// Traduit l'arbre persisté en configuration de session coeur.
    /// `state_dir` vient du lanceur (le fichier vit dedans ; Python
    /// stocke `state_dir` dans la config, nous laissons le CLI décider).
    pub fn to_core_config(&self, state_dir: &Path) -> crate::CoreConfig {
        use onionbit_network_policy::exit_policy as flags;

        let dd = &self.libtorrent.download_defaults;
        let downloads_dir = if dd.saveas.is_empty() {
            state_dir.join("downloads")
        } else {
            PathBuf::from(&dd.saveas)
        };

        // `listen_interface` Python est une IP (`"0.0.0.0"`) ;
        // `listen_interface_v6` non vide active l'écoute v6 (socket
        // dual-stack — librqbit n'ouvre qu'un seul socket d'écoute,
        // la v6 couvre alors aussi le v4).
        let listen_ip = self
            .libtorrent
            .listen_interface
            .parse::<std::net::IpAddr>()
            .unwrap_or_else(|_| {
                tracing::warn!(
                    listen_interface = %self.libtorrent.listen_interface,
                    "libtorrent/listen_interface invalide, repli sur 0.0.0.0"
                );
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
            });
        let listen_addr_v6 = if self.libtorrent.listen_interface_v6.is_empty() {
            None
        } else {
            self.libtorrent
                .listen_interface_v6
                .parse::<std::net::IpAddr>()
                .ok()
                .map(|ip| std::net::SocketAddr::new(ip, self.libtorrent.port_v6))
        };
        // Port d'écoute BitTorrent en clair : Tribler sonde la plage 6881..=6891
        // (`listen_on(port, port + 10)` avec repli 6881 si port == 0).
        let effective_bt_port = if self.libtorrent.port == 0 {
            (6881..=6891)
                .find(|&p| {
                    std::net::TcpListener::bind((listen_ip, p)).is_ok()
                        && std::net::UdpSocket::bind((listen_ip, p)).is_ok()
                })
                .unwrap_or(0)
        } else {
            self.libtorrent.port
        };
        let mut engine = onionbit_bittorrent::EngineConfig {
            output_dir: downloads_dir.clone(),
            enable_dht: self.libtorrent.dht,
            disable_lsd: !self.libtorrent.lsd,
            listen_port: Some(effective_bt_port),
            listen_ip,
            listen_addr_v6,
            enable_utp: self.libtorrent.utp,
            // Buffers uTP par connexion (extensions Rust, 0 = defaut
            // librqbit-utp) — propagés aussi aux sockets uTP des
            // lanes anonymes via `Ipv8Stack::anon_engine`.
            utp_rx_buf_size: (self.libtorrent.utp_rx_buf_size > 0)
                .then_some(self.libtorrent.utp_rx_buf_size),
            utp_tx_buf_max: (self.libtorrent.utp_tx_buf_max > 0)
                .then_some(self.libtorrent.utp_tx_buf_max),
            enable_upnp: self.libtorrent.upnp,
            enable_natpmp: self.libtorrent.natpmp,
            // `max_connections_download` Python : -1 = illimité.
            peer_limit: (self.libtorrent.max_connections_download >= 0)
                .then_some(self.libtorrent.max_connections_download as usize),
            // `active_checking` Python -> `concurrent_init_limit`
            // rqbit (init/vérification concurrentes) : <= 0 = pas de
            // borne explicite.
            concurrent_init_limit: (self.libtorrent.active_checking > 0)
                .then_some(self.libtorrent.active_checking as usize),
            fastresume_sampled_check: self.libtorrent.fastresume_check,
            // `max_*_rate` Python : 0 = illimite.
            max_upload_bps: (self.libtorrent.max_upload_rate > 0)
                .then_some(self.libtorrent.max_upload_rate),
            max_download_bps: (self.libtorrent.max_download_rate > 0)
                .then_some(self.libtorrent.max_download_rate),
            allow_mmap: self.libtorrent.allow_mmap,
            clear_orphaned_parts: self.libtorrent.clear_orphaned_parts,
            dht_readiness_timeout_secs: self.libtorrent.dht_readiness_timeout,
            // Fastresume rqbit : `session.json` + `<ih>.bitv` par
            // moteur (equivalent des checkpoints libtorrent). Dossier
            // dedie par moteur — les lanes anonymes ont le leur
            // (`anon<N>`, cf. `Ipv8Stack::anon_engine`).
            persistence_dir: Some(state_dir.join("rqbit").join("main")),
            ..Default::default()
        };
        // proxy_type 2/3 = SOCKS5 (enum libtorrent), 4/5 = HTTP non
        // supporté par librqbit. `proxy_auth` Python : `user:pass` en
        // userinfo de l'URL. Le proxy guard (loopback uniquement)
        // reste appliqué au démarrage du moteur.
        if !self.libtorrent.proxy_server.is_empty() {
            match self.libtorrent.proxy_type {
                2 | 3 => {
                    let auth = if self.libtorrent.proxy_auth.is_empty() {
                        String::new()
                    } else {
                        format!("{}@", self.libtorrent.proxy_auth)
                    };
                    engine.socks5_proxy =
                        Some(format!("socks5://{auth}{}", self.libtorrent.proxy_server));
                }
                4 | 5 => {
                    tracing::warn!(
                        proxy_type = self.libtorrent.proxy_type,
                        "libtorrent/proxy_type HTTP non supporte par librqbit (proxy ignore)"
                    );
                }
                _ => {}
            }
        }
        if !self.libtorrent.announce_to_all_tiers || !self.libtorrent.announce_to_all_trackers {
            tracing::debug!(
                "libtorrent/announce_to_all_* : librqbit annonce deja a tous les trackers"
            );
        }
        // Quotas par fonction propres a libtorrent, sans reglage
        // equivalent chez librqbit : tracés une fois plutot que
        // silencieusement ignores.
        if self.libtorrent.active_dht_limit >= 0
            || self.libtorrent.active_tracker_limit >= 0
            || self.libtorrent.active_lsd_limit >= 0
        {
            tracing::debug!(
                "libtorrent/active_{{dht,tracker,lsd}}_limit : pas d'equivalent librqbit"
            );
        }
        if self.libtorrent.max_concurrent_http_announces != 50 {
            tracing::debug!("libtorrent/max_concurrent_http_announces : non expose par librqbit");
        }
        // `recommender`/`rendezvous` : cles mortes dans Tribler 8.x
        // meme (declarees dans `tribler_config.py`, jamais relues —
        // vestiges des composants 7.x). Leur fonction historique est
        // absorbee : `recommender` -> tache periodique "check local
        // torrents" du torrent_checker (`TorrentChecker::check_oldest`),
        // `rendezvous` -> points de rendez-vous des hidden services
        // dans `TunnelCommunity` (`hidden_services.rs`). Un `enabled`
        // a `false` n'est donc pas honore — parite stricte avec le
        // comportement Python 8.x, qui ignore aussi la cle.
        for (name, enabled) in [
            ("recommender", self.recommender.enabled),
            ("rendezvous", self.rendezvous.enabled),
        ] {
            if enabled {
                tracing::debug!("composant {name} : cle morte dans Tribler 8.x, fonction absorbee");
            }
        }
        // `versioning/allow_pre` filtre les pre-versions rapportees
        // par la verification distante — inexistante pour l'instant
        // (`/api/versioning/versions/check` repond toujours sans mise
        // a jour ; `enabled` est gate dans les handlers REST).
        if self.versioning.allow_pre {
            tracing::debug!(
                "versioning/allow_pre : sans effet tant qu'aucune verification distante n'existe"
            );
        }

        let listen = self
            .ipv8
            .interfaces
            .iter()
            .find(|i| i.interface == "UDPIPv4")
            .or_else(|| self.ipv8.interfaces.first());
        // `ipv8/interfaces[UDPIPv6]` pyipv8 : socket UDP secondaire
        // du meme endpoint ("" ou absent = IPv4 seul).
        let listen_v6 = self
            .ipv8
            .interfaces
            .iter()
            .find(|i| i.interface == "UDPIPv6" && !i.ip.is_empty())
            .map(|i| format!("{}:{}", i.ip, i.port));
        // `TunnelSettings.peer_flags` pyipv8 : `{RELAY, SPEED_TEST}` ;
        // `exitnode_enabled` ajoute les sorties BT/IPv8/HTTP
        // (`TriblerTunnelCommunity.__init__`).
        let mut peer_flags = flags::PEER_FLAG_RELAY | flags::PEER_FLAG_SPEED_TEST;
        if self.tunnel_community.exitnode_enabled {
            peer_flags |=
                flags::PEER_FLAG_EXIT_BT | flags::PEER_FLAG_EXIT_IPV8 | flags::PEER_FLAG_EXIT_HTTP;
        }
        let ipv8 = crate::ipv8_stack::Ipv8Config {
            enabled: self.ipv8.enabled,
            listen_addr: listen.map_or_else(
                || format!("0.0.0.0:{}", crate::ipv8_stack::DEFAULT_IPV8_PORT),
                |i| format!("{}:{}", i.ip, i.port),
            ),
            bootstrap_peers: if self.ipv8.bootstrap.override_peers.is_empty() {
                crate::ipv8_stack::DEFAULT_BOOTSTRAP_PEERS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            } else {
                self.ipv8.bootstrap.override_peers.clone()
            },
            enable_anonymity: self.tunnel_community.enabled,
            peer_flags,
            onionbit_tunnel_community: true,
            enable_dht: self.dht_discovery.enabled,
            intro_point_peer: if self.tunnel_community.intro_point_peer.is_empty() {
                None
            } else {
                match self.tunnel_community.intro_point_peer.parse::<SocketAddr>() {
                    Ok(a) => Some(onionbit_ipv8::UdpAddress::from(a)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            value = %self.tunnel_community.intro_point_peer,
                            "tunnel_community/intro_point_peer invalide, ignore"
                        );
                        None
                    }
                }
            },
            data_exit_peer: if self.tunnel_community.data_exit_peer.is_empty() {
                None
            } else {
                match self.tunnel_community.data_exit_peer.parse::<SocketAddr>() {
                    Ok(a) => Some(onionbit_ipv8::UdpAddress::from(a)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            value = %self.tunnel_community.data_exit_peer,
                            "tunnel_community/data_exit_peer invalide, ignore"
                        );
                        None
                    }
                }
            },
            estimated_wan: if self.ipv8.estimated_wan.is_empty() {
                None
            } else {
                match self.ipv8.estimated_wan.parse::<SocketAddr>() {
                    Ok(a) => Some(onionbit_ipv8::UdpAddress::from(a)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            value = %self.ipv8.estimated_wan,
                            "ipv8/estimated_wan invalide, ignore"
                        );
                        None
                    }
                }
            },
            walker_interval: self.ipv8.walker_interval,
            min_circuits: self.tunnel_community.min_circuits,
            max_circuits: self.tunnel_community.max_circuits,
            max_joined_circuits: self.tunnel_community.max_joined_circuits as usize,
            max_relayed_bps: self.tunnel_community.max_relayed_rate,
            bandwidth: self.tunnel_community.bandwidth.clone(),
            guards_enabled: self.tunnel_community.guards_enabled,
            socks_listen_ports: self.libtorrent.socks_listen_ports.clone(),
            enable_content_discovery: self.content_discovery_community.enabled,
            listen_addr_v6: listen_v6,
            peer_cache_max: crate::ipv8_stack::DEFAULT_PEER_CACHE_MAX,
            peer_cache_max_age_secs: crate::ipv8_stack::DEFAULT_PEER_CACHE_MAX_AGE_SECS,
            peer_persist_interval_secs: crate::ipv8_stack::DEFAULT_PEER_PERSIST_INTERVAL_SECS,
            content_healths_cache_secs: crate::ipv8_stack::DEFAULT_CONTENT_HEALTHS_CACHE_SECS,
            anon_dht_rate_pps: self.tunnel_community.anon_dht_rate_pps,
            anon_dht_client_only: self.tunnel_community.anon_dht_client_only,
            anon_dht_backoff_cap_secs: self.tunnel_community.anon_dht_backoff_cap_secs,
            exit_inbound_source_ttl_secs: self.tunnel_community.exit_inbound_source_ttl_secs,
            exit_inbound_max_sources: self.tunnel_community.exit_inbound_max_sources as usize,
            // Parametres internes du debit `rate_*` de
            // `/api/statistics/ipv8` — pas exposes dans le fichier de
            // config (comme `peer_cache_max`).
            stats_rate_sample_ms: crate::ipv8_stack::DEFAULT_STATS_RATE_SAMPLE_MS,
            stats_rate_window_secs: crate::ipv8_stack::DEFAULT_STATS_RATE_WINDOW_SECS,
            enable_messaging: self.tunnel_community.messaging_enabled
                && self.tunnel_community.enabled,
            messaging_hops: self.tunnel_community.messaging_hops as usize,
        };

        crate::CoreConfig {
            state_dir: state_dir.to_path_buf(),
            downloads_dir,
            // `database.enabled` Python : sans composant DB, Tribler
            // degrade a un fonctionnement sans persistance — base en
            // memoire (meme repli que `--memory-db`).
            db_filename: if self.memory_db || !self.database.enabled {
                ":memory:".into()
            } else {
                "onionbit.db".into()
            },
            ip_policy: onionbit_network_policy::IpPolicy::strict(),
            watch_folder_dir: (self.watch_folder.enabled
                && !self.watch_folder.directory.is_empty())
            .then(|| PathBuf::from(&self.watch_folder.directory)),
            watch_folder_interval_ms: (self.watch_folder.check_interval.max(0.1) * 1000.0) as u64,
            rss_urls: if self.rss.enabled {
                self.rss.urls.clone()
            } else {
                Vec::new()
            },
            enable_torrent_checker: self.torrent_checker.enabled,
            ipv8,
            engine,
            queue: crate::config::QueueLimits {
                active_downloads: self.libtorrent.active_downloads,
                active_seeds: self.libtorrent.active_seeds,
                active_limit: self.libtorrent.active_limit,
            },
            check_after_complete: self.libtorrent.check_after_complete,
            download_defaults: crate::config::DownloadDefaults {
                anonymity_enabled: dd.anonymity_enabled,
                number_hops: dd.number_hops,
                safeseeding_enabled: dd.safeseeding_enabled,
                seeding_mode: dd.seeding_mode.clone(),
                seeding_ratio: dd.seeding_ratio,
                seeding_time: dd.seeding_time,
                auto_managed: dd.auto_managed,
                completed_dir: dd.completed_dir.clone(),
                trackers_file: dd.trackers_file.clone(),
                trackers_file_sync_url: dd.trackers_file_sync_url.clone(),
                torrent_folder: dd.torrent_folder.clone(),
                channel_download: dd.channel_download,
                add_download_to_channel: dd.add_download_to_channel,
            },
            ..Default::default()
        }
    }

    /// Superpose les valeurs effectivement en cours (`CoreConfig`
    /// effective de la session, overrides à chaud compris) pour que
    /// `GET /api/settings` reflète l'état réel et pas seulement le
    /// fichier.
    pub fn apply_runtime_view(&mut self, core: &crate::CoreConfig) {
        self.libtorrent.download_defaults.saveas = core.engine.output_dir.display().to_string();
        self.libtorrent.dht = core.engine.enable_dht;
        self.libtorrent.lsd = !core.engine.disable_lsd;
        if self.libtorrent.port != 0 {
            self.libtorrent.port = core.engine.listen_port.unwrap_or(0);
        }
        self.libtorrent.listen_interface = core.engine.listen_ip.to_string();
        self.libtorrent.listen_interface_v6 = core
            .engine
            .listen_addr_v6
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
        self.libtorrent.port_v6 = core.engine.listen_addr_v6.map(|a| a.port()).unwrap_or(0);
        self.libtorrent.utp = core.engine.enable_utp;
        self.libtorrent.upnp = core.engine.enable_upnp;
        self.libtorrent.natpmp = core.engine.enable_natpmp;
        self.libtorrent.max_connections_download =
            core.engine.peer_limit.map(|v| v as i64).unwrap_or(-1);
        self.libtorrent.active_checking = core
            .engine
            .concurrent_init_limit
            .map(|v| v as i64)
            .unwrap_or(-1);
        self.libtorrent.proxy_type = if core.engine.socks5_proxy.is_some() {
            2
        } else {
            0
        };
        self.libtorrent.proxy_server = core.engine.socks5_proxy.clone().unwrap_or_default();
        self.libtorrent.max_upload_rate = core.engine.max_upload_bps.unwrap_or(0);
        self.libtorrent.max_download_rate = core.engine.max_download_bps.unwrap_or(0);
        self.rss.urls = core.rss_urls.clone();
        self.rss.enabled = !core.rss_urls.is_empty();
        self.watch_folder.enabled = core.watch_folder_dir.is_some();
        self.watch_folder.directory = core
            .watch_folder_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        self.torrent_checker.enabled = core.enable_torrent_checker;
        self.ipv8.enabled = core.ipv8.enabled;
        self.tunnel_community.enabled = core.ipv8.enable_anonymity;
        self.tunnel_community.min_circuits = core.ipv8.min_circuits;
        self.tunnel_community.max_circuits = core.ipv8.max_circuits;
        self.tunnel_community.max_joined_circuits = core.ipv8.max_joined_circuits as u32;
        self.tunnel_community.max_relayed_rate = core.ipv8.max_relayed_bps;
        self.libtorrent.socks_listen_ports = core.ipv8.socks_listen_ports.clone();
        self.dht_discovery.enabled = core.ipv8.enable_dht;
        self.content_discovery_community.enabled = core.ipv8.enable_content_discovery;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_network_policy::exit_policy as flags;
    use std::path::Path;

    /// `peer_flags` de `TriblerTunnelCommunity` (`community.py` Python) :
    /// `{RELAY, SPEED_TEST}` par defaut, plus les sorties
    /// `EXIT_BT`/`EXIT_IPV8`/`EXIT_HTTP` quand `exitnode_enabled`.
    #[test]
    fn peer_flags_refletent_exitnode_enabled() {
        let cfg = DaemonConfig::default().to_core_config(Path::new("."));
        assert_eq!(
            cfg.ipv8.peer_flags,
            flags::PEER_FLAG_RELAY | flags::PEER_FLAG_SPEED_TEST
        );

        let mut dcfg = DaemonConfig::default();
        dcfg.tunnel_community.exitnode_enabled = true;
        let cfg = dcfg.to_core_config(Path::new("."));
        assert_eq!(
            cfg.ipv8.peer_flags,
            flags::PEER_FLAG_RELAY
                | flags::PEER_FLAG_SPEED_TEST
                | flags::PEER_FLAG_EXIT_BT
                | flags::PEER_FLAG_EXIT_IPV8
                | flags::PEER_FLAG_EXIT_HTTP
        );
    }

    /// `tunnel_community/max_joined_circuits` se propage dans
    /// `Ipv8Config` (defaut 100 = `should_join_circuit` Python).
    #[test]
    fn max_joined_circuits_se_propage() {
        let cfg = DaemonConfig::default().to_core_config(Path::new("."));
        assert_eq!(cfg.ipv8.max_joined_circuits, 100);

        let mut dcfg = DaemonConfig::default();
        dcfg.tunnel_community.max_joined_circuits = 12;
        let cfg = dcfg.to_core_config(Path::new("."));
        assert_eq!(cfg.ipv8.max_joined_circuits, 12);
    }

    /// `tunnel_community/max_relayed_rate` se propage dans
    /// `Ipv8Config::max_relayed_bps` (extension Rust : `-1` = auto —
    /// l'estimateur regle la fraction d'upload mesuree ; `0` =
    /// illimite ; `>0` = fixe).
    #[test]
    fn max_relayed_rate_se_propage() {
        let cfg = DaemonConfig::default().to_core_config(Path::new("."));
        assert_eq!(cfg.ipv8.max_relayed_bps, -1);

        let mut dcfg = DaemonConfig::default();
        dcfg.tunnel_community.max_relayed_rate = 256 * 1024;
        let cfg = dcfg.to_core_config(Path::new("."));
        assert_eq!(cfg.ipv8.max_relayed_bps, 256 * 1024);
        // Aller-retour : `apply_runtime_view` restitue la valeur.
        let mut back = DaemonConfig::default();
        back.apply_runtime_view(&cfg);
        assert_eq!(back.tunnel_community.max_relayed_rate, 256 * 1024);
    }

    /// Migration v0 → v1 : `guards_enabled` gelé à l'ancien défaut
    /// `false` par l'écriture complète du fichier est réaligné sur le
    /// défaut actuel, et le fichier est réécrit estampillé — la
    /// migration ne rejoue pas ensuite.
    #[test]
    fn migration_v0_realigne_defaut_gele() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILENAME);
        std::fs::write(&path, r#"{"tunnel_community":{"guards_enabled":false}}"#).unwrap();
        let cfg = DaemonConfig::load(&path);
        assert!(cfg.tunnel_community.guards_enabled);
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["config_version"], CURRENT_CONFIG_VERSION);
        // Écriture sparse : la valeur migrée vaut le défaut actuel →
        // omise du fichier (le défaut la résout au prochain chargement).
        assert!(stored.pointer("/tunnel_community/guards_enabled").is_none());
    }

    /// Écriture « sparse » : seules les valeurs différentes des
    /// défauts sont persistées (+ `config_version` toujours et
    /// `api.key`, générée ≠ défaut vide). Un défaut corrigé dans une
    /// version ultérieure se propage alors aux fichiers existants.
    #[test]
    fn write_ne_persiste_que_les_ecarts_aux_defauts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILENAME);
        let mut cfg = DaemonConfig::default();
        cfg.api.key = "clef-test".to_string();
        cfg.tunnel_community.bandwidth.target_delay_ms = 12;
        cfg.write(&path).unwrap();
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["config_version"], CURRENT_CONFIG_VERSION);
        assert_eq!(stored["api"]["key"], "clef-test");
        assert_eq!(
            stored["tunnel_community"]["bandwidth"]["target_delay_ms"],
            12
        );
        // Valeurs au défaut → absentes du fichier.
        assert!(stored
            .pointer("/tunnel_community/bandwidth/floor_bps")
            .is_none());
        assert!(stored.get("libtorrent").is_none());
        // Aller-retour : le fichier sparse recharge une config complète.
        let back = DaemonConfig::load(&path);
        assert_eq!(back.tunnel_community.bandwidth.target_delay_ms, 12);
        assert_eq!(back.tunnel_community.bandwidth.floor_bps, 64 * 1024);
        assert_eq!(back.api.key, "clef-test");
    }

    /// Migration v1 → v2 : `target_delay_ms` gelé à l'ancien défaut
    /// `50` (fichier complet hérité de l'écriture dense) est réaligné
    /// sur `25` — un `90` explicite est préservé.
    #[test]
    fn migration_v2_realigne_target_delay_gele() {
        let dir = tempfile::tempdir().unwrap();
        let frozen = dir.path().join("frozen.json");
        std::fs::write(
            &frozen,
            r#"{"config_version":1,"tunnel_community":{"bandwidth":{"target_delay_ms":50}}}"#,
        )
        .unwrap();
        let cfg = DaemonConfig::load(&frozen);
        assert_eq!(cfg.tunnel_community.bandwidth.target_delay_ms, 25);

        let explicit = dir.path().join("explicit.json");
        std::fs::write(
            &explicit,
            r#"{"config_version":1,"tunnel_community":{"bandwidth":{"target_delay_ms":90}}}"#,
        )
        .unwrap();
        let cfg = DaemonConfig::load(&explicit);
        assert_eq!(cfg.tunnel_community.bandwidth.target_delay_ms, 90);
    }

    /// Un `false` posé APRÈS migration (fichier estampillé) est un
    /// choix explicite — préservé au rechargement.
    #[test]
    fn choix_post_migration_preserve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILENAME);
        std::fs::write(
            &path,
            r#"{"config_version":1,"tunnel_community":{"guards_enabled":false}}"#,
        )
        .unwrap();
        let cfg = DaemonConfig::load(&path);
        assert!(!cfg.tunnel_community.guards_enabled);
    }

    /// Choix explicites legacy préservés : `enabled` et
    /// `exitnode_enabled` n'ont jamais changé de défaut — une valeur
    /// non défaut y est toujours un choix, même dans un fichier v0.
    #[test]
    fn choix_explicites_tunnels_preserves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILENAME);
        std::fs::write(
            &path,
            r#"{"tunnel_community":{"enabled":false,"exitnode_enabled":true,"guards_enabled":true}}"#,
        )
        .unwrap();
        let cfg = DaemonConfig::load(&path);
        assert!(!cfg.tunnel_community.enabled);
        assert!(cfg.tunnel_community.exitnode_enabled);
        assert!(cfg.tunnel_community.guards_enabled);
    }

    /// `config_version` n'est pas patchable via `POST /api/settings`
    /// (`merge`) — sinon un client pourrait redéclencher la migration.
    #[test]
    fn merge_ne_patch_pas_config_version() {
        let mut cfg = DaemonConfig::default();
        cfg.merge(&serde_json::json!({
            "config_version": 0,
            "tunnel_community": {"guards_enabled": false}
        }))
        .unwrap();
        assert_eq!(cfg.config_version, CURRENT_CONFIG_VERSION);
        assert!(!cfg.tunnel_community.guards_enabled);
    }

    /// Verifie que to_core_config sonde la plage 6881..=6891 quand le port configuré vaut 0.
    #[test]
    fn to_core_config_sonde_plage_bittorrent_standard() {
        let cfg = DaemonConfig::default();
        let core_cfg = cfg.to_core_config(std::path::Path::new("."));
        let port = core_cfg.engine.listen_port.unwrap_or(0);
        // Doit soit appartenir à 6881..=6891, soit 0 si toute la plage locale est saturée
        assert!(
            (6881..=6891).contains(&port) || port == 0,
            "port effectif {port} inattendu"
        );
    }

    /// Verifie qu'un port explicite est respecté tel quel.
    #[test]
    fn to_core_config_respecte_port_explicite() {
        let mut cfg = DaemonConfig::default();
        cfg.libtorrent.port = 12345;
        let core_cfg = cfg.to_core_config(std::path::Path::new("."));
        assert_eq!(core_cfg.engine.listen_port, Some(12345));
    }
}
