// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Recherche augmentee (« slow search ») — port comportemental de
//! `tribler.core.database.augmenter.AugmentedSearch`.
//!
//! Python entraine un tokenizer SentencePiece unigramme (8000 pieces)
//! sur les titres de torrents vus et decoupe la requete en sous-mots,
//! puis genere un SQL `LIKE` avec permutations tolerant aux fautes de
//! segmentation. SentencePiece n'a pas d'equivalent Rust utilisable
//! (bindings C++ uniquement) : le vocabulaire est appris ici par
//! comptage de sous-chaines (`▁` prefixe les debuts de mots, comme le
//! marqueur SentencePiece) et le decodage est un Viterbi sur ces
//! comptages — meme shape de pieces, meme SQL genere.
//!
//! Persistance : `_m_torrent_titles.model` (vocabulaire JSON
//! piece -> compte, format propre — le binaire SentencePiece est
//! proprietaire) et `_m_torrent_titles.cache.txt` (titres en attente
//! d'apprentissage, JSON identique au Python).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Taille cible du vocabulaire (`vocab_size=8000` Python).
const VOCAB_SIZE: usize = 8000;
/// `max_title_length` Python.
const MAX_TITLE_LENGTH: usize = 4192;
/// Longueur maximale (en caracteres) d'une piece candidate a
/// l'apprentissage — equivalent implicite du modele unigramme.
const MAX_PIECE_LEN: usize = 16;
/// Seuil de declenchement de `study` (`len(title_window) > 50`).
const STUDY_THRESHOLD: usize = 50;
/// Marqueur de debut de mot (equivalent du `▁` SentencePiece).
const WORD_MARK: char = '\u{2581}';

/// Nom du fichier de vocabulaire (`_m_torrent_titles.model` Python).
const MODEL_FILE: &str = "_m_torrent_titles.model";
/// Cache des titres en attente (`_m_torrent_titles.cache.txt`).
const CACHE_FILE: &str = "_m_torrent_titles.cache.txt";

struct Inner {
    /// Vocabulaire appris : piece -> compte cumule dans le corpus.
    vocab: HashMap<String, u64>,
    /// `title_window` Python : titres recus en attente d'apprentissage.
    title_window: Vec<String>,
    /// Cache charge depuis le disque (`initialized` Python).
    initialized: bool,
}

/// Augmenteur de requetes : vocabulaire de sous-mots appris des
/// titres + generation du SQL `LIKE` permute.
pub struct Augmenter {
    inner: Mutex<Inner>,
    /// `study` en cours (`schedule_study` n'empile pas deux taches).
    studying: AtomicBool,
    model_file: PathBuf,
    cache_file: PathBuf,
}

