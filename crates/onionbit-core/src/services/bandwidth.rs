// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contrôleur de congestion du débit servi — extension Rust sans
//! équivalent pyipv8 (qui ne borne jamais le débit servi).
//!
//! Famille LEDBAT / « Upload Speed Sense » d'eMule : plutôt que
//! d'estimer la capacité upload (impossible à faire fiablement sans
//! sonde externe — le pic passif est censuré par son propre plafond,
//! l'UPnP dépend de l'IGD, une sonde HTTP fuit vers un tiers), on
//! détecte la **saturation** de la ligne montante : quand la file
//! d'attente du routeur gonfle, le RTT vers les pairs s'inflate.
//!
//! À chaque tick `sample_secs` : rafale de `ping` Discovery vers
//! `probe_peers` pairs vérifiés, **minimum** des RTT collectés,
//! retard de file = min − baseline (min glissant sur
//! `base_window_secs`). Le min isole la congestion *locale* : la
//! file d'émission est commune à tous les paquets sortants — si
//! notre uplink sature, tous les RTT s'inflatent, y compris le
//! meilleur ; un pair lointain ou saturé de son côté ne fausse
//! plus le signal. AIMD :
//!
//! - retard ≤ `target_delay_ms` → `cap += max(cap / increase_div,
//!   increase_min_bps)` (montée progressive) ;
//! - retard > `target_delay_ms` → `cap = cap × decrease_pct/100`
//!   (repli rapide), borné par `floor_bps` ;
//! - aucun échantillon → plafond inchangé (un pair muet n'est pas
//!   une congestion) ; aucun pair éligible → `fallback_bps`.
//!
//! Le plafond est borné dans `[floor_bps, max_bps]` et appliqué à
//! [`TunnelCommunity::set_relay_rate_bps`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_ipv8::UdpAddress;

use crate::daemon_config::BandwidthConfig;
use crate::CoreSession;

/// Registre des pings en vol émis par la tâche de contrôle. La
/// `DiscoveryCommunity` notifie `(adresse, identifier)` de chaque
/// `pong` reçu — les ids inconnus (pings du churn, keepalives) sont
/// ignorés.
pub struct RttProbe {
    /// `(adresse, identifier)` → instant d'émission.
    pending: Mutex<HashMap<(UdpAddress, u16), Instant>>,
    /// RTT (ms) des pongs reçus pour le tick courant.
    done: Mutex<Vec<f64>>,
}

