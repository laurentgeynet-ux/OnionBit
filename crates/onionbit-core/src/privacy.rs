// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Profils d'anonymat prédéfinis (ADR-0022) — `legacy` / `full` /
//! `custom`.
//!
//! Un profil est un **preset matérialisé** : sélectionner `legacy`
//! ou `full` réécrit l'ensemble des clés couvertes (table
//! [`PrivacyProfile::preset_patch`]) via le merge atomique de
//! [`DaemonConfig::merge`]. Aucune couche d'override : le profil
//! *effectif* est dérivé en comparant la config courante au preset
//! stocké — une clé couverte modifiée à la main fait retomber
//! l'affichage sur `custom` ([`PrivacyProfile::effective`]).
//!
//! `full` exige `stealth.bridges` non vide (un client furtif sans
//! pont est une enclave vide — ADR-0017 §3) : refus ferme
//! `409 missing_prerequisites` côté API plutôt qu'un demi-mode
//! silencieux.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::daemon_config::DaemonConfig;

/// Profil d'anonymat persisté dans `privacy.profile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyProfile {
    /// Défauts actuels, compatibles Tribler (mesh IPv8 clair).
    #[default]
    Legacy,
    /// OnionBit-only : transport furtif ADR-0017, toutes les options
    /// d'anonymat/obfuscation activées.
    Full,
    /// Combinaison libre — aucun preset matérialisé.
    Custom,
}

/// Section `privacy` de `configuration.json` (ADR-0022) — intention
/// de posture, jamais de secret. Extension OnionBit sans équivalent
/// `TriblerConfig` (ADR-0015 §1 : section dédiée, pas de champ
/// partagé).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyFileConfig {
    /// Dernier profil appliqué (intention). Le profil *effectif* est
    /// dérivé par [`PrivacyProfile::effective`] : une clé couverte
    /// divergeant du preset retombe sur `custom`.
    pub profile: PrivacyProfile,
    /// Clés inconnues — préservées.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for PrivacyFileConfig {
    fn default() -> Self {
        Self {
            profile: PrivacyProfile::default(),
            extra: serde_json::Map::new(),
        }
    }
}

/// Clés couvertes **à redémarrage** (chemins `a.b` dans l'arbre de
/// config sérialisé) : modifiées par une bascule de profil, elles
/// n'ont aucun chemin d'application à chaud — `ExtSettings` est
/// immuable, `stealth.*`/`ipv8.enabled` figent la stack au démarrage.
/// Les autres clés couvertes (`download_defaults.*`, `guards_enabled`,
/// `ledger_*`, `storage.default_area`) sont relues à chaud par
/// `Session::apply_service_settings`.
const RESTART_BOUND_KEYS: &[&str] = &[
    "ipv8.enabled",
    "stealth.enabled",
    "stealth.role",
    "stealth.cover_traffic",
    "ext.enabled",
    "ext.ledger_enabled",
    "ext.obf_enabled",
    "tunnel_community.enabled",
    "tunnel_community.exitnode_enabled",
    "tunnel_community.messaging_enabled",
    "tunnel_community.messaging_hops",
    "tunnel_community.messaging_groups_enabled",
    "tunnel_community.messaging_consent_ledger",
];

/// Résultat d'une bascule de profil réussie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Chemins des clés couvertes dont la valeur a changé.
    pub applied_keys: Vec<String>,
    /// `true` si au moins une clé à redémarrage a changé.
    pub restart_required: bool,
}

