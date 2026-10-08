// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Corps des trames `gctl` — controle de groupe (ADR-0019 §3).
//!
//! Bencode canonique strict : ensemble de cles exact par
//! operation, tailles bornees (roster `<= group_max_members`, nom
//! `<= group_name_max_len`). La trame `conv` top-level porte
//! l'identite du groupe — le corps ne la repete pas.
//!
//! ```text
//! invite : { "by": <pk_bin 74o>, "name": <str>, "op": "invite",
//!            "roster": [<pk_bin 74o>...] }
//! join   : { "op": "join" }
//! leave  : { "op": "leave" }
//! roster : { "members": [ { "by": <74o>, "pk": <74o>,
//!                            "ts": <u64> } ... ], "op": "roster" }
//! ```

use std::collections::BTreeMap;

use onionbit_format::bencode::{decode, BValue};

use crate::config::MessagingConfig;
use crate::error::MessagingError;

/// Cle publique brute d'un membre (`pk_bin` LibNaCl, format
/// `to_bin` — 74 octets).
pub const PK_LEN: usize = onionbit_crypto::ipv8::keys::LIBNACL_PK_BIN_LEN;

/// Entree de roster propagee par `gctl roster`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    /// Cle publique du membre.
    pub pk: [u8; PK_LEN],
    /// Cle publique de l'invitant (`added_by`).
    pub added_by: [u8; PK_LEN],
    /// `joined_at` — dernier ecrivain gagne sur `(joined_at, pk)`
    /// pour les **ajouts** (la synchro est additive : un `left`
    /// n'est honore que sur le lien signe du membre lui-meme).
    pub joined_at: u64,
}

/// Operation de controle de groupe (corps d'une trame `gctl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gctl {
    /// Invitation : nom du groupe, roster actuel, pk de l'invitant.
    Invite {
        /// Nom affichable du groupe (borne `group_name_max_len`).
        name: String,
        /// Roster complet vu par l'invitant (borne `group_max_members`).
        roster: Vec<[u8; PK_LEN]>,
        /// Cle publique de l'invitant.
        by: [u8; PK_LEN],
    },
    /// Acceptation de l'invitation par le destinataire.
    Join,
    /// Depart — honore **uniquement** sur le lien signe du membre
    /// qui part (anti-forge, ADR-0019 §3).
    Leave,
    /// Synchronisation complete du roster (jointure et changements).
    /// **Additive** : ajoute/met a jour des membres, ne supprime pas.
    Roster {
        /// Entrees connues de l'emetteur.
        members: Vec<RosterEntry>,
    },
}

impl Gctl {
    /// Encode le corps `gctl` en bencode canonique.
    pub fn encode(&self) -> Vec<u8> {
        let mut d = BTreeMap::new();
        match self {
            Gctl::Invite { name, roster, by } => {
                d.insert(b"by".to_vec(), BValue::Bytes(by.to_vec()));
                d.insert(b"name".to_vec(), BValue::Bytes(name.clone().into_bytes()));
                d.insert(
                    b"roster".to_vec(),
                    BValue::List(roster.iter().map(|pk| BValue::Bytes(pk.to_vec())).collect()),
                );
            }
            Gctl::Join | Gctl::Leave => {}
            Gctl::Roster { members } => {
                let list: Vec<BValue> = members
                    .iter()
                    .map(|m| {
                        let mut e = BTreeMap::new();
                        e.insert(b"by".to_vec(), BValue::Bytes(m.added_by.to_vec()));
                        e.insert(b"pk".to_vec(), BValue::Bytes(m.pk.to_vec()));
                        e.insert(b"ts".to_vec(), BValue::Int(m.joined_at as i64));
                        BValue::Dict(e)
                    })
                    .collect();
                d.insert(b"members".to_vec(), BValue::List(list));
            }
        }
        d.insert(
            b"op".to_vec(),
            BValue::Bytes(match self {
                Gctl::Invite { .. } => b"invite".to_vec(),
                Gctl::Join => b"join".to_vec(),
                Gctl::Leave => b"leave".to_vec(),
                Gctl::Roster { .. } => b"roster".to_vec(),
            }),
        );
        BValue::Dict(d).encode()
    }

