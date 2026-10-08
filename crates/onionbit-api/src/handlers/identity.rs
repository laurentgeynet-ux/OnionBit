// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `GET/POST /api/identity/*` — identite IPv8 portable (ADR-0016).
//!
//! Export : `LibNaCLSK:` brut en hex, ou blob `OBID` (argon2id +
//! ChaCha20-Poly1305) quand un mot de passe est fourni — c'est le
//! blob qui voyage entre devices, donc c'est lui qui merite la
//! protection. Import : valide le format, ecrase
//! `ipv8_keypair.bin`, `restart_required` en phase `ready` —
//! activation immediate en `pending`/`locked` (etape 48d).
//!
//! Cycle de vie de la session (etape 48d) : `GET /api/identity`
//! expose `state = ready | locked | pending` ; `unlock` ouvre une
//! graine `OBSK` (rate-limite par IP + globalement), `create`
//! resout le gate de premier boot, `guest` demarre une session
//! ephemere sans rien persister, `at_rest` active/desactive le
//! scellement de la graine.
//!
//! Endpoints sensibles : derriere `api_key_auth` comme le reste de
//! `/api` ; jamais loggues, jamais dans les events SSE — mots de
//! passe, phrase et graines ne franchissent que la requete.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Query, State};
use axum::Json;
use onionbit_core::identity::{self, IdentityKind};
use onionbit_core::session::IdentityPhase;
use onionbit_crypto::identity::IdentitySeed;
use onionbit_crypto::ipv8::keys::LibNaClSecretKey;
use onionbit_crypto::keyblob::{keyblob_is_sealed, keyblob_open, keyblob_seal};
use onionbit_format::bip39::{self, Language};

use crate::error::ApiError;
use crate::state::AppState;

/// Corps `{password?}` de `POST /api/identity/export` — vide/absent
/// = export de la cle brute (l'utilisateur gere la protection).
#[derive(serde::Deserialize)]
pub struct IdentityExportBody {
    /// Mot de passe de protection du blob (optionnel).
    #[serde(default)]
    pub password: Option<String>,
}

/// Corps `{key, password?}` ou `{phrase}` de
/// `POST /api/identity/restore` — exactement un des deux.
#[derive(serde::Deserialize)]
pub struct IdentityRestoreBody {
    /// Cle hex — `LibNaCLSK:` brut ou blob `OBID…`.
    #[serde(default)]
    pub key: Option<String>,
    /// Phrase de recuperation BIP39 (24 mots, EN ou FR) — installe
    /// `identity_seed.bin` et regenere les cles derivees (ADR-0016).
    #[serde(default)]
    pub phrase: Option<String>,
    /// Mot de passe si le blob est chiffre `OBID`.
    #[serde(default)]
    pub password: Option<String>,
    /// Confirmation explicite de la conversion seedee → legacy :
    /// requis quand `identity_seed.bin` existe et que le restore se
    /// fait par `key` (la graine gagnerait sinon au prochain boot).
    #[serde(default)]
    pub force_legacy: bool,
}

#[derive(serde::Deserialize)]
pub struct RecoveryPhraseQuery {
    /// `en` (defaut) ou `fr` — wordlist d'encodage.
    #[serde(default)]
    pub lang: Option<String>,
}

/// Corps `{password}` de `POST /api/identity/unlock`.
#[derive(serde::Deserialize)]
pub struct UnlockBody {
    /// Mot de passe `OBSK` de la graine scellee.
    pub password: String,
}

/// IP cliente optionnelle (`ConnectInfo`) — tolérante a l'absence
/// de l'extension (tests qui servent le routeur sans
/// `into_make_service_with_connect_info`).
pub struct MaybeClientIp(pub Option<IpAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for MaybeClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip()),
        ))
    }
}

/// Corps `{password?}` de `POST /api/identity/create` — un mot de
/// passe non vide scelle la graine d'emblee (at-rest des la
/// creation).
#[derive(serde::Deserialize)]
pub struct CreateBody {
    /// Mot de passe at-rest optionnel.
    #[serde(default)]
    pub password: Option<String>,
}

/// Corps `{enabled, password?}` de `POST /api/identity/at_rest`.
#[derive(serde::Deserialize)]
pub struct AtRestBody {
    /// Active (`true`, mot de passe requis) ou desactive (`false`,
    /// mot de passe exige pour re-ecrire la graine en clair).
    pub enabled: bool,
    /// Mot de passe `OBSK` (nouveau pour l'activation, courant pour
    /// la desactivation).
    #[serde(default)]
    pub password: Option<String>,
}

