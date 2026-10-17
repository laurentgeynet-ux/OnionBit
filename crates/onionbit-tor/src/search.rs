// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Recherche externe `.onion` (ADR-0028 §5) — trait
//! [`OnionSearchBackend`], resultats normalises et registre par
//! `source`.
//!
//! L'extraction (JSON `q.php` aujourd'hui) est isolee derriere le
//! trait : chaque backend declare
//! ses miroirs en config et bascule avant de rendre erreur. Le
//! transport est injecte via [`OnionFetch`] — [`TorRuntime`] en
//! production, un double en test : tout le module est testable sans
//! la feature `tor` ni reseau.
//!
//! Invariants :
//!
//! - tout champ texte extrait est traite comme **non fiable** :
//!   assaini (controle/whitespace) et borne ;
//! - l'info-hash n'est retenu que s'il survit a
//!   `MagnetLink::parse` (btih hex40/base32) ;
//! - aucun terme de recherche ni URL complete n'est journalise.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::{ExternalSearchError, Result};
use crate::http::OnionResponse;
use crate::runtime::TorRuntime;
use crate::settings::TorSettings;

pub mod apibay;

/// Borne interne du titre extrait — defensive, pas un reglage : la
/// reponse est un contenu tiers non fiable.
pub(crate) const MAX_NAME_CHARS: usize = 300;

/// Borne interne de la requete utilisateur avant encodage.
pub(crate) const MAX_QUERY_CHARS: usize = 300;

/// Marqueurs heuristiques d'une page de defi anti-DDoS (Cloudflare,
/// DDoS-Guard, PoW natif…). Consultes en minuscules sur le corps —
/// un backend JSON peut aussi recevoir une page HTML de defi.
pub(crate) const CHALLENGE_MARKERS: &[&str] = &[
    "ddos-guard",
    "just a moment",
    "checking your browser",
    "verify you are human",
    "captcha",
    "challenge-platform",
    "pow challenge",
    "security check",
];

/// `true` si le corps (minuscules) contient un marqueur de defi.
pub(crate) fn has_marker(body: &str, markers: &[&str]) -> bool {
    let lower = body.to_lowercase();
    markers.iter().any(|m| lower.contains(m))
}

/// Requete utilisateur nettoyee et bornee — `None` si vide. Chaque
/// backend applique la meme hygiene avant encodage dans son URL.
pub(crate) fn clean_query(query: &str) -> Option<String> {
    let clean: String = query
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    (!clean.is_empty()).then_some(clean)
}

/// Nettoie un champ texte tiers : controle retires, whitespace
/// pliee, borne appliquee.
pub(crate) fn clean_text(raw: &str) -> String {
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

/// Resultat de recherche externe normalise (ADR-0028 §5).
///
/// Cote UI ce type devient un `TorrentResult` avec `source` —
/// les champs texte sont deja assainis ici, l'UI n'a pas a se
/// mefier du contenu tiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalResult {
    /// Titre du torrent (controle/whitespace nettoyes, borne).
    pub name: String,
    /// Info-hash hex normalise (`btih` v1).
    pub infohash: String,
    /// URI magnet complete telle que publiee par le site.
    pub magnet: String,
    /// Taille annoncee en octets (`0` si non extractible).
    pub size_bytes: u64,
    /// Seeders annonces.
    pub seeds: u32,
    /// Leechers annonces.
    pub leechers: u32,
    /// Categorie du site (texte libre assaini).
    pub category: String,
    /// Backend d'origine (`"tpb"`, …) = cle de config.
    pub source: String,
}

/// Acces borne au fetch onion — abstraction du transport pour
/// injecter un double en test. La politique de cibles est verifiee
/// dans le fetcher lui-meme ([`TorRuntime::fetch_onion`]), jamais
/// bypassable par un backend.
#[async_trait::async_trait]
pub trait OnionFetch: Send + Sync {
    /// `GET` borne d'une URL `http://….onion` — erreurs [`TorError`].
    async fn fetch(&self, url: &str) -> Result<OnionResponse>;
}

