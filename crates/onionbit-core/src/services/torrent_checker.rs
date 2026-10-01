//! `torrent_checker` — equivalent de
//! `tribler/core/torrent_checker/` : scrape de sante (seeders/
//! leechers) des torrents connus via leurs trackers.
//!
//! Protocoles supportes (fideles a `torrentchecker_session.py`) :
//! - **UDP** BEP-15 : `connect` (`!qii` magic `0x41727101980`,
//!   action 0, txn) puis `scrape` (`!qii` + `20s`xN) -> `!iii`xN
//!   (seeders, downloaded, leechers) ;
//! - **HTTP** : `GET` sur `announce`->`scrape` + `info_hash` (bencode
//!   `files` -> `complete`/`incomplete`), anti-SSRF via `ip_policy`.
//!
//! Les resultats sont persists dans `torrent_state`/`tracker_state`
//! et notifies (`TorrentHealthUpdated`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use onionbit_db::Database;
use onionbit_network_policy::IpPolicy;

use crate::error::{CoreError, Result};
use crate::notifier::{Notification, Notifier};

/// Magic BEP-15 (`UDP_TRACKER_INIT_CONNECTION_ID`).
const UDP_TRACKER_INIT_CONNECTION_ID: i64 = 0x41727101980;
/// Action `connect`.
const TRACKER_ACTION_CONNECT: i32 = 0;
/// Action `scrape`.
const TRACKER_ACTION_SCRAPE: i32 = 2;
/// Timeout d'une session de scrape.
const SCRAPE_TIMEOUT: Duration = Duration::from_secs(10);
/// Nombre max d'infohashes par requete UDP scrape (borne de paquet).
const MAX_INFOHASHES_PER_SCRAPE: usize = 32;
/// Intervalle minimum entre deux controles du meme torrent
/// (`MIN_TORRENT_CHECK_INTERVAL` Python = 900 s).
pub const MIN_TORRENT_CHECK_INTERVAL: i64 = 900;

/// Sante d'un essaim (`HealthInfo` Python).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthInfo {
    /// Info-hash v1.
    pub infohash: [u8; 20],
    /// Seeders observes.
    pub seeders: i64,
    /// Leechers observes.
    pub leechers: i64,
    /// Horodatage du controle (secondes Unix).
    pub last_check: i64,
    /// URL du tracker ayant repondu ("" = DHT/gossip).
    pub tracker: String,
    /// Controle fait localement (`self_checked`).
    pub self_checked: bool,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Service de controle de sante des torrents.
pub struct TorrentChecker {
    db: Arc<Database>,
    notifier: Notifier,
    ip_policy: Arc<IpPolicy>,
    /// Socket UDP partagee (BEP-15).
    socket: Arc<tokio::net::UdpSocket>,
}

impl std::fmt::Debug for TorrentChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentChecker").finish_non_exhaustive()
    }
}

impl TorrentChecker {
    /// Cree le service avec sa socket UDP dediee (`listen_on_udp`
    /// Python — une socket partagee entre les sessions).
    pub async fn new(db: Arc<Database>, notifier: Notifier, ip_policy: IpPolicy) -> Result<Self> {
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        Ok(Self {
            db,
            notifier,
            ip_policy: Arc::new(ip_policy),
            socket: Arc::new(socket),
        })
    }

    /// `create_tracker_session` : scrape un tracker pour les
    /// infohashes donnes et persiste les santes.
    pub async fn check_tracker(
        &self,
        tracker_url: &str,
        infohashes: &[[u8; 20]],
    ) -> Result<Vec<HealthInfo>> {
        let healths = if tracker_url.starts_with("udp://") {
            self.udp_scrape(tracker_url, infohashes).await
        } else if tracker_url.starts_with("http://") || tracker_url.starts_with("https://") {
            self.http_scrape(tracker_url, infohashes).await
        } else {
            return Err(CoreError::InvalidState("schema de tracker inconnu"));
        }?;
        self.record_healths(tracker_url, &healths);
        Ok(healths)
    }

