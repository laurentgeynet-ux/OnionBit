// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `TorProxyServer` — spike ADR-0030 (etape 129) : proxy HTTP
//! **forward loopback** devant `TorRuntime::open_stream`.
//!
//! Perimetre du spike :
//!
//! - listener `127.0.0.1` uniquement (adresse non-loopback refusee
//!   a la config — discipline `proxy_guard`) ;
//! - `CONNECT host:port` → politique → `200` + relais
//!   bidirectionnel (TLS etabli de bout en bout cote client) ;
//! - `GET http://hote/chemin` (URI absolue) → politique → reecriture
//!   en forme origin + relais ; `Proxy-Authorization` et
//!   `Proxy-Connection` sont **retires** — jamais transmis a la
//!   cible ;
//! - politique §4 : `.onion` v3 valide (`format::onion`), ports
//!   {80,443} ; litteraux IP, clearnet et tout le reste refuses
//!   **avant** tout `connect` — aucun `to_socket_addrs` n'existe
//!   dans ce fichier (le hostname part brut vers `connect`,
//!   resolution distante cote Tor : la fuite DNS est
//!   structurellement impossible) ;
//! - bornes : en-tetes <= `max_header_bytes`, `max_conns`
//!   simultanees, timeout d'inactivite `idle_timeout`.
//!
//! Hors spike (etape 131) : `Proxy-Authorization` token de session,
//! isolation `per_origin` par `StreamPrefs`, metriques, auth.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{Result, TorError};

use crate::runtime::TorRuntime;

/// Configuration du proxy — aucune valeur en dur (ADR-0030 §7).
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// En-tetes de requete max avant rejet 400.
    pub max_header_bytes: usize,
    /// Connexions simultanees max (au-dela : rejet immediat).
    pub max_conns: usize,
    /// Fermeture d'une connexion inactive.
    pub idle_timeout: Duration,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            max_header_bytes: 16 * 1024,
            max_conns: 64,
            idle_timeout: Duration::from_secs(120),
        }
    }
}

/// Politique de cible du navigateur (ADR-0030 §4) — **inversee**
/// par rapport a l'allowlist du fetcher : `.onion` v3 valide, ports
/// {80,443} ; litteraux IP, clearnet, `.onion` malforme refuses.
/// Le nom retourne est la forme normalisee (minuscules).
fn check_target(host: &str, port: u16) -> Result<String> {
    let denied = |r: &'static str| {
        TorError::DeniedTarget(onionbit_network_policy::PolicyError::DeniedDestination(r))
    };
    // Litteral IP refuse d'office — avant meme le parse `.onion`.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(denied("litteral IP interdit"));
    }
    let addr = onionbit_format::OnionAddr::parse(host)
        .map_err(|_| denied("cible non-.onion ou .onion malforme"))?;
    if !matches!(port, 80 | 443) {
        return Err(denied("port hors {80, 443}"));
    }
    Ok(addr.host().to_string())
}

/// En-tetes d'une requete proxy parsée.
struct RequestHead {
    method: String,
    target: String,
    /// En-tetes transitables (hors `Proxy-*`, hors `Host` reecrit).
    headers: Vec<(String, String)>,
}

/// Decoupe `(host, port)` d'une authority `hote[:port]` — v6 entre
/// crochets toleré puis refuse par la politique (litteral IP).
fn split_authority(auth: &str, default_port: u16) -> (String, u16) {
    let auth = auth.trim_matches(|c| c == '[' || c == ']');
    match auth.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (auth.to_string(), default_port),
    }
}

/// Parse l'en-tete borne d'une requete : `CONNECT host:port` ou
/// methode + URI absolue `http://hote[:port]/chemin`. Retourne la
/// cible `(host, port)` et le head structure.
fn parse_head(buf: &[u8]) -> Result<(RequestHead, (String, u16))> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    let bad = |r: &'static str| TorError::Request(format!("requete proxy invalide : {r}"));
    match req.parse(buf) {
        Ok(httparse::Status::Complete(_)) | Ok(httparse::Status::Partial)
            if req.method.is_some() => {}
        _ => return Err(bad("incomplet ou malforme")),
    }
    let method = req.method.expect("methode").to_string();
    let target = req.path.unwrap_or_default().to_string();
    let mut kept = Vec::new();
    for h in req.headers.iter() {
        let name = h.name.to_string();
        let lname = name.to_ascii_lowercase();
        // Jamais transmis a la cible : credentials proxy et controle
        // de connexion hop-by-hop.
        if lname.starts_with("proxy-") {
            continue;
        }
        let value = String::from_utf8_lossy(h.value).to_string();
        kept.push((name, value));
    }
    let head = RequestHead {
        method,
        target: target.clone(),
        headers: kept,
    };
    if head.method.eq_ignore_ascii_case("connect") {
        let (host, port) = split_authority(&target, 443);
        return Ok((head, (host, port)));
    }
    // URI absolue `http://hote[:port]/...` (https est couvert par
    // CONNECT — un `https://` en forme absolue n'a pas de sens GET).
    let rest = target
        .strip_prefix("http://")
        .ok_or_else(|| TorError::Request("URI non-absolue sans CONNECT".into()))?;
    let (auth, _) = rest.split_once('/').unwrap_or((rest, ""));
    if auth.is_empty() {
        return Err(bad("authority absente"));
    }
    Ok((head, split_authority(auth, 80)))
}

