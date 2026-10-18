// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `TorProxyServer` — ADR-0030 (etapes 129 + 131) : proxy HTTP
//! **forward loopback** devant `TorRuntime::open_stream`.
//!
//! Perimetre :
//!
//! - listener `127.0.0.1` uniquement (adresse non-loopback refusee
//!   a la config — discipline `proxy_guard`) ;
//! - `CONNECT host:port` → politique → `200` + relais
//!   bidirectionnel (TLS etabli de bout en bout cote client) ;
//! - `GET http://hote/chemin` (URI absolue) → politique → reecriture
//!   en forme origin + relais ; `Proxy-Authorization` et
//!   `Proxy-Connection` sont **retires** — jamais transmis a la
//!   cible ;
//! - politique §4 : `*[sous-domaine.]<56c>.onion` v3 valide
//!   (`OnionAddr::parse_host` — les formes publiees
//!   `www.bbcnews….onion` sont licites, `x.onion.evil.com` refuse),
//!   ports {80,443} ; litteraux IP, clearnet et tout le reste
//!   refuses **avant** tout `connect` — aucun `to_socket_addrs`
//!   n'existe dans ce fichier (le hostname part brut vers `connect`,
//!   resolution distante cote Tor : la fuite DNS est
//!   structurellement impossible) ;
//! - auth (etape 131) : `auth_token = Some` exige
//!   `Proxy-Authorization: Basic <b64(user:token)>` — le champ
//!   *password* est le token de session, compare en temps constant
//!   (`subtle`) ; `407` sinon, avant meme la politique ;
//! - isolation (etape 131) : un `IsolationToken` par **hote
//!   `.onion` de premier niveau** (l'adresse du service, hors
//!   sous-domaines — facon first-party de Tor Browser) via
//!   `StreamPrefs`/`connect_with_prefs` ; table bornee a
//!   `isolation_max_entries`, eviction FIFO (un surplus d'origines
//!   re-utilise alors des circuits — borne memoire/circuits, jamais
//!   un partage silencieux sans plafond) ;
//! - metriques : `TorProxyServer::metrics` (`conns_active`,
//!   `conns_total`, `denied_total`, `auth_failed_total`) exposee
//!   pour `/api/tor/status` (etape 132) ;
//! - bornes : en-tetes <= `max_header_bytes`, `max_conns`
//!   simultanees, timeout d'inactivite `idle_timeout`.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use subtle::ConstantTimeEq;
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
    /// Token de session exige via `Proxy-Authorization: Basic` —
    /// `Some` = auth obligatoire (`require_auth` effectif), `None`
    /// = loopback seul. Le user de la paire `user:pass` est ignore ;
    /// le secret est compare en temps constant et n'est jamais
    /// logue ni relaye.
    pub auth_token: Option<Arc<str>>,
    /// Nombre max d'entrees d'isolation vivantes (un token par
    /// origine `.onion` de premier niveau). Au-dela : eviction FIFO
    /// — borne la memoire et le nombre de circuits crees.
    pub isolation_max_entries: usize,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            max_header_bytes: 16 * 1024,
            max_conns: 64,
            idle_timeout: Duration::from_secs(120),
            auth_token: None,
            isolation_max_entries: 256,
        }
    }
}

/// Compteurs du proxy — exposes par `TorProxyServer::metrics` pour
/// `/api/tor/status` (etape 132). Compteurs uniquement : jamais
/// d'hote ni de contenu (ADR-0028 §5).
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    /// Connexions clientes actives en ce moment.
    pub conns_active: AtomicUsize,
    /// Total cumule de connexions acceptees.
    pub conns_total: AtomicUsize,
    /// Rejets politique (`403`) cumules.
    pub denied_total: AtomicUsize,
    /// Rejets d'auth (`407`) cumules.
    pub auth_failed_total: AtomicUsize,
}

/// Handle du proxy lance — adresse reelle d'ecoute + metriques.
pub struct TorProxyServer {
    addr: SocketAddr,
    metrics: Arc<ProxyMetrics>,
}

impl TorProxyServer {
    /// Adresse d'ecoute reelle (le port de `127.0.0.1:0` est choisi
    /// par l'OS — a publier par `GET /api/tor/status`, etape 132).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Compteurs vivants du proxy.
    pub fn metrics(&self) -> &Arc<ProxyMetrics> {
        &self.metrics
    }
}

