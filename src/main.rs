use axum::{
    extract::{Extension, FromRef},
    routing::{delete, get, post},
    Router,
};
use dotenvy::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber;

use dodo_payments::{
    handlers::{
        businesses, customers, invoices, payments, webhook_endpoints,
    },
    mock_psp,
    psp_client::{PspClient, PspClientFactory},
    worker,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub psp_client: Arc<dyn PspClient>,
}

// Allow handlers to extract PgPool from AppState.
impl FromRef<AppState> for sqlx::PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

// Allow handlers to extract the PSP client from AppState.
impl FromRef<AppState> for Arc<dyn PspClient> {
    fn from_ref(state: &AppState) -> Self {
        state.psp_client.clone()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    "dodo_payments=info,tower_http=debug".into()
                }),
        )
        .init();

    // ---------------------------------------------------------
    // Database
    // ---------------------------------------------------------

    let database_url =
        std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await?;

    tracing::info!("Database connected and migrations applied");

    // ---------------------------------------------------------
    // Mock PSP client
    // ---------------------------------------------------------

    let psp_base_url = std::env::var("PSP_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3001".to_string());

    let psp_factory = PspClientFactory::new(psp_base_url);

    let psp_client: Arc<dyn PspClient> =
        Arc::from(psp_factory.create());

    // ---------------------------------------------------------
    // Application state
    // ---------------------------------------------------------

    let app_state = AppState {
        pool: pool.clone(),
        psp_client: psp_client.clone(),
    };

    // ---------------------------------------------------------
    // Background worker
    // ---------------------------------------------------------

    let worker_pool = pool.clone();
    let worker_psp = psp_client.clone();

    tokio::spawn(async move {
        worker::run_worker(worker_pool, worker_psp).await;
    });

    // ---------------------------------------------------------
    // Main API
    // ---------------------------------------------------------

    let app = Router::new()
        // Businesses
        .route(
            "/api/businesses",
            post(businesses::create_business),
        )
        .route(
            "/api/businesses/keys",
            get(businesses::list_keys),
        )
        .route(
            "/api/businesses/keys/:key_id",
            delete(businesses::revoke_key),
        )

        // Customers
        .route(
            "/api/customers",
            post(customers::create_customer),
        )
        .route(
            "/api/customers",
            get(customers::list_customers),
        )
        .route(
            "/api/customers/:id",
            get(customers::get_customer),
        )

        // Invoices
        .route(
            "/api/invoices",
            post(invoices::create_invoice),
        )
        .route(
            "/api/invoices",
            get(invoices::list_invoices),
        )
        .route(
            "/api/invoices/:id",
            get(invoices::get_invoice),
        )

        // Payments
        .route(
            "/api/invoices/:id/pay",
            post(payments::process_payment),
        )

        // Webhooks
        .route(
            "/api/webhooks",
            post(webhook_endpoints::register_webhook),
        )
        .route(
            "/api/webhooks",
            get(webhook_endpoints::list_webhooks),
        )
        .route(
            "/api/webhooks/:id",
            delete(webhook_endpoints::delete_webhook),
        )
        .route(
            "/api/invoices/:id/open",
            post(invoices::open_invoice),
        )

        // Middleware
        .layer(TraceLayer::new_for_http())

        // AuthenticatedBusiness in auth.rs reads PgPool
        // from request extensions.
        .layer(Extension(pool.clone()))

        // Application state
        .with_state(app_state);

    // ---------------------------------------------------------
    // Mock PSP server
    // ---------------------------------------------------------

    let psp_app = Router::new()
        .route(
            "/api/psp/payments",
            post(mock_psp::process_mock_payment),
        )
        .route(
            "/health",
            get(mock_psp::health_check),
        );

    // ---------------------------------------------------------
    // Ports
    // ---------------------------------------------------------

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse::<u16>()
        .expect("Invalid PORT");

    let psp_port = std::env::var("PSP_PORT")
        .unwrap_or_else(|_| "3001".to_string())
        .parse::<u16>()
        .expect("Invalid PSP_PORT");

    // ---------------------------------------------------------
    // Listeners
    // ---------------------------------------------------------

    let app_listener =
        tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", port)
        )
        .await?;

    let psp_listener =
        tokio::net::TcpListener::bind(
            format!("0.0.0.0:{}", psp_port)
        )
        .await?;

    tracing::info!(
        "Invoice service listening on port {}",
        port
    );

    tracing::info!(
        "Mock PSP listening on port {}",
        psp_port
    );

    // ---------------------------------------------------------
    // Run both servers
    // ---------------------------------------------------------

    tokio::select! {
        result = axum::serve(app_listener, app) => {
            result?;
        }

        result = axum::serve(psp_listener, psp_app) => {
            result?;
        }
    }

    Ok(())
}

