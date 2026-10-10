// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backend `bitdaemon` de [`BtEngine`] (ONIONBIT-1).
//!
//! Enveloppe `bitdaemon::engine::Engine` + `SessionHandle` derriere
//! les memes operations que la facade rqbit : le swap est opt-in via
//! `EngineConfig::backend == EngineBackend::Bitdaemon` et les deux
//! backends se comparent a config egale.
//!
//! Cycle de vie : `Engine` est `&mut self` (acteur mono-tache, comme
//! `session_impl` upstream) — il vit dans la tache `driver` qui
//! replique `Engine::run` (tick 1 s + connect pump) et consomme un
//! canal de commandes ([`BdCmd`]) : les effets `SessionOut` emis hors
//! tick (add/remove/force_announce) et les re-glues `apply_settings`
//! (le handle session seul ne re-percute pas les limites de l'engine).
//!
//! Ecarts assumes (tranches ulterieures) :
//! - lanes anonymes : transports injectes pas encore cables ici
//!   (sockets OS `Tokio*`) — `socks5_proxy` refuse le demarrage ;
//! - zone privee `.bitv` : `storage_factory` par telechargement est
//!   refusee (fail-closed — pas d'ecriture en clair par erreur) ;
//! - fastresume `.bitv` / persistance `session.json` : non porte ;
//! - magnet sans metadonnees : le stockage `StorageIndex(0)` differe
//!   est celui du port — ecriture differee a valider upstream.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bitdaemon::core::add_torrent_params::AddTorrentParams;
use bitdaemon::disk::disk_io::PreadDiskIo;
use bitdaemon::disk::storage::StorageParams;
use bitdaemon::net::os_transports::{TokioTcpAcceptor, TokioTcpConnector, TokioUdpSocket};
use bitdaemon::session::session::SessionOut;
use bitdaemon::session::session_handle::{lock as slock, SessionHandle, TorrentHandle};
use bitdaemon_common::hash::Sha1Hash;
use bitdaemon_common::types::StorageIndex;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::config::EngineConfig;
use crate::error::{BtError, Result};

/// Nombre de threads I/O disque (`pread_disk_io` upstream, comme le
/// CLI `PreadDiskIo::new(4, …)`).
const DISK_THREADS: i64 = 4;

/// Commandes adressees a la tache driver de l'engine.
enum BdCmd {
    /// Effets session emis hors tick (announce stop, add, remove…)
    /// — l'engine les traduit en I/O reelles (`dispatch_out`).
    Out(Vec<SessionOut>),
    /// Re-glues `apply_settings` : propage les limites de debit aux
    /// `RateLimiter` de l'engine (le handle ne touche que la session).
    ApplySettings(bitdaemon_session::settings::SessionSettings),
}

/// Backend moteur bitdaemon d'une [`crate::BtEngine`].
///
/// Clone leger (`Arc` interne — meme sémantique que `BtEngine`).
pub struct BdBackend {
    /// Handle session partage avec l'engine (`Arc<Mutex<Session>>`).
    pub handle: SessionHandle,
    /// Disque positionne I/O — `new_torrent` enregistre les storages.
    pub disk: Arc<PreadDiskIo>,
    /// Canal de commandes vers la tache driver.
    cmd_tx: mpsc::UnboundedSender<BdCmd>,
    /// Arret du driver (le `stop` de `Engine::run`).
    stop_tx: watch::Sender<bool>,
    /// Tache driver (join a l'arret).
    driver: std::sync::Mutex<Option<JoinHandle<()>>>,
    /// Adresse d'ecoute effective (TCP + UDP/DHT partagent le port).
    listen_addr: Option<SocketAddr>,
}

impl std::fmt::Debug for BdBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BdBackend")
            .field("listen_addr", &self.listen_addr)
            .finish_non_exhaustive()
    }
}

