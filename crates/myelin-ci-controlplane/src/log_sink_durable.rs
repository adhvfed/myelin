//! # `log_sink_durable` — CT-004f sub-step 4a: the DURABLE [`LogPersist`] (the live index writer)
//!
//! [`DurableLogPersist`] is the production [`LogPersist`](crate::log_sink::LogPersist) the DB-free
//! adapter ([`LogPipelineSink`](crate::log_sink::LogPipelineSink)) flushes each incremental and
//! terminal checkpoint through. It holds an OLTP [`PgPool`] and writes `log_segment` / `log_anchor`
//! rows via the
//! FROZEN production bind-param SQL ([`INSERT_LOG_SEGMENT_QUERY`](crate::INSERT_LOG_SEGMENT_QUERY) /
//! [`UPSERT_LOG_ANCHOR_QUERY`](crate::UPSERT_LOG_ANCHOR_QUERY)) inside ONE tenant-scoped
//! [`with_tenant_tx`] transaction (BEGIN → set the `(tenant, region)` GUC transaction-scoped →
//! INSERT/UPSERT → COMMIT). Each bounded checkpoint lands atomically under FORCE-RLS, and a
//! re-delivered checkpoint re-runs idempotent upserts (`ON CONFLICT` on the PK) — the same
//! double-effect-0 posture the cost store proves.
//!
//! **The gap this closes:** the CI-P20 integration test (`integration_ci_p20_log_pipeline`) proved the
//! frozen SQL applies + round-trips, but with RAW inline sqlx on a session-GUC connection — there was
//! no production STORE (the same "model-only, no store" gap CT-004a closed for metering). This is that
//! store: the runner's live log path writes the index THROUGH it, tenant-scoped.
//!
//! **Sync→async bridge (the established convention).** [`LogPersist::persist`] is SYNC (the runner
//! drives the [`FirehoseSink`](myelin_ci_sandbox::FirehoseSink) synchronously off a dedicated
//! thread); `with_tenant_tx` is async. The bridge is `block_on` guarded by `block_in_place` (the SAME
//! `PgOutboxBacking` / `DurableLeaseAdapter` idiom).
//!
//! **Sub-step 4b — the pointer co-emit (DONE).** The coalesced `ci.log.available` pointers ride the
//! durable OUTBOX via [`PgRelay::co_commit_in_tx`] on the SAME `with_tenant_tx` connection as the
//! index rows — atomic (both commit or both roll back), the relay publishes iff committed. The
//! pointer's event_id is DETERMINISTIC on `(aggregate, byte_start, byte_end)`, so a re-delivered
//! checkpoint derives the same id and `ON CONFLICT (event_id) DO NOTHING` absorbs it (double-emit 0).

