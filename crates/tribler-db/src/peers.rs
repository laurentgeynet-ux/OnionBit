//! Cache de pairs IPv8 verifies (`ipv8_peers`) — recharge dans
//! `Network` au demarrage pour sauter le bootstrap DNS/marche
//! aleatoire a froid. Extension Rust : pyipv8 ne persiste pas son
//! annuaire (le graphe se reconstruit par walks), mais le cout de
//! demarrage est sensible pour la reactivite de l'UI.

use rusqlite::{params, Connection};

use crate::Result;

/// Un pair persiste (ligne de `ipv8_peers`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv8PeerRow {
    /// Cle publique binaire (`public_key_bin` pyipv8).
    pub public_key: Vec<u8>,
    /// Adresse numerique `"ip:port"`.
    pub address: String,
    /// Derniere observation (secondes Unix).
    pub last_seen: i64,
    /// Le pair emet le format d'introduction "new style".
    pub new_style: bool,
}

/// Enregistre un lot de pairs en une transaction (`INSERT OR REPLACE`
/// — `last_seen` et l'adresse sont rafraichis).
pub fn upsert_batch(conn: &Connection, peers: &[Ipv8PeerRow], now: i64) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO ipv8_peers (public_key, address, last_seen, new_style)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for p in peers {
            n += stmt.execute(params![p.public_key, p.address, now, p.new_style as i64])?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Charge les pairs les plus recemment vus (`min_last_seen` exclusif,
/// tries par fraicheur, bornes par `limit`).
pub fn list_peers(conn: &Connection, min_last_seen: i64, limit: usize) -> Result<Vec<Ipv8PeerRow>> {
    let mut stmt = conn.prepare(
        "SELECT public_key, address, last_seen, new_style FROM ipv8_peers
         WHERE last_seen >= ?1
         ORDER BY last_seen DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![min_last_seen, limit.max(1) as i64], |r| {
        Ok(Ipv8PeerRow {
            public_key: r.get(0)?,
            address: r.get(1)?,
            last_seen: r.get(2)?,
            new_style: r.get::<_, i64>(3)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Expire les entrees plus vieilles que `min_last_seen` et borne la
/// table a `max_rows` entrees (les plus recentes conservees).
pub fn prune(conn: &Connection, min_last_seen: i64, max_rows: usize) -> Result<usize> {
    let stale = conn.execute(
        "DELETE FROM ipv8_peers WHERE last_seen < ?1",
        params![min_last_seen],
    )?;
    let excess = conn.execute(
        "DELETE FROM ipv8_peers WHERE rowid NOT IN (
             SELECT rowid FROM ipv8_peers ORDER BY last_seen DESC LIMIT ?1
         )",
        params![max_rows.max(1) as i64],
    )?;
    Ok(stale + excess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_pairs_upsert_liste_prune() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let batch = vec![
                Ipv8PeerRow {
                    public_key: vec![1],
                    address: "1.2.3.4:8090".into(),
                    last_seen: 0,
                    new_style: true,
                },
                Ipv8PeerRow {
                    public_key: vec![2],
                    address: "5.6.7.8:8091".into(),
                    last_seen: 0,
                    new_style: false,
                },
                Ipv8PeerRow {
                    public_key: vec![3],
                    address: "9.9.9.9:7759".into(),
                    last_seen: 0,
                    new_style: false,
                },
            ];
            assert_eq!(upsert_batch(c, &batch, 1000)?, 3);
            // Rafraichit un pair existant : une ligne, nouvelle adresse.
            assert_eq!(
                upsert_batch(
                    c,
                    &[Ipv8PeerRow {
                        public_key: vec![1],
                        address: "4.4.4.4:1".into(),
                        last_seen: 0,
                        new_style: true,
                    }],
                    2000
                )?,
                1
            );
            let peers = list_peers(c, 0, 100)?;
            assert_eq!(peers.len(), 3);
            assert_eq!(peers[0].last_seen, 2000);
            assert_eq!(peers[0].address, "4.4.4.4:1");
            assert!(peers[0].new_style);

            // Expire les anciens et borne la table.
            assert_eq!(prune(c, 1500, 10)?, 2);
            assert_eq!(list_peers(c, 0, 100)?.len(), 1);
            assert_eq!(upsert_batch(c, &batch, 3000)?, 3);
            assert_eq!(prune(c, 0, 2)?, 1);
            assert_eq!(list_peers(c, 0, 100)?.len(), 2);
            Ok(())
        })
        .unwrap();
    }
}
