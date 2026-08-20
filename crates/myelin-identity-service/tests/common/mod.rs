#![allow(dead_code)]

use myelin_config::MyelinConfig;
use myelin_storage::SubstrateProvider;

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
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

pub async fn app_provider(max_connections: u32) -> SubstrateProvider {
    SubstrateProvider::connect(app_database_config(), max_connections)
        .await
        .expect("identity integration tests require the configured Postgres backend")
}

pub async fn admin_provider(max_connections: u32) -> SubstrateProvider {
    SubstrateProvider::connect(admin_database_config(), max_connections)
        .await
        .expect("identity integration tests require the configured Postgres backend")
}
