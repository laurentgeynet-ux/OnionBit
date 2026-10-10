// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-daemon` — binaire principal du daemon OnionBit.
//!
//! Point d'entree du processus : charge la configuration persistee
//! (`state_dir/configuration.json`, equivalent `TriblerConfig`
//! Python — les flags CLI sont des overrides), initialise le logging
//! (`tracing`), assemble `onionbit-core` (session + moteur BitTorrent +
//! base) puis demarre `onionbit-api` pour exposer le plan de controle
//! local sur `127.0.0.1` derriere la cle API. Aucun client (CLI ou
//! future UI Flutter) ne parle a autre chose qu'a `onionbit-api`.
//!
//! Sous-systeme Windows `windows` : aucune console n'est allouee — le
//! daemon vit dans la zone de notification (icône tray + menu «
//! Quitter »). `--console` rattache/alloue une console pour le debug.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod autostart;
mod console;
mod https;
mod instance;
mod logs;
mod shutdown;
mod tray;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use onionbit_api::{build, AppState};
use onionbit_core::{CoreConfig, CoreSession, DaemonConfig, Notifier, CONFIG_FILENAME};
use shutdown::ShutdownSignal;

/// Repertoire d'etat par defaut hors bundle (relatif au dossier
/// courant — usage dev).
const DEFAULT_STATE_DIR: &str = ".onionbit";

/// Resout le repertoire d'etat effectif.
///
/// `--state-dir` explicite fait foi. Sans override, un exe vivant dans
/// un bundle `dist\` (repertoire web servi ou UI a cote) adopte la
/// convention des lanceurs — `<exe>/state` : un double-clic direct sur
/// `onionbit-daemon.exe` retrouve alors la MEME base/config que le
/// daemon spawnne par l'UI (avant : `.onionbit` relatif au CWD, un
/// etat orphelin voire `state\state` selon le dossier de travail).
fn resolve_state_dir(args: &Args) -> PathBuf {
    resolve_state_dir_for(args, std::env::current_exe().ok().as_deref())
}

/// Logique de resolution testable : `exe` est le chemin du binaire
/// (dans la pratique `std::env::current_exe`).
fn resolve_state_dir_for(args: &Args, exe: Option<&Path>) -> PathBuf {
    if let Some(d) = &args.state_dir {
        return d.clone();
    }
    if let Some(exe) = exe {
        // ADR-0018 : un marqueur `OnionBit.portable` a un ancetre de
        // l'exe (layout `<root>/<os>/onionbit-daemon`) fait remonter
        // `state/` a la racine du bundle — `data/` y est voisine
        // (`PathRoots::for_state_dir`). Avant la detection bundle
        // historique : le marqueur est le signal le plus explicite.
        if let Some(root) = onionbit_core::paths::find_portable_root(exe) {
            return root.join("state");
        }
        if let Some(dir) = exe.parent() {
            let bundle = dir.join("web").join("index.html").is_file()
                || ["OnionBit.exe", "onionbit_ui.exe", "OnionBit", "onionbit_ui"]
                    .iter()
                    .any(|n| dir.join(n).is_file());
            if bundle {
                let state = dir.join("state");
                // Un payload installe sous /opt ou /usr (paquet .deb)
                // imite le layout tarball mais n'est PAS inscriptible
                // pour l'utilisateur — l'etat ne peut vivre a cote de
                // l'exe : repli sur le repertoire XDG.
                if state.is_dir() || std::fs::create_dir(&state).is_ok() {
                    return state;
                }
                return installed_state_dir();
            }
        }
    }
    PathBuf::from(DEFAULT_STATE_DIR)
}

/// Repli d'etat pour un payload installe en lecture seule (paquet
/// systeme) : `$XDG_DATA_HOME/onionbit`, sinon `~/.local/share/
/// onionbit`, sinon `.onionbit` (historique). Hors Unix le marqueur
/// portable couvre deja le cas — le fallback historique suffit.
#[cfg(unix)]
fn installed_state_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(x).join("onionbit");
    }
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".local/share/onionbit");
    }
    PathBuf::from(DEFAULT_STATE_DIR)
}

#[cfg(not(unix))]
fn installed_state_dir() -> PathBuf {
    PathBuf::from(DEFAULT_STATE_DIR)
}

