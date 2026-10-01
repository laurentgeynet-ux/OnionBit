// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Valeur bencode (BEP 3) et accesseurs de convenance.

use std::collections::BTreeMap;

/// Valeur bencode : entier, chaine d'octets, liste ou dictionnaire.
///
/// Les dictionnaires utilisent un `BTreeMap` : les cles sont
/// automatiquement triees lexicographiquement, ce que le format bencode
/// exige pour la serialisation canonique (necessaire pour que l'info-hash
/// SHA-1 soit correct).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BValue {
    /// Entier signe.
    Int(i64),
    /// Chaine d'octets (peut contenir du binaire, pas forcement UTF-8).
    Bytes(Vec<u8>),
    /// Liste de valeurs.
    List(Vec<BValue>),
    /// Dictionnaire cle -> valeur (cles triees lexicographiquement).
    Dict(BTreeMap<Vec<u8>, BValue>),
}

impl BValue {
    /// Accede a la valeur d'une cle de dictionnaire.
    pub fn get(&self, key: &[u8]) -> Option<&BValue> {
        match self {
            BValue::Dict(d) => d.get(key),
            _ => None,
        }
    }

    /// `get` avec une cle UTF-8.
    pub fn get_str(&self, key: &str) -> Option<&BValue> {
        self.get(key.as_bytes())
    }

    /// Retourne la valeur entiere si `Int`.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            BValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Retourne les octets si `Bytes`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            BValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Retourne les octets decodes en UTF-8 si `Bytes` valide.
    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| core::str::from_utf8(b).ok())
    }

    /// Retourne la liste si `List`.
    pub fn as_list(&self) -> Option<&[BValue]> {
        match self {
            BValue::List(l) => Some(l),
            _ => None,
        }
    }

    /// Retourne le dictionnaire si `Dict`.
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, BValue>> {
        match self {
            BValue::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// Re-serialise la valeur en bencode canonique (cles triees).
    pub fn encode(&self) -> Vec<u8> {
        super::encoder::encode(self)
    }
}

impl From<i64> for BValue {
    fn from(i: i64) -> Self {
        BValue::Int(i)
    }
}

impl From<u64> for BValue {
    fn from(i: u64) -> Self {
        BValue::Int(i as i64)
    }
}

impl From<&str> for BValue {
    fn from(s: &str) -> Self {
        BValue::Bytes(s.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for BValue {
    fn from(b: Vec<u8>) -> Self {
        BValue::Bytes(b)
    }
}
