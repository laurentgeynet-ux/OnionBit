// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pairs et annuaire reseau (equivalent de `peer.py` et
//! `peerdiscovery/network.py`).
//!
//! Un `Peer` = cle publique binaire (`LibNaClPK:…`) + adresses connues.
//! Le `Network` maintient :
//! - les pairs **verifies** (signature valide vue) indexes par cle et
//!   adresse ;
//! - les services (community_id) declares par chaque pair ;
//! - les adresses "walkable" connues mais non verifiees
//!   (`_all_addresses` / `WalkableAddress` Python : introduites par un
//!   pair, avec service optionnel et flag `new_style`) ;
//! - les caches `reverse_intro_lookup`/`reverse_ip_lookup` bornes ;
//! - `blacklist` (adresses) et `blacklist_mids` (MID) Python.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClPublicKey;

use crate::address::UdpAddress;
use crate::CommunityId;

/// `reverse_intro_cache_size` Python (borne FIFO du cache
/// `reverse_intro_lookup`).
const REVERSE_INTRO_CACHE_SIZE: usize = 500;

/// Pair IPv8 (equivalent de `peer.Peer`).
#[derive(Debug)]
pub struct Peer {
    /// Cle publique binaire (`key_to_bin()` Python).
    pub public_key_bin: Vec<u8>,
    /// Adresse principale connue (WAN estime).
    pub address: Option<UdpAddress>,
    /// MID = SHA-1 de la cle publique.
    pub mid: [u8; 20],
    /// Le pair supporte le format d'introduction "new style"
    /// (`new_style_intro` Python).
    pub new_style_intro: bool,
    /// Horodatage de Lamport (dernier `global_time` vu).
    lamport: Mutex<u64>,
    /// Derniere reponse reelle (pour timeouts, `last_response` Python).
    pub last_response: Mutex<Instant>,
}

impl Clone for Peer {
    fn clone(&self) -> Self {
        Self {
            public_key_bin: self.public_key_bin.clone(),
            address: self.address.clone(),
            mid: self.mid,
            new_style_intro: self.new_style_intro,
            lamport: Mutex::new(*self.lamport.lock().unwrap()),
            last_response: Mutex::new(*self.last_response.lock().unwrap()),
        }
    }
}

impl Peer {
    /// Cree un pair depuis sa cle publique binaire.
    pub fn new(public_key_bin: Vec<u8>, address: Option<UdpAddress>) -> Option<Self> {
        let pk = LibNaClPublicKey::from_bin(&public_key_bin).ok()?;
        Some(Self {
            mid: pk.mid(),
            public_key_bin,
            address,
            new_style_intro: false,
            lamport: Mutex::new(0),
            last_response: Mutex::new(Instant::now()),
        })
    }

    /// `update_clock` Python : Lamport = max(lamport, timestamp) et
    /// rafraichit `last_response`.
    pub fn update_clock(&self, timestamp: u64) {
        let mut l = self.lamport.lock().unwrap();
        *l = (*l).max(timestamp);
        *self.last_response.lock().unwrap() = Instant::now();
    }

    /// `get_lamport_timestamp`.
    pub fn lamport(&self) -> u64 {
        *self.lamport.lock().unwrap()
    }

    /// Rafraichit `last_response` sans toucher l'horloge — equivalent
    /// de `probable_peer.last_response = time()` dans `on_packet`
    /// pyipv8 (tout paquet recu d'un pair verifie, signe ou non).
    pub fn touch(&self) {
        *self.last_response.lock().unwrap() = Instant::now();
    }

    /// Delai ecoule depuis `last_response` (`time() - peer.last_response`
    /// Python — sert a `RandomChurn.should_drop`/`is_inactive`).
    pub fn last_response_elapsed(&self) -> Duration {
        self.last_response.lock().unwrap().elapsed()
    }
}

