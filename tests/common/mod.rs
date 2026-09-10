use sqlx::PgPool;
use uuid::Uuid;

pub async fn setup_test_db() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for tests");
    
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to test database");
    
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");
    
    pool
}

pub async fn create_test_business(pool: &PgPool) -> (Uuid, String) {
    use dodo_payments::auth::{generate_api_key, store_api_key};
    
    let business = sqlx::query!(
        "INSERT INTO businesses (name) VALUES ($1) RETURNING id",
        "Test Business"
    )
    .fetch_one(pool)
    .await
    .expect("Failed to create test business");

    let (full_key, hash) = generate_api_key(business.id);
    let (prefix, _) = full_key.split_once('_').unwrap();
    
    store_api_key(pool, business.id, prefix, &hash)
        .await
        .expect("Failed to store API key");

    (business.id, full_key)
}

pub async fn create_test_customer(pool: &PgPool, business_id: Uuid) -> Uuid {
    let customer = sqlx::query!(
        "INSERT INTO customers (business_id, name, email) VALUES ($1, $2, $3) RETURNING id",
        business_id,
        "Test Customer",
        "test@example.com"
    )
    .fetch_one(pool)
    .await
    .expect("Failed to create test customer");

    customer.id
}

pub async fn create_test_invoice(pool: &PgPool, business_id: Uuid, customer_id: Uuid) -> Uuid {
    use chrono::Utc;
    let due_date = Utc::now().naive_utc().date() + chrono::Duration::days(30);
    
    let invoice = sqlx::query!(
        "INSERT INTO invoices (business_id, customer_id, total_cents, due_date, state)
         VALUES ($1, $2, $3, $4, 'open') RETURNING id",
        business_id,
        customer_id,
        10000,
        due_date
    )
    .fetch_one(pool)
    .await
    .expect("Failed to create test invoice");

    invoice.id
}

pub struct TestClient {
    pub base_url: String,
    pub api_key: String,
}

impl TestClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    pub async fn post<T: serde::Serialize>(&self, path: &str, body: &T) -> reqwest::Response {
        let client = reqwest::Client::new();
        client
            .post(&format!("{}{}", self.base_url, path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .expect("Failed to send request")
    }
}