use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthenticatedBusiness;
use crate::error::{AppError, Result};
use crate::models::{CreateWebhookRequest, WebhookEndpoint, WebhookResponse};

pub async fn register_webhook(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse> {
    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Err(AppError::BadRequest("Invalid URL scheme".to_string()));
    }

    let secret = Uuid::new_v4().simple().to_string();

    let endpoint = sqlx::query_as!(
        WebhookEndpoint,
        "INSERT INTO webhook_endpoints (business_id, url, secret)
         VALUES ($1, $2, $3)
         ON CONFLICT (business_id, url) DO UPDATE SET updated_at = NOW()
         RETURNING id, business_id, url, secret, created_at, updated_at",
        auth.id,
        req.url,
        secret
    )
    .fetch_one(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok((StatusCode::CREATED, Json(WebhookResponse {
        id: endpoint.id,
        url: endpoint.url,
        secret: endpoint.secret,
    })))
}

pub async fn list_webhooks(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
) -> Result<impl IntoResponse> {
    let endpoints = sqlx::query_as!(
        WebhookEndpoint,
        "SELECT id, business_id, url, secret, created_at, updated_at
         FROM webhook_endpoints
         WHERE business_id = $1",
        auth.id
    )
    .fetch_all(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(Json(endpoints))
}

pub async fn delete_webhook(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let result = sqlx::query!(
        "DELETE FROM webhook_endpoints WHERE id = $1 AND business_id = $2",
        id,
        auth.id
    )
    .execute(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}