use myelin_events::{derive_envelope, Actor, EmitContext, EventEnvelope, EventId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{with_tenant_tx, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::log_pipeline::{LogAvailablePointer, INSERT_LOG_SEGMENT_QUERY, UPSERT_LOG_ANCHOR_QUERY};
use crate::log_sink::{FlushedJobLogs, LogPersist, LogResume, SINGLE_STEP_ID};

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

    async fn resume_async(
        &self,
        tenant: &TenantId,
        region: &Region,
        run_id: &str,
        job_id: &str,
    ) -> Result<LogResume, PgError> {
        let run = parse_uuid("run_id", run_id)?;
        let job = parse_uuid("job_id", job_id)?;
        let tenant_str = tenant.as_str().to_string();
        let region_str = region.as_str().to_string();
        let tenant_bind = tenant_str.clone();
        let region_bind = region_str.clone();
        with_tenant_tx(&self.pool, &tenant_str, &region_str, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(
                    "SELECT segment_seq, byte_start, byte_end \
                     FROM log_segment \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND job_id = $4 \
                     ORDER BY segment_seq DESC LIMIT 1",
                )
                .bind(&tenant_bind)
                .bind(&region_bind)
                .bind(run)
                .bind(job)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|error| PgError::Query(error.to_string()))?;
                let (next_segment_seq, next_byte_offset) = match row {
                    None => (0, 0),
                    Some(row) => {
                        let segment_seq: i32 = row
                            .try_get("segment_seq")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let byte_start: i64 = row
                            .try_get("byte_start")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let byte_end: i64 = row
                            .try_get("byte_end")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        if segment_seq < 0 || byte_start < 0 || byte_end < byte_start {
                            return Err(PgError::Query(
                                "durable log head has invalid sequence or byte coordinates".into(),
                            ));
                        }
                        (
                            segment_seq.checked_add(1).ok_or_else(|| {
                                PgError::Query("durable log segment sequence exhausted".into())
                            })?,
                            byte_end,
                        )
                    }
                };
                let anchor = sqlx::query(
                    "SELECT byte_start FROM log_anchor \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND job_id = $4 \
                       AND step_id = $5",
                )
                .bind(&tenant_bind)
                .bind(&region_bind)
                .bind(run)
                .bind(job)
                .bind(SINGLE_STEP_ID)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|error| PgError::Query(error.to_string()))?;
                let step_byte_start = match anchor {
                    Some(row) => row
                        .try_get::<i64, _>("byte_start")
                        .map_err(|error| PgError::Query(error.to_string()))?,
                    None if next_segment_seq == 0 => 0,
                    None => {
                        return Err(PgError::Query(
                            "durable log segments exist without their step anchor".into(),
                        ))
                    }
                };
                if step_byte_start < 0 || step_byte_start > next_byte_offset {
                    return Err(PgError::Query(
                        "durable log anchor start is outside the committed byte range".into(),
                    ));
                }
                Ok(LogResume {
                    next_segment_seq,
                    next_byte_offset,
                    step_byte_start,
                })
            })
        })
        .await
    }

    /// Write one incremental or terminal checkpoint in ONE tenant-scoped tx (FORCE-RLS). The
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
        let run = parse_uuid("run_id", &flushed.run_id)?;
        let job = parse_uuid("job_id", &flushed.job_id)?;
        let tenant_str = tenant.as_str().to_string();
        // Serialize the empty-head case as well as established streams. A row lock alone cannot
        // protect two writers racing to create segment zero, so every append takes this
        // transaction-local stream lock before re-reading and comparing the authoritative head.
        let append_lock = format!(
            "{}:{}|{}:{}|{}|{}",
            tenant_str.len(),
            tenant_str,
            region.len(),
            region,
            run,
            job
        );
        // Owned copies for the emit-context INSIDE the closure (the `&tenant_str`/`&region` below are
        // borrowed by `with_tenant_tx` for the GUC set, so the `move` closure cannot take them too).
        let tenant_owned = tenant.clone();
        let region_owned = region.clone();
        let tenant_bind = tenant_str.clone();
        let region_bind = region.clone();
        with_tenant_tx(&self.pool, &tenant_str, &region, move |conn| {
            Box::pin(async move {
                // @tenant-cross-scope: PostgreSQL advisory locking reads no tenant rows. The
                // length-framed key contains the already-validated exact tenant, region, run, and
                // job; tenant-store authority remains the predicate-bound queries below.
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(&append_lock)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;

                // Re-read under the stream lock. `resume_async` is only a latency hint used to build
                // the candidate row; this comparison is the append authority. Every newly admitted
                // segment must be the exact successor of the committed `(seq, byte_end)` head.
                let head = sqlx::query(
                    "SELECT segment_seq, byte_start, byte_end \
                     FROM log_segment \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND job_id = $4 \
                     ORDER BY segment_seq DESC LIMIT 1",
                )
                .bind(&tenant_bind)
                .bind(&region_bind)
                .bind(run)
                .bind(job)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|error| PgError::Query(error.to_string()))?;
                let (mut next_segment_seq, mut next_byte_offset) = match head {
                    None => (0, 0),
                    Some(row) => {
                        let segment_seq: i32 = row
                            .try_get("segment_seq")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let byte_start: i64 = row
                            .try_get("byte_start")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let byte_end: i64 = row
                            .try_get("byte_end")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        if segment_seq < 0 || byte_start < 0 || byte_end < byte_start {
                            return Err(PgError::Query(
                                "durable log head has invalid sequence or byte coordinates".into(),
                            ));
                        }
                        (
                            segment_seq.checked_add(1).ok_or_else(|| {
                                PgError::Query("durable log segment sequence exhausted".into())
                            })?,
                            byte_end,
                        )
                    }
                };

                for seg in &flushed.segments {
                    let seg_run = parse_uuid("run_id", &seg.run_id)?;
                    let seg_job = parse_uuid("job_id", &seg.job_id)?;
                    if seg.tenant_id != tenant_bind
                        || seg.region != region_bind
                        || seg_run != run
                        || seg_job != job
                        || seg.segment_seq < 0
                        || seg.byte_start < 0
                        || seg.byte_end < seg.byte_start
                    {
                        return Err(PgError::Query(
                            "log checkpoint contains a foreign or invalid segment".into(),
                        ));
                    }
                    if seg.segment_seq > next_segment_seq {
                        return Err(PgError::Query(format!(
                            "log append sequence gap: committed next={next_segment_seq}, incoming={}",
                            seg.segment_seq
                        )));
                    }
                    if seg.segment_seq < next_segment_seq {
                        // A replay may name an older row only if every immutable field is identical.
                        // Never execute an INSERT here: filling a historical hole would conceal a
                        // corrupt prefix instead of rejecting it.
                        let prior = sqlx::query(
                            "SELECT region, blob_ref, byte_start, byte_end, pii_key_ref \
                             FROM log_segment \
                             WHERE tenant_id = $1 AND run_id = $2 AND job_id = $3 \
                               AND segment_seq = $4",
                        )
                        .bind(&tenant_bind)
                        .bind(run)
                        .bind(job)
                        .bind(seg.segment_seq)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|error| PgError::Query(error.to_string()))?;
                        let Some(prior) = prior else {
                            return Err(PgError::Query(
                                "log replay targets a missing historical segment".into(),
                            ));
                        };
                        let exact = prior
                            .try_get::<String, _>("region")
                            .map_err(|error| PgError::Query(error.to_string()))?
                            == seg.region
                            && prior
                                .try_get::<Option<String>, _>("blob_ref")
                                .map_err(|error| PgError::Query(error.to_string()))?
                                == seg.blob_ref
                            && prior
                                .try_get::<i64, _>("byte_start")
                                .map_err(|error| PgError::Query(error.to_string()))?
                                == seg.byte_start
                            && prior
                                .try_get::<i64, _>("byte_end")
                                .map_err(|error| PgError::Query(error.to_string()))?
                                == seg.byte_end
                            && prior
                                .try_get::<String, _>("pii_key_ref")
                                .map_err(|error| PgError::Query(error.to_string()))?
                                == seg.pii_key_ref;
                        if !exact {
                            return Err(PgError::Query(
                                "log replay diverges from an immutable committed segment".into(),
                            ));
                        }
                        continue;
                    }
                    if seg.byte_start != next_byte_offset {
                        return Err(PgError::Query(format!(
                            "log append byte gap: committed next={next_byte_offset}, incoming={}",
                            seg.byte_start
                        )));
                    }
                    let inserted = sqlx::query(INSERT_LOG_SEGMENT_QUERY)
                        .bind(&seg.tenant_id) // $1 tenant_id (RLS predicate)
                        .bind(&seg.region) // $2 region (residency pin)
                        .bind(seg_run) // $3 run_id ::uuid
                        .bind(seg_job) // $4 job_id ::uuid
                        .bind(seg.segment_seq) // $5 segment_seq
                        .bind(seg.blob_ref.as_deref()) // $6 blob_ref (nullable while open)
                        .bind(seg.byte_start) // $7 byte_start
                        .bind(seg.byte_end) // $8 byte_end
                        .bind(&seg.pii_key_ref) // $9 pii_key_ref
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    if inserted.rows_affected() != 1 {
                        return Err(PgError::Query(
                            "log append conflicts with an immutable committed segment".into(),
                        ));
                    }
                    next_segment_seq = next_segment_seq.checked_add(1).ok_or_else(|| {
                        PgError::Query("durable log segment sequence exhausted".into())
                    })?;
                    next_byte_offset = seg.byte_end;
                }
                for anc in &flushed.anchors {
                    let anc_run = parse_uuid("run_id", &anc.run_id)?;
                    let anc_job = parse_uuid("job_id", &anc.job_id)?;
                    if anc.tenant_id != tenant_bind
                        || anc.region != region_bind
                        || anc_run != run
                        || anc_job != job
                        || anc.byte_start < 0
                        || anc.byte_end.is_some_and(|end| end < anc.byte_start)
                    {
                        return Err(PgError::Query(
                            "log checkpoint contains a foreign or invalid anchor".into(),
                        ));
                    }
                    let upserted = sqlx::query(UPSERT_LOG_ANCHOR_QUERY)
                        .bind(&anc.tenant_id) // $1 tenant_id
                        .bind(&anc.region) // $2 region
                        .bind(anc_run) // $3 run_id ::uuid
                        .bind(anc_job) // $4 job_id ::uuid
                        .bind(&anc.step_id) // $5 step_id
                        .bind(anc.byte_start) // $6 byte_start
                        .bind(anc.byte_end) // $7 byte_end (nullable while running)
                        .bind(anc.status.token()) // $8 status
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    if upserted.rows_affected() != 1 {
                        return Err(PgError::Query(
                            "log anchor conflicts with an immutable terminal checkpoint".into(),
                        ));
                    }
                }
                // CT-004f sub-step 4b — co-emit the coalesced `ci.log.available` pointers to the
                // durable OUTBOX on the SAME tx as the index rows (atomic: both commit or both roll
                // back; the relay publishes iff committed — `no-raw-publish` green). The event_id is
                // DETERMINISTIC on the pointer identity, so a re-delivered `finish` derives the same
                // id and `ON CONFLICT (event_id) DO NOTHING` absorbs it (double-emit 0 — parity with
                // the index upserts). references-not-payloads: the pointer names byte ranges + the
                // sealed-segment ref, never log bytes.
                for ptr in &flushed.pointers {
                    let envelope = ci_log_available_envelope(&tenant_owned, &region_owned, ptr);
                    let aggregate = envelope.aggregate.0.clone();
                    PgRelay::co_commit_in_tx(&mut *conn, &aggregate, &envelope)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                }
                Ok(())
            })
        })
        .await
    }
}

