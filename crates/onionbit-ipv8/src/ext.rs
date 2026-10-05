// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `OnionbitExtCommunity` — communaute d'extension OnionBit-only
//! (ADR-0015 §1–2, Phase 9b).
//!
//! Transport reserve aux messages qui n'ont **pas** d'equivalent
//! Tribler 8.x (attestations bilaterales, curation, negociation
//! d'obfuscation) : `community_id` dedie derive de
//! `sha1("OnionBit extension community")` — impossible a collisionner
//! avec les ID Tribler figes (hashs de cles maitresses). Un nœud
//! Tribler qui recoit ce prefixe le droppe silencieusement : zero casse
//! legacy, zero champ ajoute aux formats partages.
//!
//! **Decouverte lazy/opportuniste** — jamais de walk dedie : une
//! marche aleatoire sur un prefixe inconnu annoncerait « OnionBit
//! tourne ici » a tout sniffer. La communaute envoie un `hello` vers
//! des pairs **deja connus** via les communautes legacy ; un pair qui
//! ne repond pas (Tribler) n'est plus sollicite pendant
//! `hello_cooldown`. Le peer set de l'extension
//! (`peers_for_service(EXT_COMMUNITY_ID)`) *est* la population
//! OnionBit — la negociation de capacites est implicite par `msg_id`,
//! completee par le bitmap `caps` des `hello` pour les capacites de
//! transport (obfuscation opt-in, phases suivantes).
//!
//! Compromis assumé (ADR-0015 §2) : le `hello` reste un signal
//! observable — minimise, pas supprime ; meme statut que
//! `messaging_hash(pk)` dans le threat model.
//!
//! Filaire : trames `{v, ...}` versionnees, signees `ez_send` (pas de
//! `dist` — cette communaute n'a ni introduction ni puncture). Un
//! `hello` recu n'est repondu qu'une fois par cooldown → pas de
//! ping-pong entre nœuds OnionBit.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::LibNaClSecretKey;

use crate::address::UdpAddress;
use crate::endpoint::UdpEndpoint;
use crate::error::Ipv8Error;
use crate::packet::{prefix_of, Packet, WirePolicy};
use crate::peer::{Network, Peer};
use crate::serializer::{Reader, Writer};
use crate::CommunityId;

/// `community_id` de l'extension OnionBit :
/// `sha1(b"OnionBit extension community")` — constante de domaine
/// documentee, aucune cle maitresse (le hash n'est pas une
/// `LibNaClPK` vivante, il ne sert qu'a nommer le prefixe).
pub const EXT_COMMUNITY_ID: CommunityId = [
    0x92, 0x2d, 0x2a, 0xd9, 0xce, 0x00, 0xb0, 0xd8, 0x49, 0x52, 0x63, 0x8c, 0xfc, 0x22, 0x7d, 0xc1,
    0x34, 0x3c, 0x1a, 0xa4,
];

/// Politique filaire de l'extension : tous les messages sont signes
/// `ez_send` (ni `unsigned` ni `dist` — pas de marche, pas de
/// puncture). Un `msg_id` forge pour tomber sur 250/232 ne passe
/// donc pas non signe ici.
pub const WIRE_EXT: WirePolicy = WirePolicy {
    unsigned: &[],
    dist: &[],
};

/// Version de la trame d'extension (`{v, ...}`) — v1.
pub const EXT_PROTO_VERSION: u8 = 1;

/// `msg_id` de la communaute d'extension (OnionBit-only — espace
/// libre, les valeurs 1-255 n'ont pas a eviter les ID pyipv8).
pub mod msg {
    /// Annonce de capacites `{v, caps}` — emise opportunistement vers
    /// les pairs deja connus et en reponse a un `hello` recu.
    pub const HELLO: u8 = 1;
}

