use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{generate_api_key, store_api_key};
use crate::error::{AppError, Result};
use crate::models::{CreateBusinessRequest, CreateBusinessResponse};

#[derive(serde::Serialize)]
pub struct ApiKeyInfo {
    pub id: Uuid,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn create_business(
    State(pool): State<PgPool>,
    Json(req): Json<CreateBusinessRequest>,
) -> Result<impl IntoResponse> {
    let business = sqlx::query_as!(
        crate::models::Business,
        "INSERT INTO businesses (name) VALUES ($1) RETURNING id, name, created_at, updated_at",
        req.name
    )
    .fetch_one(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let (full_key, hash) = generate_api_key(business.id);
    let (prefix, _) = full_key.rsplit_once('_').unwrap();

    store_api_key(&pool, business.id, prefix, &hash).await?;

    Ok((StatusCode::CREATED, Json(CreateBusinessResponse {
        id: business.id,
        api_key: full_key,
    })))
}

pub async fn revoke_key(
    State(pool): State<PgPool>,
    auth: crate::auth::AuthenticatedBusiness,
    axum::extract::Path(key_id): axum::extract::Path<Uuid>,
) -> Result<impl IntoResponse> {
    sqlx::query!(
        "UPDATE api_keys SET revoked_at = NOW() WHERE id = $1 AND business_id = $2",
        key_id,
        auth.id
    )
    .execute(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_keys(
    State(pool): State<PgPool>,
    auth: crate::auth::AuthenticatedBusiness,
) -> Result<impl IntoResponse> {
    let keys = sqlx::query_as!(
        ApiKeyInfo,
        "SELECT id, key_prefix, created_at, revoked_at FROM api_keys WHERE business_id = $1",
        auth.id
    )
    .fetch_all(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(Json(keys))
}