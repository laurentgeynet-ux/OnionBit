// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Service de messagerie anonyme (ADR-0011, Phase 8 etape 37) —
//! orchestration : jointure du swarm de presence, resolution des
//! points d'introduction, liaison e2e, demultiplexage des cellules
//! `data` par swarm messagerie, emission.
//!
//! Le **codec** et la **crypto applicative** vivent dans
//! `onionbit-messaging` (proprietaire unique du protocole) ; ce
//! service ne fait que brancher le protocole sur les circuits e2e
//! de `TunnelCommunity`.
//!
//! Separation des lanes : un swarm messagerie
//! (`messaging_hash(pk)`) n'apparait JAMAIS dans `swarm_lookup`
//! (mapping BitTorrent `lookup -> download`) — le listener e2e de
//! `ipv8_stack` ignore donc nos circuits et l'injection uTP ne peut
//! pas recevoir de trame messagerie (MS-9).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use onionbit_db::messaging as dbm;
use onionbit_db::Database;
use onionbit_ipv8::UdpAddress;
use onionbit_messaging::{
    derive_messaging_keys, messaging_hash, preflight, Frame, MessagingConfig, MessagingError,
    MessagingKeys, MsgKind, RawFrame, RecvWindow,
};
use onionbit_tunnel::community::TunnelCommunity;
use onionbit_tunnel::routing::IntroductionPoint;
use tokio::sync::{broadcast, watch};

use crate::error::{CoreError, Result};

/// Capacite du canal d'evenements applicatifs (bound interne — les
/// consommateurs lents perdent des evenements, jamais le service).
const EVENTS_CAP: usize = 256;
/// Timeout d'attente de liaison e2e dans `connect`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Granularite de scrutation de la liaison dans `connect`.
const CONNECT_POLL: Duration = Duration::from_millis(50);

/// Secondes Unix courantes.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Adresse non definie `0.0.0.0:0` — `dest`/`origin` des cellules
/// `data` messagerie : le relais de rendez-vous n'interprete pas ces
/// champs (contrairement aux sorties UDP), et la trame ne pretend
/// a aucune forme applicative connue (jamais uTP).
fn unspecified() -> UdpAddress {
    UdpAddress::Ipv4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// Evenement applicatif emis par le service (futur relais API/SSE,
/// etape 40 — et oracle des bancs MS-*).
#[derive(Debug, Clone)]
pub enum MessagingEvent {
    /// Trame authentifiee admise : codec + signature Ed25519 +
    /// dechiffrement + anti-replay ont tous passe.
    Frame {
        /// `pk_bin` de l'emetteur verifie.
        contact: Vec<u8>,
        /// Type de trame.
        kind: MsgKind,
        /// `id` de trame (cible des `ack`, etape 39).
        id: [u8; 16],
        /// Corps applicatif en clair.
        body: Vec<u8>,
    },
    /// Circuit e2e lie a un contact connu (initiateur, ou repondant
    /// resolu par une premiere trame verifiee).
    Bound {
        /// `pk_bin` du contact.
        contact: Vec<u8>,
        /// `circuit_id` lie.
        circuit_id: u32,
    },
    /// Circuit repondant lie (`RP_SEEDER`) — l'expediteur n'est pas
    /// encore identifie ; il le sera a la premiere trame verifiee.
    Pending {
        /// `circuit_id` lie.
        circuit_id: u32,
    },
    /// Demande de consentement : un contact inconnu a envoye un
    /// `hello` verifie — decision utilisateur requise
    /// (`accept_contact`/`refuse_contact`/`block_contact`).
    Consent {
        /// `pk_bin` de l'emetteur verifie.
        contact: Vec<u8>,
        /// `circuit_id` lie.
        circuit_id: u32,
    },
    /// Echec de livraison borne et visible (ADR-0011 : online-only,
    /// pas de file ni de re-emission — le message est enregistre
    /// `failed` dans l'historique).
    Undeliverable {
        /// `pk_bin` du contact visé.
        contact: Vec<u8>,
        /// `id` de la trame perdue.
        id: [u8; 16],
    },
}

/// Etat de consentement d'un contact (ADR-0011, etape 38).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactState {
    /// Consenti : trames livrees a l'application.
    Active,
    /// En attente de decision utilisateur — borne (`pending_cap`)
    /// et perissable (`pending_ttl`) ; les trames verifiees sont
    /// ecartees sans livraison tant que l'etat ne change pas.
    Pending,
    /// Bloque : trames ignorees, swarm de contact non joint,
    /// circuits detruits a l'identification.
    Blocked,
}

/// Seau a jetons de limitation (trames/s). `rate == 0` = illimite
/// (le seau n'est alors pas construit).
struct TokenBucket {
    /// Jetons disponibles.
    tokens: f64,
    /// Rafale maximale (= capacite).
    cap: f64,
    /// Recharge par seconde.
    rate: f64,
    /// Derniere recharge.
    last: Instant,
}

impl TokenBucket {
    /// Seau plein de capacite = `rate` (rafale d'une seconde).
    fn new(rate: u32) -> Option<Self> {
        (rate > 0).then(|| Self {
            tokens: f64::from(rate),
            cap: f64::from(rate),
            rate: f64::from(rate),
            last: Instant::now(),
        })
    }

    /// Consomme un jeton ; `false` si le seau est vide.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.cap);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Compteurs de drops du demux (oracle MS-10 + exposition API).
#[derive(Debug, Default)]
pub struct MessagingStats {
    /// Rejets au prefiltre codec (taille/version/malforme).
    pub codec: AtomicU64,
    /// Rejets par le seau global.
    pub rate_global: AtomicU64,
    /// Rejets par le seau du contact.
    pub rate_contact: AtomicU64,
    /// Trames de contacts `blocked`.
    pub blocked: AtomicU64,
    /// `hello` refuses quand `pending` est plein.
    pub pending_full: AtomicU64,
    /// Rejets anti-replay (`seq`/`id`).
    pub replay: AtomicU64,
    /// Trames verifiees d'un contact `pending` (non livrees).
    pub pending_drop: AtomicU64,
}

/// Etat d'un contact (cle : `pk_bin`).
struct Contact {
    /// Cle publique du contact (verification des signatures).
    pk: LibNaClPublicKey,
    /// Etat de consentement.
    state: ContactState,
    /// Entree en `pending` (secondes Unix — TTL `pending_ttl`).
    pending_since: u64,
    /// Fenetre anti-replay entrante (persiste d'un circuit a l'autre
    /// — une re-emission honnete apres reouverture est absorbee par
    /// la dedup `id`, MS-2).
    recv_window: RecvWindow,
    /// Prochain `seq` sortant (monotone par direction).
    send_seq: u64,
    /// Circuit e2e lie a ce contact (`None` = a etablir).
    circuit: Option<u32>,
    /// `hello` deja emis sur le circuit lie.
    greeted: bool,
    /// Seau a jetons par contact (`None` = illimite).
    bucket: Option<TokenBucket>,
}

impl Contact {
    /// Contact `Active` neuf (intention locale : `resolve`/`connect`).
    fn active(pk: LibNaClPublicKey, cfg: &MessagingConfig) -> Self {
        Self {
            pk,
            state: ContactState::Active,
            pending_since: 0,
            recv_window: RecvWindow::new(cfg),
            send_seq: 0,
            circuit: None,
            greeted: false,
            bucket: TokenBucket::new(cfg.per_contact_rate),
        }
    }

