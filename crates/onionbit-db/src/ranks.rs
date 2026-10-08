// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fonctions de ranking de recherche — port de
//! `tribler.core.database.ranks` (`torrent_rank` et ses composantes).
//!
//! `torrent_rank` est exposee a SQLite comme la fonction scalaire
//! `search_rank` (cf. `db.rs::configure`) — equivalent de la fonction
//! Python enregistree par `MetadataStore` pour l'`ORDER BY` de
//! pertinence des recherches `txt_filter`.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// Secondes dans un jour (`SECONDS_IN_DAY` Python).
const SECONDS_IN_DAY: f64 = 86_400.0;

// Coefficients empiriques Python (commentaire `ranks.py` : leurs
// valeurs exactes ne comptent que pour le classement relatif).
/// Le premier mot de la requete pese plus que les suivants.
const POSITION_COEFF: f64 = 5.0;
/// Penalite si un mot de la requete est absent du titre.
const MISSED_WORD_PENALTY: f64 = 10.0;
/// Penalite legera pour les mots de fin de titre absents de la
/// requete (plus grand = penalite plus faible).
const REMAINDER_COEFF: f64 = 10.0;
/// Mise a l'echelle de l'erreur totale vers un rang [0, 1].
const RANK_NORMALIZATION_COEFF: f64 = 10.0;

/// `item_rank` Python : rang d'une entree de resultat de recherche
/// distante (dict avec `name`, `num_seeders`, `num_leechers`,
/// `created` en secondes Unix).
pub fn item_rank(query: &str, name: &str, seeders: i64, leechers: i64, created: i64) -> f64 {
    let freshness = if created <= 0 {
        None
    } else {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as f64)
            .unwrap_or(0.0);
        Some(now - created as f64)
    };
    torrent_rank(query, name, seeders, leechers, freshness)
}

/// `torrent_rank` Python : rang global [0, 1] = titre x sante x
/// fraicheur. `freshness` = secondes depuis la creation (`None` ou
/// negatif = inconnu, equivalent du `None` Python).
pub fn torrent_rank(
    query: &str,
    title: &str,
    seeders: i64,
    leechers: i64,
    freshness: Option<f64>,
) -> f64 {
    let tr = title_rank(query, title);
    let sr = (seeders_rank(seeders, leechers) + 9.0) / 10.0;
    let fr = (freshness_rank(freshness) + 9.0) / 10.0;
    tr * sr * fr
}

/// `seeders_rank` Python : rang de sante normalise [0, 1].
pub fn seeders_rank(seeders: i64, leechers: i64) -> f64 {
    let sl = seeders.max(0) as f64 + leechers.max(0) as f64 * 0.1;
    sl / (100.0 + sl)
}

/// `freshness_rank` Python : decroissance en demi-vie de ~30 jours ;
/// inconnu/negatif = 0.
pub fn freshness_rank(freshness: Option<f64>) -> f64 {
    match freshness {
        Some(f) if f >= 0.0 => {
            let days = f / SECONDS_IN_DAY;
            1.0 / (1.0 + days / 30.0)
        }
        _ => 0.0,
    }
}

/// Decoupe en mots `\w+` Unicode (equivalent du `word_re` Python).
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// `title_rank` Python : similarite titre/requete [0, 1].
pub fn title_rank(query: &str, title: &str) -> f64 {
    calculate_rank(&words(query), &words(title))
}

/// `calculate_rank` Python : penalite ponderee par position pour les
/// mots absents/sautes + reste de titre non mentionne.
fn calculate_rank(query: &[String], title: &[String]) -> f64 {
    if query.is_empty() {
        return 1.0;
    }
    if title.is_empty() {
        return 0.0;
    }
    let mut q_title: VecDeque<String> = title.iter().cloned().collect();
    let mut total_error = 0.0;
    for (i, word) in query.iter().enumerate() {
        let word_weight = POSITION_COEFF / (POSITION_COEFF + i as f64);
        match find_word_and_rotate_title(word, &mut q_title) {
            (true, skipped) => total_error += skipped as f64 * word_weight,
            (false, _) => total_error += MISSED_WORD_PENALTY * word_weight,
        }
    }
    let remainder_weight = 1.0 / (REMAINDER_COEFF + query.len() as f64);
    total_error += q_title.len() as f64 * remainder_weight;
    RANK_NORMALIZATION_COEFF / (RANK_NORMALIZATION_COEFF + total_error)
}

/// `find_word_and_rotate_title` Python : trouve `word` dans le titre,
/// retourne `(trouve, mots_sautes)` ; le titre est tourne en place
/// (les mots sautes passent en fin, le mot trouve est consomme).
fn find_word_and_rotate_title(word: &str, title: &mut VecDeque<String>) -> (bool, usize) {
    match title.iter().position(|w| w == word) {
        Some(skipped) => {
            title.rotate_left(skipped);
            title.pop_front();
            (true, skipped)
        }
        None => (false, 0),
    }
}

/// Enregistre la fonction scalaire SQLite `search_rank(query, title,
/// seeders, leechers, freshness)` — meme signature que l'UDF Python
/// appelee dans l'`ORDER BY` de `get_entries_query`.
pub fn register_search_rank(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "search_rank",
        5,
        rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
            | rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let query: String = ctx.get(0)?;
            let title: String = ctx.get(1).unwrap_or_default();
            let seeders: i64 = ctx.get(2).unwrap_or(0);
            let leechers: i64 = ctx.get(3).unwrap_or(0);
            let freshness: f64 = ctx.get(4).unwrap_or(-1.0);
            Ok(torrent_rank(
                &query,
                &title,
                seeders,
                leechers,
                Some(freshness).filter(|f| *f >= 0.0),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titre_identique_rang_max() {
        let r = title_rank("big buck bunny", "big buck bunny");
        assert!(r > 0.9, "rang attendu proche de 1, obtenu {r}");
    }

    #[test]
    fn titre_sans_correspondance_rang_bas() {
        let r = title_rank("ubuntu iso", "completely unrelated title");
        assert!(r < 0.5, "rang attendu bas, obtenu {r}");
    }

    #[test]
    fn seeders_croissants_rang_croissant() {
        assert!(seeders_rank(100, 0) > seeders_rank(1, 0));
    }

    #[test]
    fn fraicheur_inconnue_est_zero() {
        assert_eq!(freshness_rank(None), 0.0);
        assert_eq!(freshness_rank(Some(-5.0)), 0.0);
        assert!(freshness_rank(Some(0.0)) > 0.9);
    }
}