/// Table bornee origine → token d'isolation arti : un groupe de
/// circuits par hote `.onion` de premier niveau (le service — les
/// sous-domaines partagent son token, arti ne les distingue pas en
/// routage), facon first-party de Tor Browser. `StreamPrefs` n'est
/// pas stocke entier — `new()` + `set_isolation` au connect garde
/// les autres prefs au defaut du client.
struct IsolationMap {
    tokens: HashMap<String, arti_client::IsolationToken>,
    order: VecDeque<String>,
    max: usize,
}

impl IsolationMap {
    fn new(max: usize) -> Self {
        Self {
            tokens: HashMap::new(),
            order: VecDeque::new(),
            max: max.max(1),
        }
    }

    /// Token de `origin`, cree si absent. A saturation, le plus
    /// ancien est evince — son prochain connect repartira sur un
    /// token frais (circuits neufs : pas de melange retro-actif).
    fn token_for(&mut self, origin: &str) -> arti_client::IsolationToken {
        if let Some(t) = self.tokens.get(origin) {
            return *t;
        }
        while self.tokens.len() >= self.max {
            match self.order.pop_front() {
                Some(old) => {
                    self.tokens.remove(&old);
                }
                None => break,
            }
        }
        let t = arti_client::IsolationToken::new();
        self.tokens.insert(origin.to_string(), t);
        self.order.push_back(origin.to_string());
        t
    }
}

/// Politique de cible du navigateur (ADR-0030 §4) — **inversee**
/// par rapport a l'allowlist du fetcher : hote `.onion` v3 valide
/// (sous-domaines toleres — `OnionAddr::parse_host`), ports
/// {80,443} ; litteraux IP, clearnet, `.onion` malforme refuses.
///
/// Retourne `(host complet normalise, origine d'isolation)` — le
/// host conserve les sous-domaines (envoye au pair par arti),
/// l'origine est l'adresse du service (cle first-party).
fn check_target(host: &str, port: u16) -> Result<(String, String)> {
    let denied = |r: &'static str| {
        TorError::DeniedTarget(onionbit_network_policy::PolicyError::DeniedDestination(r))
    };
    // Litteral IP refuse d'office — avant meme le parse `.onion`.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Err(denied("litteral IP interdit"));
    }
    let (host, addr) = onionbit_format::OnionAddr::parse_host(host)
        .map_err(|_| denied("cible non-.onion ou .onion malforme"))?;
    if !matches!(port, 80 | 443) {
        return Err(denied("port hors {80, 443}"));
    }
    Ok((host, addr.host().to_string()))
}

