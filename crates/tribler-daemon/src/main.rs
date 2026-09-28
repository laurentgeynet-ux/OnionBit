//! `tribler-daemon` — binaire principal du daemon Tribler-Rust-Torrent.
//!
//! Point d'entree du processus : charge la configuration, initialise le
//! logging (`tracing`), assemble `tribler-core` (session + moteur
//! BitTorrent + base) puis demarre `tribler-api` pour exposer le plan de
//! controle local sur `127.0.0.1`. Aucun client (CLI ou future UI
//! Flutter) ne parle a autre chose qu'a `tribler-api`.
//!
//! Etat : etape 8 — BitTorrent + API + DB, sans IPv8 (etapes 9-12).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use tribler_api::{build, AppState};
use tribler_core::{CoreConfig, CoreSession, Notifier};

/// Adresse d'ecoute par defaut du plan de controle (loopback
/// uniquement — l'API n'est jamais exposee sur le reseau local).
/// Correspond a `DEFAULT_API` de `tribler-cli`.
const DEFAULT_LISTEN: &str = "127.0.0.1:8085";

/// Repertoire d'etat par defaut (relatif au dossier courant).
const DEFAULT_STATE_DIR: &str = ".tribler";

#[derive(Parser)]
#[command(name = "tribler-daemon", about = "Daemon Tribler-Rust-Torrent")]
struct Args {
    /// Adresse d'ecoute de l'API REST/SSE (loopback par defaut).
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: String,

    /// Repertoire d'etat (base SQLite, telechargements).
    #[arg(long, default_value = DEFAULT_STATE_DIR)]
    state_dir: PathBuf,

    /// Mode offline (tests) : desactive DHT/trackers/ecoute de pairs et la stack IPv8 —
    /// aucun trafic sortant.
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

    /// Port d'ecoute UDP pour la stack IPv8 (defaut 8090, 0 = dynamique).
    #[arg(long, default_value_t = tribler_core::ipv8_stack::DEFAULT_IPV8_PORT)]
    ipv8_port: u16,

    /// Pair d'amorcage IPv8 supplementaire au format `ip:port` ou `host:port`
    /// (repetable, s'ajoute aux noeuds bootstrap par defaut).
    #[arg(long = "bootstrap")]
    bootstrap_peers: Vec<String>,
}

/// Initialise le logging `tracing` (fmt, filtre `RUST_LOG`, info par
/// defaut — cf. AGENTS.md "Niveaux de log").
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let args = Args::parse();

    let listen: SocketAddr = match args.listen.parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, listen = %args.listen, "adresse --listen invalide");
            return ExitCode::FAILURE;
        }
    };
    if !listen.ip().is_loopback() {
        tracing::error!(
            listen = %listen,
            "--listen doit etre une adresse loopback (127.0.0.1 ou ::1) : \
             l'API de controle ne doit jamais etre exposee sur le reseau"
        );
        return ExitCode::FAILURE;
    }

    let config = if args.offline {
        CoreConfig::offline(args.state_dir)
    } else {
        let mut cfg = CoreConfig {
            state_dir: args.state_dir.clone(),
            downloads_dir: args.state_dir.join("downloads"),
            ..Default::default()
        };
        if !args.no_ipv8 {
            let mut ipv8 = tribler_core::Ipv8Config::production();
            ipv8.listen_addr = format!("0.0.0.0:{}", args.ipv8_port);
            ipv8.enable_anonymity = !args.no_anonymity;
            if !args.bootstrap_peers.is_empty() {
                ipv8.bootstrap_peers.extend(args.bootstrap_peers);
            }
            cfg.ipv8 = ipv8;
        }
        cfg
    };

    let session = match CoreSession::start(config, Notifier::new()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "demarrage de la session impossible");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(listen = %listen, "demarrage de l'API de controle");

    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, listen = %listen, "bind impossible");
            session.stop().await;
            return ExitCode::FAILURE;
        }
    };

    let app = build(AppState::new(session.clone()));

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