/// Ouvre `http://127.0.0.1:<port>/` dans le navigateur par defaut.
/// `explorer`/`xdg-open` depuis un binaire GUI : aucune console ne
/// s'ouvre (contrairement a un .cmd/.bat).
fn open_web_ui(port: u16) {
    let url = format!("http://127.0.0.1:{port}/");
    #[cfg(windows)]
    let spawned = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();
    if let Err(e) = spawned {
        tracing::warn!(error = %e, url = %url, "ouverture du navigateur impossible");
    }
}

#[derive(Parser)]
#[command(name = "onionbit-daemon", about = "Daemon OnionBit")]
struct Args {
    /// Adresse d'ecoute de l'API REST/SSE (loopback uniquement).
    /// Par defaut : `api/http_host` + `api/http_port` de
    /// `configuration.json` (port 0 = aleatoire, comme Python — le
    /// port reel est publie dans `api/http_port_running`).
    #[arg(long)]
    listen: Option<String>,

    /// Repertoire d'etat (base SQLite, telechargements, configuration.json).
    /// Par defaut : `<exe>/state` dans un bundle dist, sinon `.onionbit`.
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Mode offline (tests) : desactive DHT/trackers/ecoute de pairs et la stack IPv8 —
    /// aucun trafic sortant. La cle API du fichier de configuration reste exigee.
    #[arg(long)]
    offline: bool,

    /// Desactive la stack IPv8 (pas d'overlay, pas de recherche distante) —
    /// equivalent de `ipv8.enabled = false` cote Tribler. Ignore en mode --offline.
    #[arg(long)]
    no_ipv8: bool,

    /// Desactive la TunnelCommunity : les telechargements avec `anon_hops > 0`
    /// seront refuses, la recherche distante reste active. Ignore en mode --offline.
    #[arg(long)]
    no_anonymity: bool,

    /// Port d'ecoute UDP pour la stack IPv8 (override de
    /// `ipv8/interfaces[UDPIPv4].port` ; 0 = dynamique).
    #[arg(long)]
    ipv8_port: Option<u16>,

    /// Pair d'amorcage IPv8 supplementaire au format `ip:port` ou `host:port`
    /// (repetable, s'ajoute aux noeuds bootstrap de la configuration).
    #[arg(long = "bootstrap")]
    bootstrap_peers: Vec<String>,

    /// Rattache une console pour voir les logs (Windows : le binaire
    /// est en sous-systeme GUI, aucune console n'est allouee par
    /// defaut ; une nouvelle console est creee si le processus parent
    /// n'en a pas).
    #[arg(long)]
    console: bool,

    /// Pas d'icone de zone de notification (tests, sessions non
    /// interactives). Equivalent de `tray/enabled = false`.
    #[arg(long)]
    no_tray: bool,

    /// Ouvre l'interface web dans le navigateur par defaut une fois
    /// l'API bindée (raccourci « OnionBit Web » : le daemon demarre au
    /// besoin puis ouvre l'URL — sans passer par un .cmd). Si une
    /// instance tourne deja, le navigateur s'ouvre quand meme et le
    /// processus sort sans toucher aux logs.
    #[arg(long)]
    open_webui: bool,

    /// Repertoire du build Flutter web servi sous `/` (override de
    /// `api/web_ui_dir` ; `api/web_ui_enabled=false` desactive).
    #[arg(long)]
    web_ui_dir: Option<PathBuf>,

    /// Gate de premier boot (ADR-0016) : passe par le lanceur de
    /// l'UI. Sans identite sur disque, la session expose l'API en
    /// `identity_pending` — aucune cle jetable n'est creee et aucun
    /// datagramme signe n'est emis avant le choix utilisateur
    /// (nouvelle / restaurer / invite). Sans ce flag (headless,
    /// ponts), une identite absente est auto-generee comme avant.
    #[arg(long)]
    first_run_gate: bool,

