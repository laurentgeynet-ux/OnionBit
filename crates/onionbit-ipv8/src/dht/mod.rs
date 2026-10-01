// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! DHT overlay IPv8 (equivalent de `pyipv8/ipv8/dht/`).
//!
//! - `routing` : `Node`, `Bucket`, `RoutingTable`, `calc_node_id`,
//!   `distance` (port fidele de `dht/routing.py`) ;
//! - `storage` : stockage borne des valeurs (`dht/storage.py`) ;
//! - `payloads` : les 10 messages DHT + blobs de valeur
//!   (`dht/payload.py`) ;
//! - `community` : `DhtCommunity` = `DHTCommunity` +
//!   `DHTDiscoveryCommunity` (meme `community_id`).

pub mod community;
pub mod payloads;
pub mod routing;
pub mod storage;

pub use community::{DhtCommunity, DhtError, DhtValue, FindOutcome, DHT_COMMUNITY_ID};
pub use routing::{calc_node_id, distance, Node, RoutingTable};
pub use storage::Storage;
