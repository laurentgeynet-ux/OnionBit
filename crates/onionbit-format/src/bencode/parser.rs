// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decodeur bencode (BEP 3) borne.
//!
//! Le parse est iteratif-friendly via recursion controlee par
//! [`crate::limits::MAX_BENCODE_DEPTH`], et les tailles de chaines sont
//! bornees par la taille totale de l'entree (pas d'allocation sur
//! confiance). Les cles de dictionnaire ne sont pas exigees triees en
//! entree (des fichiers du monde reel peuvent etre non canoniques), mais
//! elles sont stockees dans un `BTreeMap` donc re-ordonnees
//! canoniquement a la re-serialisation.

use std::collections::BTreeMap;

use super::value::BValue;
use crate::error::FormatError;
use crate::limits;

/// Resultat d'un parse : la valeur + l'offset juste apres sa fin.
pub struct Parsed {
    pub value: BValue,
    /// Offset du premier octet apres la valeur parsee.
    pub end: usize,
}

/// Decode une valeur bencode complete depuis `data`.
///
/// Retourne une erreur si des octets non vides suivent la valeur racine
/// (sauf si `allow_trailing` est vrai).
pub fn decode(data: &[u8]) -> Result<BValue, FormatError> {
    decode_at(data, 0, 0).and_then(|p| {
        if p.end != data.len() {
            Err(FormatError::BadBencode {
                offset: p.end,
                reason: format!(
                    "octets restants non consommes ({} restants)",
                    data.len() - p.end
                ),
            })
        } else {
            Ok(p.value)
        }
    })
}

/// Decode une valeur bencode en commencant a `offset`, et retourne aussi
/// l'offset de fin (utile pour extraire le dictionnaire `info` brut d'un
/// .torrent sans le re-serialiser).
pub fn decode_at(data: &[u8], offset: usize, depth: usize) -> Result<Parsed, FormatError> {
    if depth > limits::MAX_BENCODE_DEPTH {
        return Err(FormatError::DepthExceeded {
            max: limits::MAX_BENCODE_DEPTH,
        });
    }
    let first = *data.get(offset).ok_or(FormatError::Truncated { offset })?;
    match first {
        b'i' => decode_int(data, offset),
        b'l' => decode_list(data, offset, depth),
        b'd' => decode_dict(data, offset, depth),
        b'0'..=b'9' => decode_bytes(data, offset),
        _ => Err(FormatError::BadBencode {
            offset,
            reason: format!("octet inattendu 0x{first:02x}"),
        }),
    }
}

fn decode_int(data: &[u8], offset: usize) -> Result<Parsed, FormatError> {
    let start = offset + 1;
    let end = find_byte(data, b'e', start)?;
    let digits = &data[start..end];
    // BEP 3 : pas de zeros de tete ("i03e" illegal), "-0" illegal.
    if digits.is_empty()
        || (digits[0] == b'0' && digits.len() > 1)
        || (digits[0] == b'-' && (digits.len() == 1 || digits[1] == b'0'))
    {
        return Err(FormatError::BadBencode {
            offset,
            reason: "entier mal forme".into(),
        });
    }
    let s = core::str::from_utf8(digits).map_err(|_| FormatError::BadBencode {
        offset,
        reason: "entier non ASCII".into(),
    })?;
    let value = s.parse::<i64>().map_err(|_| FormatError::BadBencode {
        offset,
        reason: format!("entier hors bornes: {s}"),
    })?;
    Ok(Parsed {
        value: BValue::Int(value),
        end: end + 1,
    })
}

fn decode_bytes(data: &[u8], offset: usize) -> Result<Parsed, FormatError> {
    let colon = find_byte(data, b':', offset)?;
    let len_str = &data[offset..colon];
    if len_str.is_empty() || (len_str[0] == b'0' && len_str.len() > 1) {
        return Err(FormatError::BadBencode {
            offset,
            reason: "longueur de chaine mal formee".into(),
        });
    }
    let len: usize = core::str::from_utf8(len_str)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FormatError::BadBencode {
            offset,
            reason: "longueur de chaine non numerique".into(),
        })?;
    let start = colon + 1;
    let end = start.checked_add(len).ok_or(FormatError::BadBencode {
        offset,
        reason: "longueur de chaine en overflow".into(),
    })?;
    if end > data.len() {
        return Err(FormatError::Truncated { offset });
    }
    Ok(Parsed {
        value: BValue::Bytes(data[start..end].to_vec()),
        end,
    })
}