    /// Profil d'anonymat materialise au **premier boot** seulement
    /// (configuration.json absent — ADR-0024, deploiement Docker) :
    /// `legacy`/`full` sont les presets du selecteur (ADR-0022,
    /// `full` exige `stealth.bridges` deja present), `bridge` est la
    /// variante serveur (ADR-0022 §7 : table `full` + `stealth.role`,
    /// sans prerequis). Ignore silencieusement sur un state_dir deja
    /// initialise — la configuration existante fait toujours foi.
    #[arg(long, value_parser = ["legacy", "full", "bridge"], env = "ONIONBIT_PROFILE")]
    profile: Option<String>,
}

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialise le logging `tracing` (fmt, filtre `RUST_LOG`, info par
/// defaut — cf. AGENTS.md "Niveaux de log").
/// Ecrit dans `state_dir/logs/onionbit.log` — fichier du run courant,
/// le precedent est archive en `onionbit.log.N` (cf. `logs::rotate`,
/// `logging/max_files`) ; exploite par l'endpoint `/api/logging` et
/// l'onglet Diagnostic de l'UI — et sur stdout quand une console est
/// disponible (`--console` ou redirection du lanceur).
fn init_tracing(state_dir: &std::path::Path, daemon_config: &DaemonConfig) {
    // Directive d'origine (RUST_LOG ou `info`) — `PUT
    // /api/ipv8/asyncio/debug` recharge le filtre a chaud :
    // `enable` → `debug`, `disable` → directive d'origine.
    // `ipv8/logger_level` Tribler : niveau du logger `ipv8` Python —
    // applique aux crates overlay (`onionbit_ipv8`, `onionbit_tunnel`)
    // en plus du niveau global.
    // Filtre par defaut : `RUST_LOG` redonne le controle total, sinon
    // `info`. `librqbit_upnp` boucle un `warn` par tentative de mapping
    // (routeurs qui refusent en 500) : on le bride en release, le
    // toggle debug / RUST_LOG peuvent toujours le relacher.
    let mut default_directive =
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info,librqbit_upnp=error".to_string());
    let ipv8_level = daemon_config.ipv8.logger_level.trim();
    let mut invalid_level = false;
    match ipv8_level.parse::<tracing::level_filters::LevelFilter>() {
        Ok(lvl) if !ipv8_level.eq_ignore_ascii_case("INFO") => {
            default_directive =
                format!("{default_directive},onionbit_ipv8={lvl},onionbit_tunnel={lvl}");
        }
        Ok(_) => {}
        Err(_) => invalid_level = true,
    }
    let filter = EnvFilter::new(&default_directive);
    let (filter, filter_reload) = tracing_subscriber::reload::Layer::new(filter);
    let logs_dir = state_dir.join("logs");
    let _ = std::fs::create_dir_all(&logs_dir);
    // Un fichier par run : `onionbit.log` = run courant, le precedent
    // est archive (`onionbit.log.1`…) selon `logging/max_files` — plus
    // lisible que la rotation quotidienne qui concatene tous les runs
    // du jour dans le meme fichier.
    logs::rotate(&logs_dir, daemon_config.logging.max_files);
    let file_appender = tracing_appender::rolling::never(&logs_dir, "onionbit.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    Box::leak(Box::new(guard));

    // Horodatage en heure locale (formatteur `SystemTime` par defaut =
    // UTC — le decalage devenait confus en lisant onionbit.log).
    // `LocalTime` retombe sur UTC si l'offset local est indeterminable.
    let timer = tracing_subscriber::fmt::time::LocalTime::rfc_3339();
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_timer(timer.clone());
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_timer(timer)
        .with_writer(non_blocking);

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(onionbit_core::asyncio::DebugLogLayer);
    // Sous-systeme GUI sans console : le handle stdout est invalide,
    // la couche fmt ne servirait qu'a echouer en silence.
    if console::stdout_available() {
        registry.with(stdout_layer).init();
    } else {
        registry.init();
    }
    if invalid_level {
        tracing::warn!(ipv8_level, "ipv8/logger_level invalide (ignore)");
    }

    // Pont `PUT /debug` → `EnvFilter` : le hook vit dans
    // `onionbit-core` (la couche REST ne depend pas du daemon).
    // En mode debug, les crates librqbit vendored restent a `info` :
    // leurs journaux par pair/datagramme (manage_peer timeouts, DHT
    // response_reader, udp_tracker, utp out-of-order) produisaient
    // ~30-40k lignes/session et noyaient les logs metier. `RUST_LOG`
    // redonne le controle total pour deboguer librqbit lui-meme.
    onionbit_core::asyncio::set_filter_reload(move |enable| {
        let directive = if enable {
            concat!(
                "debug",
                ",librqbit=info",
                ",librqbit_dht=info",
                ",librqbit_utp=info",
                ",librqbit_tracker_comms=info",
                ",librqbit_dualstack_sockets=info",
            )
            .to_string()
        } else {
            default_directive.clone()
        };
        if let Err(e) = filter_reload.modify(|f| *f = EnvFilter::new(directive)) {
            tracing::warn!(error = %e, "rechargement du filtre de log impossible");
        }
    });

    // Restaure le mode debug persiste (`logging/debug`) : le choix
    // info/debug de l'onglet Logs survit au redemarrage.
    if daemon_config.logging.debug {
        onionbit_core::asyncio::debug_log().set_enabled(true);
    }
}

/// Ctrl-C d'une console attachee. En sous-systeme GUI sans console,
/// l'enregistrement du handler peut echouer — le futur ne doit alors
/// JAMAIS se resoudre (sinon le graceful shutdown partirait au
/// demarrage).
async fn ctrl_c_or_never() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// SIGTERM unix (`docker stop`, `systemctl stop`, `kill`) — meme
/// sequence d'arret propre que Ctrl-C (ADR-0024, etape 89). Sous
/// Windows ou si l'enregistrement echoue, le futur ne se resout
/// jamais (les autres sources d'arrest restent les seules voies).
#[cfg(unix)]
async fn sigterm_or_never() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            term.recv().await;
            tracing::info!("SIGTERM recu, arret propre");
        }
        Err(e) => {
            tracing::warn!(error = %e, "handler SIGTERM impossible");
            std::future::pending::<()>().await;
        }
    }
}

