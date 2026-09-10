use crate::error::{AppError, Result};
use crate::models::{DeliveryStatus, WebhookDelivery, WebhookEndpoint};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub fn generate_signature(secret: &str, payload: &serde_json::Value) -> String {
    let payload_str = payload.to_string();

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");

    mac.update(payload_str.as_bytes());

    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_signature(
    secret: &str,
    payload: &serde_json::Value,
    signature: &str,
) -> bool {
    let expected = generate_signature(secret, payload);
    expected == signature
}

pub async fn enqueue_webhook(
    pool: &PgPool,
    business_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<Vec<WebhookDelivery>> {
    let endpoints = sqlx::query_as!(
        WebhookEndpoint,
        "SELECT id, business_id, url, secret, created_at, updated_at
         FROM webhook_endpoints
         WHERE business_id = $1",
        business_id
    )
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let mut deliveries = Vec::new();

    for endpoint in endpoints {
        let signature = generate_signature(&endpoint.secret, &payload);

        let delivery = sqlx::query_as!(
            WebhookDelivery,
            r#"INSERT INTO webhook_deliveries
               (endpoint_id, event_type, payload, signature, attempt, status)
               VALUES ($1, $2, $3, $4, 1, 'pending')
               RETURNING id, endpoint_id, event_type, payload, signature, attempt,
                         status as "status: DeliveryStatus",
                         response_status, response_body, error_message,
                         scheduled_at, completed_at, created_at, updated_at"#,
            endpoint.id,
            event_type,
            payload,
            signature
        )
        .fetch_one(pool)
        .await
        .map_err(|_| AppError::Internal)?;

        deliveries.push(delivery);
    }

    Ok(deliveries)
}

pub async fn process_pending_webhooks(pool: &PgPool) -> Result<()> {
    let now = Utc::now();

    let pending = sqlx::query_as!(
        WebhookDelivery,
        r#"SELECT id, endpoint_id, event_type, payload, signature, attempt,
                  status as "status: DeliveryStatus",
                  response_status, response_body, error_message,
                  scheduled_at, completed_at, created_at, updated_at
           FROM webhook_deliveries
           WHERE status = 'pending'
             AND scheduled_at <= $1
           ORDER BY created_at ASC
           LIMIT 100"#,
        now
    )
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    for delivery in pending {
        let endpoint = sqlx::query_as!(
            WebhookEndpoint,
            "SELECT id, business_id, url, secret, created_at, updated_at
             FROM webhook_endpoints
             WHERE id = $1",
            delivery.endpoint_id
        )
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::Internal)?;

        let endpoint = match endpoint {
            Some(e) => e,
            None => {
                mark_delivery_failed(
                    pool,
                    delivery.id,
                    "Endpoint not found".to_string(),
                )
                .await?;

                continue;
            }
        };

        let delivered = deliver_webhook(&endpoint, &delivery).await;

        if delivered {
            sqlx::query!(
                "UPDATE webhook_deliveries
                 SET status = 'delivered',
                     completed_at = NOW(),
                     updated_at = NOW()
                 WHERE id = $1",
                delivery.id
            )
            .execute(pool)
            .await
            .map_err(|_| AppError::Internal)?;
        } else {
            let next_attempt = delivery.attempt + 1;
            let max_attempts = 5;

            if next_attempt > max_attempts {
                mark_delivery_failed(
                    pool,
                    delivery.id,
                    "Max retry attempts exceeded".to_string(),
                )
                .await?;

                mark_delivery_exhausted(pool, delivery.id).await?;
            } else {
                let delay = calculate_backoff(next_attempt);
                let scheduled_at = Utc::now() + delay;

                sqlx::query!(
                    "UPDATE webhook_deliveries
                     SET attempt = $1,
                         scheduled_at = $2,
                         updated_at = NOW()
                     WHERE id = $3",
                    next_attempt,
                    scheduled_at,
                    delivery.id
                )
                .execute(pool)
                .await
                .map_err(|_| AppError::Internal)?;
            }
        }
    }

    Ok(())
}

async fn deliver_webhook(
    endpoint: &WebhookEndpoint,
    delivery: &WebhookDelivery,
) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();

    let response = client
        .post(&endpoint.url)
        .header("Content-Type", "application/json")
        .header("X-Webhook-Signature", &delivery.signature)
        .header("X-Webhook-Id", delivery.id.to_string())
        .header("X-Webhook-Event", &delivery.event_type)
        .json(&delivery.payload)
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();

            if status.is_success() {
                true
            } else {
                tracing::warn!(
                    "Webhook delivery failed with status {}: {}",
                    status,
                    delivery.id
                );

                false
            }
        }

        Err(e) => {
            tracing::warn!(
                "Webhook delivery error: {} - {}",
                e,
                delivery.id
            );

            false
        }
    }
}

fn calculate_backoff(attempt: i32) -> Duration {
    // Exponential backoff: 1s, 2s, 4s, 8s, 16s
    let seconds = 2_i64.pow((attempt - 1) as u32);
    Duration::seconds(seconds)
}

async fn mark_delivery_failed(
    pool: &PgPool,
    delivery_id: Uuid,
    error_message: String,
) -> Result<()> {
    sqlx::query!(
        "UPDATE webhook_deliveries
         SET status = 'failed',
             error_message = $1,
             completed_at = NOW(),
             updated_at = NOW()
         WHERE id = $2",
        error_message,
        delivery_id
    )
    .execute(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(())
}

async fn mark_delivery_exhausted(
    pool: &PgPool,
    delivery_id: Uuid,
) -> Result<()> {
    sqlx::query!(
        "UPDATE webhook_deliveries
         SET status = 'exhausted',
             updated_at = NOW()
         WHERE id = $1",
        delivery_id
    )
    .execute(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(())
}

pub async fn retry_failed_webhooks(
    pool: &PgPool,
    hours: i64,
) -> Result<()> {
    let cutoff = Utc::now() - Duration::hours(hours);

    sqlx::query!(
        "UPDATE webhook_deliveries
         SET status = 'pending',
             scheduled_at = NOW(),
             updated_at = NOW()
         WHERE status = 'failed'
           AND completed_at >= $1",
        cutoff
    )
    .execute(pool)
    .await
    .map_err(|_| AppError::Internal)?;

    Ok(())
}