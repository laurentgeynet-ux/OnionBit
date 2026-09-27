//! Services secondaires du daemon (etape 14 — equivalents de
//! `tribler.core.*` : `watch_folder`, `rss`, `torrent_checker`,
//! `content_discovery` — ce dernier vit cote overlay dans
//! `tribler-ipv8::content_discovery`).

pub mod rss;
pub mod torrent_checker;
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
    ip_policy: &tribler_network_policy::IpPolicy,
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
    let mut count = 0usize;
    for addr in tokio::net::lookup_host((host, port)).await? {
        ip_policy.check(&addr)?;
        count += 1;
    }
    if count == 0 {
        return Err(CoreError::InvalidState("hote sans adresse resolue"));
    }
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| CoreError::InvalidState("client http indisponible"))?;
    client
        .get(url)
        .send()
        .await
        .map_err(|_| CoreError::InvalidState("requete http echouee"))
}

/// Lit un corps de reponse borne a `HTTP_BODY_LIMIT` octets.
pub async fn read_body_limited(resp: reqwest::Response) -> Result<bytes::Bytes> {
    if resp
        .content_length()
        .is_some_and(|l| l as usize > HTTP_BODY_LIMIT)
    {
        return Err(CoreError::InvalidState("corps http trop volumineux"));
    }
    resp.bytes()
        .await
        .map_err(|_| CoreError::InvalidState("lecture du corps http impossible"))
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
