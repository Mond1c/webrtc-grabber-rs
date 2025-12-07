use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignallingError {
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Invalid message format: {0}")]
    InvalidMessageFormat(String),

    #[error("SFU error: {0}")]
    SfuError(#[from] anyhow::Error),

    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl IntoResponse for SignallingError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            SignallingError::AuthenticationFailed(msg) => (StatusCode::UNAUTHORIZED, msg),
            SignallingError::PeerNotFound(msg) => (StatusCode::NOT_FOUND, msg),
            SignallingError::Timeout(msg) => (StatusCode::REQUEST_TIMEOUT, msg),
            SignallingError::InvalidMessageFormat(msg) => (StatusCode::BAD_REQUEST, msg),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, SignallingError>;
