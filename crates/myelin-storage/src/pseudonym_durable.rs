use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};

pub const PSEUDONYM_MAP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS pseudonym_map (
    tenant_id        text  NOT NULL,
    region           text  NOT NULL,
    principal_id     text  NOT NULL,
    pseudonym_render text  NOT NULL,
    real_id_key_ref  text  NOT NULL,
    nonce            bytea NOT NULL,
    ciphertext       bytea NOT NULL,
    PRIMARY KEY (tenant_id, region, principal_id)
);
CREATE INDEX IF NOT EXISTS pseudonym_map_reverse \
  ON pseudonym_map (tenant_id, region, pseudonym_render);";

pub const PSEUDONYM_MAP_RLS_POLICY: &str = "\
ALTER TABLE pseudonym_map ENABLE ROW LEVEL SECURITY;
ALTER TABLE pseudonym_map FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON pseudonym_map;
CREATE POLICY myelin_tenant_isolation ON pseudonym_map \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const PSEUDONYM_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS identity_pseudonym_erasure_ledger (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    subject   text NOT NULL,
    dek_class text NOT NULL,
    erased_at text NOT NULL,
    PRIMARY KEY (tenant_id, region, subject)
);";

pub fn pseudonym_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0020_pseudonym_map", PSEUDONYM_MAP_MIGRATION),
        Migration::plain("0021_pseudonym_map_rls", PSEUDONYM_MAP_RLS_POLICY),
        Migration::plain(
            "0022_identity_pseudonym_erasure_ledger",
            PSEUDONYM_ERASURE_LEDGER_MIGRATION,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePseudonymRow {
    pub principal_id: String,
    pub pseudonym_render: String,
    pub real_id_key_ref: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

#[derive(Clone)]
pub struct DurablePseudonymBacking {
    provider: SubstrateProvider,
}

impl DurablePseudonymBacking {
    pub fn new(provider: SubstrateProvider) -> DurablePseudonymBacking {
        DurablePseudonymBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn put_mapping(
        &self,
        tenant: &str,
        row: DurablePseudonymRow,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO pseudonym_map \
                           (tenant_id, region, principal_id, pseudonym_render, \
                            real_id_key_ref, nonce, ciphertext) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7) \
                         ON CONFLICT (tenant_id, region, principal_id) DO UPDATE SET \
                           pseudonym_render = EXCLUDED.pseudonym_render, \
                           real_id_key_ref = EXCLUDED.real_id_key_ref, \
                           nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&row.principal_id)
                    .bind(&row.pseudonym_render)
                    .bind(&row.real_id_key_ref)
                    .bind(&row.nonce)
                    .bind(&row.ciphertext)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn get_by_principal(
        &self,
        tenant: &str,
        principal_id: &str,
    ) -> Result<Option<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(row.map(|r| row_to_pseudonym(&r)))
                })
            })
            .await
    }

    pub async fn get_by_pseudonym(
        &self,
        tenant: &str,
        pseudonym_render: &str,
    ) -> Result<Option<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let rendering = pseudonym_render.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND pseudonym_render = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&rendering)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(row.map(|r| row_to_pseudonym(&r)))
                })
            })
            .await
    }

    pub async fn mappings_in(
        &self,
        tenant: &str,
    ) -> Result<Vec<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map WHERE tenant_id = $1 AND region = $2 \
                         ORDER BY principal_id",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(rows.iter().map(row_to_pseudonym).collect::<Vec<_>>())
                })
            })
            .await
    }

    pub async fn shred(&self, tenant: &str, principal_id: &str) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let result = sqlx::query(
                        "DELETE FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(result.rows_affected() > 0)
                })
            })
            .await
    }
}

fn row_to_pseudonym(r: &sqlx::postgres::PgRow) -> DurablePseudonymRow {
    DurablePseudonymRow {
        principal_id: r.get("principal_id"),
        pseudonym_render: r.get("pseudonym_render"),
        real_id_key_ref: r.get("real_id_key_ref"),
        nonce: r.get("nonce"),
        ciphertext: r.get("ciphertext"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableErasureLedgerRow {
    pub subject: String,
    pub dek_class: String,
    pub erased_at: String,
}

#[derive(Clone)]
pub struct DurableErasureLedgerBacking {
    provider: SubstrateProvider,
}

impl DurableErasureLedgerBacking {
    pub fn new(provider: SubstrateProvider) -> DurableErasureLedgerBacking {
        DurableErasureLedgerBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn record(
        &self,
        tenant: &str,
        subject: &str,
        dek_class: &str,
        erased_at: &str,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let subject = subject.to_string();
        let dek_class = dek_class.to_string();
        let erased_at = erased_at.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO identity_pseudonym_erasure_ledger \
                           (tenant_id, region, subject, dek_class, erased_at) \
                         VALUES ($1, $2, $3, $4, $5) \
                         ON CONFLICT (tenant_id, region, subject) DO UPDATE SET \
                           dek_class = EXCLUDED.dek_class, erased_at = EXCLUDED.erased_at",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject)
                    .bind(&dek_class)
                    .bind(&erased_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn entries_in(
        &self,
        tenant: &str,
    ) -> Result<Vec<DurableErasureLedgerRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT subject, dek_class, erased_at \
                         FROM identity_pseudonym_erasure_ledger \
                         WHERE tenant_id = $1 AND region = $2 ORDER BY subject",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(rows
                        .iter()
                        .map(|r| DurableErasureLedgerRow {
                            subject: r.get("subject"),
                            dek_class: r.get("dek_class"),
                            erased_at: r.get("erased_at"),
                        })
                        .collect::<Vec<_>>())
                })
            })
            .await
    }

    pub async fn is_erased(&self, tenant: &str, subject: &str) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let subject = subject.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM identity_pseudonym_erasure_ledger \
                         WHERE tenant_id = $1 AND region = $2 AND subject = $3)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(exists)
                })
            })
            .await
    }
}
