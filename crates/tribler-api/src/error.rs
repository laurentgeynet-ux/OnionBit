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
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.body }))).into_response()
    }
}

impl From<tribler_core::CoreError> for ApiError {
    fn from(e: tribler_core::CoreError) -> Self {
        match e {
            tribler_core::CoreError::Bt(tribler_bittorrent::BtError::NotFound(id)) => {
                ApiError::not_found(format!("this download does not exist: {id}"))
            }
            tribler_core::CoreError::InvalidState(m) => ApiError::not_found(m),
            other => ApiError::internal(other.to_string()),
        }
    }
}

impl From<tribler_db::DbError> for ApiError {
    fn from(e: tribler_db::DbError) -> Self {
        ApiError::internal(e.to_string())
    }
}

impl From<tribler_bittorrent::BtError> for ApiError {
    fn from(e: tribler_bittorrent::BtError) -> Self {
        ApiError::from(tribler_core::CoreError::Bt(e))
    }
}
