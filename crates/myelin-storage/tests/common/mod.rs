#![cfg(feature = "integration")]

use futures::FutureExt;

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

#[allow(dead_code)]
pub async fn delete_outbox_for_aggregate(pool: &sqlx::PgPool, aggregate: &str) {
    for _ in 0..5 {
        let _ = sqlx::query("DELETE FROM outbox_quarantine WHERE aggregate = $1")
            .bind(aggregate)
            .execute(pool)
            .await;
        if sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
            .bind(aggregate)
            .execute(pool)
            .await
            .is_ok()
        {
            return;
        }
    }
}