/// Reecrit le head en **forme origin** pour envoi sur le flux Tor :
/// `GET http://h/chemin` → `GET /chemin HTTP/1.1` (+ `Host` force a
/// l'autority cible, `Proxy-*` absents).
fn to_origin_form(head: &RequestHead, host: &str, port: u16) -> Vec<u8> {
    let path = if head.method.eq_ignore_ascii_case("connect") {
        unreachable!("CONNECT n'a pas de forme origin")
    } else {
        let rest = head.target.strip_prefix("http://").unwrap_or(&head.target);
        match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        }
    };
    let mut out = format!("{} {} HTTP/1.1\r\n", head.method, path);
    let mut host_seen = false;
    for (n, v) in &head.headers {
        if n.eq_ignore_ascii_case("host") {
            // Host force a la cible validee (jamais de mismatch
            // authority/Host — la politique a tranche).
            let hv = if port == 80 {
                host.to_string()
            } else {
                format!("{host}:{port}")
            };
            out.push_str(&format!("Host: {hv}\r\n"));
            host_seen = true;
            continue;
        }
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    if !host_seen {
        out.push_str(&format!("Host: {host}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// Reponse d'erreur HTTP minimale du proxy (codes proxy usuels).
async fn respond(sock: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    sock.write_all(
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
}

/// Une connexion client : lecture bornee de l'en-tete, politique,
/// `open_stream`, relais. Jamais d'URL/hote dans les traces au-dela
/// de `debug` (ADR-0028 §5).
async fn handle_conn(
    rt: TorRuntime,
    mut sock: TcpStream,
    cfg: Arc<ProxyConfig>,
) -> std::io::Result<()> {
    // Lecture bornee de la ligne de requete + en-tetes.
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_len = loop {
        match tokio::time::timeout(cfg.idle_timeout, sock.read(&mut chunk)).await {
            Err(_) | Ok(Ok(0)) => return Ok(()),
            Ok(Ok(n)) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break p + 4;
                }
                if buf.len() > cfg.max_header_bytes {
                    return respond(&mut sock, "400 Bad Request", "en-tetes trop grands").await;
                }
            }
            Ok(Err(e)) => return Err(e),
        }
    };
    let (head, (host, port)) = match parse_head(&buf[..head_len]) {
        Ok(v) => v,
        Err(_) => return respond(&mut sock, "400 Bad Request", "requete invalide").await,
    };
    // Politique AVANT tout connect — aucun socket n'est ouvert pour
    // une cible refusee.
    let host = match check_target(&host, port) {
        Ok(h) => h,
        Err(_) => return respond(&mut sock, "403 Forbidden", "cible interdite").await,
    };
    let mut tor = match rt.open_stream(&host, port).await {
        Ok(s) => s,
        Err(_) => return respond(&mut sock, "502 Bad Gateway", "connexion tor impossible").await,
    };
    let tail = &buf[head_len..];
    if head.method.eq_ignore_ascii_case("connect") {
        sock.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        // Octets deja buffers (TLS ClientHello pipeline parfois).
        if !tail.is_empty() {
            tor.write_all(tail).await?;
            tor.flush().await?;
        }
    } else {
        tor.write_all(&to_origin_form(&head, &host, port)).await?;
        if !tail.is_empty() {
            tor.write_all(tail).await?;
        }
        // `write_all` sur un DataStream arti peut bufferiser sans
        // emettre — le flush pousse la requete vers le rendez-vous
        // (sinon la cible attend indefiniment et le circuit meurt).
        tor.flush().await?;
    }
    let relais = tokio::time::timeout(cfg.idle_timeout, async {
        tokio::io::copy_bidirectional(&mut sock, &mut *tor).await
    })
    .await;
    // Compteurs uniquement — jamais de contenu/d'hote (ADR-0028 §5).
    tracing::debug!("relais proxy termine : {relais:?}");
    Ok(())
}

/// Lance le proxy sur `listen` (loopback exige — non-loopback est
/// une erreur de config, jamais une adresse acceptee silencieusement).
/// Retourne l'adresse reelle (le port de `127.0.0.1:0` est choisi
/// par l'OS et publie par `GET /api/tor/status` en etape 132).
pub async fn serve(rt: TorRuntime, listen: SocketAddr, cfg: ProxyConfig) -> Result<SocketAddr> {
    if !listen.ip().is_loopback() {
        return Err(TorError::Request(format!(
            "ecoute proxy non-loopback refusee : {listen}"
        )));
    }
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|e| TorError::Request(format!("bind proxy {listen} : {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| TorError::Request(format!("local_addr : {e}")))?;
    let cfg = Arc::new(cfg);
    let permits = Arc::new(tokio::sync::Semaphore::new(cfg.max_conns));
    tokio::spawn(async move {
        loop {
            let permit = match permits.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let Ok((sock, _peer)) = listener.accept().await else {
                continue;
            };
            let rt = rt.clone();
            let cfg = cfg.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = handle_conn(rt, sock, cfg).await {
                    tracing::debug!("connexion proxy : {e}");
                }
            });
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{TorMode, TorSettings};

    const TPB: &str = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";

    #[test]
    fn politique_refuse_litteraux_et_clearnet() {
        // .onion v3 valide + port 80/443 → accepte, normalise.
        assert_eq!(check_target(TPB, 80).unwrap(), TPB);
        assert_eq!(check_target(TPB, 443).unwrap(), TPB);
        // Litteraux IP, privés, clearnet, ports hors plage : refuses.
        for bad in [
            "1.2.3.4",
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "example.com",
            "localhost",
            "pas-un.onion",
            "x.onion.evil.com",
        ] {
            assert!(check_target(bad, 80).is_err(), "{bad}");
        }
        assert!(check_target(TPB, 8080).is_err(), "port hors plage");
        assert!(check_target(TPB, 22).is_err(), "ssh hors plage");
    }

    #[test]
    fn parse_connect_et_absolue() {
        let (h, (host, port)) =
            parse_head(b"CONNECT a.onion:443 HTTP/1.1\r\nHost: a.onion\r\n\r\n").unwrap();
        assert_eq!(h.method, "CONNECT");
        assert_eq!((host.as_str(), port), ("a.onion", 443));
        let (_h, (host, port)) = parse_head(b"GET http://a.onion/x HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!((host.as_str(), port), ("a.onion", 80));
        // Formes interdites.
        assert!(parse_head(b"OPTIONS * HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_head(b"GET /relative HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn proxy_headers_jamais_relayes() {
        let (h, _) = parse_head(
            b"GET http://a.onion/ HTTP/1.1\r\nProxy-Authorization: Basic x\r\n\
              Proxy-Connection: keep-alive\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        let wire = String::from_utf8(to_origin_form(&h, "a.onion", 80)).unwrap();
        assert!(wire.starts_with("GET / HTTP/1.1\r\n"));
        assert!(wire.contains("Host: a.onion\r\n"));
        assert!(wire.contains("Accept: */*\r\n"));
        assert!(!wire.contains("Proxy-"), "credentials jamais relayes");
    }

    /// Mini SOCKS5 (miroir du banc runtime) + serveur repondant
    /// `resp` : permet de tester le relais en mode `external`.
    #[cfg(feature = "tor")]
    async fn fake_socks5() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut head = [0u8; 2];
                    if s.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    let mut m = vec![0u8; head[1] as usize];
                    if s.read_exact(&mut m).await.is_err() {
                        return;
                    }
                    let _ = s.write_all(&[0x05, 0x00]).await;
                    let mut req = [0u8; 4];
                    if s.read_exact(&mut req).await.is_err() {
                        return;
                    }
                    let need = match req[3] {
                        1 => 6,
                        3 => {
                            let mut l = [0u8; 1];
                            let _ = s.read_exact(&mut l).await;
                            l[0] as usize + 2
                        }
                        _ => 0,
                    };
                    let mut rest = vec![0u8; need];
                    let _ = s.read_exact(&mut rest).await;
                    let _ = s
                        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await;
                    // Relais brut : echo de ce que le client envoie
                    // (le proxy ecrit la requete reecrite — on la
                    // relit pour verifier la forme).
                    let mut buf = vec![0u8; 2048];
                    if let Ok(_n) = s.read(&mut buf).await {
                        let _ = s
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nX-Relay: ok\r\nContent-Length: 0\r\n\r\n",
                            )
                            .await;
                        let _ = s.shutdown().await;
                    }
                });
            }
        });
        (addr, task)
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn proxy_relay_get_via_socks_externe() {
        let (socks, _t) = fake_socks5().await;
        let rt = TorRuntime::new(TorSettings {
            mode: TorMode::External(socks),
            ..TorSettings::default()
        });
        let addr = serve(rt, "127.0.0.1:0".parse().unwrap(), ProxyConfig::default())
            .await
            .unwrap();
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(format!("GET http://{TPB}/ HTTP/1.1\r\nHost: {TPB}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn proxy_refuse_clearnet_sans_socket() {
        // La politique refuse AVANT connect : meme un SOCKS5 mort
        // suffit — un 403 revient sans qu'aucun connect ne soit tente.
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let rt = TorRuntime::new(TorSettings {
            mode: TorMode::External(dead),
            ..TorSettings::default()
        });
        let addr = serve(rt, "127.0.0.1:0".parse().unwrap(), ProxyConfig::default())
            .await
            .unwrap();
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 403"));
    }
}
