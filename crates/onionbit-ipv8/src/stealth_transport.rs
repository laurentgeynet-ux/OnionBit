// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `StealthTransport` (ADR-0017, etape 51) : `DatagramTransport`
//! morphe — tout datagramme emis/recu sur le socket UDP est une
//! trame stealth (handshake `rep(X')`/`rep(Y')` ou trame AEAD),
//! jamais un octet de protocole en clair. Composition sur
//! [`RawUdpTransport`] : compteurs d'octets et tap mesurent la forme
//! **filaire morphee** a la frontiere socket.
//!
//! Proprietes de defense (spec ADR-0017 §2/§3) :
//!
//! - **Silence absolu** : tout datagramme qui n'ouvre ni handshake
//!   valide ni trame de session est ignore — aucune reponse, aucune
//!   allocation non bornee (`StealthError::Reject` uniforme : MAC,
//!   horodatage, rejeu, saturation indiscernables).
//! - **Aucun repli clair** : `send_to` vers une destination sans
//!   session et hors `bridges` configures = drop local ; jamais de
//!   UDP brut. NAT rebinding, perte, expiration → nouveau handshake
//!   ou silence.
//! - **Budgets pre-authentification** : un candidat `hs1` (taille
//!   dans les bornes, source inconnue) coute un jeton **par IP** +
//!   un jeton du **plafond global** avant le moindre DH/AEAD — le
//!   par-IP est spoofable, le global est la vraie derniere ligne.
//! - **File pre-handshake bornee** : datagrammes applicatifs mis en
//!   attente pendant le handshake, plafond en count + octets.
//! - **Sessions bornees + purge** : `max_sessions`, idle timeout,
//!   rejeu `X'` a deux fenetres (voir [`XPrimeFilter`]).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use crate::error::Ipv8Error;
use crate::stealth::{
    hs1_open, hs1_seal, hs2_open, hs2_seal, Hs1ClientCtx, StealthError, StealthParams,
    StealthSession, XPrimeFilter,
};
use crate::transport::{BoxFut, DatagramTransport, RawUdpTransport, RxHandler, TapEvent};
use onionbit_crypto::stealth::HiddenEph;

/// Role du noeud stealth (ADR-0017 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StealthRole {
    /// Censure : initie vers les ponts configures, n'accepte rien.
    Client,
    /// Pont : accepte les handshakes entrants ET initie vers ses
    /// uplinks (ex. gateways) listes dans `bridges`.
    Bridge,
    /// Passerelle de sortie : accepte les handshakes, n'initie pas.
    Gateway,
}

impl StealthRole {
    /// Peut recevoir un `hs1` (roles serveur du handshake).
    fn accepts_hs1(self) -> bool {
        matches!(self, StealthRole::Bridge | StealthRole::Gateway)
    }

    /// Peut initier `hs1` vers `bridges` (roles amont).
    fn initiates(self) -> bool {
        matches!(self, StealthRole::Client | StealthRole::Bridge)
    }
}

/// Scheme des liens d'invitation hors-bande (ADR-0017 §3).
pub const BRIDGE_LINK_SCHEME: &str = "onionbit-bridge://";

/// Pont amont : adresse + `bridge_pk` X25519 (lien d'invitation
/// hors-bande `onionbit-bridge://<ip>:<port>#<bridge_pk_hex>`).
#[derive(Debug, Clone)]
pub struct BridgeEntry {
    /// `ip:port` du pont.
    pub addr: SocketAddr,
    /// Cle publique d'admission du pont.
    pub pk: [u8; 32],
}

impl BridgeEntry {
    /// Parse un lien `onionbit-bridge://<addr>#<64-hex>`. Validation
    /// stricte : scheme exact, `SocketAddr` (v4/v6 entre crochets),
    /// cle 32 octets hex, port non nul — tout ecart est `Err`,
    /// jamais de devinette.
    pub fn parse_link(link: &str) -> Result<Self, StealthError> {
        let body = link
            .strip_prefix(BRIDGE_LINK_SCHEME)
            .ok_or(StealthError::Malformed("scheme attendu"))?;
        let (addr_s, pk_s) = body
            .split_once('#')
            .ok_or(StealthError::Malformed("separateur # absent"))?;
        if pk_s.len() != 64 {
            return Err(StealthError::Malformed("cle : 64 hex requis"));
        }
        let mut pk = [0u8; 32];
        hex::decode_to_slice(pk_s, &mut pk)
            .map_err(|_| StealthError::Malformed("cle : hex invalide"))?;
        let addr: SocketAddr = addr_s
            .parse()
            .map_err(|_| StealthError::Malformed("adresse invalide"))?;
        if addr.port() == 0 {
            return Err(StealthError::Malformed("port 0 invalide"));
        }
        Ok(Self { addr, pk })
    }

    /// Serialise le lien d'invitation.
    pub fn to_link(&self) -> String {
        format!("{BRIDGE_LINK_SCHEME}{}#{}", self.addr, hex::encode(self.pk))
    }
}

