//! DB-free shutdown proof for the production dead-runner reaper.

use std::time::Duration;

use myelin_ci_controlplane::{ci_region_queue_store_test_support, JobQueueReaper};
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn reaper_observes_pre_signalled_shutdown_without_database_access() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("syntactically valid lazy pool");
    let reaper = JobQueueReaper::new(
        ci_region_queue_store_test_support(pool),
        "fr-par",
        Duration::from_secs(60),
    );
    let (_shutdown, receiver) = tokio::sync::watch::channel(true);
    reaper.run_until_shutdown(receiver).await;
}