/// En-tetes d'une requete proxy parsée.
struct RequestHead {
    method: String,
    target: String,
    /// Valeur brute `Proxy-Authorization` (consommee par l'auth,
    /// jamais inseree dans `headers` ni relayee).
    proxy_auth: Option<String>,
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
    let Some(method) = req.method.map(|m| m.to_string()) else {
        return Err(bad("methode absente"));
    };
    let target = req.path.unwrap_or_default().to_string();
    let mut kept = Vec::new();
    let mut proxy_auth = None;
    for h in req.headers.iter() {
        let name = h.name.to_string();
        let lname = name.to_ascii_lowercase();
        // Jamais transmis a la cible : credentials proxy et controle
        // de connexion hop-by-hop. La valeur auth est capturee pour
        // la verification locale — puis jetee.
        if lname == "proxy-authorization" {
            proxy_auth = Some(String::from_utf8_lossy(h.value).to_string());
            continue;
        }
        if lname.starts_with("proxy-") {
            continue;
        }
        let value = String::from_utf8_lossy(h.value).to_string();
        kept.push((name, value));
    }
    let head = RequestHead {
        method,
        target: target.clone(),
        proxy_auth,
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

/// Verification `Proxy-Authorization: Basic` (etape 131) : le
/// *password* de la paire `user:pass` decodee est compare au token
/// de session en **temps constant** (timing-safe — `subtle`, meme
/// discipline que `onionbit-crypto`). Le user est libre (DilloRust
/// fournit `http_proxyuser`/`http_proxypass`) ; `None` = auth
/// desactivee. La valeur n'est jamais loguee ni relayee.
fn auth_ok(head: &RequestHead, token: Option<&str>) -> bool {
    let Some(token) = token else { return true };
    let Some(raw) = head.proxy_auth.as_deref() else {
        return false;
    };
    let Some(b64) = raw
        .trim()
        .strip_prefix("Basic ")
        .or_else(|| raw.trim().strip_prefix("basic "))
    else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    // Le mot de passe suit le premier ':' — le user peut contenir
    // ce qu'il veut avant (jamais de ':' dans un token de session :
    // la convention est documentee dans `ProxyConfig::auth_token`).
    let Some(pos) = decoded.iter().position(|&b| b == b':') else {
        return false;
    };
    decoded[pos + 1..].ct_eq(token.as_bytes()).into()
}

/// Reponse d'erreur HTTP minimale du proxy (codes proxy usuels) —
/// `extra` porte les en-tetes propres au code (`Proxy-Authenticate`
/// du `407`, vide sinon).
async fn respond(
    sock: &mut TcpStream,
    status: &str,
    body: &str,
    extra: &str,
) -> std::io::Result<()> {
    sock.write_all(
        format!(
            "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
}

/// Une connexion client : lecture bornee de l'en-tete, auth,
/// politique, `open_stream_isolated`, relais. Jamais d'URL/hote
/// dans les traces au-dela de `debug` (ADR-0028 §5).
async fn handle_conn(
    rt: TorRuntime,
    mut sock: TcpStream,
    cfg: Arc<ProxyConfig>,
    isolation: Arc<Mutex<IsolationMap>>,
    metrics: Arc<ProxyMetrics>,
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
                    return respond(&mut sock, "400 Bad Request", "en-tetes trop grands", "").await;
                }
            }
            Ok(Err(e)) => return Err(e),
        }
    };
    let (head, (host, port)) = match parse_head(&buf[..head_len]) {
        Ok(v) => v,
        Err(_) => return respond(&mut sock, "400 Bad Request", "requete invalide", "").await,
    };
    // Auth AVANT la politique : le credential protege l'usage du
    // proxy lui-meme (407 = reponse canonique d'un forward proxy).
    if !auth_ok(&head, cfg.auth_token.as_deref()) {
        metrics.auth_failed_total.fetch_add(1, Ordering::Relaxed);
        return respond(
            &mut sock,
            "407 Proxy Authentication Required",
            "auth proxy requise",
            "Proxy-Authenticate: Basic realm=\"onionbit\"\r\n",
        )
        .await;
    }
    // Politique AVANT tout connect — aucun socket n'est ouvert pour
    // une cible refusee.
    let (host, origin) = match check_target(&host, port) {
        Ok(v) => v,
        Err(_) => {
            metrics.denied_total.fetch_add(1, Ordering::Relaxed);
            return respond(&mut sock, "403 Forbidden", "cible interdite", "").await;
        }
    };
    // Un groupe de circuits par origine `.onion` — le token sort du
    // verrou avant tout `.await` (mutex synchrone, jamais garde).
    let iso = isolation
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .token_for(&origin);
    let mut tor = match rt.open_stream_isolated(&host, port, iso).await {
        Ok(s) => s,
        Err(_) => {
            return respond(&mut sock, "502 Bad Gateway", "connexion tor impossible", "").await
        }
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
/// Retourne le `TorProxyServer` : `local_addr` est l'adresse reelle
/// (le port de `127.0.0.1:0` est choisi par l'OS et publie par
/// `GET /api/tor/status` en etape 132), `metrics` les compteurs.
pub async fn serve(rt: TorRuntime, listen: SocketAddr, cfg: ProxyConfig) -> Result<TorProxyServer> {
    if !listen.ip().is_loopback() {
        return Err(TorError::Request(format!(
            "ecoute proxy non-loopback refusee : {listen}"
        )));
    }
    if let Some(t) = cfg.auth_token.as_deref() {
        if t.is_empty() || t.contains(':') {
            return Err(TorError::Request(
                "auth_token proxy vide ou contenant ':' — refuse".into(),
            ));
        }
    }
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|e| TorError::Request(format!("bind proxy {listen} : {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| TorError::Request(format!("local_addr : {e}")))?;
    let cfg = Arc::new(cfg);
    let permits = Arc::new(tokio::sync::Semaphore::new(cfg.max_conns));
    let isolation = Arc::new(Mutex::new(IsolationMap::new(cfg.isolation_max_entries)));
    let metrics = Arc::new(ProxyMetrics::default());
    let metrics2 = metrics.clone();
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
            let isolation = isolation.clone();
            let metrics = metrics2.clone();
            metrics.conns_total.fetch_add(1, Ordering::Relaxed);
            metrics.conns_active.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let _permit = permit;
                let m = metrics.clone();
                let _done = ConnGuard(m);
                if let Err(e) = handle_conn(rt, sock, cfg, isolation, metrics).await {
                    tracing::debug!("connexion proxy : {e}");
                }
            });
        }
    });
    Ok(TorProxyServer { addr, metrics })
}