/// Configuration du transport stealth — aucun seuil en dur.
#[derive(Debug, Clone)]
pub struct StealthConfig {
    /// Role topologique.
    pub role: StealthRole,
    /// Secret statique `b` du pont — requis quand
    /// `role.accepts_hs1()` (`Bridge`/`Gateway`), inutilise sinon.
    pub bridge_sk: Option<[u8; 32]>,
    /// `bridge_pk` correspondant (transcript `hs1`).
    pub bridge_pk: Option<[u8; 32]>,
    /// Ponts amonts (client : ses entrees ; bridge : ses gateways).
    pub bridges: Vec<BridgeEntry>,
    /// Identite client vehiculee chiffree dans `hs1` (pk maitresse
    /// en v1 — choisie par l'appelant, max [`HS1_ID_MAX`]).
    pub client_id: Vec<u8>,
    /// Parametres crypto/filaire (MTU, padding, fenetres).
    pub params: StealthParams,
    /// File pre-handshake : nombre max de datagrammes en attente par
    /// session en cours d'etablissement.
    pub pre_hs_queue_max: usize,
    /// File pre-handshake : budget cumule d'octets par session.
    pub pre_hs_queue_bytes: usize,
    /// Jetons de tentative `hs1` par IP par seconde (refill).
    pub hs1_per_ip_per_sec: u32,
    /// Rafale `hs1` max par IP.
    pub hs1_per_ip_burst: u32,
    /// Plafond **global** de tentatives `hs1` par tick (derniere
    /// ligne anti-DoS CPU — le par-IP est spoofable).
    pub hs1_global_per_sec: u32,
    /// Table du rate-limiter : IP distinctes max (sature → silence).
    pub ratelimit_max_ips: usize,
    /// Sessions etablies max.
    pub max_sessions: usize,
    /// Intervalle de reemission `hs1` tant que la session est
    /// `Pending` (resilience a la perte).
    pub hs_retry_secs: u64,
    /// Tentatives `hs1` max avant abandon de la session `Pending`.
    pub hs_attempts_max: u8,
    /// Timeout d'une session `Pending` sans reponse.
    pub pending_timeout_secs: u64,
    /// Idle timeout d'une session `Established` (purge → silence).
    pub session_idle_timeout_secs: u64,
    /// Periode du tick interne (purge, retry, refill jetons, cover).
    pub tick_ms: u64,
    /// Cover traffic opt-in : trames `inner=vide` (no-op applicatif)
    /// emises vers une session etablie a intervalle randomise.
    pub cover_traffic: bool,
    /// Intervalle `[min,max]` ms entre trames de cover.
    pub cover_interval_ms: (u64, u64),
}

impl Default for StealthConfig {
    fn default() -> Self {
        Self {
            role: StealthRole::Client,
            bridge_sk: None,
            bridge_pk: None,
            bridges: Vec::new(),
            client_id: Vec::new(),
            params: StealthParams::default(),
            pre_hs_queue_max: 64,
            pre_hs_queue_bytes: 64 * 1280,
            hs1_per_ip_per_sec: 4,
            hs1_per_ip_burst: 8,
            hs1_global_per_sec: 256,
            ratelimit_max_ips: 50_000,
            max_sessions: 10_000,
            hs_retry_secs: 2,
            hs_attempts_max: 8,
            pending_timeout_secs: 30,
            session_idle_timeout_secs: 300,
            tick_ms: 500,
            cover_traffic: false,
            cover_interval_ms: (500, 2000),
        }
    }
}

/// Compteurs de diagnostic — cause interne de rejet, jamais sur le
/// fil. Consumes par le banc hostile (etape 51) et `/api/stealth`
/// (etape 52).
#[derive(Debug, Default)]
pub struct StealthMetrics {
    /// Datagrammes non candidats (session inconnue, taille hors
    /// bornes, role sans acceptation) — aucun travail crypto.
    pub rx_garbage: AtomicU64,
    /// Candidats `hs1` rejetes par le rate-limit par IP.
    pub rl_ip_dropped: AtomicU64,
    /// Candidats `hs1` rejetes par le plafond global.
    pub rl_global_dropped: AtomicU64,
    /// Candidats `hs1` rejetes apres crypto (MAC/ts/rejeu/saturation
    /// — causes volontairement fusionnees : uniformite externe).
    pub hs1_rejected: AtomicU64,
    /// `hs1` acceptes → `hs2` emis.
    pub hs1_accepted: AtomicU64,
    /// `hs2` acceptes cote initiateur.
    pub hs2_accepted: AtomicU64,
    /// `hs2` candidats rejetes.
    pub hs2_rejected: AtomicU64,
    /// Trames applicatives ouvertes.
    pub frames_in: AtomicU64,
    /// Trames morphees emises.
    pub frames_out: AtomicU64,
    /// `hs1` emis (premiers envois + retries).
    pub hs1_sent: AtomicU64,
    /// `hs2` emis.
    pub hs2_sent: AtomicU64,
    /// Datagrammes applicatifs dropes (file pre-hs pleine ou dest
    /// hors session/ponts — jamais emis en clair).
    pub queue_dropped: AtomicU64,
    /// Sessions purgees (idle/pending timeout/attempts).
    pub sessions_expired: AtomicU64,
    /// Trames de session rejetees (rejeu/fenetre/AEAD).
    pub frames_rejected: AtomicU64,
    /// Trames de cover emises.
    pub cover_sent: AtomicU64,
}

/// Bucket de jetons par IP (token bucket borne).
struct RateBucket {
    /// Jetons restants.
    tokens: f64,
    /// Dernier refill.
    last: Instant,
}

