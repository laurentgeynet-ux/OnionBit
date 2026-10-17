// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet
// SPDX-License-Identifier: GPL-3.0-or-later

//! Service `.onion` d'identite (ADR-0029, etape 122) — hebergement
//! d'un service cache v3 sur la lane arti embarquee.
//!
//! - La cle `HsIdKeypair` vient de la graine d'identite via HKDF
//!   `onionbit/identity/tor-hs/v1` — **jamais** `ipv8-sign` (deux
//!   plans non liables). Elle est inseree dans le keystore primaire
//!   avant le lancement : l'identite derivee est autoritaire et
//!   l'adresse `.onion` est stable entre redemarrages.
//! - Le slot garde `RunningOnionService` + taches vivants ; son
//!   drop arrete la publication.
//! - Les rendez-vous entrants (`RendRequest`) sont acceptes circuit
//!   par circuit ; chaque `StreamRequest` est routee par son port
//!   virtuel `BEGIN` vers le handler souscrit
//!   (`Inner.link_handlers` — messagerie etape 124, catalogue 126,
//!   coffre 127). Port non souscrit = rejet `DONE` conforme au
//!   comportement d'un service Tor standard ; aucun listener TCP
//!   local n'est ouvert (zero surface loopback).

use std::sync::{Arc, Mutex};

use safelog::DisplayRedacted;
use tor_hsservice::config::OnionServiceConfigBuilder;
use tor_hsservice::{HsIdKeypairSpecifier, HsNickname, RunningOnionService};

use crate::error::{Result, TorError};
use crate::runtime::Inner;
use crate::settings::TorMode;

/// Nickname interne du service d'identite — prefixe des artefacts
/// keymgr (`hss/<nickname>/`) et du state `tor-persist`.
pub(crate) const HS_NICKNAME: &str = "onionbit-identity";

/// Etat haut niveau du service, remonte par `status_events` —
/// chaine stable exposee via `GET /api/tor/status`.
pub(crate) type ServiceState = Arc<Mutex<&'static str>>;

/// Service `.onion` heberge — garder vivant tant que la lane le
/// sert ; le drop coupe les taches et depublie le service.
pub(crate) struct OnionServiceSlot {
    /// Adresse `.onion` du service (connue des le lancement — elle
    /// est derivee de la cle, pas du reseau).
    pub address: Option<String>,
    /// Etat courant mis a jour par la tache de statut.
    pub state: ServiceState,
    _service: Arc<RunningOnionService>,
    _drain: tokio::task::JoinHandle<()>,
    _watch: tokio::task::JoinHandle<()>,
}

/// Traduit l'etat haut niveau `tor-hsservice` en libelle stable.
fn map_state(state: tor_hsservice::status::State) -> &'static str {
    use tor_hsservice::status::State;
    match state {
        State::Shutdown => "shutdown",
        State::Bootstrapping => "bootstrapping",
        State::DegradedReachable | State::DegradedUnreachable => "degraded",
        s if s.is_fully_reachable() => "running",
        _ => "bootstrapping",
    }
}

