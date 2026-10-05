// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tables `msg_contacts`/`msg_messages` (ADR-0011, etape 39) :
//! persistance de la messagerie e2e — etat de consentement,
//! compteurs `seq`, retention optionnelle par contact avec
//! suppression reelle (`DELETE`, `secure_delete` = zeroisation du
//! corps avant suppression).
//!
//! `onionbit-db` ne connait pas `onionbit-messaging` (sens des
//! dependances) : etats et statuts sont des `TEXT` — l'adaptation
//! vit dans `onionbit-core::services::messaging`.

use rusqlite::{params, Connection, OptionalExtension};

use crate::Result;

/// Ligne de `msg_contacts` (horodatages en secondes Unix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgContactRow {
    /// `pk_bin` du contact (cle primaire).
    pub public_key: Vec<u8>,
    /// Etat de consentement : `"active" | "pending" | "blocked"`.
    pub state: String,
    /// Prochain `seq` sortant.
    pub send_seq: i64,
    /// Plus grand `seq` entrant admis (reprise anti-replay).
    pub recv_top: i64,
    /// Retention des messages en secondes (`0` = conservation).
    pub retention_secs: i64,
    /// Zeroiser le corps avant suppression a l'expiration.
    pub secure_delete: bool,
    /// Creation de la fiche.
    pub created_at: i64,
    /// Derniere modification.
    pub updated_at: i64,
}

/// Ligne de `msg_messages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgMessageRow {
    /// `id` de trame (16 octets, cle primaire).
    pub id: Vec<u8>,
    /// `pk_bin` du contact.
    pub contact_pk: Vec<u8>,
    /// `"in"` (recu) ou `"out"` (emis).
    pub direction: String,
    /// `seq` de la trame.
    pub seq: i64,
    /// Horodatage emetteur de la trame.
    pub ts: i64,
    /// Corps applicatif en clair (persistance en clair — assumee v1).
    pub body: Vec<u8>,
    /// `"received" | "sent" | "acked" | "failed"`.
    pub status: String,
    /// Date d'insertion locale.
    pub created_at: i64,
}

