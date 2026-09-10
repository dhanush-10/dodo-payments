use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct MockPspPaymentRequest {
    pub card_token: String,
    pub amount_cents: i32,
    pub currency: String,
}

#[derive(Debug, Serialize)]
pub struct MockPspPaymentResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psp_ref: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

pub async fn process_mock_payment(
    Json(req): Json<MockPspPaymentRequest>,
) -> impl IntoResponse {
    match req.card_token.as_str() {
        "tok_success" => {
            sleep(Duration::from_millis(100)).await;
            (
                StatusCode::OK,
                Json(MockPspPaymentResponse {
                    status: "succeeded".to_string(),
                    psp_ref: Some(Uuid::new_v4()),
                    code: None,
                }),
            )
        }
        "tok_insufficient_funds" => {
            sleep(Duration::from_millis(100)).await;
            (
                StatusCode::OK,
                Json(MockPspPaymentResponse {
                    status: "failed".to_string(),
                    psp_ref: None,
                    code: Some("insufficient_funds".to_string()),
                }),
            )
        }
        "tok_card_declined" => {
            sleep(Duration::from_millis(100)).await;
            (
                StatusCode::OK,
                Json(MockPspPaymentResponse {
                    status: "failed".to_string(),
                    psp_ref: None,
                    code: Some("card_declined".to_string()),
                }),
            )
        }
        "tok_timeout" => {
            sleep(Duration::from_secs(30)).await;
            (
                StatusCode::OK,
                Json(MockPspPaymentResponse {
                    status: "succeeded".to_string(),
                    psp_ref: Some(Uuid::new_v4()),
                    code: None,
                }),
            )
        }
        "tok_network_error" => {
            sleep(Duration::from_millis(100)).await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(MockPspPaymentResponse {
                    status: "failed".to_string(),
                    psp_ref: None,
                    code: Some("network_error".to_string()),
                }),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(MockPspPaymentResponse {
                status: "failed".to_string(),
                psp_ref: None,
                code: Some("invalid_token".to_string()),
            }),
        ),
    }
}

pub async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}