//! `tribler-daemon` — binaire principal du daemon Tribler-Rust-Torrent.
//!
//! Point d'entree du processus : charge la configuration persistee
//! (`state_dir/configuration.json`, equivalent `TriblerConfig`
//! Python — les flags CLI sont des overrides), initialise le logging
//! (`tracing`), assemble `tribler-core` (session + moteur BitTorrent +
//! base) puis demarre `tribler-api` pour exposer le plan de controle
//! local sur `127.0.0.1` derriere la cle API. Aucun client (CLI ou
//! future UI Flutter) ne parle a autre chose qu'a `tribler-api`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

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
}

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialise le logging `tracing` (fmt, filtre `RUST_LOG`, info par
/// defaut — cf. AGENTS.md "Niveaux de log").
/// Ecrit a la fois sur stdout et dans le fichier tournant `state_dir/logs/tribler.log`
/// (exploite par l'endpoint `/api/logging` et l'onglet Diagnostic de l'UI).
fn init_tracing(state_dir: &std::path::Path) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
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

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(&args.state_dir);

    // Configuration persistee (`configuration.json`, equivalent de
    // `TriblerConfigManager` : absent ou corrompu -> defauts ; la cle
    // API est generee au premier run et le fichier normalise).
    let config_path = args.state_dir.join(CONFIG_FILENAME);
    let mut daemon_config = DaemonConfig::load(&config_path);
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

    if !daemon_config.api.http_enabled {
        // api/http_enabled=false : le daemon tourne sans plan de
        // controle HTTP (comme Tribler sans REST manager).
        tracing::warn!("api/http_enabled=false : l'API de controle n'est pas exposee");
        let _ = tokio::signal::ctrl_c().await;
        session.stop().await;
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

    // Publie le port reel (`api/http_port_running`, lu par les clients
    // comme tribler-cli — Python fait de meme en fin de demarrage).
    if let Ok(addr) = listener.local_addr() {
        daemon_config.api.http_port_running = addr.port();
        if let Err(e) = daemon_config.write(&config_path) {
            tracing::warn!(error = %e, "reecriture de configuration.json impossible");
        }
    }

    let app =
        build(AppState::new(session.clone()).with_daemon_config(daemon_config, Some(config_path)));

    // Arret propre : Ctrl-C -> session.stop() -> fin du serveur.
    let shutdown = async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("signal d'arret recu, fermeture de la session");
        session.stop().await;
    };

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
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