fn decode_list(data: &[u8], offset: usize, depth: usize) -> Result<Parsed, FormatError> {
    let mut pos = offset + 1;
    let mut items = Vec::new();
    loop {
        match data.get(pos) {
            None => return Err(FormatError::Truncated { offset: pos }),
            Some(b'e') => {
                return Ok(Parsed {
                    value: BValue::List(items),
                    end: pos + 1,
                })
            }
            Some(_) => {
                let p = decode_at(data, pos, depth + 1)?;
                items.push(p.value);
                pos = p.end;
            }
        }
    }
}

fn decode_dict(data: &[u8], offset: usize, depth: usize) -> Result<Parsed, FormatError> {
    let mut pos = offset + 1;
    let mut map = BTreeMap::new();
    loop {
        match data.get(pos) {
            None => return Err(FormatError::Truncated { offset: pos }),
            Some(b'e') => {
                return Ok(Parsed {
                    value: BValue::Dict(map),
                    end: pos + 1,
                })
            }
            Some(b'0'..=b'9') => {
                let key = decode_bytes(data, pos)?;
                let val = decode_at(data, key.end, depth + 1)?;
                let key_bytes = match key.value {
                    BValue::Bytes(b) => b,
                    _ => unreachable!("decode_bytes retourne toujours Bytes"),
                };
                map.insert(key_bytes, val.value);
                pos = val.end;
            }
            Some(_) => {
                return Err(FormatError::BadBencode {
                    offset: pos,
                    reason: "cle de dictionnaire non-chaine".into(),
                })
            }
        }
    }
}

/// Trouve l'index du premier octet `needle` a partir de `offset`.
fn find_byte(data: &[u8], needle: u8, offset: usize) -> Result<usize, FormatError> {
    data[offset..]
        .iter()
        .position(|&b| b == needle)
        .map(|i| offset + i)
        .ok_or(FormatError::Truncated { offset })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_entier() {
        assert_eq!(decode(b"i42e").unwrap(), BValue::Int(42));
        assert_eq!(decode(b"i-7e").unwrap(), BValue::Int(-7));
        assert_eq!(decode(b"i0e").unwrap(), BValue::Int(0));
    }

    #[test]
    fn decode_chaine() {
        assert_eq!(decode(b"4:spam").unwrap(), BValue::Bytes(b"spam".to_vec()));
        assert_eq!(decode(b"0:").unwrap(), BValue::Bytes(vec![]));
    }

    #[test]
    fn decode_liste_et_dict() {
        let v = decode(b"li42e3:abce").unwrap();
        assert_eq!(
            v,
            BValue::List(vec![BValue::Int(42), BValue::Bytes(b"abc".to_vec())])
        );
        let d = decode(b"d1:ai1e1:bi2ee").unwrap();
        assert_eq!(d.get_str("a").unwrap().as_int(), Some(1));
        assert_eq!(d.get_str("b").unwrap().as_int(), Some(2));
    }

    #[test]
    fn rejections() {
        assert!(decode(b"i03e").is_err()); // zero de tete
        assert!(decode(b"i-0e").is_err()); // -0
        assert!(decode(b"3:ab").is_err()); // tronque
        assert!(decode(b"i42eX").is_err()); // residu
        assert!(decode(b"l").is_err()); // liste non fermee
    }

    #[test]
    fn profondeur_maximale_bornee() {
        // Liste imbriquee a profondeur > MAX_BENCODE_DEPTH.
        let mut s = vec![b'l'; limits::MAX_BENCODE_DEPTH + 2];
        s.extend_from_slice(&[b'e'; limits::MAX_BENCODE_DEPTH + 2]);
        assert!(decode(&s).is_err());
    }
}
