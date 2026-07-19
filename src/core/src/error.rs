use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("authentication failed")]
    Unauthorized,
    #[error("actor is not allowed to perform this action")]
    Forbidden,
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("invalid request: {0}")]
    Validation(String),
    #[error("protocol version is not supported: {0}")]
    UnsupportedProtocol(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("embedding service error: {0}")]
    Embedding(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl CoreError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Validation(_) => "validation_error",
            Self::UnsupportedProtocol(_) => "unsupported_protocol",
            Self::Conflict(_) => "conflict",
            Self::Storage(_) => "storage_error",
            Self::Embedding(_) => "embedding_error",
            Self::Config(_) => "config_error",
            Self::Internal(_) => "internal_error",
        }
    }

    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Validation(_) | Self::UnsupportedProtocol(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Storage(_) | Self::Embedding(_) | Self::Config(_) | Self::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    #[must_use]
    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code().to_owned(),
            message: self.to_string(),
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
