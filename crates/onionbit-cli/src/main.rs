// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `onionbit-cli` — outil de pilotage en ligne de commande.
//!
//! Client du plan de controle expose par `onionbit-daemon` via
//! `onionbit-api` (REST/SSE sur `127.0.0.1`). Ne contient aucune
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
/// `onionbit-daemon`) quand `configuration.json` ne publie pas de
/// `api/http_port_running`.
const DEFAULT_API: &str = "http://127.0.0.1:8085";

/// Repertoire d'etat par defaut du daemon (meme convention que
/// `onionbit-daemon --state-dir`).
const DEFAULT_STATE_DIR: &str = ".onionbit";

/// Timeout des appels HTTP de pilotage.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(
    name = "onionbit-cli",
    about = "Pilotage du daemon OnionBit via l'API REST locale"
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
    /// Priorite : `--state-dir` > `ONIONBIT_STATE_DIR` > `.onionbit`
    /// (l'env sert surtout au conteneur Docker — `ENV
    /// ONIONBIT_STATE_DIR=/data/state`, ADR-0024).
    #[arg(long, env = "ONIONBIT_STATE_DIR", default_value = DEFAULT_STATE_DIR, global = true)]
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
    /// Circuits et guards tunnel (GET /api/ipv8/tunnel/*).
    Tunnel {
        /// `circuits` (defaut), `guards` (ADR-0010), `relays`,
        /// `exits`, `swarms`, `peers`, `downloads` (circuits par
        /// telechargement anonyme, extension Rust).
        #[arg(long, default_value = "circuits")]
        show: String,
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

/// `tunnel` — circuits/guards/relays/exits/swarms/peers/downloads.
async fn cmd_tunnel(client: &reqwest::Client, api: &str, show: &str) -> Result<(), String> {
    let path = match show {
        "circuits" | "guards" | "relays" | "exits" | "swarms" | "peers" => {
            format!("{api}/api/ipv8/tunnel/{show}")
        }
        "downloads" => format!("{api}/api/ipv8/tunnel/debug/circuit-downloads"),
        other => {
            return Err(format!(
                "vue inconnue « {other} » — attendu : circuits|guards|relays|exits|swarms|peers|downloads"
            ))
        }
    };
    let resp = client
        .get(&path)
        .send()
        .await
        .map_err(|e| format!("daemon injoignable sur {api} ({e})"))?;
    let body = ensure_ok(resp).await?;
    match show {
        "circuits" => {
            let items = body["circuits"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("(aucun circuit)");
                return Ok(());
            }
            println!(
                "{:<10}  {:<9}  {:>4}/{:<4}  {:>9}  {:>9}  SAUTS",
                "ID", "ETAT", "HOPS", "GOAL", "UP", "DOWN"
            );
            for c in items {
                let hops = c["verified_hops"]
                    .as_array()
                    .map(|h| {
                        h.iter()
                            .filter_map(|v| v.as_str().map(|s| s[..8.min(s.len())].to_string()))
                            .collect::<Vec<_>>()
                            .join(">")
                    })
                    .unwrap_or_default();
                println!(
                    "{:<10}  {:<9}  {:>4}/{:<4}  {:>9}  {:>9}  {}",
                    c["circuit_id"].as_u64().unwrap_or(0),
                    c["state"].as_str().unwrap_or("?"),
                    c["actual_hops"].as_u64().unwrap_or(0),
                    c["goal_hops"].as_u64().unwrap_or(0),
                    c["bytes_up"].as_u64().unwrap_or(0),
                    c["bytes_down"].as_u64().unwrap_or(0),
                    hops,
                );
            }
        }
        "guards" => {
            let items = body["guards"].as_array().cloned().unwrap_or_default();
            let enabled = body["enabled"].as_bool().unwrap_or(false);
            println!("guards_enabled = {enabled}");
            if items.is_empty() {
                println!("(aucun guard adopte)");
                return Ok(());
            }
            println!(
                "{:<42}  {:<22}  {:<7}  {:>7}  ADOPTE",
                "MID", "ADRESSE", "ROLE", "ECHECS"
            );
            for g in items {
                println!(
                    "{:<42}  {:<22}  {:<7}  {:>7}  {}",
                    g["mid"].as_str().unwrap_or("?"),
                    g["address"].as_str().unwrap_or(""),
                    if g["reserve"].as_bool().unwrap_or(false) {
                        "reserve"
                    } else {
                        "actif"
                    },
                    g["failures"].as_u64().unwrap_or(0),
                    g["adopted_at"].as_u64().unwrap_or(0),
                );
            }
        }
        "downloads" => {
            let items = body["downloads"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("(aucun download anonyme)");
                return Ok(());
            }
            println!(
                "{:<42}  {:<14}  {:>4}  {:<6}  {:>5}  CIRCUITS",
                "INFOHASH", "ETAT", "HOPS", "SEEDER", "PAIRS"
            );
            for d in items {
                let cids = d["circuits"]
                    .as_array()
                    .map(|cs| {
                        cs.iter()
                            .filter_map(|c| c["circuit_id"].as_u64().map(|id| id.to_string()))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                println!(
                    "{:<42}  {:<14}  {:>4}  {:<6}  {:>5}  {}",
                    d["info_hash"].as_str().unwrap_or("?"),
                    d["state"].as_str().unwrap_or("?"),
                    d["hops"].as_u64().unwrap_or(0),
                    d["seeder"].as_bool().unwrap_or(false),
                    d["swarm_peers"].as_u64().unwrap_or(0),
                    cids,
                );
            }
        }
        other => {
            let key = match other {
                "relays" => "relays",
                "exits" => "exits",
                "swarms" => "swarms",
                _ => "peers",
            };
            let items = body[key].as_array().cloned().unwrap_or_default();
            println!(
                "{}",
                serde_json::to_string_pretty(&items).unwrap_or_default()
            );
        }
    }
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
        Command::Tunnel { show } => cmd_tunnel(&client, api, &show).await,
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
        // arrivent avec onionbit-daemon (etape 8) ; les handlers sont
        // couverts par crates/onionbit-api/tests/api.rs.
    }
}