    /// Contact `Pending` neuf (`hello` entrant verifie d'un inconnu).
    fn pending(pk: LibNaClPublicKey, cfg: &MessagingConfig) -> Self {
        let mut c = Self::active(pk, cfg);
        c.state = ContactState::Pending;
        c.pending_since = now_secs();
        c
    }
}

/// Liaison d'un circuit e2e a la messagerie.
struct CircuitBinding {
    /// Cles applicatives HKDF de cette liaison (`send`/`recv` selon
    /// notre role — initiateur ou repondant).
    keys: MessagingKeys,
    /// `pk_bin` du contact si connu : l'initiateur le sait des la
    /// liaison (le swarm cible porte `messaging_hash(pk)`), le
    /// repondant l'apprend a la premiere trame verifiee.
    contact: Option<Vec<u8>>,
}

/// Service de messagerie e2e (une instance par `Ipv8Stack`).
pub struct MessagingService {
    /// Community tunnel — transport uniquement.
    tunnel: Arc<TunnelCommunity>,
    /// Cle d'identite du demon (signature des trames + `seeder_sk`
    /// du swarm de presence : le `seeder_pk` publie est notre `pk`,
    /// donc le handshake e2e authentifie le destinataire).
    key: LibNaClSecretKey,
    /// Bornes du protocole (`onionbit-messaging`).
    cfg: MessagingConfig,
    /// Sauts des circuits messagerie (`hops` de `join_swarm`).
    hops: usize,
    /// `messaging_hash` de notre propre swarm de presence.
    own_mh: [u8; 20],
    /// Swarms de contacts joints : `messaging_hash(pk)` -> `pk_bin`.
    swarms: Mutex<HashMap<[u8; 20], Vec<u8>>>,
    /// Etat par contact (`pk_bin`).
    contacts: Mutex<HashMap<Vec<u8>, Contact>>,
    /// Liaisons par `circuit_id`.
    circuits: Mutex<HashMap<u32, CircuitBinding>>,
    /// Seau a jetons global (toutes trames entrantes confondues —
    /// borne le cout codec+verification, MS-10).
    global_bucket: Mutex<Option<TokenBucket>>,
    /// Persistance contacts+messages (`None` = memoire seule,
    /// tests). En clair v1 — assume dans l'ADR.
    db: Option<Arc<Database>>,
    /// Compteurs de drops (oracle des bancs + API).
    stats: MessagingStats,
    /// Emetteur d'evenements applicatifs.
    events_tx: broadcast::Sender<MessagingEvent>,
    /// Arrets des taches du service.
    stops: Mutex<Vec<watch::Sender<bool>>>,
}

impl MessagingService {
    /// Demarre le service : joint le swarm de presence
    /// (`messaging_hash(own_pk)`, `seeder_sk` = cle d'identite),
    /// lance le relais `e2e_ready` et le moniteur de presence
    /// (points d'introduction + re-annonce DHT).
    ///
    /// `hops` = sauts des circuits `IP_SEEDER`/`RP_DOWNLOADER`
    /// (hors +1 swarm automatique — comme les `anon_hops` BT).
    /// `db` = persistance (`None` = memoire seule, tests).
    pub fn start(
        tunnel: Arc<TunnelCommunity>,
        key: LibNaClSecretKey,
        cfg: MessagingConfig,
        hops: usize,
        db: Option<Arc<Database>>,
    ) -> Arc<Self> {
        let own_mh = messaging_hash(&key.public_key());
        // `seeder_sk` = cle d'identite : le `seeder_pk` annonce par
        // les points d'introduction est notre `pk` publique — le
        // `create-e2e` d'un expéditeur verifie `auth` contre elle
        // (authentification transport DU destinataire ; le sens
        // inverse est prouve par les signatures de trames).
        tunnel.join_swarm_with_key(own_mh, hops, Some(key.clone()));
        let (events_tx, _) = broadcast::channel(EVENTS_CAP);
        let svc = Arc::new(Self {
            tunnel,
            key,
            global_bucket: Mutex::new(TokenBucket::new(cfg.global_rate)),
            cfg,
            hops,
            own_mh,
            swarms: Mutex::new(HashMap::new()),
            contacts: Mutex::new(HashMap::new()),
            circuits: Mutex::new(HashMap::new()),
            db,
            stats: MessagingStats::default(),
            events_tx,
            stops: Mutex::new(Vec::new()),
        });
        svc.load_state();
        svc.spawn_e2e_listener();
        svc.spawn_presence_monitor();
        svc
    }

