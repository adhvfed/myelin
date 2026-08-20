use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::backup::{wal_offset_from_bigint, wal_offset_to_bigint, WalOffset};
use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};

pub const RESTORE_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS restore_erasure_ledger (
    tenant_id            text   NOT NULL,
    region               text   NOT NULL,
    completed_at_offset  bigint NOT NULL,
    PRIMARY KEY (tenant_id, region)
);";

pub const RESTORE_WAL_OFFSET_INVARIANTS_EXPAND_MIGRATION: &str = "\
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conrelid = 'restore_erasure_ledger'::regclass
           AND conname = 'restore_erasure_ledger_offset_nonnegative'
    ) THEN
        ALTER TABLE restore_erasure_ledger
            ADD CONSTRAINT restore_erasure_ledger_offset_nonnegative
            CHECK (completed_at_offset >= 0) NOT VALID;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conrelid = 'post_pit_erasure_ledger'::regclass
           AND conname = 'post_pit_erasure_ledger_offset_nonnegative'
    ) THEN
        ALTER TABLE post_pit_erasure_ledger
            ADD CONSTRAINT post_pit_erasure_ledger_offset_nonnegative
            CHECK (completed_at_offset >= 0) NOT VALID;
    END IF;
END
$$;";

pub const RESTORE_WAL_OFFSET_INVARIANTS_VALIDATE_MIGRATION: &str = "\
ALTER TABLE restore_erasure_ledger
    VALIDATE CONSTRAINT restore_erasure_ledger_offset_nonnegative;
ALTER TABLE post_pit_erasure_ledger
    VALIDATE CONSTRAINT post_pit_erasure_ledger_offset_nonnegative;";

pub fn restore_verify_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0051_restore_erasure_ledger",
        RESTORE_ERASURE_LEDGER_MIGRATION,
    )])
}

pub fn restore_wal_offset_invariant_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0122_restore_wal_offset_invariants_expand",
            RESTORE_WAL_OFFSET_INVARIANTS_EXPAND_MIGRATION,
        ),
        Migration::plain(
            "0123_restore_wal_offset_invariants_validate",
            RESTORE_WAL_OFFSET_INVARIANTS_VALIDATE_MIGRATION,
        ),
    ])
}

#[derive(Clone)]
pub struct DurableRestoreErasureLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurableRestoreErasureLedger {
    pub fn new(provider: SubstrateProvider) -> DurableRestoreErasureLedger {
        DurableRestoreErasureLedger {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn record_async(
        &self,
        tenant: &TenantId,
        completed_at_offset: WalOffset,
    ) -> Result<(), ProviderError> {
        let tenant = tenant.0.clone();
        let region = self.region();
        let offset = wal_offset_to_bigint(completed_at_offset).ok_or_else(|| {
            ProviderError::from(crate::pg::PgError::Query(
                "restore erasure offset exceeds the PostgreSQL bigint range".into(),
            ))
        })?;
        sqlx::query(
            "INSERT INTO restore_erasure_ledger (tenant_id, region, completed_at_offset) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (tenant_id, region) DO UPDATE SET \
               completed_at_offset = GREATEST( \
                   restore_erasure_ledger.completed_at_offset, \
                   EXCLUDED.completed_at_offset \
               )",
        )
        .bind(&tenant)
        .bind(&region)
        .bind(offset)
        .execute(self.provider.db_pool())
        .await
        .map_err(|e| ProviderError::from(crate::pg::PgError::Query(e.to_string())))?;
        Ok(())
    }

    pub(crate) fn record_erased_at(&self, tenant: &TenantId, completed_at_offset: WalOffset) {
        tokio::task::block_in_place(|| {
            self.rt
                .block_on(self.record_async(tenant, completed_at_offset))
        })
        .unwrap_or_else(|e| {
            panic!(
                "FAIL-STATIC: durable restore-verify erasure ledger write failed (an unrecorded \
                 erasure lets the restore-verify gate GREEN a resurrected subject): {e}"
            )
        });
    }

    async fn records_async(&self) -> Result<Vec<(TenantId, WalOffset)>, ProviderError> {
        let region = self.region();
        let rows = sqlx::query(
            "SELECT tenant_id, completed_at_offset FROM restore_erasure_ledger \
             WHERE region = $1 ORDER BY tenant_id",
        )
        .bind(&region)
        .fetch_all(self.provider.db_pool())
        .await
        .map_err(|e| ProviderError::from(crate::pg::PgError::Query(e.to_string())))?;
        rows.iter()
            .map(|row| {
                let tenant: String = row
                    .try_get("tenant_id")
                    .map_err(restore_erasure_row_decode)?;
                let offset: i64 = row
                    .try_get("completed_at_offset")
                    .map_err(restore_erasure_row_decode)?;
                let offset = wal_offset_from_bigint(offset).ok_or_else(|| {
                    ProviderError::from(crate::pg::PgError::Query(
                        "restore erasure ledger contains a negative WAL offset".into(),
                    ))
                })?;
                Ok((TenantId(tenant), offset))
            })
            .collect()
    }

    pub(crate) fn records(&self) -> Vec<(TenantId, WalOffset)> {
        tokio::task::block_in_place(|| self.rt.block_on(self.records_async())).unwrap_or_else(|e| {
            panic!(
                "FAIL-STATIC: durable restore-verify erasure ledger read failed (an incomplete \
                     erased set would let the gate green a resurrected subject): {e}"
            )
        })
    }
}

fn restore_erasure_row_decode(error: sqlx::Error) -> ProviderError {
    ProviderError::from(crate::pg::PgError::Query(format!(
        "restore erasure ledger row decode failed: {error}"
    )))
}
