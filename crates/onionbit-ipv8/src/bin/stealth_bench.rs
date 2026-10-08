// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `stealth_bench` — outils de banc ADR-0017 (etape 55). Binaire de
//! dev uniquement : orchestration socket-level du transport morphe,
//! jamais de compteur interne comme oracle.
//!
//! Sous-commandes :
//! - `link --key <stealth_bridge.key> --addr <ip:port>` : derive la
//!   `bridge_pk` du secret persiste et imprime le lien
//!   `onionbit-bridge://` (l'adresse passee peut etre celle du tap,
//!   pour que tout le trafic transite par la capture).
//! - `tap --listen <a> --upstream <a> --pcap <f> [--loss F]
//!   [--dup F] [--reorder F] [--jitter-ms N]` : relais UDP
//!   bidirectionnel qui consigne chaque datagramme en PCAP
//!   (LINKTYPE_RAW, en-tetes IPv4/UDP fabriquees) et injecte
//!   optionnellement perte/duplication/reordonnancement.
//! - `probe --target <a> [--count N] [--pace-ms N] [--window-ms N]
//!   [--replay <pcap>]` : probing actif — sondes calibrees (tailles
//!   hs1/trame, garbage, look-alikes DNS/IPv8) + rejoue optionnel de
//!   datagrammes captures ; rapporte reponses et ratio
//!   d'amplification `recv/sent` en JSON.
//! - `synth --kind dns|quic|wg|noise|ipv8 --pcap <f> [--count N]
//!   [--span-ms N]` : corpus synthetique etiquete pour le
//!   classifieur comparatif.
//! - `analyze --pcap <f> [--against <f2>] [--needle label=hex]...`
//!   : rapport JSON — entropie, marqueurs protocolaires, constance
//!   des prefixes, histogramme de tailles, doublons (et inter-runs).
//! - `classify --cap <label>=<pcap> ... [--window-ms N]` : vecteurs
//!   de features par fenetre temporelle + 1-NN leave-one-out —
//!   mesure honnete de la separation observable entre corpus.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- util

