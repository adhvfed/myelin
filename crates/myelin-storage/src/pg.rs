use sqlx::postgres::PgPool;
use sqlx::Row;

pub const REBAC_TUPLE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS rebac_tuple (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    object_id text NOT NULL,
    relation  text NOT NULL,
    subject   text NOT NULL,
    PRIMARY KEY (tenant_id, region, object_id, relation, subject)
);
CREATE INDEX IF NOT EXISTS rebac_tuple_rev
    ON rebac_tuple (tenant_id, region, subject, relation, object_id);";

#[derive(Debug)]
pub enum PgError {
    Connect(String),
    Migrate(String),
    Query(String),
    Publish(String),
}

impl core::fmt::Display for PgError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PgError::Connect(e) => write!(f, "postgres connect failed: {e}"),
            PgError::Migrate(e) => write!(f, "postgres migration failed: {e}"),
            PgError::Query(e) => write!(f, "postgres query failed: {e}"),
            PgError::Publish(e) => write!(f, "outbox relay publish failed: {e}"),
        }
    }
}

impl std::error::Error for PgError {}

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    region: String,
    authz_queries: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PgStore {
    pub async fn connect(
        database_url: &str,
        region: &str,
        max_connections: u32,
    ) -> Result<PgStore, PgError> {
        if region.trim().is_empty() {
            return Err(PgError::Connect(
                "region pin is empty - refusing to open a region-less OLTP pool (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        let pool = crate::tenant_tx::connect_pool_with_reset(database_url, region, max_connections)
            .await?;
        Ok(PgStore {
            pool,
            region: region.to_string(),
            authz_queries: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    pub fn authz_query_count(&self) -> u64 {
        self.authz_queries.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn health_check(&self) -> Result<(), PgError> {
        use sqlx::Connection;
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        conn.ping()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    fn ensure_region(&self) -> Result<(), PgError> {
        if self.region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty - refusing a region-less tenant-scoped op (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub async fn migrate(&self) -> Result<(), PgError> {
        let migrations = crate::migration::Migrations::of([
            crate::migration::Migration::plain("0001_outbox", myelin_events::OUTBOX_MIGRATION),
            crate::migration::Migration::plain("0002_rebac_tuple", REBAC_TUPLE_MIGRATION),
            crate::migration::Migration::plain(
                "0003_rebac_rls_policy",
                "ALTER TABLE rebac_tuple ENABLE ROW LEVEL SECURITY;\n\
                 ALTER TABLE rebac_tuple FORCE ROW LEVEL SECURITY;\n\
                 DROP POLICY IF EXISTS myelin_tenant_isolation ON rebac_tuple;\n\
                 CREATE POLICY myelin_tenant_isolation ON rebac_tuple \
                   USING (tenant_id = current_setting('myelin.tenant_id', true) \
                          AND region = current_setting('myelin.region', true)) \
                   WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
                               AND region = current_setting('myelin.region', true));",
            ),
        ]);
        crate::pg_migrator::PgMigrator::apply(&self.pool, &migrations).await
    }

    pub async fn put_tuple(
        &self,
        tenant: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        self.ensure_region()?;
        let region = self.region.clone();
        let (tenant_owned, object_id, relation, subject) = (
            tenant.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                Self::insert_tuple_on_conn(
                    conn,
                    &tenant_owned,
                    &region,
                    &object_id,
                    &relation,
                    &subject,
                )
                .await
            })
        })
        .await
    }

    pub async fn put_tuple_in_region(
        &self,
        tenant: &str,
        row_region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        self.ensure_region()?;
        let (tenant_owned, row_region, object_id, relation, subject) = (
            tenant.to_string(),
            row_region.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
                     VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
                )
                .bind(&tenant_owned)
                .bind(&row_region)
                .bind(&object_id)
                .bind(&relation)
                .bind(&subject)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(())
            })
        })
        .await
    }

    pub async fn reverse_index(
        &self,
        tenant: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        self.ensure_region()?;
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tenant_owned, subject, relation) = (
            tenant.to_string(),
            subject.to_string(),
            relation.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT object_id FROM rebac_tuple \
                     WHERE tenant_id = $1 AND subject = $2 AND relation = $3 ORDER BY object_id",
                )
                .bind(&tenant_owned)
                .bind(&subject)
                .bind(&relation)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(rows
                    .iter()
                    .map(|r| r.get::<String, _>("object_id"))
                    .collect())
            })
        })
        .await
    }

    pub async fn check_tuple(
        &self,
        tenant: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<bool, PgError> {
        self.ensure_region()?;
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tenant_owned, object_id, relation, subject) = (
            tenant.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM rebac_tuple \
                     WHERE tenant_id = $1 AND object_id = $2 AND relation = $3 AND subject = $4)",
                )
                .bind(&tenant_owned)
                .bind(&object_id)
                .bind(&relation)
                .bind(&subject)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(exists)
            })
        })
        .await
    }

    pub async fn list_objects(
        &self,
        tenant: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        self.reverse_index(tenant, subject, relation).await
    }

    pub async fn scoped_conn(
        &self,
        acting_tenant: &str,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, PgError> {
        self.scoped_conn_in_region(acting_tenant, &self.region)
            .await
    }

    pub async fn scoped_conn_in_region(
        &self,
        acting_tenant: &str,
        region: &str,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, PgError> {
        if region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty - refusing a region-less tenant-scoped transaction \
                 (residency fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(format!("begin tenant-scoped transaction: {e}")))?;
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
        )
        .bind(acting_tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(format!("set transaction-scoped tenant GUC: {e}")))?;
        Ok(tx)
    }

    pub async fn reverse_index_in_region(
        &self,
        tenant: &str,
        region: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        if region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty - refusing a region-less tenant-scoped read (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tenant_owned, subject, relation) = (
            tenant.to_string(),
            subject.to_string(),
            relation.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT object_id FROM rebac_tuple \
                     WHERE tenant_id = $1 AND subject = $2 AND relation = $3 ORDER BY object_id",
                )
                .bind(&tenant_owned)
                .bind(&subject)
                .bind(&relation)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(rows
                    .iter()
                    .map(|r| r.get::<String, _>("object_id"))
                    .collect())
            })
        })
        .await
    }

    pub fn relay(&self) -> crate::pgrelay::PgRelay {
        crate::pgrelay::PgRelay::new(self.pool.clone())
    }

    pub async fn insert_tuple_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(region)
        .bind(object_id)
        .bind(relation)
        .bind(subject)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    pub async fn delete_tuple_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        sqlx::query(
            "DELETE FROM rebac_tuple \
             WHERE tenant_id = $1 AND region = $2 AND object_id = $3 AND relation = $4 \
               AND subject = $5",
        )
        .bind(tenant)
        .bind(region)
        .bind(object_id)
        .bind(relation)
        .bind(subject)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    pub async fn tuples_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
    ) -> Result<Vec<(String, String, String)>, PgError> {
        let rows = sqlx::query(
            "SELECT object_id, relation, subject FROM rebac_tuple \
             WHERE tenant_id = $1 AND region = $2 ORDER BY object_id, relation, subject",
        )
        .bind(tenant)
        .bind(region)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<String, _>("object_id"),
                    r.get::<String, _>("relation"),
                    r.get::<String, _>("subject"),
                )
            })
            .collect())
    }
}
