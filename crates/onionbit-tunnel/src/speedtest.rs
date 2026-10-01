// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Port de `ipv8-rust-tunnels/src/speedtest.rs` : mesure de debit
//! d'un circuit via les cellules `test-request`(21)/`test-response`
//! (22) du backend Rust — le format filaire reel de Tribler 8.x
//! (`identifier` u32). Les cellules 19/20 du backend Python pur
//! (`identifier` u16) sont aussi traitees pour l'interop avec un
//! pyipv8 sans extension native.
//!
//! `run_speedtest` reproduit `run_test` : boucle d'emission infinie
//! throttlee par le RTT moyen des 10 dernieres reponses, snapshots
//! periodiques des stats toutes les `SPEED_TEST_CALLBACK_MS`, puis un
//! snapshot final `done=true` apres `2 * target_rtt` ms de drain.
//! Les `stats` ont la forme Python `{request_id: [ts_envoi_ms,
//! octets_envoyes, ts_reception_ms, octets_recus]}` — le calcul
//! `up`/`down` (en MiB/s) reste dans le handler REST, comme
//! `run_speed_test` pyipv8.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use onionbit_ipv8::UdpAddress;
use rand::Rng;

use crate::community::TunnelCommunity;
use crate::payload::{self as tp};
use crate::routing::{CIRCUIT_TYPE_RP_DOWNLOADER, CIRCUIT_TYPE_RP_SEEDER, PEER_FLAG_SPEED_TEST};

/// `target_rtt` passe par `tunnel_endpoint.py` (`run_speedtest(...,
/// 100, callback, 500)` — le 5e argument est `target_rtt`).
pub const SPEED_TEST_TARGET_RTT_MS: u64 = 100;
/// Intervalle des snapshots de stats vers le client (500 ms).
pub const SPEED_TEST_CALLBACK_MS: u64 = 500;
/// Capacite du canal interne `test_channel` (`200` dans socket.rs).
pub const SPEED_TEST_CHANNEL_CAP: usize = 200;
/// Buffer aleatoire reutilise (`random_data = [0; 2048]`).
const SPEED_TEST_RANDOM_BUF: usize = 2048;
/// Fenetre de lissage du throttle (`rtts[-10:]`, `sum/10`).
const RTT_WINDOW: usize = 10;

/// `stats` de `run_speedtest` : `{request_id: [ts_envoi_ms,
/// octets_envoyes, ts_reception_ms, octets_recus]}`.
pub type SpeedTestStats = HashMap<u32, [u64; 4]>;

