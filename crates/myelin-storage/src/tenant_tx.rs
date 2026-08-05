//! ## `residency-pin` lint — region pinned OUT-OF-BAND (`@residency-cell-pinned:file`)
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Executor, PgConnection};

use crate::pg::PgError;

pub type TxScope<'c, R> = Pin<Box<dyn Future<Output = Result<R, PgError>> + Send + 'c>>;

pub type TypedTxScope<'c, R, E> = Pin<Box<dyn Future<Output = Result<R, E>> + Send + 'c>>;

pub async fn with_tenant_tx_error<R, F, E>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, E>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TypedTxScope<'c, R, E> + Send,
    R: Send,
    E: From<PgError>,
{
    let mut tx = pool.begin().await.map_err(|e| {
        E::from(PgError::Query(format!(
            "begin tenant-scoped transaction: {e}"
        )))
    })?;

    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        E::from(PgError::Query(format!(
            "set transaction-scoped tenant GUC: {e}"
        )))
    })?;

    let out = op(&mut tx).await?;
    tx.commit().await.map_err(|e| {
        E::from(PgError::Query(format!(
            "commit tenant-scoped transaction: {e}"
        )))
    })?;
    Ok(out)
}

pub async fn with_tenant_tx<R, F>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, PgError>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TxScope<'c, R> + Send,
    R: Send,
{
    with_tenant_tx_error(pool, tenant, region, op).await
}

pub async fn with_tenant_repeatable_read_tx<R, F>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, PgError>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TxScope<'c, R> + Send,
    R: Send,
{
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| PgError::Query(format!("begin tenant-scoped read transaction: {e}")))?;
    // @tenant-cross-scope: configures the transaction snapshot before the tenant GUC is installed.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(format!("set tenant read snapshot isolation: {e}")))?;
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        PgError::Query(format!(
            "set repeatable-read transaction-scoped tenant GUC: {e}"
        ))
    })?;
    let out = op(&mut tx).await?;
    tx.commit()
        .await
        .map_err(|e| PgError::Query(format!("commit tenant-scoped read transaction: {e}")))?;
    Ok(out)
}

pub async fn connect_pool_with_reset(
    database_url: &str,
    region: &str,
    max_connections: u32,
) -> Result<PgPool, PgError> {
    let opts = PgConnectOptions::from_str(database_url)
        .map_err(|e| PgError::Connect(e.to_string()))?
        .application_name(&format!("myelin:{region}"));
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .after_release(|conn, _meta| {
            Box::pin(async move {
                conn.execute("RESET ALL").await?;
                Ok(true)
            })
        })
        .connect_with(opts)
        .await
        .map_err(|e| PgError::Connect(e.to_string()))
}
