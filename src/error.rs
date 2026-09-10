use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;
use sqlx::Error as SqlxError;

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Database error")]
    Database(#[from] SqlxError),

    #[error("Authentication failed")]
    Unauthorized,

    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Resource not found")]
    NotFound,

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Invalid state transition: {0}")]
    InvalidStateTransition(String),

    #[error("Idempotency key conflict: {0}")]
    IdempotencyConflict(String),

    #[error("PSP error: {0}")]
    PspError(String),

    #[error("Webhook delivery failed: {0}")]
    WebhookError(String),

    #[error("Internal server error")]
    Internal,

    #[error("Concurrent modification detected")]
    ConcurrentModification,

    #[error("Customer already exists")]
    CustomerAlreadyExists,

    #[error("Invoice already paid")]
    AlreadyPaid,

    #[error("Configuration error: {0}")]
    Config(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error, message, details) = match &self {
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication required".to_string(),
                None,
            ),
            AppError::InvalidApiKey => (
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
                "Invalid or revoked API key".to_string(),
                None,
            ),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Resource not found".to_string(),
                None,
            ),
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "bad_request",
                msg.clone(),
                None,
            ),
            AppError::InvalidStateTransition(msg) => (
                StatusCode::CONFLICT,
                "invalid_state_transition",
                msg.clone(),
                None,
            ),
            AppError::IdempotencyConflict(msg) => (
                StatusCode::CONFLICT,
                "idempotency_conflict",
                msg.clone(),
                None,
            ),
            AppError::PspError(msg) => (
                StatusCode::BAD_GATEWAY,
                "psp_error",
                msg.clone(),
                None,
            ),
            AppError::Database(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Database operation failed".to_string(),
                Some(err.to_string()),
            ),
            AppError::CustomerAlreadyExists => (
                StatusCode::CONFLICT,
                "customer_exists",
                "Customer with this email already exists".to_string(),
                None,
            ),
            AppError::AlreadyPaid => (
                StatusCode::CONFLICT,
                "already_paid",
                "Invoice is already paid".to_string(),
                None,
            ),
            AppError::ConcurrentModification => (
                StatusCode::CONFLICT,
                "concurrent_modification",
                "Resource was modified concurrently".to_string(),
                None,
            ),
            AppError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred".to_string(),
                None,
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred".to_string(),
                None,
            ),
        };

        let body = ErrorResponse {
            error: error.to_string(),
            message,
            details,
        };

        (status, Json(body)).into_response()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;