    /// Decode un corps `gctl` — strict : operation connue, ensemble
    /// de cles exact par operation, tailles bornees.
    pub fn decode_body(body: &[u8], cfg: &MessagingConfig) -> Result<Self, MessagingError> {
        let value = decode(body)?;
        let dict = value.as_dict().ok_or(MessagingError::Malformed(
            "gctl : n'est pas un dictionnaire",
        ))?;
        let op = dict
            .get(b"op".as_ref())
            .and_then(BValue::as_bytes)
            .ok_or(MessagingError::Malformed("gctl : op absent"))?;
        let pk_of = |v: &BValue| -> Result<[u8; PK_LEN], MessagingError> {
            let b = v
                .as_bytes()
                .ok_or(MessagingError::Malformed("gctl : pk non binaire"))?;
            <[u8; PK_LEN]>::try_from(b)
                .map_err(|_| MessagingError::Malformed("gctl : pk taille inattendue"))
        };
        match op {
            b"invite" => {
                if dict.len() != 4 {
                    return Err(MessagingError::Malformed("gctl invite : cles"));
                }
                let name_b = dict
                    .get(b"name".as_ref())
                    .and_then(BValue::as_bytes)
                    .ok_or(MessagingError::Malformed("gctl invite : name absent"))?;
                if name_b.is_empty() || name_b.len() > cfg.group_name_max_len {
                    return Err(MessagingError::Malformed("gctl invite : name hors borne"));
                }
                let name = String::from_utf8(name_b.to_vec())
                    .map_err(|_| MessagingError::Malformed("gctl invite : name non utf8"))?;
                let roster_v = dict
                    .get(b"roster".as_ref())
                    .and_then(BValue::as_list)
                    .ok_or(MessagingError::Malformed("gctl invite : roster absent"))?;
                if roster_v.is_empty() || roster_v.len() > cfg.group_max_members {
                    return Err(MessagingError::Malformed("gctl invite : roster hors borne"));
                }
                let roster = roster_v.iter().map(pk_of).collect::<Result<Vec<_>, _>>()?;
                let by = pk_of(
                    dict.get(b"by".as_ref())
                        .ok_or(MessagingError::Malformed("gctl invite : by absent"))?,
                )?;
                Ok(Gctl::Invite { name, roster, by })
            }
            b"join" | b"leave" => {
                if dict.len() != 1 {
                    return Err(MessagingError::Malformed("gctl join/leave : cles"));
                }
                Ok(if op == b"join" {
                    Gctl::Join
                } else {
                    Gctl::Leave
                })
            }
            b"roster" => {
                if dict.len() != 2 {
                    return Err(MessagingError::Malformed("gctl roster : cles"));
                }
                let list = dict
                    .get(b"members".as_ref())
                    .and_then(BValue::as_list)
                    .ok_or(MessagingError::Malformed("gctl roster : members absent"))?;
                if list.is_empty() || list.len() > cfg.group_max_members {
                    return Err(MessagingError::Malformed("gctl roster : borne"));
                }
                let members =
                    list.iter()
                        .map(|v| {
                            let e = v.as_dict().ok_or(MessagingError::Malformed(
                                "gctl roster : entree non dict",
                            ))?;
                            if e.len() != 3 {
                                return Err(MessagingError::Malformed("gctl roster : entree cles"));
                            }
                            let joined_at = e
                                .get(b"ts".as_ref())
                                .and_then(BValue::as_int)
                                .ok_or(MessagingError::Malformed("gctl roster : ts absent"))?;
                            if joined_at < 0 {
                                return Err(MessagingError::Malformed("gctl roster : ts negatif"));
                            }
                            Ok(RosterEntry {
                                pk: pk_of(e.get(b"pk".as_ref()).ok_or(
                                    MessagingError::Malformed("gctl roster : pk absent"),
                                )?)?,
                                added_by: pk_of(e.get(b"by".as_ref()).ok_or(
                                    MessagingError::Malformed("gctl roster : by absent"),
                                )?)?,
                                joined_at: joined_at as u64,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                Ok(Gctl::Roster { members })
            }
            _ => Err(MessagingError::Malformed("gctl : op inconnue")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MessagingConfig {
        MessagingConfig::default()
    }

    /// Roundtrip des quatre operations.
    #[test]
    fn gctl_roundtrip() {
        let ops = [
            Gctl::Invite {
                name: "groupe test".into(),
                roster: vec![[1u8; PK_LEN], [2u8; PK_LEN]],
                by: [9u8; PK_LEN],
            },
            Gctl::Join,
            Gctl::Leave,
            Gctl::Roster {
                members: vec![
                    RosterEntry {
                        pk: [1u8; PK_LEN],
                        added_by: [1u8; PK_LEN],
                        joined_at: 100,
                    },
                    RosterEntry {
                        pk: [2u8; PK_LEN],
                        added_by: [9u8; PK_LEN],
                        joined_at: 150,
                    },
                ],
            },
        ];
        for op in &ops {
            assert_eq!(&Gctl::decode_body(&op.encode(), &cfg()).unwrap(), op);
        }
    }

    /// Corps hostiles : op inconnue, cles supplementaires, roster
    /// hors borne, nom trop long, entree roster malformee.
    #[test]
    fn gctl_corps_hostiles() {
        let c = cfg();
        // op inconnue
        let mut d = BTreeMap::new();
        d.insert(b"op".to_vec(), BValue::Bytes(b"kick".to_vec()));
        assert!(Gctl::decode_body(&BValue::Dict(d).encode(), &c).is_err());
        // invite : cle supplementaire
        let mut d = BTreeMap::new();
        d.insert(b"op".to_vec(), BValue::Bytes(b"invite".to_vec()));
        d.insert(b"by".to_vec(), BValue::Bytes([1u8; PK_LEN].to_vec()));
        d.insert(b"name".to_vec(), BValue::Bytes(b"n".to_vec()));
        d.insert(
            b"roster".to_vec(),
            BValue::List(vec![BValue::Bytes([1u8; PK_LEN].to_vec())]),
        );
        d.insert(b"extra".to_vec(), BValue::Int(1));
        assert!(Gctl::decode_body(&BValue::Dict(d).encode(), &c).is_err());
        // invite : roster vide / trop grand
        let inv = Gctl::Invite {
            name: "n".into(),
            roster: vec![],
            by: [1u8; PK_LEN],
        };
        assert!(Gctl::decode_body(&inv.encode(), &c).is_err());
        let inv = Gctl::Invite {
            name: "n".into(),
            roster: vec![[0u8; PK_LEN]; c.group_max_members + 1],
            by: [1u8; PK_LEN],
        };
        assert!(Gctl::decode_body(&inv.encode(), &c).is_err());
        // invite : nom hors borne
        let inv = Gctl::Invite {
            name: "x".repeat(c.group_name_max_len + 1),
            roster: vec![[1u8; PK_LEN]],
            by: [1u8; PK_LEN],
        };
        assert!(Gctl::decode_body(&inv.encode(), &c).is_err());
        // roster : entree a cle inconnue
        let mut e = BTreeMap::new();
        e.insert(b"by".to_vec(), BValue::Bytes([1u8; PK_LEN].to_vec()));
        e.insert(b"pk".to_vec(), BValue::Bytes([2u8; PK_LEN].to_vec()));
        e.insert(b"ts".to_vec(), BValue::Int(1));
        e.insert(b"x".to_vec(), BValue::Int(1));
        let mut d = BTreeMap::new();
        d.insert(b"op".to_vec(), BValue::Bytes(b"roster".to_vec()));
        d.insert(b"members".to_vec(), BValue::List(vec![BValue::Dict(e)]));
        assert!(Gctl::decode_body(&BValue::Dict(d).encode(), &c).is_err());
        // roster : ts negatif
        let mut e = BTreeMap::new();
        e.insert(b"by".to_vec(), BValue::Bytes([1u8; PK_LEN].to_vec()));
        e.insert(b"pk".to_vec(), BValue::Bytes([2u8; PK_LEN].to_vec()));
        e.insert(b"ts".to_vec(), BValue::Int(-1));
        let mut d = BTreeMap::new();
        d.insert(b"op".to_vec(), BValue::Bytes(b"roster".to_vec()));
        d.insert(b"members".to_vec(), BValue::List(vec![BValue::Dict(e)]));
        assert!(Gctl::decode_body(&BValue::Dict(d).encode(), &c).is_err());
    }
}