/// Hors unix : pas de SIGTERM — futur jamais resolu.
#[cfg(not(unix))]
async fn sigterm_or_never() {
    std::future::pending::<()>().await;
}

/// Attend la premiere source d'arret : `ShutdownSignal` (tray «
/// Quitter », `PUT /api/shutdown`), Ctrl-C ou SIGTERM (unix).
async fn wait_shutdown_sources(signal: &ShutdownSignal) {
    tokio::select! {
        _ = signal.wait() => {}
        _ = ctrl_c_or_never() => {}
        _ = sigterm_or_never() => {}
    }
}

/// Cree l'icone systray si `tray.enabled` et pas `--no-tray`. `None`
/// hors Windows ou si la creation a echoue (le daemon continue).
/// Resout le repertoire du build web servi par l'API
/// (`api/web_ui_*`) :
/// 1. `--web-ui-dir` / `api/web_ui_dir` explicite (warn + desactive si
///    `index.html` absent) ;
/// 2. detection auto : `<exe>/web` (dist), `state_dir/web`, puis
///    `app/build/web` du depot (dev).
fn resolve_web_ui_dir(
    args: &Args,
    daemon_config: &DaemonConfig,
    state_dir: &std::path::Path,
) -> Option<PathBuf> {
    if !daemon_config.api.web_ui_enabled {
        return None;
    }
    if let Some(dir) = args.web_ui_dir.clone().or_else(|| {
        // `api/web_ui_dir` peut etre un spec `@root/…` (ADR-0018).
        (!daemon_config.api.web_ui_dir.is_empty()).then(|| {
            onionbit_core::paths::PathRoots::for_state_dir(state_dir)
                .resolve_persisted(&daemon_config.api.web_ui_dir)
        })
    }) {
        return if dir.join("index.html").is_file() {
            Some(dir)
        } else {
            tracing::warn!(
                dir = %dir.display(),
                "web_ui_dir sans index.html — interface web non servie"
            );
            None
        };
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let candidates = [
        exe_dir.as_ref().map(|d| d.join("web")),
        Some(state_dir.join("web")),
        // Dev : <depot>/app/build/web depuis crates/onionbit-daemon.
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build/web")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|d| d.join("index.html").is_file())
}

fn spawn_tray(
    args: &Args,
    daemon_config: &DaemonConfig,
    state_dir: &std::path::Path,
    tooltip: String,
    signal: &ShutdownSignal,
    api_port: std::sync::Arc<std::sync::atomic::AtomicU16>,
    web_ui_served: bool,
) -> Option<tray::TrayHandle> {
    // `headless` Python force l'absence de tray comme `--no-tray`.
    if args.no_tray || daemon_config.headless || !daemon_config.tray.enabled {
        return None;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // `OnionBit.exe` est le nom produit actuel ; `onionbit_ui.exe`
    // reste reconnu (bundles anterieurs au renommage).
    let ui_exe = exe_dir.and_then(|d| {
        ["OnionBit.exe", "onionbit_ui.exe"]
            .iter()
            .map(|n| d.join(n))
            .find(|p| p.exists())
    });
    // `start_minimized` Python ne s'applique pas ici : chez Tribler le
    // core et l'UI sont le MEME processus (`run_tribler`), la cle ne
    // regit que l'etat de la fenetre. Ici le daemon ne lance jamais
    // l'UI — c'est `OnionBit.exe` qui demarre le daemon
    // (`daemon_launcher`), et le menu « Ouvrir OnionBit » du tray reste
    // le seul chemin daemon → UI (action utilisateur explicite).
    // La cle Run doit survivre au repertoire courant : chemins absolus.
    let autostart_cmd = match (std::env::current_exe(), std::path::absolute(state_dir)) {
        (Ok(exe), Ok(state)) => {
            format!("\"{}\" --state-dir \"{}\"", exe.display(), state.display())
        }
        _ => String::new(),
    };
    tray::spawn(tray::TrayOptions {
        tooltip,
        logs_dir: state_dir.join("logs"),
        ui_exe,
        api_port,
        web_ui_served,
        autostart_cmd,
        icon_color: parse_tray_icon_color(&daemon_config.tray_icon_color),
        shutdown: signal.clone(),
        tooltip_rx: None,
    })
}

/// `tray_icon_color` Python : `#RRGGBB` (l'UI ecrit `#E82901`).
/// Vide = `None` ; invalide = warn + `None`.
fn parse_tray_icon_color(s: &str) -> Option<[u8; 3]> {
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let parsed = (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16))
        .and_then(Result::ok);
    match parsed {
        Some(rgb) => Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]),
        None => {
            tracing::warn!(tray_icon_color = %s, "couleur tray invalide (attendu #RRGGBB)");
            None
        }
    }
}