impl Default for RttProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl RttProbe {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            done: Mutex::new(Vec::new()),
        }
    }

    /// Réinitialise la rafale : les pings émis avant ne seront plus
    /// comptés (pongs tardifs = bruit filtré).
    pub fn clear(&self) {
        self.pending.lock().unwrap().clear();
        self.done.lock().unwrap().clear();
    }

    /// Enregistre un ping émis (`send_ping` a renvoyé `id`).
    pub fn register(&self, addr: &UdpAddress, id: u16) {
        self.pending
            .lock()
            .unwrap()
            .insert((addr.clone(), id), Instant::now());
    }

    /// Callback `pong` branché sur `DiscoveryCommunity::set_pong_probe`.
    pub fn on_pong(&self, addr: &UdpAddress, id: u16) {
        let sent = self.pending.lock().unwrap().remove(&(addr.clone(), id));
        if let Some(t) = sent {
            self.done
                .lock()
                .unwrap()
                .push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Vide et renvoie les RTT (ms) collectés pour le tick.
    pub fn take(&self) -> Vec<f64> {
        std::mem::take(&mut *self.done.lock().unwrap())
    }
}

/// Vue instantanée du contrôleur (endpoint diagnostics).
#[derive(Debug, Clone)]
pub struct BandwidthSnapshot {
    /// Plafond servi actuellement appliqué au tunnel (octets/s).
    pub effective_relay_bps: u64,
    /// Baseline RTT (ms) — min glissant des minimums de rafale sur
    /// `base_window_secs` ; `None` tant qu'aucun pong n'a été reçu.
    pub base_rtt_ms: Option<f64>,
    /// RTT minimum de la dernière rafale (ms) ; `None` idem.
    pub min_rtt_ms: Option<f64>,
    /// Nombre de pongs exploités au dernier tick.
    pub rtt_samples: usize,
    /// Datagrammes servis perdus faute de budget (`relay_rate_dropped`).
    pub relay_dropped: u64,
}

/// Contrôleur AIMD du plafond servi (état partagé session).
pub struct CongestionController {
    /// Plafond actuellement appliqué (octets/s) — `0` = pas encore.
    applied: AtomicU64,
    /// RTT minimum par tick pour la baseline `(instant, ms)`.
    mins: Mutex<VecDeque<(Instant, f64)>>,
    /// Minimum + nombre d'échantillons du dernier tick (diagnostic).
    last: Mutex<Option<(f64, usize)>>,
}

impl Default for CongestionController {
    fn default() -> Self {
        Self::new()
    }
}

impl CongestionController {
    pub fn new() -> Self {
        Self {
            applied: AtomicU64::new(0),
            mins: Mutex::new(VecDeque::new()),
            last: Mutex::new(None),
        }
    }

    /// Plafond actuellement en vigueur (dernier `update` émis ou
    /// valeur notée en mode fixe).
    pub fn applied_bps(&self) -> u64 {
        self.applied.load(Ordering::Relaxed)
    }

    /// Plafond courant, ou `fallback_bps` borné si rien n'a encore
    /// été appliqué (lecture pour `apply_service_settings`).
    pub fn current_bps(&self, cfg: &BandwidthConfig) -> u64 {
        let cur = self.applied.load(Ordering::Relaxed);
        if cur > 0 {
            cur
        } else {
            cfg.fallback_bps.clamp(cfg.floor_bps, cfg.max_bps)
        }
    }

    /// Enregistre le plafond servi en vigueur en mode fixe/illimité
    /// (`max_relayed_rate >= 0`) — la tâche ne calcule pas
    /// `update` dans ce mode mais le diagnostic doit refléter la
    /// valeur réellement appliquée (`0` = illimité).
    pub fn note_applied(&self, bps: u64) {
        self.applied.store(bps, Ordering::Relaxed);
    }

    /// Décision d'un tick : `samples` = RTT (ms) de la rafale de
    /// pings. Le signal retenu est le **minimum** : la file
    /// d'émission locale étant commune à tous les paquets, notre
    /// congestion gonfle tous les RTT, tandis qu'un pair lointain
    /// ou saturé de son côté n'affecte pas le meilleur échantillon.
    /// Sans échantillon, le plafond courant est conservé (un pair
    /// muet n'est pas une preuve de congestion). Renvoie le plafond
    /// à appliquer.
    pub fn update(&self, samples: &[f64], cfg: &BandwidthConfig) -> u64 {
        let cur = self.current_bps(cfg);
        let Some(min_rtt) = min_of(samples) else {
            self.applied.store(cur, Ordering::Relaxed);
            return cur;
        };

        let mut window = self.mins.lock().unwrap();
        window.push_back((Instant::now(), min_rtt));
        let horizon = Duration::from_secs(cfg.base_window_secs);
        while window.front().is_some_and(|(t, _)| t.elapsed() > horizon) {
            window.pop_front();
        }
        let base = window.iter().map(|(_, m)| *m).fold(f64::INFINITY, f64::min);
        let queue_delay = (min_rtt - base).max(0.0);

        let cap = if queue_delay <= cfg.target_delay_ms as f64 {
            // Ligne tranquille : montée additive.
            cur.saturating_add((cur / cfg.increase_div).max(cfg.increase_min_bps))
        } else {
            // File d'attente qui gonfle : repli multiplicatif.
            (cur * cfg.decrease_pct / 100).max(cfg.floor_bps)
        }
        .clamp(cfg.floor_bps, cfg.max_bps);

        *self.last.lock().unwrap() = Some((min_rtt, samples.len()));
        self.applied.store(cap, Ordering::Relaxed);
        cap
    }

    /// Vue pour `/api/statistics` — `dropped` est le compteur de
    /// pertes du limiteur tunnel (passé par l'appelant).
    pub fn snapshot(&self, dropped: u64) -> BandwidthSnapshot {
        let window = self.mins.lock().unwrap();
        let base = window.iter().map(|(_, m)| *m).fold(f64::INFINITY, f64::min);
        let last = *self.last.lock().unwrap();
        BandwidthSnapshot {
            effective_relay_bps: self.applied.load(Ordering::Relaxed),
            base_rtt_ms: base.is_finite().then_some(base),
            min_rtt_ms: last.map(|(m, _)| m),
            rtt_samples: last.map(|(_, n)| n).unwrap_or(0),
            relay_dropped: dropped,
        }
    }
}

/// Minimum d'un lot d'échantillons (`None` si vide).
fn min_of(samples: &[f64]) -> Option<f64> {
    samples.iter().copied().reduce(f64::min)
}

/// Boucle périodique du contrôleur — spawnée par la session quand
/// `ipv8/enabled`. À chaque tick `sample_secs` : rafale de pings
/// Discovery vers les pairs vérifiés les plus frais, attente
/// `probe_wait_ms`, puis décision AIMD appliquée à chaud. En mode
/// `max_relayed_rate >= 0` la valeur fixe n'est pas recalculée mais
/// republiée pour le diagnostic.
pub async fn run_bandwidth_task(
    session: CoreSession,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) {
    let ctrl = session.bandwidth();
    let probe = Arc::new(RttProbe::new());
    if let Some(stack) = session.ipv8() {
        let p = probe.clone();
        stack
            .discovery
            .set_pong_probe(Arc::new(move |addr, id| p.on_pong(addr, id)));
    }
    let mut tick = tokio::time::interval(Duration::from_secs(
        session.config().ipv8.bandwidth.sample_secs.max(1),
    ));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {}
        }
        let Some(stack) = session.ipv8() else {
            continue;
        };
        let cfg = session.effective_config().ipv8;
        // `-1` (auto) : contrôleur AIMD. `0`/`>0` : valeur fixe posée
        // par `apply_service_settings` — republiée pour le diagnostic
        // (`0` = illimité), jamais recalculée ici.
        if cfg.max_relayed_bps >= 0 {
            ctrl.note_applied(cfg.max_relayed_bps.max(0) as u64);
            continue;
        }
        let Some(tunnel) = stack.tunnel.as_ref() else {
            continue;
        };

        // Pairs éligibles : vérifiés et adressés — les plus frais en
        // priorité (dédoublonnés par adresse : plusieurs clés peuvent
        // partager une même socket).
        let mut peers: Vec<_> = stack
            .network
            .verified_peers()
            .into_iter()
            .filter_map(|p| {
                let age = p.last_response_elapsed();
                p.address.map(|a| (age, a))
            })
            .collect();
        peers.sort_by_key(|(age, _)| *age);
        let mut seen = HashSet::new();
        let addrs: Vec<UdpAddress> = peers
            .into_iter()
            .filter_map(|(_, addr)| seen.insert(addr.clone()).then_some(addr))
            .take(cfg.bandwidth.probe_peers)
            .collect();
        if addrs.is_empty() {
            // Aucun pair à sonder : plafond de repli tant que le
            // réseau n'offre pas de signal.
            let cap = cfg
                .bandwidth
                .fallback_bps
                .clamp(cfg.bandwidth.floor_bps, cfg.bandwidth.max_bps);
            ctrl.note_applied(cap);
            tunnel.set_relay_rate_bps(cap);
            continue;
        }

        probe.clear();
        let mut sent = 0usize;
        for addr in &addrs {
            if let Ok(id) = stack.discovery.send_ping(addr).await {
                probe.register(addr, id);
                sent += 1;
            }
        }
        if sent == 0 {
            continue;
        }
        tokio::select! {
            _ = stop.changed() => break,
            _ = tokio::time::sleep(Duration::from_millis(cfg.bandwidth.probe_wait_ms)) => {}
        }
        let cap = ctrl.update(&probe.take(), &cfg.bandwidth);
        tunnel.set_relay_rate_bps(cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BandwidthConfig {
        BandwidthConfig::default()
    }

    /// Sans échantillon : le plafond reste la valeur courante (ou le
    /// repli borné au premier appel) — aucune lecture ne peut prouver
    /// la congestion.
    #[test]
    fn sans_echantillon_plafond_inchange() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        // Jamais appliqué : repli initial.
        assert_eq!(ctrl.update(&[], &bw), bw.fallback_bps);
        // Puis stable tant qu'aucun pong n'arrive.
        assert_eq!(ctrl.update(&[], &bw), bw.fallback_bps);
    }

    /// Baseline stable : le plafond monte additivement, borné par
    /// `max_bps`.
    #[test]
    fn ligne_tranquille_montee_additive() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        let mut cap = ctrl.update(&[30.0, 32.0, 28.0], &bw);
        let first = cap;
        for _ in 0..10 {
            cap = ctrl.update(&[30.0, 32.0, 28.0], &bw);
            assert!(cap > first);
        }
        for _ in 0..500 {
            cap = ctrl.update(&[30.0, 32.0, 28.0], &bw);
        }
        assert_eq!(cap, bw.max_bps);
    }

    /// Congestion : RTT au-dessus de la baseline + cible → repli
    /// multiplicatif jusqu'au plancher.
    #[test]
    fn congestion_repli_multiplicatif() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        // Pose la baseline à ~30 ms puis gonfle le plafond.
        let mut cap = 0;
        for _ in 0..12 {
            cap = ctrl.update(&[30.0], &bw);
        }
        assert!(cap > bw.fallback_bps);
        // La file gonfle : +80 ms → > target_delay (50 ms).
        let degraded: Vec<f64> = vec![110.0, 115.0, 108.0];
        let mut cur = cap;
        for _ in 0..20 {
            let next = ctrl.update(&degraded, &bw);
            assert!(next <= cur);
            cur = next;
        }
        assert_eq!(cur, bw.floor_bps);
    }

    /// Le premier échantillon pose la baseline : un RTT élevé mais
    /// constant n'est PAS de la congestion (ligne naturellement
    /// lente / pairs lointains) — le plafond ne doit pas chuter sous
    /// le repli pour ça.
    #[test]
    fn rtt_eleve_stable_pas_de_congestion() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        let mut cap = ctrl.update(&[400.0, 410.0], &bw);
        for _ in 0..5 {
            cap = ctrl.update(&[400.0, 410.0], &bw);
        }
        assert!(cap >= bw.fallback_bps);
    }

    /// Pairs hétérogènes : seul le min compte — des pairs lointains
    /// ou saturés (gros RTT propres) ne déclenchent pas de repli
    /// tant qu'un chemin reste fluide.
    #[test]
    fn pairs_lointains_ne_penalisent_pas() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        // Baseline établie sur un chemin à ~30 ms.
        let mut cap = ctrl.update(&[30.0, 300.0, 450.0], &bw);
        // Les pairs lointains dérivent encore plus haut : leur
        // congestion à eux ne doit pas couler le plafond.
        for _ in 0..10 {
            let next = ctrl.update(&[30.0, 380.0, 900.0], &bw);
            assert!(next > cap);
            cap = next;
        }
    }

    /// Congestion réelle : tous les échantillons s'inflatent — la
    /// file locale est commune, même le meilleur chemin est retardé.
    #[test]
    fn inflation_globale_replie() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        let mut cap = 0;
        for _ in 0..8 {
            cap = ctrl.update(&[30.0, 45.0, 60.0], &bw);
        }
        // Tous les pairs voient +100 ms → congestion locale.
        let next = ctrl.update(&[130.0, 145.0, 160.0], &bw);
        assert!(next < cap);
    }

    /// Minimum d'un lot : le pire pair n'influe pas ; vide → None.
    #[test]
    fn min_d_echantillons() {
        assert_eq!(min_of(&[10.0, 12.0, 500.0]), Some(10.0));
        assert_eq!(min_of(&[]), None);
        assert_eq!(min_of(&[42.0]), Some(42.0));
    }

    /// `RttProbe` : seuls les `(adresse, id)` enregistrés produisent
    /// un échantillon ; `clear` invalide les pongs tardifs.
    #[test]
    fn sonde_apparie_et_filtre() {
        let probe = RttProbe::new();
        let a = UdpAddress::from("1.2.3.4:7759".parse::<std::net::SocketAddr>().unwrap());
        let b = UdpAddress::from("5.6.7.8:7759".parse::<std::net::SocketAddr>().unwrap());
        probe.register(&a, 42);
        probe.on_pong(&b, 42); // mauvaise adresse → ignoré
        probe.on_pong(&a, 43); // mauvais id → ignoré
        probe.on_pong(&a, 42); // apparié
        let rtts = probe.take();
        assert_eq!(rtts.len(), 1);
        assert_eq!(probe.take().len(), 0); // vidé
        probe.register(&a, 7);
        probe.clear();
        probe.on_pong(&a, 7); // tardif → filtré
        assert!(probe.take().is_empty());
    }

    /// `note_applied` / `current_bps` : mode fixe republié, repli
    /// borné quand rien n'a été appliqué.
    #[test]
    fn note_applied_et_repli() {
        let ctrl = CongestionController::new();
        let bw = cfg();
        assert_eq!(ctrl.current_bps(&bw), bw.fallback_bps);
        ctrl.note_applied(0);
        assert_eq!(ctrl.applied_bps(), 0); // 0 = illimité publié tel quel
        ctrl.note_applied(200_000);
        assert_eq!(ctrl.current_bps(&bw), 200_000);
    }
}
