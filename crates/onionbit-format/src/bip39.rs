// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Phrase de recuperation BIP39 — 24 mots pour 32 octets d'entropie
//! (ADR-0016, `IdentitySeed` portable).
//!
//! Wordlists officielles anglaise et francaise vendored sous
//! `assets/` (2048 mots chacune, stockees en NFKD comme la spec
//! l'exige). Encodage : entropie 256 bits + checksum SHA-256 (8 bits)
//! = 264 bits = 24 groupes de 11 bits, chaque groupe indexant la
//! wordlist.
//!
//! Decodage : l'entree est normalisee **NFKD** avant decoupage
//! (macOS emet du NFD au clavier ; la spec BIP39 exige NFKD). Les
//! deux listes sont essayees — elles partagent ~100 mots identiques
//! (`abandon`, `accident`...) donc ce n'est pas la liste qui arbitre
//! mais le checksum SHA-256 : une phrase valide dans les deux listes
//! (theoriquement possible) est resolue par la premiere verification
//! de checksum reussie, l'anglais d'abord (liste canonique).
//!
//! Rigueur volontaire : un seul espace entre les mots, casse exacte.
//! `split_whitespace` tolerant masquerait les phrases mal copiees ;
//! un rejet type force l'utilisateur a ressaisir proprement.

use std::collections::HashMap;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Wordlist anglaise officielle BIP39 (2048 mots, NFKD).
const WORDS_EN_RAW: &str = include_str!("../assets/bip39_en.txt");
/// Wordlist francaise officielle BIP39 (2048 mots, NFKD).
const WORDS_FR_RAW: &str = include_str!("../assets/bip39_fr.txt");

/// Entropie transportee par la phrase : 32 octets → 24 mots.
pub const ENTROPY_LEN: usize = 32;
/// Nombre de mots de la phrase (264 bits / 11).
pub const WORD_COUNT: usize = 24;

/// Langue de la wordlist utilisee a l'encodage (locale UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// Wordlist anglaise (canonique).
    English,
    /// Wordlist francaise officielle.
    French,
}

/// Erreurs de decodage d'une phrase de recuperation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Bip39Error {
    /// Nombre de mots different de [`WORD_COUNT`].
    #[error("phrase de {got} mots — {WORD_COUNT} attendus")]
    BadWordCount {
        /// Nombre de mots effectivement lus.
        got: usize,
    },
    /// Mot absent des deux wordlists (faute de frappe, casse,
    /// langue non supportee ou melange de langues).
    #[error("mot inconnu : « {word} »")]
    UnknownWord {
        /// Le mot incrimine.
        word: String,
    },
    /// Les mots existent mais le checksum SHA-256 ne correspond
    /// pas — phrase corrompue (mot permute, mauvaise langue...).
    #[error("checksum de la phrase invalide")]
    ChecksumMismatch,
}

/// Wordlist d'une langue, indexee positionnellement.
fn wordlist(lang: Language) -> &'static [&'static str] {
    static EN: OnceLock<Vec<&'static str>> = OnceLock::new();
    static FR: OnceLock<Vec<&'static str>> = OnceLock::new();
    let cell = match lang {
        Language::English => &EN,
        Language::French => &FR,
    };
    cell.get_or_init(|| {
        let raw = match lang {
            Language::English => WORDS_EN_RAW,
            Language::French => WORDS_FR_RAW,
        };
        let words: Vec<&'static str> = raw.lines().collect();
        debug_assert_eq!(words.len(), 2048, "wordlist BIP39 = 2048 mots");
        words
    })
}

/// Index inverse `mot → position` d'une langue.
fn word_index(lang: Language) -> &'static HashMap<&'static str, u16> {
    static EN: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    static FR: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    let cell = match lang {
        Language::English => &EN,
        Language::French => &FR,
    };
    cell.get_or_init(|| {
        wordlist(lang)
            .iter()
            .enumerate()
            .map(|(i, w)| (*w, i as u16))
            .collect()
    })
}

/// Octet de checksum BIP39 : premier octet de SHA-256(entropie).
fn checksum(entropy: &[u8; ENTROPY_LEN]) -> u8 {
    Sha256::digest(entropy)[0]
}

