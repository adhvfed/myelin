#![cfg(feature = "integration")]

use futures::FutureExt;
use myelin_config::MyelinConfig;

pub fn admin_database_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
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