    /// Persiste les santes et notifie (`process_torrents_health` +
    /// tracker_state update Python).
    fn record_healths(&self, tracker_url: &str, healths: &[HealthInfo]) {
        for h in healths {
            let _ = self.db.with(|c| {
                onionbit_db::health::upsert_tracker(c, tracker_url)?;
                onionbit_db::health::upsert_torrent_state(c, &h.infohash)?;
                onionbit_db::health::link_tracker(c, &h.infohash, tracker_url)?;
                onionbit_db::health::update_torrent_health(
                    c,
                    &h.infohash,
                    h.seeders,
                    h.leechers,
                    h.last_check,
                    h.self_checked,
                )
            });
            self.notifier.notify(Notification::TorrentHealthUpdated {
                infohash: hex::encode(h.infohash),
                seeders: h.seeders,
                leechers: h.leechers,
            });
        }
    }

    /// Scrape UDP BEP-15 : `connect` puis `scrape`.
    async fn udp_scrape(
        &self,
        tracker_url: &str,
        infohashes: &[[u8; 20]],
    ) -> Result<Vec<HealthInfo>> {
        let parsed = url::Url::parse(tracker_url)
            .map_err(|_| CoreError::InvalidState("url de tracker invalide"))?;
        let host = parsed
            .host_str()
            .ok_or(CoreError::InvalidState("tracker udp sans hote"))?;
        let port = parsed
            .port()
            .ok_or(CoreError::InvalidState("tracker udp sans port"))?;
        // Anti-SSRF : chaque adresse resolue doit passer la politique.
        let mut target: Option<SocketAddr> = None;
        for addr in tokio::net::lookup_host((host, port)).await? {
            self.ip_policy.check(&addr)?;
            target.get_or_insert(addr);
        }
        let target = target.ok_or(CoreError::InvalidState("tracker sans adresse"))?;

        // connect : !q conn_id, i action, i txn
        let txn: i32 = rand::random::<u32>() as i32;
        let mut req = Vec::with_capacity(16);
        req.extend_from_slice(&UDP_TRACKER_INIT_CONNECTION_ID.to_be_bytes());
        req.extend_from_slice(&TRACKER_ACTION_CONNECT.to_be_bytes());
        req.extend_from_slice(&txn.to_be_bytes());
        let resp = self.udp_roundtrip(target, &req).await?;
        if resp.len() < 16 {
            return Err(CoreError::InvalidState("reponse connect tronquee"));
        }
        let action = i32::from_be_bytes(resp[0..4].try_into().unwrap());
        let rtxn = i32::from_be_bytes(resp[4..8].try_into().unwrap());
        if action != TRACKER_ACTION_CONNECT || rtxn != txn {
            return Err(CoreError::InvalidState("connect refuse par le tracker"));
        }
        let conn_id = i64::from_be_bytes(resp[8..16].try_into().unwrap());

        // scrape : !q conn, i action, i txn, 20s x N
        let txn2: i32 = rand::random::<u32>() as i32;
        let batch = &infohashes[..infohashes.len().min(MAX_INFOHASHES_PER_SCRAPE)];
        let mut req = Vec::with_capacity(16 + 20 * batch.len());
        req.extend_from_slice(&conn_id.to_be_bytes());
        req.extend_from_slice(&TRACKER_ACTION_SCRAPE.to_be_bytes());
        req.extend_from_slice(&txn2.to_be_bytes());
        for ih in batch {
            req.extend_from_slice(ih);
        }
        let resp = self.udp_roundtrip(target, &req).await?;
        if resp.len() < 8 || resp.len() - 8 != batch.len() * 12 {
            return Err(CoreError::InvalidState("reponse scrape invalide"));
        }
        let action = i32::from_be_bytes(resp[0..4].try_into().unwrap());
        let rtxn = i32::from_be_bytes(resp[4..8].try_into().unwrap());
        if action != TRACKER_ACTION_SCRAPE || rtxn != txn2 {
            return Err(CoreError::InvalidState("scrape refuse par le tracker"));
        }
        let now = now_unix();
        let mut out = Vec::with_capacity(batch.len());
        for (i, ih) in batch.iter().enumerate() {
            let off = 8 + i * 12;
            let complete = i32::from_be_bytes(resp[off..off + 4].try_into().unwrap());
            let incomplete = i32::from_be_bytes(resp[off + 8..off + 12].try_into().unwrap());
            out.push(HealthInfo {
                infohash: *ih,
                seeders: i64::from(complete),
                leechers: i64::from(incomplete),
                last_check: now,
                tracker: tracker_url.to_string(),
                self_checked: true,
            });
        }
        Ok(out)
    }

