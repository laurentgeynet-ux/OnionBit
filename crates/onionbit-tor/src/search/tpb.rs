// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backend `tpb` — scraping borne de la page de recherche des
//! miroirs The Pirate Bay `.onion` (ADR-0028 §5).
//!
//! Structure attendue (markup TPB classique) : `<table
//! id="searchResult">` dont chaque ligne `<tr>` porte le titre dans
//! `a.detLink`, le magnet dans `a[href^="magnet:"]`, la categorie
//! dans `td.vertTh`, la taille dans le texte `.detDesc` (« Size X
//! GiB ») et SE/LE dans les deux dernieres cellules
//! `td[align="right"]`. Une ligne sans magnet valide est ignoree ;
//! si **aucune** ligne n'est extractible la page est consideree
//! restructuree (`StructureChanged`).

use std::sync::Arc;
use std::sync::OnceLock;

use scraper::{Html, Selector};

use onionbit_format::magnet::MagnetLink;

use crate::error::{ExternalSearchError, TorError};
use crate::http::OnionResponse;
use crate::settings::OnionBackendSettings;

use super::{ExternalResult, OnionFetch, OnionSearchBackend};

/// Selecteurs statiques — compiles une fois (`Selector::parse` est
/// faillible en signature ; ces chaines constantes ne peuvent pas
/// echouer).
macro_rules! sel {
    ($lit:literal) => {
        Selector::parse($lit).expect("selecteur CSS statique invalide")
    };
}

fn sel_table() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("#searchResult"))
}
fn sel_rows() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("#searchResult tr"))
}
fn sel_name() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("a.detLink"))
}
fn sel_magnet() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("a[href^='magnet:']"))
}
fn sel_category() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("td.vertTh"))
}
fn sel_desc() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!(".detDesc"))
}
fn sel_seedleech() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| sel!("td[align='right']"))
}

/// Borne interne du titre extrait — defensive, pas un reglage : la
/// page est un contenu tiers non fiable.
const MAX_NAME_CHARS: usize = 300;

/// Borne interne de la requete utilisateur avant encodage.
const MAX_QUERY_CHARS: usize = 300;

/// Marqueurs heuristiques d'une page de defi anti-DDoS (Cloudflare,
/// DDoS-Guard, PoW natif TPB…). Consultes en minuscules sur le
/// corps.
const CHALLENGE_MARKERS: &[&str] = &[
    "ddos-guard",
    "just a moment",
    "checking your browser",
    "verify you are human",
    "captcha",
    "challenge-platform",
    "pow challenge",
    "security check",
];

/// Marqueurs d'une page de resultats vide legitime — distingue
/// `NoResults` d'une refonte (`StructureChanged`).
const EMPTY_MARKERS: &[&str] = &[
    "no results",
    "no hits",
    "did not match",
    "nothing found",
    "0 results",
];

/// Backend de recherche `tpb` : failover sur `mirrors` puis
/// scraping borne.
pub struct TpbBackend {
    /// Identifiant de source (= cle de config, ex. `"tpb"`).
    source: String,
    /// Miroirs + template + borne — deja valides au chargement.
    cfg: OnionBackendSettings,
    /// Transport injecte (`TorRuntime` en production).
    fetcher: Arc<dyn OnionFetch>,
}

/// Issue d'un essai sur un miroir — interne, pliee dans la
/// classification finale.
enum Outcome {
    /// Reponse exploitable.
    Results(Vec<ExternalResult>),
    /// Page de defi anti-DDoS.
    Captcha,
    /// Reponse 200 sans lignes extractibles ni marqueur « vide ».
    StructureChanged,
    /// Le site repond « aucun resultat ».
    NoResults,
    /// Echec transport ou statut inattendu — miroir injoignable.
    Unreachable,
}

impl TpbBackend {
    pub fn new(source: &str, cfg: &OnionBackendSettings, fetcher: Arc<dyn OnionFetch>) -> Self {
        Self {
            source: source.to_string(),
            cfg: cfg.clone(),
            fetcher,
        }
    }

