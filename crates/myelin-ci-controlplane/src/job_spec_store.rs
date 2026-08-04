use myelin_ci_sandbox::{JobSpecTemplate, TrustTier};
use myelin_storage::{with_tenant_tx, with_tenant_tx_error, PgError};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::job_queue_store::{parse_id, trust_token};
use crate::scheduler::{EnqueueOutcome, INSERT_JOB_QUEUE_QUERY};
use crate::DurableEnqueue;

pub const MAX_JOB_TIMEOUT_SECS: u32 = 6 * 60 * 60;

pub const INSERT_JOB_SPEC_QUERY: &str = "\
INSERT INTO ci_job_spec (tenant_id, region, job_id, run_id, idem_token, spec, stage)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (tenant_id, job_id) DO NOTHING
RETURNING job_id";

pub const SELECT_JOB_SPEC_QUERY: &str = "\
SELECT spec FROM ci_job_spec WHERE tenant_id = $1 AND job_id = $2";

pub const SELECT_JOB_SPEC_IDENTITY_QUERY: &str = "\
SELECT run_id::text AS run_id, idem_token, stage, spec
FROM ci_job_spec WHERE tenant_id = $1 AND job_id = $2";

const SELECT_EXACT_DISPATCH_QUERY: &str = "\
SELECT q.region, q.job_id::text AS queue_job_id, q.run_id::text AS queue_run_id,
       q.lane, q.labels, q.trust_tier, q.concurrency_group, q.fair_key,
       q.idem_token AS queue_idem_token, q.stage AS queue_stage,
       q.claim_window_secs AS queue_claim_window_secs,
       q.reservation_write_version AS queue_reservation_write_version,
       s.region AS spec_region, s.run_id::text AS spec_run_id,
       s.idem_token AS spec_idem_token, s.spec, s.stage AS spec_stage
FROM job_queue q
JOIN ci_job_spec s ON s.tenant_id = q.tenant_id AND s.job_id = q.job_id
WHERE q.tenant_id = $1 AND q.idem_token = $2";

pub const NON_TERMINAL_NULL_STAGE_JOBS_QUERY: &str = "\
SELECT count(*) FROM job_queue q \
WHERE q.region = $1 AND q.state <> 'terminal' AND q.stage IS NULL";

pub const NON_TERMINAL_NULL_CLAIM_WINDOW_JOBS_QUERY: &str = "\
SELECT count(*) FROM job_queue q \
WHERE q.region = $1 AND q.state <> 'terminal' AND q.claim_window_secs IS NULL";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableCiJobLaunchTemplate {
    pub spec: JobSpecTemplate,
    pub project_id: String,
    pub ci_run_id: String,
    pub token_authority_handle: String,
}

#[derive(Debug)]
pub enum CiJobSpecStoreError {
    Db(String),
    BadId {
        field: &'static str,
        value: String,
    },
    SpecNotFound {
        tenant_id: String,
        job_id: String,
    },
    CorruptSpec {
        job_id: String,
        detail: String,
    },
    SpecEncode(String),
    TrustTierMismatch {
        enqueue: &'static str,
        spec: &'static str,
    },
    TimeoutTooLong {
        requested: u32,
        ceiling: u32,
    },
    MissingStage {
        job_id: String,
    },
    ClaimWindowMismatch {
        enqueue: i64,
        spec: i64,
    },
    ClaimWindowUnderivable(String),
}

