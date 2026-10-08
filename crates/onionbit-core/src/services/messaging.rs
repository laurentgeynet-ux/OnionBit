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

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use onionbit_crypto::ipv8::dh::{pair_open_in, pair_seal_in};
use onionbit_crypto::ipv8::keys::{LibNaClPublicKey, LibNaClSecretKey};
use onionbit_db::conversations as dbc;
use onionbit_db::messaging as dbm;
use onionbit_db::Database;
use onionbit_ipv8::UdpAddress;
use onionbit_messaging::{
    conv, derive_messaging_keys,
    gctl::{Gctl, RosterEntry},
    gmsg, hello, messaging_hash, preflight, Frame, MessagingConfig, MessagingError, MessagingKeys,
    MsgKind, RawFrame, RecvWindow,
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

/// Magic du blob coffre (`OBV1` + version integree dans `v`) —
/// octets en clair hors du chiffre pour un sanity check avant AEAD.
const VAULT_MAGIC: &[u8; 4] = b"OBV1";
/// Version du document coffre.
const VAULT_VERSION: u8 = 1;
/// Domaine HKDF du coffre — separe ce chiffrement des enveloppes
/// OBF et des `tx` de ledger (meme cle de paire, infos distinctes —
/// ADR-0015 §7).
const VAULT_HKDF_INFO: &[u8] = b"onionbit/vault/v1";
/// Taille maximale du blob coffre accepte a l'import (1 Mio — une
/// liste de contacts reelle tient dans quelques ko ; borne contre
/// les collages/hostiles de taille arbitraire).
const VAULT_BLOB_MAX: usize = 1 << 20;

/// Lookup de confiance `identity` (ADR-0015) injecte par
/// `Ipv8Stack` : `pk_bin` → score des curateurs suivis.
type TrustLookup = Arc<dyn Fn(&[u8]) -> i64 + Send + Sync>;

/// Entree de coffre exportee (cle hex + pseudonyme local + etat).
#[derive(serde::Serialize)]
struct VaultEntry {
    pk: String,
    alias: String,
    state: String,
}

/// Document coffre v1 — chiffre en entier dans le blob AEAD.
#[derive(serde::Serialize)]
struct VaultDoc {
    v: u8,
    exported: u64,
    contacts: Vec<VaultEntry>,
}

/// Forme lue a l'import — les champs inconnus sont ignores, les
/// absents toleres (l'import reste compatible avec des coffres
/// plus riches produits plus tard).
#[derive(serde::Deserialize)]
struct VaultDocIn {
    v: u8,
    contacts: Vec<VaultEntryIn>,
}

/// Entree lue a l'import (`alias`/`state` optionnels).
#[derive(serde::Deserialize)]
struct VaultEntryIn {
    pk: String,
    alias: Option<String>,
    state: Option<String>,
}

/// Libelle stable d'un etat de consentement (DB + coffre).
fn contact_state_str(state: ContactState) -> &'static str {
    match state {
        ContactState::Active => "active",
        ContactState::Pending => "pending",
        ContactState::Blocked => "blocked",
    }
}
/// Granularite de scrutation de la liaison dans `connect`.
const CONNECT_POLL: Duration = Duration::from_millis(50);
/// Borne du pseudonyme local d'un contact (caracteres) — label UI,
/// pas un identifiant.
const MAX_CONTACT_ALIAS_CHARS: usize = 64;
/// Fenetre d'historique scannee pour la re-emission/le constat
/// d'echec des `Msg` sortants a la reception d'`accept`/`reject`
/// (les plus recents d'abord — suffisant pour la fenetre de
/// consentement).
const CONSENT_RESEND_SCAN: u32 = 64;

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
    /// Contact cree par une intention locale (`resolve`/`connect`) —
    /// visible dans `GET /contacts` meme sans circuit lie (hors
    /// ligne) ; sans lui l'UI ne verrait la creation qu'au prochain
    /// pull.
    ContactAdded {
        /// `pk_bin` du contact cree.
        contact: Vec<u8>,
    },
    /// Transition d'etat de liaison e2e d'un contact (indicateur de
    /// l'API/UI) — `Bound` signale deja le succes ; cet evenement
    /// couvre `connecting`/`failed`/`none` (deliaison).
    Link {
        /// `pk_bin` du contact.
        contact: Vec<u8>,
        /// Etat de liaison courant.
        link: LinkState,
    },
    /// Trame admise dans une conversation de groupe (ADR-0019) —
    /// `gctl` compris : le consommateur re-interroge le roster ou
    /// l'historique selon `kind`.
    Conv {
        /// `conv_id` de la conversation.
        conv: [u8; 16],
        /// `pk_bin` de l'emetteur verifie.
        contact: Vec<u8>,
        /// Type de trame.
        kind: MsgKind,
        /// `id` de trame.
        id: [u8; 16],
        /// Corps applicatif en clair.
        body: Vec<u8>,
    },
    /// Invitation a un groupe recue d'un contact actif — decision
    /// utilisateur requise (`group_accept`/`group_decline`).
    GroupInvite {
        /// `conv_id` du groupe propose.
        conv: [u8; 16],
        /// Nom affiche du groupe.
        name: String,
        /// `pk_bin` de l'invitant.
        by: Vec<u8>,
    },
}

/// Etat de liaison e2e d'un contact — oracle de l'indicateur UI
/// (vert/orange/rouge/gris). Distinct de [`ContactState`] (le
/// consentement) : un contact consenti peut etre sans circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Circuit e2e lie — trames emissibles.
    Bound,
    /// Liaison en cours : `connect` explicite en vol ou tentative
    /// automatique du tunnel (`pending_e2e`/RP_DOWNLOADER non lie).
    Connecting,
    /// Derniere tentative `connect` echouee — aucune liaison en
    /// cours. Re-tentative via `connect_peer` (ou automatique :
    /// `do_peer_discovery` re-essaie tant que le swarm est joint).
    Failed,
    /// Aucune liaison ni tentative connue (jamais tente ou
    /// delie faute de retry en cours).
    None,
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

/// Portee d'un pair (ADR-0019 §3) : un pair `Group` est connu
/// seulement via un roster — son lien sert les compteurs
/// anti-replay mais le confinement l'empeche de parler en 1:1,
/// de devenir `pending` ou d'inviter hors de son groupe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactScope {
    /// Contact au sens propre (consentement complet).
    Contact,
    /// Pair confine a un roster de groupe.
    Group,
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
    /// Demandes passees en `blocked` par la gate confiance
    /// (`consent_gate_flagged` — score identity < 0).
    pub consent_blocked: AtomicU64,
    /// Demandes admises `Active` directement par la gate confiance
    /// (`consent_gate_endorsed` — score identity > 0).
    pub consent_auto_accepted: AtomicU64,
    /// Demandes refusees par la gate dette (`consent_gate_ledger`
    /// + `ledger_enforce` — deficit > `max_deficit_bytes`).
    pub consent_ledger_refused: AtomicU64,
    /// Trames v2 ecartees au routage `conv` (ADR-0019) : conv
    /// inconnue, directe depuis un pair `scope='group'`, membre
    /// absent du roster, conv `left`.
    pub group_rejected: AtomicU64,
}