/// Insere ou met a jour l'etat d'un contact (creation conservee).
pub fn upsert_contact(conn: &Connection, row: &MsgContactRow) -> Result<()> {
    conn.execute(
        "INSERT INTO msg_contacts
             (public_key, state, send_seq, recv_top, retention_secs,
              secure_delete, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(public_key) DO UPDATE SET
             state=excluded.state,
             updated_at=excluded.updated_at",
        params![
            row.public_key,
            row.state,
            row.send_seq,
            row.recv_top,
            row.retention_secs,
            row.secure_delete as i64,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

/// Etat d'un contact (`None` = inconnu).
pub fn contact_state(conn: &Connection, pk: &[u8]) -> Result<Option<String>> {
    conn.query_row(
        "SELECT state FROM msg_contacts WHERE public_key=?1",
        params![pk],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Fiche complete d'un contact.
pub fn get_contact(conn: &Connection, pk: &[u8]) -> Result<Option<MsgContactRow>> {
    conn.query_row(
        "SELECT public_key, state, send_seq, recv_top, retention_secs,
                secure_delete, created_at, updated_at
         FROM msg_contacts WHERE public_key=?1",
        params![pk],
        row_to_contact,
    )
    .optional()
    .map_err(Into::into)
}

/// Tous les contacts (ordre de creation).
pub fn list_contacts(conn: &Connection) -> Result<Vec<MsgContactRow>> {
    let mut stmt = conn.prepare(
        "SELECT public_key, state, send_seq, recv_top, retention_secs,
                secure_delete, created_at, updated_at
         FROM msg_contacts ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], row_to_contact)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Compteurs `seq` d'un contact (persistes a chaque trame admise).
pub fn set_seqs(
    conn: &Connection,
    pk: &[u8],
    send_seq: i64,
    recv_top: i64,
    now: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE msg_contacts SET send_seq=?2, recv_top=?3, updated_at=?4
         WHERE public_key=?1",
        params![pk, send_seq, recv_top, now],
    )?;
    Ok(())
}

/// Retention d'un contact (`retention_secs=0` = conservation,
/// `secure_delete` zeroise le corps avant le `DELETE` d'expiration).
pub fn set_retention(
    conn: &Connection,
    pk: &[u8],
    retention_secs: i64,
    secure_delete: bool,
    now: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE msg_contacts SET retention_secs=?2, secure_delete=?3,
                updated_at=?4
         WHERE public_key=?1",
        params![pk, retention_secs, secure_delete as i64, now],
    )?;
    Ok(())
}

/// Suppression reelle du contact et de ses messages (`ON DELETE
/// CASCADE` + `PRAGMA foreign_keys` actif sur la connexion).
pub fn delete_contact(conn: &Connection, pk: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM msg_messages WHERE contact_pk=?1", params![pk])?;
    conn.execute("DELETE FROM msg_contacts WHERE public_key=?1", params![pk])?;
    Ok(())
}

/// Insere un message (idempotent — un `id` connu est ignore).
pub fn insert_message(conn: &Connection, row: &MsgMessageRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO msg_messages
             (id, contact_pk, direction, seq, ts, body, status, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            row.id,
            row.contact_pk,
            row.direction,
            row.seq,
            row.ts,
            row.body,
            row.status,
            row.created_at,
        ],
    )?;
    Ok(n > 0)
}

/// Messages d'un contact, du plus recent au plus ancien (`limit`
/// borne la lecture — l'historique n'est jamais charge en entier).
pub fn list_messages(conn: &Connection, pk: &[u8], limit: u32) -> Result<Vec<MsgMessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, contact_pk, direction, seq, ts, body, status, created_at
         FROM msg_messages WHERE contact_pk=?1
         ORDER BY ts DESC, created_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pk, limit], row_to_message)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Statut de livraison (`sent -> acked`, ou `failed` a l'emission).
pub fn set_message_status(conn: &Connection, id: &[u8], status: &str) -> Result<()> {
    conn.execute(
        "UPDATE msg_messages SET status=?2 WHERE id=?1",
        params![id, status],
    )?;
    Ok(())
}

/// Suppression reelle d'un message (`DELETE` — pas de marqueur).
pub fn delete_message(conn: &Connection, id: &[u8]) -> Result<()> {
    conn.execute("DELETE FROM msg_messages WHERE id=?1", params![id])?;
    Ok(())
}

/// Messages expires a purger : `(contact, retention, secure)`.
/// Retourne le nombre de lignes supprimees — `secure_delete` zeroise
/// le corps avant le `DELETE` (residu minimal dans les pages).
pub fn prune_expired(conn: &Connection, now: i64) -> Result<u64> {
    let mut stmt = conn.prepare(
        "SELECT public_key, retention_secs, secure_delete FROM msg_contacts
         WHERE retention_secs > 0",
    )?;
    let cfgs: Vec<(Vec<u8>, i64, bool)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut deleted = 0u64;
    for (pk, retention, secure) in cfgs {
        let cutoff = now - retention;
        if secure {
            conn.execute(
                "UPDATE msg_messages SET body=zeroblob(length(body))
                 WHERE contact_pk=?1 AND created_at<?2",
                params![pk, cutoff],
            )?;
        }
        deleted += conn.execute(
            "DELETE FROM msg_messages WHERE contact_pk=?1 AND created_at<?2",
            params![pk, cutoff],
        )? as u64;
    }
    Ok(deleted)
}

fn row_to_contact(r: &rusqlite::Row) -> rusqlite::Result<MsgContactRow> {
    Ok(MsgContactRow {
        public_key: r.get(0)?,
        state: r.get(1)?,
        send_seq: r.get(2)?,
        recv_top: r.get(3)?,
        retention_secs: r.get(4)?,
        secure_delete: r.get::<_, i64>(5)? != 0,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn row_to_message(r: &rusqlite::Row) -> rusqlite::Result<MsgMessageRow> {
    Ok(MsgMessageRow {
        id: r.get(0)?,
        contact_pk: r.get(1)?,
        direction: r.get(2)?,
        seq: r.get(3)?,
        ts: r.get(4)?,
        body: r.get(5)?,
        status: r.get(6)?,
        created_at: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(pk: &[u8]) -> MsgContactRow {
        MsgContactRow {
            public_key: pk.to_vec(),
            state: "active".into(),
            send_seq: 0,
            recv_top: 0,
            retention_secs: 0,
            secure_delete: false,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn msg(pk: &[u8], id: u8, ts: i64) -> MsgMessageRow {
        MsgMessageRow {
            id: vec![id; 16],
            contact_pk: pk.to_vec(),
            direction: "out".into(),
            seq: id as i64,
            ts,
            body: b"corps".to_vec(),
            status: "sent".into(),
            created_at: ts,
        }
    }

    /// Cycle contact + messages : upsert, statuts, suppression
    /// reelle, purge de retention avec zeroisation.
    #[test]
    fn cycle_persistance_messagerie() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let pk = vec![7u8; 64];
            upsert_contact(c, &contact(&pk))?;
            assert_eq!(contact_state(c, &pk)?, Some("active".into()));
            set_seqs(c, &pk, 3, 9, 110)?;
            let row = get_contact(c, &pk)?.unwrap();
            assert_eq!((row.send_seq, row.recv_top), (3, 9));

            insert_message(c, &msg(&pk, 1, 100))?;
            // Doublon d'id : ignore.
            assert!(!insert_message(c, &msg(&pk, 1, 100))?);
            set_message_status(c, &[1u8; 16], "acked")?;
            let msgs = list_messages(c, &pk, 10)?;
            assert_eq!(msgs.len(), 1);
            assert_eq!(msgs[0].status, "acked");

            // Retention : le message expire est supprime ; en
            // `secure_delete` le corps serait zeroise avant — ici
            // on verifie la suppression reelle.
            set_retention(c, &pk, 50, true, 120)?;
            assert_eq!(prune_expired(c, 200)?, 1);
            assert!(list_messages(c, &pk, 10)?.is_empty());

            // Suppression reelle du contact (cascade messages).
            insert_message(c, &msg(&pk, 2, 200))?;
            delete_contact(c, &pk)?;
            assert!(get_contact(c, &pk)?.is_none());
            assert!(list_messages(c, &pk, 10)?.is_empty());
            Ok(())
        })
        .unwrap();
    }
}
