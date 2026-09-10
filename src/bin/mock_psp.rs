use axum::{
    routing::{get, post},
    Router,
};
use dotenvy::dotenv;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber;

use dodo_payments::mock_psp;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dodo_payments=info".into()),
        )
        .init();

    let app = Router::new()
        .route("/api/psp/payments", post(mock_psp::process_mock_payment))
        .route("/health", get(mock_psp::health_check))
        .layer(TraceLayer::new_for_http());

    let port = std::env::var("PSP_PORT")
        .unwrap_or_else(|_| "3001".to_string())
        .parse::<u16>()
        .expect("Invalid PSP_PORT");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Mock PSP listening on {}", addr);

    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}
