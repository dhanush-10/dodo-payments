mod common;

use common::{
    create_test_business, create_test_customer, create_test_invoice, setup_test_db, TestClient,
};
use dodo_payments::models::PaymentRequest;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_idempotency_same_request() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);
    let payment_req = PaymentRequest {
        card_token: "tok_success".to_string(),
    };

    let idempotency_key = "test-key-1";

    // First request
    let response1 = client
        .post_with_idempotency(
            &format!("/api/invoices/{}/pay", invoice_id),
            &payment_req,
            idempotency_key,
        )
        .await;

    assert!(response1.status().is_success());

    let body1: serde_json::Value = response1.json().await.unwrap();
    let status1 = body1["status"].as_str().unwrap();

    // Second request with same key
    let response2 = client
        .post_with_idempotency(
            &format!("/api/invoices/{}/pay", invoice_id),
            &payment_req,
            idempotency_key,
        )
        .await;

    assert!(response2.status().is_success());

    let body2: serde_json::Value = response2.json().await.unwrap();
    let status2 = body2["status"].as_str().unwrap();

    assert_eq!(
        status1, status2,
        "Same idempotency key should return same result"
    );
    assert_eq!(body1, body2, "Full response should be identical");

    // Verify only one payment attempt exists
    let attempts = sqlx::query!(
        "SELECT COUNT(*) as count FROM payment_attempts WHERE invoice_id = $1 AND idempotency_key = $2",
        invoice_id,
        idempotency_key
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        attempts.count.unwrap(),
        1,
        "Only one payment attempt should exist"
    );
}

#[tokio::test]
#[serial]
async fn test_idempotency_different_body() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);
    let idempotency_key = "test-key-2";

    let req1 = PaymentRequest {
        card_token: "tok_success".to_string(),
    };

    let req2 = PaymentRequest {
        card_token: "tok_card_declined".to_string(),
    };

    // First request with valid token
    let response1 = client
        .post_with_idempotency(
            &format!("/api/invoices/{}/pay", invoice_id),
            &req1,
            idempotency_key,
        )
        .await;
    assert!(response1.status().is_success());

    // Second request with different body but same key - should be rejected
    let response2 = client
        .post_with_idempotency(
            &format!("/api/invoices/{}/pay", invoice_id),
            &req2,
            idempotency_key,
        )
        .await;

    assert_eq!(
        response2.status().as_u16(),
        409,
        "Different request body with same key should be rejected"
    );
}
