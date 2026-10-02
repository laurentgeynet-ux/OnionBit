// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Estimateur de capacité upload — extension Rust sans équivalent
//! pyipv8 (qui ne borne jamais le débit servi).
//!
//! Quand `tunnel_community/max_relayed_rate = -1` (mode auto), le
//! plafond appliqué à [`TunnelCommunity::set_relay_rate_bps`] est
//! `max(upload_mesuré × bandwidth/share, floor_bps)`. Le trafic servi
//! étant symétrique (chaque datagramme relayé = 1 réception + 1
//! émission), baser le plafond sur l'upload — le facteur limitant des
//! lignes grand public — borne aussi le download consommé.
//!
//! Sources de mesure, la meilleure disponible l'emporte :
//!
//! - **UPnP** (`WANCommonInterfaceConfig:GetLinkLayerMaxBitRates`) :
//!   débit WAN provisionné lu sur le routeur — trafic LAN uniquement,
//!   même canal que la redirection de port déjà utilisée ;
//! - **sonde upload** (`bandwidth/probe_up_urls`) : POST d'un blob
//!   vers des endpoints configurés — opt-in, liste vide = aucun
//!   trafic sortant ; soumise à `ip_policy` (anti-SSRF) ;
//! - **pic passif** : débit upload soutenu maximal observé sur
//!   l'endpoint UDP pendant les transferts réels — borne basse,
//!   toujours disponible.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::daemon_config::BandwidthConfig;
use crate::CoreSession;

/// Boucle périodique de l'estimateur — spawnée par la session quand
/// `ipv8/enabled` (l'endpoint existe). Après `warmup_secs` : mesure
/// initiale (UPnP puis sondes opt-in), puis à chaque tick
/// `sample_secs` : pic passif des compteurs + re-mesure selon
/// `measure_interval_secs` + application du plafond servi en mode
/// `max_relayed_rate = -1`.
pub async fn run_bandwidth_task(
    session: CoreSession,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) {
    let est = session.bandwidth();
    let warmup = Duration::from_secs(session.config().ipv8.bandwidth.warmup_secs);
    tokio::select! {
        _ = stop.changed() => return,
        _ = tokio::time::sleep(warmup) => {}
    }
    {
        let cfg = session.effective_config().ipv8.bandwidth;
        est.measure(&cfg, &session.config().ip_policy).await;
    }
    let mut tick = tokio::time::interval(Duration::from_secs(
        session.config().ipv8.bandwidth.sample_secs.max(1),
    ));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_measure = Instant::now();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {}
        }
        let Some(stack) = session.ipv8() else {
            continue;
        };
        let (up_total, _down_total) = stack.endpoint.bytes_counters();
        est.sample_counters(up_total);
        let cfg = session.effective_config().ipv8;
        if last_measure.elapsed() >= Duration::from_secs(cfg.bandwidth.measure_interval_secs) {
            est.measure(&cfg.bandwidth, &session.config().ip_policy)
                .await;
            last_measure = Instant::now();
        }
        // `-1` (auto) : fraction de l'upload mesure appliquee a chaud.
        // `0`/`>0` : valeur fixe deja posee par le chemin apply
        // (`POST /api/settings`) — la tache n'y touche pas mais la
        // publie pour le diagnostic (sinon `effective_relay_bps`
        // resterait 0 alors qu'un plafond est bien en vigueur).
        if cfg.max_relayed_bps < 0 {
            if let Some(tunnel) = stack.tunnel.as_ref() {
                tunnel.set_relay_rate_bps(est.effective_bps(&cfg.bandwidth));
            }
        } else {
            est.note_applied(cfg.max_relayed_bps.max(0) as u64);
        }
    }
}

/// Type de service UPnP portant le débit WAN.
const WAN_CIC_SERVICE: &str = "urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1";

/// Origine de la mesure de capacité (diagnostic `/api/statistics`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateSource {
    /// Débit WAN lu sur le routeur (`GetLinkLayerMaxBitRates`).
    Upnp,
    /// Sonde HTTP POST configurée (`bandwidth/probe_up_urls`).
    Probe,
    /// Pic passif des compteurs endpoint (borne basse).
    Passive,
}

