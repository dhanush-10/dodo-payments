use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    Json,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthenticatedBusiness;
use crate::error::{AppError, Result};
use crate::models::{
    Invoice, InvoiceState, PaymentAttempt, PaymentRequest, PaymentResponse, PaymentStatus,
};
use crate::psp_client::PspClient;
use crate::state_machine::{can_pay, payment_result_transition};
use crate::webhooks;
use std::sync::Arc;

pub async fn process_payment(
    State(pool): State<PgPool>,
    State(psp_client): State<Arc<dyn PspClient>>,
    auth: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<PaymentRequest>,
) -> Result<impl IntoResponse> {
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            AppError::BadRequest("Idempotency-Key header required".to_string())
        })?;

    let existing = sqlx::query_as!(
        PaymentAttempt,
        r#"SELECT id, invoice_id, idempotency_key, psp_reference,
                  status as "status: PaymentStatus",
                  error_code, error_message, requested_at, completed_at, created_at
           FROM payment_attempts
           WHERE invoice_id = $1 AND idempotency_key = $2"#,
        invoice_id,
        idempotency_key
    )
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::Internal)?;

    if let Some(existing) = existing {
        let response = PaymentResponse {
            id: existing.id,
            invoice_id: existing.invoice_id,
            status: existing.status,
            psp_reference: existing.psp_reference,
            error_code: existing.error_code,
            requested_at: existing.requested_at,
            completed_at: existing.completed_at,
        };

        return Ok(Json(response));
    }

    let mut tx = pool.begin().await.map_err(|_| AppError::Internal)?;

    let invoice = sqlx::query_as!(
        Invoice,
        r#"SELECT id, business_id, customer_id, state as "state: InvoiceState",
                  total_cents, due_date, created_at, updated_at, paid_at, version
           FROM invoices
           WHERE id = $1 AND business_id = $2
           FOR UPDATE"#,
        invoice_id,
        auth.id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AppError::Internal)?;

    let invoice = match invoice {
        Some(i) => i,
        None => return Err(AppError::NotFound),
    };

    if !can_pay(invoice.state) {
        if invoice.state == InvoiceState::Paid {
            return Err(AppError::AlreadyPaid);
        }

        return Err(AppError::InvalidStateTransition(format!(
            "Cannot pay invoice in state {:?}",
            invoice.state
        )));
    }

    let attempt = sqlx::query_as!(
        PaymentAttempt,
        r#"INSERT INTO payment_attempts (invoice_id, idempotency_key, status)
           VALUES ($1, $2, 'pending')
           RETURNING id, invoice_id, idempotency_key, psp_reference,
                     status as "status: PaymentStatus",
                     error_code, error_message, requested_at, completed_at, created_at"#,
        invoice_id,
        idempotency_key
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| AppError::Internal)?;

    let psp_result = psp_client
        .process_payment(&req.card_token, invoice.total_cents, idempotency_key)
        .await;

    let (status, psp_ref, error_code, error_message) = match psp_result {
        Ok(response) => match response.status {
            crate::models::PspStatus::succeeded => {
                (PaymentStatus::Succeeded, response.psp_ref, None, None)
            }
            crate::models::PspStatus::failed => (
                PaymentStatus::Failed,
                None,
                response.code,
                Some("PSP payment failed".to_string()),
            ),
        },
        Err(e) => {
            let msg = e.to_string();
            (PaymentStatus::Failed, None, Some("psp_error".to_string()), Some(msg))
        }
    };

    sqlx::query!(
        "UPDATE payment_attempts
         SET status = $1,
             psp_reference = $2,
             error_code = $3,
             error_message = $4,
             completed_at = NOW()
         WHERE id = $5",
        status as PaymentStatus,
        psp_ref,
        error_code,
        error_message,
        attempt.id
    )
    .execute(&mut *tx)
    .await
    .map_err(|_| AppError::Internal)?;

    let final_invoice_state = if status == PaymentStatus::Succeeded {
        let new_state = payment_result_transition(invoice.state, true)?;

        sqlx::query!(
            "UPDATE invoices
             SET state = $1,
                 paid_at = NOW(),
                 version = version + 1
             WHERE id = $2 AND version = $3",
            new_state as InvoiceState,
            invoice_id,
            invoice.version
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| AppError::ConcurrentModification)?;

        new_state
    } else {
        invoice.state
    };

    tx.commit().await.map_err(|_| AppError::Internal)?;

    // NOTE: error_code is cloned below wherever it's used inside a spawn,
    // because the function needs the original value again afterward for
    // the response body — moving it into tokio::spawn would make it
    // unavailable there.
    if status == PaymentStatus::Succeeded {
        let pool_clone = pool.clone();
        let business_id = auth.id;
        let total_cents = invoice.total_cents;

        tokio::spawn(async move {
            let invoice_data = serde_json::json!({
                "id": invoice_id,
                "state": "paid",
                "total_cents": total_cents,
                "paid_at": chrono::Utc::now()
            });

            let _ = webhooks::enqueue_webhook(
                &pool_clone,
                business_id,
                "invoice.paid",
                invoice_data,
            )
            .await;
        });
    } else {
        let pool_clone = pool.clone();
        let business_id = auth.id;
        let total_cents = invoice.total_cents;
        let error_code_for_webhook = error_code.clone();
        let final_state_for_webhook = final_invoice_state;

        tokio::spawn(async move {
            let invoice_data = serde_json::json!({
                "id": invoice_id,
                "state": final_state_for_webhook,
                "error": error_code_for_webhook,
                "total_cents": total_cents
            });

            let _ = webhooks::enqueue_webhook(
                &pool_clone,
                business_id,
                "invoice.payment_failed",
                invoice_data,
            )
            .await;
        });
    }

    Ok(Json(PaymentResponse {
        id: attempt.id,
        invoice_id: attempt.invoice_id,
        status,
        psp_reference: psp_ref,
        error_code,
        requested_at: attempt.requested_at,
        completed_at: Some(chrono::Utc::now()),
    }))
}