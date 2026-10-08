// This file is part of OnionBit - a Rust port of the Tribler daemon.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Erreurs HTTP de l'API, au format `{error: {handled, message}}`
//! de `tribler.core.restapi`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// Erreur serialisable telle que l'API Python la rend.
#[derive(Debug, serde::Serialize)]
pub struct ApiErrorBody {
    /// L'erreur a ete anticipee/geree (vs bug interne).
    pub handled: bool,
    /// Message lisible.
    pub message: String,
}

/// Erreur d'endpoint : statut HTTP + corps `{error: {...}}`.
#[derive(Debug)]
pub struct ApiError {
    /// Statut HTTP.
    pub status: StatusCode,
    /// Corps d'erreur.
    pub body: ApiErrorBody,
}

impl ApiError {
    /// 404 — ressource absente.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }

    /// 400 — requete invalide.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }

    /// 401 — cle API absente ou invalide (`ApiKeyMiddleware` Python :
    /// `{"error": {"handled": true, "message": "Unauthorized access"}}`).
    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ApiErrorBody {
                handled: true,
                message: "Unauthorized access".into(),
            },
        }
    }

    /// 409 — conflit d'etat (`identity_pending`, `identity_locked`
    /// — ADR-0016 etape 48d).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }

    /// 403 — action interdite par politique (zone privee non
    /// navigable en clair — ADR-0018).
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }

    /// 429 — plafond de tentatives atteint (`identity/unlock`,
    /// ADR-0016).
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }

    /// 500 — erreur interne.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiErrorBody {
                handled: false,
                message: message.into(),
            },
        }
    }

    /// 500 avec `handled: true` — equivalent de
    /// `return_handled_exception` Python (ex. `KeyError` sur un
    /// parametre obligatoire absent du corps JSON).
    pub fn internal_handled(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiErrorBody {
                handled: true,
                message: message.into(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.body }))).into_response()
    }
}

impl From<onionbit_core::CoreError> for ApiError {
    fn from(e: onionbit_core::CoreError) -> Self {
        match e {
            onionbit_core::CoreError::Bt(onionbit_bittorrent::BtError::NotFound(id)) => {
                ApiError::not_found(format!("this download does not exist: {id}"))
            }
            onionbit_core::CoreError::InvalidState(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        }
    }
}

impl From<onionbit_db::DbError> for ApiError {
    fn from(e: onionbit_db::DbError) -> Self {
        ApiError::internal(e.to_string())
    }
}

impl From<onionbit_bittorrent::BtError> for ApiError {
    fn from(e: onionbit_bittorrent::BtError) -> Self {
        ApiError::from(onionbit_core::CoreError::Bt(e))
    }
}
