//! `tribler-cli` — outil de pilotage en ligne de commande.
//!
//! Client du plan de controle expose par `tribler-daemon` via
//! `tribler-api` (REST/SSE sur `127.0.0.1`). Ne contient aucune
//! logique metier : traduit des sous-commandes CLI en appels REST, a
//! l'image de `mule-cli` dans le projet eMule-Rust.
//!
//! Implemente : `status`, `list`, `add`, `remove`, `pause`, `resume`
//! (etape 7).

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::json;

/// Adresse d'ecoute par defaut du daemon (bind loopback, cf.
/// `tribler-daemon`). Le Python Tribler utilise `api/http_port=0`
/// (port aleatoire) ; on documente un port fixe pour le CLI.
const DEFAULT_API: &str = "http://127.0.0.1:8085";

/// Timeout des appels HTTP de pilotage.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(
    name = "tribler-cli",
    about = "Pilotage du daemon Tribler-Rust via l'API REST locale"
)]
struct Cli {
    /// URL de base de l'API du daemon.
    #[arg(long, default_value = DEFAULT_API, global = true)]
    api: String,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Verifie que le daemon repond et affiche un resume.
    Status,
    /// Liste les telechargements (GET /api/downloads).
    List,
    /// Ajoute un telechargement (magnet, http(s) ou .torrent local).
    Add {
        /// Magnet/URI http(s), ou chemin d'un fichier `.torrent`.
        source: String,
        /// Demarrer en pause.
        #[arg(long)]
        paused: bool,
    },
    /// Supprime un telechargement.
    Remove {
        /// Info-hash hex (40 caracteres).
        infohash: String,
        /// Supprimer aussi les donnees sur disque.
        #[arg(long)]
        remove_data: bool,
    },
    /// Met en pause un telechargement.
    Pause {
        /// Info-hash hex.
        infohash: String,
    },
    /// Reprend un telechargement.
    Resume {
        /// Info-hash hex.
        infohash: String,
    },
}

/// Construit le client HTTP partage.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("client HTTP")
}

/// Consomme la reponse : corps JSON si succes, sinon message d'erreur
/// formate depuis `{error:{handled,message}}` (format Tribler).
async fn ensure_ok(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = body["error"]["message"]
            .as_str()
            .unwrap_or("(pas de message)");
        return Err(format!("HTTP {status} : {msg}"));
    }
    Ok(body)
}

/// `status` — joignabilite + resume.
async fn cmd_status(api: &str) -> Result<(), String> {
    let resp = client()
        .get(format!("{api}/api/downloads"))
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    let body = ensure_ok(resp).await?;
    let n = body["downloads"].as_array().map_or(0, Vec::len);
    println!("daemon OK sur {api} — {n} telechargement(s)");
    Ok(())
}

/// `list` — tableau des telechargements.
async fn cmd_list(api: &str) -> Result<(), String> {
    let resp = client()
        .get(format!("{api}/api/downloads"))
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    let body = ensure_ok(resp).await?;
    let downloads = body["downloads"].as_array().cloned().unwrap_or_default();
    if downloads.is_empty() {
        println!("(aucun telechargement)");
        return Ok(());
    }
    println!(
        "{:<40}  {:<22}  {:>6}  {:>9}  {:>9}  NOM",
        "INFOHASH", "STATUS", "PROGR.", "DOWN/s", "UP/s"
    );
    for d in downloads {
        println!(
            "{:<40}  {:<22}  {:>5.1}%  {:>9}  {:>9}  {}",
            d["infohash"].as_str().unwrap_or("?"),
            d["status"].as_str().unwrap_or("?"),
            d["progress"].as_f64().unwrap_or(0.0) * 100.0,
            d["speed_down"].as_u64().unwrap_or(0),
            d["speed_up"].as_u64().unwrap_or(0),
            d["name"].as_str().unwrap_or("?"),
        );
    }
    Ok(())
}

/// `add` — magnet/URI ou chemin `.torrent` local.
async fn cmd_add(api: &str, source: &str, paused: bool) -> Result<(), String> {
    // Le champ JSON depend de la nature de la source : `uri` pour
    // magnet/http, `torrent` pour un chemin local (comme le Python).
    let is_uri = source.starts_with("magnet:") || source.starts_with("http");
    let mut req = json!({"paused": paused});
    if is_uri {
        req["uri"] = json!(source);
    } else {
        req["torrent"] = json!(source);
    }
    let resp = client()
        .put(format!("{api}/api/downloads"))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    ensure_ok(resp).await?;
    println!("ajoute : {source}");
    Ok(())
}

/// `remove`.
async fn cmd_remove(api: &str, infohash: &str, remove_data: bool) -> Result<(), String> {
    let resp = client()
        .delete(format!("{api}/api/downloads/{infohash}"))
        .json(&json!({"remove_data": remove_data}))
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    ensure_ok(resp).await?;
    println!("supprime : {infohash}");
    Ok(())
}

/// `pause`/`resume` — PATCH state.
async fn cmd_patch(api: &str, infohash: &str, state: &str) -> Result<(), String> {
    let resp = client()
        .patch(format!("{api}/api/downloads/{infohash}"))
        .json(&json!({"state": state}))
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    ensure_ok(resp).await?;
    println!("{state} : {infohash}");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Command::Status => cmd_status(&cli.api).await,
        Command::List => cmd_list(&cli.api).await,
        Command::Add { source, paused } => cmd_add(&cli.api, &source, paused).await,
        Command::Remove {
            infohash,
            remove_data,
        } => cmd_remove(&cli.api, &infohash, remove_data).await,
        Command::Pause { infohash } => cmd_patch(&cli.api, &infohash, "stop").await,
        Command::Resume { infohash } => cmd_patch(&cli.api, &infohash, "resume").await,
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn le_squelette_compile() {
        // Les tests d'integration bout en bout (CLI -> API -> session)
        // arrivent avec tribler-daemon (etape 8) ; les handlers sont
        // couverts par crates/tribler-api/tests/api.rs.
    }
}