/// Nom d'etat expose par `GET /api/identity` (et reutilise pour les
/// 409 du gate : `identity_pending` / `identity_locked`).
fn phase_name(phase: IdentityPhase) -> &'static str {
    match phase {
        IdentityPhase::Pending => "pending",
        IdentityPhase::Locked => "locked",
        IdentityPhase::Ready => "ready",
    }
}

/// `GET /api/identity` — etat du cycle de vie ADR-0016 :
/// `state`, `seeded`, `mode` (`"guest"`/`"persistent"`) et la cle
/// publique en `ready`. Jamais de materiel prive.
pub async fn get_identity(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let phase = state.session.identity_phase();
    match phase {
        IdentityPhase::Pending => Ok(Json(serde_json::json!({
            "state": "pending",
            "seeded": false,
            "storage_removable": state.session.storage_removable(),
        }))),
        IdentityPhase::Locked => Ok(Json(serde_json::json!({
            "state": "locked",
            "seeded": true,
            "at_rest": true,
            "storage_removable": state.session.storage_removable(),
        }))),
        IdentityPhase::Ready => {
            let Some(stack) = state.session.ipv8() else {
                // Stack desactivee : pas d'identite chargee (parite
                // historique du 404).
                return Err(ApiError::not_found("ipv8 desactive"));
            };
            Ok(Json(serde_json::json!({
                "state": "ready",
                "public_key": stack.public_key_hex(),
                "seeded": stack.identity_kind() == IdentityKind::Seeded,
                "mode": if state.session.is_guest() { "guest" } else { "persistent" },
                "persistent": !state.session.is_guest(),
                // Racine sur volume amovible ou sans ACL (cle USB,
                // exFAT…) : l'UI propose alors `identity.at_rest`
                // (bandeau non bloquant, ADR-0018 etape 63).
                "storage_removable": state.session.storage_removable(),
                // La cle privee n'est jamais exposee en lecture simple —
                // seul l'export explicite la fournit.
            })))
        }
    }
}

/// Verrou commun aux trois resolutions du gate : refuse hors
/// `Pending`/`Locked`, demarre la session avec le materiel resolu.
async fn resolve_identity(
    state: &AppState,
    material: identity::IdentityMaterial,
) -> Result<Json<serde_json::Value>, ApiError> {
    let phase = state.session.identity_phase();
    if phase == IdentityPhase::Ready {
        return Ok(Json(serde_json::json!({ "state": "ready" })));
    }
    state
        .session
        .try_start_identity(Some(material))
        .await
        .map_err(|e| ApiError::internal(format!("demarrage identite: {e}")))?;
    Ok(Json(serde_json::json!({
        "state": "ready",
        "restart_required": false,
    })))
}

/// `POST /api/identity/unlock` — ouvre la graine `OBSK` et demarre
/// les composants identitaires. Rate-limite par IP et globalement
/// (`identity.unlock_*` de la config) ; double appel = `ready`
/// idempotent ; mauvais mot de passe = erreur uniforme 400 (rien
/// n'est revele sur l'etat du blob).
pub async fn unlock_identity(
    State(state): State<AppState>,
    MaybeClientIp(client_ip): MaybeClientIp,
    Json(body): Json<UnlockBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let phase = state.session.identity_phase();
    match phase {
        // Idempotent : un second unlock sur une session deja
        // resolue repond `ready` sans rien retenter.
        IdentityPhase::Ready => return Ok(Json(serde_json::json!({ "state": "ready" }))),
        IdentityPhase::Pending => {
            return Err(ApiError::conflict("identity_pending"));
        }
        IdentityPhase::Locked => {}
    }
    let (per_ip_max, global_max, window_secs) = {
        let cfg = state
            .daemon_config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            cfg.identity.unlock_per_ip_per_min,
            cfg.identity.unlock_global_per_min,
            cfg.identity.unlock_window_secs,
        )
    };
    let ip = client_ip.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    if !state.unlock_limiter.admit(
        ip,
        per_ip_max,
        global_max,
        std::time::Duration::from_secs(window_secs),
    ) {
        return Err(ApiError::too_many_requests(
            "trop de tentatives de deverrouillage",
        ));
    }
    let state_dir = state.session.config().state_dir.clone();
    let material = tokio::task::spawn_blocking(move || {
        identity::unlock_seed(&state_dir, body.password.as_bytes())
    })
    .await
    .map_err(|e| ApiError::internal(format!("unlock task: {e}")))?
    // Erreur uniforme : mauvais mot de passe et blob corrompu ne se
    // distinguent pas pour l'appelant.
    .map_err(|_| ApiError::bad_request("mot de passe incorrect"))?;
    resolve_identity(&state, material).await
}