    /// URL de recherche sur `mirror` : `{host}`/`{q}` substitues,
    /// requete percent-encodee (form-urlencoded — `+` pour espace,
    /// convention des templates TPB).
    fn search_url(&self, mirror: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        self.cfg
            .url_template
            .replace("{host}", mirror)
            .replace("{q}", &encoded)
    }

    /// Classe la reponse d'un miroir — la page de defi est detectee
    /// par statut (403/503) **et** marqueurs de corps.
    fn classify(&self, resp: &OnionResponse) -> Outcome {
        let body = String::from_utf8_lossy(&resp.body);
        if resp.status != 200 {
            return if matches!(resp.status, 403 | 503) || has_marker(&body, CHALLENGE_MARKERS) {
                Outcome::Captcha
            } else {
                Outcome::Unreachable
            };
        }
        if has_marker(&body, CHALLENGE_MARKERS) {
            return Outcome::Captcha;
        }
        match parse_results(&body, &self.source, self.cfg.max_results) {
            Some(results) => Outcome::Results(results),
            None if has_marker(&body, EMPTY_MARKERS) => Outcome::NoResults,
            None => Outcome::StructureChanged,
        }
    }
}

#[async_trait::async_trait]
impl OnionSearchBackend for TpbBackend {
    async fn search(
        &self,
        query: &str,
    ) -> std::result::Result<Vec<ExternalResult>, ExternalSearchError> {
        // Nettoyage : trim + caracteres de controle retires, borne.
        let clean: String = query
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_QUERY_CHARS)
            .collect::<String>()
            .trim()
            .to_string();
        if clean.is_empty() {
            return Err(ExternalSearchError::EmptyQuery);
        }

        let mut tried = 0usize;
        let mut saw_captcha = false;
        let mut saw_structure = false;
        let mut saw_noresults = false;
        for mirror in &self.cfg.mirrors {
            let url = self.search_url(mirror, &clean);
            match self.fetcher.fetch(&url).await {
                // Fatal pour tous les miroirs : pas la peine de
                // retenter (meme cause, meme effet).
                Err(TorError::Disabled) => return Err(ExternalSearchError::TorDisabled),
                Err(TorError::NotBuilt | TorError::NotReady) => {
                    return Err(ExternalSearchError::TorNotReady)
                }
                Err(_) => tried += 1,
                Ok(resp) => {
                    tried += 1;
                    match self.classify(&resp) {
                        Outcome::Results(rows) => return Ok(rows),
                        Outcome::Captcha => saw_captcha = true,
                        Outcome::StructureChanged => saw_structure = true,
                        Outcome::NoResults => saw_noresults = true,
                        Outcome::Unreachable => {}
                    }
                }
            }
        }
        // Priorite de diagnostic : un miroir qui a repondu « aucun
        // resultat » est une reponse valide ; un defi ou une refonte
        // est plus informatif qu'un simple injoignable.
        Err(if saw_noresults {
            ExternalSearchError::NoResults
        } else if saw_captcha {
            ExternalSearchError::CaptchaDetected
        } else if saw_structure {
            ExternalSearchError::StructureChanged
        } else {
            ExternalSearchError::MirrorUnreachable { tried }
        })
    }
}

/// `true` si le corps (minuscules) contient un marqueur de defi.
fn has_marker(body: &str, markers: &[&str]) -> bool {
    let lower = body.to_lowercase();
    markers.iter().any(|m| lower.contains(m))
}