/// Assemble the `ci.log.available` [`EventEnvelope`] for one pointer, as the CI control-plane SERVICE
/// principal (mirroring the CI producer emit-context convention — a `Service` principal, `schema_ver`
/// 1, root causality). references-not-payloads (the draft carries the byte range + ref, never bytes).
fn ci_log_available_envelope(
    tenant: &TenantId,
    region: &str,
    ptr: &LogAvailablePointer,
) -> EventEnvelope {
    let draft = ptr.to_draft();
    let ctx = EmitContext {
        event_id: ci_log_event_id(tenant, ptr),
        tenant: tenant.clone(),
        region: Region::new(region.to_string()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        schema_ver: 1,
        // The CI producer convention (`ci_pipeline_driver::service_ctx_base`) stamps a fixed service
        // timestamp; the log-availability fact's ordering is the per-aggregate `seq`, not the clock.
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:00Z".into()),
        caused_by: None,
    };
    derive_envelope(draft, ctx, None)
}

/// The DETERMINISTIC `ci.log.available` event id — stable on `(tenant, run, job, step, byte_start,
/// byte_end)` so a re-delivered `finish` derives the SAME id and the outbox `ON CONFLICT (event_id)`
/// dedups it (a fresh ULID would double-emit the notification). A fast FNV-1a idempotency key, not a
/// security primitive (the same idiom as `cost_id_for` / `snapshot_event_id`).
///
/// The key includes the `tenant` (the shared `outbox` `UNIQUE(event_id)` is GLOBAL, not tenant-scoped
/// — so two tenants must never collide) and the run/job ids in their CANONICAL uuid form (so the key
/// matches the `uuid`-PK normalization the `log_segment`/`log_anchor` rows dedup on — a redelivery
/// that carried the id in a different textual form still derives the same key). A non-uuid id (which
/// would already have aborted the row inserts before this runs) falls back to the raw string.
fn ci_log_event_id(tenant: &TenantId, ptr: &LogAvailablePointer) -> EventId {
    let canon = |id: &str| {
        Uuid::parse_str(id)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| id.into())
    };
    let keyed = format!(
        "{}\u{0}{}\u{0}{}\u{0}{}@{}:{}",
        tenant.as_str(),
        canon(&ptr.coord.run_id),
        canon(&ptr.coord.job_id),
        ptr.coord.step_id,
        ptr.byte_start,
        ptr.byte_end,
    );
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
    for b in keyed.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV-1a prime
    }
    EventId(format!("cilog-{hash:016x}"))
}

impl LogPersist for DurableLogPersist {
    fn resume(
        &self,
        tenant: &TenantId,
        region: &Region,
        run_id: &str,
        job_id: &str,
    ) -> Result<LogResume, Box<dyn std::error::Error + Send + Sync>> {
        let future = self.resume_async(tenant, region, run_id, job_id);
        let result = match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.rt.block_on(future)),
            Err(_) => self.rt.block_on(future),
        };
        result.map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
    }

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
