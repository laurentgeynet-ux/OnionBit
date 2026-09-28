//! `tribler-daemon` — binaire principal du daemon Tribler-Rust-Torrent.
//!
//! Point d'entree du processus : charge la configuration persistee
//! (`state_dir/configuration.json`, equivalent `TriblerConfig`
//! Python — les flags CLI sont des overrides), initialise le logging
//! (`tracing`), assemble `tribler-core` (session + moteur BitTorrent +
//! base) puis demarre `tribler-api` pour exposer le plan de controle
//! local sur `127.0.0.1` derriere la cle API. Aucun client (CLI ou
//! future UI Flutter) ne parle a autre chose qu'a `tribler-api`.
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
mod shutdown;
mod tray;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use shutdown::ShutdownSignal;
use tribler_api::{build, AppState};
use tribler_core::{CoreConfig, CoreSession, DaemonConfig, Notifier, CONFIG_FILENAME};

/// Repertoire d'etat par defaut (relatif au dossier courant).
const DEFAULT_STATE_DIR: &str = ".tribler";

#[derive(Parser)]
#[command(name = "tribler-daemon", about = "Daemon Tribler-Rust-Torrent")]
struct Args {
    /// Adresse d'ecoute de l'API REST/SSE (loopback uniquement).
    /// Par defaut : `api/http_host` + `api/http_port` de
    /// `configuration.json` (port 0 = aleatoire, comme Python — le
    /// port reel est publie dans `api/http_port_running`).
    #[arg(long)]
    listen: Option<String>,

