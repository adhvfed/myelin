use myelin_events::{derive_envelope, Actor, EmitContext, EventEnvelope, EventId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{with_tenant_tx, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::log_pipeline::{LogAvailablePointer, INSERT_LOG_SEGMENT_QUERY, UPSERT_LOG_ANCHOR_QUERY};
use crate::log_sink::{FlushedJobLogs, LogPersist, LogResume, SINGLE_STEP_NO};

pub struct DurableLogPersist {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl DurableLogPersist {
    pub fn with_pg(pool: PgPool, rt: tokio::runtime::Handle) -> DurableLogPersist {
        DurableLogPersist { pool, rt }
    }

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
        let workflow_run = parse_uuid("workflow_run_id", run_id)?;
        let job = parse_uuid("job_id", job_id)?;
        let tenant_str = tenant.as_str().to_string();
        let region_str = region.as_str().to_string();
        let tenant_bind = tenant_str.clone();
        let region_bind = region_str.clone();
        with_tenant_tx(&self.pool, &tenant_str, &region_str, move |conn| {
            Box::pin(async move {
                let canonical_run_id: String = sqlx::query_scalar(
                    "SELECT launch.spec->>'ci_run_id' \
                     FROM ci_job_spec AS launch \
                     JOIN job_queue AS queue \
                       ON queue.tenant_id=launch.tenant_id AND queue.region=launch.region \
                      AND queue.job_id=launch.job_id AND queue.run_id=launch.run_id \
                     WHERE launch.tenant_id=$1 AND launch.region=$2 \
                       AND launch.job_id=$3 AND launch.run_id=$4",
                )
                .bind(&tenant_bind)
                .bind(&region_bind)
                .bind(job)
                .bind(workflow_run)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    PgError::Query(format!("resolve canonical CI log run: {error}"))
                })?;
                let run = parse_uuid("ci_run_id", &canonical_run_id)?;
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
                .bind(SINGLE_STEP_NO.to_string())
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
                    canonical_run_id: Some(canonical_run_id),
                    next_segment_seq,
                    next_byte_offset,
                    step_byte_start,
                })
            })
        })
        .await
    }

    async fn persist_async(
        &self,
        tenant: &TenantId,
        flushed: FlushedJobLogs,
    ) -> Result<(), PgError> {
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
        let append_lock = format!(
            "{}:{}|{}:{}|{}|{}",
            tenant_str.len(),
            tenant_str,
            region.len(),
            region,
            run,
            job
        );
        let tenant_owned = tenant.clone();
        let region_owned = region.clone();
        let tenant_bind = tenant_str.clone();
        let region_bind = region.clone();
        with_tenant_tx(&self.pool, &tenant_str, &region, move |conn| {
            Box::pin(async move {
                // @tenant-cross-scope: PostgreSQL advisory locking reads no tenant rows. The
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(&append_lock)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;

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
                        .bind(&seg.tenant_id)
                        .bind(&seg.region)
                        .bind(seg_run)
                        .bind(seg_job)
                        .bind(seg.segment_seq)
                        .bind(seg.blob_ref.as_deref())
                        .bind(seg.byte_start)
                        .bind(seg.byte_end)
                        .bind(&seg.pii_key_ref)
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
                        .bind(&anc.tenant_id)
                        .bind(&anc.region)
                        .bind(anc_run)
                        .bind(anc_job)
                        .bind(&anc.step_id)
                        .bind(anc.byte_start)
                        .bind(anc.byte_end)
                        .bind(anc.status.token())
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    if upserted.rows_affected() != 1 {
                        return Err(PgError::Query(
                            "log anchor conflicts with an immutable terminal checkpoint".into(),
                        ));
                    }
                }
                for ptr in &flushed.pointers {
                    let envelope =
                        ci_log_available_envelope(&tenant_owned, &region_owned, ptr)
                            .map_err(|error| PgError::Query(error.to_string()))?;
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

fn ci_log_available_envelope(
    tenant: &TenantId,
    region: &str,
    ptr: &LogAvailablePointer,
) -> Result<EventEnvelope, crate::log_pipeline::LogReferenceError> {
    let draft = ptr.to_draft(tenant)?;
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
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:00Z".into()),
        caused_by: None,
    };
    Ok(derive_envelope(draft, ctx, None))
}

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
        ptr.coord.step_no,
        ptr.byte_start,
        ptr.byte_end,
    );
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in keyed.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
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
        let fut = self.persist_async(tenant, flushed);
        let res = match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.rt.block_on(fut)),
            Err(_) => self.rt.block_on(fut),
        };
        res.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}

fn parse_uuid(field: &'static str, value: &str) -> Result<Uuid, PgError> {
    Uuid::parse_str(value)
        .map_err(|e| PgError::Query(format!("log index {field} is not a uuid ({value:?}): {e}")))
}