fn arg(args: &[String], name: &str) -> Option<String> {
    let flag = format!("--{name}");
    args.iter()
        .position(|a| a == &flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn arg_req(args: &[String], name: &str) -> Result<String, String> {
    arg(args, name).ok_or_else(|| format!("argument --{name} manquant"))
}

fn arg_f(args: &[String], name: &str, def: f64) -> Result<f64, String> {
    match arg(args, name) {
        None => Ok(def),
        Some(v) => v
            .parse()
            .map_err(|_| format!("--{name} : flottant attendu, recu {v}")),
    }
}

fn arg_u(args: &[String], name: &str, def: u64) -> Result<u64, String> {
    match arg(args, name) {
        None => Ok(def),
        Some(v) => v
            .parse()
            .map_err(|_| format!("--{name} : entier attendu, recu {v}")),
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn frand() -> f64 {
    let mut b = [0u8; 8];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

fn rand_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut v);
    v
}

// ---------------------------------------------------------------- pcap

/// Ecrivain PCAP (libpcap LE, LINKTYPE_RAW = IPv4 nu). Les en-tetes
/// IPv4/UDP sont fabriquees a partir des adresses reelles — la
/// charge utile est telle que vue sur le fil.
struct PcapW {
    w: BufWriter<File>,
}

impl PcapW {
    fn create(path: &str) -> std::io::Result<Self> {
        let mut w = BufWriter::new(File::create(path)?);
        w.write_all(&0xa1b2c3d4u32.to_le_bytes())?; // magic
        w.write_all(&2u16.to_le_bytes())?; // version majeure
        w.write_all(&4u16.to_le_bytes())?; // mineure
        w.write_all(&0i32.to_le_bytes())?; // tz
        w.write_all(&0u32.to_le_bytes())?; // sigfigs
        w.write_all(&65535u32.to_le_bytes())?; // snaplen
        w.write_all(&101u32.to_le_bytes())?; // LINKTYPE_RAW
        Ok(Self { w })
    }

    /// `ts_us` en microsecondes Unix. v4 uniquement (le banc tourne
    /// en loopback).
    fn rec(
        &mut self,
        ts_us: u64,
        src: SocketAddr,
        dst: SocketAddr,
        payload: &[u8],
    ) -> std::io::Result<()> {
        let (SocketAddr::V4(s4), SocketAddr::V4(d4)) = (src, dst) else {
            return Ok(()); // v6 non consigne — hors perimetre du banc
        };
        let total = (20 + 8 + payload.len()) as u32;
        let mut pkt = Vec::with_capacity(total as usize);
        // IPv4
        pkt.push(0x45); // version + ihl
        pkt.push(0); // dscp
        pkt.extend_from_slice(&(total as u16).to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes()); // id
        pkt.extend_from_slice(&0x4000u16.to_be_bytes()); // DF
        pkt.push(64); // ttl
        pkt.push(17); // udp
        pkt.extend_from_slice(&0u16.to_be_bytes()); // cksum (calcule plus bas)
        pkt.extend_from_slice(&s4.ip().octets());
        pkt.extend_from_slice(&d4.ip().octets());
        let cksum = ipv4_cksum(&pkt);
        pkt[10] = (cksum >> 8) as u8;
        pkt[11] = cksum as u8;
        // UDP
        pkt.extend_from_slice(&s4.port().to_be_bytes());
        pkt.extend_from_slice(&d4.port().to_be_bytes());
        pkt.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes()); // cksum optionnel en v4
        pkt.extend_from_slice(payload);

        self.w
            .write_all(&((ts_us / 1_000_000) as u32).to_le_bytes())?;
        self.w
            .write_all(&((ts_us % 1_000_000) as u32).to_le_bytes())?;
        self.w.write_all(&(pkt.len() as u32).to_le_bytes())?;
        self.w.write_all(&(pkt.len() as u32).to_le_bytes())?;
        self.w.write_all(&pkt)?;
        Ok(())
    }
}

fn ipv4_cksum(h: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for c in h[..20].chunks(2) {
        sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Datagramme relu d'un PCAP : payload UDP + adresses + horodatage.
struct Cap {
    ts_us: u64,
    src: SocketAddr,
    payload: Vec<u8>,
}

fn read_pcap(path: &str) -> Result<Vec<Cap>, String> {
    let mut f = File::open(path).map_err(|e| format!("pcap {path}: {e}"))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)
        .map_err(|e| format!("pcap {path}: {e}"))?;
    if data.len() < 24 {
        return Err(format!("pcap {path} trop court"));
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap_or([0; 4]));
    let le = match magic {
        0xa1b2c3d4 => true,
        0xd4c3b2a1 => false,
        m => return Err(format!("pcap {path} : magic {m:#x} inconnu")),
    };
    let rd32 = |b: &[u8]| -> u32 {
        let a: [u8; 4] = b.try_into().unwrap_or([0; 4]);
        if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    let linktype = rd32(&data[20..24]);
    let mut out = Vec::new();
    let mut off = 24usize;
    while off + 16 <= data.len() {
        let ts =
            rd32(&data[off..off + 4]) as u64 * 1_000_000 + rd32(&data[off + 4..off + 8]) as u64;
        let incl = rd32(&data[off + 8..off + 12]) as usize;
        off += 16;
        if off + incl > data.len() {
            break;
        }
        let pkt = &data[off..off + incl];
        off += incl;
        // LINKTYPE_RAW (101) : IPv4 nu → on saute ip+udp.
        if linktype != 101 || pkt.len() < 28 || pkt[0] >> 4 != 4 {
            continue;
        }
        let ihl = ((pkt[0] & 0x0f) as usize) * 4;
        if pkt.len() < ihl + 8 || pkt[9] != 17 {
            continue;
        }
        let src = SocketAddr::from((
            [pkt[12], pkt[13], pkt[14], pkt[15]],
            u16::from_be_bytes([pkt[ihl], pkt[ihl + 1]]),
        ));
        out.push(Cap {
            ts_us: ts,
            src,
            payload: pkt[ihl + 8..].to_vec(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- link

fn cmd_link(args: &[String]) -> Result<(), String> {
    let key = arg_req(args, "key")?;
    let addr: SocketAddr = arg_req(args, "addr")?
        .parse()
        .map_err(|_| "--addr : ip:port attendu".to_string())?;
    let sk_raw = std::fs::read(&key).map_err(|e| format!("cle {key}: {e}"))?;
    let sk: &[u8; 32] = sk_raw
        .as_slice()
        .try_into()
        .map_err(|_| format!("cle {key}: 32 octets attendus, recu {}", sk_raw.len()))?;
    let pk = onionbit_crypto::stealth::bridge_public(sk);
    println!(
        "{}",
        onionbit_ipv8::stealth_transport::BridgeEntry { addr, pk }.to_link()
    );
    Ok(())
}

// ---------------------------------------------------------------- tap

/// Relais UDP → PCAP. Le premier emetteur non-upstream devient le
/// client appris ; tout datagramme venant d'`upstream` lui est
/// retourne. Pertes/dups/delais s'appliquent aux deux directions.
async fn cmd_tap(args: &[String]) -> Result<(), String> {
    let listen: SocketAddr = arg_req(args, "listen")?
        .parse()
        .map_err(|_| "--listen invalide".to_string())?;
    let upstream: SocketAddr = arg_req(args, "upstream")?
        .parse()
        .map_err(|_| "--upstream invalide".to_string())?;
    let pcap_path = arg_req(args, "pcap")?;
    let loss = arg_f(args, "loss", 0.0)?;
    let dup = arg_f(args, "dup", 0.0)?;
    let reorder = arg_f(args, "reorder", 0.0)?;
    let jitter = arg_u(args, "jitter-ms", 50)?;

    let sock = Arc::new(
        tokio::net::UdpSocket::bind(listen)
            .await
            .map_err(|e| format!("bind {listen}: {e}"))?,
    );
    let pcap = Arc::new(Mutex::new(
        PcapW::create(&pcap_path).map_err(|e| format!("pcap {pcap_path}: {e}"))?,
    ));
    let client: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
    let n_rx = AtomicU64::new(0);
    let n_drop = AtomicU64::new(0);
    let n_dup = AtomicU64::new(0);

    eprintln!(
        "tap: {listen} <-> {upstream} (loss={loss} dup={dup} reorder={reorder} jitter={jitter}ms)"
    );
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // Windows remonte l'ICMP port-unreachable de
                // l'amont comme erreur de recv — bruit sans
                // consequence ; on ne loggue que par paquets de 128.
                if n_rx.fetch_add(1, Ordering::Relaxed).is_multiple_of(128) {
                    eprintln!("tap recv: {e}");
                }
                continue;
            }
        };
        n_rx.fetch_add(1, Ordering::Relaxed);
        // Direction : tout ce qui n'est pas upstream vient du client.
        let dst = if src == upstream {
            *client.lock().unwrap_or_else(|e| e.into_inner())
        } else {
            *client.lock().unwrap_or_else(|e| e.into_inner()) = Some(src);
            Some(upstream)
        };
        let Some(dst) = dst else {
            continue; // trafic amont avant tout client : ignore
        };
        pcap.lock()
            .unwrap_or_else(|e| e.into_inner())
            .rec(now_us(), src, dst, &buf[..n])
            .ok();
        // Impairments.
        if frand() < loss {
            n_drop.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let payload = buf[..n].to_vec();
        if frand() < reorder {
            let d = Duration::from_millis((frand() * jitter as f64) as u64);
            let (s2, p2) = (sock.clone(), payload.clone());
            tokio::spawn(async move {
                tokio::time::sleep(d).await;
                s2.send_to(&p2, dst).await.ok();
            });
        } else {
            sock.send_to(&payload, dst).await.ok();
        }
        if frand() < dup {
            n_dup.fetch_add(1, Ordering::Relaxed);
            sock.send_to(&payload, dst).await.ok();
        }
        let rx = n_rx.load(Ordering::Relaxed);
        if rx.is_multiple_of(1000) {
            eprintln!(
                "tap: rx={rx} drop={} dup={}",
                n_drop.load(Ordering::Relaxed),
                n_dup.load(Ordering::Relaxed)
            );
        }
    }
}

// ---------------------------------------------------------------- probe

/// Sondes calibrees : tailles de trames/handshake stealth, garbage
/// uniforme, look-alikes IPv8/DNS — tout doit etre absorbe en
/// silence (uniformite du rejet).
fn probe_corpus() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = Vec::new();
    for n in [
        1usize, 16, 32, 44, 64, 97, 128, 197, 256, 512, 1024, 1200, 1280,
    ] {
        v.push(rand_bytes(n));
    }
    v.push(vec![0u8; 1280]);
    v.push(vec![0xffu8; 1280]);
    // look-alike IPv8 : prefixe version + community id ext + garbage.
    let mut p = vec![0x00, 0x02];
    p.extend_from_slice(&onionbit_ipv8::ext::EXT_COMMUNITY_ID);
    p.extend_from_slice(&[0xf0]); // msg_id arbitraire
    p.extend_from_slice(&rand_bytes(64));
    v.push(p);
    // look-alike DNS query.
    let mut d = rand_bytes(12);
    d[2] = 0x01;
    d.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01");
    v.push(d);
    // look-alike BitTorrent peer-wire.
    let mut bt = vec![0x13];
    bt.extend_from_slice(b"BitTorrent protocol");
    bt.extend_from_slice(&rand_bytes(48));
    v.push(bt);
    v
}

async fn cmd_probe(args: &[String]) -> Result<(), String> {
    let target: SocketAddr = arg_req(args, "target")?
        .parse()
        .map_err(|_| "--target invalide".to_string())?;
    let count = arg_u(args, "count", 500)? as usize;
    let pace = arg_u(args, "pace-ms", 2)?;
    let window = arg_u(args, "window-ms", 1500)?;
    let replay = arg(args, "replay");

    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("bind probe: {e}"))?;
    let corpus = probe_corpus();
    let mut sent_bytes: u64 = 0;
    let mut sent: u64 = 0;
    for i in 0..count {
        let d = &corpus[i % corpus.len()];
        sock.send_to(d, target)
            .await
            .map_err(|e| format!("probe send: {e}"))?;
        sent_bytes += d.len() as u64;
        sent += 1;
        if pace > 0 {
            tokio::time::sleep(Duration::from_millis(pace)).await;
        }
    }
    if let Some(p) = replay {
        for c in read_pcap(&p)? {
            sock.send_to(&c.payload, target)
                .await
                .map_err(|e| format!("replay send: {e}"))?;
            sent_bytes += c.payload.len() as u64;
            sent += 1;
            if pace > 0 {
                tokio::time::sleep(Duration::from_millis(pace)).await;
            }
        }
    }
    // Fenetre de collecte : toute reponse est une fuite.
    let mut recv_bytes: u64 = 0;
    let mut recv: u64 = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(window);
    let mut buf = vec![0u8; 65536];
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => {
                recv += 1;
                recv_bytes += n as u64;
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
    let amp = if sent_bytes > 0 {
        recv_bytes as f64 / sent_bytes as f64
    } else {
        0.0
    };
    println!(
        "{}",
        serde_json::json!({
            "target": target.to_string(),
            "probes_sent": sent,
            "bytes_sent": sent_bytes,
            "responses": recv,
            "bytes_recv": recv_bytes,
            "amplification": amp,
            "oracle_silence": recv == 0,
            "oracle_amplification_le_1": amp <= 1.0,
        })
    );
    Ok(())
}

// ---------------------------------------------------------------- synth

/// Corpus synthetique etiquete — formes de paquets representatives
/// (tailles/direction/cadence), contenu pseudo-aleatoire. Sert de
/// reference au classifieur comparatif, pas de preuve de
/// dissimulation.
fn cmd_synth(args: &[String]) -> Result<(), String> {
    let kind = arg_req(args, "kind")?;
    let pcap_path = arg_req(args, "pcap")?;
    let count = arg_u(args, "count", 2000)? as usize;
    let span_ms = arg_u(args, "span-ms", 60_000)?;
    let a: SocketAddr = "10.0.0.1:40000".parse().map_err(|e| format!("{e}"))?;
    let b: SocketAddr = "10.0.0.2:40001".parse().map_err(|e| format!("{e}"))?;

    let mut w = PcapW::create(&pcap_path).map_err(|e| format!("pcap: {e}"))?;
    let mut ts = now_us();
    for _ in 0..count {
        let step_us = (span_ms * 1000 / count.max(1) as u64).max(1);
        ts += step_us + (frand() * step_us as f64) as u64;
        let (src, dst, payload) = match kind.as_str() {
            // requete petite montante, reponse descendante plus grosse
            "dns" => {
                if frand() < 0.5 {
                    (a, b, rand_bytes(30 + (frand() * 60.0) as usize))
                } else {
                    (b, a, rand_bytes(80 + (frand() * 320.0) as usize))
                }
            }
            // initial ~1200+ puis flux bidirectionnel variable
            "quic" => {
                let init = frand() < 0.1;
                let n = if init {
                    1200 + (frand() * 150.0) as usize
                } else {
                    200 + (frand() * 1000.0) as usize
                };
                if frand() < 0.5 {
                    (a, b, rand_bytes(n))
                } else {
                    (b, a, rand_bytes(n))
                }
            }
            // handshake a tailles fixes puis donnees
            "wg" => {
                let r = frand();
                let n = if r < 0.02 {
                    148
                } else if r < 0.04 {
                    92
                } else {
                    96 + (frand() * 1300.0) as usize
                };
                if frand() < 0.6 {
                    (a, b, rand_bytes(n))
                } else {
                    (b, a, rand_bytes(n))
                }
            }
            // ipv8 legacy : prefixe proto + community id + payload
            // signe-like — tailles typiques hello/intro/dht.
            "ipv8" => {
                let mut p = vec![0x00, 0x02];
                p.extend_from_slice(&onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID);
                p.push(248); // introduction_request-like
                p.extend_from_slice(&rand_bytes(60 + (frand() * 200.0) as usize));
                if frand() < 0.5 {
                    (a, b, p)
                } else {
                    (b, a, p)
                }
            }
            // bruit : cadence et tailles uniformes, direction pile
            "noise" => {
                let n = 40 + (frand() * 1360.0) as usize;
                if frand() < 0.5 {
                    (a, b, rand_bytes(n))
                } else {
                    (b, a, rand_bytes(n))
                }
            }
            k => return Err(format!("kind inconnu: {k}")),
        };
        w.rec(ts, src, dst, &payload)
            .map_err(|e| format!("pcap rec: {e}"))?;
    }
    println!("synth {kind}: {count} datagrammes -> {pcap_path}");
    Ok(())
}

// ---------------------------------------------------------------- analyze

/// Features scalaires d'une fenetre de capture — partagees entre
/// `analyze` (reportage) et `classify` (1-NN).
struct Feats {
    #[allow(dead_code)] // expose dans le JSON via `datagrams`
    count: usize,
    mean_len: f64,
    std_len: f64,
    entropy: f64,
    up_ratio: f64,
    mean_iat_ms: f64,
    pps: f64,
}

fn window_feats(caps: &[Cap], up: SocketAddr) -> Feats {
    let n = caps.len();
    if n == 0 {
        return Feats {
            count: 0,
            mean_len: 0.0,
            std_len: 0.0,
            entropy: 0.0,
            up_ratio: 0.0,
            mean_iat_ms: 0.0,
            pps: 0.0,
        };
    }
    let lens: Vec<f64> = caps.iter().map(|c| c.payload.len() as f64).collect();
    let mean = lens.iter().sum::<f64>() / n as f64;
    let var = lens.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / n as f64;
    let mut hist = [0u64; 256];
    let mut up_bytes = 0u64;
    let mut total = 0u64;
    for c in caps {
        for b in &c.payload {
            hist[*b as usize] += 1;
        }
        if c.src == up {
            up_bytes += c.payload.len() as u64;
        }
        total += c.payload.len() as u64;
    }
    let ent = hist
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = *c as f64 / total as f64;
            -p * p.log2()
        })
        .sum::<f64>();
    let mut iat = Vec::new();
    for w in caps.windows(2) {
        iat.push(w[1].ts_us.saturating_sub(w[0].ts_us) as f64 / 1000.0);
    }
    let mean_iat = if iat.is_empty() {
        0.0
    } else {
        iat.iter().sum::<f64>() / iat.len() as f64
    };
    let span_s = (caps.last().map(|c| c.ts_us).unwrap_or(0)
        - caps.first().map(|c| c.ts_us).unwrap_or(0)) as f64
        / 1e6;
    Feats {
        count: n,
        mean_len: mean,
        std_len: var.sqrt(),
        entropy: ent,
        up_ratio: if total > 0 {
            up_bytes as f64 / total as f64
        } else {
            0.0
        },
        mean_iat_ms: mean_iat,
        pps: if span_s > 0.0 { n as f64 / span_s } else { 0.0 },
    }
}

fn cmd_analyze(args: &[String]) -> Result<(), String> {
    let pcap_path = arg_req(args, "pcap")?;
    let caps = read_pcap(&pcap_path)?;
    let up: SocketAddr = arg(args, "up")
        .and_then(|s| s.parse().ok())
        .or_else(|| caps.first().map(|c| c.src))
        .ok_or("capture vide".to_string())?;

    // Marqueurs protocolaires — ce qu'un DPI chercherait.
    let mut needles: Vec<(String, Vec<u8>)> = vec![
        ("LibNaCLPK:".to_string(), b"LibNaCLPK:".to_vec()),
        ("LibNaCLSK:".to_string(), b"LibNaCLSK:".to_vec()),
        (
            "ext-community-id".to_string(),
            onionbit_ipv8::ext::EXT_COMMUNITY_ID.to_vec(),
        ),
        (
            "discovery-community-id".to_string(),
            onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID.to_vec(),
        ),
        (
            "dht-community-id".to_string(),
            onionbit_ipv8::dht::DHT_COMMUNITY_ID.to_vec(),
        ),
        (
            "content-discovery-community-id".to_string(),
            onionbit_ipv8::content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID.to_vec(),
        ),
        ("onionbit".to_string(), b"onionbit".to_vec()),
        ("OnionBit".to_string(), b"OnionBit".to_vec()),
        (
            "bittorrent-proto".to_string(),
            b"BitTorrent protocol".to_vec(),
        ),
        ("dht-query".to_string(), b"d1:ad2:id20:".to_vec()),
        ("dht-reply".to_string(), b"d1:rd2:id20:".to_vec()),
    ];
    // Aiguilles supplementaires : --needle label=hex (les
    // community_id tunnel vivent hors de cette crate).
    for a in args {
        if let Some(rest) = a.strip_prefix("--needle=") {
            if let Some((label, hx)) = rest.split_once('=') {
                if let Ok(bytes) = hex::decode(hx) {
                    needles.push((label.to_string(), bytes));
                }
            }
        }
    }

    let mut marker_hits: HashMap<String, u64> = HashMap::new();
    for c in &caps {
        for (label, needle) in &needles {
            if c.payload
                .windows(needle.len())
                .any(|w| w == needle.as_slice())
            {
                *marker_hits.entry(label.clone()).or_default() += 1;
            }
        }
    }
    // prefixe IPv8 : octet0 ∈ {0,1,2} suivi d'un community_id connu —
    // deja couvert par les aiguilles community-id, on teste aussi le
    // couple version+id au tout debut du datagramme.
    let mut ipv8_prefixed = 0u64;
    for c in &caps {
        if c.payload.len() > 22
            && matches!(c.payload[0], 0..=2)
            && (c.payload[1..21] == onionbit_ipv8::ext::EXT_COMMUNITY_ID[..]
                || c.payload[1..21] == onionbit_ipv8::discovery::DISCOVERY_COMMUNITY_ID[..])
        {
            ipv8_prefixed += 1;
        }
    }

    // Constance des prefixes : part du byte dominant a chaque offset
    // 0..16 — un magic fixe produirait 1.0.
    let min_len = caps.iter().map(|c| c.payload.len()).min().unwrap_or(0);
    let offsets = min_len.min(16);
    let mut constancy = Vec::new();
    for i in 0..offsets {
        let mut freq = [0u64; 256];
        for c in &caps {
            freq[c.payload[i] as usize] += 1;
        }
        let top = *freq.iter().max().unwrap_or(&0) as f64 / caps.len().max(1) as f64;
        constancy.push(top);
    }

    // Histogramme de tailles + doublons intra-capture.
    let buckets = [64usize, 128, 256, 512, 768, 1024, 1280, 1500, usize::MAX];
    let mut hist = vec![0u64; buckets.len()];
    let mut seen: HashMap<u64, usize> = HashMap::new();
    let mut dup = 0u64;
    for c in &caps {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in &c.payload {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        if seen.insert(h, 1).is_some() {
            dup += 1;
        }
        let i = buckets
            .iter()
            .position(|b| c.payload.len() <= *b)
            .unwrap_or(buckets.len() - 1);
        hist[i] += 1;
    }

    let feats = window_feats(&caps, up);

    // Constance inter-runs : `--against` compte les datagrammes
    // identiques entre deux captures (attendu : 0).
    let mut against = serde_json::Value::Null;
    if let Some(p2) = arg(args, "against") {
        let caps2 = read_pcap(&p2)?;
        let set2: std::collections::HashSet<u64> = caps2
            .iter()
            .map(|c| {
                let mut h: u64 = 0xcbf29ce484222325;
                for b in &c.payload {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                h
            })
            .collect();
        let shared = caps
            .iter()
            .filter(|c| {
                let mut h: u64 = 0xcbf29ce484222325;
                for b in &c.payload {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
                set2.contains(&h)
            })
            .count();
        against = serde_json::json!({ "file": p2, "identical_datagrams": shared });
    }

    let max_const = constancy.iter().cloned().fold(0.0, f64::max);
    println!(
        "{}",
        serde_json::json!({
            "file": pcap_path,
            "datagrams": caps.len(),
            "bytes": caps.iter().map(|c| c.payload.len()).sum::<usize>(),
            "entropy_bits_per_byte": feats.entropy,
            "features": {
                "mean_len": feats.mean_len,
                "std_len": feats.std_len,
                "up_ratio": feats.up_ratio,
                "mean_iat_ms": feats.mean_iat_ms,
                "pps": feats.pps,
            },
            "marker_hits": marker_hits,
            "ipv8_prefixed": ipv8_prefixed,
            "prefix_constancy_top_share": constancy,
            "size_histogram": hist,
            "duplicate_datagrams": dup,
            "against": against,
            "oracle_no_markers": marker_hits.values().all(|v| *v == 0) && ipv8_prefixed == 0,
            "oracle_no_constants": max_const < 0.9 && dup == 0,
            "oracle_entropy": feats.entropy > 6.5,
        })
    );
    Ok(())
}

// ---------------------------------------------------------------- classify

fn cmd_classify(args: &[String]) -> Result<(), String> {
    let window_ms = arg_u(args, "window-ms", 2000)?;
    let mut corpus: Vec<(String, Vec<Feats>)> = Vec::new();
    for a in args {
        if let Some(rest) = a.strip_prefix("--cap=") {
            let Some((label, path)) = rest.split_once('=') else {
                return Err(format!("--cap label=fichier attendu, recu {rest}"));
            };
            let caps = read_pcap(path)?;
            let up = caps
                .first()
                .map(|c| c.src)
                .ok_or_else(|| format!("{path} vide"))?;
            // decoupage en fenetres temporelles fixes
            let t0 = caps.first().map(|c| c.ts_us).unwrap_or(0);
            let span = window_ms * 1000;
            let mut buckets: HashMap<u64, Vec<Cap>> = HashMap::new();
            for c in caps {
                let k = c.ts_us.saturating_sub(t0) / span;
                buckets.entry(k).or_default().push(c);
            }
            let feats: Vec<Feats> = buckets
                .into_values()
                .filter(|w| w.len() >= 4) // fenetres quasi-vides : features sans sens
                .map(|w| window_feats(&w, up))
                .collect();
            corpus.push((label.to_string(), feats));
        }
    }
    if corpus.len() < 2 {
        return Err("au moins deux corpus --cap attendus".to_string());
    }
    // vecteur de features -> [f64;6] ; normalisation z sur le corpus
    let to_vec = |f: &Feats| -> [f64; 6] {
        [
            f.mean_len,
            f.std_len,
            f.entropy,
            f.up_ratio,
            f.mean_iat_ms.ln().max(0.0),
            f.pps.ln().max(0.0),
        ]
    };
    let mut all: Vec<(&str, [f64; 6])> = Vec::new();
    for (l, fs) in &corpus {
        for f in fs {
            all.push((l.as_str(), to_vec(f)));
        }
    }
    let mut mu = [0.0f64; 6];
    let mut sd = [0.0f64; 6];
    for (_, v) in &all {
        for i in 0..6 {
            mu[i] += v[i] / all.len() as f64;
        }
    }
    for (_, v) in &all {
        for i in 0..6 {
            sd[i] += (v[i] - mu[i]).powi(2) / all.len() as f64;
        }
    }
    for s in sd.iter_mut() {
        *s = s.sqrt().max(1e-9);
    }
    // 1-NN leave-one-out
    let mut per_label: HashMap<String, (u64, u64)> = HashMap::new(); // (ok, total)
    for i in 0..all.len() {
        let (li, vi) = &all[i];
        let mut best = f64::MAX;
        let mut bl = "";
        for (j, (lj, vj)) in all.iter().enumerate() {
            if i == j {
                continue;
            }
            let d: f64 = (0..6)
                .map(|k| ((vi[k] - mu[k]) / sd[k] - (vj[k] - mu[k]) / sd[k]).powi(2))
                .sum::<f64>();
            if d < best {
                best = d;
                bl = lj;
            }
        }
        let e = per_label.entry(li.to_string()).or_default();
        e.1 += 1;
        if bl == *li {
            e.0 += 1;
        }
    }
    let mut report = serde_json::Map::new();
    let mut tot_ok = 0u64;
    let mut tot = 0u64;
    for (l, (ok, n)) in &per_label {
        report.insert(
            l.clone(),
            serde_json::json!({ "windows": n, "correct": ok, "accuracy": *ok as f64 / *n.max(&1) as f64 }),
        );
        tot_ok += ok;
        tot += n;
    }
    println!(
        "{}",
        serde_json::json!({
            "windows_total": tot,
            "accuracy_global": if tot > 0 { tot_ok as f64 / tot as f64 } else { 0.0 },
            "per_label": report,
            "note": "mesure honnete de la separation observable — un classifieur plus fort peut mieux faire ; l'objectif est la non-regression, pas de battre tous les classifieurs",
        })
    );
    Ok(())
}

// ---------------------------------------------------------------- main

fn usage() -> ! {
    eprintln!(
        "stealth_bench <cmd> [args]\n\
         \x20 link --key <f> --addr <ip:port>\n\
         \x20 tap --listen <a> --upstream <a> --pcap <f> [--loss F] [--dup F] [--reorder F] [--jitter-ms N]\n\
         \x20 probe --target <a> [--count N] [--pace-ms N] [--window-ms N] [--replay <pcap>]\n\
         \x20 synth --kind dns|quic|wg|noise|ipv8 --pcap <f> [--count N] [--span-ms N]\n\
         \x20 analyze --pcap <f> [--against <f2>] [--up <ip:port>] [--needle label=hex]...\n\
         \x20 classify --cap label=<pcap> ... [--window-ms N]"
    );
    std::process::exit(2);
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        usage();
    };
    let rest = &args[1..];
    let r = match cmd {
        "link" => cmd_link(rest),
        "tap" => cmd_tap(rest).await,
        "probe" => cmd_probe(rest).await,
        "synth" => cmd_synth(rest),
        "analyze" => cmd_analyze(rest),
        "classify" => cmd_classify(rest),
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("stealth_bench: {e}");
        std::process::exit(1);
    }
}