impl core::fmt::Display for CiJobSpecStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiJobSpecStoreError::Db(e) => write!(f, "durable ci_job_spec store error: {e}"),
            CiJobSpecStoreError::BadId { field, value } => write!(
                f,
                "durable ci_job_spec op refused: {field} `{value}` is not a UUID (the \
                 ci_job_spec.{field} column is uuid - CI job/run ids are uuids in production)"
            ),
            CiJobSpecStoreError::SpecNotFound { tenant_id, job_id } => write!(
                f,
                "no durable launch template for tenant `{tenant_id}` job `{job_id}` - the runner \
                 cannot resolve the leased job (fail-closed; the row stays leased for the reaper)"
            ),
            CiJobSpecStoreError::CorruptSpec { job_id, detail } => write!(
                f,
                "corrupt durable launch template for job `{job_id}` (jsonb decode failed closed): \
                 {detail}"
            ),
            CiJobSpecStoreError::SpecEncode(e) => {
                write!(f, "durable ci_job_spec persist refused: launch template did not serialize to jsonb: {e}")
            }
            CiJobSpecStoreError::TrustTierMismatch { enqueue, spec } => write!(
                f,
                "durable dispatch refused (SECURITY): the job_queue row's trust_tier `{enqueue}` does \
                 not match the dispatched spec's trust_tier `{spec}` - the claim-gating tier MUST come \
                 from the spec that executes (no widening/defaulting)"
            ),
            CiJobSpecStoreError::TimeoutTooLong { requested, ceiling } => write!(
                f,
                "durable dispatch refused: spec timeout_secs {requested} exceeds the {ceiling}s \
                 ceiling - a job may not outlive the runner's lease (double-run guard, fail-closed)"
            ),
            CiJobSpecStoreError::MissingStage { job_id } => write!(
                f,
                "durable ci_job_spec for job `{job_id}` has a NULL stage (a pre-rewire historical row) \
                 - the reporter fails closed rather than attribute a verdict to an unknown stage"
            ),
            CiJobSpecStoreError::ClaimWindowMismatch { enqueue, spec } => write!(
                f,
                "durable dispatch refused: the job_queue row's claim_window_secs {enqueue} does not \
                 match the {spec}s window the dispatched spec's own topology derives - the immutable \
                 claim ceiling MUST come from the spec that executes (no widening/defaulting)"
            ),
            CiJobSpecStoreError::ClaimWindowUnderivable(detail) => write!(
                f,
                "durable dispatch refused: {detail}"
            ),
        }
    }
}

impl std::error::Error for CiJobSpecStoreError {}

impl From<PgError> for CiJobSpecStoreError {
    fn from(error: PgError) -> Self {
        Self::from_pg(error)
    }
}

