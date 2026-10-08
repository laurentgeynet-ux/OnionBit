// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration de la couche messagerie — toutes les bornes sont
//! ici (aucune valeur en dur dans le protocole).

use std::time::Duration;

/// Bornes et budgets de la couche messagerie (ADR-0011).
#[derive(Debug, Clone)]
pub struct MessagingConfig {
    /// Taille maximale d'une trame serialisee sur le fil.
    pub max_frame_len: usize,
    /// Taille maximale du `body` applicatif.
    pub max_body_len: usize,
    /// Taille de la fenetre de reception anti-replay (en seqs).
    pub recv_window: u32,
    /// Capacite du cache de deduplication par `id` de trame.
    pub dedup_cap: usize,
    /// Capacite de l'etat `pending` (consentements en attente).
    pub pending_cap: usize,
    /// TTL d'un consentement en attente sans reponse.
    pub pending_ttl: Duration,
    /// Budget de trames entrantes par contact (seau a jetons/s,
    /// `0` = illimite — usage banc uniquement).
    pub per_contact_rate: u32,
    /// Capacite de rafale du seau par contact (`0` = capacite =
    /// debit, aucune rafale — usage banc). Doit couvrir la rafale
    /// de negociation de groupe (`hello`+`join`+`roster`) : sinon
    /// le premier `msg` derriere un `accept` est ecarte au seau
    /// sans retransmission (MG-13).
    pub per_contact_burst: u32,
    /// Budget de trames entrantes global (seau a jetons/s,
    /// `0` = illimite — usage banc uniquement).
    pub global_rate: u32,
    /// Capacite de rafale du seau global (`0` = capacite = debit).
    /// Agrege les rafales de tous les liens — une arrivee massive
    /// de `join`/`roster`/`ack` de groupe ne doit pas affamer les
    /// `msg` applicatifs.
    pub global_burst: u32,
    /// Cadence de reannonce DHT du swarm de presence.
    pub announce_interval: Duration,
    /// Cadence de verification des points d'introduction
    /// (`ensure_introduction_points` est idempotent — il complete
    /// seulement ce qui manque). Bien plus rapide que
    /// `announce_interval` : le premier appel peut echouer tant que
    /// le pair epingle n'est pas verifie, et sans retry rapide la
    /// presence resterait absente jusqu'a 5 min (MS-13).
    pub ip_check_interval: Duration,
    /// Gate de consentement par confiance ext (ADR-0015
    /// `kind=identity`) : un emetteur dont le score local est < 0
    /// (flague par un curateur suivi) devient `blocked` au lieu de
    /// `pending` — la demande n'atteint jamais l'utilisateur.
    /// Necessite le `trust_lookup` injecte cote service.
    pub consent_gate_flagged: bool,
    /// Meme gate, cote positif : score > 0 → contact `Active` direct
    /// (le consentement est delegue aux curateurs suivis — choix
    /// explicite, defaut `false`).
    pub consent_gate_endorsed: bool,
    /// Gate de consentement par dette (ADR-0015 §5) : un pair dont le
    /// deficit ledger depasse `max_deficit_bytes` ne peut pas ouvrir
    /// de `pending`. N'opere que si `ledger_enforce` est actif cote
    /// tunnel (un seul interrupteur d'enforcement).
    pub consent_gate_ledger: bool,
    /// Groupes et pieces jointes actifs (ADR-0019) : annonce
    /// `CAP_MSG_V2` ext + `HELLO_CAP_GROUPS` dans les `hello` v2 et
    /// accepte les `gctl invite` entrants.
    pub groups_enabled: bool,
    /// Nombre maximal de membres par groupe (circuits e2e par nœud
    /// ≈ membres-1 ; les `gctl` de corps plus larges sont rejetes
    /// au codec).
    pub group_max_members: usize,
    /// Nombre maximal de conversations de groupe actives+invitees.
    pub group_max_convs: usize,
    /// Capacite des invitations de groupe en attente (`invited`).
    pub group_pending_cap: usize,
    /// TTL d'une invitation de groupe sans reponse.
    pub group_pending_ttl: Duration,
    /// Borne des noms affiches — nom de groupe (`gctl invite`) et
    /// nom de piece jointe (`attach`), en octets UTF-8.
    pub group_name_max_len: usize,
    /// Borne par fichier uploade vers le staging de pieces jointes
    /// (octets).
    pub attach_max_bytes: u64,
    /// Nombre maximal de pieces jointes par message (`mid`).
    pub attach_max_per_msg: usize,
    /// Quota global du staging `@state/messaging/` (octets).
    pub attach_stage_max_bytes: u64,
    /// TTL du seed d'une piece jointe emise (`0` = politique
    /// `seeding_mode` normale du moteur).
    pub attach_seed_ttl: Duration,
    /// Purge aussi le fichier stage a l'expiration du seed.
    pub attach_purge_on_expire: bool,
    /// Zone de reception des pieces jointes (ADR-0018) :
    /// `"public"` (defaut) ou `"private"` — `private` refuse tant
    /// que la zone est fermee (`409 identity_locked`) ; en session
    /// invitee le prive vit sous `temp/.guest/` (ephemere).
    pub attach_area: String,
    /// TTL d'un upload stage jamais attache.
    pub upload_ttl: Duration,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            max_frame_len: 32 * 1024,
            max_body_len: 30 * 1024,
            recv_window: 64,
            dedup_cap: 4096,
            pending_cap: 64,
            pending_ttl: Duration::from_secs(600),
            per_contact_rate: 2,
            per_contact_burst: 16,
            global_rate: 10,
            global_burst: 64,
            announce_interval: Duration::from_secs(300),
            ip_check_interval: Duration::from_secs(10),
            consent_gate_flagged: false,
            consent_gate_endorsed: false,
            consent_gate_ledger: false,
            groups_enabled: true,
            group_max_members: 16,
            group_max_convs: 64,
            group_pending_cap: 16,
            group_pending_ttl: Duration::from_secs(3600),
            group_name_max_len: 128,
            attach_max_bytes: 512 * 1024 * 1024,
            attach_max_per_msg: 8,
            attach_stage_max_bytes: 4096 * 1024 * 1024,
            attach_seed_ttl: Duration::from_secs(604800),
            attach_purge_on_expire: true,
            attach_area: "public".to_string(),
            upload_ttl: Duration::from_secs(86400),
        }
    }
}
