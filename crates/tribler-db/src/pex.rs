//! Persistance des stores PEX des swarms caches (`tunnel_pex`) —
//! extension Rust : pyipv8 garde `PexCommunity` en memoire ; on
//! persiste les annonces propres (`intro_points_for`) et les points
//! appris pour que le role de point d'introduction survive a un
//! redemarrage (hidden seeding joignable sans attendre un nouveau
//! `establish-intro` du seeder).

use rusqlite::{params, Connection};

use crate::Result;

/// Ligne `tunnel_pex` : soit une annonce propre (`own`, `seeder_pk`
/// seul significatif), soit un point d'introduction appris.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PexRow {
    /// `info_hash` du swarm cache.
    pub info_hash: Vec<u8>,
    /// `true` = `intro_points_for` (annonce propre du `seeder_pk`).
    pub own: bool,
    /// Cle publique du point d'introduction appris (vide si `own`).
    pub peer_key: Vec<u8>,
    /// Cle publique du seeder annonce.
    pub seeder_pk: Vec<u8>,
    /// Adresse numerique `"ip:port"` du point appris (vide si `own`).
    pub address: String,
    /// `PEER_SOURCE_*` du point appris.
    pub source: i64,
    /// `last_seen` du point appris (secondes Unix).
    pub last_seen: i64,
}

/// Remplace integralement le cache PEX par le snapshot courant (une
/// transaction : `DELETE` + batch insert — la carte est petite et
/// volatile, un diff ligne a ligne n'apporterait rien).
pub fn replace_all(conn: &Connection, rows: &[PexRow]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM tunnel_pex", [])?;
    let mut n = 0;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO tunnel_pex
             (info_hash, own, peer_key, seeder_pk, address, source, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for r in rows {
            n += stmt.execute(params![
                r.info_hash,
                r.own as i64,
                r.peer_key,
                r.seeder_pk,
                r.address,
                r.source,
                r.last_seen
            ])?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Charge toutes les lignes (le filtrage TTL des points appris est
/// fait par l'appelant — `PexStore` applique sa propre eviction).
pub fn list(conn: &Connection) -> Result<Vec<PexRow>> {
    let mut stmt = conn.prepare(
        "SELECT info_hash, own, peer_key, seeder_pk, address, source, last_seen
         FROM tunnel_pex",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PexRow {
            info_hash: r.get(0)?,
            own: r.get::<_, i64>(1)? != 0,
            peer_key: r.get(2)?,
            seeder_pk: r.get(3)?,
            address: r.get(4)?,
            source: r.get(5)?,
            last_seen: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Supprime les entrees d'un swarm (dechargement du store cote
/// tunnel : `is_done` apres `stop_announce`).
pub fn delete_swarm(conn: &Connection, info_hash: &[u8]) -> Result<()> {
    conn.execute(
        "DELETE FROM tunnel_pex WHERE info_hash = ?1",
        params![info_hash],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pex_replace_liste_delete() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let ih = vec![7u8; 20];
            let rows = vec![
                PexRow {
                    info_hash: ih.clone(),
                    own: true,
                    peer_key: Vec::new(),
                    seeder_pk: vec![9u8; 64],
                    address: String::new(),
                    source: 0,
                    last_seen: 0,
                },
                PexRow {
                    info_hash: ih.clone(),
                    own: false,
                    peer_key: vec![1u8; 64],
                    seeder_pk: vec![9u8; 64],
                    address: "1.2.3.4:8090".into(),
                    source: 2,
                    last_seen: 1_700_000_000,
                },
            ];
            assert_eq!(replace_all(c, &rows)?, 2);
            let got = list(c)?;
            assert_eq!(got.len(), 2);
            assert!(got.iter().any(|r| r.own));
            assert!(got.iter().any(|r| !r.own && r.address == "1.2.3.4:8090"));
            // Snapshot suivant : remplacement integral.
            assert_eq!(replace_all(c, &rows[..1])?, 1);
            assert_eq!(list(c)?.len(), 1);
            delete_swarm(c, &ih)?;
            assert!(list(c)?.is_empty());
            Ok(())
        })
        .unwrap();
    }
}