    /// Aller-retour UDP borne par `SCRAPE_TIMEOUT`.
    async fn udp_roundtrip(&self, target: SocketAddr, req: &[u8]) -> Result<Vec<u8>> {
        let socket = self.socket.clone();
        tokio::time::timeout(SCRAPE_TIMEOUT, async move {
            socket.send_to(req, target).await?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let (n, src) = socket.recv_from(&mut buf).await?;
                if src == target {
                    return Ok::<Vec<u8>, std::io::Error>(buf[..n].to_vec());
                }
            }
        })
        .await
        .map_err(|_| CoreError::InvalidState("scrape udp en timeout"))?
        .map_err(CoreError::Io)
    }

    /// Scrape HTTP : `announce` -> `scrape`, `info_hash` x N, corps
    /// bencode `files` -> `complete`/`incomplete`
    /// (`process_scrape_response` Python).
    async fn http_scrape(
        &self,
        tracker_url: &str,
        infohashes: &[[u8; 20]],
    ) -> Result<Vec<HealthInfo>> {
        let scrape_url = tracker_url.replace("announce", "scrape");
        let parsed = url::Url::parse(&scrape_url)
            .map_err(|_| CoreError::InvalidState("url de scrape invalide"))?;
        // `info_hash` transporte des octets bruts : percent-encodage
        // manuel (`add_url_params` Python envoie les 20 octets
        // percent-encodes, pas une perte UTF-8).
        let sep = if parsed.query().is_some() { "&" } else { "?" };
        let mut full = format!("{scrape_url}{sep}");
        for (i, ih) in infohashes
            .iter()
            .take(MAX_INFOHASHES_PER_SCRAPE)
            .enumerate()
        {
            if i > 0 {
                full.push('&');
            }
            full.push_str("info_hash=");
            full.push_str(&url::form_urlencoded::byte_serialize(ih).collect::<String>());
        }
        let resp = super::fetch_checked(&full, &self.ip_policy).await?;
        let body = super::read_body_limited(resp).await?;
        let value = onionbit_format::bencode::decode(&body)?;
        let Some(files) = value.get(b"files").and_then(|v| v.as_dict()) else {
            return Err(CoreError::InvalidState("reponse scrape sans `files`"));
        };
        let now = now_unix();
        let mut out = Vec::with_capacity(infohashes.len());
        for ih in infohashes.iter().take(MAX_INFOHASHES_PER_SCRAPE) {
            let (seeders, leechers) = files
                .get(ih.as_slice())
                .and_then(|f| f.as_dict())
                .map(|d| {
                    (
                        d.get(b"complete".as_slice())
                            .and_then(|v| v.as_int())
                            .unwrap_or(0),
                        d.get(b"incomplete".as_slice())
                            .and_then(|v| v.as_int())
                            .unwrap_or(0),
                    )
                })
                .unwrap_or((0, 0));
            out.push(HealthInfo {
                infohash: *ih,
                seeders,
                leechers,
                last_check: now,
                tracker: tracker_url.to_string(),
                self_checked: true,
            });
        }
        Ok(out)
    }

    /// Controle periodique : un torrent choisi dans `torrent_state`
    /// (non controle depuis `MIN_TORRENT_CHECK_INTERVAL`), scrape de
    /// ses trackers connus.
    pub async fn check_oldest(&self) -> Result<usize> {
        let row: std::result::Result<Vec<u8>, _> = self.db.with(|c| {
            c.query_row(
                "SELECT infohash FROM torrent_state ORDER BY last_check ASC LIMIT 1",
                [],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .map_err(onionbit_db::DbError::from)
        });
        let Ok(ih) = row else {
            return Ok(0);
        };
        let trackers = self
            .db
            .with(|c| onionbit_db::health::trackers_of(c, &ih))
            .unwrap_or_default();
        if trackers.is_empty() {
            return Ok(0);
        }
        let mut infohash = [0u8; 20];
        if ih.len() != 20 {
            return Ok(0);
        }
        infohash.copy_from_slice(&ih);
        let mut checked = 0;
        for t in &trackers {
            if let Ok(healths) = self.check_tracker(t, &[infohash]).await {
                checked += healths.len();
            }
        }
        Ok(checked)
    }
}