/// Vue instantanée de l'estimateur (endpoint diagnostics).
#[derive(Debug, Clone)]
pub struct BandwidthSnapshot {
    /// Capacité upload estimée (octets/s) — `0` = pas encore mesurée.
    pub up_bps: u64,
    /// Capacité download mesurée (octets/s) — remplie par la sonde
    /// UPnP (`NewDownstreamMaxBitRate`), `0` sinon.
    pub down_bps: u64,
    /// Source de la mesure active (`None` = aucune mesure encore).
    pub source: Option<&'static str>,
    /// Pic passif upload observé (octets/s).
    pub passive_peak_up_bps: u64,
    /// Plafond servi actuellement appliqué au tunnel (octets/s).
    pub effective_relay_bps: u64,
    /// Datagrammes servis perdus faute de budget (`relay_rate_dropped`).
    pub relay_dropped: u64,
}

/// Estimateur de capacité : mesures actives (UPnP/sonde) + pic passif
/// des compteurs endpoint.
pub struct BandwidthEstimator {
    /// Dernière mesure active `(up, down, source)`.
    measured: Mutex<Option<(u64, u64, EstimateSource)>>,
    /// Pic passif upload observé (octets/s).
    passive_peak_up: AtomicU64,
    /// Compteur endpoint `(instant, up_total)` du dernier échantillon.
    last_counters: Mutex<Option<(Instant, u64)>>,
    /// Plafond actuellement appliqué (dernier `effective_bps` émis).
    applied: AtomicU64,
}

impl Default for BandwidthEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl BandwidthEstimator {
    pub fn new() -> Self {
        Self {
            measured: Mutex::new(None),
            passive_peak_up: AtomicU64::new(0),
            last_counters: Mutex::new(None),
            applied: AtomicU64::new(0),
        }
    }

    /// Échantillonne les compteurs endpoint (delta/dt → pic passif).
    /// Appelé périodiquement par la tâche de mesure.
    pub fn sample_counters(&self, up_total: u64) {
        let mut last = self.last_counters.lock().unwrap();
        if let Some((t, prev_up)) = *last {
            let dt = t.elapsed().as_secs_f64();
            if dt > 0.1 && up_total >= prev_up {
                let rate = ((up_total - prev_up) as f64 / dt) as u64;
                self.passive_peak_up.fetch_max(rate, Ordering::Relaxed);
            }
        }
        *last = Some((Instant::now(), up_total));
    }

    /// Capacité upload estimée : meilleure de la mesure active et du
    /// pic passif (`None` tant que rien n'a été observé).
    pub fn estimated_up_bps(&self) -> Option<u64> {
        let measured = self.measured.lock().unwrap().map(|(up, _, _)| up);
        let passive = self.passive_peak_up.load(Ordering::Relaxed);
        match (measured, passive) {
            (m, 0) => m,
            (Some(m), p) => Some(m.max(p)),
            (None, p) => Some(p),
        }
    }

    /// Plafond servi à appliquer en mode auto :
    /// `max(upload × share, floor)` ; `fallback` tant que la capacité
    /// n'a jamais été mesurée.
    pub fn effective_bps(&self, bw: &BandwidthConfig) -> u64 {
        let rate = match self.estimated_up_bps() {
            Some(up) if up > 0 => ((up as f64 * bw.share) as u64).max(bw.floor_bps),
            _ => bw.fallback_bps,
        };
        self.applied.store(rate, Ordering::Relaxed);
        rate
    }

    /// Enregistre le plafond servi en vigueur en mode fixe/illimité
    /// (`max_relayed_rate >= 0`) — la tache ne calcule pas
    /// `effective_bps` dans ce mode mais le diagnostic doit refleter
    /// la valeur reellement appliquee (`0` = illimite).
    pub fn note_applied(&self, bps: u64) {
        self.applied.store(bps, Ordering::Relaxed);
    }