/// Decompte `conns_active` a la fin de la connexion, quel que soit
/// le chemin de sortie (erreur, EOF, panique sous tache).
struct ConnGuard(Arc<ProxyMetrics>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.conns_active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{TorMode, TorSettings};

    const TPB: &str = "piratebayo3klnzokct3wt5yyxb2vpebbuyjl7m623iaxmqhsd52coid.onion";

    #[test]
    fn politique_refuse_litteraux_et_clearnet() {
        // .onion v3 valide + port 80/443 → accepte, normalise —
        // (host complet, origine d'isolation = service).
        assert_eq!(check_target(TPB, 80).unwrap(), (TPB.into(), TPB.into()));
        assert_eq!(check_target(TPB, 443).unwrap(), (TPB.into(), TPB.into()));
        // Sous-domaines toleres : host complet conserve, origine =
        // le service de base (first-party, etape 131).
        let sub = format!("www.{TPB}");
        assert_eq!(check_target(&sub, 443).unwrap(), (sub.clone(), TPB.into()));
        let deep = format!("a.b.c.{TPB}");
        assert_eq!(check_target(&deep, 80).unwrap(), (deep, TPB.into()));
        // Litteraux IP, privés, clearnet, sous-domaines malformes,
        // ports hors plage : refuses.
        for bad in [
            "1.2.3.4",
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "example.com",
            "localhost",
            "pas-un.onion",
            "x.onion.evil.com",
            "www.pas-un.onion",
            &format!("so_us.{TPB}"),
            &format!("-a.{TPB}"),
            &format!("a..{TPB}"),
        ] {
            assert!(check_target(bad, 80).is_err(), "{bad}");
        }
        assert!(check_target(TPB, 8080).is_err(), "port hors plage");
        assert!(check_target(TPB, 22).is_err(), "ssh hors plage");
    }

    #[test]
    fn auth_token_accepte_et_refuse() {
        let head = |auth: &str| RequestHead {
            method: "GET".into(),
            target: "http://a.onion/".into(),
            proxy_auth: Some(auth.into()),
            headers: vec![],
        };
        let token = "s3cret-session";
        // Desactive : tout passe.
        assert!(auth_ok(&head("garbage"), None));
        // Bon mot de passe (user libre), casse `Basic` tolerée.
        let ok = base64::engine::general_purpose::STANDARD.encode("browser:s3cret-session");
        assert!(auth_ok(&head(&format!("Basic {ok}")), Some(token)));
        let ok2 = base64::engine::general_purpose::STANDARD.encode("u:s3cret-session");
        assert!(auth_ok(&head(&format!("basic {ok2}")), Some(token)));
        // Mauvais mdp, user==token sans ':', scheme non-Basic,
        // b64 invalide, champ absent.
        let ko = base64::engine::general_purpose::STANDARD.encode("browser:mauvais");
        assert!(!auth_ok(&head(&format!("Basic {ko}")), Some(token)));
        assert!(!auth_ok(&head(&format!("Basic {token}")), Some(token)));
        assert!(!auth_ok(&head("Bearer xyz"), Some(token)));
        assert!(!auth_ok(&head("Basic !!!"), Some(token)));
        let h = RequestHead {
            method: "GET".into(),
            target: String::new(),
            proxy_auth: None,
            headers: vec![],
        };
        assert!(!auth_ok(&h, Some(token)));
    }

    #[test]
    fn isolation_par_origine_bornee() {
        let mut map = IsolationMap::new(2);
        let other = "3g2upl4pq6kufc4m.onion";
        // Meme origine → meme token (circuits partages).
        let t1 = map.token_for(TPB);
        assert_eq!(t1, map.token_for(TPB));
        // Origine distincte → token distinct (pas de melange).
        let t2 = map.token_for(other);
        assert_ne!(t1, t2);
        // Borne atteinte : le plus ancien est evince.
        let t3 = map.token_for("duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion");
        assert_ne!(t3, t1);
        // TPB evince : nouvelle demande → token frais, jamais t2.
        let t4 = map.token_for(TPB);
        assert_ne!(t4, t1);
        assert_ne!(t4, t2);
        assert!(map.tokens.len() <= 2);
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
        let proxy = serve(rt, "127.0.0.1:0".parse().unwrap(), ProxyConfig::default())
            .await
            .unwrap();
        assert!(proxy.local_addr().ip().is_loopback());
        let mut c = TcpStream::connect(proxy.local_addr()).await.unwrap();
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
        assert_eq!(proxy.metrics().conns_total.load(Ordering::Relaxed), 1);
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
        let proxy = serve(rt, "127.0.0.1:0".parse().unwrap(), ProxyConfig::default())
            .await
            .unwrap();
        let mut c = TcpStream::connect(proxy.local_addr()).await.unwrap();
        c.write_all(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 403"));
        assert_eq!(proxy.metrics().denied_total.load(Ordering::Relaxed), 1);
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn proxy_auth_407_puis_relais_avec_token() {
        let (socks, _t) = fake_socks5().await;
        let rt = TorRuntime::new(TorSettings {
            mode: TorMode::External(socks),
            ..TorSettings::default()
        });
        let cfg = ProxyConfig {
            auth_token: Some("s3cret".into()),
            ..ProxyConfig::default()
        };
        let proxy = serve(rt, "127.0.0.1:0".parse().unwrap(), cfg)
            .await
            .unwrap();
        // Sans credential : 407 + Proxy-Authenticate — jamais relaye
        // a la politique ni au connect.
        let mut c = TcpStream::connect(proxy.local_addr()).await.unwrap();
        c.write_all(format!("GET http://{TPB}/ HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 407"), "{resp}");
        assert!(resp.contains("Proxy-Authenticate"), "{resp}");
        assert_eq!(proxy.metrics().auth_failed_total.load(Ordering::Relaxed), 1);
        // Avec le bon token : relais complet.
        let cred = base64::engine::general_purpose::STANDARD.encode("dillo:s3cret");
        let mut c2 = TcpStream::connect(proxy.local_addr()).await.unwrap();
        c2.write_all(
            format!("GET http://{TPB}/ HTTP/1.1\r\nProxy-Authorization: Basic {cred}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let mut buf2 = vec![0u8; 4096];
        let n2 = tokio::time::timeout(Duration::from_secs(5), c2.read(&mut buf2))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buf2[..n2]).starts_with("HTTP/1.1 200 OK"));
    }

    #[cfg(feature = "tor")]
    #[tokio::test]
    async fn conns_active_remonte_a_zero() {
        let (socks, _t) = fake_socks5().await;
        let rt = TorRuntime::new(TorSettings {
            mode: TorMode::External(socks),
            ..TorSettings::default()
        });
        let proxy = serve(rt, "127.0.0.1:0".parse().unwrap(), ProxyConfig::default())
            .await
            .unwrap();
        let c = TcpStream::connect(proxy.local_addr()).await.unwrap();
        // Le compteur monte a l'accept (eventuellement — spawn
        // asynchrone : on attend qu'il apparaisse).
        for _ in 0..50 {
            if proxy.metrics().conns_active.load(Ordering::Relaxed) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(proxy.metrics().conns_active.load(Ordering::Relaxed), 1);
        drop(c);
        for _ in 0..50 {
            if proxy.metrics().conns_active.load(Ordering::Relaxed) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(proxy.metrics().conns_active.load(Ordering::Relaxed), 0);
    }
}
