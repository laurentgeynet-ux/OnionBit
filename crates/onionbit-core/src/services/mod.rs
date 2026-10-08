// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Services secondaires du daemon (etape 14 — equivalents de
//! `tribler.core.*` : `watch_folder`, `rss`, `torrent_checker`,
//! `content_discovery` — ce dernier vit cote overlay dans
//! `onionbit-ipv8::content_discovery`).

pub mod bandwidth;
pub mod messaging;
pub mod rss;
pub mod torrent_checker;
pub mod versioning;
pub mod watch_folder;

use crate::error::{CoreError, Result};

/// Timeout par defaut des requetes HTTP de services (RSS, trackers).
pub const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Taille max d'un corps de reponse HTTP de service (borne anti
/// abus : ni les flux RSS ni les scrapes ne depassent quelques Mo).
pub const HTTP_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// `fetch` anti-SSRF : resout l'hote, applique `ip_policy` a CHAQUE
/// adresse resolue (refus ferme), puis renvoie la `reqwest::Response`
/// (status/headers pour le GET conditionnel RSS, corps borne via
/// [`read_body_limited`]).
///
/// Utilise pour toute URL controlee par un tiers (flux RSS, liens
/// `.torrent`, trackers HTTP).
pub async fn fetch_checked(
    url: &str,
    ip_policy: &onionbit_network_policy::IpPolicy,
) -> Result<reqwest::Response> {
    fetch_checked_with(url, ip_policy, HTTP_TIMEOUT, None).await
}

/// Variante de [`fetch_checked`] a timeout et `User-Agent`
/// parametrables (sonde de version : `ClientTimeout(total=5)` Python,
/// UA exige par l'API GitHub).
pub async fn fetch_checked_with(
    url: &str,
    ip_policy: &onionbit_network_policy::IpPolicy,
    timeout: std::time::Duration,
    user_agent: Option<&str>,
) -> Result<reqwest::Response> {
    let parsed =
        url::Url::parse(url).map_err(|_| CoreError::InvalidState("url invalide pour le fetch"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(CoreError::InvalidState("schema http(s) attendu"));
    }
    let host = parsed
        .host_str()
        .ok_or(CoreError::InvalidState("url sans hote"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or(CoreError::InvalidState("url sans port"))?;
    let mut addrs = Vec::new();
    for addr in tokio::net::lookup_host((host, port)).await? {
        ip_policy.check(&addr)?;
        addrs.push(addr);
    }
    if addrs.is_empty() {
        return Err(CoreError::InvalidState("hote sans adresse resolue"));
    }
    // Anti-DNS-rebinding : reqwest est epingle aux seules adresses
    // deja validees par `ip_policy` — `resolve_to_addrs` court-
    // circuite le resolveur systeme pour cet hote, donc un domaine
    // a TTL 0 ne peut pas re-resoudre vers une IP interne entre le
    // check et la connexion. Le client est reconstruit a chaque
    // fetch parce que l'epinglage est par hote : il ne peut pas etre
    // mutualise (chemins peu frequents : RSS, trackers, version).
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addrs)
        .build()
        .map_err(|_| CoreError::InvalidState("client http indisponible"))?;
    let mut request = client.get(url);
    if let Some(ua) = user_agent {
        request = request.header(reqwest::header::USER_AGENT, ua);
    }
    request
        .send()
        .await
        .map_err(|_| CoreError::InvalidState("requete http echouee"))
}

/// Lit un corps de reponse borne a `HTTP_BODY_LIMIT` octets.
/// `content_length` est absent en `Transfer-Encoding: chunked` —
/// l'accumulation est bornee quoi qu'il arrive (un serveur distant
/// ne peut pas gonfler la reponse au-dela du plafond).
pub async fn read_body_limited(mut resp: reqwest::Response) -> Result<bytes::Bytes> {
    if resp
        .content_length()
        .is_some_and(|l| l as usize > HTTP_BODY_LIMIT)
    {
        return Err(CoreError::InvalidState("corps http trop volumineux"));
    }
    let mut body = bytes::BytesMut::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|_| CoreError::InvalidState("lecture du corps http impossible"))?
    {
        if body.len() + chunk.len() > HTTP_BODY_LIMIT {
            return Err(CoreError::InvalidState("corps http trop volumineux"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

/// Extrait les valeurs textuelles d'un flux XML (equivalent de
/// `ET.parse + tree.iter() text` du `parse_rss` Python : toute valeur
/// de texte est candidate — le filtre `.torrent`/magnet est applique
/// par l'appelant).
pub fn xml_text_values(content: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(content);
    let mut out = Vec::new();
    for seg in text.split('<') {
        if let Some((_, value)) = seg.split_once('>') {
            let v = value.trim();
            if !v.is_empty() {
                out.push(v.to_string());
            }
        }
    }
    out
}