    /// Restauration au restart (MS-11) : etat de consentement,
    /// compteur `send_seq` et `recv_top` (fenetre reprise en
    /// conservateur) de chaque contact persiste.
    fn load_state(&self) {
        let Some(db) = &self.db else { return };
        let rows = match db.with(dbm::list_contacts) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "restauration messagerie impossible");
                return;
            }
        };
        let mut contacts = self.contacts.lock().unwrap();
        for row in rows {
            let Ok(pk) = LibNaClPublicKey::from_bin(&row.public_key) else {
                continue;
            };
            let mut c = Contact::active(pk, &self.cfg);
            c.send_seq = row.send_seq.max(0) as u64;
            if row.recv_top > 0 {
                c.recv_window = RecvWindow::resume(&self.cfg, row.recv_top as u64);
            }
            match row.state.as_str() {
                "blocked" => c.state = ContactState::Blocked,
                "pending" => {
                    c.state = ContactState::Pending;
                    // Le TTL repart a la restauration (le pair peut
                    // renvoyer un `hello` — le pending redevient utile).
                    c.pending_since = now_secs();
                }
                _ => {}
            }
            contacts.insert(row.public_key, c);
        }
    }

    /// Abonne un receveur aux evenements applicatifs.
    pub fn subscribe(&self) -> broadcast::Receiver<MessagingEvent> {
        self.events_tx.subscribe()
    }

    /// Cle publique `pk_bin` de notre identite messagerie
    /// (= cle d'identite du demon — exposition API).
    pub fn public_key_bin(&self) -> Vec<u8> {
        self.key.public_key().to_bin()
    }

    /// `messaging_hash` de notre propre swarm de presence
    /// (exposition API : c'est l'adresse que nos contacts resolvent).
    pub fn own_messaging_hash(&self) -> [u8; 20] {
        self.own_mh
    }

    /// Arret du service : quitte les swarms messagerie et stoppe les
    /// taches (les circuits e2e meurent avec leurs circuits tunnels).
    pub fn stop(&self) {
        self.tunnel.leave_swarm(&self.own_mh);
        let mh: Vec<[u8; 20]> = self.swarms.lock().unwrap().keys().copied().collect();
        for m in mh {
            self.tunnel.leave_swarm(&m);
        }
        for tx in self.stops.lock().unwrap().drain(..) {
            let _ = tx.send(true);
        }
    }

    /// Resout les points d'introduction d'un contact via le chemin
    /// DHT de la sortie d'un circuit (`send_peers_request(None)`).
    /// Joint le swarm du contact en downloader si besoin.
    /// Refuse si le contact est `blocked`.
    pub async fn resolve(&self, contact_pk: &[u8]) -> Result<Vec<IntroductionPoint>> {
        let pk = LibNaClPublicKey::from_bin(contact_pk)
            .map_err(|e| CoreError::State(format!("cle de contact invalide: {e}")))?;
        self.require_not_blocked(contact_pk)?;
        let mh = self.ensure_contact_swarm(&pk, contact_pk);
        self.tunnel
            .send_peers_request_when_ready(mh, None, self.hops)
            .await
            .map_err(|e| CoreError::State(format!("peers-request messagerie: {e}")))
    }

    /// Lie un circuit e2e vers un contact via un point
    /// d'introduction (chemin PEX, ou un IP resolu par [`Self::resolve`]).
    /// Retourne le `circuit_id` lie. Refuse si le contact est
    /// `blocked`.
    pub async fn connect(&self, contact_pk: &[u8], intro: &IntroductionPoint) -> Result<u32> {
        let pk = LibNaClPublicKey::from_bin(contact_pk)
            .map_err(|e| CoreError::State(format!("cle de contact invalide: {e}")))?;
        self.require_not_blocked(contact_pk)?;
        let mh = self.ensure_contact_swarm(&pk, contact_pk);
        // Circuit deja lie -> rien a faire (idempotent).
        if let Some(cid) = self.contact_circuit(contact_pk) {
            return Ok(cid);
        }
        self.tunnel
            .create_e2e(mh, intro)
            .await
            .map_err(|e| CoreError::State(format!("create-e2e messagerie: {e}")))?;
        // Le relais `e2e_ready` pose la liaison de facon asynchrone :
        // scruter `contacts[pk].circuit` jusqu'au timeout.
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            if let Some(cid) = self.contact_circuit(contact_pk) {
                return Ok(cid);
            }
            if std::time::Instant::now() >= deadline {
                return Err(CoreError::State("timeout de liaison e2e messagerie".into()));
            }
            tokio::time::sleep(CONNECT_POLL).await;
        }
    }

    /// Envoie un message applicatif a un contact dont le circuit est
    /// lie (voir [`Self::connect`]). `NotConnected` (InvalidState) si
    /// aucun circuit — la resolution/liaison explicite reste a la
    /// charge de l'appelant (online-only : pas de file).
    ///
    /// Un `hello` (corps = notre `pk_bin`) precede le premier
    /// message sur chaque liaison : c'est la trame qui identifie
    /// l'expediteur cote repondant (consentement = etape 38).
    ///
    /// Exige un contact `Active` — sur un contact `pending`, il
    /// faut d'abord [`Self::accept_contact`] (le refus et le blocage
    /// sont `refuse_contact`/`block_contact`).
    pub async fn send(&self, contact_pk: &[u8], body: Vec<u8>) -> Result<[u8; 16]> {
        self.require_state(contact_pk, ContactState::Active)?;
        let Some(cid) = self.contact_circuit(contact_pk) else {
            // Online-only : pas de file — le message est enregistre
            // `failed` (visible en historique) et signale
            // `Undeliverable` (MS-7).
            let seq = self.peek_send_seq(contact_pk);
            let f = Frame::new(MsgKind::Msg, seq, now_secs(), body);
            self.persist_message(Self::msg_row(
                contact_pk, "out", f.seq, f.ts, &f.body, "failed", &f.id,
            ));
            let _ = self.events_tx.send(MessagingEvent::Undeliverable {
                contact: contact_pk.to_vec(),
                id: f.id,
            });
            return Err(CoreError::InvalidState(
                "messagerie : contact hors ligne — non delivre",
            ));
        };
        if !self.is_greeted(contact_pk) {
            self.send_frame(
                contact_pk,
                cid,
                MsgKind::Hello,
                self.key.public_key().to_bin(),
            )
            .await?;
        }
        self.send_frame(contact_pk, cid, MsgKind::Msg, body).await
    }

    /// Accepte la demande de consentement d'un contact `pending` :
    /// passe `Active`, notifie l'emetteur (`accept`) et re-emet
    /// `Bound`. Sans effet si le contact n'est pas `pending`.
    pub async fn accept_contact(&self, contact_pk: &[u8]) -> Result<()> {
        let cid = {
            let mut contacts = self.contacts.lock().unwrap();
            let Some(c) = contacts.get_mut(contact_pk) else {
                return Err(CoreError::InvalidState("messagerie : contact inconnu"));
            };
            if c.state != ContactState::Pending {
                return Err(CoreError::InvalidState(
                    "messagerie : contact non en attente",
                ));
            }
            c.state = ContactState::Active;
            c.circuit
        };
        self.persist_contact(contact_pk, ContactState::Active);
        if let Some(cid) = cid {
            // Notification au pair (best effort — le circuit peut
            // mourir entre-temps sans invalider l'acceptation).
            let _ = self
                .send_frame(contact_pk, cid, MsgKind::Accept, Vec::new())
                .await;
        }
        let _ = self.events_tx.send(MessagingEvent::Bound {
            contact: contact_pk.to_vec(),
            circuit_id: cid.unwrap_or_default(),
        });
        Ok(())
    }

    /// Refuse la demande : notifie `reject` (best effort) puis
    /// oublie le contact — un futur `hello` le remettra en
    /// `pending`. Pour un refus definitif, [`Self::block_contact`].
    pub async fn refuse_contact(&self, contact_pk: &[u8]) -> Result<()> {
        let cid = self.contact_circuit(contact_pk);
        if let Some(cid) = cid {
            let _ = self
                .send_frame(contact_pk, cid, MsgKind::Reject, Vec::new())
                .await;
        }
        self.remove_contact(contact_pk).await;
        Ok(())
    }

    /// Bloque un contact : trames entrantes ignorees a la
    /// verification, swarm de contact quitte (plus jamais joint),
    /// circuit detruit. Persistant jusqu'a [`Self::unblock_contact`].
    pub async fn block_contact(&self, contact_pk: &[u8]) -> Result<()> {
        let mh = {
            let mut contacts = self.contacts.lock().unwrap();
            let Some(c) = contacts.get_mut(contact_pk) else {
                return Err(CoreError::InvalidState("messagerie : contact inconnu"));
            };
            c.state = ContactState::Blocked;
            c.pending_since = 0;
            messaging_hash(&c.pk)
        };
        self.persist_contact(contact_pk, ContactState::Blocked);
        // Desarme l'acceptation e2e du swarm du contact.
        self.tunnel.leave_swarm(&mh);
        self.swarms.lock().unwrap().remove(&mh);
        if let Some(cid) = self.contact_circuit(contact_pk) {
            self.unbind_circuit(cid);
            self.tunnel.remove_circuit(cid, "contact bloque").await;
        }
        Ok(())
    }

    /// Debloque un contact (oublie son etat — un futur `hello` le
    /// remet en `pending`).
    pub async fn unblock_contact(&self, contact_pk: &[u8]) -> Result<()> {
        if self.contact_state(contact_pk) != Some(ContactState::Blocked) {
            return Err(CoreError::InvalidState("messagerie : contact non bloque"));
        }
        self.remove_contact(contact_pk).await;
        Ok(())
    }

    /// Contacts en attente de consentement : `(pk_bin, depuis_secs)`.
    pub fn pending_contacts(&self) -> Vec<(Vec<u8>, u64)> {
        let now = now_secs();
        self.contacts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, c)| c.state == ContactState::Pending)
            .map(|(pk, c)| (pk.clone(), now.saturating_sub(c.pending_since)))
            .collect()
    }

    /// Etat de consentement d'un contact (`None` = inconnu).
    pub fn contact_state(&self, contact_pk: &[u8]) -> Option<ContactState> {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .map(|c| c.state)
    }

    /// Retention des messages d'un contact : `retention_secs=0` =
    /// conservation illimitee ; `secure_delete` zeroise le corps en
    /// base avant le `DELETE` d'expiration (purge au tick du
    /// moniteur de presence).
    pub fn set_retention(
        &self,
        contact_pk: &[u8],
        retention_secs: u64,
        secure_delete: bool,
    ) -> Result<()> {
        if self.contact_state(contact_pk).is_none() {
            return Err(CoreError::InvalidState("messagerie : contact inconnu"));
        }
        let Some(db) = &self.db else { return Ok(()) };
        let now = now_secs() as i64;
        db.with(|c| dbm::set_retention(c, contact_pk, retention_secs as i64, secure_delete, now))
            .map_err(|e| CoreError::State(format!("retention messagerie: {e}")))
    }

    /// Historique borne d'un contact (le plus recent d'abord).
    pub fn history(&self, contact_pk: &[u8], limit: u32) -> Result<Vec<dbm::MsgMessageRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        db.with(|c| dbm::list_messages(c, contact_pk, limit))
            .map_err(|e| CoreError::State(format!("historique messagerie: {e}")))
    }

    /// Suppression reelle d'un message (`DELETE` — pas de marqueur).
    pub fn delete_message(&self, id: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        db.with(|c| dbm::delete_message(c, id))
            .map_err(|e| CoreError::State(format!("suppression message: {e}")))
    }

    /// Contacts persistes (passe relais vers l'API — etape 40).
    pub fn stored_contacts(&self) -> Result<Vec<dbm::MsgContactRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        db.with(dbm::list_contacts)
            .map_err(|e| CoreError::State(format!("liste contacts: {e}")))
    }

    /// Compteurs de drops (instantane).
    pub fn stats_snapshot(&self) -> [(&'static str, u64); 7] {
        [
            ("codec", self.stats.codec.load(Ordering::Relaxed)),
            (
                "rate_global",
                self.stats.rate_global.load(Ordering::Relaxed),
            ),
            (
                "rate_contact",
                self.stats.rate_contact.load(Ordering::Relaxed),
            ),
            ("blocked", self.stats.blocked.load(Ordering::Relaxed)),
            (
                "pending_full",
                self.stats.pending_full.load(Ordering::Relaxed),
            ),
            ("replay", self.stats.replay.load(Ordering::Relaxed)),
            (
                "pending_drop",
                self.stats.pending_drop.load(Ordering::Relaxed),
            ),
        ]
    }

    /// Persiste l'etat de consentement d'un contact (upsert — les
    /// compteurs `seq` conserves cote DB par l'`ON CONFLICT`).
    fn persist_contact(&self, pk_bin: &[u8], state: ContactState) {
        let Some(db) = &self.db else { return };
        let st = match state {
            ContactState::Active => "active",
            ContactState::Pending => "pending",
            ContactState::Blocked => "blocked",
        };
        let (send_seq, recv_top) = {
            let contacts = self.contacts.lock().unwrap();
            contacts
                .get(pk_bin)
                .map(|c| (c.send_seq, c.recv_window.top().unwrap_or(0)))
                .unwrap_or_default()
        };
        let now = now_secs() as i64;
        let row = dbm::MsgContactRow {
            public_key: pk_bin.to_vec(),
            state: st.into(),
            send_seq: send_seq as i64,
            recv_top: recv_top as i64,
            retention_secs: 0,
            secure_delete: false,
            created_at: now,
            updated_at: now,
        };
        if let Err(e) = db.with(|c| dbm::upsert_contact(c, &row)) {
            tracing::warn!(error = %e, "persistance contact messagerie");
        }
    }

    /// Persiste les compteurs `seq` du contact (apres chaque trame
    /// emise/admise — la reprise au restart est exacte).
    fn persist_seqs(&self, pk_bin: &[u8]) {
        let Some(db) = &self.db else { return };
        let (send_seq, recv_top) = {
            let contacts = self.contacts.lock().unwrap();
            match contacts.get(pk_bin) {
                Some(c) => (c.send_seq, c.recv_window.top().unwrap_or(0)),
                None => return,
            }
        };
        let now = now_secs() as i64;
        if let Err(e) = db.with(|c| dbm::set_seqs(c, pk_bin, send_seq as i64, recv_top as i64, now))
        {
            tracing::warn!(error = %e, "persistance seqs messagerie");
        }
    }

    /// Persiste un message (`direction` `in`/`out`, `status` de
    /// livraison — voir [`dbm::MsgMessageRow`]).
    fn persist_message(&self, row: dbm::MsgMessageRow) {
        let Some(db) = &self.db else { return };
        if let Err(e) = db.with(|c| dbm::insert_message(c, &row)) {
            tracing::warn!(error = %e, "persistance message");
        }
    }

    /// Ligne `msg_messages` prete a inserer.
    fn msg_row(
        contact_pk: &[u8],
        direction: &'static str,
        seq: u64,
        ts: u64,
        body: &[u8],
        status: &'static str,
        id: &[u8; 16],
    ) -> dbm::MsgMessageRow {
        dbm::MsgMessageRow {
            id: id.to_vec(),
            contact_pk: contact_pk.to_vec(),
            direction: direction.into(),
            seq: seq as i64,
            ts: ts as i64,
            body: body.to_vec(),
            status: status.into(),
            created_at: now_secs() as i64,
        }
    }

    /// Statut de livraison d'un message emis (`acked`/`failed`).
    fn set_msg_status(&self, id: &[u8; 16], status: &'static str) {
        let Some(db) = &self.db else { return };
        if let Err(e) = db.with(|c| dbm::set_message_status(c, id, status)) {
            tracing::warn!(error = %e, "persistance statut message");
        }
    }

    /// Erreur si le contact existe et est `blocked`.
    fn require_not_blocked(&self, contact_pk: &[u8]) -> Result<()> {
        if self.contact_state(contact_pk) == Some(ContactState::Blocked) {
            return Err(CoreError::InvalidState("messagerie : contact bloque"));
        }
        Ok(())
    }

    /// Erreur si le contact n'est pas dans l'etat attendu.
    fn require_state(&self, contact_pk: &[u8], state: ContactState) -> Result<()> {
        match self.contact_state(contact_pk) {
            Some(s) if s == state => Ok(()),
            Some(ContactState::Blocked) => {
                Err(CoreError::InvalidState("messagerie : contact bloque"))
            }
            Some(ContactState::Pending) => Err(CoreError::InvalidState(
                "messagerie : consentement en attente",
            )),
            _ => Err(CoreError::InvalidState("messagerie : contact inconnu")),
        }
    }

    /// Oublie completement un contact : etat, liaison, swarm,
    /// persistance (`DELETE` reel — messages en cascade).
    async fn remove_contact(&self, contact_pk: &[u8]) {
        if let Some(db) = &self.db {
            if let Err(e) = db.with(|c| dbm::delete_contact(c, contact_pk)) {
                tracing::warn!(error = %e, "suppression contact messagerie");
            }
        }
        let removed = self.contacts.lock().unwrap().remove(contact_pk);
        if let Some(c) = removed {
            let mh = messaging_hash(&c.pk);
            self.tunnel.leave_swarm(&mh);
            self.swarms.lock().unwrap().remove(&mh);
            if let Some(cid) = c.circuit {
                self.unbind_circuit(cid);
                self.tunnel.remove_circuit(cid, "contact supprime").await;
            }
        }
    }

    /// Purge les `pending` expires (`pending_ttl`) — appele au tick
    /// du moniteur et avant chaque admission.
    fn purge_expired_pending(&self) {
        let now = now_secs();
        let ttl = self.cfg.pending_ttl.as_secs();
        let expired: Vec<(Vec<u8>, Option<u32>)> = {
            let mut contacts = self.contacts.lock().unwrap();
            let expired: Vec<Vec<u8>> = contacts
                .iter()
                .filter(|(_, c)| {
                    c.state == ContactState::Pending && now.saturating_sub(c.pending_since) >= ttl
                })
                .map(|(pk, _)| pk.clone())
                .collect();
            expired
                .into_iter()
                .filter_map(|pk| contacts.remove(&pk).map(|c| (pk, c.circuit)))
                .collect()
        };
        if let Some(db) = &self.db {
            for (pk, _) in &expired {
                if let Err(e) = db.with(|c| dbm::delete_contact(c, pk)) {
                    tracing::warn!(error = %e, "purge contact expire");
                }
            }
        }
        for (_, cid) in expired {
            if let Some(cid) = cid {
                self.unbind_circuit(cid);
                let t = self.tunnel.clone();
                tokio::spawn(async move {
                    t.remove_circuit(cid, "pending expire").await;
                });
            }
        }
    }

    /// Snapshot diagnostic : `(pk_bin, circuit lie)` par contact.
    pub fn bound_contacts(&self) -> Vec<(Vec<u8>, Option<u32>)> {
        self.contacts
            .lock()
            .unwrap()
            .iter()
            .map(|(pk, c)| (pk.clone(), c.circuit))
            .collect()
    }

    /// `circuit_id` lie du contact, s'il existe.
    fn contact_circuit(&self, contact_pk: &[u8]) -> Option<u32> {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .and_then(|c| c.circuit)
    }

    /// `send_seq` courant du contact (sans consommer — sert a
    /// dater les messages `failed` hors ligne).
    fn peek_send_seq(&self, contact_pk: &[u8]) -> u64 {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .map(|c| c.send_seq)
            .unwrap_or_default()
    }

    /// `hello` deja emis pour ce contact.
    fn is_greeted(&self, contact_pk: &[u8]) -> bool {
        self.contacts
            .lock()
            .unwrap()
            .get(contact_pk)
            .is_some_and(|c| c.greeted)
    }

    /// Joint le swarm du contact en downloader (`seeder_sk = None`)
    /// et cree l'etat de contact `Active` (intention locale) ;
    /// retourne le `messaging_hash`. Un contact `blocked` ne joint
    /// jamais son swarm (acceptation e2e desarmee).
    fn ensure_contact_swarm(&self, pk: &LibNaClPublicKey, pk_bin: &[u8]) -> [u8; 20] {
        let mh = messaging_hash(pk);
        let (blocked, created) = {
            let mut contacts = self.contacts.lock().unwrap();
            let existed = contacts.contains_key(pk_bin);
            let c = contacts
                .entry(pk_bin.to_vec())
                .or_insert_with(|| Contact::active(pk.clone(), &self.cfg));
            (c.state == ContactState::Blocked, !existed)
        };
        if created {
            self.persist_contact(pk_bin, ContactState::Active);
        }
        if !blocked {
            let mut swarms = self.swarms.lock().unwrap();
            if let std::collections::hash_map::Entry::Vacant(e) = swarms.entry(mh) {
                e.insert(pk_bin.to_vec());
                self.tunnel.join_swarm(mh, self.hops, false);
            }
        }
        mh
    }

    /// Scelle et emet une trame sur le circuit du contact.
    async fn send_frame(
        &self,
        contact_pk: &[u8],
        cid: u32,
        kind: MsgKind,
        body: Vec<u8>,
    ) -> Result<[u8; 16]> {
        let (send_key, seq) = {
            let circuits = self.circuits.lock().unwrap();
            let mut contacts = self.contacts.lock().unwrap();
            let (Some(binding), Some(contact)) = (circuits.get(&cid), contacts.get_mut(contact_pk))
            else {
                return Err(CoreError::InvalidState("messagerie : liaison disparue"));
            };
            let seq = contact.send_seq;
            contact.send_seq += 1;
            (binding.keys.send, seq)
        };
        let frame = Frame::new(kind, seq, now_secs(), body);
        let wire = frame
            .seal(&self.key, &send_key, &self.cfg)
            .map_err(|e| CoreError::State(format!("seal trame: {e}")))?;
        if kind == MsgKind::Msg {
            self.persist_message(Self::msg_row(
                contact_pk,
                "out",
                seq,
                frame.ts,
                &frame.body,
                "sent",
                &frame.id,
            ));
        }
        self.persist_seqs(contact_pk);
        if let Err(e) = self
            .tunnel
            .send_data(cid, &unspecified(), &unspecified(), &wire)
            .await
        {
            // Le circuit est mort entre-temps : delier pour que le
            // prochain envoi retente une liaison ; le message passe
            // `failed` (visible — pas de file).
            if kind == MsgKind::Msg {
                self.set_msg_status(&frame.id, "failed");
                let _ = self.events_tx.send(MessagingEvent::Undeliverable {
                    contact: contact_pk.to_vec(),
                    id: frame.id,
                });
            }
            self.unbind_circuit(cid);
            return Err(CoreError::State(format!("send_data messagerie: {e}")));
        }
        if kind == MsgKind::Hello {
            if let Some(c) = self.contacts.lock().unwrap().get_mut(contact_pk) {
                c.greeted = true;
            }
        }
        Ok(frame.id)
    }

    /// Relais `e2e_ready` : demultiplexe les liaisons par swarm —
    /// `own_mh` = repondant (contact a identifier), swarm contact =
    /// initiateur (contact connu). Les swarms BitTorrent sont
    /// ignores (listener dedie dans `ipv8_stack`).
    fn spawn_e2e_listener(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops.lock().unwrap().push(tx);
        let mut e2e = self.tunnel.e2e_ready();
        let svc = self.clone();
        tokio::spawn(async move {
            loop {
                let (cid, lookup) = tokio::select! {
                    _ = rx_stop.changed() => break,
                    ev = e2e.recv() => match ev {
                        Ok(v) => v,
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    },
                };
                svc.on_e2e_ready(cid, lookup);
            }
        });
    }

    /// Liaison e2e vers un swarm messagerie : derive les cles
    /// applicatives, abonne les donnees du circuit et lance la
    /// reception.
    fn on_e2e_ready(self: &Arc<Self>, cid: u32, lookup: [u8; 20]) {
        // Ctype -> role : `RP_DOWNLOADER` = initiateur du lien e2e.
        let ctype = self
            .tunnel
            .circuits_info()
            .into_iter()
            .find(|c| c.circuit_id == cid)
            .map(|c| c.ctype)
            .unwrap_or_default();
        let (initiator, contact) = if lookup == self.own_mh {
            // Un pair nous contacte : toujours repondant.
            (false, None)
        } else if let Some(pk_bin) = self.swarms.lock().unwrap().get(&lookup) {
            (
                ctype == onionbit_tunnel::routing::CIRCUIT_TYPE_RP_DOWNLOADER,
                Some(pk_bin.clone()),
            )
        } else {
            // Swarm non messagerie (BitTorrent ou autre) : pas pour nous.
            return;
        };
        let Some(shared) = self.tunnel.e2e_shared_secret(cid) else {
            return;
        };
        let Ok(keys) = derive_messaging_keys(&shared, initiator) else {
            return;
        };
        self.circuits.lock().unwrap().insert(
            cid,
            CircuitBinding {
                keys: keys.clone(),
                contact: contact.clone(),
            },
        );
        match contact {
            Some(pk_bin) => {
                if let Some(c) = self.contacts.lock().unwrap().get_mut(&pk_bin) {
                    c.circuit = Some(cid);
                }
                let _ = self.events_tx.send(MessagingEvent::Bound {
                    contact: pk_bin,
                    circuit_id: cid,
                });
            }
            None => {
                let _ = self
                    .events_tx
                    .send(MessagingEvent::Pending { circuit_id: cid });
            }
        }
        // Reception des trames sur ce circuit (abonne dedie — le
        // subscriber BitTorrent ne s'abonne jamais a nos circuits).
        let rx = self.tunnel.subscribe_circuit_data(cid);
        let svc = self.clone();
        tokio::spawn(async move {
            svc.recv_circuit(cid, keys, rx).await;
        });
    }

    /// Boucle de reception d'un circuit : parse chaque `data`,
    /// identifie l'emetteur si besoin, verifie, anti-replay, livre.
    async fn recv_circuit(
        self: &Arc<Self>,
        cid: u32,
        keys: MessagingKeys,
        mut rx: tokio::sync::mpsc::Receiver<onionbit_tunnel::community::CircuitData>,
    ) {
        while let Some(msg) = rx.recv().await {
            self.handle_incoming(cid, &keys, &msg.data);
        }
        // Canal ferme = circuit detruit : nettoyer la liaison.
        self.unbind_circuit(cid);
    }

    /// Delie un circuit (mort ou erreur d'emission) sans toucher la
    /// fenetre anti-replay du contact — elle survit a la
    /// reouverture (MS-2).
    fn unbind_circuit(&self, cid: u32) {
        let contact = self
            .circuits
            .lock()
            .unwrap()
            .remove(&cid)
            .and_then(|b| b.contact);
        if let Some(pk_bin) = contact {
            if let Some(c) = self.contacts.lock().unwrap().get_mut(&pk_bin) {
                if c.circuit == Some(cid) {
                    c.circuit = None;
                }
            }
        }
    }

    /// Codec + identification + verification + consentement +
    /// budgets + anti-replay d'une trame entrante. Ne panique
    /// jamais ; toute trame hostile est ecartee en silence (debug).
    ///
    /// Ordre des barrieres (anti-DoS, MS-10) : taille+`v` avant tout
    /// parse ([`preflight`]) -> seau global -> codec+AEAD ->
    /// identification/signature -> etat de consentement -> seau
    /// du contact -> anti-replay.
    fn handle_incoming(self: &Arc<Self>, cid: u32, keys: &MessagingKeys, data: &[u8]) {
        if let Err(e) = preflight(data, &self.cfg) {
            self.stats.codec.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(circuit_id = cid, error = %e, "trame messagerie rejetee au prefiltre");
            return;
        }
        if let Some(b) = self.global_bucket.lock().unwrap().as_mut() {
            if !b.take() {
                self.stats.rate_global.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        let raw = match RawFrame::parse(data, &keys.recv, &self.cfg) {
            Ok(r) => r,
            Err(e) => {
                self.stats.codec.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(circuit_id = cid, error = %e, "trame messagerie rejetee au codec");
                return;
            }
        };
        let bound = self
            .circuits
            .lock()
            .unwrap()
            .get(&cid)
            .and_then(|b| b.contact.clone());
        let pk_bin = match bound {
            // Initiateur (ou repondant deja resolu) : une seule cle.
            Some(pk_bin) => {
                let Some(Ok(_)) = self.verify_against(&pk_bin, &raw) else {
                    tracing::debug!(circuit_id = cid, "signature de trame invalide");
                    return;
                };
                pk_bin
            }
            // Repondant non resolu : `hello` declare l'emetteur
            // (corps = sa `pk_bin`), sinon balayage des contacts
            // connus (reouverture de circuit par un contact existant).
            None => match self.identify(&raw) {
                Some(pk) => {
                    if !self.admit_inbound(cid, &pk, keys) {
                        return;
                    }
                    pk
                }
                None => {
                    tracing::debug!(
                        circuit_id = cid,
                        "trame d'emetteur non identifiable, ignoree"
                    );
                    return;
                }
            },
        };
        // Barriere de consentement : `pending` n'est jamais livre,
        // `blocked` ne devrait pas arriver (circuit detruit a la
        // liaison) — garde-fou si la trame precedait le blocage.
        // `hello` n'est jamais livre non plus (trame de controle).
        let state = self.contact_state(&pk_bin);
        match state {
            Some(ContactState::Active) => {}
            Some(ContactState::Pending) => {
                self.stats.pending_drop.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Some(ContactState::Blocked) => {
                self.stats.blocked.fetch_add(1, Ordering::Relaxed);
                return;
            }
            None => return,
        }
        if raw.kind == MsgKind::Hello {
            return;
        }
        // Dedup `id` d'abord (gratuite — une re-emission honnete ne
        // consomme pas de jeton), puis seau du contact, puis la
        // fenetre `seq` : une trame ecartee au budget n'est PAS
        // admise et sa re-emission ulterieure peut etre livree.
        let admitted = {
            let mut contacts = self.contacts.lock().unwrap();
            match contacts.get_mut(&pk_bin) {
                None => false,
                Some(c) => {
                    if c.recv_window.seen_id(&raw.id) {
                        self.stats.replay.fetch_add(1, Ordering::Relaxed);
                        false
                    } else if raw.kind != MsgKind::Ack
                        && c.bucket.as_mut().is_some_and(|b| !b.take())
                    {
                        // `ack` exempte du seau : trame de controle
                        // verifiee/dedupe — elle ne doit pas faire
                        // perdre de `msg` sous rafale (le seau global
                        // borne toujours son cout codec+signature).
                        self.stats.rate_contact.fetch_add(1, Ordering::Relaxed);
                        false
                    } else {
                        match c.recv_window.admit(raw.seq, &raw.id) {
                            Ok(()) => true,
                            Err(e) => {
                                self.stats.replay.fetch_add(1, Ordering::Relaxed);
                                tracing::debug!(circuit_id = cid, error = %e, "trame messagerie rejetee (anti-replay)");
                                false
                            }
                        }
                    }
                }
            }
        };
        if admitted {
            self.persist_seqs(&pk_bin);
            match raw.kind {
                // ACK applicatif : `body` = `id` de la trame
                // acquittee -> statut `acked` de notre `out`.
                MsgKind::Ack => {
                    if let Ok(id) = <[u8; 16]>::try_from(raw.body.as_slice()) {
                        self.set_msg_status(&id, "acked");
                    }
                }
                // Message : historique `received` + ACK applicatif
                // en retour (`body` = `id` de cette trame).
                MsgKind::Msg => {
                    self.persist_message(Self::msg_row(
                        &pk_bin, "in", raw.seq, raw.ts, &raw.body, "received", &raw.id,
                    ));
                    let svc = self.clone();
                    let pk = pk_bin.clone();
                    let ack_id = raw.id;
                    tokio::spawn(async move {
                        let _ = svc
                            .send_frame(&pk, cid, MsgKind::Ack, ack_id.to_vec())
                            .await;
                    });
                }
                _ => {}
            }
            let _ = self.events_tx.send(MessagingEvent::Frame {
                contact: pk_bin,
                kind: raw.kind,
                id: raw.id,
                body: raw.body,
            });
        }
    }

    /// Admission d'un emetteur identifie sur un circuit non lie :
    /// lie le circuit au contact et tranche selon le consentement —
    /// `blocked` detruit le circuit, inconnu entre en `pending`
    /// borne (sinon drop), `Active`/`Pending` se lient. Retourne
    /// `true` si la trame peut poursuivre vers la livraison.
    fn admit_inbound(&self, cid: u32, pk_bin: &[u8], keys: &MessagingKeys) -> bool {
        match self.contact_state(pk_bin) {
            Some(ContactState::Blocked) => {
                self.stats.blocked.fetch_add(1, Ordering::Relaxed);
                self.unbind_circuit(cid);
                let t = self.tunnel.clone();
                tokio::spawn(async move {
                    t.remove_circuit(cid, "contact bloque").await;
                });
                false
            }
            Some(ContactState::Pending) | Some(ContactState::Active) => {
                self.bind_existing(cid, pk_bin, keys);
                true
            }
            None => {
                // Inconnu : admission `pending` bornee (capacite +
                // TTL purges d'abord).
                self.purge_expired_pending();
                let full = self
                    .contacts
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|c| c.state == ContactState::Pending)
                    .count()
                    >= self.cfg.pending_cap;
                if full {
                    self.stats.pending_full.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) else {
                    return false;
                };
                {
                    let mut contacts = self.contacts.lock().unwrap();
                    let mut c = Contact::pending(pk, &self.cfg);
                    c.circuit = Some(cid);
                    contacts.insert(pk_bin.to_vec(), c);
                }
                self.persist_contact(pk_bin, ContactState::Pending);
                self.bind_circuit(cid, pk_bin, keys);
                let _ = self.events_tx.send(MessagingEvent::Consent {
                    contact: pk_bin.to_vec(),
                    circuit_id: cid,
                });
                false
            }
        }
    }

    /// Lie le circuit a un contact existant (reouverture ou
    /// re-pending) — le `hello` sera re-emis : `greeted` rearme.
    fn bind_existing(&self, cid: u32, pk_bin: &[u8], keys: &MessagingKeys) {
        self.bind_circuit(cid, pk_bin, keys);
        if let Some(c) = self.contacts.lock().unwrap().get_mut(pk_bin) {
            c.circuit = Some(cid);
            c.greeted = false;
        }
    }

    /// Pose/met a jour la liaison du circuit (creee si absente —
    /// `unbind_circuit` a pu la retirer apres une erreur d'emission).
    fn bind_circuit(&self, cid: u32, pk_bin: &[u8], keys: &MessagingKeys) {
        self.circuits
            .lock()
            .unwrap()
            .entry(cid)
            .and_modify(|b| b.contact = Some(pk_bin.to_vec()))
            .or_insert_with(|| CircuitBinding {
                keys: keys.clone(),
                contact: Some(pk_bin.to_vec()),
            });
    }

    /// Verifie la trame contre la cle du contact `pk_bin`.
    fn verify_against(
        &self,
        pk_bin: &[u8],
        raw: &RawFrame,
    ) -> Option<std::result::Result<Frame, MessagingError>> {
        self.contacts
            .lock()
            .unwrap()
            .get(pk_bin)
            .map(|c| raw.verify(&c.pk))
    }

    /// Identifie l'emetteur d'une trame sur un circuit non lie :
    /// `hello` (corps = `pk_bin` declare, verifie par signature)
    /// d'abord, sinon chaque contact connu hors `blocked` jusqu'a
    /// la premiere verification reussie. Une `pk` `blocked`
    /// identifiee remonte tout de meme — `admit_inbound` detruit
    /// le circuit (l'acceptation e2e est desarmee).
    fn identify(&self, raw: &RawFrame) -> Option<Vec<u8>> {
        if raw.kind == MsgKind::Hello {
            if let Ok(pk) = LibNaClPublicKey::from_bin(&raw.body) {
                if raw.verify(&pk).is_ok() {
                    return Some(pk.to_bin());
                }
            }
            return None;
        }
        let pks: Vec<Vec<u8>> = self.contacts.lock().unwrap().keys().cloned().collect();
        pks.into_iter().find(|pk_bin| {
            self.contact_state(pk_bin) != Some(ContactState::Blocked)
                && matches!(self.verify_against(pk_bin, raw), Some(Ok(_)))
        })
    }

    /// Presence : maintient des points d'introduction sur notre
    /// swarm a `ip_check_interval` (rapide — un premier essai peut
    /// echouer tant que les pairs ne sont pas verifies) et re-publie
    /// l'annonce DHT a `announce_interval` (pendant de
    /// `ensure_introduction_points`+`reannounce` du moniteur de
    /// swarms BitTorrent).
    fn spawn_presence_monitor(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops.lock().unwrap().push(tx);
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(svc.cfg.ip_check_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Reannonce immediate au demarrage puis a cadence lente.
            let mut next_announce = std::time::Instant::now();
            loop {
                tokio::select! {
                    _ = rx_stop.changed() => break,
                    _ = tick.tick() => {}
                }
                crate::ipv8_stack::ensure_introduction_points(&svc.tunnel, svc.own_mh);
                svc.purge_expired_pending();
                // Retention : purge des messages expires (DELETE
                // reel, `secure_delete` zeroise le corps avant).
                if let Some(db) = &svc.db {
                    let now = now_secs() as i64;
                    if let Err(e) = db.with(|c| dbm::prune_expired(c, now)) {
                        tracing::warn!(error = %e, "purge retention messagerie");
                    }
                }
                if std::time::Instant::now() >= next_announce {
                    next_announce = std::time::Instant::now() + svc.cfg.announce_interval;
                    let t = svc.tunnel.clone();
                    let mh = svc.own_mh;
                    tokio::spawn(async move {
                        t.reannounce_intro_points(mh).await;
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_ipv8::endpoint::UdpEndpoint;
    use onionbit_ipv8::peer::Network;
    use onionbit_tunnel::settings::TunnelSettings;
    use onionbit_tunnel::TUNNEL_COMMUNITY_ID;

    /// Service de test sur un tunnel loopback reel (le protocole ne
    /// depend pas du transport : les liaisons sont injectees a la
    /// main et `handle_incoming` est pilote directement).
    async fn make_service(hops: usize) -> (Arc<MessagingService>, LibNaClSecretKey) {
        let (tunnel, key) = bare_tunnel().await;
        (
            MessagingService::start(tunnel, key.clone(), MessagingConfig::default(), hops, None),
            key,
        )
    }

    /// Tunnel loopback de test (sans service — pour `start` manuel).
    async fn bare_tunnel() -> (Arc<TunnelCommunity>, LibNaClSecretKey) {
        let key = LibNaClSecretKey::generate();
        let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_run = ep.clone();
        tokio::spawn(async move {
            let _ = ep_run.run().await;
        });
        let tunnel = TunnelCommunity::new_with_id(
            key.clone(),
            Arc::new(Network::default()),
            ep,
            TunnelSettings {
                max_intro_points: 0,
                ..TunnelSettings::default()
            },
            TUNNEL_COMMUNITY_ID,
        )
        .await;
        (tunnel, key)
    }

    /// `make_service` avec une `MessagingConfig` explicite.
    async fn make_service_cfg(cfg: MessagingConfig) -> Arc<MessagingService> {
        let (tunnel, key) = bare_tunnel().await;
        MessagingService::start(tunnel, key, cfg, 0, None)
    }

    /// `make_service` sur une `Database` memoire (persistance).
    async fn make_service_db(db: Arc<Database>) -> Arc<MessagingService> {
        let (tunnel, key) = bare_tunnel().await;
        MessagingService::start(tunnel, key, MessagingConfig::default(), 0, Some(db))
    }

    /// Injecte un circuit non lie (`contact: None`) — le repondant
    /// avant identification.
    fn unbound(svc: &MessagingService, cid: u32, keys: &MessagingKeys) {
        svc.circuits.lock().unwrap().insert(
            cid,
            CircuitBinding {
                keys: keys.clone(),
                contact: None,
            },
        );
    }

    /// `hello` filaire d'un pair (corps = sa `pk_bin`).
    fn hello_wire(peer: &LibNaClSecretKey, seq: u64, key: &[u8; 32]) -> Vec<u8> {
        Frame::new(MsgKind::Hello, seq, 1, peer.public_key().to_bin())
            .seal(peer, key, &MessagingConfig::default())
            .unwrap()
    }

    /// Injecte une liaison + un contact connus et retourne les cles.
    fn bind(
        svc: &MessagingService,
        cid: u32,
        peer: &LibNaClSecretKey,
        keys: &MessagingKeys,
    ) -> Vec<u8> {
        let pk_bin = peer.public_key().to_bin();
        svc.circuits.lock().unwrap().insert(
            cid,
            CircuitBinding {
                keys: keys.clone(),
                contact: Some(pk_bin.clone()),
            },
        );
        let mut c = Contact::active(peer.public_key(), &svc.cfg);
        c.circuit = Some(cid);
        c.greeted = true;
        svc.contacts.lock().unwrap().insert(pk_bin.clone(), c);
        // La FK `msg_messages.contact_pk` exige la ligne contact.
        svc.persist_contact(&pk_bin, ContactState::Active);
        pk_bin
    }

    fn wire(peer: &LibNaClSecretKey, seq: u64, key: &[u8; 32], body: &[u8]) -> Vec<u8> {
        Frame::new(MsgKind::Msg, seq, 1, body.to_vec())
            .seal(peer, key, &MessagingConfig::default())
            .unwrap()
    }

    /// Re-emission honnete a travers une reouverture de circuit
    /// (MS-2) : la trame livree sur le premier circuit est absorbee
    /// par la dedup quand elle revient sur le circuit ouvert en
    /// remplacement — la fenetre de reception vit sur le contact,
    /// pas sur le circuit.
    #[tokio::test]
    async fn dedup_survive_a_la_reouverture_du_circuit() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [7u8; 32],
            recv: [8u8; 32],
        };
        let pk_bin = bind(&svc, 11, &peer, &keys);
        let mut events = svc.subscribe();

        let w = wire(&peer, 0, &keys.recv, b"perdu en vol");
        svc.handle_incoming(11, &keys, &w);
        assert!(events.try_recv().is_ok(), "premiere livraison -> evenement");

        // Reouverture : un nouveau circuit se lie au meme contact,
        // l'ancien est oublie.
        svc.circuits.lock().unwrap().insert(
            12,
            CircuitBinding {
                keys: keys.clone(),
                contact: Some(pk_bin.clone()),
            },
        );
        svc.contacts
            .lock()
            .unwrap()
            .get_mut(&pk_bin)
            .expect("contact")
            .circuit = Some(12);
        svc.circuits.lock().unwrap().remove(&11);

        // La meme trame, relivree sur le circuit de remplacement :
        // absorbee par la dedup (fenetre attachee au contact).
        svc.handle_incoming(12, &keys, &w);
        assert!(
            events.try_recv().is_err(),
            "re-emission sur nouveau circuit -> aucun evenement"
        );
    }

    /// MS-9 — les trames messagerie ne ressemblent jamais a du uTP :
    /// meme si une trame s'egarait vers la lane BitTorrent, le filtre
    /// `could_be_utp` de `spawn_e2e_listener` l'ecarterait avant
    /// l'injection `TunnelUdpSocket`. Verifie la forme wire de chaque
    /// type de trame.
    #[tokio::test]
    async fn trames_jamais_acceptees_par_le_filtre_utp() {
        let peer = LibNaClSecretKey::generate();
        let key = [9u8; 32];
        let cfg = MessagingConfig::default();
        for kind in [MsgKind::Hello, MsgKind::Msg, MsgKind::Ack] {
            let wire = Frame::new(kind, 0, 1, b"corps".to_vec())
                .seal(&peer, &key, &cfg)
                .expect("seal");
            assert!(
                !onionbit_network_policy::exit_policy::could_be_utp(&wire),
                "trame {kind:?} acceptee comme uTP"
            );
        }
        // Le filtre rejette aussi les octets bruts bencode.
        assert!(!onionbit_network_policy::exit_policy::could_be_utp(
            b"d1:v i1ee"
        ));
    }

    /// Deduplication : la meme trame relivree (re-emission honnete,
    /// MS-2) ne produit qu'un seul evenement ; un `seq` rejoue sous
    /// un `id` neuf est rejete par la fenetre.
    #[tokio::test]
    async fn dedup_et_rejeu_au_niveau_service() {
        // Budget desactive : le test isole la dedup/fenetre (la
        // limitation de debit a son banc dedie).
        let svc = make_service_cfg(MessagingConfig {
            per_contact_rate: 0,
            global_rate: 0,
            ..Default::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        bind(&svc, 42, &peer, &keys);
        let mut events = svc.subscribe();

        // La meme trame wire deux fois -> un seul evenement.
        let w = wire(&peer, 0, &keys.recv, b"premier");
        svc.handle_incoming(42, &keys, &w);
        svc.handle_incoming(42, &keys, &w);
        let ev = events.try_recv().expect("un evenement attendu");
        assert!(matches!(ev, MessagingEvent::Frame { .. }));
        assert!(
            events.try_recv().is_err(),
            "dedup : pas de second evenement"
        );

        // `seq` rejoue sous un `id` neuf -> rejete par la fenetre.
        let w2 = wire(&peer, 0, &keys.recv, b"rejeu");
        svc.handle_incoming(42, &keys, &w2);
        assert!(events.try_recv().is_err(), "seq rejoue -> aucun evenement");

        // `seq` suivant valide -> admis.
        let w3 = wire(&peer, 1, &keys.recv, b"second");
        svc.handle_incoming(42, &keys, &w3);
        assert!(events.try_recv().is_ok(), "seq suivant -> evenement");
    }

    /// Un `hello` sur un circuit non lie identifie l'emetteur :
    /// corps = `pk_bin`, signature verifiee contre elle, puis le
    /// circuit se lie a ce contact.
    #[tokio::test]
    async fn hello_identifie_l_emetteur_sur_circuit_non_lie() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [3u8; 32],
            recv: [4u8; 32],
        };
        svc.circuits.lock().unwrap().insert(
            7,
            CircuitBinding {
                keys: keys.clone(),
                contact: None,
            },
        );
        let hello = Frame::new(MsgKind::Hello, 0, 1, peer.public_key().to_bin())
            .seal(&peer, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(7, &keys, &hello);

        // Le circuit s'est lie a la cle du peer.
        let bound = svc
            .bound_contacts()
            .into_iter()
            .find(|(_, c)| *c == Some(7));
        assert!(bound.is_some(), "hello verifie -> contact lie au circuit");

        // Un `hello` signe par une AUTRE cle que celle declaree est
        // rejete (usurpation du corps).
        let liar = LibNaClSecretKey::generate();
        svc.circuits.lock().unwrap().insert(
            9,
            CircuitBinding {
                keys: keys.clone(),
                contact: None,
            },
        );
        let forged = Frame::new(MsgKind::Hello, 0, 1, peer.public_key().to_bin())
            .seal(&liar, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(9, &keys, &forged);
        assert!(
            svc.bound_contacts().iter().all(|(_, c)| *c != Some(9)),
            "hello usurpe -> pas de liaison"
        );
    }

    /// Les trames malformees ou non verifiables sont ecartees sans
    /// evenement ni panic (surface hostile — MS-3).
    #[tokio::test]
    async fn trames_hostiles_ecartees_sans_panic() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [5u8; 32],
            recv: [6u8; 32],
        };
        bind(&svc, 5, &peer, &keys);
        let mut events = svc.subscribe();
        for data in [
            &b""[..],
            b"pas du bencode",
            &[0u8; 64][..],
            b"d1:v i1ee",
            &vec![0xFF; 40_000][..],
        ] {
            svc.handle_incoming(5, &keys, data);
        }
        assert!(events.try_recv().is_err());
    }

    /// MS-6 — cycle de consentement : `hello` d'un inconnu ->
    /// `Consent` + `Pending` ; ses `msg` sont verifies puis ecartes
    /// (`pending_drop`) ; `send` refuse ; `accept_contact` ->
    /// `Active` + `Bound` + livraison.
    #[tokio::test]
    async fn consentement_pending_puis_accept() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = peer.public_key().to_bin();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        unbound(&svc, 20, &keys);
        let mut events = svc.subscribe();

        svc.handle_incoming(20, &keys, &hello_wire(&peer, 0, &keys.recv));
        let ev = events.try_recv().expect("Consent attendu");
        assert!(matches!(ev, MessagingEvent::Consent { ref contact, .. } if *contact == pk_bin));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Pending));

        // `msg` d'un pending : verifie mais jamais livre.
        let w = wire(&peer, 1, &keys.recv, b"avant accord");
        svc.handle_incoming(20, &keys, &w);
        assert!(events.try_recv().is_err());
        assert_eq!(svc.stats.pending_drop.load(Ordering::Relaxed), 1);

        // `send` refuse tant que le consentement n'est pas donne.
        assert!(svc.send(&pk_bin, b"x".to_vec()).await.is_err());

        // Acceptation : Active + Bound ; les trames passent.
        svc.accept_contact(&pk_bin).await.unwrap();
        let ev = events.try_recv().expect("Bound attendu");
        assert!(matches!(ev, MessagingEvent::Bound { .. }));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Active));
        let w2 = wire(&peer, 2, &keys.recv, b"apres accord");
        svc.handle_incoming(20, &keys, &w2);
        assert!(events.try_recv().is_ok(), "trame post-accept livree");
    }

    /// MS-6 — `refuse_contact` oublie le contact (un futur `hello`
    /// le remet en `pending`) ; `block_contact` persiste : trames
    /// comptees `blocked`, liaison/circuit detruits, swarm desarme.
    #[tokio::test]
    async fn consentement_refus_puis_blocage() {
        let (svc, _) = make_service(0).await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = peer.public_key().to_bin();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        unbound(&svc, 30, &keys);

        svc.handle_incoming(30, &keys, &hello_wire(&peer, 0, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Pending));
        svc.refuse_contact(&pk_bin).await.unwrap();
        assert_eq!(svc.contact_state(&pk_bin), None, "refus -> oublie");

        // Un nouveau `hello` repropose le consentement.
        unbound(&svc, 31, &keys);
        svc.handle_incoming(31, &keys, &hello_wire(&peer, 1, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Pending));

        // Blocage : trames comptees, circuit detruit a
        // l'identification, swarm non joint.
        svc.block_contact(&pk_bin).await.unwrap();
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Blocked));
        assert!(svc.resolve(&pk_bin).await.is_err(), "resolve refuse");
        assert!(svc.connect(&pk_bin, &intro()).await.is_err());
        unbound(&svc, 32, &keys);
        svc.handle_incoming(32, &keys, &hello_wire(&peer, 2, &keys.recv));
        assert_eq!(svc.stats.blocked.load(Ordering::Relaxed), 1);
        assert!(
            !svc.circuits.lock().unwrap().contains_key(&32),
            "circuit du bloque delie"
        );
        // `ensure_contact_swarm` ne joint pas le swarm d'un bloque.
        let mh = messaging_hash(&peer.public_key());
        let _ = svc.ensure_contact_swarm(&peer.public_key(), &pk_bin);
        assert!(!svc.swarms.lock().unwrap().contains_key(&mh));

        svc.unblock_contact(&pk_bin).await.unwrap();
        assert_eq!(svc.contact_state(&pk_bin), None);
    }

    /// MS-6 — `pending` borne : au-dela de `pending_cap`, les `hello`
    /// sont ecartes (`pending_full` compte) ; le TTL purge les
    /// consentements expires.
    #[tokio::test]
    async fn pending_borne_et_ttl() {
        let cfg = MessagingConfig {
            pending_cap: 2,
            global_rate: 0,
            per_contact_rate: 0,
            ..Default::default()
        };
        let svc = make_service_cfg(cfg).await;
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let p1 = LibNaClSecretKey::generate();
        let p2 = LibNaClSecretKey::generate();
        let p3 = LibNaClSecretKey::generate();
        unbound(&svc, 40, &keys);
        unbound(&svc, 41, &keys);
        unbound(&svc, 42, &keys);
        svc.handle_incoming(40, &keys, &hello_wire(&p1, 0, &keys.recv));
        svc.handle_incoming(41, &keys, &hello_wire(&p2, 0, &keys.recv));
        assert_eq!(svc.pending_contacts().len(), 2);
        // Capacite pleine : le troisieme `hello` est ecarte.
        svc.handle_incoming(42, &keys, &hello_wire(&p3, 0, &keys.recv));
        assert_eq!(svc.stats.pending_full.load(Ordering::Relaxed), 1);
        assert_eq!(svc.pending_contacts().len(), 2);
        assert_eq!(svc.contact_state(&p3.public_key().to_bin()), None);

        // TTL : un `pending` vieilli est purge — la place se libere.
        let p1_bin = p1.public_key().to_bin();
        svc.contacts
            .lock()
            .unwrap()
            .get_mut(&p1_bin)
            .expect("contact")
            .pending_since = 0;
        svc.purge_expired_pending();
        assert_eq!(svc.pending_contacts().len(), 1);
        unbound(&svc, 43, &keys);
        svc.handle_incoming(43, &keys, &hello_wire(&p3, 0, &keys.recv));
        assert_eq!(
            svc.contact_state(&p3.public_key().to_bin()),
            Some(ContactState::Pending)
        );
    }

    /// MS-10 — budgets : le seau par contact ecarte au-dela du
    /// debit configure ; le seau global borne toutes les trames
    /// confondues.
    #[tokio::test]
    async fn budgets_contact_et_global() {
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        // Seau contact = 2/s : la troisieme trame est ecartee.
        let svc = make_service_cfg(MessagingConfig {
            per_contact_rate: 2,
            global_rate: 0,
            ..Default::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        bind(&svc, 50, &peer, &keys);
        let mut events = svc.subscribe();
        for seq in 0..3 {
            let w = wire(&peer, seq, &keys.recv, b"rafale");
            svc.handle_incoming(50, &keys, &w);
        }
        let mut n = 0;
        while events.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 2, "2 trames livrees, la 3e au budget");
        assert_eq!(svc.stats.rate_contact.load(Ordering::Relaxed), 1);

        // Seau global = 1/s : tout au-dela est ecarte avant le codec.
        let svc = make_service_cfg(MessagingConfig {
            global_rate: 1,
            per_contact_rate: 0,
            ..Default::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        bind(&svc, 60, &peer, &keys);
        for seq in 0..3 {
            let w = wire(&peer, seq, &keys.recv, b"rafale");
            svc.handle_incoming(60, &keys, &w);
        }
        assert_eq!(svc.stats.rate_global.load(Ordering::Relaxed), 2);
    }

    /// Prefiltre : taille et version sont verifiees AVANT le parse
    /// bencode — une trame valide passe, `v != 1` et le non-bencode
    /// sont ecartes a cout constant.
    #[test]
    fn prefiltre_taille_et_version() {
        let cfg = MessagingConfig::default();
        assert!(preflight(b"", &cfg).is_err());
        assert!(preflight(b"pas du bencode", &cfg).is_err());
        assert!(preflight(&vec![0u8; cfg.max_frame_len + 1], &cfg).is_err());
        // `v=2` : suffixe present mais version refusee tot.
        let bad = b"d4:body0:2:id16:0123456789abcdef3:seqi0e3:sig64:00000000000000000000000000000000000000000000000000000000000000002:tsi1e4:type3:msg1:vi2ee";
        assert!(matches!(
            preflight(bad, &cfg),
            Err(MessagingError::UnknownVersion(2))
        ));
        // Trame valide : le prefiltre laisse passer.
        let peer = LibNaClSecretKey::generate();
        let w = wire(&peer, 0, &[1u8; 32], b"ok");
        preflight(&w, &cfg).expect("trame valide au prefiltre");
    }

    /// MS-7 — offline borne et visible : `send` sans circuit ->
    /// erreur + evenement `Undeliverable` + ligne `failed` dans
    /// l'historique (pas de file, pas de re-emission).
    #[tokio::test]
    async fn offline_undeliverable_visible_en_historique() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db.clone()).await;
        let peer = LibNaClSecretKey::generate();
        // Contact Active sans circuit lie (offline).
        let pk_bin = bind(
            &svc,
            0,
            &peer,
            &MessagingKeys {
                send: [1u8; 32],
                recv: [2u8; 32],
            },
        );
        svc.contacts
            .lock()
            .unwrap()
            .get_mut(&pk_bin)
            .expect("contact")
            .circuit = None;
        svc.circuits.lock().unwrap().remove(&0);
        let mut events = svc.subscribe();

        assert!(svc.send(&pk_bin, b"perdu".to_vec()).await.is_err());
        let ev = events.try_recv().expect("Undeliverable attendu");
        let id = match ev {
            MessagingEvent::Undeliverable { contact, id } => {
                assert_eq!(contact, pk_bin);
                id
            }
            _ => panic!("Undeliverable attendu, recu {ev:?}"),
        };
        let hist = svc.history(&pk_bin, 10).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].status, "failed");
        assert_eq!(hist[0].id, id.to_vec());
    }

    /// MS-11 — restart : etat de consentement, `send_seq` et
    /// `recv_top` restaures — un `seq` rejoue sous `recv_top` est
    /// refuse par la fenetre reprise (conservateur).
    #[tokio::test]
    async fn restart_restaure_contacts_et_seqs() {
        let db = Arc::new(Database::memory().unwrap());
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = {
            let svc = make_service_db(db.clone()).await;
            let pk_bin = bind(&svc, 7, &peer, &keys);
            // Livre une trame entrante : `recv_top` + historique.
            let w = wire(&peer, 5, &keys.recv, b"avant restart");
            svc.handle_incoming(7, &keys, &w);
            // Un `seq` sortant persiste via `persist_seqs`.
            {
                let mut contacts = svc.contacts.lock().unwrap();
                contacts.get_mut(&pk_bin).expect("contact").send_seq = 3;
            }
            svc.persist_seqs(&pk_bin);
            svc.persist_contact(&pk_bin, ContactState::Active);
            pk_bin
        };
        // Nouveau service sur la meme base : restaure l'etat.
        let svc2 = make_service_db(db).await;
        assert_eq!(
            svc2.contact_state(&pk_bin),
            Some(ContactState::Active),
            "contact restaure"
        );
        assert_eq!(svc2.history(&pk_bin, 10).unwrap().len(), 1);
        unbound(&svc2, 9, &keys);
        // `seq <= recv_top` hors dedup (cache vide au restart) ->
        // refuse par la fenetre reprise.
        let old = wire(&peer, 5, &keys.recv, b"rejeu");
        svc2.handle_incoming(9, &keys, &old);
        assert_eq!(svc2.stats.replay.load(Ordering::Relaxed), 1);
        // `seq` neuf : admis (historique -> 2 lignes entrantes).
        let new = wire(&peer, 6, &keys.recv, b"apres restart");
        svc2.handle_incoming(9, &keys, &new);
        let hist = svc2.history(&pk_bin, 10).unwrap();
        assert_eq!(hist.len(), 2, "trame post-restart persistee");
    }

    /// MS-11 — ACK applicatif : un `ack` entrant passe le `out`
    /// correspondant a `acked` ; un `msg` entrant est persiste
    /// `received`.
    #[tokio::test]
    async fn ack_applicatif_et_historique() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = bind(&svc, 11, &peer, &keys);

        // `msg` entrant -> ligne `received`.
        let w = wire(&peer, 0, &keys.recv, b"recu");
        svc.handle_incoming(11, &keys, &w);
        let hist = svc.history(&pk_bin, 10).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].status, "received");
        assert_eq!(hist[0].direction, "in");

        // `ack` entrant -> notre `out` passe `acked`.
        let our_id = [7u8; 16];
        svc.persist_message(MessagingService::msg_row(
            &pk_bin, "out", 0, 1, b"emis", "sent", &our_id,
        ));
        let ack = Frame::new(MsgKind::Ack, 1, 1, our_id.to_vec())
            .seal(&peer, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(11, &keys, &ack);
        let hist = svc.history(&pk_bin, 10).unwrap();
        let out = hist.iter().find(|m| m.direction == "out").unwrap();
        assert_eq!(out.status, "acked");
    }

    /// MS-11 — retention : expiration -> `DELETE` reel (le corps
    /// est zeroise avant en `secure_delete` — verifie via la purge
    /// DB directe) ; `delete_message` supprime sans marqueur.
    #[tokio::test]
    async fn retention_et_suppression_reelle() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db.clone()).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = bind(&svc, 13, &peer, &keys);
        svc.persist_contact(&pk_bin, ContactState::Active);
        svc.set_retention(&pk_bin, 60, true).unwrap();

        // Message date -> insere avec `created_at` ancien en DB.
        svc.persist_message(MessagingService::msg_row(
            &pk_bin, "in", 0, 1, b"vieux", "received", &[1u8; 16],
        ));
        svc.persist_message(MessagingService::msg_row(
            &pk_bin, "in", 1, 1, b"neuf", "received", &[2u8; 16],
        ));
        // Vieillit artificiellement le premier message.
        db.with(|c| {
            c.execute(
                "UPDATE msg_messages SET created_at=1 WHERE id=?1",
                rusqlite::params![[1u8; 16]],
            )?;
            Ok(())
        })
        .unwrap();
        let now = now_secs() as i64;
        db.with(|c| dbm::prune_expired(c, now)).unwrap();
        let hist = svc.history(&pk_bin, 10).unwrap();
        assert_eq!(hist.len(), 1, "le message expire est supprime");
        assert_eq!(hist[0].id, vec![2u8; 16]);

        // Suppression explicite reelle.
        svc.delete_message(&[2u8; 16]).unwrap();
        assert!(svc.history(&pk_bin, 10).unwrap().is_empty());
    }

    /// `IntroductionPoint` factice pour `connect` (jamais atteint —
    /// le garde-fou `blocked` precede toute emission).
    fn intro() -> IntroductionPoint {
        IntroductionPoint {
            address: UdpAddress::Ipv4("0.0.0.0:0".parse().unwrap()),
            peer_key: vec![0u8; 64],
            seeder_pk: vec![0u8; 64],
            source: onionbit_tunnel::routing::PEER_SOURCE_PEX,
            last_seen_secs: 0,
        }
    }
}