impl BdBackend {
    /// Demarre l'engine bitdaemon sur les transports OS.
    ///
    /// `socks5_proxy` est refuse pour l'instant : les transports
    /// injectes des lanes anonymes ne sont pas encore cables et un
    /// repli silencieux vers une connexion directe violerait le kill
    /// switch (`onionbit-network-policy`).
    pub async fn start(config: &EngineConfig) -> Result<Self> {
        if config.socks5_proxy.is_some() {
            return Err(BtError::Engine(
                "backend bitdaemon : socks5_proxy non supporte (lanes anonymes non cablees)".into(),
            ));
        }
        let disk = PreadDiskIo::new(DISK_THREADS, Arc::new(|| {}));

        let connector: Arc<TokioTcpConnector> = Arc::new(TokioTcpConnector);
        let port = config.listen_port.unwrap_or(0);
        let udp = TokioUdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
            .await
            .map_err(|e| BtError::Engine(format!("bind udp :{port} — {e}")))?;
        let udp_port = bitdaemon::net::transport::DatagramSocket::local_addr(&udp).port();

        let mut eng = bitdaemon::engine::Engine::new(connector, Some(Arc::new(udp)));
        eng.set_resolver(Arc::new(bitdaemon::net::resolver::SystemResolver));
        let session = eng.session.clone();
        {
            let mut s = slock(&session);
            s.set_disk(disk.clone());
            s.listen_port = port;
            s.set_dht_port(udp_port);
        }
        if config.enable_dht {
            slock(&session).enable_dht(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                None,
                bitdaemon::dht::dht_settings::DhtSettings::default(),
            );
        }
        // Limites de session (0 = illimite upstream).
        {
            let mut st = slock(&session).settings.clone();
            st.upload_rate_limit = config.max_upload_bps.unwrap_or(0) as i64;
            st.download_rate_limit = config.max_download_bps.unwrap_or(0) as i64;
            eng.apply_settings(st);
        }

        let listen_addr = if config.listen_port.is_some() {
            let acceptor =
                TokioTcpAcceptor::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
                    .await
                    .map_err(|e| BtError::Engine(format!("bind tcp :{port} — {e}")))?;
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
            eng.start_acceptor(Arc::new(acceptor));
            Some(addr)
        } else {
            None
        };
        eng.start_udp_pump();
        // Routeurs bootstrap upstream `dht_bootstrap_nodes`.
        if config.enable_dht {
            for host in ["router.bittorrent.com", "dht.transmissionbt.com"] {
                eng.dispatch_out(SessionOut::DhtAddNode {
                    host: host.into(),
                    port: 6881,
                });
            }
        }

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<BdCmd>();
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let driver = tokio::spawn(async move {
            // Replique `Engine::run` + canal de commandes (tick 1 s,
            // connect pump, dispatch des effets hors-tick).
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = stop_rx.changed() => break,
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(BdCmd::Out(outs)) => {
                                for o in outs {
                                    eng.dispatch_out(o);
                                }
                            }
                            Some(BdCmd::ApplySettings(s)) => eng.apply_settings(s),
                            None => break,
                        }
                        continue;
                    }
                }
                eng.tick(std::time::Instant::now()).await;
                let st = eng.peer_list_state();
                let stime = slock(&eng.session).session_time;
                let budget = eng.connection_speed.max(0) as usize;
                let candidates = eng.connect_pump(stime, &st, budget);
                for (ih, ep) in candidates {
                    eng.spawn_dial(ih, ep, false);
                }
            }
        });

        tracing::info!(
            listen = ?listen_addr,
            dht = config.enable_dht,
            "backend bitdaemon demarre"
        );
        Ok(Self {
            handle: SessionHandle::from_shared(session),
            disk,
            cmd_tx,
            stop_tx,
            driver: std::sync::Mutex::new(Some(driver)),
            listen_addr,
        })
    }

    /// Arret propre : le driver sort de sa boucle et droppe l'engine
    /// (les sockets injectees se ferment avec lui).
    pub async fn stop(&self) {
        let _ = self.stop_tx.send(true);
        let driver = self.driver.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(d) = driver {
            let _ = d.await;
        }
        tracing::info!("backend bitdaemon arrete");
    }

    /// Adresse d'ecoute effective.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    /// Envoie des effets `SessionOut` au driver (no-op si arrete).
    fn dispatch(&self, out: Vec<SessionOut>) {
        if !out.is_empty() {
            let _ = self.cmd_tx.send(BdCmd::Out(out));
        }
    }

    /// `add_torrent_params` : enregistre le storage (comme le CLI —
    /// `disk.new_torrent` quand les metadonnees sont connues,
    /// `StorageIndex(0)` differe sinon) puis insere le torrent dans la
    /// session et route les effets.
    pub fn add_torrent_params(&self, atp: &AddTorrentParams) -> Result<TorrentHandle> {
        let storage = if let Some(ti) = &atp.ti {
            let save_path =
                std::path::PathBuf::from(String::from_utf8_lossy(&atp.save_path).into_owned());
            let params = StorageParams {
                files: ti.files(),
                renamed: &Default::default(),
                path: save_path,
                part_file_dir: std::path::PathBuf::new(),
                mode: match atp.storage_mode {
                    bitdaemon::core::StorageMode::Sparse => {
                        bitdaemon::disk::storage::StorageMode::Sparse
                    }
                    _ => bitdaemon::disk::storage::StorageMode::Allocate,
                },
                priorities: &[],
                info_hash: *atp.info_hashes.v1.as_bytes(),
                v1: atp.info_hashes.has_v1(),
                v2: atp.info_hashes.has_v2(),
            };
            self.disk.new_torrent(&params)
        } else {
            StorageIndex(0)
        };
        let mut out = Vec::new();
        let sess = self.handle.session();
        let ih = slock(&sess)
            .add_torrent_params(atp, storage, &mut out)
            .ok_or_else(|| BtError::Engine("add_torrent_params refuse".into()))?;
        self.dispatch(out);
        self.handle
            .find_torrent(&ih)
            .ok_or_else(|| BtError::Engine("torrent absent apres add".into()))
    }

    /// Applique les limites de session (0/`None` = illimite) — via le
    /// canal de commandes pour re-percuter les `RateLimiter` engine.
    pub fn set_ratelimits(&self, upload_bps: Option<u64>, download_bps: Option<u64>) {
        let sess = self.handle.session();
        let mut st = slock(&sess).settings.clone();
        st.upload_rate_limit = upload_bps.unwrap_or(0).min(i64::MAX as u64) as i64;
        st.download_rate_limit = download_bps.unwrap_or(0).min(i64::MAX as u64) as i64;
        let _ = self.cmd_tx.send(BdCmd::ApplySettings(st));
    }

    /// Limites session courantes.
    pub fn ratelimits(&self) -> (Option<u64>, Option<u64>) {
        let sess = self.handle.session();
        let g = slock(&sess);
        let s = &g.settings;
        let nz = |v: i64| (v > 0).then_some(v as u64);
        (nz(s.upload_rate_limit), nz(s.download_rate_limit))
    }

    /// `remove_torrent` (+ `delete_files` optionnel) avec dispatch
    /// des announces `stopped`.
    pub fn remove(&self, ih: &Sha1Hash, delete_files: bool) -> Result<()> {
        let mut out = Vec::new();
        let sess = self.handle.session();
        let mut s = slock(&sess);
        if !s.torrents.contains_key(ih) {
            return Err(BtError::NotFound(bitdaemon_common::hex::to_hex(
                ih.as_bytes(),
            )));
        }
        // `delete_files` supprime aussi le torrent de la session
        // (upstream `~torrent` sur job disque) — les deux chemins
        // emettent l'announce `stopped`.
        if delete_files {
            s.delete_files(ih, 0, &mut out);
        } else {
            s.remove_torrent(ih, &mut out);
        }
        drop(s);
        self.dispatch(out);
        Ok(())
    }

    /// Pause/reprise (`Session::set_paused` — effets routes).
    pub fn set_paused(&self, ih: &Sha1Hash, paused: bool) -> Result<()> {
        let mut out = Vec::new();
        {
            let sess = self.handle.session();
            let mut s = slock(&sess);
            if !s.torrents.contains_key(ih) {
                return Err(BtError::NotFound(bitdaemon_common::hex::to_hex(
                    ih.as_bytes(),
                )));
            }
            s.set_paused(ih, paused, &mut out);
        }
        self.dispatch(out);
        Ok(())
    }

    /// `force_reannounce` sur tous les trackers du torrent.
    pub fn force_announce(&self, ih: &Sha1Hash) -> Result<()> {
        let h = self
            .handle
            .find_torrent(ih)
            .ok_or_else(|| BtError::NotFound(bitdaemon_common::hex::to_hex(ih.as_bytes())))?;
        let mut out = Vec::new();
        h.force_announce(None, &mut out);
        self.dispatch(out);
        Ok(())
    }
}
