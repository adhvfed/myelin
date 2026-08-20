#![allow(dead_code)]

use futures::FutureExt;
use myelin_config::MyelinConfig;
use myelin_storage::SubstrateProvider;
use sqlx::postgres::{PgPool, PgPoolOptions};

pub fn app_database_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

pub fn admin_database_config() -> MyelinConfig {
    let mut config = app_database_config();
    let admin_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config.database_url = admin_url.clone();
    config.database_migration_url = admin_url;
    config
}

pub async fn app_provider(max_connections: u32) -> SubstrateProvider {
    SubstrateProvider::connect(app_database_config(), max_connections)
        .await
        .expect("storage integration tests require the configured Postgres backend")
}

pub async fn admin_provider(max_connections: u32) -> SubstrateProvider {
    SubstrateProvider::connect(admin_database_config(), max_connections)
        .await
        .expect("storage integration tests require the configured admin Postgres backend")
}

pub async fn admin_pool(max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(&admin_database_config().database_url)
        .await
        .expect("storage integration tests require the configured admin Postgres backend")
}

pub async fn with_cleanup<BodyFut, CleanupFut>(
    body: impl FnOnce() -> BodyFut,
    cleanup: impl FnOnce() -> CleanupFut,
) where
    BodyFut: std::future::Future<Output = ()>,
    CleanupFut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    cleanup().await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
