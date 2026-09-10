use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthenticatedBusiness;
use crate::error::{AppError, Result};
use crate::models::{CreateCustomerRequest, Customer};

pub async fn create_customer(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Json(req): Json<CreateCustomerRequest>,
) -> Result<impl IntoResponse> {
    let customer = sqlx::query_as!(
        Customer,
        "INSERT INTO customers (business_id, name, email)
         VALUES ($1, $2, $3)
         ON CONFLICT (business_id, email) DO NOTHING
         RETURNING id, business_id, name, email, created_at, updated_at",
        auth.id,
        req.name,
        req.email
    )
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    match customer {
        Some(c) => Ok((StatusCode::CREATED, Json(c))),
        None => {
            // Check if conflict exists
            let existing = sqlx::query_as!(
                Customer,
                "SELECT id, business_id, name, email, created_at, updated_at
                 FROM customers
                 WHERE business_id = $1 AND email = $2",
                auth.id,
                req.email
            )
            .fetch_optional(&pool)
            .await
            .map_err(|_| AppError::Internal)?;

            if existing.is_some() {
                Err(AppError::CustomerAlreadyExists)
            } else {
                Err(AppError::Internal)
            }
        }
    }
}

pub async fn get_customer(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let customer = sqlx::query_as!(
        Customer,
        "SELECT id, business_id, name, email, created_at, updated_at
         FROM customers
         WHERE id = $1 AND business_id = $2",
        id,
        auth.id
    )
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    match customer {
        Some(c) => Ok(Json(c)),
        None => Err(AppError::NotFound),
    }
}

pub async fn list_customers(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
) -> Result<impl IntoResponse> {
    let customers = sqlx::query_as!(
        Customer,
        "SELECT id, business_id, name, email, created_at, updated_at
         FROM customers
         WHERE business_id = $1
         ORDER BY created_at DESC",
        auth.id
    )
    .fetch_all(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(Json(customers))
}