/// Lance le service `.onion` d'identite sur le client embedded de
/// `inner` — erreur explicite en mode externe ou client non pret.
pub(crate) fn launch(inner: &Arc<Inner>, hs_seed: [u8; 32]) -> Result<()> {
    if let TorMode::External(_) = inner.settings.mode {
        return Err(TorError::OnionService(
            "service .onion : mode `external` non supporte (le tor systeme heberge ses propres services via torrc)".into(),
        ));
    }
    let client = inner
        .client
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or(TorError::NotReady)?;

    // Cle de service derivee de la graine — inseree dans le keystore
    // primaire (`state/keystore`) avant le lancement : l'identite
    // derivee est autoritaire, une cle anterieure auto-generee serait
    // une divergence. `TorClient::keymgr()` n'est public que derriere
    // `onion-service-cli-extra` (__is_experimental) — on construit un
    // `KeyMgr` propre sur le meme repertoire : le keystore est une
    // collection de fichiers, le service les relira a son demarrage.
    let keypair = tor_llcrypto::pk::ed25519::Keypair::from(&hs_seed);
    let hsid = tor_hscrypto::pk::HsIdKeypair::from(
        tor_llcrypto::pk::ed25519::ExpandedKeypair::from(&keypair),
    );
    let nickname = HsNickname::new(HS_NICKNAME.to_string())
        .map_err(|e| TorError::OnionService(format!("nickname invalide : {e}")))?;
    let spec = HsIdKeypairSpecifier::new(nickname.clone());
    let keystore_dir = inner.settings.state_dir.join("state").join("keystore");
    let store = tor_keymgr::ArtiNativeKeystore::from_path_and_mistrust(
        &keystore_dir,
        &fs_mistrust::Mistrust::new(),
    )
    .map_err(|e| TorError::OnionService(format!("keystore : {e}")))?;
    let keymgr = tor_keymgr::KeyMgrBuilder::default()
        .primary_store(Box::new(store))
        .build()
        .map_err(|e| TorError::OnionService(format!("keymgr : {e}")))?;
    keymgr
        .insert::<tor_hscrypto::pk::HsIdKeypair>(
            hsid,
            &spec,
            tor_keymgr::KeystoreSelector::Primary,
            true,
        )
        .map_err(|e| TorError::OnionService(format!("insertion cle hsid : {e}")))?;

    let config = OnionServiceConfigBuilder::default()
        .nickname(nickname)
        .build()
        .map_err(|e| TorError::OnionService(format!("config service : {e}")))?;
    let (service, mut rend) = client
        .launch_onion_service(config)
        .map_err(|e| TorError::OnionService(format!("launch : {e}")))?
        .ok_or_else(|| TorError::OnionService("service desactive dans la config".into()))?;

    let address = service
        .onion_address()
        .map(|a| a.display_unredacted().to_string());
    let state: ServiceState = Arc::new(Mutex::new("bootstrapping"));

    // Rendez-vous entrants : accepte chaque circuit puis route
    // chaque stream par port virtuel `BEGIN` vers le handler
    // souscrit (ADR-0029 etape 124). Port non souscrit = `DONE` —
    // le rejet standard d'un service Tor ne distingue pas OnionBit
    // d'un service ordinaire (empreinte, req.rs §accept).
    let drain_inner = inner.clone();
    let drain = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(rend) = rend.next().await {
            let inner = drain_inner.clone();
            tokio::spawn(async move {
                let Ok(mut reqs) = rend.accept().await else {
                    return;
                };
                while let Some(sreq) = reqs.next().await {
                    let port = match sreq.request() {
                        tor_proto::stream::IncomingStreamRequest::Begin(b) => b.port(),
                        _ => {
                            let _ = sreq
                                .reject(tor_cell::relaycell::msg::End::new_with_reason(
                                    tor_cell::relaycell::msg::EndReason::DONE,
                                ))
                                .await;
                            continue;
                        }
                    };
                    let handler = inner
                        .link_handlers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&port)
                        .cloned();
                    match handler {
                        Some(h) => match sreq
                            .accept(tor_cell::relaycell::msg::Connected::new_empty())
                            .await
                        {
                            Ok(ds) => h.on_stream(Box::new(ds)),
                            Err(e) => {
                                tracing::debug!(error = %e, "stream .onion : accept impossible")
                            }
                        },
                        None => {
                            let _ = sreq
                                .reject(tor_cell::relaycell::msg::End::new_with_reason(
                                    tor_cell::relaycell::msg::EndReason::DONE,
                                ))
                                .await;
                        }
                    }
                }
            });
        }
    });

    // Suivi d'etat → expose a l'API.
    let mut events = service.status_events();
    let watch_state = state.clone();
    let watch = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(st) = events.next().await {
            *watch_state.lock().unwrap_or_else(|e| e.into_inner()) = map_state(st.state());
        }
        // Fin du flux = service arrete.
        *watch_state.lock().unwrap_or_else(|e| e.into_inner()) = "shutdown";
    });

    *inner.onion.lock().unwrap_or_else(|e| e.into_inner()) = Some(OnionServiceSlot {
        address,
        state,
        _service: service,
        _drain: drain,
        _watch: watch,
    });
    Ok(())
}
