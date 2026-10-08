// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tables de conversations de messagerie (ADR-0019, v21) :
//! `msg_conversations` (unifie direct deterministe + groupe),
//! `msg_members` (roster par groupe), `msg_delivery` (statut de
//! livraison par membre), `msg_attachments` (offres et receptions
//! de pieces jointes — descripteurs magnet, jamais de contenu).
//!
//! Comme `messaging.rs`, ce module ne connait pas
//! `onionbit-messaging` : etats et roles sont des `TEXT` —
//! l'adaptation vit dans `onionbit-core::services::messaging`.

use rusqlite::{params, Connection, OptionalExtension};

use crate::Result;

/// Ligne de `msg_conversations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgConversationRow {
    /// `conv_id` (16 o — cle primaire).
    pub conv_id: Vec<u8>,
    /// `"direct"` (deterministe) ou `"group"` (aleatoire).
    pub kind: String,
    /// Nom affiche (`''` pour les conversations directes — l'UI
    /// retombe sur l'alias/cle abregee du contact).
    pub name: String,
    /// `"invited" | "active" | "left"`.
    pub state: String,
    /// Creation de la conversation.
    pub created_at: i64,
    /// Derniere activite (message, roster, attach).
    pub updated_at: i64,
    /// Horodatage du dernier message lu (badge non lu).
    pub last_read_ts: i64,
}

/// Vue de liste : conversation + compteur de non lus + dernier
/// horodatage de message — calculee en SQL (une requete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgConversationListRow {
    /// La conversation.
    pub row: MsgConversationRow,
    /// Messages entrants posterieurs a `last_read_ts`.
    pub unread: i64,
    /// `ts` du dernier message de la conversation (0 = vide).
    pub last_ts: i64,
}

/// Ligne de `msg_members` (roster d'un groupe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgMemberRow {
    /// `conv_id` du groupe.
    pub conv_id: Vec<u8>,
    /// `pk_bin` du membre.
    pub member_pk: Vec<u8>,
    /// `pk_bin` de l'invitant (`added_by`).
    pub added_by: Vec<u8>,
    /// `"member" | "left"`.
    pub state: String,
    /// `joined_at` — dernier ecrivain gagne sur `(joined_at, pk)`
    /// pour les ajouts (synchro additive).
    pub joined_at: i64,
}

/// Ligne de `msg_delivery` (statut par membre d'un message sortant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgDeliveryRow {
    /// `id` de la ligne `msg_messages` porteuse — `mid` d'un `msg`
    /// de groupe, `attach_id` d'une offre (ancre FK).
    pub msg_id: Vec<u8>,
    /// `pk_bin` du membre destinataire.
    pub member_pk: Vec<u8>,
    /// `"sent" | "acked" | "failed"`.
    pub status: String,
    /// `id` de la trame v2 emise vers ce membre — corps de son
    /// `ack` (`None` si l'emission a echoue avant la trame).
    pub frame_id: Option<Vec<u8>>,
    /// Horodatage de la derniere transition.
    pub ts: i64,
}

/// Ligne de `msg_attachments` (offre ou reception de piece jointe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgAttachmentRow {
    /// `attach_id` (16 o — cle primaire).
    pub attach_id: Vec<u8>,
    /// `conv_id` de la conversation porteuse.
    pub conv_id: Vec<u8>,
    /// `id` de la trame `attach` (message) associee.
    pub msg_id: Vec<u8>,
    /// Infohash **sale** du torrent ephemere (20 o).
    pub ih: Vec<u8>,
    /// Nom affiche du fichier.
    pub name: String,
    /// Taille annoncee en octets.
    pub size: i64,
    /// `"offer"` (nous seedons) ou `"recv"` (offre recue).
    pub role: String,
    /// `seeding | offered | accepted | downloading | done |
    /// expired | declined`.
    pub state: String,
    /// Date d'insertion locale.
    pub created_at: i64,
}

// ── Conversations ────────────────────────────────────────────