/// Encode 32 octets d'entropie en phrase de 24 mots.
pub fn encode(entropy: &[u8; ENTROPY_LEN], lang: Language) -> String {
    // Flux de 33 octets : entropie + checksum — lu par groupes
    // de 11 bits MSB-first.
    let mut bits = [0u8; ENTROPY_LEN + 1];
    bits[..ENTROPY_LEN].copy_from_slice(entropy);
    bits[ENTROPY_LEN] = checksum(entropy);
    let words = wordlist(lang);
    let mut phrase = String::with_capacity(WORD_COUNT * 8);
    for i in 0..WORD_COUNT {
        let mut idx = 0u16;
        for b in 0..11 {
            let pos = i * 11 + b;
            idx = (idx << 1) | u16::from(bits[pos / 8] >> (7 - pos % 8) & 1);
        }
        if i > 0 {
            phrase.push(' ');
        }
        phrase.push_str(words[idx as usize]);
    }
    phrase
}

/// Resout chaque mot contre une wordlist. `Err(word)` = premier mot
/// absent de cette liste.
fn resolve(tokens: &[&str], lang: Language) -> Result<[u16; WORD_COUNT], String> {
    let index = word_index(lang);
    let mut out = [0u16; WORD_COUNT];
    for (i, tok) in tokens.iter().enumerate() {
        match index.get(tok) {
            Some(&idx) => out[i] = idx,
            None => return Err((*tok).to_string()),
        }
    }
    Ok(out)
}

/// Indices → entropie, avec verification du checksum.
fn indices_to_entropy(indices: &[u16; WORD_COUNT]) -> Result<[u8; ENTROPY_LEN], Bip39Error> {
    let mut bits = [0u8; ENTROPY_LEN + 1];
    for (i, &idx) in indices.iter().enumerate() {
        for b in 0..11 {
            if idx & (1 << (10 - b)) != 0 {
                let pos = i * 11 + b;
                bits[pos / 8] |= 1 << (7 - pos % 8);
            }
        }
    }
    let mut entropy = [0u8; ENTROPY_LEN];
    entropy.copy_from_slice(&bits[..ENTROPY_LEN]);
    if bits[ENTROPY_LEN] != checksum(&entropy) {
        return Err(Bip39Error::ChecksumMismatch);
    }
    Ok(entropy)
}

