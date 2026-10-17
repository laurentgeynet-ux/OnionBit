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

pub mod qphp;

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

    /// Miroirs `.onion` configures — exposes pour la sonde de
    /// joignabilite `GET /api/tor/probe` (l'UI affiche la sante
    /// reelle de la source : un onion deprecie/mort est visible
    /// sans avoir a lancer une recherche).
    fn mirrors(&self) -> &[String] {
        &[]
    }
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
            // JS, verifie 2026-10). Transport 100 % onion : aucun
            // backend clearnet n'est accepte (anonymat degrade par
            // construction — un exit verrait la requete).
            "tpb" => {
                out.insert(
                    id.clone(),
                    Arc::new(qphp::QphpBackend::new(id, cfg, fetcher.clone()))
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

    /// `tpb` decode le JSON `q.php` — la source remontee reste
    /// l'id de config (l'UI badge `TPB`).
    #[tokio::test]
    async fn tpb_decode_json_qphp() {
        let backends = build_backends(
            &settings_with("tpb"),
            Arc::new(StubFetch(
                r#"[{"id":"1","name":"T","info_hash":"2c6b6858d61da9543d4231a71db4b1c9264b0685","seeders":"5","leechers":"1","size":"42","category":"207"}]"#,
            )),
        );
        let out = backends["tpb"].search("x").await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "tpb");
        assert_eq!(out[0].infohash, "2c6b6858d61da9543d4231a71db4b1c9264b0685");
    }

    /// Un id inconnu — dont `apibay` (backend clearnet via exit
    /// Tor, retire : anonymat degrade) — est ignore avec un warn ;
    /// une config persistee d'avant la suppression reste inerte.
    #[test]
    fn id_inconnu_ou_clearnet_ignore() {
        for id in ["zombie", "apibay"] {
            let backends = build_backends(&settings_with(id), Arc::new(StubFetch("[]")));
            assert!(backends.is_empty(), "{id} devrait etre ignore");
        }
    }
}