    /// Vue pour `/api/statistics` — `dropped` est le compteur de
    /// pertes du limiteur tunnel (passé par l'appelant).
    pub fn snapshot(&self, dropped: u64) -> BandwidthSnapshot {
        let m = *self.measured.lock().unwrap();
        let passive = self.passive_peak_up.load(Ordering::Relaxed);
        BandwidthSnapshot {
            // Meilleure estimation disponible (mesure active ou pic
            // passif) — sinon l'UI afficherait « mesure en cours » a
            // l'infini quand l'IGD ne repond pas (Freebox par ex.).
            up_bps: m.map(|(up, _, _)| up).unwrap_or(0).max(passive),
            down_bps: m.map(|(_, d, _)| d).unwrap_or(0),
            source: m
                .map(|(_, _, s)| match s {
                    EstimateSource::Upnp => "upnp",
                    EstimateSource::Probe => "probe",
                    EstimateSource::Passive => "passive",
                })
                // Seul le pic passif a ete observe : borne basse.
                .or((passive > 0).then_some("passive")),
            passive_peak_up_bps: passive,
            effective_relay_bps: self.applied.load(Ordering::Relaxed),
            relay_dropped: dropped,
        }
    }

    /// Cycle de mesure actif : UPnP puis sondes upload. La meilleure
    /// mesure disponible est conservée (une sonde en échec n'efface
    /// pas une mesure antérieure).
    pub async fn measure(
        &self,
        bw: &BandwidthConfig,
        ip_policy: &onionbit_network_policy::IpPolicy,
    ) {
        if bw.measure_upnp {
            if let Some((up, down)) =
                measure_upnp_wan(Duration::from_secs(bw.probe_timeout_secs)).await
            {
                *self.measured.lock().unwrap() = Some((up, down, EstimateSource::Upnp));
                tracing::info!(
                    up_bps = up,
                    down_bps = down,
                    "capacite WAN mesuree via UPnP"
                );
            } else {
                // Silencieux sinon : un IGD sans
                // `WANCommonInterfaceConfig` ou en erreur SOAP ne
                // laissait aucune trace — impossible de distinguer
                // « pas de routeur » de « mesure pas encore faite ».
                tracing::info!(
                    "mesure UPnP du debit WAN sans resultat (IGD absent ou service non supporte)"
                );
            }
        }
        if !bw.probe_up_urls.is_empty() {
            if let Some(up) = measure_probe_upload(bw, ip_policy).await {
                // La sonde (débit réellement atteint) est plus fiable
                // que le débit provisionné UPnP : elle l'emporte si
                // supérieure, sinon on garde UPnP.
                let mut m = self.measured.lock().unwrap();
                let current = m.map(|(up, _, _)| up).unwrap_or(0);
                if up > current {
                    *m = Some((up, 0, EstimateSource::Probe));
                    tracing::info!(up_bps = up, "capacite upload mesuree par sonde");
                }
            }
        }
    }
}

/// `GetLinkLayerMaxBitRates` de `WANCommonInterfaceConfig` : débit
/// WAN provisionné lu sur le routeur (bits/s → octets/s). `None` si
/// pas d'IGD, service absent ou réponse invalide.
async fn measure_upnp_wan(timeout: Duration) -> Option<(u64, u64)> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let search = tokio::spawn(async move {
        let _ =
            librqbit_upnp::discover_once(&tx, librqbit_upnp::SSDP_SEARCH_ROOT_ST, timeout, None)
                .await;
    });
    let mut result = None;
    while let Some(resp) = rx.recv().await {
        let Ok(desc) = librqbit_upnp::discover_services(resp.location.clone()).await else {
            continue;
        };
        // `iter_services` n'est pas `Send` : collecter les URLs de
        // controle avant le `.await` SOAP.
        let control_urls: Vec<url::Url> = desc
            .devices
            .iter()
            .flat_map(|d| d.iter_services(tracing::Span::none()))
            .filter(|(_, s)| s.service_type == WAN_CIC_SERVICE)
            .filter_map(|(_, s)| resp.location.join(&s.control_url).ok())
            .collect();
        for url in control_urls {
            if let Some((up, down)) = query_link_layer_bitrates(url, timeout).await {
                result = Some((up, down));
                break;
            }
        }
        if result.is_some() {
            break;
        }
    }
    search.abort();
    result
}