impl CiJobSpecStoreError {
    fn from_pg(e: PgError) -> Self {
        CiJobSpecStoreError::Db(e.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub enqueue: EnqueueOutcome,
    pub spec_inserted: bool,
}

#[derive(Clone)]
pub struct CiJobSpecStore {
    pool: PgPool,
}

impl CiJobSpecStore {
    pub fn with_pg(pool: PgPool) -> CiJobSpecStore {
        CiJobSpecStore { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn co_persist_dispatch(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        self.co_persist_dispatch_inner(enq, launch, stage, false)
            .await
    }

    pub async fn co_persist_active_flow_dispatch(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        self.co_persist_dispatch_inner(enq, launch, stage, true)
            .await
    }

    async fn co_persist_dispatch_inner(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
        require_active_flow: bool,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        validate_dispatch(enq.trust_tier, Some(enq.claim_window_secs), launch)?;

        let job_uuid = parse_id_local("job_id", &enq.job_id)?;
        let run_uuid = parse_id_local("run_id", &enq.run_id)?;
        let spec_json = serde_json::to_value(launch)
            .map_err(|e| CiJobSpecStoreError::SpecEncode(e.to_string()))?;
        let stage = stage.to_string();

        let labels = enq.labels.clone();
        let group = enq.concurrency_group.clone();
        let lane = enq.lane.as_str();
        let trust = trust_token(enq.trust_tier);
        let tenant_id = enq.tenant_id.clone();
        let region = enq.region.clone();
        let fair_key = enq.fair_key.clone();
        let idem = enq.idem_token.clone();
        let workflow_run_id = enq.run_id.clone();
        let claim_window_secs = enq.claim_window_secs;
        let reservation_write_version = enq.reservation_write_version.value();
        if enq.stage != stage {
            return Err(CiJobSpecStoreError::Db(
                "durable dispatch stage differs between queue authority and spec identity".into(),
            ));
        }

        let (enqueued, spec_inserted) =
            with_tenant_tx(&self.pool, &enq.tenant_id, &enq.region, move |conn| {
                Box::pin(async move {
                    if require_active_flow {
                        let state = sqlx::query_scalar::<_, String>(
                            "SELECT state FROM workflow_run \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
                        )
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(&workflow_run_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                        if state.as_deref() != Some("running") {
                            return Err(PgError::Query(
                                "manifest dispatch refused: owning Flow run is not active".into(),
                            ));
                        }
                    }
                    let jq_row = sqlx::query(INSERT_JOB_QUEUE_QUERY)
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(job_uuid)
                        .bind(run_uuid)
                        .bind(lane)
                        .bind(&labels)
                        .bind(trust)
                        .bind(group.as_deref())
                        .bind(&fair_key)
                        .bind(&idem)
                        .bind(&stage)
                        .bind(claim_window_secs)
                        .bind(reservation_write_version)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    let spec_row = sqlx::query(INSERT_JOB_SPEC_QUERY)
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(job_uuid)
                        .bind(run_uuid)
                        .bind(&idem)
                        .bind(&spec_json)
                        .bind(&stage)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    let exact = sqlx::query(SELECT_EXACT_DISPATCH_QUERY)
                        .bind(&tenant_id)
                        .bind(&idem)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?
                        .ok_or_else(|| {
                            PgError::Query(
                                "durable dispatch replay readback found no joined queue/spec row"
                                    .into(),
                            )
                        })?;
                    verify_exact_dispatch(
                        &exact,
                        &region,
                        job_uuid,
                        run_uuid,
                        lane,
                        &labels,
                        trust,
                        group.as_deref(),
                        &fair_key,
                        &idem,
                        &stage,
                        claim_window_secs,
                        reservation_write_version,
                        &spec_json,
                    )?;
                    Ok((jq_row.is_some(), spec_row.is_some()))
                })
            })
            .await
            .map_err(CiJobSpecStoreError::from_pg)?;

        Ok(DispatchOutcome {
            enqueue: if enqueued {
                EnqueueOutcome::Inserted
            } else {
                EnqueueOutcome::DuplicateIdem
            },
            spec_inserted,
        })
    }

    pub async fn get_launch_template(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let row = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(SELECT_JOB_SPEC_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(job_uuid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))
            })
        })
        .await
        .map_err(CiJobSpecStoreError::from_pg)?;
        let row = row.ok_or_else(|| CiJobSpecStoreError::SpecNotFound {
            tenant_id: tenant_id.to_string(),
            job_id: job_id.to_string(),
        })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|error| CiJobSpecStoreError::Db(error.to_string()))?;
        decode_launch_template(job_id, spec_json)
    }

