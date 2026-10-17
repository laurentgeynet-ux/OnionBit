// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backend `qphp` — decodeur de l'API JSON `q.php?q=…` de The
//! Pirate Bay, proxifiee par l'hote `.onion` TPB lui-meme (son
//! propre frontend appelle `Get(server + '/q.php…')`, verifie
//! 2026-10) — transport 100 % onion, aucune sortie clearnet :
//! la variante `apibay` via un exit Tor a ete retiree (anonymat
//! degrade par construction — le noeud de sortie voit la requete).
//!
//! Le site TPB ne sert plus de markup statique (coquille JS) :
//! `q.php` est le service que son propre frontend interroge —
//! source plus stable et non ambigue (pas de scraping). Format de
//! chaque ligne : `{"id","name","info_hash","seeders","leechers",
//! "size","category",…}` — chaines ou nombres toleres par champ.
//!
//! Cas speciaux connus : `id:"0"` / `info_hash` nul = ligne
//! sentinelle « No results returned » → `NoResults` ; un corps non
//! JSON (page de defi, refonte) → `Captcha`/`StructureChanged`.
//! L'info-hash n'est retenu que s'il survit a `MagnetLink::parse` ;
//! le magnet remonte est synthetise (`btih` + `dn`) — l'API ne
//! publie pas d'URI magnet.

use std::sync::Arc;

use onionbit_format::magnet::MagnetLink;

use crate::error::{ExternalSearchError, TorError};
use crate::http::OnionResponse;
use crate::settings::OnionBackendSettings;

use super::{
    clean_query, clean_text, has_marker, ExternalResult, OnionFetch, OnionSearchBackend,
    CHALLENGE_MARKERS,
};

/// Backend de recherche `qphp` : failover sur `mirrors` puis
/// decode JSON borne.
pub struct QphpBackend {
    /// Identifiant de source (= cle de config, ex. `"tpb"`).
    source: String,
    /// Miroirs + template + borne — deja valides au chargement.
    cfg: OnionBackendSettings,
    /// Transport injecte (`TorRuntime` en production).
    fetcher: Arc<dyn OnionFetch>,
}

/// Issue d'un essai sur un miroir — interne, pliee dans la
/// classification finale (meme taxonomie que `tpb`).
enum Outcome {
    /// Reponse exploitable.
    Results(Vec<ExternalResult>),
    /// Page de defi anti-DDoS.
    Captcha,
    /// Reponse 200 non JSON ou sans ligne decodable — refonte.
    StructureChanged,
    /// L'API repond « aucun resultat » (sentinelle `id:"0"` ou
    /// tableau vide).
    NoResults,
    /// Echec transport ou statut inattendu — miroir injoignable.
    Unreachable,
}

/// Issue du decodage d'un corps 200 — separee pour tester le
/// parseur independamment de la classification transport.
enum Parsed {
    /// Au moins une ligne exploitable.
    Results(Vec<ExternalResult>),
    /// Sentinelle « no results » ou tableau vide.
    Empty,
    /// Corps non JSON ou tableau sans aucune ligne decodable.
    Invalid,
}

impl QphpBackend {
    pub fn new(source: &str, cfg: &OnionBackendSettings, fetcher: Arc<dyn OnionFetch>) -> Self {
        Self {
            source: source.to_string(),
            cfg: cfg.clone(),
            fetcher,
        }
    }

    /// URL de requete sur `mirror` : `{host}`/`{q}` substitues,
    /// requete percent-encodee.
    fn search_url(&self, mirror: &str, query: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        self.cfg
            .url_template
            .replace("{host}", mirror)
            .replace("{q}", &encoded)
    }

    /// Classe la reponse d'un miroir — la page de defi est detectee
    /// par statut (403/503) **et** marqueurs de corps : l'API est
    /// servie derriere un frontal qui peut repondre du HTML.
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
            Parsed::Results(results) => Outcome::Results(results),
            Parsed::Empty => Outcome::NoResults,
            Parsed::Invalid => Outcome::StructureChanged,
        }
    }
}

