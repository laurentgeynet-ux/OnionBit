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

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
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

pub mod attest;

pub use attest::{kind as attest_kind, verdict as attest_verdict, Attestation};

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

/// Borne du payload `ATTEST` avant parse : v1 fait ~119 octets utiles
/// (champs varlen bornes a 255) ; la marge couvre des champs futurs
/// sans ouvrir un buffer arbitraire au decodeur.
pub const ATTEST_FRAME_MAX: usize = 1024;

/// `msg_id` de la communaute d'extension (OnionBit-only — espace
/// libre, les valeurs 1-255 n'ont pas a eviter les ID pyipv8).
pub mod msg {
    /// Annonce de capacites `{v, caps}` — emise opportunistement vers
    /// les pairs deja connus et en reponse a un `hello` recu.
    pub const HELLO: u8 = 1;
    /// Attestation de curation (`Attestation` packee, ADR-0015 §6) —
    /// poussee a la publication et re-emise aux autres pairs ext
    /// quand elle est nouvelle (gossip borne par deduplication).
    pub const ATTEST: u8 = 2;
}

/// Capacites transport annoncees dans `hello.caps` — bitmap extensible.
/// v1 : aucun bit assigne (les fonctions futures `attest_*`/`ledger_*`
/// sont negociees implicitement par `msg_id` — un pair qui ne connait
/// pas un `msg_id` l'ignore). Les bits de transport (obfuscation
/// opt-in) seront assignes en Phase 9e.
pub const LOCAL_CAPS: u64 = 0;

/// Persistance des attestations verifiees (trait injecte — pattern
/// `GuardStore`/`PeerStatsStore` : `onionbit-ipv8` definit le contrat
/// sans dependre de `onionbit-db`, `onionbit-core` fournit
/// l'adaptateur SQLite).
pub trait AttestationStore: Send + Sync {
    /// Persiste une attestation deja verifiee. Retourne `true` si elle
    /// est nouvelle ou strictement plus recente (`ts`) que la version
    /// stockee pour la meme cle `(curator, kind, subject)` — la
    /// dedup gouverne aussi la re-emission : `false` = deja connue,
    /// pas de re-gossip.
    fn put(&self, att: &Attestation) -> bool;
    /// Attestation stockee pour la cle exacte
    /// `(curator, kind, subject)` — lookup de dedup/consultation
    /// **avant** verification Ed25519 : un rejeu ou une version plus
    /// ancienne est absorbe sans payer la crypto (Ed25519 est
    /// deterministe — champs identiques = octets deja verifies).
    /// Implementation par defaut via `by_subject` ; les stores
    /// indexees la surchargent par un acces cle primaire.
    fn get(&self, curator: &[u8], kind: u8, subject: &[u8]) -> Option<Attestation> {
        self.by_subject(kind, subject)
            .into_iter()
            .find(|a| a.curator == curator)
    }
    /// Attestations stockees pour un sujet (`kind`, `subject`) —
    /// une par curateur au plus (latest-wins).
    fn by_subject(&self, kind: u8, subject: &[u8]) -> Vec<Attestation>;
    /// Les `limit` attestations les plus recentes (`ts` decroissant).
    fn latest(&self, limit: usize) -> Vec<Attestation>;
}

/// Cle de deduplication des attestations : `(curateur, kind, sujet)`.
type AttestKey = (Vec<u8>, u8, Vec<u8>);

/// `AttestationStore` en memoire — defaut sans persistance et pour
/// les tests ; borne par `max` (les plus vieilles cles sont evictees).
#[derive(Default)]
pub struct InMemoryAttestationStore {
    inner: Mutex<HashMap<AttestKey, Attestation>>,
    /// Borne de la table (`0` = illimitee — tests uniquement).
    max: usize,
}

impl InMemoryAttestationStore {
    /// Cree un store borne (`max` cles `(curateur,kind,sujet)`).
    pub fn bounded(max: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max,
        }
    }
}

impl AttestationStore for InMemoryAttestationStore {
    fn put(&self, att: &Attestation) -> bool {
        let mut m = self.inner.lock().unwrap();
        let k = (att.curator.clone(), att.kind, att.subject.clone());
        // `old.ts >= att.ts` absorbe : un `ts` egal n'ecrase jamais
        // (meme ts + verdict different = equivoque — la couche
        // protocole la rejette comme conflit avant `put` ; le store
        // reste le dernier filet).
        match m.get(&k) {
            Some(old) if old.ts >= att.ts => return false,
            _ => {}
        }
        if self.max > 0 && m.len() >= self.max && !m.contains_key(&k) {
            // Eviction de la plus vieille entree — approximation LRU
            // suffisante : la table est bornee, jamais de croissance
            // libre face a un flot d'attestations.
            if let Some(oldest) = m.iter().min_by_key(|(_, a)| a.ts).map(|(k, _)| k.clone()) {
                m.remove(&oldest);
            }
        }
        m.insert(k, att.clone());
        true
    }