/// Capacites transport annoncees dans `hello.caps` — bitmap extensible.
/// v1 : aucun bit assigne (les fonctions futures `attest_*`/`ledger_*`
/// sont negociees implicitement par `msg_id` — un pair qui ne connait
/// pas un `msg_id` l'ignore). Les bits de transport (obfuscation
/// opt-in) seront assignes en Phase 9e.
pub const LOCAL_CAPS: u64 = 0;

/// Reglages de la communaute d'extension (aucune valeur en dur
/// ailleurs).
#[derive(Debug, Clone)]
pub struct ExtSettings {
    /// Cadence du sondage opportuniste : a chaque tick, jusqu'a
    /// `hello_fanout` pairs verifies pas encore sondes recoivent un
    /// `hello`.
    pub hello_interval: Duration,
    /// Pairs sollicites par tick au maximum.
    pub hello_fanout: usize,
    /// Delai minimal entre deux `hello` emis vers le meme pair
    /// (anti-tempete + "un pair qui ne repond pas n'est plus
    /// sollicite"). Vaut aussi pour la reponse a un `hello` recu.
    pub hello_cooldown: Duration,
    /// Bitmap `caps` annonce dans nos `hello`.
    pub caps: u64,
}

impl Default for ExtSettings {
    fn default() -> Self {
        Self {
            hello_interval: Duration::from_secs(60),
            hello_fanout: 5,
            hello_cooldown: Duration::from_secs(3600),
            caps: LOCAL_CAPS,
        }
    }
}

/// Trame `hello` (`{v, caps}`) : annonce de presence/capacites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Version du protocole d'extension (`EXT_PROTO_VERSION`).
    pub version: u8,
    /// Bitmap de capacites transport (voir `LOCAL_CAPS`).
    pub caps: u64,
}

impl Hello {
    /// `onionbit_ext.hello` — `msg_id` filaire.
    pub const MSG_ID: u8 = msg::HELLO;

    /// Serialise la trame (`{v: u8, caps: u64}` — 9 octets).
    pub fn pack(&self, w: &mut Writer) -> Result<(), Ipv8Error> {
        w.u8(self.version);
        w.u64(self.caps);
        Ok(())
    }

    /// Deserialise ; `TrailingBytes` toleré (un pair plus recent peut
    /// etendre la trame) mais version inconnue rejetee en amont.
    pub fn unpack(r: &mut Reader) -> Result<Self, Ipv8Error> {
        let version = r.u8()?;
        let caps = r.u64()?;
        Ok(Self { version, caps })
    }
}

/// Etat observe d'un pair ext (`caps` + instant du dernier `hello`
/// valide recu).
#[derive(Debug, Clone)]
struct ExtPeer {
    caps: u64,
    last_hello: Instant,
}

/// Instantane d'un pair ext (`peers_info` — structure brute, la
/// serialisation REST vit dans `onionbit-api`).
#[derive(Debug, Clone)]
pub struct ExtPeerInfo {
    /// `mid` hex du pair.
    pub mid: String,
    /// `caps` annonce dans son dernier `hello`.
    pub caps: u64,
    /// Secondes depuis le dernier `hello` valide recu.
    pub last_hello_secs: u64,
}

/// Instantane de la communaute (`info` — reglages effectifs +
/// pairs ext, pour `GET /api/ipv8/ext`).
#[derive(Debug, Clone)]
pub struct ExtInfo {
    /// Cadence effective du sondage `hello` (s).
    pub hello_interval_secs: u64,
    /// Pairs sondes par tick au maximum.
    pub hello_fanout: usize,
    /// Cooldown des `hello` par pair (s).
    pub hello_cooldown_secs: u64,
    /// Bitmap `caps` annonce localement.
    pub caps: u64,
    /// Population OnionBit connue (`ext_peers`).
    pub peer_count: usize,
    /// Pairs ext (mid hex, caps, dernier `hello` recu).
    pub peers: Vec<ExtPeerInfo>,
}