impl PartialEq for Peer {
    /// `__eq__` Python : meme cle publique.
    fn eq(&self, other: &Self) -> bool {
        self.public_key_bin == other.public_key_bin
    }
}
impl Eq for Peer {}

/// `WalkableAddress` : adresse connue introduite par un pair.
#[derive(Debug, Clone)]
pub struct WalkableAddress {
    /// Cle publique du pair introducteur.
    pub introduced_by: Vec<u8>,
    /// Service par lequel l'adresse a ete decouverte.
    pub service: Option<CommunityId>,
    /// `new_style` : l'adresse utilise le nouveau format d'introduction.
    pub new_style: bool,
}

/// Annuaire reseau (equivalent de `peerdiscovery.network.Network`).
#[derive(Default)]
pub struct Network {
    /// Pairs verifies indexe par cle publique
    /// (`verified_by_public_key_bin`).
    by_key: Mutex<HashMap<Vec<u8>, Peer>>,
    /// Index adresse -> cle publique (`reverse_ip_lookup` simplifie).
    by_addr: Mutex<HashMap<SocketAddr, Vec<u8>>>,
    /// Services (community_id) declares par chaque pair
    /// (`services_per_peer`).
    services: Mutex<HashMap<Vec<u8>, Vec<CommunityId>>>,
    /// `_all_addresses` : toutes les adresses connues (marche
    /// aleatoire), mappees vers leur introducteur.
    all_addresses: Mutex<HashMap<UdpAddress, WalkableAddress>>,
    /// `reverse_intro_lookup` borne : pair -> adresses introduites.
    reverse_intro: Mutex<Vec<(Vec<u8>, Vec<UdpAddress>)>>,
    /// `blacklist` Python : adresses interdites.
    blacklist: Mutex<Vec<UdpAddress>>,
    /// `blacklist_mids` Python : mids interdits.
    blacklist_mids: Mutex<Vec<[u8; 20]>>,
}