    fn get(&self, curator: &[u8], kind: u8, subject: &[u8]) -> Option<Attestation> {
        self.inner
            .lock()
            .unwrap()
            .get(&(curator.to_vec(), kind, subject.to_vec()))
            .cloned()
    }

    fn by_subject(&self, kind: u8, subject: &[u8]) -> Vec<Attestation> {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.kind == kind && a.subject == subject)
            .cloned()
            .collect()
    }

    fn latest(&self, limit: usize) -> Vec<Attestation> {
        let mut v: Vec<Attestation> = self.inner.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.ts));
        v.truncate(limit);
        v
    }
}

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
    /// Curateurs suivis (cles publiques LibNaCl binaires) : seules
    /// leurs attestations sont stockees et re-emises — borne Sybil :
    /// un flot d'attestations signees de cles inconnues est verifie
    /// puis droppe sans stockage (ADR-0015 §6 — confiance locale).
    pub curators: HashSet<Vec<u8>>,
    /// Derive d'horloge toleree sur `ts` des attestations recues :
    /// une attestation datee dans le futur au-dela de cette marge est
    /// rejetee (sinon elle dominerait le « latest wins » pour
    /// toujours). Le passe n'est pas borne — dedup par `ts` max.
    pub attest_max_future_skew: Duration,
    /// Borne de la liste `latest` exposee par l'API.
    pub attest_list_max: usize,
    /// Fenetre du budget `ATTEST` par emetteur : borne le cout CPU
    /// du chemin de reception (parse + dedup) face a un pair qui
    /// rafale — le compteur se remet a zero a chaque fenetre.
    pub attest_rate_window: Duration,
    /// Messages `ATTEST` acceptes par emetteur (cle de transport)
    /// et par fenetre `attest_rate_window` — au-dela : drop
    /// silencieux. Assez large pour absorber un backfill legitime.
    pub attest_rate_max: u32,
    /// Borne memoire de la table de budget : pleine de fenetres
    /// actives, un emetteur inconnu est droppe sans insertion (un
    /// flot de cles Sybil fraiches ne fait pas grossir la table).
    /// `0` = illimitee (deconseille hors tests).
    pub attest_rate_table_max: usize,
}