/// `POST /api/identity/create` — resolution « nouvelle identite »
/// du gate `identity_pending` : graine neuve persistante, ou
/// `OBSK` d'emblee si `password` est fourni.
pub async fn create_identity(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.session.identity_phase() {
        IdentityPhase::Pending => {}
        IdentityPhase::Locked => return Err(ApiError::conflict("identity_locked")),
        IdentityPhase::Ready => {
            return Err(ApiError::conflict("une identite existe deja"));
        }
    }
    let state_dir = state.session.config().state_dir.clone();
    let material = tokio::task::spawn_blocking(move || {
        identity::create_seed(&state_dir, body.password.as_deref())
    })
    .await
    .map_err(|e| ApiError::internal(format!("create task: {e}")))?
    .map_err(|e| ApiError::bad_request(format!("creation impossible: {e}")))?;
    resolve_identity(&state, material).await
}

/// `POST /api/identity/guest` — session invitee : identite
/// ephemere en memoire, aucun fichier, base `:memory:` ; tout meurt
/// a la fermeture du daemon. Resout `pending` comme `locked` (le
/// detenteur du mot de passe peut preferer une session brulee).
pub async fn guest_identity(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.session.identity_phase() {
        IdentityPhase::Ready => return Err(ApiError::conflict("identite deja resolue")),
        IdentityPhase::Pending | IdentityPhase::Locked => {}
    }
    resolve_identity(&state, identity::IdentityMaterial::guest()).await
}

/// `POST /api/identity/at_rest` — active/desactive le scellement
/// `OBSK` de la graine (ADR-0016). L'activation exige une identite
/// seedee en clair et `stealth.role == "client"` ; la desactivation
/// exige le mot de passe courant. L'etat persiste dans
/// `identity.at_rest` de `configuration.json`.
pub async fn set_at_rest(
    State(state): State<AppState>,
    Json(body): Json<AtRestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.session.identity_phase() != IdentityPhase::Ready {
        return Err(ApiError::conflict(format!(
            "identity_{}",
            phase_name(state.session.identity_phase())
        )));
    }
    let stealth_role_client = {
        let cfg = state
            .daemon_config
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        !cfg.stealth.enabled || cfg.stealth.role == "client"
    };
    let state_dir = state.session.config().state_dir.clone();
    let password = body.password.unwrap_or_default();
    if password.is_empty() {
        return Err(ApiError::bad_request("mot de passe requis"));
    }
    if body.enabled {
        if !stealth_role_client {
            return Err(ApiError::bad_request(
                "identity.at_rest incompatible avec stealth.role != \"client\"",
            ));
        }
        let pw = password.clone();
        let dir = state_dir.clone();
        tokio::task::spawn_blocking(move || identity::seal_seed(&dir, pw.as_bytes()))
            .await
            .map_err(|e| ApiError::internal(format!("seal task: {e}")))?
            .map_err(|e| ApiError::bad_request(format!("scellement impossible: {e}")))?;
    } else {
        let dir = state_dir.clone();
        tokio::task::spawn_blocking(move || identity::unseal_seed(&dir, password.as_bytes()))
            .await
            .map_err(|e| ApiError::internal(format!("unseal task: {e}")))?
            .map_err(|_| ApiError::bad_request("mot de passe incorrect"))?;
    }
    let mut cfg = state
        .daemon_config
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cfg.identity.at_rest = body.enabled;
    if let Some(path) = &state.config_path {
        cfg.write(path)
            .map_err(|e| ApiError::internal(format!("ecriture configuration.json: {e}")))?;
    }
    Ok(Json(serde_json::json!({
        "modified": true,
        "at_rest": body.enabled,
    })))
}

/// `GET /api/identity/recovery_phrase?lang=en|fr` — phrase BIP39 de
/// 24 mots de l'identite seedee. **Secret complet** : derriere
/// `api_key_auth` comme tout `/api`, jamais loggé, jamais dans les
/// events SSE. 404 sur une identite legacy (pas de graine = pas de
/// phrase — la seule sauvegarde reste l'export `OBID`).
pub async fn recovery_phrase(
    State(state): State<AppState>,
    Query(q): Query<RecoveryPhraseQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    if stack.identity_kind() != IdentityKind::Seeded {
        return Err(ApiError::not_found(
            "identite legacy — pas de phrase de recuperation (export OBID uniquement)",
        ));
    }
    let lang = match q.lang.as_deref().unwrap_or("en") {
        "en" => Language::English,
        "fr" => Language::French,
        other => return Err(ApiError::bad_request(format!("langue inconnue : {other}"))),
    };
    let state_dir = state.session.config().state_dir.clone();
    let raw = std::fs::read(onionbit_core::identity::seed_path(&state_dir))
        .map_err(|_| ApiError::internal("identity_seed.bin illisible"))?;
    let seed = IdentitySeed::from_bytes(&raw)
        .map_err(|_| ApiError::internal("identity_seed.bin corrompu"))?;
    // Revelation d'un secret complet — evenement notable (tracé warn,
    // la phrase elle-meme n'est JAMAIS logguee).
    tracing::warn!("phrase de recuperation revelee via l'API");
    Ok(Json(serde_json::json!({
        "phrase": bip39::encode(seed.as_bytes(), lang),
    })))
}

