// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Adaptateur `GuardStore` sur la base SQLite (`guards`, ADR-0010).
//!
//! `onionbit-tunnel` definit le trait `GuardStore` sans dependre de
//! `onionbit-db` ; `onionbit-db` stocke des lignes brutes sans connaitre
//! `GuardRecord`. Ce module est la couture : il convertit les deux
//! representations et delegue a `Database::with` (les appels sont
//! synchrones — 5 lignes max, cout trivial sur le mutex de connexion).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use onionbit_db::guards::GuardRow;
use onionbit_db::Database;
use onionbit_ipv8::UdpAddress;
use onionbit_tunnel::guards::{GuardRecord, GuardStore};

/// `GuardStore` persistant : table `guards` de `onionbit.db`.
/// Une ecriture en echec est loggee mais n'interrompt jamais la
/// selection de circuits — le set reste fonctionnel en memoire.
pub struct DbGuardStore {
    db: Arc<Database>,
}

impl DbGuardStore {
    /// Cree le store sur la base ouverte.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

/// `SystemTime` → secondes Unix (pre-epoch = 0, borne defensible).
fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Secondes Unix → `SystemTime` (negatif = epoch, jamais de panic).
fn sys_time(secs: i64) -> SystemTime {
    if secs <= 0 {
        UNIX_EPOCH
    } else {
        UNIX_EPOCH + std::time::Duration::from_secs(secs as u64)
    }
}

/// `UdpAddress` → texte : `"ip:port"` numerique, `"dns:host:port"`
/// pour un domaine (format interne, distinct d'un `SocketAddr`).
fn address_to_text(a: &UdpAddress) -> String {
    match a {
        UdpAddress::Domain(h, p) => format!("dns:{h}:{p}"),
        _ => a
            .to_socket_addr()
            .map(|s| s.to_string())
            .unwrap_or_default(),
    }
}

/// Texte → `UdpAddress` ; `""` ou invalide = `None` (adresse perdue :
/// le `create` expirera en timeout, comme un guard qui a change d'IP).
fn text_to_address(s: &str) -> Option<UdpAddress> {
    if let Some(rest) = s.strip_prefix("dns:") {
        let (host, port) = rest.rsplit_once(':')?;
        return Some(UdpAddress::Domain(host.to_string(), port.parse().ok()?));
    }
    s.parse::<SocketAddr>().ok().map(UdpAddress::from)
}

impl GuardStore for DbGuardStore {
    fn load_guards(&self) -> Vec<GuardRecord> {
        match self.db.with(onionbit_db::guards::load) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| GuardRecord {
                    public_key_bin: r.public_key,
                    last_address: text_to_address(&r.address),
                    adopted_at: sys_time(r.adopted_at),
                    last_seen: sys_time(r.last_seen),
                    failures: r.failures.max(0) as u32,
                    reserve: r.reserve,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "chargement des guards impossible — set vide");
                Vec::new()
            }
        }
    }

    fn save_guards(&self, guards: &[GuardRecord]) {
        let rows: Vec<GuardRow> = guards
            .iter()
            .map(|g| GuardRow {
                public_key: g.public_key_bin.clone(),
                address: g
                    .last_address
                    .as_ref()
                    .map_or_else(String::new, address_to_text),
                adopted_at: unix_secs(g.adopted_at),
                last_seen: unix_secs(g.last_seen),
                failures: i64::from(g.failures),
                reserve: g.reserve,
                // `position` est reecrit par `replace_all` (rang = index).
                position: 0,
            })
            .collect();
        if let Err(e) = self.db.with(|c| onionbit_db::guards::replace_all(c, &rows)) {
            tracing::warn!(error = %e, "persistance des guards impossible");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onionbit_tunnel::guards::GuardsConfig;

    #[test]
    fn store_db_aller_retour_complet() {
        let db = Arc::new(Database::memory().unwrap());
        let store = DbGuardStore::new(db);
        let t = SystemTime::now();
        let records = vec![
            GuardRecord {
                public_key_bin: vec![7u8; 64],
                last_address: Some(UdpAddress::from(
                    "1.2.3.4:8090".parse::<SocketAddr>().unwrap(),
                )),
                adopted_at: t,
                last_seen: t,
                failures: 1,
                reserve: false,
            },
            GuardRecord {
                public_key_bin: vec![8u8; 64],
                last_address: None,
                adopted_at: UNIX_EPOCH,
                last_seen: UNIX_EPOCH,
                failures: 0,
                reserve: true,
            },
            GuardRecord {
                public_key_bin: vec![9u8; 64],
                last_address: Some(UdpAddress::Domain("relay.example".into(), 8080)),
                adopted_at: t,
                last_seen: t,
                failures: 0,
                reserve: true,
            },
        ];
        store.save_guards(&records);
        let loaded = store.load_guards();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].public_key_bin, vec![7u8; 64]);
        assert_eq!(
            loaded[0].last_address, records[0].last_address,
            "adresse numerique conservee"
        );
        assert_eq!(loaded[0].failures, 1);
        assert!(!loaded[0].reserve);
        assert_eq!(loaded[1].last_address, None);
        assert_eq!(
            loaded[2].last_address,
            Some(UdpAddress::Domain("relay.example".into(), 8080)),
            "domaine conserve"
        );
        assert!(loaded[2].reserve);
        // Ordre du set conserve (actifs puis reserve).
        let keys: Vec<_> = loaded.iter().map(|r| r.public_key_bin.clone()).collect();
        assert_eq!(keys, vec![vec![7u8; 64], vec![8u8; 64], vec![9u8; 64]]);
    }

    #[test]
    fn guardset_persiste_et_recharge_via_store() {
        let db = Arc::new(Database::memory().unwrap());
        let cfg = GuardsConfig {
            enabled: true,
            ..GuardsConfig::default()
        };
        let set = onionbit_tunnel::guards::GuardSet::new(
            cfg.clone(),
            Some(Arc::new(DbGuardStore::new(db.clone()))),
        );
        let sk = onionbit_crypto::ipv8::keys::LibNaClSecretKey::generate();
        let peer = onionbit_ipv8::Peer::new(
            sk.public_key().to_bin(),
            Some(UdpAddress::from(
                "10.9.8.7:7759".parse::<SocketAddr>().unwrap(),
            )),
        )
        .unwrap();
        set.ensure(std::slice::from_ref(&peer));
        assert_eq!(set.guard_keys(), vec![peer.public_key_bin.clone()]);

        // Redemarrage simule : nouveau set sur le meme store.
        let set2 =
            onionbit_tunnel::guards::GuardSet::new(cfg, Some(Arc::new(DbGuardStore::new(db))));
        assert_eq!(
            set2.guard_keys(),
            vec![peer.public_key_bin.clone()],
            "guard recharge depuis la base au redemarrage"
        );
    }
}