/// Insere ou met a jour une conversation (creation conservee ;
/// `state`/`name`/`updated_at` remplaces au conflit).
pub fn upsert_conversation(conn: &Connection, row: &MsgConversationRow) -> Result<()> {
    conn.execute(
        "INSERT INTO msg_conversations
             (conv_id, kind, name, state, created_at, updated_at, last_read_ts)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(conv_id) DO UPDATE SET
             name=excluded.name,
             state=excluded.state,
             updated_at=excluded.updated_at",
        params![
            row.conv_id,
            row.kind,
            row.name,
            row.state,
            row.created_at,
            row.updated_at,
            row.last_read_ts,
        ],
    )?;
    Ok(())
}

/// Conversation par `conv_id` (`None` = inconnue).
pub fn get_conversation(conn: &Connection, conv_id: &[u8]) -> Result<Option<MsgConversationRow>> {
    conn.query_row(
        "SELECT conv_id, kind, name, state, created_at, updated_at, last_read_ts
         FROM msg_conversations WHERE conv_id=?1",
        params![conv_id],
        row_to_conversation,
    )
    .optional()
    .map_err(Into::into)
}

/// Liste des conversations avec non lus et dernier horodatage —
/// tri par activite recente (liste de l'UI).
pub fn list_conversations(conn: &Connection) -> Result<Vec<MsgConversationListRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.conv_id, c.kind, c.name, c.state, c.created_at,
                c.updated_at, c.last_read_ts,
                (SELECT COUNT(*) FROM msg_messages m
                  WHERE m.conv_id=c.conv_id AND m.direction='in'
                    AND m.ts > c.last_read_ts) AS unread,
                COALESCE((SELECT MAX(m.ts) FROM msg_messages m
                          WHERE m.conv_id=c.conv_id), 0) AS last_ts
         FROM msg_conversations c
         ORDER BY last_ts DESC, c.updated_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(MsgConversationListRow {
            row: MsgConversationRow {
                conv_id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                state: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                last_read_ts: r.get(6)?,
            },
            unread: r.get(7)?,
            last_ts: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Etat d'une conversation (`None` = inconnue — utile au prefiltre
/// `conv` : directe derivee ou groupe connu).
pub fn conversation_state(conn: &Connection, conv_id: &[u8]) -> Result<Option<String>> {
    conn.query_row(
        "SELECT state FROM msg_conversations WHERE conv_id=?1",
        params![conv_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Pose l'etat d'une conversation (`invited|active|left`).
pub fn set_conversation_state(
    conn: &Connection,
    conv_id: &[u8],
    state: &str,
    now: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE msg_conversations SET state=?2, updated_at=?3 WHERE conv_id=?1",
        params![conv_id, state, now],
    )?;
    Ok(())
}

/// Renomme une conversation de groupe.
pub fn rename_conversation(conn: &Connection, conv_id: &[u8], name: &str, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE msg_conversations SET name=?2, updated_at=?3 WHERE conv_id=?1",
        params![conv_id, name, now],
    )?;
    Ok(())
}

/// Marqueur de lecture (badge non lu remis a zero).
pub fn mark_read(conn: &Connection, conv_id: &[u8], ts: i64) -> Result<()> {
    conn.execute(
        "UPDATE msg_conversations SET last_read_ts=MAX(last_read_ts,?2) WHERE conv_id=?1",
        params![conv_id, ts],
    )?;
    Ok(())
}

/// Suppression reelle d'une conversation : membres (cascade),
/// livraisons (cascade via messages), pieces jointes et messages.
pub fn delete_conversation(conn: &Connection, conv_id: &[u8]) -> Result<()> {
    conn.execute(
        "DELETE FROM msg_attachments WHERE conv_id=?1",
        params![conv_id],
    )?;
    conn.execute(
        "DELETE FROM msg_messages WHERE conv_id=?1",
        params![conv_id],
    )?;
    conn.execute(
        "DELETE FROM msg_conversations WHERE conv_id=?1",
        params![conv_id],
    )?;
    Ok(())
}

/// Nombre de conversations d'un type dans les etats donnes
/// (bornes `group_max_convs`, `group_pending_cap`).
pub fn count_conversations(conn: &Connection, kind: &str, states: &[&str]) -> Result<u64> {
    let marks = states.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT COUNT(*) FROM msg_conversations WHERE kind=? AND state IN ({marks})");
    let mut p: Vec<rusqlite::types::Value> = Vec::with_capacity(states.len() + 1);
    p.push(kind.to_string().into());
    for s in states {
        p.push((*s).to_string().into());
    }
    let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(p), |r| r.get(0))?;
    Ok(n as u64)
}

/// Touche `updated_at` (activite d'une conversation).
pub fn touch_conversation(conn: &Connection, conv_id: &[u8], now: i64) -> Result<()> {
    conn.execute(
        "UPDATE msg_conversations SET updated_at=?2 WHERE conv_id=?1",
        params![conv_id, now],
    )?;
    Ok(())
}

/// Conversations d'un type donne (`direct`|`group`), tous etats —
/// recharge au restart (miroir memoire du service).
pub fn list_by_kind(conn: &Connection, kind: &str) -> Result<Vec<MsgConversationRow>> {
    let mut stmt = conn.prepare(
        "SELECT conv_id, kind, name, state, created_at, updated_at, last_read_ts
         FROM msg_conversations WHERE kind=?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![kind], row_to_conversation)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ── Membres ─────────────────────────────────────────────────

/// Insere ou met a jour un membre (synchro **additive** : etat et
/// `joined_at` ne peuvent que progresser — le `left` n'arrive
/// jamais par cette voie, seulement par `set_member_state` sur le
/// lien signe du membre lui-meme).
pub fn upsert_member(conn: &Connection, row: &MsgMemberRow) -> Result<()> {
    conn.execute(
        "INSERT INTO msg_members (conv_id, member_pk, added_by, state, joined_at)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(conv_id, member_pk) DO UPDATE SET
             added_by=excluded.added_by,
             joined_at=MAX(msg_members.joined_at, excluded.joined_at)",
        params![
            row.conv_id,
            row.member_pk,
            row.added_by,
            row.state,
            row.joined_at,
        ],
    )?;
    Ok(())
}

/// Etat d'un membre (`None` = pas au roster).
pub fn member_state(conn: &Connection, conv_id: &[u8], pk: &[u8]) -> Result<Option<String>> {
    conn.query_row(
        "SELECT state FROM msg_members WHERE conv_id=?1 AND member_pk=?2",
        params![conv_id, pk],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Pose l'etat d'un membre (`member|left`) — appele pour un
/// `leave` recu **sur le lien signe du membre** (anti-forge) ou
/// pour notre propre depart.
pub fn set_member_state(conn: &Connection, conv_id: &[u8], pk: &[u8], state: &str) -> Result<()> {
    conn.execute(
        "UPDATE msg_members SET state=?3 WHERE conv_id=?1 AND member_pk=?2",
        params![conv_id, pk, state],
    )?;
    Ok(())
}

/// Roster d'un groupe (ordre de jointure).
pub fn list_members(conn: &Connection, conv_id: &[u8]) -> Result<Vec<MsgMemberRow>> {
    let mut stmt = conn.prepare(
        "SELECT conv_id, member_pk, added_by, state, joined_at
         FROM msg_members WHERE conv_id=?1 ORDER BY joined_at",
    )?;
    let rows = stmt.query_map(params![conv_id], |r| {
        Ok(MsgMemberRow {
            conv_id: r.get(0)?,
            member_pk: r.get(1)?,
            added_by: r.get(2)?,
            state: r.get(3)?,
            joined_at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Membres actifs d'un groupe (`state='member'`) — cibles du
/// fan-out.
pub fn list_active_members(conn: &Connection, conv_id: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare(
        "SELECT member_pk FROM msg_members
         WHERE conv_id=?1 AND state='member' ORDER BY joined_at",
    )?;
    let rows = stmt.query_map(params![conv_id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Nombre de membres actifs (borne `group_max_members`).
pub fn count_active_members(conn: &Connection, conv_id: &[u8]) -> Result<u64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM msg_members WHERE conv_id=?1 AND state='member'",
        params![conv_id],
        |r| r.get(0),
    )?;
    Ok(n as u64)
}

// ── Livraisons ──────────────────────────────────────────────

/// Insere ou met a jour le statut de livraison d'un message vers
/// un membre (`sent -> acked | failed`, dernier etat gagne par
/// progression — `acked` et `failed` sont terminaux).
pub fn upsert_delivery(conn: &Connection, row: &MsgDeliveryRow) -> Result<()> {
    conn.execute(
        "INSERT INTO msg_delivery (msg_id, member_pk, status, frame_id, ts)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(msg_id, member_pk) DO UPDATE SET
             status=excluded.status, ts=excluded.ts,
             frame_id=COALESCE(msg_delivery.frame_id, excluded.frame_id)
         WHERE msg_delivery.status='sent'",
        params![row.msg_id, row.member_pk, row.status, row.frame_id, row.ts],
    )?;
    Ok(())
}

/// Transition `accepted|downloading → done` des pieces jointes
/// recues dont le download de l'infohash a termine — appele par la
/// boucle de progression qui observe `stats.finished` (l'etat suit
/// la livraison reelle, pas seulement l'acceptation).
pub fn set_attach_done_by_ih(conn: &Connection, ih: &[u8]) -> Result<usize> {
    let n = conn.execute(
        "UPDATE msg_attachments SET state='done'
         WHERE ih=?1 AND role='recv' AND state IN ('accepted','downloading')",
        params![ih],
    )?;
    Ok(n)
}

/// Acquitte la livraison identifiee par sa `frame_id` — le corps
/// d'un `ack` de groupe reference la trame emise vers ce membre.
/// `sent -> acked` seulement (terminal). Retourne le nombre de
/// lignes transitionnees.
pub fn ack_delivery_frame(
    conn: &Connection,
    frame_id: &[u8],
    member_pk: &[u8],
    ts: i64,
) -> Result<usize> {
    let n = conn.execute(
        "UPDATE msg_delivery SET status='acked', ts=?1
         WHERE frame_id=?2 AND member_pk=?3 AND status='sent'",
        params![ts, frame_id, member_pk],
    )?;
    Ok(n)
}

/// Statuts par membre d'un message sortant.
pub fn list_delivery(conn: &Connection, msg_id: &[u8]) -> Result<Vec<MsgDeliveryRow>> {
    let mut stmt = conn.prepare(
        "SELECT msg_id, member_pk, status, frame_id, ts
         FROM msg_delivery WHERE msg_id=?1",
    )?;
    let rows = stmt.query_map(params![msg_id], |r| {
        Ok(MsgDeliveryRow {
            msg_id: r.get(0)?,
            member_pk: r.get(1)?,
            status: r.get(2)?,
            frame_id: r.get(3)?,
            ts: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

// ── Pieces jointes ──────────────────────────────────────────

/// Insere une piece jointe (idempotent — `attach_id` connu ignore).
pub fn insert_attachment(conn: &Connection, row: &MsgAttachmentRow) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO msg_attachments
             (attach_id, conv_id, msg_id, ih, name, size, role, state, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            row.attach_id,
            row.conv_id,
            row.msg_id,
            row.ih,
            row.name,
            row.size,
            row.role,
            row.state,
            row.created_at,
        ],
    )?;
    Ok(n > 0)
}

/// Piece jointe par `attach_id`.
pub fn get_attachment(conn: &Connection, attach_id: &[u8]) -> Result<Option<MsgAttachmentRow>> {
    conn.query_row(
        "SELECT attach_id, conv_id, msg_id, ih, name, size, role, state, created_at
         FROM msg_attachments WHERE attach_id=?1",
        params![attach_id],
        row_to_attachment,
    )
    .optional()
    .map_err(Into::into)
}

/// Pieces jointes d'une conversation (ordre d'arrivee).
pub fn list_attachments(conn: &Connection, conv_id: &[u8]) -> Result<Vec<MsgAttachmentRow>> {
    let mut stmt = conn.prepare(
        "SELECT attach_id, conv_id, msg_id, ih, name, size, role, state, created_at
         FROM msg_attachments WHERE conv_id=?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![conv_id], row_to_attachment)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Piece jointe par infohash (lien download <-> offre).
pub fn attachment_by_ih(conn: &Connection, ih: &[u8]) -> Result<Option<MsgAttachmentRow>> {
    conn.query_row(
        "SELECT attach_id, conv_id, msg_id, ih, name, size, role, state, created_at
         FROM msg_attachments WHERE ih=?1",
        params![ih],
        row_to_attachment,
    )
    .optional()
    .map_err(Into::into)
}

/// Transition d'etat d'une piece jointe.
pub fn set_attachment_state(conn: &Connection, attach_id: &[u8], state: &str) -> Result<()> {
    conn.execute(
        "UPDATE msg_attachments SET state=?2 WHERE attach_id=?1",
        params![attach_id, state],
    )?;
    Ok(())
}

/// Offres emises encore `seeding` (re-seed au redemarrage via
/// `restore_downloads` — la ligne `downloads` porte l'etat
/// moteur ; ici on re-annonce le `attach_seed_ttl`).
pub fn list_seeding_offers(conn: &Connection) -> Result<Vec<MsgAttachmentRow>> {
    let mut stmt = conn.prepare(
        "SELECT attach_id, conv_id, msg_id, ih, name, size, role, state, created_at
         FROM msg_attachments WHERE role='offer' AND state='seeding'",
    )?;
    let rows = stmt.query_map([], row_to_attachment)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn row_to_conversation(r: &rusqlite::Row) -> rusqlite::Result<MsgConversationRow> {
    Ok(MsgConversationRow {
        conv_id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        state: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        last_read_ts: r.get(6)?,
    })
}

fn row_to_attachment(r: &rusqlite::Row) -> rusqlite::Result<MsgAttachmentRow> {
    Ok(MsgAttachmentRow {
        attach_id: r.get(0)?,
        conv_id: r.get(1)?,
        msg_id: r.get(2)?,
        ih: r.get(3)?,
        name: r.get(4)?,
        size: r.get(5)?,
        role: r.get(6)?,
        state: r.get(7)?,
        created_at: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv_row(conv: u8, kind: &str, state: &str, ts: i64) -> MsgConversationRow {
        MsgConversationRow {
            conv_id: vec![conv; 16],
            kind: kind.into(),
            name: "groupe".into(),
            state: state.into(),
            created_at: ts,
            updated_at: ts,
            last_read_ts: 0,
        }
    }

    /// Cycle conversation : upsert, non lus, lecture, suppression
    /// reelle en cascade.
    #[test]
    fn cycle_conversation() {
        let db = crate::Database::memory().unwrap();
        db.with(|c| {
            let conv = vec![9u8; 16];
            upsert_conversation(c, &conv_row(9, "group", "active", 100))?;
            assert_eq!(conversation_state(c, &conv)?, Some("active".into()));

            // Membres : ajout additif, joined_at garde le max.
            let m = MsgMemberRow {
                conv_id: conv.clone(),
                member_pk: vec![1u8; 74],
                added_by: vec![2u8; 74],
                state: "member".into(),
                joined_at: 100,
            };
            upsert_member(c, &m)?;
            let older = MsgMemberRow {
                joined_at: 50,
                ..m.clone()
            };
            upsert_member(c, &older)?;
            assert_eq!(list_members(c, &conv)?[0].joined_at, 100);
            assert_eq!(count_active_members(c, &conv)?, 1);
            set_member_state(c, &conv, &[1u8; 74], "left")?;
            assert_eq!(member_state(c, &conv, &[1u8; 74])?, Some("left".into()));

            // Non lus : message entrant > last_read_ts.
            let pk = vec![5u8; 74];
            crate::messaging::upsert_contact(
                c,
                &crate::messaging::MsgContactRow {
                    public_key: pk.clone(),
                    state: "active".into(),
                    send_seq: 0,
                    recv_top: 0,
                    retention_secs: 0,
                    secure_delete: false,
                    alias: String::new(),
                    scope: "group".into(),
                    created_at: 100,
                    updated_at: 100,
                },
            )?;
            crate::messaging::insert_message(
                c,
                &crate::messaging::MsgMessageRow {
                    id: vec![1u8; 16],
                    contact_pk: pk.clone(),
                    direction: "in".into(),
                    seq: 1,
                    ts: 200,
                    body: b"coucou".to_vec(),
                    status: "received".into(),
                    created_at: 200,
                    conv_id: conv.clone(),
                    author_pk: Some(pk.clone()),
                    mid: Some(vec![3u8; 16]),
                },
            )?;
            let list = list_conversations(c)?;
            assert_eq!(list[0].unread, 1);
            assert_eq!(list[0].last_ts, 200);
            mark_read(c, &conv, 250)?;
            assert_eq!(list_conversations(c)?[0].unread, 0);

            // Dedup applicative.
            assert!(crate::messaging::has_group_mid(c, &conv, &pk, &[3u8; 16])?);
            assert!(!crate::messaging::has_group_mid(c, &conv, &pk, &[4u8; 16])?);

            // Livraison : sent -> acked, terminal non regressable.
            let mid_row = vec![1u8; 16];
            upsert_delivery(
                c,
                &MsgDeliveryRow {
                    msg_id: mid_row.clone(),
                    member_pk: vec![1u8; 74],
                    status: "sent".into(),
                    frame_id: Some(vec![9u8; 16]),
                    ts: 100,
                },
            )?;
            upsert_delivery(
                c,
                &MsgDeliveryRow {
                    msg_id: mid_row.clone(),
                    member_pk: vec![1u8; 74],
                    status: "acked".into(),
                    frame_id: None,
                    ts: 110,
                },
            )?;
            upsert_delivery(
                c,
                &MsgDeliveryRow {
                    msg_id: mid_row.clone(),
                    member_pk: vec![1u8; 74],
                    status: "failed".into(),
                    frame_id: None,
                    ts: 120,
                },
            )?;
            assert_eq!(list_delivery(c, &mid_row)?[0].status, "acked");

            // Correlation d'ack par `frame_id` (v22) : la trame
            // emise vers le membre identifie sa livraison.
            upsert_delivery(
                c,
                &MsgDeliveryRow {
                    msg_id: mid_row.clone(),
                    member_pk: vec![2u8; 74],
                    status: "sent".into(),
                    frame_id: Some(vec![7u8; 16]),
                    ts: 100,
                },
            )?;
            assert_eq!(ack_delivery_frame(c, &[7u8; 16], &[2u8; 74], 110)?, 1);
            // Deja acked : terminal.
            assert_eq!(ack_delivery_frame(c, &[7u8; 16], &[2u8; 74], 120)?, 0);
            let rows = list_delivery(c, &mid_row)?;
            assert_eq!(
                rows.iter()
                    .find(|r| r.member_pk == vec![2u8; 74])
                    .map(|r| r.status.as_str()),
                Some("acked")
            );

            // Attaches + cascade de suppression.
            insert_attachment(
                c,
                &MsgAttachmentRow {
                    attach_id: vec![8u8; 16],
                    conv_id: conv.clone(),
                    msg_id: vec![1u8; 16],
                    ih: vec![0u8; 20],
                    name: "f.bin".into(),
                    size: 42,
                    role: "recv".into(),
                    state: "offered".into(),
                    created_at: 100,
                },
            )?;
            assert_eq!(attachment_by_ih(c, &[0u8; 20])?.unwrap().name, "f.bin");
            delete_conversation(c, &conv)?;
            assert!(get_conversation(c, &conv)?.is_none());
            assert!(list_members(c, &conv)?.is_empty());
            assert!(list_attachments(c, &conv)?.is_empty());
            assert!(!crate::messaging::has_group_mid(c, &conv, &pk, &[3u8; 16])?);
            Ok(())
        })
        .unwrap();
    }
}