/// Communaute d'extension OnionBit-only : `hello` lazy + peer set
/// implicite (les pairs marques sous `EXT_COMMUNITY_ID`).
pub struct OnionbitExtCommunity {
    /// Identite locale.
    key: LibNaClSecretKey,
    /// Annuaire reseau partage avec les communities legacy — les
    /// candidats `hello` sont les pairs **verifies** deja decouverts
    /// par discovery/tunnel (lazy : aucune marche dediee).
    network: Arc<Network>,
    /// Endpoint UDP partage.
    endpoint: Arc<UdpEndpoint>,
    /// `ExtSettings`.
    settings: ExtSettings,
    /// Dernier `hello` emis par cle publique — anti-tempete sortant
    /// (cooldown) et anti-ping-pong (une reponse par pair par fenetre).
    probed: Mutex<HashMap<Vec<u8>, Instant>>,
    /// `caps`/activite observes par pair ext (indexe par cle publique).
    ext_peers: Mutex<HashMap<Vec<u8>, ExtPeer>>,
}

impl OnionbitExtCommunity {
    /// Cree la communaute et s'enregistre aupres de l'endpoint sous
    /// `EXT_COMMUNITY_ID` (politique `WIRE_EXT` : tout signe, pas de
    /// dist).
    pub async fn new(
        key: LibNaClSecretKey,
        network: Arc<Network>,
        endpoint: Arc<UdpEndpoint>,
        settings: ExtSettings,
    ) -> Arc<Self> {
        let community = Arc::new(Self {
            key,
            network,
            endpoint: endpoint.clone(),
            settings,
            probed: Mutex::new(HashMap::new()),
            ext_peers: Mutex::new(HashMap::new()),
        });
        let prefix = prefix_of(&EXT_COMMUNITY_ID);
        let c = community.clone();
        endpoint
            .add_prefix_listener(
                prefix,
                Arc::new(move |src, pkt| c.on_packet(src, pkt)),
                WIRE_EXT,
            )
            .await;
        community
    }