/// Minimum de workers Tokio — `block_in_place` de librqbit consomme
/// des threads workers ; trop peu = l'executor se fige pendant les
/// checks disque au demarrage (le semaphore rqbit est borne par
/// `EngineConfig::runtime_worker_threads`, il doit rester de la
/// marge au-dessus).
fn tokio_worker_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores * 2).max(8)
}

fn main() -> ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(tokio_worker_threads())
        .enable_all()
        .build()
        .expect("runtime tokio");
    rt.block_on(async_main())
}

async fn async_main() -> ExitCode {
    let args = Args::parse();
    if args.console {
        console::attach();
    }
    let state_dir = resolve_state_dir(&args);
    let _ = std::fs::create_dir_all(&state_dir);

    // Instance unique par state_dir : un second lancement (double-clic
    // sur un raccourci, autostart + demarrage manuel) n'ajoute ni
    // icone ni bind en double — il sort silencieusement. Le verrou est
    // pose AVANT `init_tracing` : sinon la rotation archivait le log
    // de l'instance vivante (`onionbit.log` -> `.1`) a chaque
    // tentative en doublon.
    let _instance = match instance::acquire(&state_dir) {
        Some(guard) => guard,
        None => {
            // `--open-webui` : le raccourci web sert aussi a rouvrir
            // l'interface quand le daemon tourne deja — on lit le port
            // reel dans la config de l'instance vivante et on sort.
            if args.open_webui {
                let (cfg, _) = DaemonConfig::load_report(&state_dir.join(CONFIG_FILENAME));
                if cfg.api.web_ui_enabled {
                    let port = match cfg.api.http_port_running {
                        p if p > 0 => p,
                        _ => cfg.api.http_port,
                    };
                    if port > 0 {
                        open_web_ui(port);
                    }
                }
            }
            return ExitCode::SUCCESS;
        }
    };

    // Configuration persistee (`configuration.json`, equivalent de
    // `TriblerConfigManager` : absent ou corrompu -> defauts ; la cle
    // API est generee au premier run et le fichier normalise). Chargee
    // AVANT `init_tracing` pour que `ipv8/logger_level` fasse partie
    // de la directive de base.
    let config_path = state_dir.join(CONFIG_FILENAME);
    // `load_report` remonte l'erreur de parse pour la notifier en
    // `report_config_error` une fois la session (et son bus) creee.
    let (mut daemon_config, config_error) = DaemonConfig::load_report(&config_path);
    init_tracing(&state_dir, &daemon_config);
    if let Some(err) = &config_error {
        // Le warn interne de `load_report` a ete emis avant
        // l'installation du subscriber : on le rejoue ici.
        tracing::warn!(
            error = %err,
            path = %config_path.display(),
            "configuration.json corrompu, repli sur les valeurs par defaut"
        );
    }
    if !config_path.exists() {
        // Premier boot : `--profile` materialise le preset choisi
        // (ADR-0024) — la config existante fait toujours foi ensuite,
        // jamais de couche d'override (ADR-0022 §1).
        if let Some(variant) = &args.profile {
            match onionbit_core::privacy::PrivacyProfile::apply_first_boot(&daemon_config, variant)
            {
                Ok(cfg) => {
                    daemon_config = cfg;
                    tracing::info!(profile = %variant, "profil materielise au premier boot");
                }
                Err(e) => {
                    tracing::error!(error = %e, profile = %variant, "--profile refuse");
                    return ExitCode::FAILURE;
                }
            }
        }
        if let Err(e) = daemon_config.write(&config_path) {
            tracing::warn!(error = %e, "ecriture initiale de configuration.json impossible");
        }
    }

    // ADR-0018 etape 57 : migration idempotente des chemins persistes
    // vers les specs `@root/…` (les absolus sous les racines sont
    // reecrits ; les externes conserves). Le fichier n'est reecrit que
    // si une valeur a change.
    let path_roots = onionbit_core::paths::PathRoots::for_state_dir(&state_dir);
    if daemon_config.migrate_persisted_paths(&path_roots) {
        if let Err(e) = daemon_config.write(&config_path) {
            tracing::warn!(error = %e, "reecriture de configuration.json (chemins portables) impossible");
        }
    }

    // Adresse d'ecoute : --listen > api/http_host+http_port du fichier.
    let listen_str = args.listen.clone().unwrap_or_else(|| {
        format!(
            "{}:{}",
            daemon_config.api.http_host, daemon_config.api.http_port
        )
    });
    let listen: SocketAddr = match listen_str.parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, listen = %listen_str, "adresse d'ecoute invalide");
            return ExitCode::FAILURE;
        }
    };
    if !listen.ip().is_loopback() {
        tracing::error!(
            listen = %listen,
            "l'ecoute doit etre une adresse loopback (127.0.0.1 ou ::1) : \
             l'API de controle ne doit jamais etre exposee sur le reseau"
        );
        return ExitCode::FAILURE;
    }

    // CoreConfig : l'arbre persiste + overrides CLI (--offline isole
    // completement, comme avant).
    let config = if args.offline {
        CoreConfig::offline(state_dir.clone())
    } else {
        let mut cfg = daemon_config.to_core_config(&state_dir);
        if args.no_ipv8 {
            cfg.ipv8.enabled = false;
        }
        if args.no_anonymity {
            cfg.ipv8.enable_anonymity = false;
        }
        if let Some(port) = args.ipv8_port {
            cfg.ipv8.listen_addr = format!("0.0.0.0:{port}");
        }
        cfg.ipv8
            .bootstrap_peers
            .extend(args.bootstrap_peers.clone());
        cfg
    };

    // Source unique d'arret : Ctrl-C, tray « Quitter », /api/shutdown.
    // Cree avant la session pour que le tray « Quitter » existe meme
    // pendant le demarrage.
    let shutdown_signal = ShutdownSignal::new();

    // Repertoire du build web servi par l'API (`api/web_ui_*`) —
    // resolu avant le tray pour activer « Ouvrir dans le navigateur ».
    let web_ui_dir = resolve_web_ui_dir(&args, &daemon_config, &state_dir);

    // Port HTTP reel publie vers le tray (« Ouvrir dans le
    // navigateur ») — 0 tant que l'API n'est pas bindée.
    let api_port = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0));

    // Icone systray immediate (le GUI Python s'affiche avant que le
    // core soit pret) — la restauration des telechargements et le
    // peuplement DHT peuvent prendre ~1 min sur de gros fichiers ; le
    // tooltip est maj avec le port reel une fois l'API bindée.
    let tray = spawn_tray(
        &args,
        &daemon_config,
        &state_dir,
        "OnionBit".into(),
        &shutdown_signal,
        api_port.clone(),
        web_ui_dir.is_some(),
    );

    // ADR-0016 : refus ferme `at_rest` × role serveur stealth avant
    // tout boot — un pont doit pouvoir redemarrer sans surveillance.
    if daemon_config.identity.at_rest
        && daemon_config.stealth.enabled
        && daemon_config.stealth.role != "client"
    {
        tracing::error!(
            role = %daemon_config.stealth.role,
            "identity.at_rest incompatible avec stealth.role != \"client\""
        );
        return ExitCode::FAILURE;
    }

    // ADR-0016 etape 48d : `--first-run-gate` (lanceur UI) maintient
    // la session en `identity_pending` sur un state_dir vierge ;
    // `locked` quand `identity.at_rest` a scelle la graine.
    let session = match CoreSession::start_gated(config, Notifier::new(), args.first_run_gate).await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "demarrage de la session impossible");
            if let Some(t) = tray {
                t.stop();
            }
            return ExitCode::FAILURE;
        }
    };

    // `report_config_error` Python : une config corrompue est signalee
    // au GUI (les clients SSE connectes apres le demarrage ne la
    // reverront pas — meme comportement que le Notifier Python).
    if let Some(err) = config_error {
        session
            .notifier()
            .notify(onionbit_core::Notification::ReportConfigError { error: err });
    }

    if !daemon_config.api.http_enabled {
        // api/http_enabled=false : le daemon tourne sans plan de
        // controle HTTP (comme Tribler sans REST manager).
        tracing::warn!("api/http_enabled=false : l'API de controle n'est pas exposee");
        wait_shutdown_sources(&shutdown_signal).await;
        tracing::info!("signal d'arret recu, fermeture de la session");
        session.stop().await;
        if let Some(t) = tray {
            t.stop();
        }
        tracing::info!("daemon arrete proprement");
        return ExitCode::SUCCESS;
    }

    tracing::info!(listen = %listen, "demarrage de l'API de controle");

    // Port configure occupe -> repli ephemere (`http_port_running` est
    // de toute facon publie ; mieux qu'un echec de demarrage).
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) if listen.port() != 0 => {
            tracing::warn!(
                error = %e,
                listen = %listen,
                "port configure indisponible, repli sur un port ephemere"
            );
            match tokio::net::TcpListener::bind(format!("{}:0", listen.ip())).await {
                Ok(l) => l,
                Err(e2) => {
                    tracing::error!(error = %e2, "bind impossible");
                    session.stop().await;
                    if let Some(t) = tray {
                        t.stop();
                    }
                    return ExitCode::FAILURE;
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, listen = %listen, "bind impossible");
            session.stop().await;
            if let Some(t) = tray {
                t.stop();
            }
            return ExitCode::FAILURE;
        }
    };

    if let Some(dir) = &web_ui_dir {
        tracing::info!(dir = %dir.display(), "interface web servie en same-origin sur /");
    }
    let app = build(
        AppState::new(session.clone())
            .with_daemon_config(daemon_config.clone(), Some(config_path.clone()))
            // ADR-0016 : le transport furtif est resolu
            // dynamiquement depuis la session (`AppState::
            // stealth_transport()`) — il n'existe qu'apres le
            // demarrage differe de l'identite en mode gate.
            .with_shutdown_notify(shutdown_signal.notifier())
            .with_web_ui_dir(web_ui_dir.clone())
            .with_web_ui_inject_key(daemon_config.api.web_ui_inject_key),
    );

    // `api/https_*` Python : second site TLS du meme routeur
    // (`start_https_site`), port reel reecrit dans
    // `https_port_running`.
    let https_handle = if daemon_config.api.https_enabled {
        match https::spawn(
            app.clone(),
            &daemon_config.api.https_host,
            daemon_config.api.https_port,
            &daemon_config.api.https_certfile,
            &state_dir,
        )
        .await
        {
            Ok((port, handle)) => {
                daemon_config.api.https_port_running = port;
                Some(handle)
            }
            Err(e) => {
                // Python : l'echec du site HTTPS n'arrete pas l'HTTP.
                tracing::error!(error = %e, "demarrage du listener HTTPS impossible");
                None
            }
        }
    } else {
        None
    };

    // Publie les ports reels (`api/http_port_running` et
    // `api/https_port_running`, lus par les clients comme onionbit-cli —
    // Python fait de meme en fin de demarrage).
    if let Ok(addr) = listener.local_addr() {
        daemon_config.api.http_port_running = addr.port();
        api_port.store(addr.port(), std::sync::atomic::Ordering::Relaxed);
    }
    if let Err(e) = daemon_config.write(&config_path) {
        tracing::warn!(error = %e, "reecriture de configuration.json impossible");
    }

    // Tooltip du tray : port REEL (http_port=0 => ephemere — afficher
    // `listen` montrerait `127.0.0.1:0`).
    if let Some(t) = &tray {
        if let Ok(addr) = listener.local_addr() {
            t.set_tooltip(format!("OnionBit — {addr}"));
        }
    }

    // `--open-webui` (raccourci « OnionBit Web ») : le navigateur
    // s'ouvre une fois le port reel connu — uniquement si l'UI web
    // est servie (sinon l'URL serait une erreur d'API).
    if args.open_webui && web_ui_dir.is_some() {
        if let Ok(addr) = listener.local_addr() {
            open_web_ui(addr.port());
        }
    }

    // Arret propre : Ctrl-C / tray « Quitter » / PUT /api/shutdown ->
    // session.stop() -> fin du serveur. `stop()` est idempotent : la
    // sequence lancee par le handler shutdown n'est pas dedoublee.
    // `drain_begin` marque la fin de `stop()` = le debut du drain des
    // connexions : c'est cette phase seule qui est bornee plus bas.
    let (drain_begin_tx, drain_begin_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = {
        let signal = shutdown_signal.clone();
        let session = session.clone();
        async move {
            wait_shutdown_sources(&signal).await;
            tracing::info!("signal d'arret recu, fermeture de la session");
            session.stop().await;
            if let Some(handle) = https_handle {
                handle.graceful_shutdown(Some(https::SHUTDOWN_GRACE));
            }
            let _ = drain_begin_tx.send(());
        }
    };

    // `ConnectInfo<SocketAddr>` : l'IP cliente est extraite par les
    // endpoints sensibles au brute-force (`POST /api/identity/unlock`,
    // ADR-0016 — rate-limit par IP + global).
    let server = async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
    };
    tokio::pin!(server);
    // Borne dure sur le drain : `with_graceful_shutdown` attend la
    // fin des connexions en vol sans limite — un flux residuel
    // (stream media, handler bloque) figeait le processus a jamais.
    // Au-dela du delai, abandonner le serveur coupe les connexions.
    let serve_result = tokio::select! {
        r = &mut server => r,
        _ = async {
            let _ = drain_begin_rx.await;
            tokio::time::sleep(https::SHUTDOWN_GRACE).await;
            tracing::warn!("drain des connexions trop long — arret force");
        } => Ok(()),
    };
    if let Some(t) = tray {
        t.stop();
    }
    if let Err(e) = serve_result {
        tracing::error!(error = %e, "serveur API en erreur");
        return ExitCode::FAILURE;
    }
    tracing::info!("daemon arrete proprement");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{resolve_state_dir_for, Args, DEFAULT_STATE_DIR};
    use clap::Parser;
    use std::path::PathBuf;

    fn args_sans_flag() -> Args {
        Args::try_parse_from(["onionbit-daemon"]).unwrap()
    }

    /// Exe isole (ni marqueur portable ni `web/` voisin) → le defaut
    /// historique `.onionbit` relatif au CWD.
    #[test]
    fn state_dir_defaut_hors_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("onionbit-daemon");
        let got = resolve_state_dir_for(&args_sans_flag(), Some(&exe));
        assert_eq!(got, PathBuf::from(DEFAULT_STATE_DIR));
        // --state-dir prime toujours.
        let args = Args::try_parse_from(["onionbit-daemon", "--state-dir", "/tmp/x"]).unwrap();
        assert_eq!(
            resolve_state_dir_for(&args, Some(&exe)),
            PathBuf::from("/tmp/x")
        );
    }

    /// Layout « tarball » (web/ voisin, repertoire inscriptible) →
    /// `state/` a cote de l'exe — et il est cree au passage.
    #[test]
    fn state_dir_bundle_inscriptible() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web")).unwrap();
        std::fs::write(dir.path().join("web/index.html"), "x").unwrap();
        let exe = dir.path().join("onionbit-daemon");
        let got = resolve_state_dir_for(&args_sans_flag(), Some(&exe));
        assert_eq!(got, dir.path().join("state"));
        assert!(got.is_dir());
    }

    /// Meme layout mais repertoire en lecture seule (paquet .deb sous
    /// /opt/onionbit) → repli `$XDG_DATA_HOME/onionbit` —
    /// l'etat ne peut vivre a cote d'un exe non inscriptible.
    /// (POSIX : le mode 0555 bloque la creation ; sous Windows le bit
    /// readonly d'un dossier n'empeche pas l'ecriture — non teste.)
    #[cfg(unix)]
    #[test]
    fn state_dir_bundle_lecture_seule_repli_xdg() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web")).unwrap();
        std::fs::write(dir.path().join("web/index.html"), "x").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let exe = dir.path().join("onionbit-daemon");
        let got = resolve_state_dir_for(&args_sans_flag(), Some(&exe));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(got, dir.path().join("state"));
        assert_eq!(got.file_name().unwrap(), "onionbit");
        assert!(
            got == std::path::Path::new(".onionbit") || got.is_absolute(),
            "repli attendu XDG ou defaut, obtenu {got:?}"
        );
    }
}
