// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Echange HTTP/1.1 borne sur un flux Tor (ADR-0028 §1).
//!
//! Le decodeur est [`httparse`] (zero-allocation, eprouve) — jamais
//! de parseur maison. Le corps est borne par `max_response_bytes`
//! quelle que soit l'annonce du serveur (`Content-Length`,
//! `Transfer-Encoding: chunked` ou fermeture — on envoie toujours
//! `Connection: close`, donc la fin de reponse est marquee par EOF).
//!
//! Ce module n'ouvre **aucune** connexion : il parle sur un flux
//! generique `AsyncRead + AsyncWrite` fourni par le transport
//! (`DataStream` arti ou `Socks5Stream` externe), ce qui permet de
//! tester tout le decodage sur `tokio::io::duplex` sans reseau.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::error::{Result, TorError};

/// Plafond interne de la zone en-tetes (16 Kio) — limite defensive
/// d'implementation, pas un reglage : aucun site legitime n'emet des
/// en-tetes plus grands.
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Marge de lecture brute au-dela du corps borne : les lignes de
/// taille `chunked` (\r\n + taille hex + trailer) gonflent le flux
/// brut sans compter dans le corps decode.
const CHUNK_OVERHEAD: usize = 64 * 1024;

/// Nombre maximal d'en-tetes parsees par reponse.
const MAX_HEADERS: usize = 64;

/// Cible HTTP resolue depuis une URL `http://<host>.onion/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnionTarget {
    /// Hote `.onion` en minuscules (tel que parse par `url`).
    pub host: String,
    /// Port effectif (defaut scheme : 80/443).
    pub port: u16,
    /// Chemin + query, jamais vide (`/` au minimum).
    pub path: String,
    /// `https` — TLS par-dessus le flux Tor (`api_via_exit` :
    /// integrite des metadonnees face a un exit malveillant).
    pub tls: bool,
}

/// Reponse HTTP decodee et bornee.
#[derive(Debug)]
pub struct OnionResponse {
    /// Code de statut HTTP.
    pub status: u16,
    /// En-tetes `(nom, valeur)` — texte lossy, non fiables.
    pub headers: Vec<(String, String)>,
    /// Corps decode (de-chunke si besoin), ≤ `max_response_bytes`.
    pub body: Vec<u8>,
}

impl OnionResponse {
    /// `true` sur les redirections 30x avec corps de navigation.
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    /// Valeur `Location` d'une redirection, si presente.
    pub fn location(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("location"))
            .map(|(_, v)| v.trim())
    }
}

/// Parse une URL de cible onion. v1 accepte `http://` seulement :
/// le transport service-cache est deja chiffre et authentifie, le
/// TLS-par-dessus est hors scope (ADR-0028 §1). Rejette auth et
/// identifiants embarques — la politique `.onion`/allowlist est
/// verifiee ensuite par l'appelant ([`TorTargetPolicy::check`]).
///
/// [`TorTargetPolicy::check`]: onionbit_network_policy::TorTargetPolicy::check
pub(crate) fn parse_target(raw: &str) -> Result<OnionTarget> {
    let u = url::Url::parse(raw).map_err(|e| TorError::InvalidUrl(e.to_string()))?;
    let tls = match u.scheme() {
        "http" => false,
        "https" => true,
        other => {
            return Err(TorError::InvalidUrl(format!(
                "scheme `{other}` non supporte (http(s):// seulement)"
            )))
        }
    };
    if !u.username().is_empty() || u.password().is_some() {
        return Err(TorError::InvalidUrl(
            "identifiants dans l'URL refuses".into(),
        ));
    }
    let host = u
        .host_str()
        .ok_or_else(|| TorError::InvalidUrl("hote absent".into()))?
        .to_ascii_lowercase();
    let port = u
        .port_or_known_default()
        .ok_or_else(|| TorError::InvalidUrl("port absent".into()))?;
    let mut path = u.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(q) = u.query() {
        path.push('?');
        path.push_str(q);
    }
    Ok(OnionTarget {
        host,
        port,
        path,
        tls,
    })
}