/// Decode une phrase de 24 mots en ses 32 octets d'entropie.
///
/// Normalisation NFKD en entree. Les listes EN et FR sont essayees
/// chacune en entier (une phrase mono-langue) : le checksum arbitre
/// les ~100 mots partages entre les deux listes.
pub fn decode(phrase: &str) -> Result<[u8; ENTROPY_LEN], Bip39Error> {
    let normalized: String = phrase.nfkd().collect();
    let trimmed = normalized.trim();
    // Decoupage strict sur l'espace simple : un double espace produit
    // un token vide → `BadWordCount` (rejet type, pas de tolerance
    // silencieuse sur un secret).
    let tokens: Vec<&str> = trimmed.split(' ').collect();
    let real_words = tokens.iter().filter(|t| !t.is_empty()).count();
    if tokens.len() != WORD_COUNT || real_words != WORD_COUNT {
        return Err(Bip39Error::BadWordCount { got: real_words });
    }
    // Les deux listes sont essayees chacune en entier ; le checksum
    // arbitre — une phrase de mots partages EN∩FR est affectee a la
    // liste dont le checksum passe (anglais d'abord, deterministe).
    // Une liste qui resout tous les mots mais echoue au checksum
    // prevaut en erreur sur un simple mot inconnu : la phrase est
    // structurellement plausible, c'est le checksum qui la trahit.
    let mut checksum_failed = false;
    let mut first_unknown: Option<String> = None;
    for attempt in [
        resolve(&tokens, Language::English),
        resolve(&tokens, Language::French),
    ] {
        match attempt {
            Ok(indices) => match indices_to_entropy(&indices) {
                Ok(entropy) => return Ok(entropy),
                Err(_) => checksum_failed = true,
            },
            Err(word) => {
                if first_unknown.is_none() {
                    first_unknown = Some(word);
                }
            }
        }
    }
    if checksum_failed {
        return Err(Bip39Error::ChecksumMismatch);
    }
    // Aucune liste n'a resolu : privilegier un mot absent des DEUX
    // listes (vrai inconnu) ; sinon melange EN/FR → premier mot hors
    // liste anglaise rapporte.
    let unknown = tokens
        .iter()
        .find(|t| {
            word_index(Language::English).get(*t).is_none()
                && word_index(Language::French).get(*t).is_none()
        })
        .map(|t| (*t).to_string())
        .or(first_unknown)
        .unwrap_or_default();
    Err(Bip39Error::UnknownWord { word: unknown })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(hex_str: &str) -> [u8; ENTROPY_LEN] {
        hex::decode(hex_str).unwrap().try_into().unwrap()
    }

    // Vecteurs officiels BIP39 (trezor/python-mnemonic `vectors.json`,
    // entropie 256 bits → 24 mots anglais).
    const VECTORS: [(&str, &str); 5] = [
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        ),
        (
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
        ),
        (
            "8080808080808080808080808080808080808080808080808080808080808080",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
        ),
        (
            "9f6a2878b2520799a44ef18bc7df394e7061a224d2c33cd015b157d746869863",
            "panda eyebrow bullet gorilla call smoke muffin taste mesh discover soft ostrich alcohol speed nation flash devote level hobby quick inner drive ghost inside",
        ),
    ];

    #[test]
    fn vecteurs_officiels_anglais() {
        for (hex, phrase) in VECTORS {
            let entropy = ent(hex);
            assert_eq!(encode(&entropy, Language::English), phrase, "encode {hex}");
            assert_eq!(decode(phrase).unwrap(), entropy, "decode {hex}");
        }
    }

    #[test]
    fn aller_retour_francais() {
        let entropy = ent("9f6a2878b2520799a44ef18bc7df394e7061a224d2c33cd015b157d746869863");
        let phrase = encode(&entropy, Language::French);
        assert_eq!(phrase.split(' ').count(), WORD_COUNT);
        assert_eq!(decode(&phrase).unwrap(), entropy);
        // Memes indices → mots differents : la phrase FR n'est pas
        // l'anglaise (controle de la wordlist chargee).
        assert_ne!(phrase, encode(&entropy, Language::English));
    }

    #[test]
    fn decode_nfc_normalise_en_nfkd() {
        // macOS saisit en NFC — le decodeur doit accepter la forme
        // composee d'une phrase francaise stockee en NFKD.
        let entropy = ent("066dca1a2bb7e8a1db2832148ce9933eea0f3ac9548d793112d9a95c9407efad");
        let phrase_nfkd = encode(&entropy, Language::French);
        let phrase_nfc: String = phrase_nfkd.nfc().collect();
        if phrase_nfc != phrase_nfkd {
            assert_eq!(decode(&phrase_nfc).unwrap(), entropy);
        }
        // Et la forme decomposee elle-meme, evidemment.
        assert_eq!(decode(&phrase_nfkd).unwrap(), entropy);
    }

    #[test]
    fn hostiles_rejets_types() {
        let ok = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        // 23 et 25 mots.
        assert!(matches!(
            decode(ok.rsplit_once(' ').unwrap().0),
            Err(Bip39Error::BadWordCount { got: 23 })
        ));
        assert!(matches!(
            decode(&format!("{ok} art")),
            Err(Bip39Error::BadWordCount { got: 25 })
        ));
        // Mot hors wordlist.
        assert!(matches!(
            decode(&ok.replacen("art", "xyzzy", 1)),
            Err(Bip39Error::UnknownWord { .. })
        ));
        // Checksum faux (dernier mot valide mais incoherent).
        assert!(matches!(
            decode(&ok.replacen("art", "zoo", 1)),
            Err(Bip39Error::ChecksumMismatch)
        ));
        // Casse mixte → mot inconnu (rejet, pas de lowercasing magique).
        assert!(matches!(
            decode(&ok.replacen("art", "Art", 1)),
            Err(Bip39Error::UnknownWord { .. })
        ));
        // Double espace → BadWordCount (token vide ignore du compte).
        assert!(matches!(
            decode(&ok.replacen(" abandon art", "  abandon art", 1)),
            Err(Bip39Error::BadWordCount { .. })
        ));
    }
}
