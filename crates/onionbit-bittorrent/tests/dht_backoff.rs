// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Regression budget/cadence DHT anonyme : le backoff des re-lookups
//! `get_peers` sans progres (patch librqbit-dht vendored) doit doubler
//! a chaque passe, plafonner a `cap`, et jitterer (jamais deux
//! intervalles strictement identiques en serie).
//!
//! La fonction testee vit dans le crate vendored `librqbit-dht` — qui
//! n'est pas membre du workspace : ses `#[cfg(test)]` propres ne
//! seraient jamais compiles. D'ou ce test cote hote.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Socket datagramme factice pour la DHT vendored : compte les
/// datagrammes sortants (avec horodatage pour mesurer la cadence) et
/// sert de canal d'injection de datagrammes entrants. En mode `echo`,
/// chaque requete DHT recoit une reponse fabriquee (id derive de
/// l'adresse cible, `nodes` vide) — de quoi peupler la routing table
/// via le bootstrap et maintenir les vagues `get_peers`.
#[derive(Debug)]
struct SondeDht {
    addr: SocketAddr,
    envois: StdMutex<Vec<Envoi>>,
    tx: UnboundedSender<(Vec<u8>, SocketAddr)>,
    rx: tokio::sync::Mutex<UnboundedReceiver<(Vec<u8>, SocketAddr)>>,
    echo: AtomicBool,
}

#[derive(Debug, Clone)]
struct Envoi {
    instant: Instant,
    get_peers: bool,
}

/// Extrait le `transaction_id` bencode (`1:t<len>:<octets>`) d'une
/// requete DHT brute — la reponse fabriquee doit le reprendre pour
/// etre matchee par `inflight_by_transaction_id`.
fn extraire_tid(buf: &[u8]) -> Option<Vec<u8>> {
    let pos = buf.windows(3).position(|w| w == b"1:t")? + 3;
    let fin_digits = buf[pos..].iter().position(|&b| b == b':')? + pos;
    let len: usize = std::str::from_utf8(&buf[pos..fin_digits])
        .ok()?
        .parse()
        .ok()?;
    let debut = fin_digits + 1;
    (debut + len <= buf.len()).then(|| buf[debut..debut + len].to_vec())
}

/// Id20 deterministe par adresse : la reponse `r.id` doit etre stable
/// pour un meme expediteur (bookkeeping de la routing table).
fn id_pour(addr: SocketAddr) -> [u8; 20] {
    let port = addr.port().to_be_bytes();
    let mut id = [0x42u8; 20];
    id[0] = port[0];
    id[1] = port[1];
    id
}

/// Reponse DHT minimale : `{"r":{"id":...}, "t":<tid>, "y":"r"}`.
fn reponse_pour(tid: &[u8], rid: [u8; 20]) -> Vec<u8> {
    let mut r = Vec::with_capacity(64);
    r.extend_from_slice(b"d1:rd2:id20:");
    r.extend_from_slice(&rid);
    r.extend_from_slice(b"e1:t");
    r.extend_from_slice(tid.len().to_string().as_bytes());
    r.push(b':');
    r.extend_from_slice(tid);
    r.extend_from_slice(b"1:y1:re");
    r
}

/// Requete `ping` brute — le minimum parse par `bprotocol`.
fn requete_ping() -> Vec<u8> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(b"d1:ad2:id20:");
    q.extend_from_slice(&[0x55u8; 20]);
    q.extend_from_slice(b"e1:q4:ping1:t2:aa1:y1:qe");
    q
}

impl librqbit::DatagramSocket for SondeDht {
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        target: SocketAddr,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<usize>> + Send + Sync + 'a>,
    > {
        self.envois.lock().expect("envois").push(Envoi {
            instant: Instant::now(),
            get_peers: buf.windows(11).any(|w| w == b"9:get_peers"),
        });
        if self.echo.load(Ordering::Relaxed) {
            if let Some(tid) = extraire_tid(buf) {
                let _ = self.tx.send((reponse_pour(&tid, id_pour(target)), target));
            }
        }
        let len = buf.len();
        Box::pin(std::future::ready(Ok(len)))
    }

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<(usize, SocketAddr)>>
                + Send
                + Sync
                + 'a,
        >,
    > {
        Box::pin(async move {
            let mut rx = self.rx.lock().await;
            match rx.recv().await {
                Some((data, src)) => {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    Ok((n, src))
                }
                None => std::future::pending().await,
            }
        })
    }

    fn bind_addr(&self) -> SocketAddr {
        self.addr
    }
}

fn sonde(echo: bool) -> std::sync::Arc<SondeDht> {
    let (tx, rx) = unbounded_channel();
    std::sync::Arc::new(SondeDht {
        addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 59999).into(),
        envois: StdMutex::new(Vec::new()),
        tx,
        rx: tokio::sync::Mutex::new(rx),
        echo: AtomicBool::new(echo),
    })
}

