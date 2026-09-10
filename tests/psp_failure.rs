mod common;

use common::{
    create_test_business, create_test_customer, create_test_invoice, setup_test_db, TestClient,
};
use dodo_payments::models::PaymentRequest;
use serial_test::serial;
use tokio::time::{sleep, Duration};

#[tokio::test]
#[serial]
async fn test_psp_timeout() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);

    let payment_req = PaymentRequest {
        card_token: "tok_timeout".to_string(),
    };

    let start = std::time::Instant::now();

    // The request should not hang indefinitely (timeout is 35s)
    let response = client
        .post(&format!("/api/invoices/{}/pay", invoice_id), &payment_req)
        .await;

    let elapsed = start.elapsed();

    // Should complete within reasonable time (not 30s+)
    assert!(
        elapsed < Duration::from_secs(35),
        "Request should not hang for 30s timeout"
    );

    // Should handle timeout gracefully
    // Either it fails quickly or completes successfully
    let status = response.status();
    assert!(status.is_success() || status.as_u16() == 502 || status.as_u16() == 504);

    // Verify invoice is not stuck in corrupt state
    let invoice = sqlx::query!("SELECT state FROM invoices WHERE id = $1", invoice_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Invoice should remain in open state (not paid or corrupt)
    assert_eq!(invoice.state, "open", "Invoice should remain in open state");

    // Verify payment attempt was recorded
    let attempts = sqlx::query!(
        "SELECT status, error_code FROM payment_attempts WHERE invoice_id = $1",
        invoice_id
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(!attempts.is_empty(), "Payment attempt should be recorded");

    let attempt = &attempts[0];
    assert_eq!(
        attempt.status, "failed",
        "Payment attempt should be marked as failed"
    );
    assert!(
        attempt.error_code.is_some(),
        "Error code should be recorded"
    );
}

#[tokio::test]
#[serial]
async fn test_psp_network_error() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);

    let payment_req = PaymentRequest {
        card_token: "tok_network_error".to_string(),
    };

    let response = client
        .post(&format!("/api/invoices/{}/pay", invoice_id), &payment_req)
        .await;

    // Should handle 500 error gracefully
    let status = response.status();
    assert!(status.as_u16() == 502 || status.as_u16() == 500 || status.is_success());

    if !status.is_success() {
        // Verify invoice state is unchanged
        let invoice = sqlx::query!("SELECT state FROM invoices WHERE id = $1", invoice_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(invoice.state, "open", "Invoice should remain in open state");

        // Verify payment attempt was recorded with error
        let attempts = sqlx::query!(
            "SELECT status, error_code FROM payment_attempts WHERE invoice_id = $1",
            invoice_id
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(!attempts.is_empty(), "Payment attempt should be recorded");
        let attempt = &attempts[0];
        assert_eq!(
            attempt.status, "failed",
            "Payment attempt should be marked as failed"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_psp_failed_payment_state() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);

    let payment_req = PaymentRequest {
        card_token: "tok_card_declined".to_string(),
    };

    let response = client
        .post(&format!("/api/invoices/{}/pay", invoice_id), &payment_req)
        .await;

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error_code"], "card_declined");

    // Verify invoice remains open
    let invoice = sqlx::query!("SELECT state FROM invoices WHERE id = $1", invoice_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        invoice.state, "open",
        "Invoice should remain open after failed payment"
    );
}