    /// Envoie un `hello` a `addr` sauf si un `hello` partit pour ce
    /// pair il y a moins de `hello_cooldown` (dedup sortant —
    /// sert pour le sondage periodique ET pour la reponse a un
    /// `hello` recu : un pair sondant n'est jamais repondu en
    /// boucle).
    async fn send_hello(&self, addr: &UdpAddress, public_key_bin: &[u8]) {
        {
            let mut probed = self.probed.lock().unwrap();
            if let Some(t) = probed.get(public_key_bin) {
                if t.elapsed() < self.settings.hello_cooldown {
                    return;
                }
            }
            probed.insert(public_key_bin.to_vec(), Instant::now());
        }
        let mut w = Writer::new();
        let _ = Hello {
            version: EXT_PROTO_VERSION,
            caps: self.settings.caps,
        }
        .pack(&mut w);
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::HELLO, &self.key, &w.into_bytes());
        if let Err(e) = self.endpoint.send_to(addr, &pkt).await {
            tracing::debug!(error = %e, target = ?addr, "hello ext perdu");
        }
    }

    /// `on_hello` : `hello` valide recu — le pair est marque ext
    /// (`discover_service` + `caps` observes) et recoit notre
    /// `hello` en retour une fois par cooldown (dedup sortant).
    fn on_hello(self: &Arc<Self>, peer: &Peer, hello: Hello) {
        let pk = peer.public_key_bin.clone();
        self.ext_peers.lock().unwrap().insert(
            pk.clone(),
            ExtPeer {
                caps: hello.caps,
                last_hello: Instant::now(),
            },
        );
        if let Some(addr) = peer.address.clone() {
            let c = self.clone();
            tokio::spawn(async move {
                c.send_hello(&addr, &pk).await;
            });
        }
    }

    /// Tick de sondage : jusqu'a `hello_fanout` pairs verifies non
    /// encore connus-ext et pas sondes depuis `hello_cooldown`
    /// recoivent un `hello`. Aucun pair n'est sollicite que la
    /// communaute ne connaisse deja par ailleurs (lazy).
    pub async fn hello_tick(&self) {
        let my_pk = self.key.public_key().to_bin();
        let ext_known: std::collections::HashSet<Vec<u8>> =
            self.ext_peers.lock().unwrap().keys().cloned().collect();
        // Purge des etats perimes (borne memoire + cooldown).
        {
            let cooldown = self.settings.hello_cooldown;
            self.probed
                .lock()
                .unwrap()
                .retain(|_, t| t.elapsed() < cooldown);
        }
        let candidates: Vec<Peer> = self
            .network
            .verified_peers()
            .into_iter()
            .filter(|p| p.public_key_bin != my_pk)
            .filter(|p| !ext_known.contains(&p.public_key_bin))
            .filter(|p| p.address.as_ref().is_some_and(|a| !a.is_unspecified()))
            .filter(|p| {
                self.probed
                    .lock()
                    .unwrap()
                    .get(&p.public_key_bin)
                    .is_none_or(|t| t.elapsed() >= self.settings.hello_cooldown)
            })
            .take(self.settings.hello_fanout)
            .collect();
        for p in candidates {
            if let Some(addr) = p.address {
                self.send_hello(&addr, &p.public_key_bin).await;
            }
        }
    }

    /// Boucle de sondage periodique (a spawner) — cadence
    /// `hello_interval`.
    pub async fn run(self: &Arc<Self>) {
        let mut tick = tokio::time::interval(self.settings.hello_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Premier tick immediat absorbe : rien a sonder a froid.
        tick.tick().await;
        loop {
            tick.tick().await;
            self.hello_tick().await;
        }
    }

    /// Dispatch par `msg_id`. Seuls les paquets signes comptent —
    /// `WIRE_EXT` refuse deja l'unsigned, defense en profondeur.
    fn on_packet(self: &Arc<Self>, src: SocketAddr, pkt: Packet) -> Result<(), Ipv8Error> {
        self.network.touch_by_addr(&src);
        if !pkt.signed {
            return Ok(());
        }
        // Un `msg_id` ext inconnu (version plus recente) tombe en
        // dehors du `if` : ignore silencieusement — c'est le point
        // d'extension prevu par ADR-0015.
        if pkt.msg_id == msg::HELLO {
            let mut r = Reader::new(&pkt.payload);
            let hello = Hello::unpack(&mut r)?;
            // Version inconnue : drop, et ne pas re-sonder ce
            // pair avec du v1 qu'il ignorerait — mais ne pas le
            // marquer ext non plus (on ne lui parlerait rien
            // d'autre).
            if hello.version != EXT_PROTO_VERSION {
                self.probed
                    .lock()
                    .unwrap()
                    .insert(pkt.public_key_bin.clone(), Instant::now());
                return Ok(());
            }
            let Some(peer) = Peer::new(pkt.public_key_bin.clone(), Some(UdpAddress::from(src)))
            else {
                return Ok(());
            };
            self.network.add_verified(peer.clone());
            self.network
                .discover_service(&pkt.public_key_bin, EXT_COMMUNITY_ID);
            self.on_hello(&peer, hello);
        }
        Ok(())
    }

    /// Nombre de pairs connus comme ext (population OnionBit visible).
    pub fn ext_peer_count(&self) -> usize {
        self.ext_peers.lock().unwrap().len()
    }

    /// Instantane complet (reglages + pairs) pour `GET /api/ipv8/ext`.
    pub fn info(&self) -> ExtInfo {
        ExtInfo {
            hello_interval_secs: self.settings.hello_interval.as_secs(),
            hello_fanout: self.settings.hello_fanout,
            hello_cooldown_secs: self.settings.hello_cooldown.as_secs(),
            caps: self.settings.caps,
            peer_count: self.ext_peer_count(),
            peers: self.peers_info(),
        }
    }

    /// Instantane des pairs ext pour l'API (`mid`, `caps`, activite).
    pub fn peers_info(&self) -> Vec<ExtPeerInfo> {
        self.ext_peers
            .lock()
            .unwrap()
            .iter()
            .map(|(pk, e)| ExtPeerInfo {
                mid: hex::encode(onionbit_crypto::hash::ipv8_mid(pk)),
                caps: e.caps,
                last_hello_secs: e.last_hello.elapsed().as_secs(),
            })
            .collect()
    }

    /// `OverlaySchema` pour `GET /api/ipv8/overlays` — la communaute
    /// apparait comme les autres (sans strategie : pas de marche).
    pub fn overlay_info(&self, is_isolated: bool) -> crate::overlays::OverlayInfo {
        crate::overlays::OverlayInfo {
            community_id: EXT_COMMUNITY_ID,
            my_peer_hex: hex::encode(self.key.public_key().to_bin()),
            // `WIRE_EXT` n'a aucun message `dist` : les trames
            // `ez_send` ne portent pas d'horodatage de Lamport.
            global_time: 0,
            peers: self
                .network
                .peers_for_service(&EXT_COMMUNITY_ID)
                .iter()
                .map(crate::overlays::overlay_peer)
                .collect(),
            overlay_name: "OnionbitExtCommunity",
            max_peers: crate::overlays::DEFAULT_MAX_PEERS,
            is_isolated,
            my_estimated_wan: UdpAddress::unspecified(),
            my_estimated_lan: UdpAddress::unspecified(),
            // Lazy par design (ADR-0015 §2) : aucune strategie de
            // marche — la liste reste vide, c'est voulu.
            strategies: Vec::new(),
            decode: crate::overlays::ext_msg_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::TapDir;

    /// L'ID dedie ne collisionne avec aucun `community_id` Tribler
    /// deploye — le prefixe doit etre droppe silencieusement par les
    /// pairs legacy (ADR-0015 §1).
    #[test]
    fn community_id_sans_collision_tribler() {
        for id in [
            crate::discovery::DISCOVERY_COMMUNITY_ID,
            crate::content_discovery::CONTENT_DISCOVERY_COMMUNITY_ID,
            // Tunnel pyipv8 + communaute installee Tribler.
            [
                0x81, 0xde, 0xd0, 0x73, 0x32, 0xbd, 0xc7, 0x75, 0xaa, 0x5a, 0x46, 0xf9, 0x6d, 0xe9,
                0xf8, 0xf3, 0x90, 0xbb, 0xc9, 0xf3,
            ],
            [
                0xa3, 0x59, 0x1a, 0x6b, 0xd8, 0x9b, 0xba, 0xca, 0x09, 0x74, 0x06, 0x2a, 0x12, 0x87,
                0xaf, 0xcf, 0xbc, 0x6f, 0xd6, 0xbc,
            ],
        ] {
            assert_ne!(EXT_COMMUNITY_ID, id);
        }
    }

    #[test]
    fn hello_roundtrip_et_trailing_toleré() {
        let h = Hello {
            version: EXT_PROTO_VERSION,
            caps: 0xdead_beef,
        };
        let mut w = Writer::new();
        h.pack(&mut w).unwrap();
        let mut bytes = w.into_bytes();
        assert_eq!(bytes.len(), 9);
        bytes.extend_from_slice(&[0xaa, 0xbb]); // extension future
        let mut r = Reader::new(&bytes);
        assert_eq!(Hello::unpack(&mut r).unwrap(), h);
        // Trame tronquee : erreur, pas de panic.
        let mut r = Reader::new(&bytes[..4]);
        assert!(Hello::unpack(&mut r).is_err());
    }

    /// Noeud ext loopback : endpoint UDP lie + boucle de reception
    /// spawnee + communaute enregistree.
    async fn node(
        caps: u64,
    ) -> (
        Arc<OnionbitExtCommunity>,
        Arc<UdpEndpoint>,
        UdpAddress,
        LibNaClSecretKey,
    ) {
        let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let SocketAddr::V4(sa) = ep.local_addr().unwrap() else {
            panic!("bind v4");
        };
        let addr = UdpAddress::Ipv4(sa);
        let network = Arc::new(Network::default());
        let key = LibNaClSecretKey::generate();
        let c = OnionbitExtCommunity::new(
            key.clone(),
            network,
            ep.clone(),
            ExtSettings {
                caps,
                ..ExtSettings::default()
            },
        )
        .await;
        let e = ep.clone();
        tokio::spawn(async move {
            let _ = e.run().await;
        });
        (c, ep, addr, key)
    }

    /// Attend `pred` jusqu'a ~2 s (sondage 10 ms) — les datagrammes
    /// loopback arrivent en quelques ms mais le runtime doit tourner.
    async fn wait_until(mut pred: impl FnMut() -> bool) {
        for _ in 0..200 {
            if pred() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition non atteinte en 2 s");
    }

    /// `hello` A→B : B marque A pair ext avec les `caps` annonces,
    /// repond son `hello` une fois, A marque B en retour.
    #[tokio::test]
    async fn hello_bilateral_marque_les_deux_pairs() {
        let (a, ep_a, addr_a, _key_a) = node(0x0).await;
        let a_sa = ep_a.local_addr().unwrap();
        let (b, ep_b, addr_b, key_b) = node(0x3).await;
        let mut tap = ep_b.set_tap().await;
        let _ = addr_a;

        a.send_hello(&addr_b, &key_b.public_key().to_bin()).await;
        let b2 = b.clone();
        let pk_a = a.key.public_key().to_bin();
        wait_until(move || {
            b2.ext_peers
                .lock()
                .unwrap()
                .get(&pk_a)
                .is_some_and(|p| p.caps == 0x0)
        })
        .await;
        // B a repondu : A connait B comme pair ext avec caps=0x3.
        wait_until(move || {
            a.ext_peers
                .lock()
                .unwrap()
                .get(&key_b.public_key().to_bin())
                .is_some_and(|p| p.caps == 0x3)
        })
        .await;
        // La reponse de B : un seul `hello` emis vers A (anti
        // ping-pong par cooldown sortant).
        let mut hellos_to_a = 0usize;
        while let Ok((dir, dst, data)) = tap.try_recv() {
            if matches!(dir, TapDir::Tx) && data.get(crate::packet::PREFIX_LEN) == Some(&msg::HELLO)
            {
                assert_eq!(dst, a_sa);
                hellos_to_a += 1;
            }
        }
        assert_eq!(hellos_to_a, 1);
    }

    /// `hello` avec version inconnue : droppe — le pair n'est ni
    /// marque ext ni re-sonde en boucle (entree `probed` posee).
    #[tokio::test]
    async fn hello_version_inconnue_pas_de_marquage() {
        let (a, _ep_a, addr_a, _ka) = node(0).await;
        let (_b, ep_b, _ab, key_b) = node(0).await;
        // B forge un `hello` v9 signe de sa cle et l'envoie a A.
        let mut w = Writer::new();
        Hello {
            version: 9,
            caps: 0,
        }
        .pack(&mut w)
        .unwrap();
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::HELLO, &key_b, &w.into_bytes());
        ep_b.send_to(&addr_a, &pkt).await.unwrap();
        let pk_b = key_b.public_key().to_bin();
        let a2 = a.clone();
        let pk_b2 = pk_b.clone();
        wait_until(move || a2.probed.lock().unwrap().contains_key(&pk_b2)).await;
        // Pas de marquage ext : on ne sait parler que v1, que ce pair
        // ignorerait — et il n'est plus re-sonde avant cooldown.
        assert!(a.ext_peers.lock().unwrap().get(&pk_b).is_none());
        a.hello_tick().await;
        assert_eq!(a.ext_peer_count(), 0);
    }

    /// Paquet mal forme (signature invalide / tronque) sur le
    /// prefixe ext : droppe sans panic ni marquage.
    #[tokio::test]
    async fn paquet_corrompu_ignore() {
        let (a, _ep_a, addr_a, _ka) = node(0).await;
        let (_b, ep_b, _ab, _kb) = node(0).await;
        let prefix = prefix_of(&EXT_COMMUNITY_ID);
        let junks: Vec<Vec<u8>> = vec![
            Vec::new(),                                   // datagramme vide
            prefix.to_vec(),                              // prefixe seul, pas de msg_id
            [prefix.to_vec(), vec![msg::HELLO]].concat(), // pas d'auth
            // Trame complete mais signature fausse.
            {
                let mut d = Packet::sign_no_dist(
                    &EXT_COMMUNITY_ID,
                    msg::HELLO,
                    &LibNaClSecretKey::generate(),
                    &[1, 0, 0, 0, 0, 0, 0, 0, 0],
                );
                let n = d.len();
                d[n - 1] ^= 0xff;
                d
            },
            // `msg_id` inconnu signe correctement : point d'extension
            // futur, ignore silencieusement.
            Packet::sign_no_dist(
                &EXT_COMMUNITY_ID,
                0x7f,
                &LibNaClSecretKey::generate(),
                &[1, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
        ];
        for d in junks {
            ep_b.send_to(&addr_a, &d).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(a.ext_peer_count(), 0);
    }

    /// `hello_tick` ne sonde que les pairs verifies, pas encore
    /// connus-ext, hors cooldown — borne par `hello_fanout`.
    #[tokio::test]
    async fn hello_tick_lazy_pairs_verifies_seulement() {
        let (a, _ep_a, _aa, _ka) = node(0).await;
        let (b, _ep_b, addr_b, key_b) = node(0).await;
        // Sans pair verifie : rien n'est emis.
        a.hello_tick().await;
        assert!(a.probed.lock().unwrap().is_empty());
        // B verifie dans l'annuaire de A (comme le ferait discovery).
        a.network
            .add_verified(Peer::new(key_b.public_key().to_bin(), Some(addr_b)).unwrap());
        a.hello_tick().await;
        assert!(a
            .probed
            .lock()
            .unwrap()
            .contains_key(&key_b.public_key().to_bin()));
        // B recoit et marque A.
        let b2 = b.clone();
        let pk_a = a.key.public_key().to_bin();
        wait_until(move || b2.ext_peers.lock().unwrap().contains_key(&pk_a)).await;
    }

    /// Un pair muet (Tribler : jamais de reponse) n'est plus sollicite
    /// avant la fin du cooldown.
    #[tokio::test]
    async fn pair_muet_plus_solicite_pendant_cooldown() {
        let (a, _ep_a, _aa, _ka) = node(0).await;
        // Pair "Tribler" : verifie avec adresse mais aucun listener ext
        // — ses hellos partent dans le vide.
        let mute_key = LibNaClSecretKey::generate();
        let mute_addr = UdpAddress::Ipv4("127.0.0.1:9".parse().unwrap());
        a.network
            .add_verified(Peer::new(mute_key.public_key().to_bin(), Some(mute_addr)).unwrap());
        a.hello_tick().await;
        assert_eq!(a.probed.lock().unwrap().len(), 1);
        // Tick suivant : deja dans `probed` → aucun nouvel envoi
        // (l'etat ne change pas — la cle reste marquee, jamais
        // promue ext).
        a.hello_tick().await;
        assert_eq!(a.probed.lock().unwrap().len(), 1);
        assert_eq!(a.ext_peer_count(), 0);
    }
}
