// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `channel_sync` (ADR-0025 §2) : synchronisation periodique des
//! canaux suivis — pull par remote-select, jamais de push.
//!
//! Un canal par fenetre `channel_sync_interval` (round-robin avec
//! jitter — jamais tous les canaux en rafale) : le select
//! `{channel_pk, origin_id, metadata_type:[400,220,500]}` est
//! envoye a un pair de l'overlay — UDP clair (community) ou
//! encapsule stealth `ENCAP` vers un relais `CAP_DISCOVERY_RELAY`
//! choisi de facon deterministe par canal (ADR-0025 §5) ; les
//! reponses sont integrees par le chemin persistant de
//! `process_select_response` (signature verifiee +
//! `public_key == channel_pk` — anti-poisoning).
//!
//! `metadata_type` couvre `CHANNEL_TORRENT` (400 — contenu),
//! `COLLECTION_NODE` (220 — racine, pour apprendre le titre) et
//! `DELETED` (500 — pierres tombales). `CHANNEL_NODE` (200) reste
//! `Rejected` : Python n'a aucune classe de payload pour lui
//! (`UnknownBlobTypeException`).

use std::sync::Arc;
use std::time::Duration;

use onionbit_db::Database;

use crate::ipv8_stack::Ipv8Stack;

/// Boucle de synchronisation des canaux suivis.
pub async fn run_channel_sync(
    stack: Arc<Ipv8Stack>,
    db: Arc<Database>,
    interval: Duration,
    stop: &mut tokio::sync::watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Premier tick immediatement ignore (meme convention que le
    // gossip — determinisme au demarrage).
    tick.tick().await;
    let mut next = 0usize;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                let channels = db
                    .call("channels.subscribed", |c| {
                        onionbit_db::channel::subscribed_channels(c)
                    })
                    .await
                    .unwrap_or_default();
                if channels.is_empty() {
                    continue;
                }
                next %= channels.len();
                let (pk, origin_id) = channels[next].clone();
                next += 1;
                let json = serde_json::json!({
                    "channel_pk": hex::encode(&pk),
                    "origin_id": origin_id,
                    "metadata_type": [400, 220, 500],
                    "first": 1,
                    "last": 100,
                })
                .to_string()
                .into_bytes();
                // Graine deterministe par canal (repliage des octets
                // de la cle) — repartition inter-ponts ADR-0025 §5 :
                // un canal donne revient toujours au meme relais.
                let seed = pk
                    .iter()
                    .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(*b as u64));
                if !stack.send_channel_select(json, seed).await {
                    tracing::debug!("channel_sync: aucun pair interrogeable");
                }
            }
        }
    }
}