/// `POST /api/identity/export` — cle secrete `LibNaCLSK:` (hex brut)
/// ou blob `OBID` si `password` fourni. `{key, encrypted}`.
pub async fn export_identity(
    State(state): State<AppState>,
    Json(body): Json<IdentityExportBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(stack) = state.session.ipv8() else {
        return Err(ApiError::not_found("ipv8 desactive"));
    };
    let raw = stack.secret_key_bin();
    let password = body.password.unwrap_or_default();
    if password.is_empty() {
        return Ok(Json(serde_json::json!({
            "key": hex::encode(&raw),
            "encrypted": false,
        })));
    }
    let blob = keyblob_seal(password.as_bytes(), &raw)
        .map_err(|_| ApiError::bad_request("export impossible"))?;
    Ok(Json(serde_json::json!({
        "key": hex::encode(&blob),
        "encrypted": true,
    })))
}

/// `POST /api/identity/restore` — deux formes exclusives :
///
/// - `{phrase}` : phrase BIP39 → installe `identity_seed.bin` et
///   regenere `ipv8_keypair.bin` + `stealth_bridge.key` (ADR-0016) ;
/// - `{key, password?}` : cle `LibNaCLSK:` hex brute ou blob `OBID`
///   (chemin legacy — refuse sur install seedee, la graine
///   gagnerait au prochain boot).
///
/// En phase `pending`/`locked` la nouvelle identite **demarre la
/// session immediatement** (`restart_required: false`) — rien ne
/// tournait encore. En `ready`, `restart_required: true` : on ne
/// change jamais d'identite sous des communautes actives.
pub async fn restore_identity(
    State(state): State<AppState>,
    Json(body): Json<IdentityRestoreBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_dir = state.session.config().state_dir.clone();
    let phase = state.session.identity_phase();
    if let Some(phrase) = body.phrase {
        if body.key.is_some() {
            return Err(ApiError::bad_request("phrase OU key, pas les deux"));
        }
        // Validation checksum AVANT toute ecriture — une phrase
        // invalide en `pending` laisse le state_dir vierge.
        let entropy = bip39::decode(&phrase)
            .map_err(|e| ApiError::bad_request(format!("phrase invalide : {e}")))?;
        let seed = IdentitySeed::from_bytes(&entropy)
            .map_err(|_| ApiError::bad_request("entropie de phrase invalide"))?;
        onionbit_core::identity::restore_seed(&state_dir, &seed)
            .map_err(|e| ApiError::bad_request(format!("ecriture impossible: {e}")))?;
        if phase != IdentityPhase::Ready {
            return resolve_identity(&state, identity::material_from_seed(&seed)).await;
        }
        return Ok(Json(serde_json::json!({ "restart_required": true })));
    }
    let Some(key) = body.key else {
        return Err(ApiError::bad_request("phrase ou key requis"));
    };
    let blob = hex::decode(key.trim()).map_err(|_| ApiError::bad_request("cle attendue en hex"))?;
    let raw = if keyblob_is_sealed(&blob) {
        let password = body.password.unwrap_or_default();
        if password.is_empty() {
            return Err(ApiError::bad_request(
                "blob OBID chiffre — mot de passe requis",
            ));
        }
        keyblob_open(password.as_bytes(), &blob)
            .map_err(|_| ApiError::bad_request("dechiffrement impossible (mot de passe ?)"))?
    } else {
        blob
    };
    // Validation format avant toute ecriture.
    LibNaClSecretKey::from_bin(&raw)
        .map_err(|_| ApiError::bad_request("cle LibNaCLSK mal formee"))?;
    onionbit_core::ipv8_stack::restore_identity_key(&state_dir, &raw, body.force_legacy)
        .map_err(|e| ApiError::bad_request(format!("ecriture impossible: {e}")))?;
    if phase != IdentityPhase::Ready {
        // Identite legacy fraichement posee : le chargement relit les
        // fichiers (meme chemin qu'un boot normal).
        let material = identity::load_or_generate(&state_dir)
            .map_err(|e| ApiError::internal(format!("chargement identite: {e}")))?;
        return resolve_identity(&state, material).await;
    }
    Ok(Json(serde_json::json!({ "restart_required": true })))
}
