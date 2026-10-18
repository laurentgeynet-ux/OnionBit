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
    /// Genere une clef de release Ed25519 hors-ligne (ADR-0032) —
    /// ecrit `<out>` (32 octets, droits 0600) et affiche la clef
    /// publique hex + `key_id` a coller dans `RELEASE_PUBKEY`.
    GenReleaseKey {
        /// Fichier de sortie de la clef privee (JAMAIS dans le
        /// repo — garder hors-ligne).
        #[arg(long)]
        out: PathBuf,
    },
    /// Signe un bundle en artefact `OBUP` (ADR-0032) : enumere les
    /// fichiers du payload, calcule sha256/size, produit
    /// `<bundle>/update.obup`. Hors-ligne — aucun appel daemon.
    SignRelease {
        /// Racine du bundle (contient `windows/`).
        #[arg(long)]
        dir: PathBuf,
        /// Clef privee de release (fichier 32 octets, ou `-` pour
        /// stdin — jamais en argument en clair : pas d'hex dans
        /// argv/l'historique shell).
        #[arg(long)]
        key: PathBuf,
        /// Version semver de la release (ex. "1.2.0").
        #[arg(long)]
        version: String,
        /// Cible (defaut "windows-x64").
        #[arg(long, default_value = "windows-x64")]
        target: String,
        /// Version minimale exigee pour appliquer la mise a jour.
        #[arg(long)]
        min_supported: Option<String>,
        /// Autorise le downgrade (signe — a utiliser avec parcimonie).
        #[arg(long)]
        rollback: bool,
        /// Fichier clef publique du successeur — hex ou 32 octets
        /// bruts (rotation `next_key`, ADR-0032).
        #[arg(long)]
        next_key: Option<PathBuf>,
        /// Chemin de sortie du blob (defaut `<dir>/update.obup`).
        #[arg(long)]
        out: Option<PathBuf>,
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
        // Hors-ligne : ces deux commandes ne touchent jamais le
        // daemon ni le reseau (signature de release, ADR-0032).
        Command::GenReleaseKey { out } => cmd_gen_release_key(&out),
        Command::SignRelease {
            dir,
            key,
            version,
            target,
            min_supported,
            rollback,
            next_key,
            out,
        } => cmd_sign_release(
            &dir,
            &key,
            &version,
            &target,
            min_supported,
            rollback,
            next_key,
            out,
        ),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------------
// Signature de release `OBUP` (ADR-0032, etape 142) — hors-ligne :
// la clef privee de release vit sur la machine de build, jamais
// dans le repo ni dans les logs ; ces commandes n'appellent pas le
// daemon.
// ------------------------------------------------------------------

/// Graine de clef privee lue depuis un fichier — zeroizee au drop
/// (meme discipline que `DbKey` ADR-0031 : le seul secret du
/// chantier ne doit pas trainer dans la memoire du processus).
struct ReleaseSecret([u8; 32]);

impl ReleaseSecret {
    /// Charge `<path>` (32 octets bruts de seed Ed25519) ; `-` lit
    /// stdin — la clef peut arriver par pipe sans toucher le disque
    /// ni figurer dans l'argv/l'historique shell.
    fn load(path: &std::path::Path) -> Result<Self, String> {
        let raw = if path == std::path::Path::new("-") {
            use std::io::Read;
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("lecture stdin : {e}"))?;
            buf
        } else {
            std::fs::read(path).map_err(|e| format!("clef illisible {} : {e}", path.display()))?
        };
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| format!("clef {} : 32 octets attendus", path.display()))?;
        Ok(Self(bytes))
    }

    fn signing_key(&self) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&self.0)
    }
}

