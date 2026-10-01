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
