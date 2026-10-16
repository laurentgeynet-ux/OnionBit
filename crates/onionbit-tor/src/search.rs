// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Recherche externe `.onion` (ADR-0028 §5) — trait
//! [`OnionSearchBackend`], resultats normalises et registre par
//! `source`.
//!
//! Le scraping est isole derriere le trait : chaque backend declare
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

pub mod tpb;

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
            "tpb" => {
                out.insert(
                    id.clone(),
                    Arc::new(tpb::TpbBackend::new(id, cfg, fetcher.clone()))
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