impl Drop for ReleaseSecret {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

/// `gen-release-key` : produit `<out>` (32 octets, 0600 sous Unix)
/// et affiche la clef publique hex + `key_id` — la publique ira
/// remplacer `RELEASE_PUBKEY` dans `onionbit-crypto/src/release.rs`
/// (etape 146 documente le pas ; le placeholder `[0;32]` est
/// fail-closed).
fn cmd_gen_release_key(out: &std::path::Path) -> Result<(), String> {
    use rand::Rng;
    let mut seed = [0u8; 32];
    rand::rng().fill_bytes(&mut seed);
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key().to_bytes();
    std::fs::write(out, seed).map_err(|e| format!("ecriture {} : {e}", out.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o600));
    }
    use zeroize::Zeroize;
    seed.zeroize();
    println!("clef privee  : {}", out.display());
    println!("clef publique: {}", hex::encode(pk));
    println!(
        "key_id       : {:#018x}",
        onionbit_crypto::release::release_key_id(&pk)
    );
    println!(
        "→ coller la clef publique dans RELEASE_PUBKEY \
         (crates/onionbit-crypto/src/release.rs)"
    );
    Ok(())
}

/// Enumere les fichiers du payload : tout le bundle sauf `state/`,
/// `windows.old/` et les artefacts `*.obup` eux-memes — chemins
/// relatifs en `/` tries (meme arborescence → meme manifest, quelle
/// que soit la machine de build).
fn enumerate_payload(dir: &std::path::Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)
            .map_err(|e| format!("lecture {} : {e}", d.display()))?
            .flatten()
        {
            let p = e.path();
            let rel = p
                .strip_prefix(dir)
                .map_err(|_| "prefixe".to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                if rel == "state" || rel == "windows.old" {
                    continue;
                }
                stack.push(p);
            } else if p.is_file() && !rel.ends_with(".obup") {
                out.push((p, rel));
            }
        }
    }
    // Tri canonique : independant de l'ordre de read_dir (FS-
    // dependant sous Windows) — la reproductibilite du manifest en
    // depend.
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

