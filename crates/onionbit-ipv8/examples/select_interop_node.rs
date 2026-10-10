// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Noeud d'interop remote-select/health/version (etape 95) :
//! `DiscoveryCommunity` + `ContentDiscoveryCommunity` sur le meme
//! endpoint, contre le noeud Python `py_select_node.py` (vraie
//! `ContentDiscoveryCommunity` Tribler sur `MetadataStore` pony).
//!
//! Preuves (marqueurs stderr) :
//!   `RUST_PEER_OK`     — pair Python verifie (intro echangee)
//!   `RUST_SELECT_RESP` — reponses select Python decompressees LZ4
//!                        et parsees `.mdblob` (types comptes)
//!   `RUST_SERVED_SELECT` — select Python servi (blob signe + LZ4)
//!   `RUST_VERSION_RESP`  — `VersionResponse` Python decodee
//!   `RUST_HEALTH_REQ`    — `HealthRequest` Python servi
//!   `RUST_HEALTH_RESP`   — `HealthPayload` Python integre
//!
//! Usage :
//! `select_interop_node --port N --target 127.0.0.1:P --duration S`

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_format::mdblob::{
    self, types, ChannelNodePayload, MetadataEntry, SignedPayloadHeader, TorrentMetadataPayload,
};
use onionbit_ipv8::content_discovery::{
    ContentDiscoveryCommunity, ContentDiscoverySettings, ContentProvider, HealthInfo,
};
use onionbit_ipv8::{DiscoveryCommunity, Network, UdpAddress, UdpEndpoint};

/// Infohashes servies par le provider Rust (deterministes).
const IH_RUST_A: [u8; 20] = [0xCC; 20];
const IH_RUST_B: [u8; 20] = [0xDD; 20];

/// `lz4.frame.compress` Python — cadre de fichier, contenu quelconque.
fn lz4_frame(data: &[u8]) -> Vec<u8> {
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    std::io::copy(&mut std::io::Cursor::new(data), &mut enc).unwrap();
    enc.finish().unwrap()
}

/// `lz4.frame.decompress` Python.
fn lz4_unframe(data: &[u8]) -> Vec<u8> {
    let mut dec = lz4_flex::frame::FrameDecoder::new(data);
    let mut out = Vec::new();
    std::io::copy(&mut dec, &mut out).unwrap_or(0);
    out
}

/// Provider minimal : deux entrees `REGULAR_TORRENT` signees par la
/// cle du noeud, serialisees `.mdblob` + cadre LZ4 (meme format que
/// `entries_to_chunk` Python — le but est que le parseur Python
/// `process_compressed_mdblob` les ingere).
struct InteropProvider {
    sk: LibNaClSecretKey,
    served: AtomicUsize,
    parsed_entries: AtomicUsize,
    health_reqs: AtomicUsize,
    healths_in: AtomicUsize,
}

type Fut<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl InteropProvider {
    fn signed_entry(sk: &LibNaClSecretKey, infohash: [u8; 20], title: &str, id: u64) -> Vec<u8> {
        let pk_bin = sk.public_key().to_bin();
        let mut pk = [0u8; 64];
        pk.copy_from_slice(&pk_bin[10..]);
        let entry = MetadataEntry::RegularTorrent(TorrentMetadataPayload {
            node: ChannelNodePayload {
                header: SignedPayloadHeader::new(types::REGULAR_TORRENT, 0, pk),
                id,
                origin_id: 0,
                timestamp: 1_770_000_000,
            },
            infohash,
            size: 424242,
            torrent_date: 1_770_000_000,
            title: title.to_string(),
            tags: "interop".to_string(),
            tracker_info: String::new(),
        });
        mdblob::encode_entry(&entry, sk).expect("serialisation mdblob")
    }
}