impl Default for ExtSettings {
    fn default() -> Self {
        Self {
            hello_interval: Duration::from_secs(60),
            hello_fanout: 5,
            hello_cooldown: Duration::from_secs(3600),
            caps: LOCAL_CAPS,
            curators: HashSet::new(),
            attest_max_future_skew: Duration::from_secs(600),
            attest_list_max: 256,
            attest_rate_window: Duration::from_secs(60),
            attest_rate_max: 256,
            attest_rate_table_max: 4096,
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

/// Score de confiance local d'un sujet (`trust_info`) — seuls les
/// curateurs suivis (+ soi) contribuent ; +1 endorse, -1 flag.
#[derive(Debug, Clone)]
pub struct TrustInfo {
    /// Somme des verdicts des curateurs suivis sur ce sujet.
    pub score: i64,
    /// `mid` hex des curateurs suivis ayant endorse.
    pub endorsements: Vec<String>,
    /// `mid` hex des curateurs suivis ayant flague.
    pub flags: Vec<String>,
    /// Attestations stockees sur le sujet (toutes curatrices —
    /// metrique de visibilite, ne compte pas dans `score`).
    pub attestation_count: usize,
}

/// Instantane de la communaute (`info` — reglages effectifs +
/// pairs ext + compteurs ATTEST, pour `GET /api/ipv8/ext`).
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
    /// Messages `ATTEST` recus (toutes causes confondues, avant
    /// tout filtre) — oracle des bancs : le budget et les drops
    /// doivent rester visibles.
    pub attest_rx: u64,
    /// `ATTEST` dropees (budget depasse, trame trop grande,
    /// curateur non suivi, rejeu/stale, `ts` futur, signature
    /// invalide, conflit a `ts` egal).
    pub attest_dropped: u64,
    /// `ATTEST` stockees (nouvelles ou strictement plus recentes).
    pub attest_stored: u64,
    /// Datagrammes `ATTEST` emis (publication + re-emission
    /// gossip) — doit retomber a zero quand le gossip s'eteint.
    pub attest_tx: u64,
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
    /// Persistance des attestations (injectee — `None` = store
    /// memoire non borne installe a la creation ; `set_*` avant tout
    /// trafic).
    attest_store: Mutex<Arc<dyn AttestationStore>>,
    /// Budget `ATTEST` par emetteur (cle de transport) :
    /// `(debut de fenetre, compte)` — borne le cout du chemin de
    /// reception face aux rafales.
    attest_rate: Mutex<HashMap<Vec<u8>, (Instant, u32)>>,
    /// Compteurs du chemin `ATTEST` (`ExtInfo` — oracles de banc).
    attest_rx: AtomicU64,
    /// Compteurs du chemin `ATTEST` (`ExtInfo` — oracles de banc).
    attest_dropped: AtomicU64,
    /// Compteurs du chemin `ATTEST` (`ExtInfo` — oracles de banc).
    attest_stored: AtomicU64,
    /// Compteurs du chemin `ATTEST` (`ExtInfo` — oracles de banc).
    attest_tx: AtomicU64,
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
            attest_store: Mutex::new(Arc::new(InMemoryAttestationStore::default())),
            attest_rate: Mutex::new(HashMap::new()),
            attest_rx: AtomicU64::new(0),
            attest_dropped: AtomicU64::new(0),
            attest_stored: AtomicU64::new(0),
            attest_tx: AtomicU64::new(0),
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

    /// Injecte le store d'attestations persistant (pattern
    /// `set_peer_stats_store` — a appeler avant tout trafic).
    pub fn set_attestation_store(&self, store: Arc<dyn AttestationStore>) {
        *self.attest_store.lock().unwrap() = store;
    }

    /// Curateur pris en compte : `ext/curators` ou notre propre cle
    /// (on est toujours son propre curateur).
    fn is_followed(&self, curator: &[u8]) -> bool {
        curator == self.key.public_key().to_bin() || self.settings.curators.contains(curator)
    }

    /// Envoie un `ATTEST` a `addr` — pas de cooldown : le volume est
    /// borne par la dedup (chaque attestation n'est re-emise qu'une
    /// fois par noeud) et par le filtre « curateur suivi ».
    async fn send_attest_to(&self, addr: &UdpAddress, payload: &[u8]) {
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &self.key, payload);
        self.attest_tx.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = self.endpoint.send_to(addr, &pkt).await {
            tracing::debug!(error = %e, target = ?addr, "attest ext perdu");
        }
    }

    /// Pousse `att` a tous les pairs ext connus sauf `except` (cle du
    /// relayeur quand on re-emet — il l'a deja).
    async fn gossip_attest(self: &Arc<Self>, att: &Attestation, except: Option<&[u8]>) {
        let payload = att.pack();
        let my_pk = self.key.public_key().to_bin();
        for p in self.network.peers_for_service(&EXT_COMMUNITY_ID) {
            if p.public_key_bin == my_pk || except == Some(p.public_key_bin.as_slice()) {
                continue;
            }
            if let Some(addr) = p.address.clone() {
                self.send_attest_to(&addr, &payload).await;
            }
        }
    }

    /// Publie une attestation signee par notre cle
    /// (`POST /api/ipv8/ext/attest`) : stockee puis poussee aux
    /// pairs ext. `ts` = horloge locale (createur = seule source
    /// legitime de son `ts`).
    pub async fn publish_attestation(
        self: &Arc<Self>,
        kind: u8,
        subject: &[u8],
        verdict: u8,
    ) -> Result<Attestation, Ipv8Error> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let att = Attestation::sign(&self.key, kind, subject, verdict, ts)?;
        if self.attest_store.lock().unwrap().put(&att) {
            self.attest_stored.fetch_add(1, Ordering::Relaxed);
        }
        self.gossip_attest(&att, None).await;
        Ok(att)
    }

