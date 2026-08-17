use myelin_events::{derive_envelope, Actor, EmitContext, EventEnvelope, EventId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{with_tenant_tx, ContentHash, PgError, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::log_pipeline::{
    AnchorStatus, LogAvailablePointer, INSERT_LOG_SEGMENT_QUERY, UPSERT_LOG_ANCHOR_QUERY,
};
use crate::log_sink::{FlushedJobLogs, LogPersist, LogResume, SINGLE_STEP_NO};

pub struct DurableLogPersist {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

#[derive(Debug)]
struct CheckpointScope {
    region: String,
    run: Uuid,
    job: Uuid,
}

fn validate_checkpoint(
    tenant: &TenantId,
    flushed: &FlushedJobLogs,
) -> Result<Option<CheckpointScope>, PgError> {
    if flushed.segments.is_empty() && flushed.anchors.is_empty() && flushed.pointers.is_empty() {
        return Ok(None);
    }
    let region = flushed
        .anchors
        .first()
        .map(|anchor| anchor.region.clone())
        .or_else(|| {
            flushed
                .segments
                .first()
                .map(|segment| segment.region.clone())
        })
        .ok_or_else(|| {
            PgError::Query(
                "log checkpoint contains availability pointers without a regional index row".into(),
            )
        })?;
    let run = parse_uuid("run_id", &flushed.run_id)?;
    let job = parse_uuid("job_id", &flushed.job_id)?;

    for segment in &flushed.segments {
        let segment_run = parse_uuid("segment run_id", &segment.run_id)?;
        let segment_job = parse_uuid("segment job_id", &segment.job_id)?;
        let blob_is_canonical = segment
            .blob_ref
            .as_deref()
            .is_some_and(|blob_ref| ContentHash::parse(blob_ref).is_ok());
        let key_belongs_to_tenant =
            PiiKeyRef::parse(&segment.pii_key_ref).is_some_and(|key| key.tenant == *tenant);
        if segment.tenant_id != tenant.as_str()
            || segment.region != region
            || segment_run != run
            || segment_job != job
            || segment.segment_seq < 0
            || segment.byte_start < 0
            || segment.byte_end <= segment.byte_start
            || !blob_is_canonical
            || !key_belongs_to_tenant
        {
            return Err(PgError::Query(
                "log checkpoint contains a foreign or invalid segment".into(),
            ));
        }
    }

    let expected_step_id = SINGLE_STEP_NO.to_string();
    for anchor in &flushed.anchors {
        let anchor_run = parse_uuid("anchor run_id", &anchor.run_id)?;
        let anchor_job = parse_uuid("anchor job_id", &anchor.job_id)?;
        let status_and_end_agree = if anchor.status.is_terminal() {
            anchor
                .byte_end
                .is_some_and(|byte_end| byte_end >= anchor.byte_start)
        } else {
            anchor.byte_end.is_none()
        };
        if anchor.tenant_id != tenant.as_str()
            || anchor.region != region
            || anchor_run != run
            || anchor_job != job
            || anchor.step_id != expected_step_id
            || anchor.byte_start < 0
            || !status_and_end_agree
        {
            return Err(PgError::Query(
                "log checkpoint contains a foreign or invalid anchor".into(),
            ));
        }
    }

    for pointer in &flushed.pointers {
        let pointer_run = parse_uuid("pointer run_id", &pointer.coord().run_id)?;
        let pointer_job = parse_uuid("pointer job_id", &pointer.coord().job_id)?;
        if pointer_run != run || pointer_job != job || pointer.coord().step_no != SINGLE_STEP_NO {
            return Err(PgError::Query(
                "log checkpoint contains a foreign availability pointer".into(),
            ));
        }
    }

    Ok(Some(CheckpointScope { region, run, job }))
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
                    "SELECT byte_start, byte_end, status FROM log_anchor \
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
                let (step_byte_start, terminal_status) = match anchor {
                    Some(row) => {
                        let byte_start = row
                            .try_get::<i64, _>("byte_start")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let byte_end = row
                            .try_get::<Option<i64>, _>("byte_end")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let status = row
                            .try_get::<String, _>("status")
                            .map_err(|error| PgError::Query(error.to_string()))?;
                        let status = AnchorStatus::from_token(&status).ok_or_else(|| {
                            PgError::Query("durable log anchor has an unknown status".into())
                        })?;
                        let terminal_status = if status.is_terminal() {
                            if byte_end != Some(next_byte_offset) {
                                return Err(PgError::Query(
                                    "terminal log anchor does not end at the committed byte head"
                                        .into(),
                                ));
                            }
                            Some(status)
                        } else {
                            if byte_end.is_some() {
                                return Err(PgError::Query(
                                    "running log anchor unexpectedly has a terminal byte end"
                                        .into(),
                                ));
                            }
                            None
                        };
                        (byte_start, terminal_status)
                    }
                    None if next_segment_seq == 0 => (0, None),
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
                    terminal_status,
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
        let Some(scope) = validate_checkpoint(tenant, &flushed)? else {
            return Ok(());
        };
        let CheckpointScope { region, run, job } = scope;
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
                        .bind(run)
                        .bind(job)
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
                    let upserted = sqlx::query(UPSERT_LOG_ANCHOR_QUERY)
                        .bind(&anc.tenant_id)
                        .bind(&anc.region)
                        .bind(run)
                        .bind(job)
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
        canon(&ptr.coord().run_id),
        canon(&ptr.coord().job_id),
        ptr.coord().step_no,
        ptr.byte_start(),
        ptr.byte_end(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_pipeline::{LogAnchorRow, LogCoord, LogSegmentRow};

    const RUN_ID: &str = "00000000-0000-0000-0000-000000000001";
    const JOB_ID: &str = "00000000-0000-0000-0000-000000000002";
    const OTHER_JOB_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn tenant() -> TenantId {
        TenantId::from_token("tenant-a")
    }

    fn running_anchor() -> LogAnchorRow {
        LogAnchorRow {
            tenant_id: tenant().as_str().to_string(),
            region: "eu-west".into(),
            run_id: RUN_ID.into(),
            job_id: JOB_ID.into(),
            step_id: SINGLE_STEP_NO.to_string(),
            byte_start: 0,
            byte_end: None,
            status: AnchorStatus::Running,
        }
    }

    fn sealed_segment() -> LogSegmentRow {
        LogSegmentRow {
            tenant_id: tenant().as_str().to_string(),
            region: "eu-west".into(),
            run_id: RUN_ID.into(),
            job_id: JOB_ID.into(),
            segment_seq: 0,
            blob_ref: Some(ContentHash::blake3(b"one log frame").to_multihash_string()),
            byte_start: 0,
            byte_end: 13,
            pii_key_ref: format!("kms://{}/0/tenant", tenant().as_str()),
        }
    }

    fn checkpoint() -> FlushedJobLogs {
        FlushedJobLogs {
            run_id: RUN_ID.into(),
            job_id: JOB_ID.into(),
            segments: vec![sealed_segment()],
            anchors: vec![running_anchor()],
            pointers: vec![],
        }
    }

    #[test]
    fn checkpoint_validation_accepts_one_canonical_tenant_bound_batch() {
        let mut checkpoint = checkpoint();
        checkpoint.pointers.push(
            LogAvailablePointer::new(
                LogCoord::new(RUN_ID, JOB_ID, SINGLE_STEP_NO),
                0,
                13,
                Some(ContentHash::blake3(b"one log frame")),
            )
            .expect("canonical pointer"),
        );

        let scope = validate_checkpoint(&tenant(), &checkpoint)
            .expect("the complete checkpoint is valid")
            .expect("the batch is not empty");
        assert_eq!(scope.region, "eu-west");
        assert_eq!(scope.run.to_string(), RUN_ID);
        assert_eq!(scope.job.to_string(), JOB_ID);
    }

    #[test]
    fn pointer_only_and_foreign_pointer_batches_are_refused_before_io() {
        let pointer =
            LogAvailablePointer::new(LogCoord::new(RUN_ID, JOB_ID, SINGLE_STEP_NO), 0, 1, None)
                .expect("canonical pointer");
        let pointer_only = FlushedJobLogs {
            run_id: RUN_ID.into(),
            job_id: JOB_ID.into(),
            segments: vec![],
            anchors: vec![],
            pointers: vec![pointer],
        };
        assert!(validate_checkpoint(&tenant(), &pointer_only)
            .unwrap_err()
            .to_string()
            .contains("without a regional index row"));

        let mut foreign = checkpoint();
        foreign.pointers.push(
            LogAvailablePointer::new(
                LogCoord::new(RUN_ID, OTHER_JOB_ID, SINGLE_STEP_NO),
                0,
                1,
                None,
            )
            .expect("well-formed but foreign pointer"),
        );
        assert!(validate_checkpoint(&tenant(), &foreign)
            .unwrap_err()
            .to_string()
            .contains("foreign availability pointer"));
    }

    #[test]
    fn malformed_segment_and_anchor_rows_are_refused_before_io() {
        let mut malformed_segment = checkpoint();
        malformed_segment.segments[0].blob_ref = Some("blake3:not-canonical".into());
        assert!(validate_checkpoint(&tenant(), &malformed_segment)
            .unwrap_err()
            .to_string()
            .contains("foreign or invalid segment"));

        let mut foreign_key = checkpoint();
        foreign_key.segments[0].pii_key_ref = "kms://tenant-b/0/tenant".into();
        assert!(validate_checkpoint(&tenant(), &foreign_key)
            .unwrap_err()
            .to_string()
            .contains("foreign or invalid segment"));

        let mut malformed_anchor = checkpoint();
        malformed_anchor.anchors[0].byte_end = Some(13);
        assert!(validate_checkpoint(&tenant(), &malformed_anchor)
            .unwrap_err()
            .to_string()
            .contains("foreign or invalid anchor"));
    }
}