/// Regression : les requetes DHT entrantes non sollicitees sont
/// bornees par `inbound_queries_per_second` — sans ce budget, chaque
/// requete reinjectee par une socket de sortie anonyme produisait
/// une reponse 1:1 (boucle d'amplification ~6 400 cellules/s mesuree
/// en mesh). Ici : 60 pings injectes, budget 10/s — la majorite doit
/// etre ignoree, le compteur `dropped_inbound_queries` le constate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_requetes_entrantes_borne_les_reponses() {
    // `echo` + bootstrap : sans bootstrap reussi le worker DHT
    // s'arrete (BootstrapFailed) et ne lit plus rien.
    let sonde = sonde(true);
    let dht = librqbit::dht::DhtState::with_config(librqbit::dht::DhtConfig {
        socket: Some(sonde.clone()),
        bootstrap_addrs: Some(vec!["127.0.0.2:39001".to_string()]),
        inbound_queries_per_second: Some(10),
        ..Default::default()
    })
    .await
    .expect("dht");

    let source = SocketAddr::from((Ipv4Addr::LOCALHOST, 40000));
    for _ in 0..60 {
        sonde
            .tx
            .send((requete_ping(), source))
            .expect("injecter la requete");
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let reponses = sonde.envois.lock().expect("envois").len();
    let stats = dht.stats();
    assert!(
        reponses <= 30,
        "{reponses} reponses pour 60 requetes — le budget inbound n'a pas borne"
    );
    assert!(
        stats.dropped_inbound_queries >= 30,
        "dropped_inbound_queries={} attendu >= 30",
        stats.dropped_inbound_queries
    );
    dht.cancellation_token().cancel();
}

/// Regression volume/cadence : un infohash sans swarm (magnet en
/// stall) fait decroitre les vagues `get_peers` par backoff
/// exponentiel jittere jusqu'au plafond, au lieu d'interroger la DHT
/// a cadence fixe. Intervalle de base reduit pour le test
/// (`requery_interval`) — le mecanisme est identique en production.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn magnet_stall_cadence_decroit_vers_le_plafond() {
    let sonde = sonde(true);
    let cap = Duration::from_secs(4);
    let dht = librqbit::dht::DhtState::with_config(librqbit::dht::DhtConfig {
        socket: Some(sonde.clone()),
        bootstrap_addrs: Some(vec!["127.0.0.2:39001".to_string()]),
        get_peers_backoff_cap: Some(cap),
        requery_interval: Some(Duration::from_secs(8)),
        ..Default::default()
    })
    .await
    .expect("dht");

    // Table peuplee par le bootstrap (echo) — laisser la premiere
    // vague atterrir puis lancer le lookup d'un infohash inexistant.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut stream = dht.get_peers(librqbit::dht::Id20::new([0x07u8; 20]), None);
    tokio::spawn(async move {
        use futures_util::StreamExt;
        while stream.next().await.is_some() {}
    });

    tokio::time::sleep(Duration::from_secs(9)).await;
    dht.cancellation_token().cancel();

    let mut instants: Vec<Instant> = sonde
        .envois
        .lock()
        .expect("envois")
        .iter()
        .filter(|e| e.get_peers)
        .map(|e| e.instant)
        .collect();
    instants.sort();
    assert!(
        instants.len() >= 3,
        "{} envois get_peers — vagues trop rares",
        instants.len()
    );
    assert!(
        instants.len() <= 16,
        "{} envois get_peers — la cadence n'a pas decru",
        instants.len()
    );

    // Intervalles entre vagues : un envoi separe du precedent par
    // plus d'un demi-intervalle de base ouvre une nouvelle vague.
    let mut vagues = Vec::new();
    let mut debut = instants[0];
    for w in instants.windows(2) {
        if w[1] - w[0] > Duration::from_millis(500) {
            vagues.push(w[1] - debut);
            debut = w[1];
        }
    }
    assert!(
        vagues.len() >= 2,
        "intervalles mesures insuffisants : {vagues:?}"
    );
    let premier = vagues[0];
    let dernier = *vagues.last().expect("vagues");
    for i in &vagues {
        assert!(
            *i <= cap + Duration::from_millis(500),
            "intervalle {i:?} au-dela du plafond {cap:?}"
        );
    }
    assert!(
        dernier >= premier.mul_f64(1.8),
        "pas de decroissance : premier={premier:?} dernier={dernier:?}"
    );
}

/// `base * 2^idle` plafonne a `cap`, jitter 0..25 % : le delai reste
/// dans `[0.75 * min(base*2^idle, cap), min(base*2^idle, cap)]`.
#[test]
fn backoff_dht_double_puis_plafonne() {
    let base = Duration::from_secs(60);
    let cap = Duration::from_secs(900);
    for idle in 1..=6u32 {
        let plafond_passe = (base * (1 << idle)).min(cap);
        for _ in 0..64 {
            let d = librqbit::dht::requery_backoff_delay(base, idle, cap);
            assert!(d <= plafond_passe, "delai {d:?} au-dessus du plafond");
            assert!(d >= plafond_passe * 3 / 4, "jitter excessif : {d:?}");
        }
    }
    // Profond dans le backoff : borne `cap` toujours respectee.
    for _ in 0..64 {
        let d = librqbit::dht::requery_backoff_delay(base, 15, cap);
        assert!(d <= cap && d >= cap * 3 / 4, "hors bornes : {d:?}");
    }
    // `cap` sous la base : le delai ne descend jamais sous `base`.
    for _ in 0..16 {
        let d = librqbit::dht::requery_backoff_delay(base, 5, Duration::from_secs(10));
        assert!(d <= base && d >= base * 3 / 4, "sous la base : {d:?}");
    }
}

/// Le jitter case la periodicite : des tirages repetes au meme point
/// du backoff ne donnent pas un intervalle constant (signature de
/// flux regulier observable par un relais).
#[test]
fn backoff_dht_jitter_non_periodique() {
    let base = Duration::from_secs(60);
    let cap = Duration::from_secs(900);
    let mut distincts = std::collections::HashSet::new();
    for _ in 0..32 {
        distincts.insert(librqbit::dht::requery_backoff_delay(base, 8, cap));
    }
    assert!(
        distincts.len() > 1,
        "delais constants — jitter absent (cadence periodique)"
    );
}