/// Extrait les lignes de resultats. `None` si le markup attendu est
/// absent (table `#searchResult` introuvable ou zero ligne valide).
fn parse_results(body: &str, source: &str, max_results: usize) -> Option<Vec<ExternalResult>> {
    let doc = Html::parse_document(body);
    // La table de resultats doit exister pour que la page soit
    // consideree « structure attendue » (meme vide).
    doc.select(sel_table()).next()?;
    let mut out = Vec::new();
    for row in doc.select(sel_rows()) {
        if out.len() >= max_results {
            break;
        }
        if let Some(r) = parse_row(&row, source) {
            out.push(r);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Extrait une ligne de resultat — `None` si la ligne est
/// incomplete ou hostile (magnet/infohash invalide, cellules
/// manquantes). Jamais de panic sur du HTML tiers.
fn parse_row(row: &scraper::ElementRef, source: &str) -> Option<ExternalResult> {
    let name_el = row.select(sel_name()).next()?;
    let name = clean_text(&safe_text(name_el));
    if name.is_empty() {
        return None;
    }

    // Le magnet est la seule source fiable de l'infohash —
    // `MagnetLink::parse` valide le btih (hex40/base32).
    let magnet = row.select(sel_magnet()).next()?.attr("href")?.to_string();
    let link = MagnetLink::parse(&magnet).ok()?;
    let infohash = link.info_hash_v1.map(hex::encode)?;

    let category = row
        .select(sel_category())
        .next()
        .map(|td| clean_text(&safe_text(td)))
        .unwrap_or_default();

    let size_bytes = row
        .select(sel_desc())
        .next()
        .map(|el| parse_size(&safe_text(el)))
        .unwrap_or(0);

    // Les deux dernieres cellules numeriques alignees a droite sont
    // SE puis LE (ordre documente du markup TPB).
    let counts: Vec<u32> = row
        .select(sel_seedleech())
        .filter_map(|td| safe_text(td).trim().replace(',', "").parse().ok())
        .collect();
    let (seeds, leechers) = match counts.as_slice() {
        [.., s, l] => (*s, *l),
        _ => return None,
    };

    Some(ExternalResult {
        name,
        infohash,
        magnet,
        size_bytes,
        seeds,
        leechers,
        category,
        source: source.to_string(),
    })
}

/// Texte visible d'un element — les sous-arbres `script`/`style`/
/// `noscript` sont ignores (un titre qui embarque du JS ne doit pas
/// remonter son contenu dans l'UI).
fn safe_text(el: scraper::ElementRef) -> String {
    let mut out = String::new();
    for node in el.descendants() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        let hidden = node.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|e| matches!(e.name(), "script" | "style" | "noscript"))
        });
        if !hidden {
            out.push_str(&text.text);
        }
    }
    out
}

/// Nettoie un champ texte tiers : controle retires, whitespace
/// pliee, borne appliquee.
fn clean_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect()
}