/// POST SOAP `GetLinkLayerMaxBitRates` → `(upstream, downstream)`
/// en octets/s. Communication LAN directe (routeur) — volontairement
/// hors `ip_policy` (les adresses privées y seraient refusées).
async fn query_link_layer_bitrates(url: url::Url, timeout: Duration) -> Option<(u64, u64)> {
    let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
    let body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
    s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:GetLinkLayerMaxBitRates xmlns:u="{WAN_CIC_SERVICE}"/>
  </s:Body>
</s:Envelope>"#
    );
    let resp = client
        .post(url)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header(
            "SOAPAction",
            format!("\"{WAN_CIC_SERVICE}#GetLinkLayerMaxBitRates\""),
        )
        .body(body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let up = xml_tag_u64(&text, "NewUpstreamMaxBitRate")?;
    let down = xml_tag_u64(&text, "NewDownstreamMaxBitRate").unwrap_or(0);
    // bits/s → octets/s ; un débit WAN nul n'est pas une mesure.
    let up_bps = up / 8;
    (up_bps > 0).then_some((up_bps, down / 8))
}

/// Extrait la valeur numérique d'une balise XML simple
/// (`<tag>123</tag>` — tolère un `>` d'attribut sur la balise
/// ouvrante). Suffisant pour la réponse SOAP d'un IGD.
fn xml_tag_u64(text: &str, tag: &str) -> Option<u64> {
    let open = text.find(tag)?;
    let gt = text[open..].find('>')? + open;
    let value = &text[gt + 1..];
    let end = value.find('<')?;
    value[..end].trim().parse().ok()
}

