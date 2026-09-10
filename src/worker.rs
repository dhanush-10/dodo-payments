use crate::psp_client::PspClient;
use crate::webhooks;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time;
use tracing::{error, info};

pub struct Worker {
    pool: PgPool,
    psp_client: Arc<dyn PspClient>,
    webhook_interval: time::Duration,
}

impl Worker {
    pub fn new(pool: PgPool, psp_client: Arc<dyn PspClient>) -> Self {
        Self {
            pool,
            psp_client,
            webhook_interval: time::Duration::from_secs(5),
        }
    }

    pub async fn run(&self) -> ! {
        info!("Worker started");

        let mut webhook_interval = time::interval(self.webhook_interval);

        loop {
            tokio::select! {
                _ = webhook_interval.tick() => {
                    if let Err(e) = webhooks::process_pending_webhooks(&self.pool).await {
                        error!("Failed to process webhooks: {}", e);
                    }
                }
            }
        }
    }
}

pub async fn run_worker(pool: PgPool, psp_client: Arc<dyn PspClient>) -> ! {
    let worker = Worker::new(pool, psp_client);
    worker.run().await
}