/// Resout une redirection `Location` (relative ou absolue) par
/// rapport a l'URL courante. La cible resultante repassera par
/// `parse_target` + `TorTargetPolicy::check` a l'iteration suivante.
pub(crate) fn resolve_redirect(current: &str, resp: &OnionResponse) -> Result<String> {
    let loc = resp
        .location()
        .ok_or_else(|| TorError::MalformedResponse("30x sans Location".into()))?;
    let base = url::Url::parse(current).map_err(|e| TorError::InvalidUrl(e.to_string()))?;
    let next = base
        .join(loc)
        .map_err(|e| TorError::RedirectDenied(format!("Location injoignable : {e}")))?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(TorError::RedirectDenied(format!(
            "scheme `{}` non supporte",
            next.scheme()
        )));
    }
    Ok(next.into())
}

/// Requete `GET` minimale : pas de cookies, pas de referer —
/// client de fetch borne, pas un navigateur (ADR-0028 §Securite).
pub(crate) fn build_request(target: &OnionTarget) -> Vec<u8> {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        target.path, target.host
    )
    .into_bytes()
}

/// Enrobe un flux Tor dans une session TLS cliente — hotes
/// clearnet de sortie `api_via_exit` (https) et services onion
/// dument certifies. `webpki-roots` (bundle Mozilla) fige le trust
/// store : deterministe, independant de l'OS. Pas d'ALPN annonce :
/// le serveur reste en HTTP/1.1, seul profil supporte par
/// [`exchange`].
#[cfg(feature = "tor")]
pub(crate) async fn wrap_tls(
    stream: Box<dyn crate::runtime::OnionIo>,
    host: &str,
) -> Result<Box<dyn crate::runtime::OnionIo>> {
    use std::sync::OnceLock;
    use tokio_rustls::TlsConnector;

    static CONNECTOR: OnceLock<TlsConnector> = OnceLock::new();
    let connector = CONNECTOR.get_or_init(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let cfg = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions TLS par defaut")
        .with_root_certificates(roots)
        .with_no_client_auth();
        TlsConnector::from(std::sync::Arc::new(cfg))
    });
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| TorError::InvalidUrl(format!("SNI invalide : {e}")))?;
    let tls = connector
        .connect(name, stream)
        .await
        .map_err(|e| TorError::Request(format!("handshake TLS : {e}")))?;
    Ok(Box::new(tls))
}

/// Envoie `request` et lit une reponse bornee — [`OnionResponse`]
/// quelle que soit la forme de corps annoncee par le serveur.
pub(crate) async fn exchange<S>(
    mut stream: S,
    request: &[u8],
    max_body: usize,
    request_timeout: Duration,
) -> Result<OnionResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let work = async move {
        stream
            .write_all(request)
            .await
            .map_err(|e| TorError::Request(e.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|e| TorError::Request(e.to_string()))?;

        // Lecture brute bornee : en-tetes + corps + marge chunked.
        // `Connection: close` => la reponse se termine par EOF ; on
        // arrete plus tot des que le corps annonce est complet.
        let raw_cap = MAX_HEADER_BYTES + max_body + CHUNK_OVERHEAD;
        let mut buf = Vec::with_capacity(raw_cap.min(64 * 1024));
        let mut chunk = [0u8; 8192];
        let mut head: Option<ParsedHead> = None;
        loop {
            if let Some(h) = &head {
                if h.body_complete(&buf[h.offset..], max_body)? {
                    break;
                }
            } else if buf.len() >= MAX_HEADER_BYTES {
                return Err(TorError::MalformedResponse(
                    "en-tetes au-dela de la limite".into(),
                ));
            }
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|e| TorError::Request(e.to_string()))?;
            if n == 0 {
                break; // EOF — corps ferme par la connexion.
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > raw_cap {
                return Err(TorError::TooLarge { limit: max_body });
            }
            if head.is_none() {
                head = parse_head(&buf)?;
            }
        }
        let head = head
            .ok_or_else(|| TorError::MalformedResponse("flux ferme avant les en-tetes".into()))?;
        head.finish(buf[head.offset..].to_vec(), max_body)
    };
    timeout(request_timeout, work)
        .await
        .map_err(|_| TorError::Timeout)?
}

/// En-tetes de reponse parses une fois `httparse` complet.
struct ParsedHead {
    /// Decalage du premier octet du corps dans le buffer brut.
    offset: usize,
    status: u16,
    headers: Vec<(String, String)>,
    chunked: bool,
    content_length: Option<usize>,
}