    /// `on_attest` : attestation recue — pipeline ordonne du moins
    /// couteux au plus couteux (ADR-0015 §6, durcissement post-revue) :
    ///
    /// 1. budget `ATTEST` par emetteur (cle de transport) ;
    /// 2. borne de taille avant parse (`ATTEST_FRAME_MAX`) ;
    /// 3. `unpack` borne + controle de forme ;
    /// 4. **prefiltre curateur suivi** — le stockage/re-emission etant
    ///    reserves aux curateurs suivis, une attestation de curateur
    ///    inconnu est dropee *avant* toute crypto (borne Sybil : un
    ///    flot de signatures valides de cles inconnues ne coute qu'un
    ///    parse borne) ;
    /// 5. lookup dedup — Ed25519 est deterministe : meme
    ///    `(curateur, kind, sujet, ts, verdict)` = octets identiques
    ///    deja verifies → rejeu/stale absorbe sans `verify` ;
    /// 6. `ts` futur borne ;
    /// 7. `verify` Ed25519 — n'est atteint que pour une attestation
    ///    potentiellement nouvelle d'un curateur suivi ;
    /// 8. **conflit** : meme cle + meme `ts` + verdict different
    ///    (signature valide → equivoque averee du curateur) → rejet
    ///    sans ecrasement ni re-emission ;
    /// 9. `put` → re-emission aux autres pairs ext si nouvelle.
    fn on_attest(self: &Arc<Self>, pkt: &Packet) -> Result<(), Ipv8Error> {
        self.attest_rx.fetch_add(1, Ordering::Relaxed);
        // (1) Budget par emetteur avant tout travail utile.
        {
            let mut rates = self.attest_rate.lock().unwrap();
            let over = match rates.get_mut(&pkt.public_key_bin) {
                Some((start, n)) => {
                    if start.elapsed() >= self.settings.attest_rate_window {
                        *start = Instant::now();
                        *n = 0;
                    }
                    *n += 1;
                    *n > self.settings.attest_rate_max
                }
                None => {
                    if rates.len() >= self.settings.attest_rate_table_max
                        && self.settings.attest_rate_table_max > 0
                    {
                        true
                    } else {
                        rates.insert(pkt.public_key_bin.clone(), (Instant::now(), 1));
                        false
                    }
                }
            };
            if over {
                self.attest_dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        }
        // (2) Trop grande pour etre une v1 : drop sans lecture.
        if pkt.payload.len() > ATTEST_FRAME_MAX {
            self.attest_dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        // (3) Parse borne (Reader echoue vite sur troncature).
        let mut r = Reader::new(&pkt.payload);
        let att = Attestation::unpack(&mut r)?;
        // (4) Prefiltre curateur — avant la crypto.
        if !self.is_followed(&att.curator) {
            self.attest_dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        // (5) Dedup avant crypto : l'entree stockee est deja verifiee ;
        //     `ts` plus ancien ou identique+meme verdict = rien a faire.
        let existing = self
            .attest_store
            .lock()
            .unwrap()
            .get(&att.curator, att.kind, &att.subject);
        if let Some(old) = &existing {
            if old.ts > att.ts || (old.ts == att.ts && old.verdict == att.verdict) {
                self.attest_dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        }
        // (6) Futur au-dela de la derive toleree : dominerait le
        //     « latest wins » pour toujours — drop.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if att.ts > now.saturating_add(self.settings.attest_max_future_skew.as_secs()) {
            self.attest_dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        // (7) Crypto — seulement pour du potentiellement nouveau
        //     d'un curateur suivi.
        if !att.verify() {
            self.attest_dropped.fetch_add(1, Ordering::Relaxed);
            return Err(Ipv8Error::InvalidSignature);
        }
        // (8) Equivoque averee : deux signatures valides du meme
        //     curateur, meme `ts`, verdicts opposes — conflit : ni
        //     ecrasement ni re-emission (preuve conservee en log).
        if let Some(old) = &existing {
            if old.ts == att.ts && old.verdict != att.verdict {
                tracing::warn!(
                    curator = %att.curator_mid(),
                    kind = att.kind,
                    ts = att.ts,
                    "attestations contradictoires a ts identique — curateur equivoque"
                );
                self.attest_dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        }
        // (9) Stockage → re-emission borne par dedup.
        if self.attest_store.lock().unwrap().put(&att) {
            self.attest_stored.fetch_add(1, Ordering::Relaxed);
            let c = self.clone();
            let sender = pkt.public_key_bin.clone();
            tokio::spawn(async move {
                c.gossip_attest(&att, Some(&sender)).await;
            });
        }
        Ok(())
    }

    /// Score de confiance local d'un sujet : +1 endorse / -1 flag par
    /// curateur suivi (+ soi), dernier verdict seul comptant (dedup
    /// `(curateur, kind, sujet)` du store — un curateur ne pese
    /// qu'une fois).
    pub fn trust_info(&self, kind: u8, subject: &[u8]) -> TrustInfo {
        let atts = self.attest_store.lock().unwrap().by_subject(kind, subject);
        let mut t = TrustInfo {
            score: 0,
            endorsements: Vec::new(),
            flags: Vec::new(),
            attestation_count: atts.len(),
        };
        for a in atts {
            if !self.is_followed(&a.curator) {
                continue;
            }
            match a.verdict {
                attest_verdict::ENDORSE => {
                    t.score += 1;
                    t.endorsements.push(a.curator_mid());
                }
                attest_verdict::FLAG => {
                    t.score -= 1;
                    t.flags.push(a.curator_mid());
                }
                _ => {}
            }
        }
        t
    }

    /// `latest` attestations stockees (API — borne
    /// `attest_list_max`).
    pub fn attestations_latest(&self, limit: usize) -> Vec<Attestation> {
        self.attest_store
            .lock()
            .unwrap()
            .latest(limit.min(self.settings.attest_list_max))
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
        // Un `msg_id` ext inconnu (version plus recente) tombe dans
        // le `_` : ignore silencieusement — c'est le point
        // d'extension prevu par ADR-0015.
        match pkt.msg_id {
            msg::HELLO => {
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
            msg::ATTEST => self.on_attest(&pkt)?,
            _ => {}
        }
        Ok(())
    }

    /// Nombre de pairs connus comme ext (population OnionBit visible).
    pub fn ext_peer_count(&self) -> usize {
        self.ext_peers.lock().unwrap().len()
    }

    /// Instantane complet (reglages + pairs + compteurs ATTEST)
    /// pour `GET /api/ipv8/ext`.
    pub fn info(&self) -> ExtInfo {
        ExtInfo {
            hello_interval_secs: self.settings.hello_interval.as_secs(),
            hello_fanout: self.settings.hello_fanout,
            hello_cooldown_secs: self.settings.hello_cooldown.as_secs(),
            caps: self.settings.caps,
            peer_count: self.ext_peer_count(),
            peers: self.peers_info(),
            attest_rx: self.attest_rx.load(Ordering::Relaxed),
            attest_dropped: self.attest_dropped.load(Ordering::Relaxed),
            attest_stored: self.attest_stored.load(Ordering::Relaxed),
            attest_tx: self.attest_tx.load(Ordering::Relaxed),
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
        node_ext(caps, HashSet::new()).await
    }

    /// `node` avec curateurs suivis (curation ADR-0015 §6).
    async fn node_ext(
        caps: u64,
        curators: HashSet<Vec<u8>>,
    ) -> (
        Arc<OnionbitExtCommunity>,
        Arc<UdpEndpoint>,
        UdpAddress,
        LibNaClSecretKey,
    ) {
        node_full(ExtSettings {
            caps,
            curators,
            ..ExtSettings::default()
        })
        .await
    }

    /// `node` avec des reglages complets (budgets, stores...).
    async fn node_full(
        settings: ExtSettings,
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
        let c = OnionbitExtCommunity::new(key.clone(), network, ep.clone(), settings).await;
        let e = ep.clone();
        tokio::spawn(async move {
            let _ = e.run().await;
        });
        (c, ep, addr, key)
    }

    /// Etablit le lien ext `a -> b` (hello + reponse) : attend que
    /// les deux cotes se connaissent comme pairs ext.
    async fn link_ext(
        a: &Arc<OnionbitExtCommunity>,
        b: &Arc<OnionbitExtCommunity>,
        addr_b: &UdpAddress,
        pk_b: &[u8],
    ) {
        let pk_a = a.key.public_key().to_bin();
        a.send_hello(addr_b, pk_b).await;
        let b2 = b.clone();
        wait_until(move || b2.ext_peers.lock().unwrap().contains_key(&pk_a)).await;
        let a2 = a.clone();
        let pk_b = pk_b.to_vec();
        wait_until(move || a2.ext_peers.lock().unwrap().contains_key(&pk_b)).await;
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

    /// Gossip `ATTEST` : A publie → B (suiveur de A) stocke et
    /// re-emet → C stocke via B (B n'est pas le curateur —
    /// l'attestation est auto-portante) ; D qui ne suit pas A droppe.
    /// La dedup eteint le cycle : aucune re-emission au second
    /// exemplaire.
    #[tokio::test]
    async fn attest_gossip_propague_aux_suiveurs() {
        let (a, _ea, _aa, key_a) = node(0).await;
        let pk_a = key_a.public_key().to_bin();
        let followed_a = HashSet::from([pk_a.clone()]);
        let (b, _eb, addr_b, key_b) = node_ext(0, followed_a.clone()).await;
        let (c, _ec, addr_c, key_c) = node_ext(0, followed_a).await;
        // D ne suit personne : son store doit rester vide.
        let (d, _ed, addr_d, key_d) = node(0).await;

        link_ext(&a, &b, &addr_b, &key_b.public_key().to_bin()).await;
        link_ext(&b, &c, &addr_c, &key_c.public_key().to_bin()).await;
        link_ext(&b, &d, &addr_d, &key_d.public_key().to_bin()).await;

        let subject = [0xab; 20];
        a.publish_attestation(attest_kind::INFOHASH, &subject, attest_verdict::ENDORSE)
            .await
            .unwrap();

        // B a stocke (curateur suivi) puis re-emis vers C et D.
        let b2 = b.clone();
        let pk_a2 = pk_a.clone();
        wait_until(move || !b2.attestations_latest(10).is_empty()).await;
        let c2 = c.clone();
        wait_until(move || !c2.attestations_latest(10).is_empty()).await;
        // D : verifie puis droppe (curateur non suivi) — borne Sybil.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(d.attestations_latest(10).is_empty());

        // Contenu + score : C suit A → +1 endorse sur le sujet.
        let atts = c.attestations_latest(10);
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].curator, pk_a2);
        assert!(atts[0].verify());
        let t = c.trust_info(attest_kind::INFOHASH, &subject);
        assert_eq!(t.score, 1);
        assert_eq!(t.endorsements.len(), 1);
        // B a le meme score (meme curateur suivi).
        assert_eq!(b.trust_info(attest_kind::INFOHASH, &subject).score, 1);
        // D : sujet inconnu → score 0.
        assert_eq!(d.trust_info(attest_kind::INFOHASH, &subject).score, 0);
    }

    /// Une attestation a signature corrompue ou `ts` trop lointain
    /// n'est jamais stockee — meme d'un curateur suivi.
    #[tokio::test]
    async fn attest_invalide_ou_future_dropee() {
        let (a, _ea, addr_a, key_a) = node(0).await;
        let pk_a = key_a.public_key().to_bin();
        // B suit A : seuls les defauts cryptographiques/temporels
        // peuvent encore faire droper.
        let (_b, ep_b, _ab, _kb) = node_ext(0, HashSet::from([pk_a.clone()])).await;
        let send = |payload: Vec<u8>, key: &LibNaClSecretKey| {
            let ep = ep_b.clone();
            let addr = addr_a.clone();
            let key = key.clone();
            async move {
                let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key, &payload);
                ep.send_to(&addr, &pkt).await.unwrap();
            }
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Signature corrompue.
        let mut att = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &[0x11; 20],
            attest_verdict::ENDORSE,
            now,
        )
        .unwrap();
        att.signature[0] ^= 0xff;
        send(att.pack(), &key_a).await;
        // `ts` dans le futur au-dela de la derive (600 s par defaut).
        let fut = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &[0x22; 20],
            attest_verdict::ENDORSE,
            now + 3600,
        )
        .unwrap();
        send(fut.pack(), &key_a).await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(a.attestations_latest(10).is_empty());
        let _ = pk_a;
    }

    /// Re-emission d'une attestation deja connue : absorbe par la
    /// dedup du store, aucune nouvelle vague de gossip.
    #[tokio::test]
    async fn attest_rejeu_sans_reemission() {
        let (a, _ea, _aa, key_a) = node(0).await;
        let pk_a = key_a.public_key().to_bin();
        let (b, ep_b, addr_b, key_b) = node_ext(0, HashSet::from([pk_a])).await;
        let mut tap = ep_b.set_tap().await;
        link_ext(&a, &b, &addr_b, &key_b.public_key().to_bin()).await;

        let subject = [0xcd; 20];
        a.publish_attestation(attest_kind::INFOHASH, &subject, attest_verdict::FLAG)
            .await
            .unwrap();
        let b2 = b.clone();
        wait_until(move || !b2.attestations_latest(10).is_empty()).await;
        // Vider le tap (ATTEST initial A→B).
        while tap.try_recv().is_ok() {}
        // Rejeu identique A→B : dedup → B ne re-emet rien.
        let att = a.attestations_latest(1)[0].clone();
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key_a, &att.pack());
        a.endpoint.send_to(&addr_b, &pkt).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut reemitted = 0usize;
        while let Ok((dir, _dst, data)) = tap.try_recv() {
            if matches!(dir, TapDir::Tx)
                && data.get(crate::packet::PREFIX_LEN) == Some(&msg::ATTEST)
            {
                reemitted += 1;
            }
        }
        assert_eq!(reemitted, 0);
        // Verdict plus recent (remplace) : stocke, score retourne.
        let newer = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &subject,
            attest_verdict::ENDORSE,
            att.ts + 10,
        )
        .unwrap();
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key_a, &newer.pack());
        a.endpoint.send_to(&addr_b, &pkt).await.unwrap();
        let b3 = b.clone();
        wait_until(move || b3.trust_info(attest_kind::INFOHASH, &subject).score == 1).await;
    }

    /// `InMemoryAttestationStore` : dedup latest-wins + `latest`
    /// ordonne, isole du reseau.
    #[test]
    fn store_memoire_dedup_latest() {
        let s = InMemoryAttestationStore::default();
        let key = LibNaClSecretKey::generate();
        let mk = |subject: u8, ts: u64, v: u8| {
            Attestation::sign(&key, attest_kind::INFOHASH, &[subject; 20], v, ts).unwrap()
        };
        let a1 = mk(1, 10, attest_verdict::ENDORSE);
        assert!(s.put(&a1));
        assert!(!s.put(&a1)); // meme objet : absorbe
                              // Plus ancien : refuse.
        assert!(!s.put(&mk(1, 5, attest_verdict::FLAG)));
        // Plus recent : remplace.
        let a2 = mk(1, 20, attest_verdict::FLAG);
        assert!(s.put(&a2));
        assert_eq!(s.by_subject(attest_kind::INFOHASH, &[1; 20])[0].ts, 20);
        // Lookup cle exacte `get` (dedup avant crypto cote
        // protocole) : trouve pour (curateur, kind, sujet), `None`
        // pour un autre sujet ou un autre curateur.
        let g = s.get(&a2.curator, attest_kind::INFOHASH, &[1; 20]).unwrap();
        assert_eq!(g.ts, 20);
        assert!(s
            .get(&a2.curator, attest_kind::INFOHASH, &[9; 20])
            .is_none());
        assert!(s.get(&[0; 42], attest_kind::INFOHASH, &[1; 20]).is_none());
        // Autre sujet : ligne separee + tri latest.
        let b1 = mk(2, 15, attest_verdict::ENDORSE);
        assert!(s.put(&b1));
        let latest = s.latest(10);
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0].ts, 20);
        // Borne : eviction de la plus vieille entree.
        let s2 = InMemoryAttestationStore::bounded(1);
        let k2 = LibNaClSecretKey::generate();
        let e1 = Attestation::sign(
            &k2,
            attest_kind::INFOHASH,
            &[7; 20],
            attest_verdict::ENDORSE,
            1,
        )
        .unwrap();
        let e2 = Attestation::sign(
            &k2,
            attest_kind::INFOHASH,
            &[8; 20],
            attest_verdict::ENDORSE,
            2,
        )
        .unwrap();
        assert!(s2.put(&e1));
        assert!(s2.put(&e2));
        assert_eq!(s2.latest(10).len(), 1);
        assert_eq!(s2.latest(10)[0].subject, vec![8; 20]);
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

