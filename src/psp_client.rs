use crate::error::{AppError, Result};
use crate::models::{PspPaymentRequest, PspPaymentResponse, PspStatus};
use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;
use uuid::Uuid;

#[async_trait]
pub trait PspClient: Send + Sync {
    async fn process_payment(
        &self,
        card_token: &str,
        amount_cents: i32,
        idempotency_key: &str,
    ) -> Result<PspPaymentResponse>;
}

pub struct MockPspClient {
    client: Client,
    base_url: String,
    timeout: Duration,
}

impl MockPspClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(35))
                .build()
                .expect("failed to build http client"),
            base_url,
            timeout: Duration::from_secs(35),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl PspClient for MockPspClient {
    async fn process_payment(
        &self,
        card_token: &str,
        amount_cents: i32,
        idempotency_key: &str,
    ) -> Result<PspPaymentResponse> {
        let url = format!("{}/api/psp/payments", self.base_url);
        let request = PspPaymentRequest {
            card_token: card_token.to_string(),
            amount_cents,
            currency: "USD".to_string(),
        };

        let response = self
            .client
            .post(&url)
            .header("Idempotency-Key", idempotency_key)
            .json(&request)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::PspError("PSP request timed out".to_string())
                } else if e.is_connect() || e.is_request() {
                    AppError::PspError("PSP network error".to_string())
                } else {
                    AppError::PspError(format!("PSP communication error: {}", e))
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            
            if status.as_u16() >= 500 {
                return Err(AppError::PspError(format!("PSP server error: {}", error_text)));
            }
            
            return Err(AppError::PspError(format!("PSP error {}: {}", status, error_text)));
        }

        let psp_response: PspPaymentResponse = response
            .json()
            .await
            .map_err(|e| AppError::PspError(format!("PSP response parsing error: {}", e)))?;

        // Validate PSP response
        match psp_response.status {
            PspStatus::succeeded => {
                if psp_response.psp_ref.is_none() {
                    return Err(AppError::PspError("PSP success response missing reference".to_string()));
                }
            }
            PspStatus::failed => {
                if psp_response.code.is_none() {
                    return Err(AppError::PspError("PSP failure response missing error code".to_string()));
                }
            }
        }

        Ok(psp_response)
    }
}

pub struct PspClientFactory {
    base_url: String,
}

impl PspClientFactory {
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }

    pub fn create(&self) -> Box<dyn PspClient> {
        Box::new(MockPspClient::new(self.base_url.clone()))
    }
}