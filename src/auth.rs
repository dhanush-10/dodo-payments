use crate::error::{AppError, Result};
use crate::models::ApiKey;
use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use async_trait::async_trait;
use axum::{extract::FromRequestParts, http::request::Parts};
use rand::rngs::OsRng;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AuthenticatedBusiness {
    pub id: Uuid,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthenticatedBusiness
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self> {
        let pool = parts
            .extensions
            .get::<PgPool>()
            .ok_or_else(|| AppError::Internal)?
            .clone();

        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized)?;

        if !auth_header.starts_with("Bearer ") {
            return Err(AppError::Unauthorized);
        }

        let api_key = &auth_header[7..];
        validate_api_key(&pool, api_key).await
    }
}

pub async fn validate_api_key(pool: &PgPool, api_key: &str) -> Result<AuthenticatedBusiness> {
    let (prefix, key) = api_key
        .rsplit_once('_')
        .ok_or_else(|| AppError::InvalidApiKey)?;

    let stored = sqlx::query_as!(
        ApiKey,
        "SELECT id, business_id, key_prefix, key_hash, created_at, revoked_at 
         FROM api_keys 
         WHERE key_prefix = $1 AND revoked_at IS NULL",
        prefix
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::InvalidApiKey)?;

    let stored = stored.ok_or_else(|| AppError::InvalidApiKey)?;

    let parsed_hash = PasswordHash::new(&stored.key_hash).map_err(|_| AppError::InvalidApiKey)?;
    let argon2 = Argon2::default();

    if argon2.verify_password(key.as_bytes(), &parsed_hash).is_ok() {
        Ok(AuthenticatedBusiness {
            id: stored.business_id,
        })
    } else {
        Err(AppError::InvalidApiKey)
    }
}

pub fn generate_api_key(_business_id: Uuid) -> (String, String) {
    let prefix = format!(
        "dk_{}",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let key = Uuid::new_v4().simple().to_string();
    let full_key = format!("{}_{}", prefix, key);

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(key.as_bytes(), &salt)
        .expect("hash failed")
        .to_string();

    (full_key, hash)
}

pub async fn store_api_key(
    pool: &PgPool,
    business_id: Uuid,
    prefix: &str,
    hash: &str,
) -> Result<ApiKey> {
    let key = sqlx::query_as!(
        ApiKey,
        "INSERT INTO api_keys (business_id, key_prefix, key_hash)
         VALUES ($1, $2, $3)
         RETURNING id, business_id, key_prefix, key_hash, created_at, revoked_at",
        business_id,
        prefix,
        hash
    )
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(key)
}

pub async fn revoke_api_key(pool: &PgPool, business_id: Uuid, key_id: Uuid) -> Result<()> {
    let result = sqlx::query!(
        "UPDATE api_keys SET revoked_at = NOW() 
         WHERE id = $1 AND business_id = $2 AND revoked_at IS NULL",
        key_id,
        business_id
    )
    .execute(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(())
}