#[async_trait::async_trait]
impl OnionFetch for TorRuntime {
    async fn fetch(&self, url: &str) -> Result<OnionResponse> {
        self.fetch_onion(url).await
    }
}

/// Backend de recherche externe — isole la fragilite du scraping
/// derriere un contrat stable (ADR-0028 §5 : registre par `source`).
#[async_trait::async_trait]
pub trait OnionSearchBackend: Send + Sync {
    /// Recherche normalisee : bascule de miroirs interne et
    /// [`ExternalSearchError`] typee — jamais d'echec muet.
    async fn search(
        &self,
        query: &str,
    ) -> std::result::Result<Vec<ExternalResult>, ExternalSearchError>;
}

/// Construit les backends declares dans `tor.search.backends` —
/// le registre est indexe par `source` (`tpb`, …). Un id sans
/// implementation est ignore avec un warn : la config file peut
/// decrire un backend pas encore (ou plus) supporte.
pub fn build_backends(
    settings: &TorSettings,
    fetcher: Arc<dyn OnionFetch>,
) -> BTreeMap<String, Arc<dyn OnionSearchBackend>> {
    let mut out = BTreeMap::new();
    for (id, cfg) in &settings.backends {
        match id.as_str() {
            // `tpb` = l'API JSON `q.php` proxifiee par l'hote onion
            // — le site ne sert plus de HTML de resultats (coquille
            // JS, verifie 2026-10) ; le scraping `tpb.rs` est mort
            // avec elle. Meme decodeur que `apibay`, transport
            // 100 % onion (pas de sortie clearnet).
            "tpb" | "apibay" => {
                out.insert(
                    id.clone(),
                    Arc::new(apibay::ApibayBackend::new(id, cfg, fetcher.clone()))
                        as Arc<dyn OnionSearchBackend>,
                );
            }
            other => tracing::warn!(
                backend = %other,
                "tor : backend sans implementation connue — ignore"
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::OnionBackendSettings;

    /// Fetcher minimal : repond toujours le meme corps 200.
    struct StubFetch(&'static str);

    #[async_trait::async_trait]
    impl OnionFetch for StubFetch {
        async fn fetch(&self, _url: &str) -> Result<OnionResponse> {
            Ok(OnionResponse {
                status: 200,
                headers: Vec::new(),
                body: self.0.as_bytes().to_vec(),
            })
        }
    }

    fn settings_with(id: &str) -> TorSettings {
        let mut s = TorSettings::default();
        s.backends.insert(
            id.to_string(),
            OnionBackendSettings {
                mirrors: vec!["m.onion".to_string()],
                url_template: "http://{host}/q.php?q={q}".to_string(),
                max_results: 10,
                retries: 0,
                retry_delay: std::time::Duration::ZERO,
            },
        );
        s
    }

    /// `tpb` et `apibay` partagent le decodeur `q.php` : le scraping
    /// HTML TPB n'existe plus (coquille JS, refonte verifiee 2026-10).
    /// La source remontee reste l'id de config — l'UI badge `TPB`.
    #[tokio::test]
    async fn tpb_decode_json_qphp() {
        for id in ["tpb", "apibay"] {
            let backends = build_backends(
                &settings_with(id),
                Arc::new(StubFetch(
                    r#"[{"id":"1","name":"T","info_hash":"2c6b6858d61da9543d4231a71db4b1c9264b0685","seeders":"5","leechers":"1","size":"42","category":"207"}]"#,
                )),
            );
            let out = backends[id].search("x").await.unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].source, id);
            assert_eq!(out[0].infohash, "2c6b6858d61da9543d4231a71db4b1c9264b0685");
        }
    }

    /// Un id inconnu est ignore (warn) — la config file peut
    /// decrire un backend disparu.
    #[test]
    fn id_inconnu_ignore() {
        let backends = build_backends(&settings_with("zombie"), Arc::new(StubFetch("[]")));
        assert!(backends.is_empty());
    }
}
