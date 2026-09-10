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
use crate::models::{
    CreateInvoiceRequest, Invoice, InvoiceResponse, InvoiceState, LineItem,
    ListInvoicesQuery,
};

pub async fn create_invoice(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<impl IntoResponse> {
    if req.line_items.is_empty() {
        return Err(AppError::BadRequest(
            "Invoice must contain at least one line item".to_string(),
        ));
    }

    for item in &req.line_items {
        if item.description.trim().is_empty() {
            return Err(AppError::BadRequest(
                "Line item description cannot be empty".to_string(),
            ));
        }

        if item.quantity <= 0 {
            return Err(AppError::BadRequest(
                "Line item quantity must be greater than zero".to_string(),
            ));
        }

        if item.unit_amount_cents <= 0 {
            return Err(AppError::BadRequest(
                "Line item unit amount must be greater than zero".to_string(),
            ));
        }
    }

    // Make sure the customer belongs to this business.
    let customer_exists = sqlx::query!(
        r#"
        SELECT id
        FROM customers
        WHERE id = $1
          AND business_id = $2
        "#,
        req.customer_id,
        auth.id
    )
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    if customer_exists.is_none() {
        return Err(AppError::NotFound);
    }

    // Calculate total safely using i64 to avoid intermediate overflow.
    let mut total_cents: i64 = 0;

    for item in &req.line_items {
        let line_total = (item.quantity as i64)
            .checked_mul(item.unit_amount_cents as i64)
            .ok_or_else(|| {
                AppError::BadRequest(
                    "Invoice total is too large".to_string(),
                )
            })?;

        total_cents = total_cents
            .checked_add(line_total)
            .ok_or_else(|| {
                AppError::BadRequest(
                    "Invoice total is too large".to_string(),
                )
            })?;
    }

    if total_cents > i32::MAX as i64 {
        return Err(AppError::BadRequest(
            "Invoice total exceeds the supported limit".to_string(),
        ));
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| AppError::Internal)?;

    let invoice = sqlx::query_as!(
        Invoice,
        r#"
        INSERT INTO invoices (
            business_id,
            customer_id,
            state,
            total_cents,
            due_date
        )
        VALUES ($1, $2, 'draft', $3, $4)
        RETURNING
            id,
            business_id,
            customer_id,
            state as "state: InvoiceState",
            total_cents,
            due_date,
            created_at,
            updated_at,
            paid_at,
            version
        "#,
        auth.id,
        req.customer_id,
        total_cents as i32,
        req.due_date
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| AppError::Internal)?;

    let mut line_items = Vec::with_capacity(req.line_items.len());

    for item in req.line_items {
        let line_item = sqlx::query_as!(
            LineItem,
            r#"
            INSERT INTO line_items (
                invoice_id,
                description,
                quantity,
                unit_amount_cents
            )
            VALUES ($1, $2, $3, $4)
            RETURNING
                id,
                invoice_id,
                description,
                quantity,
                unit_amount_cents,
                created_at
            "#,
            invoice.id,
            item.description,
            item.quantity,
            item.unit_amount_cents
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AppError::Internal)?;

        line_items.push(line_item);
    }

    tx.commit()
        .await
        .map_err(|_| AppError::Internal)?;

    let response = InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        state: invoice.state,
        total_cents: invoice.total_cents,
        due_date: invoice.due_date,
        line_items,
        created_at: invoice.created_at,
        paid_at: invoice.paid_at,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn open_invoice(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let mut tx = pool.begin().await.map_err(|_| AppError::Internal)?;

    let invoice = sqlx::query_as!(
        Invoice,
        r#"
        SELECT id, business_id, customer_id,
               state as "state: InvoiceState",
               total_cents, due_date, created_at,
               updated_at, paid_at, version
        FROM invoices
        WHERE id = $1 AND business_id = $2
        FOR UPDATE
        "#,
        invoice_id,
        auth.id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppError::Internal)?
    .ok_or(AppError::NotFound)?;

    let new_state = crate::state_machine::validate_transition(
        invoice.state,
        InvoiceState::Open,
    )?;

    sqlx::query!(
        r#"
        UPDATE invoices
        SET state = $1,
            version = version + 1,
            updated_at = NOW()
        WHERE id = $2 AND version = $3
        "#,
        new_state as InvoiceState,
        invoice_id,
        invoice.version
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| AppError::ConcurrentModification)?;

    tx.commit().await.map_err(|_| AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "id": invoice_id,
        "state": "open"
    })))
}

pub async fn list_invoices(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Query(query): Query<ListInvoicesQuery>,
) -> Result<impl IntoResponse> {
    let invoices = if let Some(state) = query.state {
        sqlx::query_as!(
            Invoice,
            r#"
            SELECT
                id,
                business_id,
                customer_id,
                state as "state: InvoiceState",
                total_cents,
                due_date,
                created_at,
                updated_at,
                paid_at,
                version
            FROM invoices
            WHERE business_id = $1
              AND state = $2
            ORDER BY created_at DESC
            "#,
            auth.id,
            state as InvoiceState
        )
        .fetch_all(&pool)
        .await
        .map_err(|_| AppError::Internal)?
    } else {
        sqlx::query_as!(
            Invoice,
            r#"
            SELECT
                id,
                business_id,
                customer_id,
                state as "state: InvoiceState",
                total_cents,
                due_date,
                created_at,
                updated_at,
                paid_at,
                version
            FROM invoices
            WHERE business_id = $1
            ORDER BY created_at DESC
            "#,
            auth.id
        )
        .fetch_all(&pool)
        .await
        .map_err(|_| AppError::Internal)?
    };

    let mut responses = Vec::with_capacity(invoices.len());

    for invoice in invoices {
        let line_items = sqlx::query_as!(
            LineItem,
            r#"
            SELECT
                id,
                invoice_id,
                description,
                quantity,
                unit_amount_cents,
                created_at
            FROM line_items
            WHERE invoice_id = $1
            ORDER BY created_at ASC
            "#,
            invoice.id
        )
        .fetch_all(&pool)
        .await
        .map_err(|_| AppError::Internal)?;

        responses.push(InvoiceResponse {
            id: invoice.id,
            customer_id: invoice.customer_id,
            state: invoice.state,
            total_cents: invoice.total_cents,
            due_date: invoice.due_date,
            line_items,
            created_at: invoice.created_at,
            paid_at: invoice.paid_at,
        });
    }

    Ok(Json(responses))
}

pub async fn get_invoice(
    State(pool): State<PgPool>,
    auth: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let invoice = sqlx::query_as!(
        Invoice,
        r#"
        SELECT
            id,
            business_id,
            customer_id,
            state as "state: InvoiceState",
            total_cents,
            due_date,
            created_at,
            updated_at,
            paid_at,
            version
        FROM invoices
        WHERE id = $1
          AND business_id = $2
        "#,
        invoice_id,
        auth.id
    )
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let invoice = invoice.ok_or(AppError::NotFound)?;

    let line_items = sqlx::query_as!(
        LineItem,
        r#"
        SELECT
            id,
            invoice_id,
            description,
            quantity,
            unit_amount_cents,
            created_at
        FROM line_items
        WHERE invoice_id = $1
        ORDER BY created_at ASC
        "#,
        invoice.id
    )
    .fetch_all(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let response = InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        state: invoice.state,
        total_cents: invoice.total_cents,
        due_date: invoice.due_date,
        line_items,
        created_at: invoice.created_at,
        paid_at: invoice.paid_at,
    };

    Ok(Json(response))
}