impl ParsedHead {
    /// `true` si le corps est complet dans `body_buf` (evite
    /// d'attendre EOF pour une reponse `Content-Length` ou chunked
    /// entierement recue).
    fn body_complete(&self, body_buf: &[u8], max_body: usize) -> Result<bool> {
        if self.chunked {
            return Ok(decode_chunked(body_buf, max_body)?.is_some());
        }
        if let Some(n) = self.content_length {
            return Ok(body_buf.len() >= n);
        }
        Ok(false) // corps delimite par la fermeture — attendre EOF.
    }

    /// Produit la reponse finale a partir du corps brut accumule.
    fn finish(&self, body_buf: Vec<u8>, max_body: usize) -> Result<OnionResponse> {
        let body = if self.chunked {
            match decode_chunked(&body_buf, max_body)? {
                Some((body, _)) => body,
                None => return Err(TorError::MalformedResponse("corps chunked tronque".into())),
            }
        } else if let Some(n) = self.content_length {
            if n > max_body {
                return Err(TorError::TooLarge { limit: max_body });
            }
            if body_buf.len() < n {
                return Err(TorError::MalformedResponse(
                    "corps tronque vs Content-Length".into(),
                ));
            }
            body_buf[..n].to_vec()
        } else {
            if body_buf.len() > max_body {
                return Err(TorError::TooLarge { limit: max_body });
            }
            body_buf
        };
        Ok(OnionResponse {
            status: self.status,
            headers: self.headers.clone(),
            body,
        })
    }
}

/// Parse la zone en-tetes avec `httparse` — `None` tant que la
/// reponse est partielle, erreur si la ligne de statut/en-tetes est
/// indecodable (httparse accepte CRLF *et* LF seul).
fn parse_head(buf: &[u8]) -> Result<Option<ParsedHead>> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut resp = httparse::Response::new(&mut headers);
    match resp.parse(buf) {
        Ok(httparse::Status::Complete(offset)) => {
            let headers: Vec<(String, String)> = resp
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_string(),
                        String::from_utf8_lossy(h.value).into_owned(),
                    )
                })
                .collect();
            let chunked = headers.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("transfer-encoding")
                    && v.to_ascii_lowercase().contains("chunked")
            });
            let content_length = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.trim().parse::<usize>().ok());
            Ok(Some(ParsedHead {
                offset,
                status: resp.code.unwrap_or(0),
                headers,
                chunked,
                content_length,
            }))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(e) => Err(TorError::MalformedResponse(e.to_string())),
    }
}

/// Decode un corps `Transfer-Encoding: chunked` : `Some((corps,
/// octets_consommes))` si complet, `None` si donnees partielles.
/// Le corps decode est borne a `max_body` (au-dela : `TooLarge`).
fn decode_chunked(buf: &[u8], max_body: usize) -> Result<Option<(Vec<u8>, usize)>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        // Ligne de taille : hex [+ extensions `;...`] terminee CRLF/LF.
        let line_end = match find_line_end(&buf[pos..]) {
            Some(e) => pos + e,
            None => return Ok(None), // ligne incomplete — il faut plus.
        };
        let line = std::str::from_utf8(&buf[pos..line_end])
            .map_err(|_| TorError::MalformedResponse("taille chunk non-UTF8".into()))?;
        let size_hex = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| {
            TorError::MalformedResponse(format!("taille chunk invalide `{size_hex}`"))
        })?;
        pos = line_end + 1; // apres le \n.
        if size == 0 {
            // Trailer : lignes jusqu'a une ligne vide — ignorees.
            loop {
                match find_line_end(&buf[pos..]) {
                    Some(e) => {
                        let stripped = if e > 0 && buf[pos + e - 1] == b'\r' {
                            e - 1
                        } else {
                            e
                        };
                        if stripped == 0 {
                            return Ok(Some((out, pos + e + 1)));
                        }
                        pos += e + 1;
                    }
                    None => return Ok(None),
                }
            }
        }
        if pos + size > buf.len() {
            return Ok(None); // donnees de chunk incompletes.
        }
        out.extend_from_slice(&buf[pos..pos + size]);
        if out.len() > max_body {
            return Err(TorError::TooLarge { limit: max_body });
        }
        pos += size;
        // CRLF/LF terminant le chunk.
        match buf.get(pos) {
            Some(b'\r') if buf.get(pos + 1) == Some(&b'\n') => pos += 2,
            Some(b'\n') => pos += 1,
            _ => {
                return Err(TorError::MalformedResponse(
                    "chunk sans fin de ligne".into(),
                ))
            }
        }
    }
}