impl ContentProvider for InteropProvider {
    fn healths_for<'a>(&'a self, request_type: u8) -> Fut<'a, Vec<HealthInfo>> {
        self.health_reqs.fetch_add(1, Ordering::Relaxed);
        eprintln!("RUST_HEALTH_REQ|type={request_type}");
        Box::pin(async move {
            vec![
                HealthInfo {
                    infohash: IH_RUST_A,
                    seeders: 5,
                    leechers: 2,
                    last_check: 1_770_000_000,
                    tracker: String::new(),
                },
                HealthInfo {
                    infohash: IH_RUST_B,
                    seeders: 1,
                    leechers: 0,
                    last_check: 1_770_000_000,
                    tracker: "udp://tracker.example:6969".to_string(),
                },
            ]
        })
    }

    fn process_health<'a>(&'a self, healths: &'a [HealthInfo]) -> Fut<'a, Vec<[u8; 20]>> {
        self.healths_in.fetch_add(healths.len(), Ordering::Relaxed);
        eprintln!("RUST_HEALTH_RESP|n={}", healths.len());
        for h in healths {
            eprintln!(
                "RUST_HEALTH|ih={}|{}/{}",
                hex::encode(h.infohash),
                h.seeders,
                h.leechers
            );
        }
        Box::pin(async move { Vec::new() })
    }

    fn remote_select<'a>(&'a self, json: &'a [u8]) -> Fut<'a, Vec<Vec<u8>>> {
        self.served.fetch_add(1, Ordering::Relaxed);
        let mut blob = Vec::new();
        blob.extend_from_slice(&Self::signed_entry(&self.sk, IH_RUST_A, "rust-alpha", 1));
        blob.extend_from_slice(&Self::signed_entry(&self.sk, IH_RUST_B, "rust-beta", 2));
        let chunk = lz4_frame(&blob);
        eprintln!(
            "RUST_SERVED_SELECT|json={}|chunk_bytes={}",
            String::from_utf8_lossy(json),
            chunk.len()
        );
        Box::pin(async move { vec![chunk] })
    }

    fn process_select_response<'a>(&'a self, blob: &'a [u8]) -> Fut<'a, Vec<serde_json::Value>> {
        let raw = lz4_unframe(blob);
        match mdblob::parse_blob(&raw) {
            Ok(entries) => {
                self.parsed_entries
                    .fetch_add(entries.len(), Ordering::Relaxed);
                let types: Vec<u16> = entries
                    .iter()
                    .filter_map(|e| e.header().map(|h| h.metadata_type))
                    .collect();
                let signed = entries
                    .iter()
                    .filter(|e| e.header().map(|h| h.verify_signature()).unwrap_or(false))
                    .count();
                eprintln!(
                    "RUST_SELECT_RESP|entries={}|types={types:?}|signed_ok={signed}",
                    entries.len()
                );
            }
            Err(e) => eprintln!("RUST_SELECT_RESP|parse_err={e}"),
        }
        Box::pin(async move { Vec::new() })
    }

    fn version_info(&self) -> (String, String) {
        ("OnionBit-interop/0.95".to_string(), "windows".to_string())
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut port = "0".to_string();
    let mut target: Option<String> = None;
    let mut duration = 14u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().unwrap(),
            "--target" => target = args.next(),
            "--duration" => duration = args.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }

    let ep = UdpEndpoint::bind(&format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let lan = UdpAddress::from("127.0.0.1:0".parse::<SocketAddr>().unwrap());
    let net = Arc::new(Network::default());
    let key = LibNaClSecretKey::generate();
    let discovery =
        DiscoveryCommunity::new(key.clone(), net.clone(), ep.clone(), lan.clone()).await;
    let provider = Arc::new(InteropProvider {
        sk: key.clone(),
        served: AtomicUsize::new(0),
        parsed_entries: AtomicUsize::new(0),
        health_reqs: AtomicUsize::new(0),
        healths_in: AtomicUsize::new(0),
    });
    let content = ContentDiscoveryCommunity::new(
        key,
        net.clone(),
        ep.clone(),
        provider.clone(),
        ContentDiscoverySettings::default(),
        discovery,
    )
    .await;
    let ep2 = ep.clone();
    tokio::spawn(async move {
        let _ = ep2.run().await;
    });

    eprintln!("rust select-interop sur {port}");
    let deadline = Instant::now() + Duration::from_secs(duration);
    let Some(t) = target else {
        eprintln!("pas de cible");
        return;
    };
    let py_addr = UdpAddress::from(t.parse::<SocketAddr>().unwrap());

    // Introduction sous le prefixe content-discovery jusqu'a
    // verification du pair Python.
    let mut peer_ok = false;
    while Instant::now() < deadline {
        if net.get_verified_by_address(&py_addr).is_some() {
            peer_ok = true;
            break;
        }
        let _ = content.walk_to(&py_addr).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !peer_ok {
        eprintln!("RUST_PEER_MISSING");
        return;
    }
    eprintln!("RUST_PEER_OK");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Rust -> Python : select plein (les 2 entrees FFA Python).
    let _ = content
        .send_remote_select(&py_addr, br#"{"first":1,"last":50}"#.to_vec())
        .await;
    // VersionRequest + HealthRequest (RANDOM=2).
    let _ = content.send_version_request(&py_addr).await;
    let _ = content
        .request_health(
            &py_addr,
            onionbit_ipv8::content_discovery::HEALTH_REQUEST_RANDOM,
        )
        .await;

    let wait = Instant::now() + Duration::from_secs(4);
    while Instant::now() < wait && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let flag = |name: &str, ok: bool| eprintln!("{name} : {}", if ok { "OK" } else { "FAIL" });
    flag("RUST_PEER_OK", peer_ok);
    flag(
        "RUST_SELECT_RESP_OK",
        provider.parsed_entries.load(Ordering::Relaxed) >= 1,
    );
    flag(
        "RUST_SERVED_SELECT",
        provider.served.load(Ordering::Relaxed) >= 1,
    );
    flag(
        "RUST_HEALTH_REQ_SERVED",
        provider.health_reqs.load(Ordering::Relaxed) >= 1,
    );
    flag(
        "RUST_HEALTH_RESP",
        provider.healths_in.load(Ordering::Relaxed) >= 1,
    );
}