/// « Size 1.37 GiB » → octets. Unites reconnues : B, KiB, MiB, GiB,
/// TiB (TPB annonce en binaire). `0` si absent/indecodable.
fn parse_size(desc: &str) -> u64 {
    let words: Vec<&str> = desc.split_whitespace().collect();
    let Some(pos) = words.iter().position(|w| w.eq_ignore_ascii_case("size")) else {
        return 0;
    };
    let Some(num) = words.get(pos + 1).and_then(|w| {
        w.trim_matches(|c: char| c.is_ascii_punctuation())
            .replace(',', ".")
            .parse::<f64>()
            .ok()
    }) else {
        return 0;
    };
    let mult = match words.get(pos + 2).map(|w| {
        w.trim_matches(|c: char| c.is_ascii_punctuation())
            .to_lowercase()
    }) {
        Some(u) if u == "b" => 1u64,
        Some(u) if u == "kib" || u == "kb" => 1 << 10,
        Some(u) if u == "mib" || u == "mb" => 1 << 20,
        Some(u) if u == "gib" || u == "gb" => 1 << 30,
        Some(u) if u == "tib" || u == "tb" => 1 << 40,
        _ => return 0,
    };
    (num * mult as f64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::OnionResponse;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SEARCH_HTML: &str = include_str!("../../tests/fixtures/tpb_search.html");
    const CHALLENGE_HTML: &str = include_str!("../../tests/fixtures/tpb_challenge.html");
    const RESTRUCTURED_HTML: &str = include_str!("../../tests/fixtures/tpb_restructured.html");
    const NORESULTS_HTML: &str = include_str!("../../tests/fixtures/tpb_noresults.html");

    /// Fetcher programmable : consomme `queue` une entree par
    /// appel, enregistre les URL demandees.
    struct StubFetch {
        queue: Mutex<VecDeque<std::result::Result<OnionResponse, TorError>>>,
        urls: Arc<Mutex<Vec<String>>>,
    }

    impl StubFetch {
        fn new(queue: Vec<std::result::Result<OnionResponse, TorError>>) -> Arc<Self> {
            Arc::new(Self {
                queue: Mutex::new(queue.into()),
                urls: Arc::new(Mutex::new(Vec::new())),
            })
        }
    }

    #[async_trait::async_trait]
    impl OnionFetch for StubFetch {
        async fn fetch(&self, url: &str) -> crate::error::Result<OnionResponse> {
            self.urls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(url.to_string());
            self.queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or(Err(TorError::Request("queue vide".into())))
        }
    }

    fn page(body: &str) -> OnionResponse {
        OnionResponse {
            status: 200,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn backend(stub: Arc<StubFetch>, mirrors: &[&str], max_results: usize) -> TpbBackend {
        TpbBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: mirrors.iter().map(|m| m.to_string()).collect(),
                url_template: "http://{host}/search.php?q={q}".into(),
                max_results,
            },
            stub,
        )
    }

    #[tokio::test]
    async fn fixture_nominale_parsee() {
        let stub = StubFetch::new(vec![Ok(page(SEARCH_HTML))]);
        let b = backend(stub.clone(), &["m1.onion"], 100);
        let out = b.search("ubuntu").await.unwrap();
        assert_eq!(out.len(), 4);
        let r = &out[0];
        assert_eq!(r.name, "Ubuntu 24.04 Desktop amd64");
        assert_eq!(r.infohash, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(r.seeds, 1523);
        assert_eq!(r.leechers, 412);
        assert_eq!(r.size_bytes, 4815 * 1024 * 1024);
        assert_eq!(r.category, "Applications > UNIX");
        assert_eq!(r.source, "tpb");
        assert!(r.magnet.starts_with("magnet:?xt=urn:btih:"));
        // La requete est percent-encodee dans l'URL construite.
        assert_eq!(
            stub.urls.lock().unwrap_or_else(|e| e.into_inner())[0],
            "http://m1.onion/search.php?q=ubuntu"
        );
    }

    /// Entrees pourries : infohash tronque, magnet malforme,
    /// `<script>` dans un titre, ligne incomplete — toutes skippees
    /// sans panic, les lignes valides survivent.
    #[tokio::test]
    async fn lignes_poisonnees_skippees() {
        let stub = StubFetch::new(vec![Ok(page(SEARCH_HTML))]);
        let b = backend(stub, &["m1.onion"], 100);
        let out = b.search("x").await.unwrap();
        // La fixture contient 4 valides + 4 pourries.
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|r| r.infohash.len() == 40));
        // Le titre avec <script> embarque est nettoye : le marqueur
        // d'injection n'apparait jamais dans le nom extrait.
        assert!(!out
            .iter()
            .any(|r| r.name.contains('<') || r.name.contains("alert")));
        // Le script est supprime mais le reste du titre survit.
        assert!(out.iter().any(|r| r.name == "SpookyFilm 2024"));
    }

    #[tokio::test]
    async fn page_antiddos_captcha() {
        let stub = StubFetch::new(vec![Ok(page(CHALLENGE_HTML))]);
        let b = backend(stub, &["m1.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::CaptchaDetected)
        ));
    }

    #[tokio::test]
    async fn statut_403_avec_defi_captcha() {
        let resp = OnionResponse {
            status: 403,
            headers: Vec::new(),
            body: b"<html>Checking your browser</html>".to_vec(),
        };
        let stub = StubFetch::new(vec![Ok(resp)]);
        let b = backend(stub, &["m1.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::CaptchaDetected)
        ));
    }

    #[tokio::test]
    async fn refonte_structure_changed() {
        let stub = StubFetch::new(vec![Ok(page(RESTRUCTURED_HTML))]);
        let b = backend(stub, &["m1.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::StructureChanged)
        ));
    }

    #[tokio::test]
    async fn page_vide_no_results() {
        let stub = StubFetch::new(vec![Ok(page(NORESULTS_HTML))]);
        let b = backend(stub, &["m1.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::NoResults)
        ));
    }

    /// Bascule : premier miroir en erreur transport, le second sert
    /// la fixture.
    #[tokio::test]
    async fn bascule_miroir_sur_echec() {
        let stub = StubFetch::new(vec![
            Err(TorError::Request("connexion refusee".into())),
            Ok(page(SEARCH_HTML)),
        ]);
        let b = backend(stub.clone(), &["m1.onion", "m2.onion"], 100);
        let out = b.search("x").await.unwrap();
        assert_eq!(out.len(), 4);
        let urls = stub.urls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("http://m1.onion/"));
        assert!(urls[1].starts_with("http://m2.onion/"));
    }

    /// Tous les miroirs en echec transport → `MirrorUnreachable`.
    #[tokio::test]
    async fn tous_miroirs_injoignables() {
        let stub = StubFetch::new(vec![
            Err(TorError::Timeout),
            Err(TorError::Request("reset".into())),
        ]);
        let b = backend(stub, &["m1.onion", "m2.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::MirrorUnreachable { tried: 2 })
        ));
    }

    /// Erreurs fatales : pas de bascule, propagation immediate.
    #[tokio::test]
    async fn erreurs_fatales_sans_bascule() {
        for (tor_err, want) in [
            (TorError::Disabled, ExternalSearchError::TorDisabled),
            (TorError::NotReady, ExternalSearchError::TorNotReady),
            (TorError::NotBuilt, ExternalSearchError::TorNotReady),
        ] {
            let stub = StubFetch::new(vec![Err(tor_err)]);
            let urls = stub.urls.clone();
            let b = backend(stub, &["m1.onion", "m2.onion"], 100);
            match (b.search("x").await.unwrap_err(), &want) {
                (ExternalSearchError::TorDisabled, ExternalSearchError::TorDisabled) => {}
                (ExternalSearchError::TorNotReady, ExternalSearchError::TorNotReady) => {}
                (got, want) => panic!("{got:?} au lieu de {want:?}"),
            }
            // Un seul essai : la cause est transversale.
            assert_eq!(urls.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
        }
    }

    #[tokio::test]
    async fn borne_max_results_respectee() {
        let stub = StubFetch::new(vec![Ok(page(SEARCH_HTML))]);
        let b = backend(stub, &["m1.onion"], 2);
        assert_eq!(b.search("x").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn requete_vide_rejetee() {
        let stub = StubFetch::new(vec![]);
        let urls = stub.urls.clone();
        let b = backend(stub, &["m1.onion"], 100);
        assert!(matches!(
            b.search("   \0\n").await,
            Err(ExternalSearchError::EmptyQuery)
        ));
        assert!(urls.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
    }

    #[tokio::test]
    async fn requete_encodee_dans_template() {
        let stub = StubFetch::new(vec![Ok(page(SEARCH_HTML))]);
        let b = backend(stub.clone(), &["m1.onion"], 100);
        b.search("deja vu & co").await.unwrap();
        assert_eq!(
            stub.urls.lock().unwrap_or_else(|e| e.into_inner())[0],
            "http://m1.onion/search.php?q=deja+vu+%26+co"
        );
    }
}