impl Augmenter {
    /// Cree l'augmenter pour `state_dir` et recharge le vocabulaire.
    /// ADR-0018 etape 58 : les fichiers vivent sous `state/cache/` —
    /// `state_cache_file` garde la lecture du plat historique tant
    /// que la migration de layout n'a pas tourne.
    pub fn new(state_dir: &Path) -> Self {
        let roots = crate::paths::PathRoots::for_state_dir(state_dir);
        let model_file = roots.state_cache_file(MODEL_FILE);
        let cache_file = roots.state_cache_file(CACHE_FILE);
        // Les ecritures (`study`, `flush_cache`) sont best-effort —
        // le dossier doit exister pour qu'elles reussissent.
        if let Some(dir) = model_file.parent().or(cache_file.parent()) {
            let _ = std::fs::create_dir_all(dir);
        }
        let vocab = std::fs::read_to_string(&model_file)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, u64>>(&s).ok())
            .unwrap_or_default();
        Self {
            inner: Mutex::new(Inner {
                vocab,
                title_window: Vec::new(),
                initialized: false,
            }),
            studying: AtomicBool::new(false),
            model_file,
            cache_file,
        }
    }

    /// `notifier.add(torrent_metadata_added, consume)` Python : tache
    /// qui consomme les notifications `TorrentMetadataCreated`.
    pub fn spawn_consumer(self: &std::sync::Arc<Self>, notifier: crate::notifier::Notifier) {
        let mut rx = notifier.subscribe();
        let this = std::sync::Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(crate::notifier::Notification::TorrentMetadataCreated { title, .. }) => {
                        this.consume_title(&title)
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// `consume_torrent_metadata` Python : empile le titre et lance
    /// `study` des que la fenetre depasse le seuil.
    pub fn consume_title(&self, title: &str) {
        let titles = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !inner.initialized {
                if let Ok(content) = std::fs::read_to_string(&self.cache_file) {
                    inner.title_window = serde_json::from_str(&content).unwrap_or_default();
                }
                inner.initialized = true;
            }
            inner
                .title_window
                .push(title.chars().take(MAX_TITLE_LENGTH).collect());
            if inner.title_window.len() <= STUDY_THRESHOLD {
                return;
            }
            std::mem::take(&mut inner.title_window)
        };
        self.schedule_study(titles);
    }

    /// `schedule_study` Python : une seule tache d'apprentissage a la
    /// fois.
    fn schedule_study(&self, titles: Vec<String>) {
        if titles.is_empty() || self.studying.swap(true, Ordering::SeqCst) {
            return;
        }
        self.study(titles);
        self.studying.store(false, Ordering::SeqCst);
    }

    /// `study` Python : fusionne les comptages de sous-chaines des
    /// nouveaux titres dans le vocabulaire, tronque a `VOCAB_SIZE`
    /// (moins 50 pieces par nouveau titre reservees, comme le budget
    /// `8000 - 50*len(titles)` de l'entrainement Python) et persiste.
    fn study(&self, titles: Vec<String>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for title in &titles {
            let normalized: String = title
                .chars()
                .map(|c| if c.is_whitespace() { WORD_MARK } else { c })
                .collect();
            let chars: Vec<char> = normalized.chars().collect();
            for i in 0..chars.len() {
                for len in 1..=MAX_PIECE_LEN.min(chars.len() - i) {
                    let piece: String = chars[i..i + len].iter().collect();
                    *inner.vocab.entry(piece).or_insert(0) += 1;
                }
            }
        }
        if inner.vocab.len() > VOCAB_SIZE {
            // Garde les pieces les plus utiles : frequence x
            // longueur (une piece longue vue souvent couvre plus).
            let mut ranked: Vec<(String, u64)> = inner.vocab.drain().collect();
            ranked.sort_by_key(|(p, c)| {
                std::cmp::Reverse(c.saturating_mul(p.chars().count() as u64))
            });
            ranked.truncate(VOCAB_SIZE.saturating_sub(50 * titles.len().min(10)));
            // Les caracteres seuls restent : ils garantissent la
            // couverture totale de l'encodeur.
            inner.vocab = ranked.into_iter().collect();
        }
        // Serialise sous verrou, mais l'ecriture disque se fait
        // apres relachement — un `fs::write` bloquant sous `inner`
        // gelait tous les `encode()` de recherche concurrents.
        let json = serde_json::to_string(&inner.vocab).ok();
        drop(inner);
        if let Some(json) = json {
            let _ = std::fs::write(&self.model_file, json);
        }
        let _ = std::fs::remove_file(&self.cache_file);
    }

    /// `needs_kickstart` Python : vocabulaire vide -> amorcer depuis
    /// la base (`seed_augmenter` envoie 10000 titres).
    pub fn needs_kickstart(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .vocab
            .is_empty()
    }

    /// `seed_augmenter` Python : empile les titres de la base dans la
    /// fenetre d'apprentissage.
    pub fn seed(&self, titles: Vec<String>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .title_window
            .extend(titles);
    }

    /// `schedule_study` immediat : vide la fenetre et apprend (pour
    /// le kickstart apres `seed`).
    pub fn study_pending(&self) {
        let titles = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut inner.title_window)
        };
        self.schedule_study(titles);
    }

    /// `on_shutdown` Python : vide le cache des titres en attente.
    pub fn flush_cache(&self) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(json) = serde_json::to_string(&inner.title_window) {
            let _ = std::fs::write(&self.cache_file, json);
        }
    }

    /// `processor.encode` : decoupe `text` en pieces du vocabulaire
    /// par Viterbi (maximise la somme des log-comptes ponderes par la
    /// longueur — decodage unigramme equivalent). Les espaces sont
    /// remplacees par `▁` avant decoupage.
    fn encode(&self, text: &str) -> Vec<String> {
        let normalized: String = text
            .chars()
            .map(|c| if c.is_whitespace() { WORD_MARK } else { c })
            .collect();
        let chars: Vec<char> = normalized.chars().collect();
        let n = chars.len();
        let vocab = &self.inner.lock().unwrap_or_else(|e| e.into_inner()).vocab;
        // dp[i] = meilleur score jusqu'au char i + piece terminant a i.
        let mut dp = vec![f64::NEG_INFINITY; n + 1];
        let mut back: Vec<Option<(usize, usize)>> = vec![None; n + 1];
        dp[0] = 0.0;
        for i in 0..n {
            if dp[i].is_infinite() {
                continue;
            }
            for len in 1..=MAX_PIECE_LEN.min(n - i) {
                let piece: String = chars[i..i + len].iter().collect();
                let count = vocab
                    .get(&piece)
                    .copied()
                    .unwrap_or(if len == 1 { 1 } else { 0 });
                if count == 0 {
                    continue;
                }
                // Score : log(compte) bonifie par la couverture —
                // favorise les pieces longues frequentes.
                let score = dp[i] + (count as f64).ln() * (len as f64);
                if score > dp[i + len] {
                    dp[i + len] = score;
                    back[i + len] = Some((i, len));
                }
            }
        }
        if back[n].is_none() && n > 0 {
            return Vec::new();
        }
        let mut pieces = Vec::new();
        let mut i = n;
        while i > 0 {
            let Some((start, _len)) = back[i] else {
                return Vec::new();
            };
            pieces.push(chars[start..i].iter().collect::<String>());
            i = start;
        }
        pieces.reverse();
        pieces
    }

    /// `to_phrases` Python : regroupe les pieces en mots — une piece
    /// commencant par `▁` ouvre une nouvelle phrase (`▁` seul ne
    /// contribue rien).
    fn to_phrases(pieces: &[String]) -> Vec<Vec<String>> {
        let mut phrases: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for piece in pieces {
            if piece.starts_with(WORD_MARK) {
                if !current.is_empty() {
                    phrases.push(std::mem::take(&mut current));
                }
                let rest = &piece[WORD_MARK.len_utf8()..];
                if !rest.is_empty() {
                    current.push(rest.to_string());
                }
            } else {
                current.push(piece.clone());
            }
        }
        if !current.is_empty() {
            phrases.push(current);
        }
        phrases
    }

    /// `augment` Python : genere `SELECT rowid FROM channel_node WHERE
    /// <conjonction LIKE permutee> LIMIT <limit> OFFSET <offset>` et
    /// ses parametres — port exact de la logique de permutations.
    pub fn augment(&self, search: &str, limit: usize, offset: usize) -> (String, Vec<String>) {
        let pieces = self.encode(search);
        // LIMIT/OFFSET inlines dans les deux branches (entiers
        // controles, comme Python) : les parametres ne portent que
        // les motifs `title LIKE ?` — le compte `?` correspond
        // toujours a `parameters.len()`.
        if pieces.is_empty() {
            return (
                format!(
                    "SELECT rowid FROM channel_node WHERE title LIKE ? LIMIT {limit} OFFSET {offset}"
                ),
                vec!["%".into()],
            );
        }
        let phrases = Self::to_phrases(&pieces);
        let mut conjunction: Vec<String> = Vec::new();
        let mut parameters: Vec<String> = Vec::new();

        for phrase in &phrases {
            if phrase.len() == 1 {
                parameters.push(format!("%{}%", phrase[0]));
                conjunction.push("title LIKE ?".into());
            } else {
                // Une disjonction par piece omise : le motif permute
                // concatene les autres pieces avec `%` a la place de
                // la piece retiree.
                let mut disjunction_len = 0;
                for i in 0..phrase.len() {
                    let mut perm = String::new();
                    for (j, piece) in phrase.iter().enumerate() {
                        if i != j && phrase.len() > 1 {
                            perm.push_str(piece);
                            if j + 1 == i {
                                perm.push('%');
                            }
                        }
                    }
                    if !perm.is_empty() {
                        disjunction_len += 1;
                        parameters.push(format!(
                            "%{perm}{}",
                            if perm.ends_with('%') { "" } else { "%" }
                        ));
                    }
                }
                conjunction.push(format!(
                    "({})",
                    vec!["title LIKE ?"; disjunction_len].join(" OR ")
                ));
            }
        }

        let mut conj_str = conjunction.join(" AND ");
        if conj_str.len() <= 2 {
            // Cas degenere Python : `()` pour une phrase unique vide.
            conj_str = "title LIKE ?".into();
            parameters = vec![format!("%{}%", phrases[0].join(""))];
        }
        let query = format!(
            "SELECT rowid FROM channel_node WHERE {conj_str} LIMIT {limit} OFFSET {offset}"
        );
        tracing::debug!(%query, ?parameters, "requete augmentee");
        (query, parameters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le nombre de `?` du SQL rendu correspond toujours a
    /// `parameters.len()` — la branche « sans pieces » bindait
    /// aussi LIMIT/OFFSET en parametres chaines, l'autre les
    /// inlinait : bindings incoherents selon le decoupage.
    #[test]
    fn augment_bind_coherent_sur_les_deux_branches() {
        let dir = tempfile::tempdir().unwrap();
        let aug = Augmenter::new(dir.path());
        for search in ["", "abc def"] {
            let (sql, params) = aug.augment(search, 5, 3);
            assert_eq!(
                sql.matches('?').count(),
                params.len(),
                "{sql} vs {params:?}"
            );
            assert!(sql.contains("LIMIT 5 OFFSET 3"), "{sql}");
        }
    }
}