/// POST d'un blob vers les URLs de sonde configurées — débit upload
/// réellement atteint (octets/s), meilleur temps sur toutes les
/// sondes. `ip_policy` s'applique (URLs contrôlées par la config).
async fn measure_probe_upload(
    bw: &BandwidthConfig,
    ip_policy: &onionbit_network_policy::IpPolicy,
) -> Option<u64> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(bw.probe_timeout_secs))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let payload = vec![0u8; bw.probe_bytes as usize];
    let mut best = None;
    for url in &bw.probe_up_urls {
        let Ok(parsed) = url::Url::parse(url) else {
            continue;
        };
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let Some(port) = parsed.port_or_known_default() else {
            continue;
        };
        let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
            continue;
        };
        // Anti-SSRF : chaque adresse résolue doit être autorisée, et
        // il faut au moins une adresse.
        let addrs: Vec<_> = addrs.collect();
        if addrs.is_empty() || addrs.iter().any(|a| ip_policy.check(a).is_err()) {
            tracing::info!(url = %url, "sonde bande passante refusee par ip_policy");
            continue;
        }
        let started = Instant::now();
        let Ok(resp) = client
            .post(parsed.clone())
            .body(payload.clone())
            .send()
            .await
        else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let secs = started.elapsed().as_secs_f64();
        if secs > 0.05 {
            let bps = (bw.probe_bytes as f64 / secs) as u64;
            best = Some(best.unwrap_or(0).max(bps));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BandwidthConfig {
        BandwidthConfig::default()
    }

    /// Plafond servi = `max(upload × share, floor)` ; `fallback`
    /// tant que rien n'a été mesuré.
    #[test]
    fn effective_bps_part_plancher_repli() {
        let est = BandwidthEstimator::new();
        let bw = cfg();
        // Jamais mesuré : valeur de repli (512 Kio/s).
        assert_eq!(est.effective_bps(&bw), bw.fallback_bps);

        // 30 Mio/s mesurés → tiers = 10 Mio/s.
        *est.measured.lock().unwrap() =
            Some((30 * 1024 * 1024, 100 * 1024 * 1024, EstimateSource::Upnp));
        assert_eq!(est.effective_bps(&bw), 10 * 1024 * 1024);

        // Lien très faible (120 Kio/s → tiers = 40 Kio/s) : plancher.
        *est.measured.lock().unwrap() = Some((120 * 1024, 0, EstimateSource::Probe));
        assert_eq!(est.effective_bps(&bw), bw.floor_bps);
    }

    /// Le pic passif (compteurs endpoint) est une borne basse de la
    /// capacité : il participe à l'estimation sans écraser une mesure
    /// active supérieure.
    #[test]
    fn pic_passif_borne_basse() {
        let est = BandwidthEstimator::new();
        // Première lecture pose la référence, la seconde mesure.
        est.sample_counters(1_000);
        std::thread::sleep(Duration::from_millis(120));
        est.sample_counters(1_000 + 600_000);
        let peak = est.passive_peak_up.load(Ordering::Relaxed);
        // ~600 Kio en ~0,12 s → ~5 Mio/s (ordre de grandeur).
        assert!(peak > 1_000_000);
        // Seule mesure disponible → le pic sert d'estimation.
        assert_eq!(est.estimated_up_bps(), Some(peak));

        // Une mesure active plus forte l'emporte.
        *est.measured.lock().unwrap() = Some((50 * 1024 * 1024, 0, EstimateSource::Upnp));
        assert_eq!(est.estimated_up_bps(), Some(50 * 1024 * 1024));
    }

    /// Extraction des balises `NewUpstream/DownstreamMaxBitRate` de la
    /// réponse SOAP d'un IGD.
    #[test]
    fn xml_tag_parse() {
        let soap = r#"<s:Envelope><s:Body>
            <u:GetLinkLayerMaxBitRatesResponse xmlns:u="x">
              <NewUpstreamMaxBitRate>20000000</NewUpstreamMaxBitRate>
              <NewDownstreamMaxBitRate>100000000</NewDownstreamMaxBitRate>
            </u:GetLinkLayerMaxBitRatesResponse></s:Body></s:Envelope>"#;
        assert_eq!(xml_tag_u64(soap, "NewUpstreamMaxBitRate"), Some(20_000_000));
        assert_eq!(
            xml_tag_u64(soap, "NewDownstreamMaxBitRate"),
            Some(100_000_000)
        );
        assert_eq!(xml_tag_u64(soap, "Absent"), None);
        assert_eq!(xml_tag_u64("<a></a>", "Nope"), None);
    }

    /// Sonde upload contre un serveur loopback : le POST doit être
    /// mesuré (politique permissive de test, comme la sonde de
    /// version `check_new_version_sonde_loopback`).
    #[tokio::test]
    async fn sonde_upload_loopback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Consomme le corps lentement : ~500 ms de lecture, sinon
            // le POST loopback boucle en <50 ms sous le seuil de
            // mesure.
            let mut buf = [0u8; 8192];
            let deadline = tokio::time::sleep(Duration::from_millis(600));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    r = sock.read(&mut buf) => {
                        if r.unwrap_or(0) == 0 {
                            break;
                        }
                    }
                }
            }
            let body = "ok";
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        let mut bw = cfg();
        bw.probe_up_urls = vec![format!("http://127.0.0.1:{port}/up")];
        bw.probe_bytes = 256 * 1024;
        bw.probe_timeout_secs = 5;
        let bps = measure_probe_upload(&bw, &onionbit_network_policy::IpPolicy::permissive()).await;
        assert!(bps.unwrap_or(0) > 0);
    }

    /// La sonde upload respecte `ip_policy` : loopback refusé en
    /// politique stricte (anti-SSRF — URLs contrôlées par la config).
    #[tokio::test]
    async fn sonde_upload_refusee_en_strict() {
        let mut bw = cfg();
        bw.probe_up_urls = vec!["http://127.0.0.1:1/up".to_string()];
        assert!(
            measure_probe_upload(&bw, &onionbit_network_policy::IpPolicy::strict())
                .await
                .is_none()
        );
    }
}
