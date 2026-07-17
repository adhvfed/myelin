//! # `log_sink_durable` — CT-004f sub-step 4a: the DURABLE [`LogPersist`] (the live index writer)
//!
//! [`DurableLogPersist`] is the production [`LogPersist`](crate::log_sink::LogPersist) the DB-free
//! adapter ([`LogPipelineSink`](crate::log_sink::LogPipelineSink)) flushes a finished job's sealed
//! index through. It holds an OLTP [`PgPool`] and writes the `log_segment` / `log_anchor` rows via the
//! FROZEN production bind-param SQL ([`INSERT_LOG_SEGMENT_QUERY`](crate::INSERT_LOG_SEGMENT_QUERY) /
//! [`UPSERT_LOG_ANCHOR_QUERY`](crate::UPSERT_LOG_ANCHOR_QUERY)) inside ONE tenant-scoped
//! [`with_tenant_tx`] transaction (BEGIN → set the `(tenant, region)` GUC transaction-scoped →
//! INSERT/UPSERT → COMMIT). So a job's whole index lands ATOMICALLY under FORCE-RLS, and a
//! re-delivered `finish` re-runs the idempotent upserts (`ON CONFLICT` on the PK) — the same
//! double-effect-0 posture the cost store proves.
//!
//! **The gap this closes:** the CI-P20 integration test (`integration_ci_p20_log_pipeline`) proved the
//! frozen SQL applies + round-trips, but with RAW inline sqlx on a session-GUC connection — there was
//! no production STORE (the same "model-only, no store" gap CT-004a closed for metering). This is that
//! store: the runner's live log path writes the index THROUGH it, tenant-scoped.
//!
//! **Sync→async bridge (the established convention).** [`LogPersist::persist`] is SYNC (the runner
//! drives [`FirehoseSink::finish`](myelin_ci_sandbox::FirehoseSink) synchronously off a dedicated
//! thread); `with_tenant_tx` is async. The bridge is `block_on` guarded by `block_in_place` (the SAME
//! `PgOutboxBacking` / `DurableLeaseAdapter` idiom).
//!
//! **What is NOT here (sub-step 4b):** the `ci.log.available` pointer → OUTBOX co-emit on the SAME tx.
//! The index rows ARE the durable truth the `details_ref` resolves against (losing a pointer does not
//! corrupt the index — a re-drive re-emits); the atomic pointer co-commit is the next slice.

use myelin_storage::{with_tenant_tx, PgError};
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;

use crate::log_pipeline::{INSERT_LOG_SEGMENT_QUERY, UPSERT_LOG_ANCHOR_QUERY};
use crate::log_sink::{FlushedJobLogs, LogPersist};

/// **The durable `log_segment` / `log_anchor` writer (CT-004f sub-step 4a).** Holds the OLTP pool +
/// the runtime handle the sync `persist` bridges its async tenant-scoped write onto.
pub struct DurableLogPersist {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableLogPersist {
    /// Build the store over `pool`, bridging async DB calls onto `rt` (the runner-loop runtime handle).
    pub fn with_pg(pool: PgPool, rt: tokio::runtime::Handle) -> DurableLogPersist {
        DurableLogPersist { pool, rt }
    }

    /// The pool this store is bound to (for a test / a caller that reads the rows back).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Write the sealed index for one finished job in ONE tenant-scoped tx (FORCE-RLS). The
    /// `(tenant, region)` GUC is transaction-scoped; both id columns are `uuid`, so the opaque string
    /// ids are parsed to [`Uuid`] (a non-uuid id is a loud [`PgError::Query`], never a silent skip).
    async fn persist_async(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), PgError> {
        // The residency pin the pipeline stamped onto every row (all rows in a flush share it). No
        // rows → nothing to persist (a job that produced no output still closed an anchor, so this is
        // effectively never empty, but stay total).
        let region = flushed
            .anchors
            .first()
            .map(|a| a.region.clone())
            .or_else(|| flushed.segments.first().map(|s| s.region.clone()));
        let Some(region) = region else {
            return Ok(());
        };
        let tenant_str = tenant.as_str().to_string();
        with_tenant_tx(&self.pool, &tenant_str, &region, move |conn| {
            Box::pin(async move {
                for seg in &flushed.segments {
                    let run = parse_uuid("run_id", &seg.run_id)?;
                    let job = parse_uuid("job_id", &seg.job_id)?;
                    sqlx::query(INSERT_LOG_SEGMENT_QUERY)
                        .bind(&seg.tenant_id) // $1 tenant_id (RLS predicate)
                        .bind(&seg.region) // $2 region (residency pin)
                        .bind(run) // $3 run_id ::uuid
                        .bind(job) // $4 job_id ::uuid
                        .bind(seg.segment_seq) // $5 segment_seq
                        .bind(seg.blob_ref.as_deref()) // $6 blob_ref (nullable while open)
                        .bind(seg.byte_start) // $7 byte_start
                        .bind(seg.byte_end) // $8 byte_end
                        .bind(&seg.pii_key_ref) // $9 pii_key_ref
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                }
                for anc in &flushed.anchors {
                    let run = parse_uuid("run_id", &anc.run_id)?;
                    let job = parse_uuid("job_id", &anc.job_id)?;
                    sqlx::query(UPSERT_LOG_ANCHOR_QUERY)
                        .bind(&anc.tenant_id) // $1 tenant_id
                        .bind(&anc.region) // $2 region
                        .bind(run) // $3 run_id ::uuid
                        .bind(job) // $4 job_id ::uuid
                        .bind(&anc.step_id) // $5 step_id
                        .bind(anc.byte_start) // $6 byte_start
                        .bind(anc.byte_end) // $7 byte_end (nullable while running)
                        .bind(anc.status.token()) // $8 status
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                }
                Ok(())
            })
        })
        .await
    }
}

impl LogPersist for DurableLogPersist {
    fn persist(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Sync→async bridge: the runner drives `finish` off a dedicated thread, so `block_on` runs
        // directly; the `try_current` guard falls back to `block_in_place` if ever driven on a
        // multi-thread worker (the PgOutboxBacking / DurableLeaseAdapter convention).
        let fut = self.persist_async(tenant, flushed);
        let res = match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.rt.block_on(fut)),
            Err(_) => self.rt.block_on(fut),
        };
        res.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}

/// Parse an opaque `(run|job)` id string to the `uuid` column type — a loud error on a non-uuid id
/// (the durable columns are `uuid NOT NULL`, so a malformed id must fail the write, never silently drop
/// a job's log index).
fn parse_uuid(field: &'static str, value: &str) -> Result<Uuid, PgError> {
    Uuid::parse_str(value)
        .map_err(|e| PgError::Query(format!("log index {field} is not a uuid ({value:?}): {e}")))
}
