//! `tribler-cli` — outil de pilotage en ligne de commande.
//!
//! Client du plan de controle expose par `tribler-daemon` via
//! `tribler-api` (REST/SSE sur `127.0.0.1`). Ne contient aucune
//! logique metier : traduit des sous-commandes CLI en appels REST, a
//! l'image de `mule-cli` dans le projet eMule-Rust.
//!
//! Implemente : `status`, `list`, `add`, `remove`, `pause`, `resume`
//! (etape 7).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::json;

/// Adresse d'ecoute de repli du daemon (bind loopback, cf.
/// `tribler-daemon`) quand `configuration.json` ne publie pas de
/// `api/http_port_running`.
const DEFAULT_API: &str = "http://127.0.0.1:8085";

/// Repertoire d'etat par defaut du daemon (meme convention que
/// `tribler-daemon --state-dir`).
const DEFAULT_STATE_DIR: &str = ".tribler";

/// Timeout des appels HTTP de pilotage.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(
    name = "tribler-cli",
    about = "Pilotage du daemon Tribler-Rust via l'API REST locale"
)]
struct Cli {
    /// URL de base de l'API du daemon. Par defaut :
    /// `api/http_host`:`api/http_port_running` de
    /// `<state-dir>/configuration.json`, sinon 127.0.0.1:8085.
    #[arg(long, global = true)]
    api: Option<String>,

    /// Cle API du daemon (`X-Api-Key`). Par defaut : `api/key` de
    /// `<state-dir>/configuration.json`.
    #[arg(long, global = true)]
    api_key: Option<String>,

    /// Repertoire d'etat du daemon (contient `configuration.json`).
    #[arg(long, default_value = DEFAULT_STATE_DIR, global = true)]
    state_dir: PathBuf,

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

/// Cible resolue : URL de base + cle API.
struct ApiTarget {
    /// `http://host:port`.
    url: String,
    /// Cle API (`None` = le daemon n'en exige pas — config absente).
    key: Option<String>,
}

/// Resout l'URL et la cle : flags explicites > `configuration.json` >
/// defauts (`127.0.0.1:8085`, pas de cle).
fn resolve(cli: &Cli) -> ApiTarget {
    let file: Option<serde_json::Value> =
        std::fs::read_to_string(cli.state_dir.join("configuration.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
    let at = |p: &str| file.as_ref().and_then(|f| f.pointer(p));

    let key = cli.api_key.clone().or_else(|| {
        at("/api/key")
            .and_then(|v| v.as_str())
            .filter(|k| !k.is_empty())
            .map(String::from)
    });
    let url = cli.api.clone().unwrap_or_else(|| {
        at("/api/http_port_running")
            .and_then(|v| v.as_u64())
            .filter(|p| *p != 0)
            .map(|port| {
                let host = at("/api/http_host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1");
                format!("http://{host}:{port}")
            })
            .unwrap_or_else(|| DEFAULT_API.to_string())
    });
    ApiTarget { url, key }
}

/// Construit le client HTTP partage (en-tete `X-Api-Key` si une cle
/// est connue).
fn client(key: Option<&str>) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(k) = key {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(k) {
            headers.insert("x-api-key", v);
        }
    }
    reqwest::Client::builder()
        .default_headers(headers)
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
async fn cmd_status(client: &reqwest::Client, api: &str) -> Result<(), String> {
    let resp = client
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
async fn cmd_list(client: &reqwest::Client, api: &str) -> Result<(), String> {
    let resp = client
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
async fn cmd_add(
    client: &reqwest::Client,
    api: &str,
    source: &str,
    paused: bool,
) -> Result<(), String> {
    // Le champ JSON depend de la nature de la source : `uri` pour
    // magnet/http, `torrent` pour un chemin local (comme le Python).
    let is_uri = source.starts_with("magnet:") || source.starts_with("http");
    let mut req = json!({"paused": paused, "cli": true});
    if is_uri {
        req["uri"] = json!(source);
    } else {
        req["torrent"] = json!(source);
    }
    let resp = client
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
async fn cmd_remove(
    client: &reqwest::Client,
    api: &str,
    infohash: &str,
    remove_data: bool,
) -> Result<(), String> {
    let resp = client
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
async fn cmd_patch(
    client: &reqwest::Client,
    api: &str,
    infohash: &str,
    state: &str,
) -> Result<(), String> {
    let resp = client
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
    let target = resolve(&cli);
    let client = client(target.key.as_deref());
    let api = target.url.as_str();
    let res = match cli.cmd {
        Command::Status => cmd_status(&client, api).await,
        Command::List => cmd_list(&client, api).await,
        Command::Add { source, paused } => cmd_add(&client, api, &source, paused).await,
        Command::Remove {
            infohash,
            remove_data,
        } => cmd_remove(&client, api, &infohash, remove_data).await,
        Command::Pause { infohash } => cmd_patch(&client, api, &infohash, "stop").await,
        Command::Resume { infohash } => cmd_patch(&client, api, &infohash, "resume").await,
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