    /// Conflit d'equivoque a `ts` identique : `endorse(ts)` accepte,
    /// `flag(meme ts)` rejete — ni ecrasement ni re-emission, score
    /// et base inchanges (deux signatures valides du meme curateur
    /// pour le meme `ts` = equivoque averee, logguee en `warn`).
    #[tokio::test]
    async fn attest_conflit_meme_ts_sans_ecrasement() {
        let (a, _ea, _aa, key_a) = node(0).await;
        let pk_a = key_a.public_key().to_bin();
        let followed = HashSet::from([pk_a]);
        let (b, ep_b, addr_b, _kb) = node_ext(0, followed.clone()).await;
        let (c, _ec, addr_c, key_c) = node_ext(0, followed).await;
        let mut tap = ep_b.set_tap().await;
        link_ext(&b, &c, &addr_c, &key_c.public_key().to_bin()).await;

        let subject = [0x77; 20];
        let send = |att: &Attestation| {
            let ep = a.endpoint.clone();
            let addr = addr_b.clone();
            let key = key_a.clone();
            let payload = att.pack();
            async move {
                let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key, &payload);
                ep.send_to(&addr, &pkt).await.unwrap();
            }
        };
        // `endorse` ts=100 : stocke par B, re-emis vers C (suiveur).
        let endorse = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &subject,
            attest_verdict::ENDORSE,
            100,
        )
        .unwrap();
        send(&endorse).await;
        let b2 = b.clone();
        wait_until(move || b2.trust_info(attest_kind::INFOHASH, &subject).score == 1).await;
        let c2 = c.clone();
        wait_until(move || !c2.attestations_latest(10).is_empty()).await;
        while tap.try_recv().is_ok() {}

        // `flag` meme ts=100, meme cle : rejet comme conflit.
        let flag = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &subject,
            attest_verdict::FLAG,
            100,
        )
        .unwrap();
        send(&flag).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // B : le verdict stocke reste `endorse` — rien n'a bouge.
        assert_eq!(b.trust_info(attest_kind::INFOHASH, &subject).score, 1);
        assert_eq!(b.attestations_latest(10).len(), 1);
        let info = b.info();
        assert_eq!(info.attest_stored, 1);
        assert!(info.attest_dropped >= 1);
        // Aucune re-emission du flag : le tap ne montre aucun
        // ATTEST sortant depuis sa purge.
        let mut reemitted = 0usize;
        while let Ok((dir, _dst, data)) = tap.try_recv() {
            if matches!(dir, TapDir::Tx)
                && data.get(crate::packet::PREFIX_LEN) == Some(&msg::ATTEST)
            {
                reemitted += 1;
            }
        }
        assert_eq!(reemitted, 0);
        // C : toujours un seul verdict `endorse` — le conflit n'a
        // pas voyage.
        assert_eq!(c.trust_info(attest_kind::INFOHASH, &subject).score, 1);
        assert_eq!(c.attestations_latest(10).len(), 1);
    }

    /// Prefiltre curateur avant crypto : un curateur non suivi est
    /// droppe — y compris quand sa signature est corrompue (le
    /// `verify` Ed25519 n'est jamais paye) ; les compteurs rendent
    /// le drop observable aux bancs.
    #[tokio::test]
    async fn attest_prefiltre_curateur_non_suivi() {
        let (_a, ep_a, _aa, key_a) = node(0).await;
        // B ne suit personne : tout curateur est non suivi.
        let (b, _eb, addr_b, _kb) = node(0).await;
        // Attestation *valide* d'un curateur non suivi.
        let valid = Attestation::sign(
            &key_a,
            attest_kind::INFOHASH,
            &[0x55; 20],
            attest_verdict::ENDORSE,
            100,
        )
        .unwrap();
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key_a, &valid.pack());
        ep_a.send_to(&addr_b, &pkt).await.unwrap();
        // Signature corrompue d'un second curateur non suivi.
        let key_e = LibNaClSecretKey::generate();
        let mut bad = Attestation::sign(
            &key_e,
            attest_kind::INFOHASH,
            &[0x66; 20],
            attest_verdict::FLAG,
            100,
        )
        .unwrap();
        bad.signature[0] ^= 0xff;
        let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key_a, &bad.pack());
        ep_a.send_to(&addr_b, &pkt).await.unwrap();

        // Les deux dropees au prefiltre — la crypto n'a pas tourne.
        let b2 = b.clone();
        wait_until(move || b2.info().attest_rx == 2).await;
        let info = b.info();
        assert_eq!(info.attest_dropped, 2);
        assert_eq!(info.attest_stored, 0);
        assert!(b.attestations_latest(10).is_empty());
    }

    /// Budget `ATTEST` par emetteur : au-dela de `attest_rate_max`
    /// par fenetre, les messages sont droppes avant tout travail —
    /// borne CPU du chemin de reception meme pour un curateur suivi.
    #[tokio::test]
    async fn attest_budget_par_pair() {
        let (a, _ea, _aa, key_a) = node(0).await;
        let pk_a = key_a.public_key().to_bin();
        let (b, _eb, addr_b, _kb) = node_full(ExtSettings {
            curators: HashSet::from([pk_a]),
            attest_rate_max: 2,
            ..ExtSettings::default()
        })
        .await;
        for i in 0..4u8 {
            let att = Attestation::sign(
                &key_a,
                attest_kind::INFOHASH,
                &[i; 20],
                attest_verdict::ENDORSE,
                100 + u64::from(i),
            )
            .unwrap();
            let pkt = Packet::sign_no_dist(&EXT_COMMUNITY_ID, msg::ATTEST, &key_a, &att.pack());
            a.endpoint.send_to(&addr_b, &pkt).await.unwrap();
        }
        let b2 = b.clone();
        wait_until(move || b2.info().attest_rx == 4).await;
        let info = b.info();
        // Seuls les 2 premiers ont traverse le budget.
        assert_eq!(info.attest_stored, 2);
        assert_eq!(info.attest_dropped, 2);
    }
}