/// Session `Pending` : handshake `hs1` emis, reponse attendue.
struct Pending {
    /// Ephemere client (re-serve pour les retries avec nouveau ts).
    x: HiddenEph,
    /// Contexte pour `hs2_open`.
    ctx: Hs1ClientCtx,
    /// `bridge_pk` visee (transcript du retry).
    bridge_pk: [u8; 32],
    /// Tentatives `hs1` emises.
    attempts: u8,
    /// Dernier envoi `hs1`.
    last_sent: Instant,
    /// Premiere tentative (timeout global `Pending`).
    started: Instant,
    /// File applicative bornee.
    queue: VecDeque<Vec<u8>>,
    /// Octets cumules en file.
    queue_bytes: usize,
}

/// Session `Established` : trames AEAD + activite.
struct Established {
    /// Crypto de session (cles directionnelles, fenetre rejeu).
    session: StealthSession,
    /// Dernier trafic (purge idle).
    last_active: Instant,
}

/// Etat par `SocketAddr` distant.
enum PeerState {
    /// Handshake en cours (initiateur).
    Pending(Pending),
    /// Session etablie.
    Established(Established),
}

/// Instant Unix courant (horodatage handshake).
fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Transport furtif : morphing complet du trafic UDP.
pub struct StealthTransport {
    /// Socket UDP sous-jacent — forme filaire morphee uniquement.
    raw: Arc<RawUdpTransport>,
    /// Configuration.
    cfg: StealthConfig,
    /// Sessions par adresse distante.
    sessions: Mutex<HashMap<SocketAddr, PeerState>>,
    /// Filtre anti-rejeu `X'` (roles serveur).
    xprime: Mutex<XPrimeFilter>,
    /// Rate-limiter `hs1` par IP (table bornee).
    buckets: Mutex<HashMap<IpAddr, RateBucket>>,
    /// Jetons du plafond global de tentatives `hs1` par tick.
    global_tokens: AtomicU64,
    /// Index `addr → pk` des ponts amonts (mutable : ajout a
    /// chaud via `POST /api/stealth/bridges`).
    bridge_pks: Mutex<HashMap<SocketAddr, [u8; 32]>>,
    /// Metriques du banc hostile.
    metrics: StealthMetrics,
}

impl StealthTransport {
    /// Lie le socket et initialise l'etat. `Err` si `role` accepte
    /// `hs1` sans `bridge_sk`/`bridge_pk` coherents — fail-closed,
    /// jamais de demarrage a moitie configure.
    pub async fn bind(
        bind: &str,
        bind_v6: Option<&str>,
        cfg: StealthConfig,
    ) -> Result<Arc<Self>, Ipv8Error> {
        if cfg.role.accepts_hs1() && (cfg.bridge_sk.is_none() || cfg.bridge_pk.is_none()) {
            return Err(Ipv8Error::Malformed(
                "stealth: role serveur sans bridge_sk/bridge_pk",
            ));
        }
        let raw = RawUdpTransport::bind_dual(bind, bind_v6).await?;
        Ok(Self::from_raw(raw, cfg))
    }

    /// Enrobe un [`RawUdpTransport`] deja lie (construction par le
    /// endpoint a l'etape 53).
    pub fn from_raw(raw: Arc<RawUdpTransport>, cfg: StealthConfig) -> Arc<Self> {
        let bridge_pks = Mutex::new(cfg.bridges.iter().map(|b| (b.addr, b.pk)).collect());
        let global_tokens = cfg.hs1_global_per_sec as u64;
        Arc::new(Self {
            raw,
            xprime: Mutex::new(XPrimeFilter::new(
                cfg.params.hs_timestamp_skew_secs.max(1),
                cfg.params.xprime_set_max,
            )),
            sessions: Mutex::new(HashMap::new()),
            buckets: Mutex::new(HashMap::new()),
            global_tokens: AtomicU64::new(global_tokens),
            bridge_pks,
            metrics: StealthMetrics::default(),
            cfg,
        })
    }

    /// Compteurs du banc hostile / API.
    pub fn metrics(&self) -> &StealthMetrics {
        &self.metrics
    }

    /// Ajoute un pont amont a chaud (`POST /api/stealth/bridges`) —
    /// deduplique sur l'adresse (la pk la plus recente gagne : une
    /// cle re-emise par invitation remplace l'ancienne).
    pub fn add_bridge(&self, entry: BridgeEntry) {
        self.bridge_pks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(entry.addr, entry.pk);
    }

    /// Nombre de ponts amonts configures (diagnostic — les adresses
    /// ne quittent jamais le transport via l'API).
    pub fn bridge_count(&self) -> usize {
        self.bridge_pks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Nombre de sessions actives (`Pending` + `Established`) —
    /// diagnostic borne.
    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Rate-limit par IP : depense un jeton ; `false` = silence.
    fn take_ip_token(&self, ip: IpAddr) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let rate = self.cfg.hs1_per_ip_per_sec as f64;
        let burst = self.cfg.hs1_per_ip_burst as f64;
        if !buckets.contains_key(&ip) && buckets.len() >= self.cfg.ratelimit_max_ips {
            return false;
        }
        let bucket = buckets.entry(ip).or_insert(RateBucket {
            tokens: burst,
            last: now,
        });
        bucket.tokens =
            (bucket.tokens + rate * now.duration_since(bucket.last).as_secs_f64()).min(burst);
        bucket.last = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }

