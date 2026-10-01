// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Encodeur bencode canonique (cles de dictionnaire triees
//! lexicographiquement par `BTreeMap`, comme exige par BEP 3 pour
//! l'integrite de l'info-hash).

use super::value::BValue;

/// Serialise une valeur en bencode.
pub fn encode(value: &BValue) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &BValue, out: &mut Vec<u8>) {
    match value {
        BValue::Int(i) => {
            out.push(b'i');
            out.extend_from_slice(i.to_string().as_bytes());
            out.push(b'e');
        }
        BValue::Bytes(b) => {
            out.extend_from_slice(b.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(b);
        }
        BValue::List(items) => {
            out.push(b'l');
            for item in items {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        BValue::Dict(map) => {
            out.push(b'd');
            // BTreeMap = iteration triee lexicographiquement.
            for (k, v) in map {
                out.extend_from_slice(k.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(k);
                encode_into(v, out);
            }
            out.push(b'e');
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn encode_aller_retour() {
        let src = b"d1:ai1e1:bli2e3:abcee";
        let v = super::super::parser::decode(src).unwrap();
        assert_eq!(encode(&v), src);
    }

    #[test]
    fn encode_dict_trie_les_cles() {
        // Cles volontairement desordonnees dans le map source.
        let mut map = BTreeMap::new();
        map.insert(b"z".to_vec(), BValue::Int(1));
        map.insert(b"a".to_vec(), BValue::Int(2));
        let v = BValue::Dict(map);
        assert_eq!(encode(&v), b"d1:ai2e1:zi1ee".to_vec());
    }
}