    pub(crate) async fn get_launch_template_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let row = sqlx::query(SELECT_JOB_SPEC_QUERY)
            .bind(tenant_id)
            .bind(job_uuid)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| CiJobSpecStoreError::Db(error.to_string()))?
            .ok_or_else(|| CiJobSpecStoreError::SpecNotFound {
                tenant_id: tenant_id.to_string(),
                job_id: job_id.to_string(),
            })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        decode_launch_template(job_id, spec_json)
    }

    pub async fn get_dispatch_identity(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<Option<ClaimedDispatchIdentity>, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let job_id_owned = job_id.to_string();
        let store = self.clone();
        let row = with_tenant_tx_error(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                store
                    .get_dispatch_identity_on_conn(conn, &tenant_id_owned, job_uuid, &job_id_owned)
                    .await
            })
        })
        .await?;
        Ok(row)
    }

    pub(crate) async fn get_dispatch_identity_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant_id: &str,
        job_id: Uuid,
        job_id_text: &str,
    ) -> Result<Option<ClaimedDispatchIdentity>, CiJobSpecStoreError> {
        let row = sqlx::query(SELECT_JOB_SPEC_IDENTITY_QUERY)
            .bind(tenant_id)
            .bind(job_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let run_id: String = row
            .try_get("run_id")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let idem_token: String = row
            .try_get("idem_token")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let stage: Option<String> = row
            .try_get("stage")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let stage = stage.ok_or_else(|| CiJobSpecStoreError::MissingStage {
            job_id: job_id_text.to_string(),
        })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let launch = decode_launch_template(job_id_text, spec_json)?;
        Ok(Some(ClaimedDispatchIdentity {
            run_id,
            idem_token,
            stage,
            reserve_handle: launch.spec.meter_to.reserve_id,
        }))
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_exact_dispatch(
    row: &sqlx::postgres::PgRow,
    region: &str,
    job_id: Uuid,
    run_id: Uuid,
    lane: &str,
    labels: &[String],
    trust: &str,
    concurrency_group: Option<&str>,
    fair_key: &str,
    idem_token: &str,
    stage: &str,
    claim_window_secs: i64,
    reservation_write_version: Option<i16>,
    spec: &serde_json::Value,
) -> Result<(), PgError> {
    let exact = row.get::<Option<i64>, _>("queue_claim_window_secs") == Some(claim_window_secs)
        && row.get::<Option<i16>, _>("queue_reservation_write_version")
            == reservation_write_version
        && row.get::<String, _>("region") == region
        && row.get::<String, _>("queue_job_id") == job_id.to_string()
        && row.get::<String, _>("queue_run_id") == run_id.to_string()
        && row.get::<String, _>("lane") == lane
        && row.get::<Vec<String>, _>("labels") == labels
        && row.get::<String, _>("trust_tier") == trust
        && row.get::<Option<String>, _>("concurrency_group").as_deref() == concurrency_group
        && row.get::<String, _>("fair_key") == fair_key
        && row.get::<String, _>("queue_idem_token") == idem_token
        && row.get::<Option<String>, _>("queue_stage").as_deref() == Some(stage)
        && row.get::<String, _>("spec_region") == region
        && row.get::<String, _>("spec_run_id") == run_id.to_string()
        && row.get::<String, _>("spec_idem_token") == idem_token
        && row.get::<serde_json::Value, _>("spec") == *spec
        && row.get::<Option<String>, _>("spec_stage").as_deref() == Some(stage);
    if exact {
        Ok(())
    } else {
        Err(PgError::Query(
            "durable dispatch replay conflicts with the existing queue/spec identity".into(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedDispatchIdentity {
    pub run_id: String,
    pub idem_token: String,
    pub stage: String,
    pub reserve_handle: String,
}

fn validate_dispatch(
    enq_trust: TrustTier,
    enq_claim_window_secs: Option<i64>,
    launch: &DurableCiJobLaunchTemplate,
) -> Result<(), CiJobSpecStoreError> {
    if launch.token_authority_handle.trim().is_empty() || launch.token_authority_handle.len() > 512
    {
        return Err(CiJobSpecStoreError::SpecEncode(
            "token authority handle is empty or overlong".into(),
        ));
    }
    if enq_trust != launch.spec.trust_tier {
        return Err(CiJobSpecStoreError::TrustTierMismatch {
            enqueue: trust_token(enq_trust),
            spec: trust_token(launch.spec.trust_tier),
        });
    }
    if launch.spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        return Err(CiJobSpecStoreError::TimeoutTooLong {
            requested: launch.spec.limits.timeout_secs,
            ceiling: MAX_JOB_TIMEOUT_SECS,
        });
    }
    if let Some(declared) = enq_claim_window_secs {
        let derived = crate::ci_claim_window::claim_window_secs_for_template(&launch.spec)
            .map_err(|error| CiJobSpecStoreError::ClaimWindowUnderivable(error.to_string()))?;
        if declared != derived {
            return Err(CiJobSpecStoreError::ClaimWindowMismatch {
                enqueue: declared,
                spec: derived,
            });
        }
    }
    Ok(())
}

fn decode_launch_template(
    job_id: &str,
    spec_json: serde_json::Value,
) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
    let launch = serde_json::from_value::<DurableCiJobLaunchTemplate>(spec_json).map_err(|e| {
        CiJobSpecStoreError::CorruptSpec {
            job_id: job_id.to_string(),
            detail: e.to_string(),
        }
    })?;
    validate_dispatch(launch.spec.trust_tier, None, &launch)?;
    Ok(launch)
}

fn parse_id_local(
    field: &'static str,
    value: &str,
) -> Result<sqlx::types::Uuid, CiJobSpecStoreError> {
    parse_id(field, value).map_err(|_| CiJobSpecStoreError::BadId {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
#[path = "job_spec_store_tests.rs"]
mod tests;
