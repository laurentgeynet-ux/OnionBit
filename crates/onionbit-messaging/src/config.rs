// This file is part of OnionBit - a Rust port of the Tribler daemon.
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
    /// Budget de trames entrantes global (seau a jetons/s,
    /// `0` = illimite — usage banc uniquement).
    pub global_rate: u32,
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
            global_rate: 10,
            announce_interval: Duration::from_secs(300),
            ip_check_interval: Duration::from_secs(10),
            consent_gate_flagged: false,
            consent_gate_endorsed: false,
            consent_gate_ledger: false,
        }
    }
}
