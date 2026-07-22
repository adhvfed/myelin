//! # Durable PG backing for the restore-verify erasure ledger (MR-009b W6b / P-ST-13, §7.4/§7.6)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/storage.md` §7.4
//! (the restore-verify gate's *erasure-held* leg: a subject erased BEFORE the backup is still erased
//! after restore) + **§7.6 (the backup-window-vs-erasure-SLA residual, `[OPEN → LEGAL]`)**. This
//! module is the REAL durable backing behind [`crate::restore_verify::ErasureLedger`]: the in-memory
//! `BTreeSet<TenantId>` (which held NO timestamp) is now the `test-support`-gated TEST DOUBLE arm, and
//! [`DurableRestoreErasureLedger`] is the always-compiled production ledger.
//!
//! ## The R1 fold-in — recording COMPLETION time (the §7.6 residual, ledger 14)
//! The old ledger recorded only *which* tenants were erased, not *when*. That cannot express the §7.6
//! residual: a backup taken BEFORE an erasure completed physically holds the pre-erasure key, so a
//! restore of that PIT can resurrect the subject even though the logical shred "reaches backups". The
//! durable table therefore records the erasure's **completion offset** (`completed_at_offset` — the
//! §7.3 cross-seam cursor, the SAME `WalOffset` a restore lands at, matching
//! [`crate::reerase::ErasureRecord::completed_at_offset`]). The gate compares the restore PIT vs each
//! erasure's completion offset: an erasure completed AFTER the PIT (inside the backup window) is the
//! §7.6 resurrection risk the gate now CATCHES (see [`crate::restore_verify::RestoreVerifyGate::run`]).
//!
//! ## The table (`restore_erasure_ledger`, migration `0051`) — NON-shred-erasable, NO RLS
//! Keyed `(tenant_id, region)`; holds the OPAQUE tenant id + the erasure completion offset — PII-free,
//! **NON-shred-erasable + NO RLS** (mirrors the W6a `identity_pseudonym_erasure_ledger`): it must
//! survive the crypto-shred it records AND a restore so the gate can still assert erasure-held. The
//! restore gate reads the whole region's erased set (a cell-scoped, cross-tenant restore verification),
//! so the durable read is region-scoped with an explicit `region` predicate (a NAMED tenant-predicate
//! exclusion, like `placement_durable`).

use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::backup::WalOffset;
use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};

// =================================================================================================
// Migration 0051 — the PII-free, NON-shred-erasable restore-verify erasure ledger (NO RLS).
// =================================================================================================

/// The `restore_erasure_ledger` table (storage.md §7.4/§7.6) — `(tenant, region)`-keyed. Holds the
/// OPAQUE tenant id + the erasure's **completion offset** (`completed_at_offset`, the §7.3 cross-seam
/// cursor the restore PIT is compared against — the §7.6 residual lever). **NON-shred-erasable + NO
/// RLS by construction:** it must SURVIVE the key destruction it records AND a restore so the gate can
/// still assert erasure-held. Forward-only (`IF NOT EXISTS`); idempotent upsert on `(tenant, region)`.
pub const RESTORE_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS restore_erasure_ledger (
    tenant_id            text   NOT NULL,
    region               text   NOT NULL,
    completed_at_offset  bigint NOT NULL,
    PRIMARY KEY (tenant_id, region)
);";

/// The forward-only migration set the durable restore-verify erasure ledger binds to (id `0051`, in
/// the free `0050+` range). Applied via the MR-022 [`SubstrateProvider::migrate`] at boot.
pub fn restore_verify_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0051_restore_erasure_ledger",
        RESTORE_ERASURE_LEDGER_MIGRATION,
    )])
}

// =================================================================================================
// DurableRestoreErasureLedger — the always-compiled production backing over the ledger table.
// =================================================================================================

/// The REAL durable restore-verify erasure ledger (production default): the `restore_erasure_ledger`
/// table behind [`crate::restore_verify::ErasureLedger`]. Cloneable; holds the tokio runtime handle so
/// the SYNC gate reads bridge onto the async store (the `block_in_place` + `block_on` convention). NO
/// RLS (non-shred-erasable — see the module docs); connects region-scoped directly to the pool.
#[derive(Clone)]
pub struct DurableRestoreErasureLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurableRestoreErasureLedger {
    /// Build the durable ledger over the MR-022 provider. **Must be called inside a tokio runtime**
    /// (captures `Handle::current()` for the sync→async bridge).
    pub fn new(provider: SubstrateProvider) -> DurableRestoreErasureLedger {
        DurableRestoreErasureLedger {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// **Record a tenant erasure with its completion offset (idempotent).** Upsert on
    /// `(tenant, region)` — a re-erase updates the completion offset, never duplicates. FAIL-STATIC
    /// LOUD via the caller (an unrecorded erasure would let the restore-verify gate green a
    /// resurrection).
    pub async fn record_async(
        &self,
        tenant: &TenantId,
        completed_at_offset: WalOffset,
    ) -> Result<(), ProviderError> {
        let tenant = tenant.0.clone();
        let region = self.region();
        let offset = completed_at_offset as i64;
        sqlx::query(
            "INSERT INTO restore_erasure_ledger (tenant_id, region, completed_at_offset) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (tenant_id, region) DO UPDATE SET \
               completed_at_offset = EXCLUDED.completed_at_offset",
        )
        .bind(&tenant)
        .bind(&region)
        .bind(offset)
        .execute(self.provider.db_pool())
        .await
        .map_err(|e| ProviderError::from(crate::pg::PgError::Query(e.to_string())))?;
        Ok(())
    }

    /// The sync write-through the [`crate::restore_verify::ErasureLedger`] `record_erased*` mutators
    /// bridge onto (FAIL-STATIC LOUD on a store fault — an unrecorded erasure is a resurrection path).
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

    /// The async form of the gate's read: every erased tenant in this region + its completion offset.
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
                Ok((TenantId(tenant), offset as WalOffset))
            })
            .collect()
    }

    /// The sync gate read (bridged onto the runtime handle) — every erased tenant + its completion
    /// offset. FAIL-STATIC LOUD on a store fault (a swallowed empty set would green every restore).
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