    /// Repertoire d'etat (base SQLite, telechargements, configuration.json).
    #[arg(long, default_value = DEFAULT_STATE_DIR)]
    state_dir: PathBuf,

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
}

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialise le logging `tracing` (fmt, filtre `RUST_LOG`, info par
/// defaut — cf. AGENTS.md "Niveaux de log").
/// Ecrit dans le fichier tournant `state_dir/logs/tribler.log`
/// (exploite par l'endpoint `/api/logging` et l'onglet Diagnostic de
/// l'UI) et sur stdout quand une console est disponible (`--console`
/// ou redirection du lanceur).
fn init_tracing(state_dir: &std::path::Path, daemon_config: &DaemonConfig) {
    // Directive d'origine (RUST_LOG ou `info`) — `PUT
    // /api/ipv8/asyncio/debug` recharge le filtre a chaud :
    // `enable` → `debug`, `disable` → directive d'origine.
    // `ipv8/logger_level` Tribler : niveau du logger `ipv8` Python —
    // applique aux crates overlay (`tribler_ipv8`, `tribler_tunnel`)
    // en plus du niveau global.
    let mut default_directive = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let ipv8_level = daemon_config.ipv8.logger_level.trim();
    let mut invalid_level = false;
    match ipv8_level.parse::<tracing::level_filters::LevelFilter>() {
        Ok(lvl) if !ipv8_level.eq_ignore_ascii_case("INFO") => {
            default_directive =
                format!("{default_directive},tribler_ipv8={lvl},tribler_tunnel={lvl}");
        }
        Ok(_) => {}
        Err(_) => invalid_level = true,
    }
    let filter = EnvFilter::new(&default_directive);
    let (filter, filter_reload) = tracing_subscriber::reload::Layer::new(filter);
    let logs_dir = state_dir.join("logs");
    let _ = std::fs::create_dir_all(&logs_dir);
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "tribler.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    Box::leak(Box::new(guard));

    let stdout_layer = tracing_subscriber::fmt::layer().with_target(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(non_blocking);

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(tribler_core::asyncio::DebugLogLayer);
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
    // `tribler-core` (la couche REST ne depend pas du daemon).
    tribler_core::asyncio::set_filter_reload(move |enable| {
        let directive = if enable {
            "debug".to_string()
        } else {
            default_directive.clone()
        };
        if let Err(e) = filter_reload.modify(|f| *f = EnvFilter::new(directive)) {
            tracing::warn!(error = %e, "rechargement du filtre de log impossible");
        }
    });
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

/// Attend la premiere source d'arret : `ShutdownSignal` (tray «
/// Quitter », `PUT /api/shutdown`) ou Ctrl-C.
async fn wait_shutdown_sources(signal: &ShutdownSignal) {
    tokio::select! {
        _ = signal.wait() => {}
        _ = ctrl_c_or_never() => {}
    }
}

/// Cree l'icone systray si `tray.enabled` et pas `--no-tray`. `None`
/// hors Windows ou si la creation a echoue (le daemon continue).
fn spawn_tray(
    args: &Args,
    daemon_config: &DaemonConfig,
    tooltip: String,
    signal: &ShutdownSignal,
) -> Option<tray::TrayHandle> {
    // `headless` Python force l'absence de tray comme `--no-tray`.
    if args.no_tray || daemon_config.headless || !daemon_config.tray.enabled {
        return None;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let ui_exe = exe_dir
        .as_ref()
        .map(|d| d.join("tribler_ui.exe"))
        .filter(|p| p.exists());
    // `start_minimized` Python : `run_tribler` n'ouvre l'UI qu'au
    // demarrage non minimise. Equivalent daemon : lancer
    // `tribler_ui.exe` quand il est livre a cote du daemon.
    if !daemon_config.start_minimized {
        if let Some(exe) = &ui_exe {
            match std::process::Command::new(exe).spawn() {
                Ok(_) => tracing::info!("interface tribler_ui lancee au demarrage"),
                Err(e) => tracing::warn!(error = %e, "lancement de tribler_ui impossible"),
            }
        }
    }
    // La cle Run doit survivre au repertoire courant : chemins absolus.
    let autostart_cmd = match (
        std::env::current_exe(),
        std::path::absolute(&args.state_dir),
    ) {
        (Ok(exe), Ok(state)) => {
            format!("\"{}\" --state-dir \"{}\"", exe.display(), state.display())
        }
        _ => String::new(),
    };
    tray::spawn(tray::TrayOptions {
        tooltip,
        logs_dir: args.state_dir.join("logs"),
        ui_exe,
        autostart_cmd,
        icon_color: parse_tray_icon_color(&daemon_config.tray_icon_color),
        shutdown: signal.clone(),
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

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    #[cfg(windows)]
    if args.console {
        console::attach();
    }

    // Configuration persistee (`configuration.json`, equivalent de
    // `TriblerConfigManager` : absent ou corrompu -> defauts ; la cle
    // API est generee au premier run et le fichier normalise). Chargee
    // AVANT `init_tracing` pour que `ipv8/logger_level` fasse partie
    // de la directive de base.
    let config_path = args.state_dir.join(CONFIG_FILENAME);
    // `load_report` remonte l'erreur de parse pour la notifier en
    // `report_config_error` une fois la session (et son bus) creee.
    let (mut daemon_config, config_error) = DaemonConfig::load_report(&config_path);
    init_tracing(&args.state_dir, &daemon_config);
    if config_error.is_some() {
        // Le warn interne de `load_report` a ete emis avant
        // l'installation du subscriber : on le rejoue ici.
        tracing::warn!(
            path = %config_path.display(),
            "configuration.json corrompu, repli sur les valeurs par defaut"
        );
    }

    // Instance unique par state_dir : un second lancement (double-clic
    // sur demarrer.cmd, autostart + demarrage manuel) n'ajoute ni
    // icone ni bind en double — il sort silencieusement.
    let _instance = match instance::acquire(&args.state_dir) {
        Some(guard) => guard,
        None => {
            tracing::warn!(
                state_dir = %args.state_dir.display(),
                "une instance du daemon est deja en cours pour ce repertoire d'etat"
            );
            return ExitCode::SUCCESS;
        }
    };
    if !config_path.exists() {
        if let Err(e) = daemon_config.write(&config_path) {
            tracing::warn!(error = %e, "ecriture initiale de configuration.json impossible");
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
        CoreConfig::offline(args.state_dir.clone())
    } else {
        let mut cfg = daemon_config.to_core_config(&args.state_dir);
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

    let session = match CoreSession::start(config, Notifier::new()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "demarrage de la session impossible");
            return ExitCode::FAILURE;
        }
    };

    // `report_config_error` Python : une config corrompue est signalee
    // au GUI (les clients SSE connectes apres le demarrage ne la
    // reverront pas — meme comportement que le Notifier Python).
    if let Some(err) = config_error {
        session
            .notifier()
            .notify(tribler_core::Notification::ReportConfigError { error: err });
    }

    // Source unique d'arret : Ctrl-C, tray « Quitter », /api/shutdown.
    let shutdown_signal = ShutdownSignal::new();

    if !daemon_config.api.http_enabled {
        // api/http_enabled=false : le daemon tourne sans plan de
        // controle HTTP (comme Tribler sans REST manager).
        tracing::warn!("api/http_enabled=false : l'API de controle n'est pas exposee");
        let tray = spawn_tray(&args, &daemon_config, "Tribler".into(), &shutdown_signal);
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

    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, listen = %listen, "bind impossible");
            session.stop().await;
            return ExitCode::FAILURE;
        }
    };

    let app = build(
        AppState::new(session.clone())
            .with_daemon_config(daemon_config.clone(), Some(config_path.clone()))
            .with_shutdown_notify(shutdown_signal.notifier()),
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
            &args.state_dir,
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
    // `api/https_port_running`, lus par les clients comme tribler-cli —
    // Python fait de meme en fin de demarrage).
    if let Ok(addr) = listener.local_addr() {
        daemon_config.api.http_port_running = addr.port();
    }
    if let Err(e) = daemon_config.write(&config_path) {
        tracing::warn!(error = %e, "reecriture de configuration.json impossible");
    }

    let tray = spawn_tray(
        &args,
        &daemon_config,
        format!("Tribler — {listen}"),
        &shutdown_signal,
    );

    // Arret propre : Ctrl-C / tray « Quitter » / PUT /api/shutdown ->
    // session.stop() -> fin du serveur. `stop()` est idempotent : la
    // sequence lancee par le handler shutdown n'est pas dedoublee.
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
        }
    };

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
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
    #[test]
    fn le_binaire_parse_ses_arguments() {
        // Validation clap minimale (les tests e2e du daemon viendront
        // avec le durcissement de l'etape 16 ; le chemin HTTP est
        // deja couvert par tribler-api/tests et tribler-cli/tests).
        use clap::CommandFactory;
        super::Args::command().debug_assert();
    }
}
