//! Pairs et annuaire reseau (equivalent de `peer.py` et
//! `peerdiscovery/network.py`).
//!
//! Un `Peer` = cle publique binaire (`LibNaClPK:…`) + adresses connues.
//! Le `Network` maintient les index par cle et par adresse, avec un
//! plafond de pairs verifies par community (comme
//! `Network.get_peers_for_service`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;

use tribler_crypto::ipv8::keys::LibNaClPublicKey;

use crate::address::UdpAddress;
use crate::CommunityId;

/// Pair IPv8 (equivalent de `peer.Peer`).
#[derive(Debug, Clone)]
pub struct Peer {
    /// Cle publique binaire (`key_to_bin()` Python).
    pub public_key_bin: Vec<u8>,
    /// Adresse principale connue (WAN estime).
    pub address: Option<UdpAddress>,
    /// MID = SHA-1 de la cle publique.
    pub mid: [u8; 20],
}

impl Peer {
    /// Cree un pair depuis sa cle publique binaire.
    pub fn new(public_key_bin: Vec<u8>, address: Option<UdpAddress>) -> Option<Self> {
        let pk = LibNaClPublicKey::from_bin(&public_key_bin).ok()?;
        Some(Self {
            mid: pk.mid(),
            public_key_bin,
            address,
        })
    }
}

/// Annuaire reseau (equivalent de `peerdiscovery.network.Network`).
///
/// Version minimale de l'etape 9 : index par cle publique et par
/// adresse, comptage par service. Le churn/graph de sorties arrive a
/// l'etape 11.
#[derive(Default)]
pub struct Network {
    /// Pairs verifies (signature valide vue) indexe par cle publique.
    by_key: Mutex<HashMap<Vec<u8>, Peer>>,
    /// Index adresse → cle publique.
    by_addr: Mutex<HashMap<SocketAddr, Vec<u8>>>,
    /// Services (community_id) declares par chaque pair.
    services: Mutex<HashMap<Vec<u8>, Vec<CommunityId>>>,
}

impl Network {
    /// Enregistre un pair verifie (signature OK) et son adresse.
    pub fn add_verified(&self, peer: Peer) {
        if let Some(addr) = &peer.address {
            if let Some(sa) = addr.to_socket_addr() {
                self.by_addr
                    .lock()
                    .unwrap()
                    .insert(sa, peer.public_key_bin.clone());
            }
        }
        self.by_key
            .lock()
            .unwrap()
            .insert(peer.public_key_bin.clone(), peer);
    }

    /// Recherche par cle publique.
    pub fn get_by_key(&self, public_key_bin: &[u8]) -> Option<Peer> {
        self.by_key.lock().unwrap().get(public_key_bin).cloned()
    }

    /// Recherche par adresse UDP numerique.
    pub fn get_by_address(&self, addr: &SocketAddr) -> Option<Peer> {
        let key = self.by_addr.lock().unwrap().get(addr).cloned()?;
        self.get_by_key(&key)
    }

    /// Marque un pair comme fournisseur d'un service.
    pub fn discover_service(&self, public_key_bin: &[u8], service: CommunityId) {
        let mut svcs = self.services.lock().unwrap();
        let list = svcs.entry(public_key_bin.to_vec()).or_default();
        if !list.contains(&service) {
            list.push(service);
        }
    }

    /// Pairs fournissant un service donne.
    pub fn peers_for_service(&self, service: &CommunityId) -> Vec<Peer> {
        let svcs = self.services.lock().unwrap();
        let keys: Vec<Vec<u8>> = svcs
            .iter()
            .filter(|(_, v)| v.contains(service))
            .map(|(k, _)| k.clone())
            .collect();
        drop(svcs);
        keys.iter().filter_map(|k| self.get_by_key(k)).collect()
    }

    /// Nombre total de pairs verifies.
    pub fn len(&self) -> usize {
        self.by_key.lock().unwrap().len()
    }

    /// `true` si aucun pair connu.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