/// `get_time_ms` (ms depuis l'epoch).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl TunnelCommunity {
    /// `crypto_endpoint.run_speedtest` : renvoie un canal de snapshots
    /// `(stats, done)` — `done=false` toutes les
    /// `SPEED_TEST_CALLBACK_MS`, puis `done=true` apres le drain.
    /// La tache se termine aussi si le client lache le receveur.
    pub fn run_speedtest(
        self: &Arc<Self>,
        circuit_id: u32,
        test_time_ms: u64,
        request_size: u16,
        response_size: u16,
    ) -> tokio::sync::mpsc::Receiver<(SpeedTestStats, bool)> {
        let (tx, rx) = tokio::sync::mpsc::channel(SPEED_TEST_CHANNEL_CAP);
        let this = self.clone();
        tokio::spawn(async move {
            let results = Arc::new(Mutex::new(SpeedTestStats::new()));
            let rtts = Arc::new(Mutex::new(Vec::<u64>::new()));

            // `receive_loop` : les `test-response` (cellule 22)
            // arrivent via `test_tx` — on enregistre
            // `[recv_ts, bytes_recus]` et le RTT pour le throttle.
            let recv_task = {
                let results = results.clone();
                let rtts = rtts.clone();
                let mut test_rx = this.test_tx.subscribe();
                tokio::spawn(async move {
                    while let Ok((tid, n)) = test_rx.recv().await {
                        let mut map = results.lock().unwrap();
                        if let Some(e) = map.get_mut(&tid) {
                            e[2] = now_ms();
                            e[3] = n as u64;
                            rtts.lock().unwrap().push(e[2].saturating_sub(e[0]));
                        }
                    }
                })
            };

            // `cb_loop` : snapshot `done=false` periodique.
            let cb_task = {
                let results = results.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(SPEED_TEST_CALLBACK_MS)).await;
                        let snap = results.lock().unwrap().clone();
                        if tx.send((snap, false)).await.is_err() {
                            break;
                        }
                    }
                })
            };

            // `send_loop` : emission continue de `test-request` tant
            // que le RTT moyen ne depasse pas `target_rtt`.
            let send_task = {
                let this = this.clone();
                let results = results.clone();
                tokio::spawn(async move {
                    let mut random_data = [0u8; SPEED_TEST_RANDOM_BUF];
                    rand::rng().fill_bytes(&mut random_data);
                    loop {
                        // `if sum(rtts[-10:])/10 > target_rtt:
                        // sleep(0)` — yield sans bloquer.
                        let throttled = {
                            let rtts = rtts.lock().unwrap();
                            rtts.len() > RTT_WINDOW
                                && rtts.iter().rev().take(RTT_WINDOW).sum::<u64>()
                                    / RTT_WINDOW as u64
                                    > SPEED_TEST_TARGET_RTT_MS
                        };
                        if throttled {
                            tokio::task::yield_now().await;
                        }
                        let tid = rand::random::<u32>();
                        let Some(addr) = this.circuit_first_hop_addr(circuit_id) else {
                            break;
                        };
                        let p = tp::SpeedTestRequest {
                            circuit_id,
                            identifier: tid,
                            response_size,
                            data: random_data[..request_size as usize].to_vec(),
                        };
                        match this.send_cell(&addr, &p).await {
                            Ok(n) => {
                                results
                                    .lock()
                                    .unwrap()
                                    .insert(tid, [now_ms(), n as u64, 0, 0]);
                            }
                            Err(_) => break,
                        }
                    }
                })
            };

            tokio::time::sleep(Duration::from_millis(test_time_ms)).await;
            send_task.abort();
            recv_task.abort();
            cb_task.abort();
            // `sleep(target_rtt * 2)` : drain des reponses encore en
            // vol avant le snapshot final.
            tokio::time::sleep(Duration::from_millis(SPEED_TEST_TARGET_RTT_MS * 2)).await;
            let snap = results.lock().unwrap().clone();
            let _ = tx.send((snap, true)).await;
        });
        rx
    }

    /// `send_test_request` de la community Python (cellule 19, u16) —
    /// envoi unitaire, hors `run_speedtest` (utilise par les pairs
    /// pyipv8 sans backend natif).
    pub(crate) fn on_py_test_request(self: &Arc<Self>, src: SocketAddr, p: tp::TestRequest) {
        // `on_test_request` pyipv8 : sans `PEER_FLAG_SPEED_TEST` dans
        // nos flags de service, on ignore ; la reponse n'est faite
        // que si le circuit_id est connu (sortie ou RP e2e).
        if self.inner.lock().unwrap().peer_flags & PEER_FLAG_SPEED_TEST == 0 {
            return;
        }
        let known = {
            let inner = self.inner.lock().unwrap();
            inner.exit_sockets.contains_key(&p.circuit_id)
                || inner.circuits.get(&p.circuit_id).is_some_and(|c| {
                    c.ctype == CIRCUIT_TYPE_RP_SEEDER || c.ctype == CIRCUIT_TYPE_RP_DOWNLOADER
                })
        };
        if !known {
            return;
        }
        let mut data = vec![0u8; p.response_size as usize];
        rand::rng().fill_bytes(&mut data);
        let reply = tp::TestResponse {
            circuit_id: p.circuit_id,
            identifier: p.identifier,
            data,
        };
        let addr = UdpAddress::from(src);
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.send_cell(&addr, &reply).await;
        });
    }

    /// `on_test_response` pyipv8 (cellule 20) : sans `TestRequestCache`
    /// en cours — on ne recoit une reponse Python qu'a une requete
    /// 19 emise par `send_test_request`, non implementee — la
    /// reponse est "unexpected" et tombe.
    pub(crate) fn on_py_test_response(&self, p: tp::TestResponse) {
        tracing::debug!(
            circuit_id = p.circuit_id,
            "test-response (19/20) inattendue — ignoree"
        );
    }

    /// `socket.on_test_request` d'`ipv8-rust-tunnels` (cellule 21) :
    /// reponse immediate `test-response`(22) de `response_size`
    /// octets aleatoires — aucun controle de flag cote backend Rust.
    pub(crate) fn on_speedtest_request(self: &Arc<Self>, src: SocketAddr, p: tp::SpeedTestRequest) {
        let mut data = vec![0u8; p.response_size as usize];
        rand::rng().fill_bytes(&mut data);
        let reply = tp::SpeedTestResponse {
            circuit_id: p.circuit_id,
            identifier: p.identifier,
            data,
        };
        let addr = UdpAddress::from(src);
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.send_cell(&addr, &reply).await;
        });
    }

    /// `socket.on_test_response` (cellule 22) : publie
    /// `(identifier, octets_cellule)` sur `test_tx` — `receive_loop`
    /// des tests en cours y puise ses mesures.
    pub(crate) fn on_speedtest_response(&self, identifier: u32, cell_len: usize) {
        // `send()` echoue sans abonnes : normal hors test.
        let _ = self.test_tx.send((identifier, cell_len));
    }
}
