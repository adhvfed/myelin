//! # Durable PG backing for the post-restore re-erasure ledger (MR-009b W6b / P-ST-14, §7.5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/storage.md` §7.5
//! (the mandatory post-restore re-erasure pass drives the erasure ledger 10.8: for each erasure
//! completed AFTER the backup's PIT, re-apply it — assert 0 resurrected subjects). This module is the
//! REAL durable backing behind the [`crate::reerase::PostRestoreErasureLedger`] seam: the in-memory
//! [`crate::reerase::InMemoryPostPitLedger`] (`records: Vec<ErasureRecord>`) is now the
//! `test-support`-gated TEST DOUBLE; [`DurablePostPitLedger`] is the always-compiled production ledger
//! [`crate::reerase::ReErasePass::run`] drives through its `&dyn` seam (zero caller change).
//!
//! ## The table (`post_pit_erasure_ledger`, migration `0052`) — NON-shred-erasable, NO RLS
//! Keyed `(tenant_id, region, subject)`; holds ONLY the OPAQUE subject id + the tenant + the
//! **cross-seam completion offset** (`completed_at_offset` — the §7.3 cursor the restore lands at) —
//! never real-identity PII. **NON-shred-erasable + NO RLS by construction** (mirrors the W6a
//! `identity_pseudonym_erasure_ledger`): it MUST survive the crypto-shred it records AND survive a
//! restore, so the post-restore re-erasure pass can replay it against a resurrected pre-erasure backup
//! — a crypto-shred/RLS lever on THIS table would defeat that. Partition isolation is the explicit
//! `(tenant_id, region)` predicate on every statement (there is no cross-tenant read PATH; the pass
//! reads the whole region's post-PIT set, exactly like the in-memory double reads its whole `Vec`).
//!
//! ## Sync seam over an async store — the write-through bridge
//! [`crate::reerase::PostRestoreErasureLedger::erasures_completed_after`] is SYNC (the pass is sync).
//! [`DurablePostPitLedger`] captures the tokio runtime handle at construction and bridges the read on
//! it (`block_in_place` + `block_on`, the SAME convention the Wave-5 durable KMS uses) — the durable
//! store is reached without changing the pass's shape.
//!
//! Compiled UNCONDITIONALLY (durable-by-default, MR-009b): `integration` remains a test-selector only.

use sqlx::Row;

use myelin_tenancy::TenantId;

use crate::backup::WalOffset;
use crate::encryption::SubjectId;
use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};
use crate::reerase::{ErasureRecord, PostRestoreErasureLedger};

// =================================================================================================
// Migration 0052 — the PII-free, NON-shred-erasable post-PIT erasure ledger (NO RLS).
// =================================================================================================

/// The `post_pit_erasure_ledger` table (storage.md §7.5 / contract 10.8) — `(tenant, region,
/// subject)`-keyed. Holds ONLY the OPAQUE subject id + the erasure's **cross-seam completion offset**
/// (`completed_at_offset`, the §7.3 cursor) — **never** real identity. **NON-shred-erasable + NO RLS
/// by construction:** it must SURVIVE the key destruction it records AND a restore so the §7.5
/// re-erasure pass can replay it. Partition isolation is the explicit `(tenant_id, region)` predicate
/// on every statement. Forward-only (`IF NOT EXISTS`); idempotent upsert on the key.
pub const POST_PIT_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS post_pit_erasure_ledger (
    tenant_id            text   NOT NULL,
    region               text   NOT NULL,
    subject              text   NOT NULL,
    completed_at_offset  bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, subject)
);";

/// The forward-only migration set the durable post-PIT ledger binds to (id `0052`, in the free
/// `0050+` range). Applied via the MR-022 [`SubstrateProvider::migrate`] at boot; idempotent on re-boot.
pub fn post_pit_durable_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0052_post_pit_erasure_ledger",
        POST_PIT_ERASURE_LEDGER_MIGRATION,
    )])
}