/// `sign-release` : hash chaque fichier du payload, construit le
/// manifest, signe avec la clef chargee, ecrit `<out>` et affiche
/// la clef publique (pour la double-check contre `RELEASE_PUBKEY`).
#[allow(clippy::too_many_arguments)]
fn cmd_sign_release(
    dir: &std::path::Path,
    key: &std::path::Path,
    version: &str,
    target: &str,
    min_supported: Option<String>,
    rollback: bool,
    next_key: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(), String> {
    use sha2::Digest;

    let secret = ReleaseSecret::load(key)?;
    let sk = secret.signing_key();
    let pk = sk.verifying_key().to_bytes();
    let key_id = onionbit_crypto::release::release_key_id(&pk);

    // `next_key` accepte hex (sortie de `gen-release-key`) ou 32
    // octets bruts — la clef publique n'est pas un secret, pas de
    // contrainte de transport.
    let next_key_bytes = match next_key {
        Some(p) => {
            let raw = std::fs::read(&p)
                .map_err(|e| format!("next_key illisible {} : {e}", p.display()))?;
            let bytes = if let Ok(v) = hex::decode(String::from_utf8_lossy(&raw).trim()) {
                v
            } else {
                raw
            };
            Some(
                <[u8; 32]>::try_from(bytes.as_slice())
                    .map_err(|_| "next_key : 32 octets attendus".to_string())?,
            )
        }
        None => None,
    };

    let payload = enumerate_payload(dir)?;
    if payload.is_empty() {
        return Err(format!("aucun fichier de payload sous {}", dir.display()));
    }
    let mut files = Vec::with_capacity(payload.len());
    for (abs, rel) in payload {
        let data = std::fs::read(&abs).map_err(|e| format!("lecture {} : {e}", abs.display()))?;
        let sha: [u8; 32] = sha2::Sha256::digest(&data).into();
        files.push(onionbit_format::obupd::ObupFile {
            path: rel,
            sha256: sha,
            size: data.len() as u64,
        });
    }

    let manifest = onionbit_format::obupd::UpdateManifest {
        version: version.to_string(),
        target: target.to_string(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("horloge : {e}"))?
            .as_secs() as i64,
        files,
        min_supported,
        rollback,
        next_key: next_key_bytes,
        notes: None,
        key_id,
    };
    let enc = onionbit_format::obupd::encode_manifest(&manifest);
    let sig = onionbit_crypto::release::sign_update_manifest(&sk, &enc);
    let blob = onionbit_format::obupd::build(&manifest, key_id, &sig);

    let out = out.unwrap_or_else(|| dir.join("update.obup"));
    std::fs::write(&out, &blob).map_err(|e| format!("ecriture {} : {e}", out.display()))?;

    // La clef publique s'affiche : c'est elle qui remplace
    // RELEASE_PUBKEY — double-check visuel avant la release.
    println!("clef publique : {}", hex::encode(pk));
    println!("key_id        : {key_id:#018x}");
    println!(
        "version       : {version}  target={target}  fichiers={}",
        manifest.files.len()
    );
    println!("blob          : {} ({} octets)", out.display(), blob.len());
    // Le seed de `secret` est zeroize au drop.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_release_round_trip_verifiable() {
        // Bundle factice + clef ephemere : le blob produit doit
        // verifier via `obupd::verify` avec la clef correspondante.
        let dir = tempfile::tempdir().expect("tmp");
        let bundle = dir.path().join("bundle");
        std::fs::create_dir_all(bundle.join("windows")).expect("w");
        std::fs::write(bundle.join("windows").join("OnionBit.exe"), b"EXE1").expect("f1");
        std::fs::write(bundle.join("windows").join("daemon.exe"), b"EXE2").expect("f2");
        std::fs::create_dir_all(bundle.join("state")).expect("s");
        std::fs::write(
            bundle.join("state").join("secret.db"),
            b"state-ne-signe-pas",
        )
        .expect("st");
        let key_path = dir.path().join("rel.key");
        let seed = [0x42u8; 32];
        std::fs::write(&key_path, seed).expect("key");

        cmd_sign_release(
            &bundle,
            &key_path,
            "9.9.9",
            "windows-x64",
            None,
            false,
            None,
            None,
        )
        .expect("sign");

        let blob = std::fs::read(bundle.join("update.obup")).expect("blob");
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let pk = sk.verifying_key().to_bytes();
        let m = onionbit_format::obupd::verify(&blob, &[pk]).expect("verify");
        assert_eq!(m.version, "9.9.9");
        // state/ et le .obup lui-meme exclus ; 2 fichiers payload.
        assert_eq!(m.files.len(), 2);
        assert!(m
            .files
            .iter()
            .all(|f| !f.path.starts_with("state/") && !f.path.ends_with(".obup")));
        assert!(m.files.iter().any(|f| f.path == "windows/OnionBit.exe"));
    }

    #[test]
    fn enumeration_deterministe_et_normalisee() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("b").join("c")).expect("mk");
        std::fs::write(dir.path().join("a.txt"), b"1").expect("w");
        std::fs::write(dir.path().join("b").join("c").join("z"), b"2").expect("w");
        std::fs::write(dir.path().join("b").join("y"), b"3").expect("w");
        let files = enumerate_payload(dir.path()).expect("enum");
        let rels: Vec<_> = files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(rels, vec!["a.txt", "b/c/z", "b/y"], "trie et '/'");
    }

    #[test]
    fn clef_privee_zeroizee() {
        // Le wrapper zeroize la graine au drop — verifie par
        // construction (Drop implemente Zeroize), smoke seulement.
        let dir = tempfile::tempdir().expect("tmp");
        let kp = dir.path().join("k");
        std::fs::write(&kp, [9u8; 32]).expect("w");
        let s = ReleaseSecret::load(&kp).expect("load");
        drop(s);
        // Pas d'acces aux octets post-drop — le test est la forme.
    }

    #[test]
    fn le_squelette_compile() {
        // Les tests d'integration bout en bout (CLI -> API -> session)
        // arrivent avec onionbit-daemon (etape 8) ; les handlers sont
        // couverts par crates/onionbit-api/tests/api.rs.
    }
}