/// Etat d'un pair messagerie (cle : `pk_bin`) — contact ou membre
/// de groupe confine (le meme objet porte les compteurs du lien
/// dans les deux cas).
struct Contact {
    /// Cle publique du pair (verification des signatures).
    pk: LibNaClPublicKey,
    /// Etat de consentement.
    state: ContactState,
    /// Portee (`Group` = confine roster — ADR-0019).
    scope: ContactScope,
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
    /// Derniere tentative `connect` echouee (non persistee — un
    /// restart repart sur `None`, la maintenance tunnel retente).
    link_failed: bool,
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
            scope: ContactScope::Contact,
            pending_since: 0,
            recv_window: RecvWindow::new(cfg),
            send_seq: 0,
            circuit: None,
            link_failed: false,
            greeted: false,
            bucket: TokenBucket::new(cfg.per_contact_rate),
        }
    }

    /// Pair `scope='group'` neuf : `Active` (le roster l'a deja
    /// admis — pas de `pending`) mais confine aux trames `conv`
    /// de ses groupes (confinement dans `handle_incoming`).
    fn group_scoped(pk: LibNaClPublicKey, cfg: &MessagingConfig) -> Self {
        let mut c = Self::active(pk, cfg);
        c.scope = ContactScope::Group;
        c
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
    /// `connect_peer` en vol par contact — l'indicateur `connecting`
    /// couvre le resolve DHT + l'attente de liaison du `POST`.
    linking: Mutex<HashSet<Vec<u8>>>,
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
    /// Lookup du score de confiance ext (`kind=identity`) d'une cle
    /// — injecte par `Ipv8Stack` quand la communaute ext tourne
    /// (`None` sinon : les gates `consent_gate_*` sont inertes).
    trust_lookup: Mutex<Option<TrustLookup>>,
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
            linking: Mutex::new(HashSet::new()),
            circuits: Mutex::new(HashMap::new()),
            db,
            stats: MessagingStats::default(),
            events_tx,
            trust_lookup: Mutex::new(None),
            stops: Mutex::new(Vec::new()),
        });
        svc.load_state();
        svc.spawn_e2e_listener();
        svc.spawn_presence_monitor();
        svc
    }

    /// Injecte le lookup de confiance ext (`kind=identity`) —
    /// appele par `Ipv8Stack` quand la communaute ext tourne. Sans
    /// lui, `consent_gate_flagged`/`endorsed` n'ont aucun effet.
    pub fn set_trust_lookup(&self, f: TrustLookup) {
        *self.trust_lookup.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// Score de confiance identity d'une cle, si le lookup est
    /// injecte (`None` = ext absente — gates confiance inertes).
    fn trust_score(&self, pk_bin: &[u8]) -> Option<i64> {
        self.trust_lookup
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|f| f(pk_bin))
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
        let mut rejoin: Vec<(Vec<u8>, LibNaClPublicKey)> = Vec::new();
        {
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            for row in rows {
                let Ok(pk) = LibNaClPublicKey::from_bin(&row.public_key) else {
                    continue;
                };
                let mut c = Contact::active(pk.clone(), &self.cfg);
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
                    _ => {
                        // `Active` : rejoindre le swarm du contact pour
                        // que `do_peer_discovery` relie le circuit e2e
                        // sans action utilisateur (sinon le contact
                        // restaure ne pourrait jamais se reconnecter).
                        rejoin.push((row.public_key.clone(), pk));
                    }
                }
                contacts.insert(row.public_key, c);
            }
        }
        for (pk_bin, pk) in rejoin {
            let mh = messaging_hash(&pk);
            let mut swarms = self.swarms.lock().unwrap_or_else(|e| e.into_inner());
            if let std::collections::hash_map::Entry::Vacant(e) = swarms.entry(mh) {
                e.insert(pk_bin);
                self.tunnel.join_swarm(mh, self.hops, false);
            }
        }
        self.load_group_peers();
        self.backfill_conversations();
    }

    /// Pairs `scope='group'` persistes (ADR-0019) : recharges en
    /// `Contact` confines — leurs compteurs `send_seq`/`recv_top`
    /// servent l'anti-replay du lien et leur cle l'identification
    /// (`identify` balaie la table). Jamais exposes en contacts.
    fn load_group_peers(&self) {
        let Some(db) = &self.db else { return };
        let rows = match db.with(|c| dbm::list_by_scope(c, "group")) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "restauration pairs de groupe impossible");
                return;
            }
        };
        let own = self.key.public_key().to_bin();
        let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
        for row in rows {
            if row.public_key == own {
                continue;
            }
            let Ok(pk) = LibNaClPublicKey::from_bin(&row.public_key) else {
                continue;
            };
            let mut c = Contact::group_scoped(pk, &self.cfg);
            c.send_seq = row.send_seq.max(0) as u64;
            if row.recv_top > 0 {
                c.recv_window = RecvWindow::resume(&self.cfg, row.recv_top as u64);
            }
            if row.state == "blocked" {
                c.state = ContactState::Blocked;
            }
            contacts.insert(row.public_key, c);
        }
    }

    /// Rejeu de la migration v21 (ADR-0019) : materialise la
    /// conversation directe de chaque contact et attribute
    /// `conv_id` aux messages historiques qui n'en ont pas. La
    /// derivation est deterministe (les deux extremites calculent
    /// la meme valeur) — le rejeu est idempotent et relancable.
    fn backfill_conversations(&self) {
        let Some(db) = &self.db else { return };
        let own = self.key.public_key().to_bin();
        let contacts = self
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let now = now_secs() as i64;
        if let Err(e) = db.with(|c| {
            for pk_bin in &contacts {
                let conv = onionbit_messaging::conv::direct_conv(&own, pk_bin);
                dbc::upsert_conversation(
                    c,
                    &dbc::MsgConversationRow {
                        conv_id: conv.to_vec(),
                        kind: "direct".into(),
                        name: String::new(),
                        state: "active".into(),
                        created_at: now,
                        updated_at: now,
                        last_read_ts: 0,
                    },
                )?;
                dbm::backfill_conv(c, &conv, pk_bin)?;
            }
            Ok(())
        }) {
            tracing::warn!(error = %e, "rejeu des conversations directes impossible");
        }
    }

    /// `conv_id` de la conversation directe avec `contact_pk`
    /// (derivation deterministe ADR-0019 — aucune negociation).
    fn direct_conv_id(&self, contact_pk: &[u8]) -> [u8; 16] {
        conv::direct_conv(&self.key.public_key().to_bin(), contact_pk)
    }

    /// Portee d'un pair (`Contact` si absent de la table — un
    /// inconnu ne passe de toute facon pas la barriere de
    /// consentement).
    fn peer_scope(&self, pk_bin: &[u8]) -> ContactScope {
        self.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(pk_bin)
            .map(|c| c.scope)
            .unwrap_or(ContactScope::Contact)
    }

    /// Materialise la conversation directe d'un contact (idempotent
    /// — creation paresseuse, ADR-0019 §2).
    fn ensure_direct_conv(&self, contact_pk: &[u8]) {
        let Some(db) = &self.db else { return };
        let conv = self.direct_conv_id(contact_pk);
        let now = now_secs() as i64;
        if let Err(e) = db.with(|c| {
            dbc::upsert_conversation(
                c,
                &dbc::MsgConversationRow {
                    conv_id: conv.to_vec(),
                    kind: "direct".into(),
                    name: String::new(),
                    state: "active".into(),
                    created_at: now,
                    updated_at: now,
                    last_read_ts: 0,
                },
            )
        }) {
            tracing::warn!(error = %e, "persistance conversation directe");
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
        let mh: Vec<[u8; 20]> = self
            .swarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect();
        for m in mh {
            self.tunnel.leave_swarm(&m);
        }
        for tx in self
            .stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
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

    /// Orchestration du `POST /contacts/connect` : `resolve` (DHT)
    /// puis `connect` sur le premier point d'introduction resolu.
    /// Pilote l'etat de liaison expose par [`Self::link_state`] :
    /// `Connecting` pendant l'appel, `Failed` si l'ensemble echoue
    /// (la maintenance tunnel `do_peer_discovery` retente ensuite
    /// d'elle-meme tant que le swarm est joint).
    pub async fn connect_peer(&self, contact_pk: &[u8]) -> Result<u32> {
        if let Some(cid) = self.contact_circuit(contact_pk) {
            return Ok(cid);
        }
        self.link_begin(contact_pk);
        let res = async {
            let ips = self.resolve(contact_pk).await?;
            let Some(ip) = ips.into_iter().next() else {
                return Err(CoreError::InvalidState(
                    "messagerie : aucun point d'introduction pour ce contact",
                ));
            };
            self.connect(contact_pk, &ip).await
        }
        .await;
        self.link_end(contact_pk, res.is_ok());
        res
    }

    /// Etat de liaison e2e courant du contact — ordre : `Bound`
    /// (circuit pose) > `Connecting` (`connect` en vol ou tentative
    /// e2e automatique du tunnel) > `Failed` > `None`.
    pub fn link_state(&self, contact_pk: &[u8]) -> LinkState {
        let (pk, failed) = {
            let contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            match contacts.get(contact_pk) {
                Some(c) if c.circuit.is_some() => return LinkState::Bound,
                Some(c) => (Some(c.pk.clone()), c.link_failed),
                None => (None, false),
            }
        };
        if self
            .linking
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(contact_pk)
        {
            return LinkState::Connecting;
        }
        if let Some(pk) = pk {
            if self.tunnel.e2e_pending(&messaging_hash(&pk)) {
                return LinkState::Connecting;
            }
        }
        if failed {
            return LinkState::Failed;
        }
        LinkState::None
    }

    /// Entree en tentative de liaison explicite : marque `linking`
    /// (et efface un `Failed` anterieur) puis notifie l'API.
    fn link_begin(&self, contact_pk: &[u8]) {
        self.linking
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(contact_pk.to_vec());
        if let Some(c) = self
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(contact_pk)
        {
            c.link_failed = false;
        }
        self.emit_link(contact_pk);
    }

    /// Fin de tentative explicite : `Failed` sur echec, puis notifie.
    fn link_end(&self, contact_pk: &[u8], ok: bool) {
        self.linking
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(contact_pk);
        if !ok {
            if let Some(c) = self
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(contact_pk)
            {
                c.link_failed = true;
            }
        }
        self.emit_link(contact_pk);
    }

    /// Emet l'etat de liaison courant du contact (recompute —
    /// l'evenement est un indice de fraicheur, jamais une source).
    fn emit_link(&self, contact_pk: &[u8]) {
        let _ = self.events_tx.send(MessagingEvent::Link {
            contact: contact_pk.to_vec(),
            link: self.link_state(contact_pk),
        });
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
        if self.peer_scope(contact_pk) == ContactScope::Group {
            return Err(CoreError::InvalidState(
                "messagerie : pair confine a un groupe",
            ));
        }
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
        let (cid, pk) = {
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            let Some(c) = contacts.get_mut(contact_pk) else {
                return Err(CoreError::InvalidState("messagerie : contact inconnu"));
            };
            if c.state != ContactState::Pending {
                return Err(CoreError::InvalidState(
                    "messagerie : contact non en attente",
                ));
            }
            c.state = ContactState::Active;
            (c.circuit, c.pk.clone())
        };
        // Consentement = bidirectionnel : rejoindre le swarm du
        // contact pour pouvoir re-lier un circuit si le courant meurt
        // (un `pending` inbound ne joint jamais le swarm de son
        // emetteur — `ensure_contact_swarm` ne cree rien ici, il
        // joint seulement).
        self.ensure_contact_swarm(&pk, contact_pk);
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
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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
        self.swarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&mh);
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
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, c)| c.state == ContactState::Pending)
            .map(|(pk, c)| (pk.clone(), now.saturating_sub(c.pending_since)))
            .collect()
    }

    /// Etat de consentement d'un contact (`None` = inconnu).
    pub fn contact_state(&self, contact_pk: &[u8]) -> Option<ContactState> {
        self.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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

    /// Pseudonyme local du contact (`""` = aucun — l'UI retombe
    /// sur la cle abregee). Trime et borne a
    /// [`MAX_CONTACT_ALIAS_CHARS`] caracteres ; un contact inconnu
    /// est refuse (`InvalidState`). Le pseudonyme n'appartient pas
    /// au consentement : il survit aux transitions d'etat
    /// (`upsert_contact` ne touche pas la colonne).
    pub fn set_alias(&self, contact_pk: &[u8], alias: &str) -> Result<()> {
        if self.contact_state(contact_pk).is_none() {
            return Err(CoreError::InvalidState("messagerie : contact inconnu"));
        }
        let trimmed = alias.trim();
        if trimmed.chars().count() > MAX_CONTACT_ALIAS_CHARS {
            return Err(CoreError::InvalidState("messagerie : pseudonyme trop long"));
        }
        let Some(db) = &self.db else { return Ok(()) };
        let now = now_secs() as i64;
        db.with(|c| dbm::set_alias(c, contact_pk, trimmed, now))
            .map_err(|e| CoreError::State(format!("pseudonyme messagerie: {e}")))
    }

    /// Pseudonyme local du contact (`None` = inconnu ou non defini).
    pub fn contact_alias(&self, contact_pk: &[u8]) -> Option<String> {
        let db = self.db.as_ref()?;
        db.with(|c| dbm::get_contact(c, contact_pk))
            .ok()
            .flatten()
            .map(|r| r.alias)
            .filter(|a| !a.is_empty())
    }

    /// Exporte le coffre de contacts — `{v, contacts: [{pk, alias,
    /// state}]}` chiffre `pair_seal_in` **pour soi-meme** (AEAD
    /// `XChaCha20-Poly1305` sous `onionbit/vault/v1`, cle derivee du
    /// DH `crypt_pk x crypt_sk` de l'identite). Portable entre devices
    /// partageant la meme identite (ADR-0016) ; contrairement aux
    /// auto-attestations `identity` (publiques par design), le blob
    /// est illisible pour quiconque n'a pas la cle privee —
    /// pseudonymes inclus.
    pub fn export_vault(&self) -> Vec<u8> {
        let entries: Vec<VaultEntry> = {
            let contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            contacts
                .iter()
                .map(|(pk_bin, c)| VaultEntry {
                    pk: hex::encode(pk_bin),
                    alias: self.contact_alias(pk_bin).unwrap_or_default(),
                    state: contact_state_str(c.state).to_owned(),
                })
                .collect()
        };
        let doc = serde_json::to_vec(&VaultDoc {
            v: VAULT_VERSION,
            exported: now_secs(),
            contacts: entries,
        })
        .unwrap_or_default();
        let pk = self.key.public_key();
        let blob = pair_seal_in(
            &pk.crypt_pk,
            &self.key.crypt_x25519().to_bytes(),
            VAULT_HKDF_INFO,
            &doc,
        )
        .unwrap_or_default();
        let mut out = Vec::with_capacity(VAULT_MAGIC.len() + blob.len());
        out.extend_from_slice(VAULT_MAGIC);
        out.extend_from_slice(&blob);
        out
    }

    /// Importe un blob coffre (dechiffrement AEAD par notre propre
    /// identite) : chaque entree inconnue devient un contact `Active`
    /// (notre propre liste est une liste de confiance — pas un
    /// `pending`), l'etat exporte `blocked` est conserve, alias
    /// restaure. Renvoie le nombre de contacts restaurés.
    /// `InvalidState` si magic/absent, AEAD ou JSON invalide.
    pub fn import_vault(&self, blob: &[u8]) -> Result<usize> {
        if blob.len() > VAULT_BLOB_MAX {
            return Err(CoreError::InvalidState("coffre : blob trop gros"));
        }
        let sealed = blob
            .strip_prefix(VAULT_MAGIC.as_slice())
            .ok_or(CoreError::InvalidState("coffre : magic OBV1 absent"))?;
        let pk = self.key.public_key();
        let doc_bytes = pair_open_in(
            &pk.crypt_pk,
            &self.key.crypt_x25519().to_bytes(),
            VAULT_HKDF_INFO,
            sealed,
        )
        .map_err(|_| CoreError::InvalidState("coffre : dechiffrement impossible"))?;
        let doc: VaultDocIn = serde_json::from_slice(&doc_bytes)
            .map_err(|_| CoreError::InvalidState("coffre : JSON invalide"))?;
        if doc.v != VAULT_VERSION {
            return Err(CoreError::InvalidState("coffre : version inconnue"));
        }
        let mut restored = 0usize;
        let mut rejoin: Vec<(Vec<u8>, LibNaClPublicKey)> = Vec::new();
        for e in &doc.contacts {
            let Ok(pk_bin) = hex::decode(&e.pk) else {
                continue;
            };
            if self
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&pk_bin)
            {
                continue;
            }
            let Ok(pk) = LibNaClPublicKey::from_bin(&pk_bin) else {
                continue;
            };
            let state = match e.state.as_deref() {
                Some("blocked") => ContactState::Blocked,
                _ => ContactState::Active,
            };
            let mut c = Contact::active(pk.clone(), &self.cfg);
            c.state = state;
            self.contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(pk_bin.clone(), c);
            self.persist_contact(&pk_bin, state);
            if state == ContactState::Active {
                rejoin.push((pk_bin.clone(), pk));
            }
            if let Some(alias) = &e.alias {
                if !alias.is_empty() {
                    let _ = self.set_alias(&pk_bin, alias);
                }
            }
            restored += 1;
        }
        // Meme rejoin que `load_state` : les contacts actifs
        // restaurés rejoignent leur swarm de presence.
        for (pk_bin, pk) in rejoin {
            let mh = messaging_hash(&pk);
            self.tunnel.join_swarm_with_key(mh, self.hops, None);
            self.swarms
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(mh, pk_bin);
        }
        Ok(restored)
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
    pub fn stats_snapshot(&self) -> [(&'static str, u64); 10] {
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
            (
                "consent_blocked",
                self.stats.consent_blocked.load(Ordering::Relaxed),
            ),
            (
                "consent_auto_accepted",
                self.stats.consent_auto_accepted.load(Ordering::Relaxed),
            ),
            (
                "consent_ledger_refused",
                self.stats.consent_ledger_refused.load(Ordering::Relaxed),
            ),
        ]
    }

    /// Persiste l'etat de consentement d'un contact (upsert — les
    /// compteurs `seq` conserves cote DB par l'`ON CONFLICT`).
    fn persist_contact(&self, pk_bin: &[u8], state: ContactState) {
        let Some(db) = &self.db else { return };
        let st = contact_state_str(state);
        let (send_seq, recv_top) = {
            let contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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
            alias: String::new(),
            scope: "contact".into(),
            created_at: now,
            updated_at: now,
        };
        if let Err(e) = db.with(|c| dbm::upsert_contact(c, &row)) {
            tracing::warn!(error = %e, "persistance contact messagerie");
        }
        self.ensure_direct_conv(pk_bin);
    }

    /// Persiste les compteurs `seq` du contact (apres chaque trame
    /// emise/admise — la reprise au restart est exacte).
    fn persist_seqs(&self, pk_bin: &[u8]) {
        let Some(db) = &self.db else { return };
        let (send_seq, recv_top) = {
            let contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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
    /// livraison — voir [`dbm::MsgMessageRow`]). `conv_id` vide =
    /// conversation directe derivee (compat des constructeurs 1:1
    /// historiques ; le groupe renseigne explicitement `conv_id`,
    /// `author_pk` et `mid`).
    fn persist_message(&self, mut row: dbm::MsgMessageRow) {
        let Some(db) = &self.db else { return };
        if row.conv_id.is_empty() {
            row.conv_id = self.direct_conv_id(&row.contact_pk).to_vec();
        }
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
            conv_id: Vec::new(),
            author_pk: None,
            mid: None,
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
        let removed = self
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(contact_pk);
        if let Some(c) = removed {
            let mh = messaging_hash(&c.pk);
            self.tunnel.leave_swarm(&mh);
            self.swarms
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&mh);
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
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(pk, c)| (pk.clone(), c.circuit))
            .collect()
    }

    /// `circuit_id` lie du contact, s'il existe.
    fn contact_circuit(&self, contact_pk: &[u8]) -> Option<u32> {
        self.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(contact_pk)
            .and_then(|c| c.circuit)
    }

    /// `send_seq` courant du contact (sans consommer — sert a
    /// dater les messages `failed` hors ligne).
    fn peek_send_seq(&self, contact_pk: &[u8]) -> u64 {
        self.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(contact_pk)
            .map(|c| c.send_seq)
            .unwrap_or_default()
    }

    /// `hello` deja emis pour ce contact.
    fn is_greeted(&self, contact_pk: &[u8]) -> bool {
        self.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            let existed = contacts.contains_key(pk_bin);
            let c = contacts
                .entry(pk_bin.to_vec())
                .or_insert_with(|| Contact::active(pk.clone(), &self.cfg));
            (c.state == ContactState::Blocked, !existed)
        };
        if created {
            self.persist_contact(pk_bin, ContactState::Active);
            let _ = self.events_tx.send(MessagingEvent::ContactAdded {
                contact: pk_bin.to_vec(),
            });
        }
        if !blocked {
            let mut swarms = self.swarms.lock().unwrap_or_else(|e| e.into_inner());
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
            let circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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
            if let Some(c) = self
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(contact_pk)
            {
                c.greeted = true;
            }
        }
        Ok(frame.id)
    }

    /// Re-emission d'une trame `Msg` deja persistee (`sent` jamais
    /// acquitte — typiquement ecartee par le `pending_drop` du
    /// repondant). L'`id` d'origine est conserve : la dedup la rend
    /// idempotente si la premiere copie etait passee, et l'`Ack` en
    /// retour retrouve la ligne d'outbox. Le `seq` est frais — un
    /// ancien `seq` pourrait tomber hors fenetre `recv_window` si le
    /// pair a admis des trames plus recentes entre-temps. Aucune
    /// ligne n'est re-persistee (la ligne `sent` existe deja).
    async fn resend_frame(
        &self,
        contact_pk: &[u8],
        cid: u32,
        id: [u8; 16],
        body: Vec<u8>,
    ) -> Result<()> {
        let (send_key, seq) = {
            let circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            let (Some(binding), Some(contact)) = (circuits.get(&cid), contacts.get_mut(contact_pk))
            else {
                return Err(CoreError::InvalidState("messagerie : liaison disparue"));
            };
            let seq = contact.send_seq;
            contact.send_seq += 1;
            (binding.keys.send, seq)
        };
        let mut frame = Frame::new(MsgKind::Msg, seq, now_secs(), body);
        frame.id = id;
        let wire = frame
            .seal(&self.key, &send_key, &self.cfg)
            .map_err(|e| CoreError::State(format!("seal trame: {e}")))?;
        self.persist_seqs(contact_pk);
        if let Err(e) = self
            .tunnel
            .send_data(cid, &unspecified(), &unspecified(), &wire)
            .await
        {
            self.set_msg_status(&frame.id, "failed");
            let _ = self.events_tx.send(MessagingEvent::Undeliverable {
                contact: contact_pk.to_vec(),
                id: frame.id,
            });
            self.unbind_circuit(cid);
            return Err(CoreError::State(format!("send_data messagerie: {e}")));
        }
        Ok(())
    }

    /// Re-emission des `Msg` sortants `sent` (non acquittes) d'un
    /// contact — appele a la reception de son `accept`. Ordre
    /// chronologique (le plus ancien d'abord).
    async fn resend_unacked(&self, contact_pk: &[u8], cid: u32) {
        let Some(db) = &self.db else {
            return;
        };
        let Ok(rows) = db.with(|c| dbm::list_messages(c, contact_pk, CONSENT_RESEND_SCAN)) else {
            return;
        };
        for row in rows
            .into_iter()
            .rev()
            .filter(|r| r.direction == "out" && r.status == "sent")
        {
            let Ok(id) = <[u8; 16]>::try_from(row.id.as_slice()) else {
                continue;
            };
            // La liaison a pu tomber entre-temps : le premier echec
            // arrete la serie (la trame passe `failed`, le circuit
            // est delie — les suivantes attendraient en vain).
            if self
                .resend_frame(contact_pk, cid, id, row.body)
                .await
                .is_err()
            {
                return;
            }
        }
    }

    /// Marque `failed` les `Msg` sortants encore `sent` d'un contact
    /// — appele a la reception de son `reject` : le repondant a
    /// oublie la liaison, ces trames ne seront jamais admises.
    fn fail_unacked(&self, contact_pk: &[u8]) {
        let Some(db) = &self.db else {
            return;
        };
        let Ok(rows) = db.with(|c| dbm::list_messages(c, contact_pk, CONSENT_RESEND_SCAN)) else {
            return;
        };
        for row in rows
            .into_iter()
            .filter(|r| r.direction == "out" && r.status == "sent")
        {
            let Ok(id) = <[u8; 16]>::try_from(row.id.as_slice()) else {
                continue;
            };
            self.set_msg_status(&id, "failed");
            let _ = self.events_tx.send(MessagingEvent::Undeliverable {
                contact: contact_pk.to_vec(),
                id,
            });
        }
    }

    /// Relais `e2e_ready` : demultiplexe les liaisons par swarm —
    /// `own_mh` = repondant (contact a identifier), swarm contact =
    /// initiateur (contact connu). Les swarms BitTorrent sont
    /// ignores (listener dedie dans `ipv8_stack`).
    fn spawn_e2e_listener(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
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
        } else if let Some(pk_bin) = self
            .swarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&lookup)
        {
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
        self.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                cid,
                CircuitBinding {
                    keys: keys.clone(),
                    contact: contact.clone(),
                },
            );
        match contact {
            Some(pk_bin) => {
                if let Some(c) = self
                    .contacts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&pk_bin)
                {
                    c.circuit = Some(cid);
                    c.link_failed = false;
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
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cid)
            .and_then(|b| b.contact);
        let mut detached = None;
        if let Some(pk_bin) = contact {
            if let Some(c) = self
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&pk_bin)
            {
                if c.circuit == Some(cid) {
                    c.circuit = None;
                    detached = Some(pk_bin);
                }
            }
        }
        // Deliaison reelle : rafraichir l'indicateur (`Connecting` si
        // la maintenance a deja une retentative e2e en cours,
        // `None` sinon).
        if let Some(pk_bin) = detached {
            self.emit_link(&pk_bin);
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
        if let Some(b) = self
            .global_bucket
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
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
            .unwrap_or_else(|e| e.into_inner())
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
        // Routage `conv` (ADR-0019) — avant la dedup : une trame
        // mal routee ne consomme ni jeton ni fenetre. `Direct` =
        // conv directe derivee ou pas de `conv` (v1) ; `Group` =
        // conv connue et emetteur au roster ; `GroupNew` = conv
        // inconnue — seule une `gctl invite` peut la creer.
        enum ConvTarget {
            Direct,
            Group([u8; 16]),
            GroupNew([u8; 16]),
        }
        let conv_target = match raw.conv {
            None => {
                // v1 : confinement — un pair `scope='group'` ne
                // parle qu'en trames v2 de groupe.
                if self.peer_scope(&pk_bin) == ContactScope::Group {
                    self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                ConvTarget::Direct
            }
            Some(c) if c == self.direct_conv_id(&pk_bin) => {
                // Directe v2 : reservee aux contacts (confinement).
                if self.peer_scope(&pk_bin) == ContactScope::Group {
                    self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                ConvTarget::Direct
            }
            Some(c) => {
                // Conv de groupe : etat de la conv + appartenance
                // de l'emetteur au roster — la `gctl invite` d'une
                // conv inconnue est admise (sa validation reste
                // applicative : invite seulement d'un contact).
                let conv_id = c;
                let known = self.db_state(|c2| dbc::conversation_state(c2, &conv_id));
                match known.as_deref() {
                    Some("active") | Some("invited") => {
                        let member = self.db_state(|c2| dbc::member_state(c2, &conv_id, &pk_bin));
                        if member.as_deref() != Some("member") {
                            self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        // `invited` n'admet que le `gctl` de
                        // negociation — jamais un `msg`.
                        if known.as_deref() == Some("invited") && raw.kind != MsgKind::Gctl {
                            self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        ConvTarget::Group(conv_id)
                    }
                    None if raw.kind == MsgKind::Gctl => ConvTarget::GroupNew(conv_id),
                    _ => {
                        self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }
        };
        // Dedup `id` d'abord (gratuite — une re-emission honnete ne
        // consomme pas de jeton), puis seau du contact, puis la
        // fenetre `seq` : une trame ecartee au budget n'est PAS
        // admise et sa re-emission ulterieure peut etre livree.
        let admitted = {
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            match contacts.get_mut(&pk_bin) {
                None => false,
                Some(c) => {
                    if c.recv_window.seen_id(&raw.id) {
                        self.stats.replay.fetch_add(1, Ordering::Relaxed);
                        // Trame deja vue : pour un `msg`, l'emetteur
                        // retransmet car notre ACK precedent s'est
                        // perdu — on le re-emet sans re-persister ni
                        // re-pousser l'evenement.
                        if raw.kind == MsgKind::Msg {
                            let svc = self.clone();
                            let pk = pk_bin.clone();
                            let ack_id = raw.id;
                            tokio::spawn(async move {
                                let _ = svc
                                    .send_frame(&pk, cid, MsgKind::Ack, ack_id.to_vec())
                                    .await;
                            });
                        }
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
            if let ConvTarget::Group(conv) | ConvTarget::GroupNew(conv) = conv_target {
                self.handle_group_frame(conv, &pk_bin, cid, &raw);
                return;
            }
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
                    let mut row = Self::msg_row(
                        &pk_bin, "in", raw.seq, raw.ts, &raw.body, "received", &raw.id,
                    );
                    if let Some(c) = raw.conv {
                        row.conv_id = c.to_vec();
                    }
                    self.persist_message(row);
                    let svc = self.clone();
                    let pk = pk_bin.clone();
                    let ack_id = raw.id;
                    tokio::spawn(async move {
                        let _ = svc
                            .send_frame(&pk, cid, MsgKind::Ack, ack_id.to_vec())
                            .await;
                    });
                }
                // Consentement accorde : les `Msg` sortants encore
                // `sent` ont pu etre ecartes cote repondant pendant
                // que nous etions `pending` (drop silencieux, jamais
                // de NACK) — re-emission sous leur `id` d'origine
                // (dedup idempotent si la copie etait passee).
                MsgKind::Accept => {
                    let svc = self.clone();
                    let pk = pk_bin.clone();
                    tokio::spawn(async move {
                        svc.resend_unacked(&pk, cid).await;
                    });
                }
                // Consentement refuse : les trames emises pendant le
                // `pending` ne seront jamais admises — `failed` +
                // `Undeliverable`, l'appelant n'attend pas pour rien.
                MsgKind::Reject => {
                    let svc = self.clone();
                    let pk = pk_bin.clone();
                    tokio::spawn(async move {
                        svc.fail_unacked(&pk);
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
                // Pair confine roster (`scope='group'`, ADR-0019) :
                // la ligne DB existe deja — admission `Active`
                // confinee sans les gates de consentement (le
                // roster l'a admis ; le confinement `conv` fait
                // le reste). Ses compteurs de lien sont restaures.
                if self.db_scope(pk_bin).as_deref() == Some("group") {
                    let Some(row) = self
                        .db
                        .as_ref()
                        .and_then(|db| db.with(|c| dbm::get_contact(c, pk_bin)).ok())
                        .flatten()
                    else {
                        return false;
                    };
                    let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) else {
                        return false;
                    };
                    let mut c = Contact::group_scoped(pk, &self.cfg);
                    c.send_seq = row.send_seq.max(0) as u64;
                    if row.recv_top > 0 {
                        c.recv_window = RecvWindow::resume(&self.cfg, row.recv_top as u64);
                    }
                    c.circuit = Some(cid);
                    self.contacts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(pk_bin.to_vec(), c);
                    self.bind_circuit(cid, pk_bin, keys);
                    return true;
                }
                // Gates ADR-0015 avant tout etat : dette ledger
                // (un pair trop endette n'ouvre pas de lane) puis
                // confiance identity (flague → `blocked` sans
                // pending ; approuve → `Active` direct).
                if self.cfg.consent_gate_ledger
                    && self.tunnel.ledger.is_enforce()
                    && self.tunnel.ledger.stat(pk_bin).map_or(0, |s| s.deficit())
                        > self.tunnel.ledger.max_deficit_bytes()
                {
                    self.stats
                        .consent_ledger_refused
                        .fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                if let Some(score) = self.trust_score(pk_bin) {
                    if self.cfg.consent_gate_flagged && score < 0 {
                        if let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) {
                            let mut c = Contact::active(pk, &self.cfg);
                            c.state = ContactState::Blocked;
                            self.contacts
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(pk_bin.to_vec(), c);
                            self.persist_contact(pk_bin, ContactState::Blocked);
                        }
                        self.stats.consent_blocked.fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                    if self.cfg.consent_gate_endorsed && score > 0 {
                        if let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) {
                            let mut c = Contact::active(pk, &self.cfg);
                            c.circuit = Some(cid);
                            self.contacts
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(pk_bin.to_vec(), c);
                            self.persist_contact(pk_bin, ContactState::Active);
                            self.bind_circuit(cid, pk_bin, keys);
                            self.stats
                                .consent_auto_accepted
                                .fetch_add(1, Ordering::Relaxed);
                            return true;
                        }
                    }
                }
                // Inconnu : admission `pending` bornee (capacite +
                // TTL purges d'abord).
                self.purge_expired_pending();
                let full = self
                    .contacts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
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
                    let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Portee DB d'un pair (`"contact"`/`"group"` ; `None` =
    /// inconnu ou pas de persistance).
    fn db_scope(&self, pk_bin: &[u8]) -> Option<String> {
        self.db
            .as_ref()
            .and_then(|db| db.with(|c| dbm::contact_scope(c, pk_bin)).ok())
            .flatten()
    }

    /// Lecture DB opportuniste (`None` si pas de persistance ou
    /// erreur — le routage `conv` reste fonctionnel en memoire).
    fn db_state<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> onionbit_db::Result<Option<T>>,
    ) -> Option<T> {
        self.db.as_ref().and_then(|db| db.with(f).ok()).flatten()
    }

    /// Lie le circuit a un contact existant (reouverture ou
    /// re-pending) — le `hello` sera re-emis : `greeted` rearme.
    fn bind_existing(&self, cid: u32, pk_bin: &[u8], keys: &MessagingKeys) {
        self.bind_circuit(cid, pk_bin, keys);
        if let Some(c) = self
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(pk_bin)
        {
            c.circuit = Some(cid);
            c.greeted = false;
        }
    }

    /// Pose/met a jour la liaison du circuit (creee si absente —
    /// `unbind_circuit` a pu la retirer apres une erreur d'emission).
    fn bind_circuit(&self, cid: u32, pk_bin: &[u8], keys: &MessagingKeys) {
        self.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
            .unwrap_or_else(|e| e.into_inner())
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
            // v1 : corps = `pk_bin` seule ; v2 : `pk_bin ‖ caps`.
            if let Ok((pk_bin, _caps)) = hello::decode_hello(&raw.body) {
                if let Ok(pk) = LibNaClPublicKey::from_bin(&pk_bin) {
                    if raw.verify(&pk).is_ok() {
                        return Some(pk.to_bin());
                    }
                }
            }
            return None;
        }
        let pks: Vec<Vec<u8>> = self
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        pks.into_iter().find(|pk_bin| {
            self.contact_state(pk_bin) != Some(ContactState::Blocked)
                && matches!(self.verify_against(pk_bin, raw), Some(Ok(_)))
        })
    }

    // ── ADR-0019 : conversations de groupe ─────────────────

    /// Dispatch d'une trame admise dans une conv de groupe
    /// (`conv` deja routee : etat + appartenance verifies).
    fn handle_group_frame(
        self: &Arc<Self>,
        conv: [u8; 16],
        sender: &[u8],
        cid: u32,
        raw: &RawFrame,
    ) {
        let (kind, id, body) = (raw.kind, raw.id, raw.body.as_slice());
        match kind {
            MsgKind::Gctl => self.handle_gctl(&conv, sender, body),
            MsgKind::Msg => {
                let Ok((mid, payload)) = gmsg::decode_gmsg(body) else {
                    self.stats.codec.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let ack = |svc: Arc<Self>, pk: Vec<u8>| {
                    tokio::spawn(async move {
                        let _ = svc
                            .send_frame_v2(&pk, cid, conv, MsgKind::Ack, id.to_vec())
                            .await;
                    });
                };
                // Dedup applicative `(conv, author, mid)` : une
                // re-emission honnete apres reouverture porte un
                // nouvel `id` de trame — la dedup `id` ne la
                // voit pas, `mid` si.
                if self
                    .db
                    .as_ref()
                    .and_then(|db| db.with(|c| dbm::has_group_mid(c, &conv, sender, &mid)).ok())
                    .unwrap_or(false)
                {
                    ack(self.clone(), sender.to_vec());
                    return;
                }
                let mut row =
                    Self::msg_row(sender, "in", raw.seq, raw.ts, &payload, "received", &id);
                row.conv_id = conv.to_vec();
                row.author_pk = Some(sender.to_vec());
                row.mid = Some(mid.to_vec());
                self.persist_message(row);
                ack(self.clone(), sender.to_vec());
                let _ = self.events_tx.send(MessagingEvent::Conv {
                    conv,
                    contact: sender.to_vec(),
                    kind,
                    id,
                    body: payload,
                });
            }
            // `ack` de groupe : acquitte la livraison par membre
            // (`msg_id` = `id` de la trame emise vers ce membre).
            MsgKind::Ack => {
                if let Ok(msg_id) = <[u8; 16]>::try_from(body) {
                    if let Some(db) = &self.db {
                        let _ = db.with(|c| {
                            dbc::upsert_delivery(
                                c,
                                &dbc::MsgDeliveryRow {
                                    msg_id: msg_id.to_vec(),
                                    member_pk: sender.to_vec(),
                                    status: "acked".into(),
                                    ts: now_secs() as i64,
                                },
                            )
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Corps `gctl` admis (trame deja verifiee + membre verifie
    /// pour les conv connues ; `GroupNew` n'admet que `invite`).
    fn handle_gctl(self: &Arc<Self>, conv: &[u8; 16], sender: &[u8], body: &[u8]) {
        let op = match Gctl::decode_body(body, &self.cfg) {
            Ok(o) => o,
            Err(e) => {
                self.stats.codec.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(error = %e, "corps gctl rejete");
                return;
            }
        };
        match op {
            Gctl::Invite { name, roster, by } => {
                self.on_group_invite(conv, sender, &name, &roster, &by)
            }
            Gctl::Join => self.on_group_join(conv, sender),
            Gctl::Leave => self.on_group_leave(conv, sender),
            Gctl::Roster { members } => self.on_group_roster(conv, sender, &members),
        }
    }

    /// `invite` : cree la conv en `invited` (cap borne) + roster —
    /// admise **seulement d'un contact actif** : un pair confine
    /// `scope='group'` ne peut pas inviter (confinement). L'invite
    /// doit s'y declarer (`by == emetteur`) et le roster porter
    /// l'emetteur et nous-meme.
    fn on_group_invite(
        &self,
        conv: &[u8; 16],
        sender: &[u8],
        name: &str,
        roster: &[[u8; 74]],
        by: &[u8; 74],
    ) {
        if self.peer_scope(sender) == ContactScope::Group
            || by.as_slice() != sender
            || !roster.iter().any(|pk| pk.as_slice() == sender)
            || !roster
                .iter()
                .any(|pk| pk.as_slice() == self.key.public_key().to_bin().as_slice())
        {
            self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Some(db) = &self.db else { return };
        if self
            .db_state(|c| dbc::conversation_state(c, conv))
            .is_none()
        {
            // Conv inconnue : borne `group_pending_cap` sur les
            // invitations en attente de decision.
            let pending = db
                .with(|c| dbc::count_conversations(c, "group", &["invited"]))
                .unwrap_or(0);
            if pending >= self.cfg.group_pending_cap as u64 {
                self.stats.group_rejected.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let now = now_secs() as i64;
            if let Err(e) = db.with(|c| {
                dbc::upsert_conversation(
                    c,
                    &dbc::MsgConversationRow {
                        conv_id: conv.to_vec(),
                        kind: "group".into(),
                        name: name.to_string(),
                        state: "invited".into(),
                        created_at: now,
                        updated_at: now,
                        last_read_ts: 0,
                    },
                )
            }) {
                tracing::warn!(error = %e, "persistance conv invitee");
                return;
            }
            let _ = self.events_tx.send(MessagingEvent::GroupInvite {
                conv: *conv,
                name: name.to_string(),
                by: sender.to_vec(),
            });
        }
        // Roster additif dans les deux cas (creation ou re-invite).
        let now = now_secs() as i64;
        for pk in roster {
            let pk_bin = pk.to_vec();
            self.persist_member(conv, &pk_bin, by.as_slice(), "member", now);
            self.ensure_group_peer_row(&pk_bin);
        }
    }

    /// `join` : confirme le membre (le `left` n'est jamais pose
    /// par un autre — la resurrection additive ne s'applique qu'au
    /// `join` emis **sur son propre lien**).
    fn on_group_join(self: &Arc<Self>, conv: &[u8; 16], sender: &[u8]) {
        let now = now_secs() as i64;
        self.persist_member(conv, sender, sender, "member", now);
        // Synchro additive en retour : le nouveau membre recoit le
        // roster complet (ses membres manquants seront ajoutes).
        self.broadcast_roster(conv, Some(sender));
        let _ = self.events_tx.send(MessagingEvent::Conv {
            conv: *conv,
            contact: sender.to_vec(),
            kind: MsgKind::Gctl,
            id: [0; 16],
            body: b"join".to_vec(),
        });
    }

    /// `leave` : le depart n'est honore que sur le lien signe du
    /// membre lui-meme (anti-forge — deja garanti : la trame est
    /// verifiee contre la cle de `sender`).
    fn on_group_leave(&self, conv: &[u8; 16], sender: &[u8]) {
        if let Some(db) = &self.db {
            let _ = db.with(|c| dbc::set_member_state(c, conv, sender, "left"));
        }
        let _ = self.events_tx.send(MessagingEvent::Conv {
            conv: *conv,
            contact: sender.to_vec(),
            kind: MsgKind::Gctl,
            id: [0; 16],
            body: b"leave".to_vec(),
        });
    }

    /// `roster` : synchro **additive** — ajoute/met a jour des
    /// membres (`joined_at` max), jamais de suppression : un
    /// `left` forge via `roster` est structurellement impossible.
    fn on_group_roster(&self, conv: &[u8; 16], sender: &[u8], members: &[RosterEntry]) {
        if self
            .db_state(|c| dbc::conversation_state(c, conv))
            .as_deref()
            == Some("left")
        {
            return;
        }
        for e in members {
            let pk_bin = e.pk.to_vec();
            self.persist_member(conv, &pk_bin, &e.added_by, "member", e.joined_at as i64);
            self.ensure_group_peer_row(&pk_bin);
            // Swarm du nouveau membre : necessaire au fan-out sortant
            // (un membre qu'on ne resout jamais est inatteignable).
            if pk_bin != sender {
                self.ensure_group_swarm(&pk_bin);
            }
        }
        let _ = self.events_tx.send(MessagingEvent::Conv {
            conv: *conv,
            contact: sender.to_vec(),
            kind: MsgKind::Gctl,
            id: [0; 16],
            body: b"roster".to_vec(),
        });
    }

    /// Persiste un membre de roster (upsert additif DB).
    fn persist_member(
        &self,
        conv: &[u8; 16],
        pk_bin: &[u8],
        added_by: &[u8],
        state: &str,
        ts: i64,
    ) {
        if let Some(db) = &self.db {
            if let Err(e) = db.with(|c| {
                dbc::upsert_member(
                    c,
                    &dbc::MsgMemberRow {
                        conv_id: conv.to_vec(),
                        member_pk: pk_bin.to_vec(),
                        added_by: added_by.to_vec(),
                        state: state.into(),
                        joined_at: ts,
                    },
                )
            }) {
                tracing::warn!(error = %e, "persistance membre groupe");
            }
        }
    }

    /// Ligne `msg_contacts` `scope='group'` pour un pair encore
    /// inconnu (ancre FK des messages + compteurs de lien) —
    /// n'ecrase jamais une ligne existante (un contact garde sa
    /// portee, un `blocked` reste `blocked`).
    fn ensure_group_peer_row(&self, pk_bin: &[u8]) {
        let Some(db) = &self.db else { return };
        let now = now_secs() as i64;
        let _ = db.with(|c| {
            if dbm::get_contact(c, pk_bin)?.is_none() {
                dbm::upsert_contact(
                    c,
                    &dbm::MsgContactRow {
                        public_key: pk_bin.to_vec(),
                        state: "active".into(),
                        send_seq: 0,
                        recv_top: 0,
                        retention_secs: 0,
                        secure_delete: false,
                        alias: String::new(),
                        scope: "group".into(),
                        created_at: now,
                        updated_at: now,
                    },
                )
            } else {
                Ok(())
            }
        });
    }

    /// Joint le swarm de presence d'un membre de groupe (comme un
    /// contact — transport seul ; le confinement reste applicatif).
    fn ensure_group_swarm(&self, pk_bin: &[u8]) {
        let Ok(pk) = LibNaClPublicKey::from_bin(pk_bin) else {
            return;
        };
        self.ensure_contact_swarm(&pk, pk_bin);
    }

    /// Synchro `roster` additive vers les membres lies (`only`
    /// restreint a un destinataire — reponse a un `join`). Envoi
    /// en taches detachees (appelee depuis le chemin de
    /// reception synchrone).
    fn broadcast_roster(self: &Arc<Self>, conv: &[u8; 16], only: Option<&[u8]>) {
        let Some(db) = &self.db else { return };
        let Ok(rows) = db.with(|c| dbc::list_members(c, conv)) else {
            return;
        };
        let own = self.key.public_key().to_bin();
        let mut targets: Vec<Vec<u8>> = Vec::new();
        let mut members: Vec<RosterEntry> = Vec::new();
        for m in &rows {
            if m.state != "member" {
                continue;
            }
            let (Ok(pk), Ok(by)) = (
                <[u8; 74]>::try_from(m.member_pk.as_slice()),
                <[u8; 74]>::try_from(m.added_by.as_slice()),
            ) else {
                continue;
            };
            members.push(RosterEntry {
                pk,
                added_by: by,
                joined_at: m.joined_at.max(0) as u64,
            });
            if m.member_pk != own && only.is_none_or(|o| o == m.member_pk) {
                targets.push(m.member_pk.clone());
            }
        }
        if members.is_empty() {
            return;
        }
        let body = Gctl::Roster { members }.encode();
        for pk_bin in targets {
            let Some(cid) = self.contact_circuit(&pk_bin) else {
                continue;
            };
            let svc = self.clone();
            let b = body.clone();
            let cv = *conv;
            tokio::spawn(async move {
                let _ = svc.send_frame_v2(&pk_bin, cid, cv, MsgKind::Gctl, b).await;
            });
        }
    }

    /// Scelle et emet une trame **v2** (`conv` signee) sur le
    /// circuit du pair — pas de persistance `msg` (la ligne de
    /// conversation et la livraison par membre sont aux appelants).
    async fn send_frame_v2(
        &self,
        pk_bin: &[u8],
        cid: u32,
        conv: [u8; 16],
        kind: MsgKind,
        body: Vec<u8>,
    ) -> Result<[u8; 16]> {
        let (send_key, seq) = {
            let circuits = self.circuits.lock().unwrap_or_else(|e| e.into_inner());
            let mut contacts = self.contacts.lock().unwrap_or_else(|e| e.into_inner());
            let (Some(binding), Some(contact)) = (circuits.get(&cid), contacts.get_mut(pk_bin))
            else {
                return Err(CoreError::InvalidState("messagerie : liaison disparue"));
            };
            let seq = contact.send_seq;
            contact.send_seq += 1;
            (binding.keys.send, seq)
        };
        let frame = Frame::new_in_conv(kind, conv, seq, now_secs(), body);
        let wire = frame
            .seal(&self.key, &send_key, &self.cfg)
            .map_err(|e| CoreError::State(format!("seal trame v2: {e}")))?;
        self.persist_seqs(pk_bin);
        if let Err(e) = self
            .tunnel
            .send_data(cid, &unspecified(), &unspecified(), &wire)
            .await
        {
            self.unbind_circuit(cid);
            return Err(CoreError::State(format!("send_data messagerie: {e}")));
        }
        if kind == MsgKind::Hello {
            if let Some(c) = self
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(pk_bin)
            {
                c.greeted = true;
            }
        }
        Ok(frame.id)
    }

    /// `hello` v2 (`pk‖caps`, `conv` directe du lien) — precede le
    /// premier `gctl`/`msg` vers un pair `scope='group'`.
    async fn greet_v2(&self, pk_bin: &[u8], cid: u32) -> Result<()> {
        let conv = self.direct_conv_id(pk_bin);
        let body = hello::encode_hello_v2(&self.key.public_key().to_bin(), hello::HELLO_CAP_GROUPS);
        self.send_frame_v2(pk_bin, cid, conv, MsgKind::Hello, body)
            .await
            .map(|_| ())
    }

    /// Emission v2 vers un membre : `hello` v2 d'abord si la
    /// liaison vient d'etre liee, puis la trame `conv` demandee.
    async fn send_to_member(
        &self,
        conv: [u8; 16],
        member: &[u8],
        kind: MsgKind,
        body: Vec<u8>,
    ) -> Result<[u8; 16]> {
        let Some(cid) = self.contact_circuit(member) else {
            return Err(CoreError::InvalidState("messagerie : membre hors ligne"));
        };
        if !self.is_greeted(member) {
            self.greet_v2(member, cid).await?;
        }
        self.send_frame_v2(member, cid, conv, kind, body).await
    }

    /// Cree un groupe : conv aleatoire, roster initial (nous +
    /// les invites — tous des contacts `Active` requis) et `gctl
    /// invite` vers chaque membre lie. Online-only : un invite
    /// hors ligne recevra le roster a sa prochaine liaison.
    pub async fn group_create(
        self: &Arc<Self>,
        name: &str,
        member_pks: &[Vec<u8>],
    ) -> Result<[u8; 16]> {
        if !self.cfg.groups_enabled {
            return Err(CoreError::InvalidState("messagerie : groupes desactives"));
        }
        if name.is_empty() || name.len() > self.cfg.group_name_max_len {
            return Err(CoreError::InvalidState(
                "messagerie : nom de groupe hors borne",
            ));
        }
        if member_pks.is_empty() || member_pks.len() + 1 > self.cfg.group_max_members {
            return Err(CoreError::InvalidState(
                "messagerie : membres de groupe hors borne",
            ));
        }
        for pk_bin in member_pks {
            self.require_state(pk_bin, ContactState::Active)?;
            if self.peer_scope(pk_bin) == ContactScope::Group {
                return Err(CoreError::InvalidState(
                    "messagerie : seuls des contacts sont invitables",
                ));
            }
        }
        if let Some(db) = &self.db {
            let n = db
                .with(|c| dbc::count_conversations(c, "group", &["invited", "active"]))
                .unwrap_or(0);
            if n >= self.cfg.group_max_convs as u64 {
                return Err(CoreError::InvalidState(
                    "messagerie : nombre de groupes sature",
                ));
            }
        }
        let conv = conv::random_conv();
        let own = self.key.public_key().to_bin();
        let now = now_secs() as i64;
        self.ensure_group_peer_row(&own);
        if let Some(db) = &self.db {
            db.with(|c| {
                dbc::upsert_conversation(
                    c,
                    &dbc::MsgConversationRow {
                        conv_id: conv.to_vec(),
                        kind: "group".into(),
                        name: name.to_string(),
                        state: "active".into(),
                        created_at: now,
                        updated_at: now,
                        last_read_ts: 0,
                    },
                )
            })?;
        }
        self.persist_member(&conv, &own, &own, "member", now);
        let mut roster: Vec<[u8; 74]> = Vec::with_capacity(member_pks.len() + 1);
        let own_arr: [u8; 74] = own
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::State("pk_bin inattendue".into()))?;
        roster.push(own_arr);
        for pk_bin in member_pks {
            self.persist_member(&conv, pk_bin, &own, "member", now);
            if let Ok(arr) = <[u8; 74]>::try_from(pk_bin.as_slice()) {
                roster.push(arr);
            }
        }
        let body = Gctl::Invite {
            name: name.to_string(),
            roster,
            by: own_arr,
        }
        .encode();
        for pk_bin in member_pks {
            let _ = self
                .send_to_member(conv, pk_bin, MsgKind::Gctl, body.clone())
                .await;
        }
        let _ = self.events_tx.send(MessagingEvent::Conv {
            conv,
            contact: own,
            kind: MsgKind::Gctl,
            id: [0; 16],
            body: b"create".to_vec(),
        });
        Ok(conv)
    }

    /// Invite un contact actif dans un groupe existant (membres
    /// peuvent inviter **leurs propres contacts** — la regle est
    /// symetrique des deux cotes).
    pub async fn group_invite(self: &Arc<Self>, conv: &[u8; 16], pk_bin: &[u8]) -> Result<()> {
        self.require_group_active(conv)?;
        self.require_state(pk_bin, ContactState::Active)?;
        if self.peer_scope(pk_bin) == ContactScope::Group {
            return Err(CoreError::InvalidState(
                "messagerie : seuls des contacts sont invitables",
            ));
        }
        if let Some(db) = &self.db {
            if db.with(|c| dbc::member_state(c, conv, pk_bin))?.as_deref() == Some("member") {
                return Err(CoreError::InvalidState("messagerie : deja membre"));
            }
            if db.with(|c| dbc::count_active_members(c, conv))? >= self.cfg.group_max_members as u64
            {
                return Err(CoreError::InvalidState("messagerie : groupe plein"));
            }
        }
        let own = self.key.public_key().to_bin();
        self.persist_member(conv, pk_bin, &own, "member", now_secs() as i64);
        self.send_roster_invite(conv, pk_bin).await
    }

    /// `gctl invite` vers un membre precis (roster actuel + nom).
    async fn send_roster_invite(&self, conv: &[u8; 16], target: &[u8]) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        let row = db.with(|c| dbc::get_conversation(c, conv))?;
        let Some(conv_row) = row else { return Ok(()) };
        let members = db.with(|c| dbc::list_members(c, conv))?;
        let own = self.key.public_key().to_bin();
        let own_arr: [u8; 74] = own
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::State("pk_bin inattendue".into()))?;
        let roster: Vec<[u8; 74]> = members
            .iter()
            .filter(|m| m.state == "member")
            .filter_map(|m| m.member_pk.as_slice().try_into().ok())
            .collect();
        let body = Gctl::Invite {
            name: conv_row.name,
            roster,
            by: own_arr,
        }
        .encode();
        self.send_to_member(*conv, target, MsgKind::Gctl, body)
            .await
            .map(|_| ())
    }

    /// Accepte une invitation : conv `invited` → `active` + `gctl
    /// join` vers l'invitant (son lien est lie — c'est un contact).
    pub async fn group_accept(self: &Arc<Self>, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else {
            return Err(CoreError::InvalidState("messagerie : sans persistance"));
        };
        if db.with(|c| dbc::conversation_state(c, conv))?.as_deref() != Some("invited") {
            return Err(CoreError::InvalidState(
                "messagerie : conversation non invitee",
            ));
        }
        // L'invitant = `added_by` de notre propre ligne membre.
        let own = self.key.public_key().to_bin();
        let inviter_pk = db
            .with(|c| dbc::list_members(c, conv))?
            .into_iter()
            .find(|m| m.member_pk == own)
            .map(|m| m.added_by)
            .unwrap_or_default();
        db.with(|c| dbc::set_conversation_state(c, conv, "active", now_secs() as i64))?;
        self.persist_member(conv, &own, &inviter_pk, "member", now_secs() as i64);
        if !inviter_pk.is_empty() {
            let _ = self
                .send_to_member(*conv, &inviter_pk, MsgKind::Gctl, Gctl::Join.encode())
                .await;
        }
        self.broadcast_roster(conv, None);
        Ok(())
    }

    /// Refuse une invitation : `invited` → `left` + `gctl leave`
    /// vers l'invitant (historique conserve — jamais de remove).
    pub async fn group_decline(self: &Arc<Self>, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else {
            return Err(CoreError::InvalidState("messagerie : sans persistance"));
        };
        if db.with(|c| dbc::conversation_state(c, conv))?.as_deref() != Some("invited") {
            return Err(CoreError::InvalidState(
                "messagerie : conversation non invitee",
            ));
        }
        db.with(|c| dbc::set_conversation_state(c, conv, "left", now_secs() as i64))?;
        let own = self.key.public_key().to_bin();
        let inviter = db
            .with(|c| dbc::list_members(c, conv))?
            .into_iter()
            .find(|m| m.member_pk == own)
            .map(|m| m.added_by)
            .unwrap_or_default();
        if !inviter.is_empty() {
            let _ = self
                .send_to_member(*conv, &inviter, MsgKind::Gctl, Gctl::Leave.encode())
                .await;
        }
        Ok(())
    }

    /// Quitte un groupe : `left` + `gctl leave` en fan-out vers
    /// les membres lies (historique conserve — suppression reelle
    /// seulement via `conversation_delete`).
    pub async fn group_leave(self: &Arc<Self>, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else {
            return Err(CoreError::InvalidState("messagerie : sans persistance"));
        };
        db.with(|c| dbc::set_conversation_state(c, conv, "left", now_secs() as i64))?;
        let own = self.key.public_key().to_bin();
        db.with(|c| dbc::set_member_state(c, conv, &own, "left"))?;
        for pk_bin in self.member_pks(conv) {
            if pk_bin == own {
                continue;
            }
            let _ = self
                .send_to_member(*conv, &pk_bin, MsgKind::Gctl, Gctl::Leave.encode())
                .await;
        }
        Ok(())
    }

    /// Message de groupe : `mid` applicatif + fan-out par membre
    /// lie ; livraison par membre dans `msg_delivery`.
    pub async fn group_send(self: &Arc<Self>, conv: &[u8; 16], body: Vec<u8>) -> Result<[u8; 16]> {
        self.require_group_active(conv)?;
        let mid: [u8; 16] = rand::random();
        let wire_body = gmsg::encode_gmsg(&mid, &body);
        if wire_body.len() > self.cfg.max_body_len {
            return Err(CoreError::InvalidState("messagerie : corps hors borne"));
        }
        let own = self.key.public_key().to_bin();
        // Ligne de conversation (une par message — la livraison
        // par membre vit dans `msg_delivery`).
        let mut row = Self::msg_row(&own, "out", 0, now_secs(), &body, "sent", &mid);
        row.conv_id = conv.to_vec();
        row.author_pk = Some(own.clone());
        row.mid = Some(mid.to_vec());
        self.persist_message(row);
        for pk_bin in self.member_pks(conv) {
            if pk_bin == own {
                continue;
            }
            let status = match self
                .send_to_member(*conv, &pk_bin, MsgKind::Msg, wire_body.clone())
                .await
            {
                Ok(frame_id) => {
                    // La livraison suit l'`id` de la trame emise
                    // vers CE membre (cible de son `ack`).
                    if let Some(db) = &self.db {
                        let _ = db.with(|c| {
                            dbc::upsert_delivery(
                                c,
                                &dbc::MsgDeliveryRow {
                                    msg_id: frame_id.to_vec(),
                                    member_pk: pk_bin.clone(),
                                    status: "sent".into(),
                                    ts: now_secs() as i64,
                                },
                            )
                        });
                    }
                    continue;
                }
                Err(_) => "failed",
            };
            if let Some(db) = &self.db {
                let _ = db.with(|c| {
                    dbc::upsert_delivery(
                        c,
                        &dbc::MsgDeliveryRow {
                            msg_id: mid.to_vec(),
                            member_pk: pk_bin.clone(),
                            status: status.into(),
                            ts: now_secs() as i64,
                        },
                    )
                });
            }
        }
        Ok(mid)
    }

    /// Conv `active` requise (et notre appartenance).
    fn require_group_active(&self, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else {
            return Err(CoreError::InvalidState("messagerie : sans persistance"));
        };
        if db.with(|c| dbc::conversation_state(c, conv))?.as_deref() != Some("active") {
            return Err(CoreError::InvalidState(
                "messagerie : conversation inactive",
            ));
        }
        Ok(())
    }

    /// `pk_bin` des membres actifs d'un groupe.
    fn member_pks(&self, conv: &[u8; 16]) -> Vec<Vec<u8>> {
        self.db
            .as_ref()
            .and_then(|db| db.with(|c| dbc::list_active_members(c, conv)).ok())
            .unwrap_or_default()
    }

    // ── API conversations (lecture pour REST/UI) ─────────────

    /// Liste des conversations avec non lus / dernier horodatage.
    pub fn conversation_list(&self) -> Result<Vec<dbc::MsgConversationListRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        Ok(db.with(dbc::list_conversations)?)
    }

    /// Messages d'une conversation (ordre total local).
    pub fn conversation_messages(
        &self,
        conv: &[u8; 16],
        limit: u32,
    ) -> Result<Vec<dbm::MsgMessageRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        Ok(db.with(|c| dbm::list_messages_by_conv(c, conv, limit))?)
    }

    /// Roster d'un groupe.
    pub fn conversation_members(&self, conv: &[u8; 16]) -> Result<Vec<dbc::MsgMemberRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        Ok(db.with(|c| dbc::list_members(c, conv))?)
    }

    /// Livraisons par membre d'un message de groupe.
    pub fn conversation_delivery(&self, msg_id: &[u8; 16]) -> Result<Vec<dbc::MsgDeliveryRow>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        Ok(db.with(|c| dbc::list_delivery(c, msg_id))?)
    }

    /// Etat d'une conversation (`invited|active|left`).
    pub fn conversation_state(&self, conv: &[u8; 16]) -> Option<String> {
        self.db_state(|c| dbc::conversation_state(c, conv))
    }

    /// Marqueur de lecture (badge non lu).
    pub fn conversation_mark_read(&self, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        Ok(db.with(|c| dbc::mark_read(c, conv, now_secs() as i64))?)
    }

    /// Suppression reelle d'une conversation (cascades).
    pub fn conversation_delete(&self, conv: &[u8; 16]) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        Ok(db.with(|c| dbc::delete_conversation(c, conv))?)
    }

    /// Presence : maintient des points d'introduction sur notre
    /// swarm a `ip_check_interval` (rapide — un premier essai peut
    /// echouer tant que les pairs ne sont pas verifies) et re-publie
    /// l'annonce DHT a `announce_interval` (pendant de
    /// `ensure_introduction_points`+`reannounce` du moniteur de
    /// swarms BitTorrent).
    fn spawn_presence_monitor(self: &Arc<Self>) {
        let (tx, mut rx_stop) = watch::channel(false);
        self.stops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
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

    /// `make_service_db` avec config explicite.
    async fn make_service_db_cfg(db: Arc<Database>, cfg: MessagingConfig) -> Arc<MessagingService> {
        let (tunnel, key) = bare_tunnel().await;
        MessagingService::start(tunnel, key, cfg, 0, Some(db))
    }

    /// Injecte un circuit non lie (`contact: None`) — le repondant
    /// avant identification.
    fn unbound(svc: &MessagingService, cid: u32, keys: &MessagingKeys) {
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
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
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                cid,
                CircuitBinding {
                    keys: keys.clone(),
                    contact: Some(pk_bin.clone()),
                },
            );
        let mut c = Contact::active(peer.public_key(), &svc.cfg);
        c.circuit = Some(cid);
        c.greeted = true;
        svc.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pk_bin.clone(), c);
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
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                12,
                CircuitBinding {
                    keys: keys.clone(),
                    contact: Some(pk_bin.clone()),
                },
            );
        svc.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&pk_bin)
            .expect("contact")
            .circuit = Some(12);
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&11);

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
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
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
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
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
        // La notification `accept` echoue sur le circuit factice du
        // banc (inexistant cote tunnel) — la deliaison emet un `Link`
        // intermediaire avant le `Bound` attendu.
        svc.accept_contact(&pk_bin).await.unwrap();
        loop {
            let ev = events.try_recv().expect("Bound attendu");
            if matches!(ev, MessagingEvent::Bound { .. }) {
                break;
            }
        }
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
            !svc.circuits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&32),
            "circuit du bloque delie"
        );
        // `ensure_contact_swarm` ne joint pas le swarm d'un bloque.
        let mh = messaging_hash(&peer.public_key());
        let _ = svc.ensure_contact_swarm(&peer.public_key(), &pk_bin);
        assert!(!svc
            .swarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&mh));

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
            .unwrap_or_else(|e| e.into_inner())
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
        // `v=3` : suffixe present mais version refusee tot (v1/v2
        // sont connues — la v2 est validee plus loin, sur `conv`).
        let bad = b"d4:body0:2:id16:0123456789abcdef3:seqi0e3:sig64:00000000000000000000000000000000000000000000000000000000000000002:tsi1e4:type3:msg1:vi3ee";
        assert!(matches!(
            preflight(bad, &cfg),
            Err(MessagingError::UnknownVersion(3))
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
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&pk_bin)
            .expect("contact")
            .circuit = None;
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&0);
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
                let mut contacts = svc.contacts.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Pseudonyme local : pose/persiste au restart/efface ; refuse
    /// sur contact inconnu ou trop long. La colonne survit aux
    /// transitions d'etat (`upsert_contact` hors champ).
    #[tokio::test]
    async fn alias_contact_persiste_et_borne() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db.clone()).await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = bind(
            &svc,
            0,
            &peer,
            &MessagingKeys {
                send: [1u8; 32],
                recv: [2u8; 32],
            },
        );

        // Inconnu refuse.
        assert!(svc.set_alias(&[9u8; 64], "bob").is_err());
        // Trop long refuse (> MAX_CONTACT_ALIAS_CHARS caracteres).
        assert!(svc.set_alias(&pk_bin, &"x".repeat(65)).is_err());

        svc.set_alias(&pk_bin, "  alice  ").unwrap();
        assert_eq!(svc.contact_alias(&pk_bin).as_deref(), Some("alice"));

        // Persiste au restart sur la meme base.
        let svc2 = make_service_db(db).await;
        assert_eq!(svc2.contact_alias(&pk_bin).as_deref(), Some("alice"));

        // Vide = efface (retour a la cle abregee cote UI).
        svc2.set_alias(&pk_bin, " ").unwrap();
        assert_eq!(svc2.contact_alias(&pk_bin), None);
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

    /// Attend que `send_seq` du contact atteigne `n` (chaque trame
    /// emise en consomme un — observable sans socket).
    async fn wait_send_seq(svc: &MessagingService, pk: &[u8], n: u64) {
        for _ in 0..100 {
            let s = svc
                .contacts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(pk)
                .map(|c| c.send_seq)
                .unwrap_or(0);
            if s >= n {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("send_seq n'atteint pas {n}");
    }

    /// Re-ACK sur retransmission : un `msg` deja vu (ACK precedent
    /// perdu en transit) doit declencher la re-emission de l'ACK —
    /// sans doublon d'historique ni d'evenement. Avant, la trame
    /// etait absorbee par la dedup et l'emetteur marquait `failed`
    /// un message pourtant bien recu.
    #[tokio::test]
    async fn retransmission_reemet_ack_sans_dupliquer() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = bind(&svc, 11, &peer, &keys);
        let mut events = svc.subscribe();

        let w = wire(&peer, 0, &keys.recv, b"recu");
        svc.handle_incoming(11, &keys, &w);
        let _ = events.try_recv(); // Frame de la 1re livraison
        wait_send_seq(&svc, &pk_bin, 1).await;

        // Retransmission filaire identique (meme `id`).
        svc.handle_incoming(11, &keys, &w);
        wait_send_seq(&svc, &pk_bin, 2).await;

        assert_eq!(
            svc.history(&pk_bin, 10).unwrap().len(),
            1,
            "pas de doublon en historique"
        );
        // `Bound` est attendu : l'ACK du 1er envoi echoue sur le
        // tunnel nu (circuit absent) -> `unbind_circuit` -> la
        // retransmission re-lie. Interdit : un `Frame` en doublon.
        while let Ok(ev) = events.try_recv() {
            assert!(
                !matches!(ev, MessagingEvent::Frame { .. }),
                "evenement Frame en doublon"
            );
        }
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

    /// Gate confiance `flagged` : un emetteur a score < 0 (flague
    /// par un curateur suivi, `kind=identity`) devient `Blocked`
    /// sans `pending` ni evenement `Consent` — la demande
    /// n'atteint pas l'utilisateur.
    #[tokio::test]
    async fn consent_gate_flagged_bloque_sans_pending() {
        let svc = make_service_cfg(MessagingConfig {
            consent_gate_flagged: true,
            ..MessagingConfig::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = peer.public_key().to_bin();
        let flagged = pk_bin.clone();
        svc.set_trust_lookup(Arc::new(
            move |pk| {
                if pk == flagged.as_slice() {
                    -1
                } else {
                    0
                }
            },
        ));
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        unbound(&svc, 60, &keys);
        let mut events = svc.subscribe();

        svc.handle_incoming(60, &keys, &hello_wire(&peer, 0, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Blocked));
        assert!(events.try_recv().is_err(), "aucun Consent pour un flague");
        assert_eq!(svc.stats.consent_blocked.load(Ordering::Relaxed), 1);
    }

    /// Gate `endorsed` : score > 0 → `Active` direct (consentement
    /// delegue aux curateurs suivis) ; les trames passent aussitot.
    #[tokio::test]
    async fn consent_gate_endorsed_admet_active() {
        let svc = make_service_cfg(MessagingConfig {
            consent_gate_endorsed: true,
            ..MessagingConfig::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = peer.public_key().to_bin();
        let endorsed = pk_bin.clone();
        svc.set_trust_lookup(Arc::new(
            move |pk| {
                if pk == endorsed.as_slice() {
                    2
                } else {
                    0
                }
            },
        ));
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        unbound(&svc, 61, &keys);

        svc.handle_incoming(61, &keys, &hello_wire(&peer, 0, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Active));
        assert_eq!(svc.stats.consent_auto_accepted.load(Ordering::Relaxed), 1);
        // Le circuit est lie : une trame verifiee du contact est
        // livree immediatement (pas de `pending_drop`).
        svc.handle_incoming(61, &keys, &wire(&peer, 1, &keys.recv, b"salut"));
        assert_eq!(svc.stats.pending_drop.load(Ordering::Relaxed), 0);
    }

    /// Gate dette : `consent_gate_ledger` + `ledger_enforce` — un
    /// pair dont le deficit depasse `max_deficit_bytes` ne peut pas
    /// ouvrir de `pending` (refuse silencieusement, sans etat).
    #[tokio::test]
    async fn consent_gate_ledger_refuse_debiteur() {
        let svc = make_service_cfg(MessagingConfig {
            consent_gate_ledger: true,
            ..MessagingConfig::default()
        })
        .await;
        let peer = LibNaClSecretKey::generate();
        let pk_bin = peer.public_key().to_bin();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };

        // Sans enforce : le debiteur passe en pending comme tout
        // inconnu (mesure seule, jamais d'exclusion).
        svc.tunnel
            .ledger
            .note_served(&pk_bin, svc.tunnel.ledger.max_deficit_bytes() + 1);
        unbound(&svc, 62, &keys);
        svc.handle_incoming(62, &keys, &hello_wire(&peer, 0, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), Some(ContactState::Pending));
        svc.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&pk_bin);

        // Enforce : le meme deficit refuse l'admission.
        svc.tunnel.ledger.set_enforce(true);
        unbound(&svc, 63, &keys);
        svc.handle_incoming(63, &keys, &hello_wire(&peer, 1, &keys.recv));
        assert_eq!(svc.contact_state(&pk_bin), None);
        assert_eq!(svc.stats.consent_ledger_refused.load(Ordering::Relaxed), 1);
    }

    /// Coffre portable : export `OBV1` chiffre pour soi, import sur
    /// un service **neuf partageant la meme identite** (ADR-0016) —
    /// contacts et pseudonymes restaures, blob illisible pour un
    /// tiers (autre cle → AEAD refuse).
    #[tokio::test]
    async fn vault_export_import_restaure_contacts() {
        let (tunnel_a, key) = bare_tunnel().await;
        let db_a = Arc::new(Database::memory().unwrap());
        let svc_a = MessagingService::start(
            tunnel_a,
            key.clone(),
            MessagingConfig::default(),
            0,
            Some(db_a),
        );
        let friend = LibNaClSecretKey::generate();
        let pk_bin = bind(
            &svc_a,
            0,
            &friend,
            &MessagingKeys {
                send: [1u8; 32],
                recv: [2u8; 32],
            },
        );
        svc_a.set_alias(&pk_bin, "alice").unwrap();

        let blob = svc_a.export_vault();
        assert!(blob.starts_with(b"OBV1"));

        // « Nouveau device » : autre tunnel, autre base — meme cle
        // d'identite.
        let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_run = ep.clone();
        tokio::spawn(async move {
            let _ = ep_run.run().await;
        });
        let tunnel_b = TunnelCommunity::new_with_id(
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
        let db_b = Arc::new(Database::memory().unwrap());
        let svc_b =
            MessagingService::start(tunnel_b, key, MessagingConfig::default(), 0, Some(db_b));
        assert_eq!(svc_b.contact_state(&pk_bin), None);

        let restored = svc_b.import_vault(&blob).unwrap();
        assert_eq!(restored, 1);
        assert_eq!(svc_b.contact_state(&pk_bin), Some(ContactState::Active));
        assert_eq!(svc_b.contact_alias(&pk_bin).as_deref(), Some("alice"));

        // Un tiers (autre cle) ne peut pas lire le coffre.
        let svc_c = make_service(0).await.0;
        assert!(svc_c.import_vault(&blob).is_err());
        // Magic absent → erreur propre, pas de panic.
        assert!(svc_b.import_vault(b"XXXX").is_err());
    }

    #[tokio::test]
    async fn vault_import_borne_et_ne_reactive_pas_un_bloque() {
        // Blob hors borne → refuse avant tout decrypt.
        let (svc, _tmp) = make_service(0).await;
        let huge = [b"OBV1".as_slice(), &vec![0u8; VAULT_BLOB_MAX]].concat();
        assert!(svc.import_vault(&huge).is_err());

        // Un contact deja present (bloque) n'est pas re-active par
        // l'import : l'existant local prime toujours l'export.
        let (tunnel_a, key) = bare_tunnel().await;
        let svc_a =
            MessagingService::start(tunnel_a, key.clone(), MessagingConfig::default(), 0, None);
        let friend = LibNaClSecretKey::generate();
        let pk_bin = bind(
            &svc_a,
            0,
            &friend,
            &MessagingKeys {
                send: [3u8; 32],
                recv: [4u8; 32],
            },
        );
        let blob = svc_a.export_vault();

        let (svc_b, _tmp_b) = make_service(0).await;
        // `svc_b` a une autre identite : import impossible (AEAD).
        assert!(svc_b.import_vault(&blob).is_err());
        // Meme identite sur un autre service : contact deja bloque
        // localement → le `Active` du coffre ne le reactive pas.
        let ep = UdpEndpoint::bind("127.0.0.1:0").await.unwrap();
        let ep_run = ep.clone();
        tokio::spawn(async move {
            let _ = ep_run.run().await;
        });
        let tunnel_b = TunnelCommunity::new_with_id(
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
        let svc_c = MessagingService::start(tunnel_b, key, MessagingConfig::default(), 0, None);
        let mut preexisting = Contact::active(friend.public_key(), &svc_c.cfg);
        preexisting.state = ContactState::Blocked;
        svc_c
            .contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pk_bin.clone(), preexisting);
        assert_eq!(svc_c.import_vault(&blob).unwrap(), 0);
        assert_eq!(svc_c.contact_state(&pk_bin), Some(ContactState::Blocked));
    }

    /// Reception d'un `accept` : les `Msg` sortants encore `sent`
    /// (emis pendant que le repondant nous gardait `pending` — drop
    /// silencieux, sans NACK) sont re-emis sous leur `id` d'origine.
    /// Ici le `cid` n'existe pas dans la table du tunnel : la
    /// tentative se solde par `failed` + `Undeliverable`, ce qui
    /// prouve le declenchement ; le chemin heureux bout-en-bout est
    /// couvert par `live_messaging_e2e_vault_migration`.
    #[tokio::test]
    async fn accept_reemet_les_messages_non_acquittes() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [9u8; 32],
            recv: [8u8; 32],
        };
        let pk_bin = bind(&svc, 77, &peer, &keys);
        let id = [42u8; 16];
        svc.persist_message(MessagingService::msg_row(
            &pk_bin, "out", 0, 1, b"perdu", "sent", &id,
        ));
        let mut events = svc.subscribe();

        let accept = Frame::new(MsgKind::Accept, 1, 1, Vec::new())
            .seal(&peer, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(77, &keys, &accept);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let hist = svc.history(&pk_bin, 10).unwrap();
            if hist.iter().any(|r| r.id == id && r.status == "failed") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "le `sent` n'a jamais ete retouche apres `accept`"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // L'echec de la re-emission est signale (circuit mort).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match events.try_recv() {
                Ok(MessagingEvent::Undeliverable { id: ev_id, .. }) if ev_id == id => break,
                _ => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Undeliverable jamais emis pour la re-emission"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    /// Reception d'un `reject` : les `Msg` sortants encore `sent`
    /// ne seront jamais admis (le repondant a oublie la liaison) —
    /// ils basculent `failed` et `Undeliverable` est emis.
    #[tokio::test]
    async fn reject_marque_les_messages_non_acquittes() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [7u8; 32],
            recv: [6u8; 32],
        };
        let pk_bin = bind(&svc, 78, &peer, &keys);
        let id = [43u8; 16];
        svc.persist_message(MessagingService::msg_row(
            &pk_bin, "out", 0, 1, b"jamais", "sent", &id,
        ));
        let mut events = svc.subscribe();

        let reject = Frame::new(MsgKind::Reject, 1, 1, Vec::new())
            .seal(&peer, &keys.recv, &MessagingConfig::default())
            .unwrap();
        svc.handle_incoming(78, &keys, &reject);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let hist = svc.history(&pk_bin, 10).unwrap();
            if hist.iter().any(|r| r.id == id && r.status == "failed") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "le `sent` n'est jamais passe `failed` apres `reject`"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match events.try_recv() {
                Ok(MessagingEvent::Undeliverable { id: ev_id, .. }) if ev_id == id => break,
                _ => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "Undeliverable jamais emis apres `reject`"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    // ── ADR-0019 : conversations de groupe ──────────────────

    /// Trame v2 filaire (`conv` signee) d'un pair.
    fn wire_v2(
        peer: &LibNaClSecretKey,
        seq: u64,
        key: &[u8; 32],
        conv: [u8; 16],
        kind: MsgKind,
        body: Vec<u8>,
    ) -> Vec<u8> {
        Frame::new_in_conv(kind, conv, seq, 1, body)
            .seal(peer, key, &MessagingConfig::default())
            .unwrap()
    }

    /// Conv de groupe persistante (`state` donne) + notre ligne
    /// membre (ancre FK des messages `out`).
    fn seed_conv(svc: &MessagingService, conv: [u8; 16], state: &str) {
        let own = svc.key.public_key().to_bin();
        svc.ensure_group_peer_row(&own);
        let db = svc.db.as_ref().expect("db");
        db.with(|c| {
            dbc::upsert_conversation(
                c,
                &dbc::MsgConversationRow {
                    conv_id: conv.to_vec(),
                    kind: "group".into(),
                    name: "g".into(),
                    state: state.into(),
                    created_at: 1,
                    updated_at: 1,
                    last_read_ts: 0,
                },
            )?;
            dbc::upsert_member(
                c,
                &dbc::MsgMemberRow {
                    conv_id: conv.to_vec(),
                    member_pk: own.clone(),
                    added_by: own,
                    state: "member".into(),
                    joined_at: 1,
                },
            )
        })
        .unwrap();
    }

    /// Lie un pair `scope='group'` : Contact confine + circuit +
    /// lignes DB (contact `scope='group'` + membre du roster).
    fn bind_group_member(
        svc: &MessagingService,
        cid: u32,
        conv: &[u8; 16],
        peer: &LibNaClSecretKey,
        keys: &MessagingKeys,
    ) -> Vec<u8> {
        let pk_bin = peer.public_key().to_bin();
        svc.circuits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                cid,
                CircuitBinding {
                    keys: keys.clone(),
                    contact: Some(pk_bin.clone()),
                },
            );
        let mut c = Contact::group_scoped(peer.public_key(), &svc.cfg);
        c.circuit = Some(cid);
        c.greeted = true;
        svc.contacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pk_bin.clone(), c);
        svc.ensure_group_peer_row(&pk_bin);
        svc.persist_member(conv, &pk_bin, &pk_bin, "member", 1);
        pk_bin
    }

    /// `gctl invite` d'un **contact actif** : conv `invited`,
    /// roster pose, evenement `GroupInvite` emis.
    #[tokio::test]
    async fn group_invite_d_un_contact_actif() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = bind(&svc, 90, &peer, &keys);
        let mut events = svc.subscribe();
        let conv = conv::random_conv();
        let own = svc.key.public_key().to_bin();
        let invite = Gctl::Invite {
            name: "canal".into(),
            roster: vec![
                pk_bin.as_slice().try_into().unwrap(),
                own.as_slice().try_into().unwrap(),
            ],
            by: pk_bin.as_slice().try_into().unwrap(),
        }
        .encode();
        let w = wire_v2(&peer, 0, &keys.recv, conv, MsgKind::Gctl, invite);
        svc.handle_incoming(90, &keys, &w);
        assert_eq!(svc.conversation_state(&conv).as_deref(), Some("invited"));
        let members = svc.conversation_members(&conv).unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.iter().all(|m| m.state == "member"));
        match events.try_recv() {
            Ok(MessagingEvent::GroupInvite { conv: c, name, by }) => {
                assert_eq!((c, name.as_str(), by), (conv, "canal", pk_bin));
            }
            other => panic!("GroupInvite attendu, recu {other:?}"),
        }
        // `accept` -> `active` (le `join` part en best effort).
        svc.group_accept(&conv).await.unwrap();
        assert_eq!(svc.conversation_state(&conv).as_deref(), Some("active"));
    }

    /// `gctl invite` d'un pair `scope='group'` : rejetee —
    /// l'invitation n'est admise que d'un contact actif (le membre
    /// confine ne peut pas inviter vers d'autres convs).
    #[tokio::test]
    async fn group_invite_d_un_pair_confine_rejetee() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        // Le pair est deja membre d'un groupe A.
        let conv_a = conv::random_conv();
        seed_conv(&svc, conv_a, "active");
        let pk_bin = bind_group_member(&svc, 91, &conv_a, &peer, &keys);
        // Il tente d'inviter vers une conv B inconnue.
        let conv_b = conv::random_conv();
        let own = svc.key.public_key().to_bin();
        let invite = Gctl::Invite {
            name: "autre".into(),
            roster: vec![
                pk_bin.as_slice().try_into().unwrap(),
                own.as_slice().try_into().unwrap(),
            ],
            by: pk_bin.as_slice().try_into().unwrap(),
        }
        .encode();
        let w = wire_v2(&peer, 0, &keys.recv, conv_b, MsgKind::Gctl, invite);
        svc.handle_incoming(91, &keys, &w);
        assert_eq!(svc.conversation_state(&conv_b), None);
        assert_eq!(svc.stats.group_rejected.load(Ordering::Relaxed), 1);
    }

    /// Dedup applicative `(conv, author, mid)` : le meme message
    /// re-emis avec un `id` de trame different (reouverture) n'est
    /// pas duplique ; un `mid` different est livre.
    #[tokio::test]
    async fn group_msg_dedup_par_mid() {
        let db = Arc::new(Database::memory().unwrap());
        // Debit illimite : la dedup `mid` doit etre l'oracle, pas
        // le seau (trois trames dans la meme rafale).
        let svc = make_service_db_cfg(
            db,
            MessagingConfig {
                per_contact_rate: 0,
                global_rate: 0,
                ..Default::default()
            },
        )
        .await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let conv = conv::random_conv();
        seed_conv(&svc, conv, "active");
        let pk_bin = bind_group_member(&svc, 92, &conv, &peer, &keys);
        let mid = [9u8; 16];
        let body = gmsg::encode_gmsg(&mid, b"salut");
        svc.handle_incoming(
            92,
            &keys,
            &wire_v2(&peer, 0, &keys.recv, conv, MsgKind::Msg, body.clone()),
        );
        // Re-emission : meme `mid`, nouvel `id` (le codec l'alloue).
        svc.handle_incoming(
            92,
            &keys,
            &wire_v2(&peer, 1, &keys.recv, conv, MsgKind::Msg, body),
        );
        // `mid` different -> livre.
        let body2 = gmsg::encode_gmsg(&[10u8; 16], b"re");
        svc.handle_incoming(
            92,
            &keys,
            &wire_v2(&peer, 2, &keys.recv, conv, MsgKind::Msg, body2),
        );
        let hist = svc.conversation_messages(&conv, 10).unwrap();
        assert_eq!(hist.len(), 2);
        assert!(hist.iter().all(|m| m.mid.is_some()));
        assert!(hist
            .iter()
            .all(|m| m.author_pk.as_deref() == Some(pk_bin.as_slice())));
    }

    /// Anti-forge : `roster` est additif (un `left` de B ne peut
    /// pas etre forge par A) ; le `leave` de A ne marque que A.
    #[tokio::test]
    async fn group_leave_anti_forge_et_roster_additif() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let conv = conv::random_conv();
        seed_conv(&svc, conv, "active");
        let a = LibNaClSecretKey::generate();
        let b = LibNaClSecretKey::generate();
        let a_bin = bind_group_member(&svc, 93, &conv, &a, &keys);
        let b_bin = bind_group_member(&svc, 94, &conv, &b, &keys);
        // A envoie un roster marquant B — additif : rien ne change.
        let roster = Gctl::Roster {
            members: vec![RosterEntry {
                pk: b_bin.as_slice().try_into().unwrap(),
                added_by: a_bin.as_slice().try_into().unwrap(),
                joined_at: 5,
            }],
        }
        .encode();
        svc.handle_incoming(
            93,
            &keys,
            &wire_v2(&a, 0, &keys.recv, conv, MsgKind::Gctl, roster),
        );
        let state = |pk: &[u8]| {
            svc.db
                .as_ref()
                .unwrap()
                .with(|c| dbc::member_state(c, &conv, pk))
                .unwrap()
        };
        assert_eq!(state(&b_bin).as_deref(), Some("member"));
        // `leave` de A : honore (son propre lien signe) — B intact.
        svc.handle_incoming(
            93,
            &keys,
            &wire_v2(&a, 1, &keys.recv, conv, MsgKind::Gctl, Gctl::Leave.encode()),
        );
        assert_eq!(state(&a_bin).as_deref(), Some("left"));
        assert_eq!(state(&b_bin).as_deref(), Some("member"));
    }

    /// Confinement : un pair `scope='group'` ne peut parler ni en
    /// v1 ni sur la conv directe — seules ses convs de groupe.
    #[tokio::test]
    async fn confinement_pair_groupe() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let conv = conv::random_conv();
        seed_conv(&svc, conv, "active");
        let pk_bin = bind_group_member(&svc, 95, &conv, &peer, &keys);
        // v1 (sans conv) -> rejet.
        svc.handle_incoming(95, &keys, &wire(&peer, 0, &keys.recv, b"1:1"));
        // v2 conv directe -> rejet.
        let direct = svc.direct_conv_id(&pk_bin);
        svc.handle_incoming(
            95,
            &keys,
            &wire_v2(&peer, 1, &keys.recv, direct, MsgKind::Msg, b"1:1".to_vec()),
        );
        assert_eq!(svc.stats.group_rejected.load(Ordering::Relaxed), 2);
        let hist = svc.conversation_messages(&conv, 10).unwrap();
        assert!(hist.is_empty());
    }

    /// `group_create` + `group_send` : conv creee, roster pose,
    /// ligne de conversation avec `mid`, livraison par membre
    /// (offline en test -> `failed` — oracle online-only).
    #[tokio::test]
    async fn group_send_fanout_et_historique() {
        let db = Arc::new(Database::memory().unwrap());
        let svc = make_service_db(db).await;
        let peer = LibNaClSecretKey::generate();
        let keys = MessagingKeys {
            send: [1u8; 32],
            recv: [2u8; 32],
        };
        let pk_bin = bind(&svc, 96, &peer, &keys);
        let conv = svc
            .group_create("canal", std::slice::from_ref(&pk_bin))
            .await
            .unwrap();
        assert_eq!(svc.conversation_state(&conv).as_deref(), Some("active"));
        assert_eq!(svc.conversation_members(&conv).unwrap().len(), 2);
        let mid = svc.group_send(&conv, b"bonjour".to_vec()).await.unwrap();
        let hist = svc.conversation_messages(&conv, 10).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].direction, "out");
        assert_eq!(hist[0].mid.as_deref(), Some(mid.as_slice()));
        // Livraison : tentative d'envoi tracee par membre (le
        // send_data echoue sans vrai circuit -> `failed`).
        let deliv = svc.conversation_delivery(&mid).unwrap();
        assert_eq!(deliv.len(), 1);
        assert_eq!(deliv[0].member_pk, pk_bin);
        // Un membre non-contact ne peut pas etre invite.
        let p2 = LibNaClSecretKey::generate();
        let b2 = bind_group_member(&svc, 97, &conv, &p2, &keys);
        assert!(svc.group_invite(&conv, &b2).await.is_err());
        // Un pair confine refuse le `send` 1:1.
        assert!(svc.send(&b2, b"x".to_vec()).await.is_err());
    }
}