impl Network {
    /// `add_verified_peer` : enregistre un pair verifie. Fidele a
    /// pyipv8 :
    /// - `mid` en `blacklist_mids` -> ignore ;
    /// - deja connu par cle -> mise a jour d'adresse seulement ;
    /// - adresse deja dans `_all_addresses` -> verifie ;
    /// - adresse non blacklistee -> inscrite dans `_all_addresses`
    ///   (`WalkableAddress(b"", None, False)` : introduite par
    ///   personne, pas de service, pas new-style) puis verifie ;
    /// - sinon (adresse blacklistee inconnue) -> PAS verifie.
    pub fn add_verified(&self, peer: Peer) {
        if self.blacklist_mids.lock().unwrap().contains(&peer.mid) {
            return;
        }
        if let Some(known) = self.by_key.lock().unwrap().get_mut(&peer.public_key_bin) {
            // `known.addresses.update(...)` + objet partage Python :
            // le pair stocke absorbe l'adresse et le flag
            // `new_style_intro` du nouvel exemplaire.
            if let Some(a) = &peer.address {
                known.address = Some(a.clone());
            }
            known.new_style_intro |= peer.new_style_intro;
            return;
        }
        if let Some(addr) = &peer.address {
            let known_walkable = self.all_addresses.lock().unwrap().contains_key(addr);
            if !known_walkable {
                if self.blacklist.lock().unwrap().contains(addr) {
                    // Adresse blacklistee et inconnue : pas de verify.
                    return;
                }
                self.all_addresses.lock().unwrap().insert(
                    addr.clone(),
                    WalkableAddress {
                        introduced_by: Vec::new(),
                        service: None,
                        new_style: false,
                    },
                );
            }
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

    /// `get_verified_by_public_key_bin`.
    pub fn get_by_key(&self, public_key_bin: &[u8]) -> Option<Peer> {
        self.by_key.lock().unwrap().get(public_key_bin).cloned()
    }

    /// `get_verified_by_address` (adresse UDP numerique).
    pub fn get_by_address(&self, addr: &SocketAddr) -> Option<Peer> {
        let key = self.by_addr.lock().unwrap().get(addr).cloned()?;
        self.get_by_key(&key)
    }

    /// Variante `UdpAddress` (utile quand l'adresse n'est pas un
    /// `SocketAddr` — domaine).
    pub fn get_verified_by_address(&self, addr: &UdpAddress) -> Option<Peer> {
        if let Some(sa) = addr.to_socket_addr() {
            return self.get_by_address(&sa);
        }
        // Domaine : recherche lineaire (rare).
        self.by_key
            .lock()
            .unwrap()
            .values()
            .find(|p| p.address.as_ref() == Some(addr))
            .cloned()
    }

    /// Rafraichit `last_response` du pair verifie a cette adresse.
    /// Equivalent du touch de `Community.on_packet` pyipv8 — tout
    /// datagramme recu (paquet signe, non signe ou cellule tunnel)
    /// prouve que le pair est vivant.
    pub fn touch_by_addr(&self, addr: &SocketAddr) {
        if let Some(peer) = self.get_by_address(addr) {
            peer.touch();
        }
    }

    /// `discover_services`.
    pub fn discover_service(&self, public_key_bin: &[u8], service: CommunityId) {
        self.discover_services(public_key_bin, &[service]);
    }

    /// `discover_services` (ensemble).
    pub fn discover_services(&self, public_key_bin: &[u8], services: &[CommunityId]) {
        let mut svcs = self.services.lock().unwrap();
        let list = svcs.entry(public_key_bin.to_vec()).or_default();
        for s in services {
            if !list.contains(s) {
                list.push(*s);
            }
        }
    }

    /// `get_services_for_peer` : services (`community_id`) annonces
    /// par un pair (cle = `public_key_bin`).
    pub fn services_for_peer(&self, public_key_bin: &[u8]) -> Vec<CommunityId> {
        self.services
            .lock()
            .unwrap()
            .get(public_key_bin)
            .cloned()
            .unwrap_or_default()
    }

    /// `get_peers_for_service`.
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

    /// Liste de tous les pairs vérifiés connus.
    pub fn all_verified_peers(&self) -> Vec<Peer> {
        self.by_key.lock().unwrap().values().cloned().collect()
    }

    /// `discover_address` : un pair nous a introduit une adresse.
    /// Si l'adresse est blacklistee : le pair est quand meme verifie
    /// (comportement Python).
    pub fn discover_address(
        &self,
        peer: &Peer,
        address: UdpAddress,
        service: Option<CommunityId>,
        new_style: bool,
    ) {
        if self.blacklist.lock().unwrap().contains(&address) {
            self.add_verified(peer.clone());
            return;
        }
        let mut all = self.all_addresses.lock().unwrap();
        let dominated = match all.get(&address) {
            // Garde l'entree si l'introducteur precedent est encore
            // un pair verifie (sinon on reecrit).
            Some(w) => !self.by_key.lock().unwrap().contains_key(&w.introduced_by),
            None => true,
        };
        if !all.contains_key(&address) || dominated {
            all.insert(
                address.clone(),
                WalkableAddress {
                    introduced_by: peer.public_key_bin.clone(),
                    service,
                    new_style,
                },
            );
            // `reverse_intro_lookup` borne (FIFO approxime).
            let mut rev = self.reverse_intro.lock().unwrap();
            if let Some((_, addrs)) = rev.iter_mut().find(|(k, _)| *k == peer.public_key_bin) {
                addrs.push(address);
            } else {
                rev.push((peer.public_key_bin.clone(), vec![address]));
                if rev.len() > REVERSE_INTRO_CACHE_SIZE {
                    rev.remove(0);
                }
            }
        }
        drop(all);
        self.add_verified(peer.clone());
    }

    /// `is_new_style`.
    pub fn is_new_style(&self, address: &UdpAddress) -> bool {
        self.all_addresses
            .lock()
            .unwrap()
            .get(address)
            .map(|w| w.new_style)
            .unwrap_or(false)
    }

    /// `get_walkable_addresses` : adresses connues non encore verifiees.
    /// `service` filtre sur la community d'introduction ;
    /// `old_style` exclut les adresses `new_style`.
    pub fn get_walkable_addresses(
        &self,
        service: Option<&CommunityId>,
        old_style: bool,
    ) -> Vec<UdpAddress> {
        let all = self.all_addresses.lock().unwrap();
        // `get_peers_for_service(service_id)` Python : seules les
        // adresses des pairs DU SERVICE demande sont exclues des
        // walkables — pas tous les pairs verifies. Exclure tous les
        // verifies rendait les adresses decouvertes par la discovery
        // (puis verifiees la) invisibles pour l'overlay tunnel, qui ne
        // marchait alors que sur son pair de bootstrap.
        let verified_addrs: Vec<UdpAddress> = match service {
            Some(sid) => self
                .peers_for_service(sid)
                .iter()
                .filter_map(|p| p.address.clone())
                .collect(),
            None => self
                .by_key
                .lock()
                .unwrap()
                .values()
                .filter_map(|p| p.address.clone())
                .collect(),
        };
        let svcs = self.services.lock().unwrap();
        all.iter()
            .filter(|(addr, w)| {
                if verified_addrs.contains(addr) {
                    return false;
                }
                if let Some(sid) = service {
                    if old_style && w.new_style {
                        return false;
                    }
                    let mut peer_svcs = svcs.get(&w.introduced_by).cloned().unwrap_or_default();
                    if let Some(s) = w.service {
                        peer_svcs.push(s);
                    }
                    return peer_svcs.contains(sid);
                }
                true
            })
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// `get_introductions_from` : adresses introduites par ce pair.
    pub fn get_introductions_from(&self, peer: &Peer) -> Vec<UdpAddress> {
        let rev = self.reverse_intro.lock().unwrap();
        if let Some((_, addrs)) = rev.iter().find(|(k, _)| *k == peer.public_key_bin) {
            return addrs.clone();
        }
        drop(rev);
        self.all_addresses
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, w)| w.introduced_by == peer.public_key_bin)
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// `remove_by_address` : retire pairs et adresses utilisant cette
    /// adresse.
    pub fn remove_by_address(&self, address: &UdpAddress) {
        let key = self
            .by_key
            .lock()
            .unwrap()
            .iter()
            .find(|(_, p)| p.address.as_ref() == Some(address))
            .map(|(k, _)| k.clone());
        if let Some(k) = key {
            self.remove_peer_key(&k);
        }
        self.all_addresses.lock().unwrap().remove(address);
        if let Some(sa) = address.to_socket_addr() {
            self.by_addr.lock().unwrap().remove(&sa);
        }
    }

    /// `remove_peer` (par cle publique).
    pub fn remove_peer_key(&self, public_key_bin: &[u8]) {
        if let Some(p) = self.by_key.lock().unwrap().remove(public_key_bin) {
            if let Some(sa) = p.address.and_then(|a| a.to_socket_addr()) {
                self.by_addr.lock().unwrap().remove(&sa);
            }
        }
        self.services.lock().unwrap().remove(public_key_bin);
    }

    /// `true` si la cle est un pair connu.
    pub fn contains_key(&self, public_key_bin: &[u8]) -> bool {
        self.by_key.lock().unwrap().contains_key(public_key_bin)
    }

    /// Tous les pairs verifies.
    pub fn verified_peers(&self) -> Vec<Peer> {
        self.by_key.lock().unwrap().values().cloned().collect()
    }

    /// Ajoute une adresse a la `blacklist`.
    pub fn add_blacklist(&self, addr: UdpAddress) {
        self.blacklist.lock().unwrap().push(addr);
    }

    /// Ajoute un mid a `blacklist_mids`.
    pub fn add_blacklist_mid(&self, mid: [u8; 20]) {
        self.blacklist_mids.lock().unwrap().push(mid);
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