    /// Plafond global : depense un jeton ; `false` = silence.
    fn take_global_token(&self) -> bool {
        self.global_tokens
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                if t > 0 {
                    Some(t - 1)
                } else {
                    None
                }
            })
            .is_ok()
    }

    /// Traitement d'un datagramme filaire (appele par la boucle de
    /// reception du socket brut) : demorph puis remonte `on_rx`, ou
    /// silence. Toute erreur interne est absorbee ici — elle ne
    /// devient jamais un paquet.
    fn on_wire(self: &Arc<Self>, src: SocketAddr, data: &[u8], on_rx: &RxHandler) {
        // Chemin rapide : session etablie → trame.
        enum Action {
            Deliver(Vec<u8>),
            // (hs2 emis, plaintexts de file a flusher)
            EstablishedThenFlush(Vec<u8>, Vec<Vec<u8>>),
            Silence,
        }

        let action = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            // Classification sans emprunt retenu : Established /
            // Pending / inconnu — les bras empruntent a nouveau.
            enum Kind {
                Est,
                Pend,
                None,
            }
            let kind = match sessions.get(&src) {
                Some(PeerState::Established(_)) => Kind::Est,
                Some(PeerState::Pending(_)) => Kind::Pend,
                None => Kind::None,
            };
            match kind {
                Kind::Est => {
                    let e = match sessions.get_mut(&src) {
                        Some(PeerState::Established(e)) => e,
                        _ => unreachable!("etat lu a l'instant"),
                    };
                    match e.session.open_frame(data) {
                        Ok(pt) => {
                            e.last_active = Instant::now();
                            if pt.is_empty() {
                                // Cover traffic : no-op applicatif.
                                Action::Silence
                            } else {
                                self.metrics.frames_in.fetch_add(1, Ordering::Relaxed);
                                Action::Deliver(pt)
                            }
                        }
                        Err(_) => {
                            self.metrics.frames_rejected.fetch_add(1, Ordering::Relaxed);
                            Action::Silence
                        }
                    }
                }
                Kind::Pend => {
                    // hs2 attendu de ce bridge : tentative d'ouverture.
                    let Some(PeerState::Pending(p)) = sessions.remove(&src) else {
                        unreachable!("etat lu a l'instant")
                    };
                    match hs2_open(&p.x, &p.ctx, data, now_ts(), &self.cfg.params) {
                        Ok(sess) => {
                            self.metrics.hs2_accepted.fetch_add(1, Ordering::Relaxed);
                            let mut plain = Vec::with_capacity(p.queue.len());
                            let mut e = Established {
                                session: sess,
                                last_active: Instant::now(),
                            };
                            for q in p.queue {
                                match e.session.seal_frame(&q) {
                                    Ok(f) => plain.push(f),
                                    Err(_) => {
                                        self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            sessions.insert(src, PeerState::Established(e));
                            // Le premier flush reemet les
                            // datagrammes en attente — pas de
                            // reponse a renvoyer.
                            Action::EstablishedThenFlush(Vec::new(), plain)
                        }
                        Err(_) => {
                            self.metrics.hs2_rejected.fetch_add(1, Ordering::Relaxed);
                            sessions.insert(src, PeerState::Pending(p));
                            Action::Silence
                        }
                    }
                }
                Kind::None => {
                    // Candidat `hs1` : borne de taille avant tout
                    // budget/crypto (zero allocation sur garbage).
                    let min = 32 + 16 + 11;
                    if !self.cfg.role.accepts_hs1()
                        || data.len() < min
                        || data.len() > self.cfg.params.mtu
                    {
                        self.metrics.rx_garbage.fetch_add(1, Ordering::Relaxed);
                        Action::Silence
                    } else if !self.take_ip_token(src.ip()) {
                        self.metrics.rl_ip_dropped.fetch_add(1, Ordering::Relaxed);
                        Action::Silence
                    } else if !self.take_global_token() {
                        self.metrics
                            .rl_global_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        Action::Silence
                    } else {
                        // Budget consomme : travail crypto.
                        let mut filt = self.xprime.lock().unwrap_or_else(|e| e.into_inner());
                        let sk = self.cfg.bridge_sk.expect("role verifie a bind");
                        let pk = self.cfg.bridge_pk.expect("role verifie a bind");
                        match hs1_open(&sk, &pk, data, &mut filt, now_ts(), &self.cfg.params) {
                            Ok(acc) => {
                                drop(filt);
                                let y = HiddenEph::generate();
                                let (d2, sess) = hs2_seal(&y, &acc, now_ts(), &self.cfg.params);
                                if sessions.len() < self.cfg.max_sessions {
                                    sessions.insert(
                                        src,
                                        PeerState::Established(Established {
                                            session: sess,
                                            last_active: Instant::now(),
                                        }),
                                    );
                                    self.metrics.hs1_accepted.fetch_add(1, Ordering::Relaxed);
                                    Action::EstablishedThenFlush(d2, Vec::new())
                                } else {
                                    // Table pleine : la session n'est
                                    // pas installee, le hs2 n'est pas
                                    // emis — silence uniforme.
                                    self.metrics.rx_garbage.fetch_add(1, Ordering::Relaxed);
                                    Action::Silence
                                }
                            }
                            Err(_) => {
                                self.metrics.hs1_rejected.fetch_add(1, Ordering::Relaxed);
                                Action::Silence
                            }
                        }
                    }
                }
            }
        };

        match action {
            Action::Deliver(pt) => on_rx(src, &pt),
            Action::EstablishedThenFlush(hs2, frames) => {
                let raw = self.raw.clone();
                let metrics_out = !hs2.is_empty() || !frames.is_empty();
                if metrics_out {
                    tokio::spawn(async move {
                        if !hs2.is_empty() {
                            let _ = raw.send_to(src, &hs2).await;
                        }
                        for f in frames {
                            let _ = raw.send_to(src, &f).await;
                            tokio::task::yield_now().await;
                        }
                    });
                }
            }
            Action::Silence => {}
        }
    }

    /// Tick periodique : purge idle/pending, retry `hs1`, refill du
    /// plafond global, cover traffic optionnel.
    async fn tick(self: &Arc<Self>) {
        let now = Instant::now();
        let mut resend: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
        let mut cover: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            let cfg = &self.cfg;
            let metrics = &self.metrics;
            sessions.retain(|addr, st| {
                match st {
                    PeerState::Established(e) => {
                        let keep = now.duration_since(e.last_active)
                            < Duration::from_secs(cfg.session_idle_timeout_secs);
                        if !keep {
                            metrics.sessions_expired.fetch_add(1, Ordering::Relaxed);
                        }
                        keep
                    }
                    PeerState::Pending(p) => {
                        let timed_out = now.duration_since(p.started)
                            >= Duration::from_secs(cfg.pending_timeout_secs)
                            || p.attempts >= cfg.hs_attempts_max;
                        if timed_out {
                            metrics.sessions_expired.fetch_add(1, Ordering::Relaxed);
                            metrics
                                .queue_dropped
                                .fetch_add(p.queue.len() as u64, Ordering::Relaxed);
                            return false;
                        }
                        if now.duration_since(p.last_sent) >= Duration::from_secs(cfg.hs_retry_secs)
                        {
                            match hs1_seal(
                                &p.x,
                                &p.bridge_pk,
                                &cfg.client_id,
                                now_ts(),
                                &cfg.params,
                            ) {
                                Ok((d1, _)) => {
                                    // ctx inchange : rep_x identique
                                    // (meme ephemere x) — le nouveau
                                    // datagramme n'a que ts/pad frais.
                                    p.attempts += 1;
                                    p.last_sent = now;
                                    resend.push((*addr, d1));
                                    metrics.hs1_sent.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        true
                    }
                }
            });

            // Cover traffic : une trame `inner=vide` vers chaque
            // session etablie a cadence randomisee — le peer l'ouvre
            // et ignore (no-op applicatif).
            if self.cfg.cover_traffic {
                let (lo, hi) = self.cfg.cover_interval_ms;
                let span = hi.saturating_sub(lo).max(1);
                let mut b = [0u8; 8];
                rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
                let due = lo + u64::from_le_bytes(b) % span;
                for (addr, st) in sessions.iter_mut() {
                    if let PeerState::Established(e) = st {
                        // frequence ~1 trame par `due` ms moyen —
                        // decidee par tirage uniforme sur le tick.
                        let mut b2 = [0u8; 8];
                        rand::Rng::fill_bytes(&mut rand::rng(), &mut b2);
                        if u64::from_le_bytes(b2)
                            % (due.max(self.cfg.tick_ms) / self.cfg.tick_ms.max(1).max(1))
                            == 0
                        {
                            if let Ok(f) = e.session.seal_frame(&[]) {
                                cover.push((*addr, f));
                            }
                        }
                    }
                }
            }
        }

        // Refill du plafond global (par seconde, prorata tick).
        let refill = (self.cfg.hs1_global_per_sec as u64 * self.cfg.tick_ms).div_ceil(1000);
        self.global_tokens.fetch_add(refill, Ordering::Relaxed);
        // Plafonne le reservoir a une rafale de refill (anti-accumul.).
        let cap = refill.max(1);
        let _ = self
            .global_tokens
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                Some(t.min(cap.max(self.cfg.hs1_global_per_sec as u64)))
            });

        let n_cover = cover.len() as u64;
        for (addr, dgram) in resend.into_iter().chain(cover) {
            let _ = self.raw.send_to(addr, &dgram).await;
            tokio::task::yield_now().await;
        }
        if n_cover > 0 {
            self.metrics
                .cover_sent
                .fetch_add(n_cover, Ordering::Relaxed);
        }
    }
}

impl DatagramTransport for StealthTransport {
    fn send_to<'a>(&'a self, dst: SocketAddr, data: &'a [u8]) -> BoxFut<'a, Result<(), Ipv8Error>> {
        Box::pin(async move {
            enum Out {
                /// Trame a emettre immediatement.
                Frame(Vec<u8>),
                /// Nouveau handshake : `hs1` a emettre (data mise en
                /// file, elle partira au flush post-`hs2`).
                Handshake(Vec<u8>),
                /// Drop local — destination hors session/ponts (kill
                /// switch : jamais de clair).
                Drop,
            }
            let out = {
                let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
                match sessions.get_mut(&dst) {
                    Some(PeerState::Established(e)) => {
                        e.last_active = Instant::now();
                        match e.session.seal_frame(data) {
                            Ok(f) => {
                                self.metrics.frames_out.fetch_add(1, Ordering::Relaxed);
                                Out::Frame(f)
                            }
                            Err(_) => {
                                self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                                Out::Drop
                            }
                        }
                    }
                    Some(PeerState::Pending(p)) => {
                        if p.queue.len() >= self.cfg.pre_hs_queue_max
                            || p.queue_bytes + data.len() > self.cfg.pre_hs_queue_bytes
                        {
                            self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                        } else {
                            p.queue_bytes += data.len();
                            p.queue.push_back(data.to_vec());
                        }
                        Out::Drop
                    }
                    None => {
                        if !self.cfg.role.initiates() || sessions.len() >= self.cfg.max_sessions {
                            self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                            Out::Drop
                        } else if let Some(&pk) = self
                            .bridge_pks
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get(&dst)
                        {
                            let x = HiddenEph::generate();
                            match hs1_seal(&x, &pk, &self.cfg.client_id, now_ts(), &self.cfg.params)
                            {
                                Ok((d1, ctx)) => {
                                    let mut queue = VecDeque::new();
                                    queue.push_back(data.to_vec());
                                    sessions.insert(
                                        dst,
                                        PeerState::Pending(Pending {
                                            x,
                                            ctx,
                                            bridge_pk: pk,
                                            attempts: 1,
                                            last_sent: Instant::now(),
                                            started: Instant::now(),
                                            queue,
                                            queue_bytes: data.len(),
                                        }),
                                    );
                                    self.metrics.hs1_sent.fetch_add(1, Ordering::Relaxed);
                                    Out::Handshake(d1)
                                }
                                Err(_) => {
                                    self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                                    Out::Drop
                                }
                            }
                        } else {
                            self.metrics.queue_dropped.fetch_add(1, Ordering::Relaxed);
                            Out::Drop
                        }
                    }
                }
            };
            match out {
                Out::Frame(f) | Out::Handshake(f) => {
                    self.raw.send_to(dst, &f).await?;
                    Ok(())
                }
                Out::Drop => Ok(()),
            }
        })
    }

    fn run(self: Arc<Self>, on_rx: RxHandler) -> BoxFut<'static, Result<(), Ipv8Error>> {
        Box::pin(async move {
            // Tache de tick : purge/retry/refill/cover.
            let me = self.clone();
            let ticker = tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_millis(me.cfg.tick_ms.max(10)));
                loop {
                    iv.tick().await;
                    me.tick().await;
                }
            });
            // La boucle brute remonte la forme filaire ; `on_wire`
            // demorph puis remonte le plaintext a l'endpoint.
            let me2 = self.clone();
            let wire_rx: RxHandler = Arc::new(move |src: SocketAddr, data: &[u8]| {
                me2.on_wire(src, data, &on_rx);
            });
            let res = self.raw.clone().run(wire_rx).await;
            ticker.abort();
            res
        })
    }

    fn local_addr(&self) -> Result<SocketAddr, Ipv8Error> {
        self.raw.local_addr()
    }

    fn local_addr_v6(&self) -> Option<Result<SocketAddr, Ipv8Error>> {
        self.raw.local_addr_v6()
    }

    /// `(up, down)` **filaires** — octets morphes, pas plaintext.
    fn bytes_counters(&self) -> (u64, u64) {
        self.raw.bytes_counters()
    }

    /// Tap des datagrammes filaires morphes (oracle PCAP interne).
    fn set_tap(&self) -> broadcast::Receiver<TapEvent> {
        self.raw.set_tap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
    use tokio::sync::mpsc;

    /// Config de test : boucles serrees, fenetres reduites.
    fn cfg(role: StealthRole) -> (StealthConfig, Option<(LibNaClSecretKey, [u8; 32])>) {
        let bridge = if role.accepts_hs1() {
            let k = LibNaClSecretKey::generate();
            let pk = *k.public_key().crypt_x25519().as_bytes();
            Some((k, pk))
        } else {
            None
        };
        let mut c = StealthConfig {
            role,
            tick_ms: 50,
            hs_retry_secs: 1,
            pending_timeout_secs: 5,
            session_idle_timeout_secs: 2,
            ..Default::default()
        };
        if let Some((k, pk)) = &bridge {
            c.bridge_sk = Some(*k.crypt_x25519().as_bytes());
            c.bridge_pk = Some(*pk);
        }
        (c, bridge)
    }

    /// Lance le transport + run() ; retourne le transport et un
    /// receveur de plaintext applicatif.
    async fn spawn(
        cfg: StealthConfig,
    ) -> (
        Arc<StealthTransport>,
        mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>,
        tokio::task::JoinHandle<()>,
    ) {
        let t = StealthTransport::bind("127.0.0.1:0", None, cfg)
            .await
            .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let h: RxHandler = Arc::new(move |src, data| {
            let _ = tx.send((src, data.to_vec()));
        });
        let me = t.clone();
        let j = tokio::spawn(async move {
            let _ = me.run(h).await;
        });
        (t, rx, j)
    }

    /// Attend un plaintext avec timeout.
    async fn recv_timeout(
        rx: &mut mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>,
        ms: u64,
    ) -> Option<(SocketAddr, Vec<u8>)> {
        tokio::time::timeout(Duration::from_millis(ms), rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Socket UDP brut pour envoyer des datagrammes arbitraires.
    async fn raw_sock() -> tokio::net::UdpSocket {
        tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap()
    }

    #[tokio::test]
    async fn handshake_client_pont_puis_donnees() {
        let (b_cfg, _bk) = cfg(StealthRole::Bridge);
        let (bridge, mut rx_b, _jb) = spawn(b_cfg).await;
        let b_addr = bridge.local_addr().unwrap();

        let (mut c_cfg, _ck) = cfg(StealthRole::Client);
        c_cfg.bridges = vec![BridgeEntry {
            addr: b_addr,
            pk: _bk.as_ref().unwrap().1,
        }];
        let (client, mut rx_c, _jc) = spawn(c_cfg).await;

        // Premier envoi → declenche hs1, queue, hs2, flush.
        client.send_to(b_addr, b"bonjour-le-pont").await.unwrap();
        let got = recv_timeout(&mut rx_b, 2000)
            .await
            .expect("le pont doit recevoir le plaintext");
        assert_eq!(got.1, b"bonjour-le-pont");
        assert_eq!(client.session_count(), 1);
        assert_eq!(bridge.session_count(), 1);

        // Reponse pont → client.
        bridge
            .send_to(client.local_addr().unwrap(), b"reponse")
            .await
            .unwrap();
        let got = recv_timeout(&mut rx_c, 2000)
            .await
            .expect("le client doit recevoir la reponse");
        assert_eq!(got.1, b"reponse");
    }

    #[tokio::test]
    async fn probing_garbage_silence_total_et_zero_amplification() {
        let (b_cfg, _bk) = cfg(StealthRole::Bridge);
        let (bridge, _rx, _j) = spawn(b_cfg).await;
        let b_addr = bridge.local_addr().unwrap();
        let sock = raw_sock().await;

        let up0 = bridge.bytes_counters().0;
        let down0 = bridge.bytes_counters().1;
        // 1000 datagrammes aleatoires de taille fixe (96 o) — le
        // compteur filaire permet de compter les recus malgre les
        // drops noyau de la rafale.
        let mut garbage = vec![0u8; 96];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut garbage);
        for _ in 0..1000usize {
            sock.send_to(&garbage, b_addr).await.unwrap();
        }
        // Laisse tout traiter.
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Zéro réponse sur le socket sondant.
        let mut buf = [0u8; 2048];
        let res = tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await;
        assert!(res.is_err(), "le pont a repondu a du garbage");
        // Zéro session, zéro octet emis → amplification = 0.
        assert_eq!(bridge.session_count(), 0);
        assert_eq!(bridge.bytes_counters().0, up0);
        assert_eq!(bridge.metrics().hs1_accepted.load(Ordering::Relaxed), 0);
        let m = bridge.metrics();
        let rejected = m.hs1_rejected.load(Ordering::Relaxed)
            + m.rl_ip_dropped.load(Ordering::Relaxed)
            + m.rl_global_dropped.load(Ordering::Relaxed)
            + m.rx_garbage.load(Ordering::Relaxed);
        // Chaque datagramme recu est compte comme rejete — silence
        // total quelle que soit la cause (meme droppes noyau exclus).
        let received = (bridge.bytes_counters().1 - down0) / 96;
        assert_eq!(rejected, received, "un recu n.est pas rejete");
        assert!(received > 0, "le banc n.a rien mesure");
    }

    #[tokio::test]
    async fn hs1_rejoue_une_seule_reponse() {
        // Un capteur rejoue le hs1 capture : le filtre X' rend la
        // deuxieme tentative silencieuse (anti-probing par rejeu).
        let (b_cfg, bk) = cfg(StealthRole::Bridge);
        let b_pk = bk.as_ref().unwrap().1;
        let (bridge, _rx, _j) = spawn(b_cfg).await;
        let b_addr = bridge.local_addr().unwrap();
        let sock = raw_sock().await;

        // Forge un hs1 valide hors transport (cle connue).
        let x = HiddenEph::generate();
        let (d1, _ctx) =
            hs1_seal(&x, &b_pk, b"capteur", now_ts(), &StealthParams::default()).unwrap();
        sock.send_to(&d1, b_addr).await.unwrap();
        let mut buf = [0u8; 2048];
        let r1 = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await;
        assert!(r1.is_ok(), "premier hs1 valide doit obtenir hs2");
        // Rejeu exact → silence.
        sock.send_to(&d1, b_addr).await.unwrap();
        let r2 = tokio::time::timeout(Duration::from_millis(400), sock.recv_from(&mut buf)).await;
        assert!(r2.is_err(), "rejeu de hs1 doit etre silencieux");
        assert_eq!(bridge.metrics().hs1_accepted.load(Ordering::Relaxed), 1);
        // Compteur filaire emis = 1 datagramme (le hs2).
        let (n, _src) = r1.unwrap().unwrap();
        assert_eq!(bridge.bytes_counters().0 as usize, n);
    }

    #[tokio::test]
    async fn rejeu_exact_de_trame_et_reordonnancement() {
        let (b_cfg, bk) = cfg(StealthRole::Bridge);
        let (bridge, mut rx_b, _jb) = spawn(b_cfg).await;
        let b_addr = bridge.local_addr().unwrap();

        let (mut c_cfg, _ck) = cfg(StealthRole::Client);
        c_cfg.bridges = vec![BridgeEntry {
            addr: b_addr,
            pk: bk.unwrap().1,
        }];
        let (client, _rx_c, _jc) = spawn(c_cfg).await;
        client.send_to(b_addr, b"open").await.unwrap();
        recv_timeout(&mut rx_b, 2000).await.expect("session");

        // Tap filaire : rejoue la trame suivante en double et en
        // desordre — la fenetre rejette le duplicat, accepte le
        // desordre.
        client.send_to(b_addr, b"f1").await.unwrap();
        client.send_to(b_addr, b"f2").await.unwrap();
        let a = recv_timeout(&mut rx_b, 2000).await.unwrap().1;
        let b2 = recv_timeout(&mut rx_b, 2000).await.unwrap().1;
        assert_eq!([a.as_slice(), b2.as_slice()].as_slice(), &[b"f1", b"f2"]);
    }

    #[tokio::test]
    async fn file_pre_handshake_bornee_et_dest_inconnu_drop() {
        let (mut c_cfg, _ck) = cfg(StealthRole::Client);
        let fake_bridge: SocketAddr = "127.0.0.1:9".parse().unwrap(); // discard
        c_cfg.bridges = vec![BridgeEntry {
            addr: fake_bridge,
            pk: [7u8; 32],
        }];
        c_cfg.pre_hs_queue_max = 8;
        let attempts_max = c_cfg.hs_attempts_max as u64;
        let (client, _rx, _j) = spawn(c_cfg).await;

        // Destination inconnue → drop silencieux, pas de session.
        let anywhere: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        client.send_to(anywhere, b"clairement-pas").await.unwrap();
        assert_eq!(client.session_count(), 0);
        assert_eq!(client.metrics().queue_dropped.load(Ordering::Relaxed), 1);
        let up0 = client.bytes_counters().0;

        // 20 datagrammes vers un pont qui ne repond pas : file 8 +
        // retries bornes, jamais de clair vers `anywhere`.
        for _ in 0..20 {
            client.send_to(fake_bridge, b"attente").await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        // Bytes emis = uniquement des hs1 morphes (retries), jamais le
        // plaintext — le kill switch tient.
        let m = client.metrics();
        assert!(m.queue_dropped.load(Ordering::Relaxed) >= 12);
        assert!(m.hs1_sent.load(Ordering::Relaxed) >= 1);
        let up = client.bytes_counters().0 - up0;
        assert!(
            up < 1280 * (attempts_max + 2),
            "trop d'octets emis pour des retries bornes: {up}"
        );
        // Apres pending_timeout, la session est purgee.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(client.session_count(), 0);
    }

    #[tokio::test]
    async fn nat_rebinding_client_nouvelle_session() {
        // Rebind du port source : l'ancienne session meurt en silence
        // chez le pont, le nouveau port refait un handshake.
        let (b_cfg, bk) = cfg(StealthRole::Bridge);
        let (bridge, mut rx_b, _jb) = spawn(b_cfg).await;
        let b_addr = bridge.local_addr().unwrap();

        let (mut c_cfg, _ck) = cfg(StealthRole::Client);
        c_cfg.bridges = vec![BridgeEntry {
            addr: b_addr,
            pk: bk.unwrap().1,
        }];
        let (c1, _r1, j1) = spawn(c_cfg.clone()).await;
        c1.send_to(b_addr, b"avant-rebind").await.unwrap();
        assert!(recv_timeout(&mut rx_b, 2000).await.is_some());
        let a1 = c1.local_addr().unwrap();
        j1.abort();
        drop(c1);

        // Nouveau bind (autre port) = NAT rebinding.
        let (c2, _r2, _j2) = spawn(c_cfg).await;
        let a2 = c2.local_addr().unwrap();
        assert_ne!(a1.port(), a2.port());
        c2.send_to(b_addr, b"apres-rebind").await.unwrap();
        let got = recv_timeout(&mut rx_b, 2000)
            .await
            .expect("rebind doit re-handshaker");
        assert_eq!(got.1, b"apres-rebind");
        assert_eq!(got.0.port(), a2.port());
    }

    #[test]
    fn liens_bridge_parse_serialize_hostile() {
        let pk = [0xABu8; 32];
        let e = BridgeEntry {
            addr: "192.0.2.1:8443".parse().unwrap(),
            pk,
        };
        let link = e.to_link();
        assert_eq!(
            link,
            format!("onionbit-bridge://192.0.2.1:8443#{}", hex::encode(pk))
        );
        let back = BridgeEntry::parse_link(&link).unwrap();
        assert_eq!(back.addr, e.addr);
        assert_eq!(back.pk, pk);
        // IPv6 entre crochets.
        let v6 =
            BridgeEntry::parse_link(&format!("onionbit-bridge://[::1]:443#{}", hex::encode(pk)))
                .unwrap();
        assert!(v6.addr.is_ipv6());

        // Hostile : tout ecart → Err, jamais de valeur par defaut.
        let hex_pk = hex::encode(pk);
        for bad in [
            "",
            "onionbit-bridge://",
            "http://1.2.3.4:80#key",
            &format!("onionbit-bridge://192.0.2.1:8443#{hex_pk}x"), // 65 hex
            &format!("onionbit-bridge://192.0.2.1:8443#{}", &hex_pk[..62]), // tronquee
            &format!("onionbit-bridge://192.0.2.1:8443#{hex_pk}ff"),
            "onionbit-bridge://192.0.2.1:8443", // pas de #
            "onionbit-bridge://:8443#",         // vide
            &format!("onionbit-bridge://192.0.2.1:0#{hex_pk}"), // port 0
            &format!("onionbit-bridge://pas-une-ip:8443#{hex_pk}"),
            &format!("onionbit-bridge://192.0.2.1:99999#{hex_pk}"),
            &format!(
                "onionbit-bridge://192.0.2.1:8443#{}",
                hex_pk.to_uppercase().replace("AB", "ZZ") // non-hex
            ),
            &format!("ONIONBIT-BRIDGE://192.0.2.1:8443#{hex_pk}"), // casse scheme
            &format!(" onionbit-bridge://192.0.2.1:8443#{hex_pk}"), // espace
        ] {
            assert!(
                BridgeEntry::parse_link(bad).is_err(),
                "lien hostile accepte: {bad:?}"
            );
        }
    }
}
