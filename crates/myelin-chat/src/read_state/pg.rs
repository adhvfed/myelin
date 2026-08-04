use sqlx::postgres::PgPool;

use super::ReadMarker;
use crate::store::{ConversationId, MessageId, StoreError};

pub const READ_STATE_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS {table} (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    conversation_id text        NOT NULL,
    principal       text        NOT NULL,
    last_read       text        NOT NULL,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, conversation_id, principal)
);";

#[derive(Clone)]
pub struct PgReadStateRecord {
    pool: PgPool,
    region: String,
    table: String,
}

impl PgReadStateRecord {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        table: impl Into<String>,
    ) -> PgReadStateRecord {
        PgReadStateRecord {
            pool,
            region: region.into(),
            table: table.into(),
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        let ddl = READ_STATE_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("read_state DDL: {e}")))?;
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{}')",
            self.table
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {} TO myelin_app", self.table))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("grant: {e}")))?;
        Ok(())
    }

    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
        region: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(region)
            .execute(&mut **conn)
            .await
            .map_err(|e| StoreError::Cold(format!("set session scope: {e}")))?;
        Ok(())
    }

    pub async fn upsert(&self, marker: &ReadMarker) -> Result<MessageId, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &marker.conv.tenant, &marker.conv.region)
            .await?;
        let persisted: String = sqlx::query_scalar(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, principal, last_read) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (tenant_id, region, conversation_id, principal) \
             DO UPDATE SET last_read = GREATEST({}.last_read, EXCLUDED.last_read), updated_at = now() \
             RETURNING last_read",
            self.table, self.table
        ))
        .bind(&marker.conv.tenant)
        .bind(&marker.conv.region)
        .bind(&marker.conv.conversation_id)
        .bind(&marker.principal)
        .bind(marker.last_read.as_str())
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("upsert read_state: {e}")))?;
        Ok(MessageId(persisted))
    }

    pub async fn load(
        &self,
        conv: &ConversationId,
        principal: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let got: Option<String> = sqlx::query_scalar(&format!(
            "SELECT last_read FROM {} WHERE tenant_id = $1 AND region = $2 \
             AND conversation_id = $3 AND principal = $4",
            self.table
        ))
        .bind(&conv.tenant)
        .bind(&conv.region)
        .bind(&conv.conversation_id)
        .bind(principal)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("load read_state: {e}")))?;
        Ok(got.map(MessageId))
    }

    pub async fn purge_principal(&self, tenant: &str, principal: &str) -> Result<u64, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let res = sqlx::query(&format!(
            "DELETE FROM {} WHERE tenant_id = $1 AND principal = $2",
            self.table
        ))
        .bind(tenant)
        .bind(principal)
        .execute(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("purge read_state: {e}")))?;
        Ok(res.rows_affected())
    }
}
