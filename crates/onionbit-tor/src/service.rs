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

/// Nickname du service `.onion` **coffre** (etape 127) — service
/// separe de l'identite : sa propre cle derivee
/// (`tor-hs-vault/v1`), sa propre adresse, son autorisation client.
pub(crate) const HS_VAULT_NICKNAME: &str = "onionbit-vault";

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

/// Keystore primaire arti (`state/keystore`) reconstruit — le
/// `keymgr()` du client n'est public que derriere
/// `onion-service-cli-extra` (`__is_experimental`) ; le keystore
/// est une collection de fichiers, services et clients les
/// relisent a la demande.
fn own_keymgr(inner: &Arc<Inner>) -> Result<tor_keymgr::KeyMgr> {
    let keystore_dir = inner.settings.state_dir.join("state").join("keystore");
    let store = tor_keymgr::ArtiNativeKeystore::from_path_and_mistrust(
        &keystore_dir,
        &fs_mistrust::Mistrust::new(),
    )
    .map_err(|e| TorError::OnionService(format!("keystore : {e}")))?;
    tor_keymgr::KeyMgrBuilder::default()
        .primary_store(Box::new(store))
        .build()
        .map_err(|e| TorError::OnionService(format!("keymgr : {e}")))
}

/// Insere la cle d'autorisation client `KS_hsc_desc_enc` (etape
/// 127) pour `hs_id` dans le keystore primaire — lue par le
/// connecteur `hsclient` a chaque connexion (le client dechiffre
/// alors le descripteur du service en restricted-discovery).
pub(crate) fn install_client_desc_key(
    inner: &Arc<Inner>,
    hs_id: tor_hscrypto::pk::HsId,
    secret: tor_hscrypto::pk::HsClientDescEncSecretKey,
) -> Result<()> {
    let keypair = tor_hscrypto::pk::HsClientDescEncKeypair::new(
        tor_hscrypto::pk::HsClientDescEncKey::from(&secret),
        secret,
    );
    let spec = tor_hsclient::HsClientDescEncKeypairSpecifier::new(hs_id);
    own_keymgr(inner)?
        .insert::<tor_hscrypto::pk::HsClientDescEncKeypair>(
            keypair,
            &spec,
            tor_keymgr::KeystoreSelector::Primary,
            true,
        )
        .map_err(|e| TorError::OnionService(format!("insertion cle client : {e}")))?;
    Ok(())
}

/// Dispatch generique d'un flux entrant : port `BEGIN` → handler
/// (la table `lookup` vient du plan — `inner.link_handlers` pour
/// l'identite, table figee pour le coffre). Port non souscrit =
/// `DONE` standard.
type HandlerLookup = Arc<dyn Fn(u16) -> Option<Arc<dyn crate::OnionLinkHandler>> + Send + Sync>;

/// Lance le service `.onion` d'identite sur le client embedded de
/// `inner` — erreur explicite en mode externe ou client non pret.
pub(crate) fn launch(inner: &Arc<Inner>, hs_seed: [u8; 32]) -> Result<()> {
    let inner2 = inner.clone();
    let slot = launch_service(
        inner,
        hs_seed,
        HS_NICKNAME,
        None,
        Arc::new(move |port| {
            inner2
                .link_handlers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&port)
                .cloned()
        }),
    )?;
    *inner.onion.lock().unwrap_or_else(|e| e.into_inner()) = Some(slot);
    Ok(())
}

/// Lance le service `.onion` **coffre** (etape 127) — descripteur
/// chiffre par `restricted_discovery` (seul le detenteur de la cle
/// client x25519 derivee de l'identite peut le decouvrir ; tout
/// autre visiteur ne voit qu'un service injoignable). `handler` sert
/// les streams entrants — le coffre n'expose qu'un port.
pub(crate) fn launch_vault(
    inner: &Arc<Inner>,
    hs_seed: [u8; 32],
    client_key: tor_hscrypto::pk::HsClientDescEncKey,
    vault_port: u16,
    handler: Arc<dyn crate::OnionLinkHandler>,
) -> Result<()> {
    use std::str::FromStr;
    let mut builder = OnionServiceConfigBuilder::default();
    builder.restricted_discovery().enabled(true);
    builder.restricted_discovery().static_keys().access().push((
        tor_hsservice::config::restricted_discovery::HsClientNickname::from_str("owner")
            .map_err(|e| TorError::OnionService(format!("nickname client : {e}")))?,
        client_key,
    ));
    let slot = launch_service(
        inner,
        hs_seed,
        HS_VAULT_NICKNAME,
        Some(builder),
        Arc::new(move |port| (port == vault_port).then(|| handler.clone())),
    )?;
    *inner.onion_vault.lock().unwrap_or_else(|e| e.into_inner()) = Some(slot);
    Ok(())
}

/// Corps commun du lancement : insertion de la `HsIdKeypair`
/// derivee (autoritaire — l'adresse est stable entre runs),
/// config service, rendez-vous entrants routes par port vers
/// `lookup`. `rd_builder` surcharge la config par defaut
/// (restricted-discovery du coffre).
fn launch_service(
    inner: &Arc<Inner>,
    hs_seed: [u8; 32],
    nick: &str,
    rd_builder: Option<OnionServiceConfigBuilder>,
    lookup: HandlerLookup,
) -> Result<OnionServiceSlot> {
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
    // une divergence. Le keystore est reconstruit sur le meme
    // repertoire (`own_keymgr`) : les fichiers sont relus par le
    // service a son demarrage.
    let keypair = tor_llcrypto::pk::ed25519::Keypair::from(&hs_seed);
    let hsid = tor_hscrypto::pk::HsIdKeypair::from(
        tor_llcrypto::pk::ed25519::ExpandedKeypair::from(&keypair),
    );
    let nickname = HsNickname::new(nick.to_string())
        .map_err(|e| TorError::OnionService(format!("nickname invalide : {e}")))?;
    let spec = HsIdKeypairSpecifier::new(nickname.clone());
    own_keymgr(inner)?
        .insert::<tor_hscrypto::pk::HsIdKeypair>(
            hsid,
            &spec,
            tor_keymgr::KeystoreSelector::Primary,
            true,
        )
        .map_err(|e| TorError::OnionService(format!("insertion cle hsid : {e}")))?;

    let config = rd_builder
        .unwrap_or_default()
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
    let drain_lookup = lookup.clone();
    let drain = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(rend) = rend.next().await {
            let lookup = drain_lookup.clone();
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
                    let handler = lookup(port);
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

    Ok(OnionServiceSlot {
        address,
        state,
        _service: service,
        _drain: drain,
        _watch: watch,
    })
}
