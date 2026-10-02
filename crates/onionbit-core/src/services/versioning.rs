// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sonde de mise a jour — equivalent de `VersioningManager.check_version`
//! (`tribler.core.versioning.manager`).
//!
//! [`probe_urls`] construit la liste ordonnee des URL (config
//! `versioning/*`) : sondes additionnelles `check_urls` puis l'API
//! GitHub des releases du depot `github_repo` — en tete quand
//! `allow_pre` (`releases?per_page=1`, pre-versions incluses), en
//! queue sinon (`releases/latest`, stables uniquement).
//! [`check_new_version`] passe chaque sonde par
//! [`super::fetch_checked_with`] : anti-SSRF `ip_policy`, timeout
//! borne, `User-Agent` exige par l'API GitHub. Trafic direct,
//! jamais par les circuits onion (sonde identifiante par nature).

use std::time::Duration;

use onionbit_network_policy::IpPolicy;
use serde_json::Value;

use crate::daemon_config::VersioningConfig;

/// `User-Agent` des sondes (equivalent du `Tribler/{v} (...)` Python ;
/// l'API GitHub refuse les requetes sans UA).
pub fn probe_user_agent(current: &str) -> String {
    format!(
        "OnionBit/{current} (os={}; arch={})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// URLs sondees dans l'ordre — port de `check_version` Python :
/// `check_urls` (`{current}` substitue par la version courante,
/// comme `releases/latest?current=` cote tribler.org) puis l'API
/// GitHub du depot — inseree en tete quand `allow_pre`, ajoutee en
/// queue sinon. `github_repo` vide = pas de sonde GitHub ; liste
/// vide = pas de trafic sortant.
pub fn probe_urls(cfg: &VersioningConfig, current: &str) -> Vec<String> {
    let mut urls: Vec<String> = cfg
        .check_urls
        .iter()
        .map(|u| u.replace("{current}", current))
        .collect();
    if !cfg.github_repo.is_empty() {
        let url = format!(
            "https://api.github.com/repos/{}/releases{}",
            cfg.github_repo,
            if cfg.allow_pre {
                "?per_page=1&page=1"
            } else {
                "/latest"
            }
        );
        if cfg.allow_pre {
            urls.insert(0, url);
        } else {
            urls.push(url);
        }
    }
    urls
}

/// Sonde `urls` dans l'ordre et renvoie la version distante si elle
/// est strictement plus recente que `current`, `None` sinon.
///
/// Semantique Python : echec de sonde ou reponse sans nom -> sonde
/// suivante ; reponse <= `current` -> arret (deja a jour).
pub async fn check_new_version(
    current: &str,
    urls: &[String],
    timeout: Duration,
    ip_policy: &IpPolicy,
) -> Option<String> {
    let ua = probe_user_agent(current);
    for url in urls {
        let resp = match super::fetch_checked_with(url, ip_policy, timeout, Some(&ua)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::info!(url = %url, error = %e, "sonde de version echouee");
                continue;
            }
        };
        if !resp.status().is_success() {
            tracing::info!(url = %url, status = %resp.status(), "sonde de version en echec");
            continue;
        }
        let Ok(body) = super::read_body_limited(resp).await else {
            continue;
        };
        let Ok(json) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        let Some(name) = release_name(&json) else {
            continue;
        };
        // `removeprefix("v")` Python : un seul prefixe retire.
        let trimmed = name.trim();
        let distant = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        if version_newer(distant, current) {
            return Some(distant.to_owned());
        }
        break;
    }
    None
}

/// Nom de version d'une reponse de sonde : `name` (ou `tag_name` en
/// repli) d'un objet — `releases/latest`, sonde custom — ou du
/// premier element d'un tableau — `releases?per_page=1`.
///
/// Divergence documentee : `response_dict["name"]` Python leve
/// `TypeError` sur un tableau JSON — la sonde GitHub `allow_pre` de
/// Tribler echoue donc toujours ; ici le tableau est accepte
/// (`docs/reference_tribler/api_endpoints_complet.md`).
fn release_name(json: &Value) -> Option<&str> {
    let obj = match json {
        Value::Array(a) => a.first()?,
        other => other,
    };
    // `name` peut etre `null` cote GitHub -> repli sur `tag_name`.
    obj.get("name")
        .and_then(Value::as_str)
        .or_else(|| obj.get("tag_name").and_then(Value::as_str))
        .filter(|s| !s.trim().is_empty())
}

/// `candidate` strictement superieure a `current` au sens
/// `packaging.Version` (sous-ensemble : segments numeriques, puis
/// suffixe `dev` < `a`/`alpha` < `b`/`beta` < `rc`/`c`/`pre` <
/// release < `post`).
pub fn version_newer(candidate: &str, current: &str) -> bool {
    version_key(candidate) > version_key(current)
}

/// Cle de tri `(segments numeriques, rang, numero)` d'une version ;
/// les zeros terminaux sont tronques (`8.2` == `8.2.0`).
fn version_key(v: &str) -> (Vec<u64>, u8, u64) {
    let t = v.trim();
    let v = t
        .strip_prefix('v')
        .or_else(|| t.strip_prefix('V'))
        .unwrap_or(t);
    let num_len = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let (num_part, suffix) = v.split_at(num_len);
    let mut nums: Vec<u64> = num_part
        .split('.')
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    while nums.last() == Some(&0) {
        nums.pop();
    }
    let (rank, n) = prerelease_key(suffix);
    (nums, rank, n)
}

/// Rang + numero du suffixe pre/post-release (`4` = stable).
fn prerelease_key(suffix: &str) -> (u8, u64) {
    let s = suffix
        .trim_start_matches(['-', '_', '.'])
        .to_ascii_lowercase();
    if s.is_empty() {
        return (4, 0);
    }
    const RANKS: &[(&str, u8)] = &[
        ("dev", 0),
        ("alpha", 1),
        ("a", 1),
        ("beta", 2),
        ("b", 2),
        ("preview", 3),
        ("pre", 3),
        ("rc", 3),
        ("c", 3),
        ("post", 5),
        ("rev", 5),
    ];
    for (prefix, rank) in RANKS {
        if let Some(rest) = s.strip_prefix(prefix) {
            let n = rest
                .trim_start_matches(['-', '_', '.'])
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
                .unwrap_or(0);
            return (*rank, n);
        }
    }
    // Suffixe inconnu : assimile a la release (ni pre ni post).
    (4, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_config::VersioningConfig;

    fn cfg(allow_pre: bool, repo: &str, check_urls: &[&str]) -> VersioningConfig {
        VersioningConfig {
            allow_pre,
            github_repo: repo.to_owned(),
            check_urls: check_urls.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn probe_urls_ordre_python() {
        // Stable : sondes custom d'abord, `releases/latest` en queue.
        let u = probe_urls(
            &cfg(false, "o/r", &["https://x/releases?current={current}"]),
            "1.0.0",
        );
        assert_eq!(
            u,
            [
                "https://x/releases?current=1.0.0",
                "https://api.github.com/repos/o/r/releases/latest",
            ]
        );
        // allow_pre : liste GitHub (pre-versions) inseree en tete.
        let u = probe_urls(&cfg(true, "o/r", &["https://x/r"]), "1.0.0");
        assert_eq!(
            u,
            [
                "https://api.github.com/repos/o/r/releases?per_page=1&page=1",
                "https://x/r",
            ]
        );
        // Repo vide : aucune sonde GitHub ; tout vide : pas de trafic.
        assert!(probe_urls(&cfg(false, "", &[]), "1.0.0").is_empty());
        assert_eq!(
            probe_urls(&cfg(false, "", &["https://x/r"]), "1.0.0"),
            ["https://x/r"]
        );
    }

    #[test]
    fn version_newer_semantique_pep440() {
        assert!(version_newer("8.2.1", "8.2.0"));
        assert!(version_newer("8.10.0", "8.2.9"));
        assert!(version_newer("v9.0.0", "9.0.0rc1"));
        assert!(version_newer("8.2.0", "8.2.0b2"));
        assert!(version_newer("8.2.0.post1", "8.2.0"));
        assert!(version_newer("8.2.0rc2", "8.2.0rc1"));
        assert!(!version_newer("8.2.0rc1", "8.2.0"));
        assert!(!version_newer("8.2.0", "8.2.0"));
        assert!(!version_newer("8.2", "8.2.0"));
        assert!(!version_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn release_name_objet_liste_repli() {
        assert_eq!(
            release_name(&serde_json::json!({"name": "v1.2.3"})),
            Some("v1.2.3")
        );
        // Tableau `releases?per_page=1` accepte (cf. divergence Python).
        assert_eq!(
            release_name(&serde_json::json!([{"name": "v2.0.0"}])),
            Some("v2.0.0")
        );
        assert_eq!(
            release_name(&serde_json::json!({"name": null, "tag_name": "v3.0.0"})),
            Some("v3.0.0")
        );
        assert_eq!(release_name(&serde_json::json!({"name": null})), None);
        assert_eq!(release_name(&serde_json::json!([])), None);
    }

    /// Sonde HTTP factice sur loopback : prouve le chemin complet
    /// `fetch_checked_with` + parse + comparaison (politique
    /// permissive de test — `CoreConfig::offline`).
    #[tokio::test]
    async fn check_new_version_sonde_loopback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Lit la requete (fin des en-tetes) avant de repondre :
            // fermer avec des donnees reçues non lues enverrait RST.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = r#"{"name": "v99.0.0"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
        });
        let urls = vec![format!("http://127.0.0.1:{port}/releases")];
        let v = check_new_version(
            "1.0.0",
            &urls,
            Duration::from_secs(5),
            &IpPolicy::permissive(),
        )
        .await;
        assert_eq!(v.as_deref(), Some("99.0.0"));
    }

    /// La sonde reste soumise a `ip_policy` : loopback refuse en
    /// politique stricte (anti-SSRF — pas de trafic vers une cible
    /// controlee par la config).
    #[tokio::test]
    async fn check_new_version_refuse_loopback_en_strict() {
        let urls = vec!["http://127.0.0.1:1/releases".to_string()];
        assert!(
            check_new_version("1.0.0", &urls, Duration::from_secs(5), &IpPolicy::strict())
                .await
                .is_none()
        );
    }
}
