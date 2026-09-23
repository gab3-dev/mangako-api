use std::net::AddrParseError;

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid environment variable {name}: {message}")]
    InvalidEnv { name: &'static str, message: String },
    #[error("invalid socket address")]
    InvalidAddr(#[from] AddrParseError),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("database migration error")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("external service error: {0}")]
    External(#[from] reqwest::Error),
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("{message}")]
    BadRequest { message: String },
    #[error("{message}")]
    Conflict { message: String },
    #[error("missing or invalid API token")]
    Unauthorized,
    #[error("rate limit exceeded")]
    TooManyRequests,
    #[error("manga not found")]
    MangaNotFound,
    #[error("volume not found")]
    VolumeNotFound,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub code: &'static str,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, code) = match &self {
            Self::BadRequest { .. } => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Conflict { .. } => (StatusCode::CONFLICT, "conflict"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Self::MangaNotFound => (StatusCode::NOT_FOUND, "manga_not_found"),
            Self::VolumeNotFound => (StatusCode::NOT_FOUND, "volume_not_found"),
            Self::MissingEnv { .. } | Self::InvalidEnv { .. } | Self::InvalidAddr(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "configuration_error")
            }
            Self::Database(_) | Self::Migration(_) | Self::External(_) | Self::Io(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        if status.is_server_error() {
            tracing::error!(error = ?self, %status, "request failed");
        }

        let body = Json(ErrorResponse {
            code,
            message: self.to_string(),
        });

        (status, body).into_response()
    }
}