/// Offset de la fin de la premiere ligne (position du `\n`) —
/// `None` si incomplete.
fn find_line_end(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ecrit `response` cote serveur et rend le corps client decode.
    async fn roundtrip(response: &[u8], max_body: usize) -> Result<OnionResponse> {
        let (mut server, client) = tokio::io::duplex(64 * 1024);
        let resp = response.to_vec();
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            server.write_all(&resp).await.unwrap();
            server.shutdown().await.unwrap();
        });
        let target = parse_target("http://example.onion/x").unwrap();
        let req = build_request(&target);
        let out = exchange(client, &req, max_body, Duration::from_secs(5)).await;
        writer.await.unwrap();
        out
    }

    #[tokio::test]
    async fn content_length_decode() {
        let r = roundtrip(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA",
            1024,
        )
        .await
        .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"hello");
    }

    #[tokio::test]
    async fn chunked_decode_et_borne() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let r = roundtrip(raw, 1024).await.unwrap();
        assert_eq!(r.body, b"hello world");
        // Corps decode au-dela de la borne -> TooLarge.
        assert!(matches!(
            roundtrip(raw, 4).await,
            Err(TorError::TooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn corps_delimite_par_eof() {
        let r = roundtrip(b"HTTP/1.1 200 OK\r\n\r\npas de longueur", 1024)
            .await
            .unwrap();
        assert_eq!(r.body, b"pas de longueur");
    }

    #[tokio::test]
    async fn redirection_30x_et_location() {
        let r = roundtrip(
            b"HTTP/1.1 302 Found\r\nLocation: /moved\r\nContent-Length: 0\r\n\r\n",
            1024,
        )
        .await
        .unwrap();
        assert!(r.is_redirect());
        assert_eq!(r.location(), Some("/moved"));
        let next = resolve_redirect("http://a.onion/p?q=1", &r).unwrap();
        assert_eq!(next, "http://a.onion/moved");
    }

    #[tokio::test]
    async fn redirection_https_suivie_schemes_exotiques_refuses() {
        // `https` est suivi (TLS au-dessus du flux Tor — `api_via_exit`).
        let r = roundtrip(
            b"HTTP/1.1 301\r\nLocation: https://x.onion/\r\nContent-Length: 0\r\n\r\n",
            1024,
        )
        .await
        .unwrap();
        assert_eq!(
            resolve_redirect("http://a.onion/", &r).unwrap(),
            "https://x.onion/"
        );
        // Tout autre scheme reste refuse.
        let r = roundtrip(
            b"HTTP/1.1 301\r\nLocation: ftp://x.onion/\r\nContent-Length: 0\r\n\r\n",
            1024,
        )
        .await
        .unwrap();
        assert!(matches!(
            resolve_redirect("http://a.onion/", &r),
            Err(TorError::RedirectDenied(_))
        ));
    }

    #[tokio::test]
    async fn cible_https_tls_et_rejets() {
        // `https` est parse avec `tls` — la politique borne ensuite
        // l'hote et le port, comme pour `http`.
        let t = parse_target("https://x.onion/a?b=1").unwrap();
        assert!(t.tls);
        assert_eq!(t.port, 443);
        assert!(!parse_target("http://x.onion/").unwrap().tls);
        assert!(parse_target("http://user:pw@x.onion/").is_err());
        assert!(parse_target("ftp://x.onion/").is_err());
        assert!(parse_target("pas une url").is_err());
        let t = parse_target("http://X.ONION:8080/p?q=1#frag").unwrap();
        assert_eq!(t.host, "x.onion");
        assert_eq!(t.port, 8080);
        assert_eq!(t.path, "/p?q=1");
    }

    #[tokio::test]
    async fn timeout_sur_flux_muet() {
        let (_server, client) = tokio::io::duplex(1024);
        let req = build_request(&parse_target("http://x.onion/").unwrap());
        let res = exchange(client, &req, 1024, Duration::from_millis(50)).await;
        assert!(matches!(res, Err(TorError::Timeout)));
    }
}
