mod common;

use common::{
    create_test_business, create_test_customer, create_test_invoice, setup_test_db, TestClient,
};
use dodo_payments::models::PaymentRequest;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_concurrent_payments_no_double_charge() {
    let pool = setup_test_db().await;
    let (business_id, api_key) = create_test_business(&pool).await;
    let customer_id = create_test_customer(&pool, business_id).await;
    let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

    let client = TestClient::new("http://localhost:3000".to_string(), api_key);

    let payment_req = PaymentRequest {
        card_token: "tok_success".to_string(),
    };

    let mut handles = vec![];

    for _ in 0..5 {
        let client = client.clone();
        let invoice_id = invoice_id;
        let payment_req = payment_req.clone();
        handles.push(tokio::spawn(async move {
            let response = client
                .post(&format!("/api/invoices/{}/pay", invoice_id), &payment_req)
                .await;
            response
        }));
    }

    let results = futures::future::join_all(handles).await;

    let mut success_count = 0;
    for result in results {
        if let Ok(response) = result {
            if response.status().is_success() {
                let body: serde_json::Value = response.json().await.unwrap();
                if body["status"] == "succeeded" {
                    success_count += 1;
                }
            }
        }
    }

    assert!(
        success_count == 1 || success_count == 0,
        "At most one payment should succeed"
    );

    // Verify invoice state
    let invoice = sqlx::query!("SELECT state FROM invoices WHERE id = $1", invoice_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    if success_count == 1 {
        assert_eq!(invoice.state, "paid");
    } else {
        assert_eq!(invoice.state, "open");
    }

    // Verify only one successful payment attempt
    let attempts = sqlx::query!(
        "SELECT status FROM payment_attempts WHERE invoice_id = $1",
        invoice_id
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let successful_attempts = attempts.iter().filter(|a| a.status == "succeeded").count();

    assert!(successful_attempts <= 1, "No double charge");
}