// =================================================================================================
// DurablePostPitLedger — the always-compiled production PostRestoreErasureLedger over the ledger table.
// =================================================================================================

/// The REAL durable post-PIT erasure ledger (production default): the `post_pit_erasure_ledger` table
/// behind the [`PostRestoreErasureLedger`] seam. Cloneable (the provider/pool is an `Arc`-backed
/// handle). Holds the tokio runtime handle so the SYNC seam read bridges onto the async store.
///
/// **NON-shred-erasable + NO RLS** (see the module docs): connects region-scoped (the re-erasure pass
/// reads the whole region's post-PIT set), NOT through the per-request RLS `with_tenant_tx` convention
/// — this table has no RLS policy (it must survive the shred it records). A NAMED tenant-predicate
/// exclusion (like `placement_durable` / `events_durable`): every statement carries the explicit
/// `region` (+ `tenant_id` on writes) predicate, and the durable read is the whole-region enumeration
/// the pass needs. Erasure-record writes are FAIL-STATIC LOUD (an unrecorded post-PIT erasure is a
/// silent resurrection path — a subject erased after the backup that the pass then never re-applies).
#[derive(Clone)]
pub struct DurablePostPitLedger {
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurablePostPitLedger {
    /// Build the durable ledger over the MR-022 provider. **Must be called inside a tokio runtime**
    /// (it captures `Handle::current()` — the same multi-threaded-runtime bridge the Wave-5 KMS uses —
    /// so the sync seam read can `block_on` the async store).
    pub fn new(provider: SubstrateProvider) -> DurablePostPitLedger {
        DurablePostPitLedger {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// **Record a completed erasure (§7.5 / 10.8) — PII-free, idempotent.** The GDPR erasure-ledger
    /// seam writes one row per erasure on DSR completion; the re-erasure pass replays them. Upsert on
    /// `(tenant, region, subject)` (a re-erase updates the completion offset, never duplicates).
    pub async fn record(
        &self,
        tenant: &TenantId,
        subject: &SubjectId,
        completed_at_offset: WalOffset,
    ) -> Result<(), ProviderError> {
        let tenant = tenant.0.clone();
        let region = self.region();
        let subject = subject.0.clone();
        let offset = completed_at_offset as i64;
        sqlx::query(
            "INSERT INTO post_pit_erasure_ledger \
               (tenant_id, region, subject, completed_at_offset) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (tenant_id, region, subject) DO UPDATE SET \
               completed_at_offset = EXCLUDED.completed_at_offset",
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

    /// The async form of the §7.5 selection: every erasure in this region completed strictly AFTER
    /// `pit` (the resurrection-risk set the pass re-applies). Ordered by subject for a deterministic
    /// re-apply order.
    async fn completed_after_async(&self, pit: WalOffset) -> Result<Vec<ErasureRecord>, ProviderError> {
        let region = self.region();
        let pit_i = pit as i64;
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
        Ok(rows
            .iter()
            .map(|r| {
                let tenant: String = r.get("tenant_id");
                let subject: String = r.get("subject");
                let offset: i64 = r.get("completed_at_offset");
                ErasureRecord::new(SubjectId::new(subject), TenantId(tenant), offset as WalOffset)
            })
            .collect())
    }
}

impl PostRestoreErasureLedger for DurablePostPitLedger {
    /// The §7.5 selection over the durable store, bridged onto the runtime handle (the sync seam the
    /// pass drives). A store fault is FAIL-STATIC LOUD (an incomplete post-PIT read would let a
    /// resurrected subject slip past re-erasure — never swallowed to an empty set).
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord> {
        tokio::task::block_in_place(|| self.rt.block_on(self.completed_after_async(pit)))
            .unwrap_or_else(|e| {
                panic!(
                    "FAIL-STATIC: durable post-PIT erasure ledger read failed (an incomplete \
                     post-PIT set would let a post-backup-erased subject escape the §7.5 re-erasure \
                     pass — a silent resurrection): {e}"
                )
            })
    }
}
