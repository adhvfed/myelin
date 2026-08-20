use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::backup::{wal_offset_from_bigint, wal_offset_to_bigint, WalOffset};
use crate::encryption::SubjectId;
use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};
use crate::reerase::{ErasureRecord, PostRestoreErasureLedger};

pub const POST_PIT_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS post_pit_erasure_ledger (
    tenant_id            text   NOT NULL,
    region               text   NOT NULL,
    subject              text   NOT NULL,
    completed_at_offset  bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, subject)
);";

pub fn post_pit_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0052_post_pit_erasure_ledger",
        POST_PIT_ERASURE_LEDGER_MIGRATION,
    )])
}

#[derive(Clone)]
pub struct DurablePostPitLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurablePostPitLedger {
    pub fn new(provider: SubstrateProvider) -> DurablePostPitLedger {
        DurablePostPitLedger {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn record(
        &self,
        tenant: &TenantId,
        subject: &SubjectId,
        completed_at_offset: WalOffset,
    ) -> Result<(), ProviderError> {
        let tenant = tenant.0.clone();
        let region = self.region();
        let subject = subject.0.clone();
        let offset = wal_offset_to_bigint(completed_at_offset).ok_or_else(|| {
            ProviderError::from(crate::pg::PgError::Query(
                "post-PIT erasure offset exceeds the PostgreSQL bigint range".into(),
            ))
        })?;
        sqlx::query(
            "INSERT INTO post_pit_erasure_ledger \
               (tenant_id, region, subject, completed_at_offset) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (tenant_id, region, subject) DO UPDATE SET \
               completed_at_offset = GREATEST( \
                   post_pit_erasure_ledger.completed_at_offset, \
                   EXCLUDED.completed_at_offset \
               )",
        )
        .bind(&tenant)
        .bind(&region)
        .bind(&subject)
        .bind(offset)
        .execute(self.provider.db_pool())
        .await
        .map_err(|e| ProviderError::from(crate::pg::PgError::Query(e.to_string())))?;
        Ok(())
    }

    async fn completed_after_async(
        &self,
        pit: WalOffset,
    ) -> Result<Vec<ErasureRecord>, ProviderError> {
        let region = self.region();
        let pit_i = wal_offset_to_bigint(pit).ok_or_else(|| {
            ProviderError::from(crate::pg::PgError::Query(
                "post-PIT restore target exceeds the PostgreSQL bigint range".into(),
            ))
        })?;
        let rows = sqlx::query(
            "SELECT tenant_id, subject, completed_at_offset \
             FROM post_pit_erasure_ledger \
             WHERE region = $1 AND completed_at_offset > $2 \
             ORDER BY tenant_id, subject",
        )
        .bind(&region)
        .bind(pit_i)
        .fetch_all(self.provider.db_pool())
        .await
        .map_err(|e| ProviderError::from(crate::pg::PgError::Query(e.to_string())))?;
        rows.iter()
            .map(|row| {
                let tenant: String = row.try_get("tenant_id").map_err(post_pit_row_decode)?;
                let subject: String = row.try_get("subject").map_err(post_pit_row_decode)?;
                let offset: i64 = row
                    .try_get("completed_at_offset")
                    .map_err(post_pit_row_decode)?;
                let offset = wal_offset_from_bigint(offset).ok_or_else(|| {
                    ProviderError::from(crate::pg::PgError::Query(
                        "post-PIT erasure ledger contains a negative WAL offset".into(),
                    ))
                })?;
                Ok(ErasureRecord::new(
                    SubjectId::new(subject),
                    TenantId(tenant),
                    offset,
                ))
            })
            .collect()
    }
}

fn post_pit_row_decode(error: sqlx::Error) -> ProviderError {
    ProviderError::from(crate::pg::PgError::Query(format!(
        "post-PIT erasure ledger row decode failed: {error}"
    )))
}

impl PostRestoreErasureLedger for DurablePostPitLedger {
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord> {
        tokio::task::block_in_place(|| self.rt.block_on(self.completed_after_async(pit)))
            .unwrap_or_else(|e| {
                panic!(
                    "FAIL-STATIC: durable post-PIT erasure ledger read failed (an incomplete \
                     post-PIT set would let a post-backup-erased subject escape the §7.5 re-erasure \
                     pass - a silent resurrection): {e}"
                )
            })
    }
}