/// Erreurs d'application d'un profil (ADR-0022 §4).
#[derive(Debug, Error)]
pub enum PrivacyError {
    /// `full` sans aucun pont configuré — rien n'est muté.
    #[error("missing prerequisites: {0:?}")]
    MissingPrerequisites(Vec<&'static str>),
    /// Combinaison inter-sections invalide après application.
    #[error("invalid combination: {0}")]
    InvalidCombination(&'static str),
    /// Le merge du preset a produit un arbre invalide.
    #[error("preset merge: {0}")]
    Merge(#[from] serde_json::Error),
}

impl PrivacyProfile {
    /// Table de correspondance ADR-0022 §3 — **source unique** : ni
    /// l'API ni l'UI ne dupliquent cette table. `None` pour `custom`
    /// (aucune clé écrite — la combinaison courante fait foi).
    ///
    /// Clés jamais couvertes (données utilisateur ou flux dédiés) :
    /// `stealth.bridges`, `stealth.tuning.*`, `ext.curators`,
    /// `identity.*`, `api.*`, ports/interfaces, `anon_hops` par
    /// téléchargement.
    fn preset_patch(self) -> Option<Value> {
        match self {
            PrivacyProfile::Custom => None,
            PrivacyProfile::Legacy => Some(serde_json::json!({
                "ipv8": { "enabled": true },
                "stealth": {
                    "enabled": false,
                    "role": "client",
                    "cover_traffic": false,
                },
                "libtorrent": {
                    "download_defaults": {
                        "anonymity_enabled": true,
                        "number_hops": 1,
                        "safeseeding_enabled": true,
                    },
                },
                "tunnel_community": {
                    "enabled": true,
                    "exitnode_enabled": false,
                    "guards_enabled": true,
                    "messaging_enabled": true,
                    "messaging_hops": 1,
                    "messaging_groups_enabled": true,
                    "ledger_enabled": true,
                    "ledger_enforce": false,
                    "messaging_consent_ledger": false,
                },
                "ext": {
                    "enabled": true,
                    "ledger_enabled": true,
                    "obf_enabled": false,
                },
                "storage": { "default_area": "public" },
            })),
            PrivacyProfile::Full => Some(serde_json::json!({
                // `ipv8.enabled=false` × `stealth.enabled=true` sont
                // toujours écrites ensemble : le merge atomique
                // garantit de ne jamais persister l'hybride refusé au
                // démarrage (ADR-0017).
                "ipv8": { "enabled": false },
                "stealth": {
                    "enabled": true,
                    "role": "client",
                    "cover_traffic": true,
                },
                "libtorrent": {
                    "download_defaults": {
                        "anonymity_enabled": true,
                        "number_hops": 3,
                        "safeseeding_enabled": true,
                    },
                },
                "tunnel_community": {
                    "enabled": true,
                    "exitnode_enabled": false,
                    "guards_enabled": true,
                    "messaging_enabled": true,
                    "messaging_hops": 3,
                    "messaging_groups_enabled": true,
                    "ledger_enabled": true,
                    "ledger_enforce": true,
                    "messaging_consent_ledger": true,
                },
                // `obf_enabled` conservé `true` bien que redondant
                // sous stealth (l'overlay le force à `false`) : un
                // retour ultérieur vers `custom` sans stealth garde
                // l'obfuscation ext plutôt que de retomber à découvert.
                "ext": {
                    "enabled": true,
                    "ledger_enabled": true,
                    "obf_enabled": true,
                },
                "storage": { "default_area": "private" },
            })),
        }
    }

    /// Entrées aplaties `(chemin « a.b », valeur attendue)` du preset
    /// — dérivées du patch pour que la couverture reste la source
    /// unique (couverture identique entre presets, cf. tests).
    fn covered_entries(self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        if let Some(patch) = self.preset_patch() {
            flatten_patch(&patch, String::new(), &mut out);
        }
        out
    }

    /// Profil **effectif** : le profil stocké si toutes les clés
    /// couvertes égalent encore son preset, sinon `custom` + la liste
    /// des clés divergentes (une option modifiée à la main dans
    /// Réglages retombe automatiquement sur « Personnalisé »).
    pub fn effective(cfg: &DaemonConfig) -> (Self, Vec<String>) {
        let stored = cfg.privacy.profile;
        let entries = stored.covered_entries();
        if entries.is_empty() {
            return (PrivacyProfile::Custom, Vec::new());
        }
        let current = serde_json::to_value(cfg).unwrap_or_default();
        let diverged: Vec<String> = entries
            .into_iter()
            .filter(|(path, expected)| get_path(&current, path) != Some(expected))
            .map(|(path, _)| path)
            .collect();
        if diverged.is_empty() {
            (stored, Vec::new())
        } else {
            (PrivacyProfile::Custom, diverged)
        }
    }

    /// Applique le preset `target` sur une **copie** : rien n'est muté
    /// en cas d'erreur (même discipline tout-ou-rien que
    /// `POST /api/settings`). Retourne la config modifiée et le
    /// détail de la bascule.
    pub fn apply(
        cfg: &DaemonConfig,
        target: Self,
    ) -> Result<(DaemonConfig, ApplyOutcome), PrivacyError> {
        // Prérequis *avant* toute mutation : `full` sans pont serait
        // une enclave vide — refus ferme, pas de demi-mode.
        if target == PrivacyProfile::Full && cfg.stealth.bridges.is_empty() {
            return Err(PrivacyError::MissingPrerequisites(vec!["stealth.bridges"]));
        }
        let before = serde_json::to_value(cfg).unwrap_or_default();
        let mut next = cfg.clone();
        if let Some(patch) = target.preset_patch() {
            next.merge(&patch)?;
        }
        next.privacy.profile = target;
        // La combinaison résultante passe le validateur commun —
        // jamais de config qui refuse de démarrer.
        next.validate_combination()
            .map_err(PrivacyError::InvalidCombination)?;
        let after = serde_json::to_value(&next).unwrap_or_default();
        let applied_keys: Vec<String> = target
            .covered_entries()
            .into_iter()
            .filter(|(path, _)| get_path(&before, path) != get_path(&after, path))
            .map(|(path, _)| path)
            .collect();
        let restart_required = applied_keys
            .iter()
            .any(|k| RESTART_BOUND_KEYS.contains(&k.as_str()));
        Ok((
            next,
            ApplyOutcome {
                applied_keys,
                restart_required,
            },
        ))
    }
}

/// `true` si une clé couverte **à redémarrage** de `persisted`
/// diffère de la config chargée au démarrage du daemon (`startup`,
/// snapshot au bind de l'API) — signal `restart_pending` du
/// `GET /api/privacy/profile` : les clés froides écrites depuis le
/// boot attendront le prochain lancement. Les clés à chaud ne
/// comptent pas : `apply_service_settings` les rend effectives
/// immédiatement.
pub fn restart_pending(persisted: &DaemonConfig, startup: &DaemonConfig) -> bool {
    let cur = serde_json::to_value(persisted).unwrap_or_default();
    let boot = serde_json::to_value(startup).unwrap_or_default();
    RESTART_BOUND_KEYS
        .iter()
        .any(|k| get_path(&cur, k) != get_path(&boot, k))
}

/// Aplatit un patch JSON en `(chemin, feuille)` — les objets sont
/// descendus, toute autre valeur est une feuille couverte.
fn flatten_patch(v: &Value, prefix: String, out: &mut Vec<(String, Value)>) {
    if let Value::Object(map) = v {
        for (k, sub) in map {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            flatten_patch(sub, path, out);
        }
    } else {
        out.push((prefix, v.clone()));
    }
}

/// Valeur au chemin `a.b.c` dans l'arbre sérialisé, `None` si absente.
fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `restart_pending` : une clé froide modifiée depuis le boot
    /// est détectée, une clé chaude ou hors couverture non.
    #[test]
    fn restart_pending_diff_sur_cles_froides() {
        let startup = DaemonConfig::default();
        let mut persisted = DaemonConfig::default();
        assert!(!restart_pending(&persisted, &startup));
        persisted.stealth.cover_traffic = true;
        assert!(restart_pending(&persisted, &startup));
        // Clé a chaud : appliquée sans redémarrage — pas de pending.
        persisted.stealth.cover_traffic = false;
        persisted.libtorrent.download_defaults.number_hops = 3;
        assert!(!restart_pending(&persisted, &startup));
        // Clé hors couverture : jamais « pending ».
        persisted.libtorrent.download_defaults.number_hops = 1;
        persisted.api.http_port = 9999;
        assert!(!restart_pending(&persisted, &startup));
    }

    /// Le preset `legacy` est exactement le défaut : une config par
    /// défaut affiche le profil `legacy` effectif sans divergence.
    #[test]
    fn defauts_est_effectif_legacy() {
        let cfg = DaemonConfig::default();
        let (eff, diverged) = PrivacyProfile::effective(&cfg);
        assert_eq!(eff, PrivacyProfile::Legacy);
        assert!(diverged.is_empty(), "divergences inattendues: {diverged:?}");
    }

    /// `apply(legacy)` sur les défauts : aucune clé ne change, pas de
    /// redémarrage requis.
    #[test]
    fn apply_legacy_sur_defauts_ne_change_rien() {
        let cfg = DaemonConfig::default();
        let (next, out) =
            PrivacyProfile::apply(&cfg, PrivacyProfile::Legacy).expect("apply legacy");
        assert!(out.applied_keys.is_empty());
        assert!(!out.restart_required);
        assert_eq!(next.privacy.profile, PrivacyProfile::Legacy);
    }

    /// `full` sans pont : refus ferme, config inchangée.
    #[test]
    fn apply_full_sans_pont_refuse() {
        let cfg = DaemonConfig::default();
        let err = PrivacyProfile::apply(&cfg, PrivacyProfile::Full).unwrap_err();
        assert!(matches!(err, PrivacyError::MissingPrerequisites(m) if m == &["stealth.bridges"]));
    }

    /// `full` avec pont : bascule complète — stealth on, ipv8 off,
    /// hops 3, obf, ledger enforcé, zone privée ; restart requis.
    #[test]
    fn apply_full_avec_pont() {
        let mut cfg = DaemonConfig::default();
        cfg.stealth
            .bridges
            .push("onionbit-bridge://127.0.0.1:9000#aa".repeat(1));
        let (next, out) = PrivacyProfile::apply(&cfg, PrivacyProfile::Full).expect("apply full");
        assert!(!next.ipv8.enabled);
        assert!(next.stealth.enabled);
        assert_eq!(next.stealth.role, "client");
        assert!(next.stealth.cover_traffic);
        assert_eq!(next.libtorrent.download_defaults.number_hops, 3);
        assert!(next.tunnel_community.ledger_enforce);
        assert!(next.tunnel_community.messaging_consent_ledger);
        assert_eq!(next.tunnel_community.messaging_hops, 3);
        assert!(next.ext.obf_enabled);
        assert_eq!(next.storage.default_area, "private");
        assert_eq!(next.privacy.profile, PrivacyProfile::Full);
        assert!(out.restart_required);
        assert!(out.applied_keys.contains(&"ipv8.enabled".to_string()));
        assert!(out.applied_keys.contains(&"stealth.enabled".to_string()));
        // Le pont n'est jamais récrit par le preset.
        assert_eq!(next.stealth.bridges, cfg.stealth.bridges);
        // Effectif = full sans divergence.
        let (eff, diverged) = PrivacyProfile::effective(&next);
        assert_eq!(eff, PrivacyProfile::Full);
        assert!(diverged.is_empty());
    }

    /// `full → legacy` : restauration dans le même patch atomique
    /// (`ipv8.enabled` revient à `true`, `stealth` retombe).
    #[test]
    fn aller_retour_full_legacy() {
        let mut cfg = DaemonConfig::default();
        cfg.stealth.bridges.push("onionbit-bridge://a#k".into());
        let (full, _) = PrivacyProfile::apply(&cfg, PrivacyProfile::Full).expect("full");
        let (back, out) = PrivacyProfile::apply(&full, PrivacyProfile::Legacy).expect("legacy");
        assert!(back.ipv8.enabled);
        assert!(!back.stealth.enabled);
        assert_eq!(back.libtorrent.download_defaults.number_hops, 1);
        assert!(!back.ext.obf_enabled);
        assert_eq!(back.storage.default_area, "public");
        let (eff, diverged) = PrivacyProfile::effective(&back);
        assert_eq!(eff, PrivacyProfile::Legacy);
        assert!(diverged.is_empty());
        assert!(out.restart_required);
        // Les ponts survivent au retour legacy.
        assert_eq!(back.stealth.bridges, cfg.stealth.bridges);
    }

    /// Divergence → `custom` : modifier une clé couverte fait retomber
    /// le profil effectif sur « personnalisé » avec la clé listée.
    #[test]
    fn divergence_bascule_sur_custom() {
        let mut cfg = DaemonConfig::default();
        cfg.libtorrent.download_defaults.number_hops = 2;
        let (eff, diverged) = PrivacyProfile::effective(&cfg);
        assert_eq!(eff, PrivacyProfile::Custom);
        assert_eq!(
            diverged,
            vec!["libtorrent.download_defaults.number_hops".to_string()]
        );
    }

    /// Une clé **non couverte** modifiée ne fait pas diverger.
    #[test]
    fn cle_hors_preset_ne_diverge_pas() {
        let mut cfg = DaemonConfig::default();
        cfg.stealth.bridges.push("onionbit-bridge://a#k".into());
        cfg.stealth.pad_max_extra = 128;
        cfg.ext.curators.push("aa".repeat(32));
        let (eff, diverged) = PrivacyProfile::effective(&cfg);
        assert_eq!(eff, PrivacyProfile::Legacy);
        assert!(diverged.is_empty());
    }

    /// `custom` n'écrit aucune clé couverte.
    #[test]
    fn apply_custom_ne_touche_rien() {
        let mut cfg = DaemonConfig::default();
        cfg.libtorrent.download_defaults.number_hops = 2;
        let (next, out) = PrivacyProfile::apply(&cfg, PrivacyProfile::Custom).expect("custom");
        assert_eq!(next.privacy.profile, PrivacyProfile::Custom);
        assert_eq!(next.libtorrent.download_defaults.number_hops, 2);
        assert!(out.applied_keys.is_empty());
        assert!(!out.restart_required);
    }

    /// Les deux presets couvrent **le même ensemble** de clés (la
    /// divergence n'est jamais mesurée sur une clé absente d'un des
    /// deux presets).
    #[test]
    fn couverture_identique_entre_presets() {
        let legacy: std::collections::BTreeSet<String> = PrivacyProfile::Legacy
            .covered_entries()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        let full: std::collections::BTreeSet<String> = PrivacyProfile::Full
            .covered_entries()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(legacy, full);
        // 20 clés couvertes (table ADR-0022 §3).
        assert_eq!(legacy.len(), 20);
    }

    /// `validate_combination` : l'hybride stealth×ipv8 est refusé,
    /// `at_rest` × rôle pont aussi — le même validateur sert au
    /// démarrage, à `POST /api/settings` et aux profils.
    #[test]
    fn combinaisons_refusees() {
        let mut cfg = DaemonConfig::default();
        // Hybride stealth × ipv8 legacy : refus.
        cfg.stealth.enabled = true; // ipv8.enabled toujours true
        assert!(cfg.validate_combination().is_err());
        // at_rest × rôle pont : refus (stealth client exclu — un pont
        // doit redémarrer sans surveillance).
        let mut cfg2 = DaemonConfig::default();
        cfg2.ipv8.enabled = false;
        cfg2.stealth.enabled = true;
        cfg2.stealth.role = "bridge".into();
        cfg2.identity.at_rest = true;
        assert!(cfg2.validate_combination().is_err());
        // stealth client + at_rest : combinaison licite.
        cfg2.stealth.role = "client".into();
        assert!(cfg2.validate_combination().is_ok());
    }

    /// Un preset `full` appliqué passe le validateur (role client +
    /// ipv8 off — aucune combinaison interdite).
    #[test]
    fn full_est_une_combinaison_valide() {
        let mut cfg = DaemonConfig::default();
        cfg.stealth.bridges.push("onionbit-bridge://a#k".into());
        let (next, _) = PrivacyProfile::apply(&cfg, PrivacyProfile::Full).expect("full");
        next.validate_combination().expect("full valide");
    }
}