#[async_trait::async_trait]
impl OnionSearchBackend for QphpBackend {
    async fn search(
        &self,
        query: &str,
    ) -> std::result::Result<Vec<ExternalResult>, ExternalSearchError> {
        // Nettoyage : trim + caracteres de controle retires, borne.
        let Some(clean) = clean_query(query) else {
            return Err(ExternalSearchError::EmptyQuery);
        };

        let mut tried = 0usize;
        let mut saw_captcha = false;
        let mut saw_structure = false;
        let mut saw_noresults = false;
        // `1 + retries` tentatives par miroir : un rendez-vous onion
        // rate souvent de facon transitoire (intro/garde occupee,
        // descripteur expire) — un service stable parait injoignable
        // sur un seul essai. Seuls les echecs de transport et les
        // statuts injoignables retentent ; les reponses classees
        // (defi, refonte, vide) sont definitives pour ce miroir.
        let attempts = 1 + self.cfg.retries;
        for mirror in &self.cfg.mirrors {
            let url = self.search_url(mirror, &clean);
            for attempt in 0..attempts {
                if attempt > 0 && !self.cfg.retry_delay.is_zero() {
                    tokio::time::sleep(self.cfg.retry_delay).await;
                }
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
                            Outcome::Captcha => {
                                saw_captcha = true;
                                break;
                            }
                            Outcome::StructureChanged => {
                                saw_structure = true;
                                break;
                            }
                            Outcome::NoResults => {
                                saw_noresults = true;
                                break;
                            }
                            // Statut injoignable : prochaine tentative.
                            Outcome::Unreachable => {}
                        }
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

/// Decode le corps JSON `q.php` — un tableau d'objets aux champs
/// tolerants (chaine *ou* nombre). Jamais de panic sur du contenu
/// tiers.
fn parse_results(body: &str, source: &str, max_results: usize) -> Parsed {
    let Ok(serde_json::Value::Array(rows)) = serde_json::from_str(body) else {
        return Parsed::Invalid;
    };
    if rows.is_empty() {
        return Parsed::Empty;
    }
    let mut out = Vec::new();
    let mut sentinels = 0usize;
    for row in &rows {
        if out.len() >= max_results {
            break;
        }
        if is_sentinel(row) {
            sentinels += 1;
            continue;
        }
        if let Some(r) = parse_row(row, source) {
            out.push(r);
        }
    }
    if !out.is_empty() {
        Parsed::Results(out)
    } else if sentinels > 0 {
        Parsed::Empty
    } else {
        Parsed::Invalid
    }
}

/// `true` si la ligne est la sentinelle « No results returned »
/// de l'API (`id:"0"` et/ou `info_hash` integral nul).
fn is_sentinel(row: &serde_json::Value) -> bool {
    let id = field_str(row, "id");
    let hash = field_str(row, "info_hash");
    id == "0" || (!hash.is_empty() && hash.chars().all(|c| c == '0'))
}

/// Champ texte tolerant : chaine prise telle quelle, nombre
/// stringifie, tout le reste vide.
fn field_str<'v>(row: &'v serde_json::Value, key: &str) -> std::borrow::Cow<'v, str> {
    match row.get(key) {
        Some(serde_json::Value::String(s)) => std::borrow::Cow::Borrowed(s.as_str()),
        Some(serde_json::Value::Number(n)) => std::borrow::Cow::Owned(n.to_string()),
        _ => std::borrow::Cow::Borrowed(""),
    }
}

/// Champ numerique tolerant — `0` si absent ou indecodable.
fn field_u64(row: &serde_json::Value, key: &str) -> u64 {
    field_str(row, key).trim().parse().unwrap_or(0)
}

/// Deschappe les entites HTML que l'API laisse dans les titres
/// (`&amp;`, `&#39;`, `&#x27;`…). Bornee : entites nommees
/// usuelles + numeriques decimales/hexadecimales ≤ 10 chars ; une
/// sequence non reconnue reste litterale.
fn unescape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let Some(end) = after.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|num| {
                    num.strip_prefix('x')
                        .or_else(|| num.strip_prefix('X'))
                        .map_or_else(
                            || num.parse::<u32>().ok(),
                            |h| u32::from_str_radix(h, 16).ok(),
                        )
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Extrait une ligne JSON — `None` si l'info-hash ne survit pas a
/// `MagnetLink::parse` (btih hex40/base32) ou si le nom est vide.
fn parse_row(row: &serde_json::Value, source: &str) -> Option<ExternalResult> {
    let name = clean_text(&unescape_html(&field_str(row, "name")));
    if name.is_empty() {
        return None;
    }
    let hash = field_str(row, "info_hash");
    // Le magnet synthetise est valide par le parseur canonique —
    // meme filiere de confiance que le scraping (btih normalise).
    let magnet = format!(
        "magnet:?xt=urn:btih:{}&dn={}",
        hash.to_lowercase(),
        url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
    );
    let link = MagnetLink::parse(&magnet).ok()?;
    let infohash = link.info_hash_v1.map(hex::encode)?;

    Some(ExternalResult {
        name,
        infohash,
        magnet,
        size_bytes: field_u64(row, "size"),
        seeds: u32::try_from(field_u64(row, "seeders")).unwrap_or(u32::MAX),
        leechers: u32::try_from(field_u64(row, "leechers")).unwrap_or(u32::MAX),
        category: category_label(&field_str(row, "category")),
        source: source.to_string(),
    })
}

/// Categorie numerique TPB → libelle de la famille (premier
/// chiffre : 1xx Audio, 2xx Video, …). Vide si inconnue.
fn category_label(cat: &str) -> String {
    match cat.trim().chars().next() {
        Some('1') => "Audio",
        Some('2') => "Video",
        Some('3') => "Applications",
        Some('4') => "Games",
        Some('5') => "Porn",
        Some('6') => "Other",
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

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

    fn page(status: u16, body: &str) -> OnionResponse {
        OnionResponse {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn backend(stub: Arc<StubFetch>, mirrors: &[&str], max_results: usize) -> QphpBackend {
        QphpBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: mirrors.iter().map(|m| m.to_string()).collect(),
                url_template: "http://{host}/q.php?q={q}".into(),
                max_results,
                retries: 0,
                retry_delay: std::time::Duration::ZERO,
            },
            stub,
        )
    }

    const ROW_A: &str = r#"{"id":"59191690","name":"Ubuntu 22.04 LTS","info_hash":"2C6B6858D61DA9543D4231A71DB4B1C9264B0685","leechers":"12","seeders":"40","size":"3654957056","num_files":"1","username":"u","added":"1652877231","status":"vip","category":"303","imdb":""}"#;
    const ROW_B: &str = r#"{"id":"1","name":"Test  <b>titre</b>","info_hash":"ba77b012352d816702c1baab26f6b39660fc9b39","leechers":3,"seeders":13,"size":"298682283","category":"208"}"#;

    #[tokio::test]
    async fn reponse_json_normalisee() {
        let stub = StubFetch::new(vec![Ok(page(200, &format!("[{ROW_A},{ROW_B}]")))]);
        let b = backend(stub.clone(), &["x-mirror.onion"], 100);
        let out = b.search("ubuntu lts").await.unwrap();
        assert_eq!(out.len(), 2);
        let r = &out[0];
        assert_eq!(r.name, "Ubuntu 22.04 LTS");
        // Info-hash normalise en minuscules quelle que soit la casse recue.
        assert_eq!(r.infohash, "2c6b6858d61da9543d4231a71db4b1c9264b0685");
        assert_eq!(r.seeds, 40);
        assert_eq!(r.leechers, 12);
        assert_eq!(r.size_bytes, 3654957056);
        assert_eq!(r.category, "Applications");
        assert_eq!(r.source, "tpb");
        assert!(r.magnet.starts_with("magnet:?xt=urn:btih:2c6b6858"));
        assert!(r.magnet.contains("dn=Ubuntu+22.04+LTS"));
        // Les champs numeriques JSON (non chaines) sont toleres.
        assert_eq!(out[1].seeds, 13);
        assert_eq!(out[1].category, "Video");
        assert_eq!(
            stub.urls.lock().unwrap_or_else(|e| e.into_inner())[0],
            "http://x-mirror.onion/q.php?q=ubuntu+lts"
        );
    }

    /// Les titres q.php arrivent HTML-escapes — les entites sont
    /// decodees avant normalisation (nom ET `dn` du magnet).
    #[tokio::test]
    async fn entites_html_desescapees() {
        let row = r#"{"id":"1","name":"A &amp; B &#39;C&#39; &lt;ok&gt; &#xE9;","info_hash":"2c6b6858d61da9543d4231a71db4b1c9264b0685","seeders":"1","leechers":"0","size":"1","category":"208"}"#;
        let stub = StubFetch::new(vec![Ok(page(200, &format!("[{row}]")))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        let out = b.search("x").await.unwrap();
        assert_eq!(out[0].name, "A & B 'C' <ok> é");
        assert!(!out[0].magnet.contains("amp"));
        // Sequence inconnue -> litterale conservee.
        assert_eq!(unescape_html("x &yolo; y &unfini"), "x &yolo; y &unfini");
    }

    /// Lignes pourries : info-hash non-hex40, nom vide, ligne non
    /// objet — skippees, les valides survivent.
    #[tokio::test]
    async fn lignes_poisonnees_skippees() {
        let body = format!(
            r#"[{{"name":"bad","info_hash":"ZZZZ"}},{{"name":"","info_hash":"{0}"}},42,{ROW_A}]"#,
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let stub = StubFetch::new(vec![Ok(page(200, &body))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        let out = b.search("x").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].infohash.len(), 40);
    }

    #[tokio::test]
    async fn sentinelle_no_results() {
        let stub = StubFetch::new(vec![Ok(page(
            200,
            r#"[{"id":"0","name":"No results returned","info_hash":"0000000000000000000000000000000000000000","leechers":"0","seeders":"0","size":"0","category":"0"}]"#,
        ))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::NoResults)
        ));
    }

    #[tokio::test]
    async fn tableau_vide_no_results() {
        let stub = StubFetch::new(vec![Ok(page(200, "[]"))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::NoResults)
        ));
    }

    #[tokio::test]
    async fn corps_non_json_structure_changed() {
        let stub = StubFetch::new(vec![Ok(page(
            200,
            "<html><body>Enable JS in your browser!</body></html>",
        ))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::StructureChanged)
        ));
    }

    #[tokio::test]
    async fn page_antiddos_captcha() {
        let stub = StubFetch::new(vec![Ok(page(
            200,
            "<html>Checking your browser… ddos-guard</html>",
        ))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::CaptchaDetected)
        ));
    }

    #[tokio::test]
    async fn statut_503_captcha() {
        let stub = StubFetch::new(vec![Ok(page(503, "Service Unavailable"))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::CaptchaDetected)
        ));
    }

    /// Statut inattendu hors defi → miroir injoignable → bascule.
    #[tokio::test]
    async fn statut_404_unreachable() {
        let stub = StubFetch::new(vec![Ok(page(404, "nope"))]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::MirrorUnreachable { tried: 1 })
        ));
    }

    #[tokio::test]
    async fn borne_max_results() {
        let body = format!("[{ROW_A},{ROW_A},{ROW_B},{ROW_A}]");
        let stub = StubFetch::new(vec![Ok(page(200, &body))]);
        let b = backend(stub, &["x-mirror.onion"], 2);
        let out = b.search("x").await.unwrap();
        assert_eq!(out.len(), 2);
    }

    /// Retry transitoire : premier essai en erreur transport, la
    /// retentive (meme miroir) sert la reponse — `retries: 1`.
    #[tokio::test]
    async fn retry_transitoire_puis_succes() {
        let stub = StubFetch::new(vec![
            Err(TorError::Request("circuit rompu".into())),
            Ok(page(200, &format!("[{ROW_A}]"))),
        ]);
        let b = QphpBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: vec!["m1.org".into()],
                url_template: "http://{host}/q.php?q={q}".into(),
                max_results: 10,
                retries: 1,
                retry_delay: std::time::Duration::ZERO,
            },
            stub.clone(),
        );
        let out = b.search("x").await.unwrap();
        assert_eq!(out.len(), 1);
        let urls = stub.urls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(urls.len(), 2);
        assert!(urls.iter().all(|u| u.starts_with("http://m1.org/")));
    }

    /// Les reponses classees ne retentent pas : un 404 est
    /// injoignable → retry ; une page de defi est definitive.
    #[tokio::test]
    async fn retry_statut_injoignable_pas_defi() {
        // 404 transitoire → retentive → succes.
        let stub = StubFetch::new(vec![Ok(page(404, "nope")), Ok(page(200, "[]"))]);
        let b = QphpBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: vec!["m1.org".into()],
                url_template: "http://{host}/q.php?q={q}".into(),
                max_results: 10,
                retries: 1,
                retry_delay: std::time::Duration::ZERO,
            },
            stub.clone(),
        );
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::NoResults)
        ));
        assert_eq!(stub.urls.lock().unwrap_or_else(|e| e.into_inner()).len(), 2);
        // Defi → classifie, pas de retentive (queue a une entree
        // de rab qui ne doit pas etre consommee).
        let stub = StubFetch::new(vec![Ok(page(503, "cf")), Ok(page(200, "[]"))]);
        let b = QphpBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: vec!["m1.org".into()],
                url_template: "http://{host}/q.php?q={q}".into(),
                max_results: 10,
                retries: 1,
                retry_delay: std::time::Duration::ZERO,
            },
            stub.clone(),
        );
        assert!(matches!(
            b.search("x").await,
            Err(ExternalSearchError::CaptchaDetected)
        ));
        assert_eq!(stub.urls.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
    }

    /// Bascule : premier miroir en erreur transport, le second sert
    /// la reponse JSON.
    #[tokio::test]
    async fn bascule_miroir_sur_echec() {
        let stub = StubFetch::new(vec![
            Err(TorError::Request("connexion refusee".into())),
            Ok(page(200, &format!("[{ROW_A}]"))),
        ]);
        let b = backend(stub.clone(), &["m1.org", "m2.org"], 100);
        let out = b.search("x").await.unwrap();
        assert_eq!(out.len(), 1);
        let urls = stub.urls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("http://m1.org/"));
        assert!(urls[1].starts_with("http://m2.org/"));
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
            let b = backend(stub, &["m1.org", "m2.org"], 100);
            match (b.search("x").await.unwrap_err(), &want) {
                (ExternalSearchError::TorDisabled, ExternalSearchError::TorDisabled) => {}
                (ExternalSearchError::TorNotReady, ExternalSearchError::TorNotReady) => {}
                (got, want) => panic!("{got:?} au lieu de {want:?}"),
            }
            assert_eq!(urls.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
        }
    }

    /// Fixture reellement servie par l'onion TPB
    /// (`q.php?q=french`, capturee 2026-10 via `onion_fetch_probe`)
    /// — le meme decodeur sert le backend `tpb` (100 % onion).
    #[tokio::test]
    async fn reponse_onion_tpb_reelle_decodee() {
        const BODY: &str = include_str!("../../tests/fixtures/qphp_tpb_onion.json");
        let stub = StubFetch::new(vec![Ok(page(200, BODY))]);
        let b = QphpBackend::new(
            "tpb",
            &OnionBackendSettings {
                mirrors: vec!["un-onion-v3.onion".into()],
                url_template: "http://{host}/q.php?q={q}".into(),
                max_results: 100,
                retries: 0,
                retry_delay: std::time::Duration::ZERO,
            },
            stub,
        );
        let out = b.search("french").await.unwrap();
        assert_eq!(out.len(), 10);
        assert!(out
            .iter()
            .all(|r| r.infohash.len() == 40 && r.source == "tpb" && !r.name.is_empty()));
        assert_eq!(
            out[0].name,
            "Anatomy Of A Fall (2023) FRENCH.ENSUB 720p WEBRip-WORLD"
        );
        assert_eq!(out[0].category, "Video");
    }

    #[tokio::test]
    async fn requete_vide_refusee() {
        let stub = StubFetch::new(vec![]);
        let b = backend(stub, &["x-mirror.onion"], 100);
        assert!(matches!(
            b.search("   \u{0000}  ").await,
            Err(ExternalSearchError::EmptyQuery)
        ));
    }
}
