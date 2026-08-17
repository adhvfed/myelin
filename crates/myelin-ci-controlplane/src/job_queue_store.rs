use std::time::Duration;

use myelin_ci_sandbox::TrustTier;
use myelin_storage::{with_tenant_tx, PgError};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgArguments, PgPool};
use sqlx::query::Query;
use sqlx::types::Uuid;
use sqlx::{Acquire, Postgres, Row};

use crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS;
use crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
#[cfg(any(test, feature = "test-support"))]
use crate::scheduler::CANCEL_SUPERSEDED_QUERY;
use crate::scheduler::{
    EnqueueOutcome, Lane, AUTHORIZE_JOB_LAUNCH_QUERY, COMPLETE_JOB_QUERY, CONSUME_CLAIM_QUERY,
    CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY, CONSUME_PREPARATION_CLAIM_QUERY,
    CONSUME_SECRET_WITHHELD_CLAIM_QUERY, HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY,
    READ_COMPLETION_DISPOSITION_QUERY, READ_EXHAUSTED_COMPLETION_REPLAY_QUERY,
    READ_PREPARATION_COMPLETION_REPLAY_QUERY, READ_SECRET_WITHHELD_COMPLETION_REPLAY_QUERY,
    RENEW_PREPARATION_LEASE_QUERY, REQUEUE_PREPARATION_CLAIM_QUERY,
    RESET_REQUEUED_PREPARATION_CI_JOB_SURFACE_QUERY, VERIFY_JOB_LAUNCH_LIVE_QUERY,
};

#[derive(Debug)]
pub enum JobQueueStoreError {
    InvalidInput(String),
    Db(String),
    BadId { field: &'static str, value: String },
    CorruptRow(String),
}

impl core::fmt::Display for JobQueueStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            JobQueueStoreError::InvalidInput(e) => {
                write!(f, "durable job_queue input refused: {e}")
            }
            JobQueueStoreError::Db(e) => write!(f, "durable job_queue store error: {e}"),
            JobQueueStoreError::BadId { field, value } => write!(
                f,
                "durable job_queue op refused: {field} `{value}` is not a UUID (the \
                 job_queue.{field} column is uuid - CI job/run ids are uuids in production)"
            ),
            JobQueueStoreError::CorruptRow(e) => {
                write!(
                    f,
                    "corrupt durable job_queue row (outside the frozen token set): {e}"
                )
            }
        }
    }
}

impl std::error::Error for JobQueueStoreError {}

impl JobQueueStoreError {
    pub(crate) fn from_pg(e: PgError) -> Self {
        JobQueueStoreError::Db(e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct DurableEnqueue {
    pub tenant_id: String,
    pub region: String,
    pub job_id: String,
    pub run_id: String,
    pub lane: Lane,
    pub labels: Vec<String>,
    pub trust_tier: TrustTier,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
    pub idem_token: String,
    pub stage: String,
    pub claim_window_secs: i64,
    pub reservation_write_version: crate::ReservationWriteVersionMarker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeasedJob {
    pub tenant_id: String,
    pub job_id: Uuid,
    pub run_id: Uuid,
    pub lane: Lane,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
    pub trust_tier: TrustTier,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub claim_window_secs: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobLaunchClaim {
    pub tenant_id: String,
    pub region: String,
    pub wf_run_id: String,
    pub job_id: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LockedJobClaim {
    pub state: String,
    pub idem_token: String,
    pub stage: Option<String>,
    pub trust_tier: String,
    pub lease_owner: Option<String>,
    pub lease_epoch: i64,
    pub claim_nonce: Option<String>,
    pub claim_started_at_epoch_secs: Option<i64>,
    pub claim_expires_at_epoch_secs: Option<i64>,
    pub claim_window_secs: Option<i64>,
    pub claim_is_live: bool,
}

pub const LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY: &str = "\
SELECT state, idem_token, stage, trust_tier, lease_owner, lease_epoch,
       claim_nonce::text AS claim_nonce, claim_window_secs,
       FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint AS claim_started_at_epoch_secs,
       FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint AS claim_expires_at_epoch_secs,
       COALESCE(claim_expires_at > statement_timestamp(), false) AS claim_is_live
FROM job_queue
WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND run_id = $4::uuid
FOR UPDATE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimConsumeOutcome {
    Consumed,
    AlreadyConsumed,
    Refused,
}

pub(crate) struct ClaimConsumeSpec<'a> {
    pub tenant_id: &'a str,
    pub job_id: Uuid,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: Uuid,
    pub stage: &'a str,
    pub completion_receipt: &'a str,
    pub alternate_replay_receipt: Option<&'a str>,
}

pub(crate) struct PreparationClaimConsumeSpec<'a> {
    pub tenant_id: &'a str,
    pub region: &'a str,
    pub job_id: Uuid,
    pub wf_run_id: Uuid,
    pub ci_run_id: Uuid,
    pub idem_token: &'a str,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: Uuid,
    pub stage: &'a str,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub reserve_handle: &'a str,
    pub completion_receipt: &'a str,
}

#[derive(Clone, Copy)]
enum PreparationClaimResolution {
    Prepared,
    SecretWithheld,
    AttemptsExhausted,
}

impl PreparationClaimResolution {
    fn mutation_query(self) -> &'static str {
        match self {
            PreparationClaimResolution::Prepared => CONSUME_PREPARATION_CLAIM_QUERY,
            PreparationClaimResolution::SecretWithheld => CONSUME_SECRET_WITHHELD_CLAIM_QUERY,
            PreparationClaimResolution::AttemptsExhausted => {
                CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY
            }
        }
    }

    fn replay_query(self) -> &'static str {
        match self {
            PreparationClaimResolution::Prepared => READ_PREPARATION_COMPLETION_REPLAY_QUERY,
            PreparationClaimResolution::SecretWithheld => {
                READ_SECRET_WITHHELD_COMPLETION_REPLAY_QUERY
            }
            PreparationClaimResolution::AttemptsExhausted => READ_EXHAUSTED_COMPLETION_REPLAY_QUERY,
        }
    }
}

fn bind_preparation_claim<'query, 'spec: 'query>(
    sql: &'query str,
    spec: &'query PreparationClaimConsumeSpec<'spec>,
) -> Query<'query, Postgres, PgArguments> {
    sqlx::query(sql)
        .bind(spec.tenant_id)
        .bind(spec.region)
        .bind(spec.job_id)
        .bind(spec.wf_run_id)
        .bind(spec.idem_token)
        .bind(spec.lease_owner)
        .bind(spec.lease_epoch)
        .bind(spec.claim_nonce)
        .bind(spec.stage)
        .bind(spec.claim_started_at_epoch_secs)
        .bind(spec.claim_expires_at_epoch_secs)
        .bind(spec.ci_run_id)
        .bind(spec.reserve_handle)
        .bind(spec.completion_receipt)
}

fn bind_preparation_completion_replay<'query, 'spec: 'query>(
    sql: &'query str,
    spec: &'query PreparationClaimConsumeSpec<'spec>,
) -> Query<'query, Postgres, PgArguments> {
    sqlx::query(sql)
        .bind(spec.tenant_id)
        .bind(spec.region)
        .bind(spec.job_id)
        .bind(spec.wf_run_id)
        .bind(spec.idem_token)
        .bind(spec.stage)
        .bind(spec.completion_receipt)
        .bind(spec.ci_run_id)
        .bind(spec.reserve_handle)
        .bind(spec.lease_owner)
        .bind(spec.lease_epoch)
        .bind(spec.claim_nonce)
        .bind(spec.claim_started_at_epoch_secs)
        .bind(spec.claim_expires_at_epoch_secs)
}

fn bind_exhausted_completion_replay<'query, 'spec: 'query>(
    sql: &'query str,
    spec: &'query PreparationClaimConsumeSpec<'spec>,
) -> Query<'query, Postgres, PgArguments> {
    sqlx::query(sql)
        .bind(spec.tenant_id)
        .bind(spec.region)
        .bind(spec.job_id)
        .bind(spec.wf_run_id)
        .bind(spec.idem_token)
        .bind(spec.stage)
        .bind(spec.completion_receipt)
        .bind(spec.ci_run_id)
        .bind(spec.reserve_handle)
}

pub(crate) struct PreparationRequeueSpec<'a> {
    pub tenant_id: &'a str,
    pub region: &'a str,
    pub job_id: &'a str,
    pub wf_run_id: &'a str,
    pub ci_run_id: &'a str,
    pub idem_token: &'a str,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: &'a str,
    pub stage: &'a str,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub reserve_handle: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparationRequeueOutcome {
    Requeued,
    NoOp,
}

#[derive(Clone)]
pub struct CiJobQueueStore {
    pub(crate) pool: PgPool,
}

pub(crate) struct RetainedCiJobLaunch {
    connection: Option<PoolConnection<Postgres>>,
    lock_key: i64,
}

impl RetainedCiJobLaunch {
    pub(crate) async fn validate(&mut self) -> Result<(), JobQueueStoreError> {
        let Some(connection) = self.connection.as_mut() else {
            return Err(JobQueueStoreError::Db(
                "launch ownership has no retained database session".into(),
            ));
        };
        // @tenant-cross-scope: this inspects only the current PostgreSQL session's advisory-lock
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM pg_locks
                WHERE pid = pg_backend_pid()
                  AND locktype = 'advisory'
                  AND granted
                  AND objsubid = 1
                  AND ((classid::bigint << 32) | objid::bigint) = $1
             )",
        )
        .bind(self.lock_key)
        .fetch_one(&mut **connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("validate launch session lock: {error}"))
        })?;
        if !owned {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch session lock was lost before sandbox release".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn release(mut self) -> Result<(), JobQueueStoreError> {
        let Some(mut connection) = self.connection.take() else {
            return Err(JobQueueStoreError::Db(
                "launch ownership has no retained database session to release".into(),
            ));
        };
        // @tenant-cross-scope: this releases one PostgreSQL session advisory lock by its derived
        let released: bool = sqlx::query_scalar(
            "SELECT pg_advisory_unlock($1) /* tenant_id generation verified by launch CAS */",
        )
        .bind(self.lock_key)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("validate/release launch session lock: {error}"))
        })?;
        if !released {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch session lock was lost during sandbox release".into(),
            ));
        }
        Ok(())
    }
}

impl Drop for RetainedCiJobLaunch {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            connection.close_on_drop();
        }
    }
}

impl CiJobQueueStore {
    pub fn with_pg(pool: PgPool) -> CiJobQueueStore {
        CiJobQueueStore { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub(crate) async fn lock_for_token_mint_on_conn(
        connection: &mut sqlx::PgConnection,
        tenant_id: &str,
        region: &str,
        job_id: &str,
        run_id: &str,
    ) -> Result<Option<LockedJobClaim>, JobQueueStoreError> {
        let row = sqlx::query(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY)
            .bind(tenant_id)
            .bind(region)
            .bind(job_id)
            .bind(run_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| {
                JobQueueStoreError::Db(format!("lock job claim for token mint: {error}"))
            })?;
        Ok(row.map(|row| LockedJobClaim {
            state: row.get("state"),
            idem_token: row.get("idem_token"),
            stage: row.get("stage"),
            trust_tier: row.get("trust_tier"),
            lease_owner: row.get("lease_owner"),
            lease_epoch: row.get("lease_epoch"),
            claim_nonce: row.get("claim_nonce"),
            claim_started_at_epoch_secs: row.get("claim_started_at_epoch_secs"),
            claim_expires_at_epoch_secs: row.get("claim_expires_at_epoch_secs"),
            claim_window_secs: row.get("claim_window_secs"),
            claim_is_live: row.get("claim_is_live"),
        }))
    }

    pub async fn enqueue(
        &self,
        job: &DurableEnqueue,
    ) -> Result<EnqueueOutcome, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", &job.job_id)?;
        let run_uuid = parse_id("run_id", &job.run_id)?;
        let labels = job.labels.clone();
        let group = job.concurrency_group.clone();
        let lane = job.lane.as_str();
        let trust = trust_token(job.trust_tier);
        let tenant_id = job.tenant_id.clone();
        let region = job.region.clone();
        let fair_key = job.fair_key.clone();
        let idem = job.idem_token.clone();
        let stage = job.stage.clone();
        let claim_window_secs = job.claim_window_secs;
        let reservation_write_version = job.reservation_write_version.value();
        if !(1..=MAX_CI_JOB_CLAIM_WINDOW_SECS as i64).contains(&claim_window_secs) {
            return Err(JobQueueStoreError::InvalidInput(format!(
                "claim window {claim_window_secs}s is outside the durable 1..={MAX_CI_JOB_CLAIM_WINDOW_SECS}s bound"
            )));
        }
        let inserted = with_tenant_tx(&self.pool, &job.tenant_id, &job.region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(INSERT_JOB_QUEUE_QUERY)
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
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(if inserted {
            EnqueueOutcome::Inserted
        } else {
            EnqueueOutcome::DuplicateIdem
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn cancel_superseded(
        &self,
        tenant_id: &str,
        region: &str,
        group: &str,
        keep_job_id: &str,
    ) -> Result<Vec<Uuid>, JobQueueStoreError> {
        let keep_uuid = parse_id("job_id", keep_job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let group_owned = group.to_string();
        let rows = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(CANCEL_SUPERSEDED_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&group_owned)
                    .bind(keep_uuid)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(r.get::<Uuid, _>("job_id"));
        }
        Ok(out)
    }

    pub async fn complete(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<bool, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let moved = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(COMPLETE_JOB_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(job_uuid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(moved)
    }

    pub async fn authorize_launch(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<bool, JobQueueStoreError> {
        let Some(mut launch) = self.authorize_launch_retained(claim).await? else {
            return Ok(false);
        };
        launch.validate().await?;
        launch.release().await?;
        Ok(true)
    }

    pub(crate) async fn authorize_launch_retained(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<Option<RetainedCiJobLaunch>, JobQueueStoreError> {
        self.authorize_launch_retained_inner(claim, None).await
    }

    pub async fn authorize_launch_v2(
        &self,
        claim: &CiJobLaunchClaim,
        generation: &crate::ci_credential_generation::CiPhaseGenerationGate,
    ) -> Result<bool, JobQueueStoreError> {
        let Some(mut launch) = self.authorize_launch_v2_retained(claim, generation).await? else {
            return Ok(false);
        };
        launch.validate().await?;
        launch.release().await?;
        Ok(true)
    }

    pub(crate) async fn authorize_launch_v2_retained(
        &self,
        claim: &CiJobLaunchClaim,
        generation: &crate::ci_credential_generation::CiPhaseGenerationGate,
    ) -> Result<Option<RetainedCiJobLaunch>, JobQueueStoreError> {
        if generation.purpose != crate::ci_credential_generation::CiCredentialPurpose::Workload {
            return Err(JobQueueStoreError::Db(
                "the V2 launch fence accepts only a workload credential generation".into(),
            ));
        }
        self.authorize_launch_retained_inner(claim, Some(generation))
            .await
    }

    async fn authorize_launch_retained_inner(
        &self,
        claim: &CiJobLaunchClaim,
        generation: Option<&crate::ci_credential_generation::CiPhaseGenerationGate>,
    ) -> Result<Option<RetainedCiJobLaunch>, JobQueueStoreError> {
        let job_id = parse_id("job_id", &claim.job_id)?;
        let wf_run_id = parse_id("run_id", &claim.wf_run_id)?;
        let claim_nonce = parse_id("claim_nonce", &claim.claim_nonce)?;
        let mut connection = self.pool.acquire().await.map_err(|error| {
            JobQueueStoreError::Db(format!("acquire launch fence session: {error}"))
        })?;
        // @tenant-cross-scope: PostgreSQL session-lock state is connection infrastructure, not a
        let clean_session: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (
                SELECT 1
                FROM pg_locks
                WHERE pid = pg_backend_pid() AND locktype = 'advisory'
             )",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("inspect launch fence session: {error}"))
        })?;
        if !clean_session {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch fence session retained an advisory lock; refusing re-entrant ownership"
                    .into(),
            ));
        }
        let mut transaction = connection.begin().await.map_err(|error| {
            JobQueueStoreError::Db(format!("begin launch fence transaction: {error}"))
        })?;
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(&claim.tenant_id)
        .bind(&claim.region)
        .execute(&mut *transaction)
        .await
        .map_err(|error| JobQueueStoreError::Db(format!("scope retained launch fence: {error}")))?;
        let launch_query = match generation {
            None => AUTHORIZE_JOB_LAUNCH_QUERY,
            Some(_) => crate::scheduler::AUTHORIZE_JOB_LAUNCH_V2_QUERY,
        };
        let mut query = sqlx::query(launch_query)
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(job_id)
            .bind(wf_run_id)
            .bind(&claim.lease_owner)
            .bind(claim.lease_epoch)
            .bind(claim_nonce)
            .bind(claim.claim_started_at_epoch_secs)
            .bind(claim.claim_expires_at_epoch_secs)
            .bind(CI_RUNNER_EXECUTION_LEASE_TTL_SECS);
        if let Some(generation) = generation {
            let ci_run_id = parse_id("ci_run_id", &generation.ci_run_id)?;
            query = query
                .bind(generation.binding_version)
                .bind(generation.generation_id.clone())
                .bind(generation.jti.clone())
                .bind(generation.issued_at_epoch_secs)
                .bind(generation.expires_at_epoch_secs)
                .bind(ci_run_id)
                .bind(generation.token_authority_handle.clone())
                .bind(generation.idem_token.clone())
                .bind(generation.checkout_commit.clone());
        }
        let row = query
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| JobQueueStoreError::Db(format!("authorize launch fence: {error}")))?;
        if row.is_none() {
            transaction.rollback().await.map_err(|error| {
                JobQueueStoreError::Db(format!("rollback refused launch fence: {error}"))
            })?;
            return Ok(None);
        }
        let lock_key: i64 = sqlx::query_scalar(
            "SELECT hashtextextended(
                jsonb_build_array($1::text, $2::text, $3::text, $4::text, $5::text)::text,
                0
             )",
        )
        .bind(&claim.tenant_id)
        .bind(&claim.region)
        .bind(job_id)
        .bind(claim.lease_epoch)
        .bind(claim_nonce)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| JobQueueStoreError::Db(format!("derive launch session lock: {error}")))?;
        // @tenant-cross-scope: this acquires one PostgreSQL session advisory lock by its derived
        let locked_result: Result<bool, sqlx::Error> = sqlx::query_scalar(
            "SELECT pg_try_advisory_lock($1) /* tenant_id generation verified by launch CAS */",
        )
        .bind(lock_key)
        .fetch_one(&mut *transaction)
        .await;
        let locked = match locked_result {
            Ok(locked) => locked,
            Err(error) => {
                let _ = transaction.rollback().await;
                connection.close_on_drop();
                return Err(JobQueueStoreError::Db(format!(
                    "acquire launch session lock: {error}"
                )));
            }
        };
        if !locked {
            transaction.rollback().await.map_err(|error| {
                JobQueueStoreError::Db(format!("rollback colliding launch fence: {error}"))
            })?;
            return Err(JobQueueStoreError::Db(
                "launch session lock is already owned; refusing a colliding or duplicate launch"
                    .into(),
            ));
        }
        if let Err(error) = transaction.commit().await {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(format!(
                "commit launch fence: {error}"
            )));
        }
        Ok(Some(RetainedCiJobLaunch {
            connection: Some(connection),
            lock_key,
        }))
    }

    pub async fn renew_preparation_lease(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<bool, JobQueueStoreError> {
        let job_id = parse_id("job_id", &claim.job_id)?;
        let wf_run_id = parse_id("run_id", &claim.wf_run_id)?;
        let claim_nonce = parse_id("claim_nonce", &claim.claim_nonce)?;
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        let lease_owner = claim.lease_owner.clone();
        let lease_epoch = claim.lease_epoch;
        let claim_started_at_epoch_secs = claim.claim_started_at_epoch_secs;
        let claim_expires_at_epoch_secs = claim.claim_expires_at_epoch_secs;
        let execution_lease = CI_RUNNER_EXECUTION_LEASE_TTL_SECS.to_string();
        let renewed = with_tenant_tx(&self.pool, &claim.tenant_id, &claim.region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(RENEW_PREPARATION_LEASE_QUERY)
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(job_id)
                    .bind(wf_run_id)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(claim_nonce)
                    .bind(claim_started_at_epoch_secs)
                    .bind(claim_expires_at_epoch_secs)
                    .bind(&execution_lease)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(renewed)
    }

    pub(crate) async fn verify_launch_live(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<bool, JobQueueStoreError> {
        let job_id = parse_id("job_id", &claim.job_id)?;
        let wf_run_id = parse_id("run_id", &claim.wf_run_id)?;
        let claim_nonce = parse_id("claim_nonce", &claim.claim_nonce)?;
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        let lease_owner = claim.lease_owner.clone();
        let lease_epoch = claim.lease_epoch;
        let claim_started_at_epoch_secs = claim.claim_started_at_epoch_secs;
        let claim_expires_at_epoch_secs = claim.claim_expires_at_epoch_secs;
        let live = with_tenant_tx(&self.pool, &claim.tenant_id, &claim.region, move |conn| {
            Box::pin(async move {
                let row: Option<i32> = sqlx::query_scalar(VERIFY_JOB_LAUNCH_LIVE_QUERY)
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(job_id)
                    .bind(wf_run_id)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(claim_nonce)
                    .bind(claim_started_at_epoch_secs)
                    .bind(claim_expires_at_epoch_secs)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(live)
    }

    pub(crate) async fn consume_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: ClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        let consumed = sqlx::query(CONSUME_CLAIM_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.job_id)
            .bind(spec.lease_owner)
            .bind(spec.lease_epoch)
            .bind(spec.claim_nonce)
            .bind(spec.completion_receipt)
            .bind(spec.stage)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        if consumed.is_some() {
            return Ok(ClaimConsumeOutcome::Consumed);
        }
        let disposition = sqlx::query(READ_COMPLETION_DISPOSITION_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.job_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        let Some(row) = disposition else {
            return Ok(ClaimConsumeOutcome::Refused);
        };
        let state: String = row.get("state");
        let stored_receipt: Option<String> = row.get("completion_receipt");
        if state == "terminal"
            && (stored_receipt.as_deref() == Some(spec.completion_receipt)
                || stored_receipt.as_deref() == spec.alternate_replay_receipt)
        {
            Ok(ClaimConsumeOutcome::AlreadyConsumed)
        } else {
            Ok(ClaimConsumeOutcome::Refused)
        }
    }

    async fn consume_preparation_resolution_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationClaimConsumeSpec<'_>,
        resolution: PreparationClaimResolution,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        let consumed = bind_preparation_claim(resolution.mutation_query(), &spec)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        if consumed.is_some() {
            return Ok(ClaimConsumeOutcome::Consumed);
        }

        let replay_query = match resolution {
            PreparationClaimResolution::Prepared | PreparationClaimResolution::SecretWithheld => {
                bind_preparation_completion_replay(resolution.replay_query(), &spec)
            }
            PreparationClaimResolution::AttemptsExhausted => {
                bind_exhausted_completion_replay(resolution.replay_query(), &spec)
            }
        };
        let replay = replay_query
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;

        Ok(if replay.is_some() {
            ClaimConsumeOutcome::AlreadyConsumed
        } else {
            ClaimConsumeOutcome::Refused
        })
    }

    pub(crate) async fn consume_preparation_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        Self::consume_preparation_resolution_on_conn(
            conn,
            spec,
            PreparationClaimResolution::Prepared,
        )
        .await
    }

    pub(crate) async fn consume_secret_withheld_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        Self::consume_preparation_resolution_on_conn(
            conn,
            spec,
            PreparationClaimResolution::SecretWithheld,
        )
        .await
    }

    pub(crate) async fn consume_preparation_claim_exhausted_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        Self::consume_preparation_resolution_on_conn(
            conn,
            spec,
            PreparationClaimResolution::AttemptsExhausted,
        )
        .await
    }

    pub(crate) async fn requeue_preparation_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationRequeueSpec<'_>,
    ) -> Result<PreparationRequeueOutcome, PgError> {
        let requeued = sqlx::query(REQUEUE_PREPARATION_CLAIM_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.region)
            .bind(spec.job_id)
            .bind(spec.wf_run_id)
            .bind(spec.idem_token)
            .bind(spec.lease_owner)
            .bind(spec.lease_epoch)
            .bind(spec.claim_nonce)
            .bind(spec.stage)
            .bind(spec.claim_started_at_epoch_secs)
            .bind(spec.claim_expires_at_epoch_secs)
            .bind(spec.ci_run_id)
            .bind(spec.reserve_handle)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        if requeued.is_none() {
            return Ok(PreparationRequeueOutcome::NoOp);
        }
        sqlx::query(RESET_REQUEUED_PREPARATION_CI_JOB_SURFACE_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.job_id)
            .execute(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(PreparationRequeueOutcome::Requeued)
    }

    pub async fn heartbeat(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
        lease_owner: &str,
        extend_secs: u64,
    ) -> Result<bool, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let owner = lease_owner.to_string();
        let ttl = extend_secs.to_string();
        let extended = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(HEARTBEAT_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(job_uuid)
                    .bind(&owner)
                    .bind(&ttl)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(extended)
    }
}

pub struct JobQueueReaper {
    store: crate::CiRegionQueueStore,
    region: String,
    interval: Duration,
    cancelled_accounting: Option<(sqlx::PgPool, myelin_storage::DurableCostLedger)>,
    cancelled_cursor: std::sync::Mutex<Option<crate::job_queue_region::RecoveryCursor>>,
    timed_out_cursor: std::sync::Mutex<Option<crate::job_queue_region::RecoveryCursor>>,
}

impl JobQueueReaper {
    pub fn new(
        store: crate::CiRegionQueueStore,
        region: impl Into<String>,
        interval: Duration,
    ) -> Self {
        JobQueueReaper {
            store,
            region: region.into(),
            interval,
            cancelled_accounting: None,
            cancelled_cursor: std::sync::Mutex::new(None),
            timed_out_cursor: std::sync::Mutex::new(None),
        }
    }

    pub fn with_cancelled_accounting(
        mut self,
        pool: sqlx::PgPool,
        ledger: myelin_storage::DurableCostLedger,
    ) -> Self {
        self.cancelled_accounting = Some((pool, ledger));
        self
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub async fn reap_once(&self) -> Result<u64, JobQueueStoreError> {
        let mut changed = 0_u64;
        let mut failures = 0_u64;
        let mut first_failure = None;
        match self.store.seal_expired_prelaunch_usage(&self.region).await {
            Ok(sealed) => changed = changed.saturating_add(sealed),
            Err(error) => {
                failures = failures.saturating_add(1);
                first_failure
                    .get_or_insert_with(|| format!("prelaunch-usage sealing failed: {error}"));
            }
        }
        match self.store.reap(&self.region).await {
            Ok(reaped) => changed = changed.saturating_add(reaped),
            Err(error) => {
                failures = failures.saturating_add(1);
                first_failure.get_or_insert_with(|| format!("lease recovery failed: {error}"));
            }
        }
        let Some((pool, ledger)) = &self.cancelled_accounting else {
            return if failures == 0 {
                Ok(changed)
            } else {
                Err(JobQueueStoreError::Db(format!(
                    "{failures} reaper operation(s) failed after {changed} row(s) were recovered; \
                     first failure: {}",
                    first_failure.unwrap_or_else(|| "unknown recovery failure".into())
                )))
            };
        };
        let after = self
            .cancelled_cursor
            .lock()
            .map_err(|_| JobQueueStoreError::Db("cancelled-recovery cursor lock poisoned".into()))?
            .clone();
        let mut candidates = self
            .store
            .abandoned_cancelled(&self.region, after.as_ref())
            .await?;
        if candidates.is_empty() && after.is_some() {
            candidates = self.store.abandoned_cancelled(&self.region, None).await?;
        }
        let next_cursor =
            candidates
                .last()
                .map(|candidate| crate::job_queue_region::RecoveryCursor {
                    tenant_id: candidate.tenant_id.clone(),
                    job_id: candidate.job_id.clone(),
                });
        *self.cancelled_cursor.lock().map_err(|_| {
            JobQueueStoreError::Db("cancelled-recovery cursor lock poisoned".into())
        })? = next_cursor;

        let mut cancelled_failures = 0_u64;
        for candidate in candidates {
            let authority = match crate::PgCiRunSupersession::new(
                pool.clone(),
                ledger.clone(),
                myelin_tenancy::TenantId(candidate.tenant_id),
                myelin_tenancy::Region(self.region.clone()),
                tokio::runtime::Handle::current(),
            ) {
                Ok(authority) => authority,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    cancelled_failures = cancelled_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
            match authority
                .reconcile_abandoned_job(&candidate.wf_run_id, &candidate.job_id)
                .await
            {
                Ok(true) => changed = changed.saturating_add(1),
                Ok(false) => {}
                Err(error) => {
                    failures = failures.saturating_add(1);
                    cancelled_failures = cancelled_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                }
            }
        }
        let after = self
            .timed_out_cursor
            .lock()
            .map_err(|_| JobQueueStoreError::Db("timed-out recovery cursor lock poisoned".into()))?
            .clone();
        let mut prelaunch = self
            .store
            .active_prelaunch(&self.region, after.as_ref())
            .await?;
        if prelaunch.is_empty() && after.is_some() {
            prelaunch = self.store.active_prelaunch(&self.region, None).await?;
        }
        let next_cursor =
            prelaunch
                .last()
                .map(|candidate| crate::job_queue_region::RecoveryCursor {
                    tenant_id: candidate.tenant_id.clone(),
                    job_id: candidate.job_id.clone(),
                });
        *self.timed_out_cursor.lock().map_err(|_| {
            JobQueueStoreError::Db("timed-out recovery cursor lock poisoned".into())
        })? = next_cursor;
        let mut timed_out_failures = 0_u64;
        for candidate in prelaunch {
            let authority = match crate::PgCiRunSupersession::new(
                pool.clone(),
                ledger.clone(),
                myelin_tenancy::TenantId(candidate.tenant_id),
                myelin_tenancy::Region(self.region.clone()),
                tokio::runtime::Handle::current(),
            ) {
                Ok(authority) => authority,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    timed_out_failures = timed_out_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
            match authority
                .reconcile_timed_out_prelaunch_job(&candidate.wf_run_id, &candidate.job_id)
                .await
            {
                Ok(true) => changed = changed.saturating_add(1),
                Ok(false) => {}
                Err(error) => {
                    failures = failures.saturating_add(1);
                    timed_out_failures = timed_out_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                }
            }
        }
        if failures > 0 {
            let first = first_failure.unwrap_or_else(|| "unknown reconciliation failure".into());
            return if failures == cancelled_failures && timed_out_failures == 0 {
                Err(JobQueueStoreError::Db(format!(
                    "{cancelled_failures} cancelled recovery candidate(s) failed after {changed} \
                     row(s) were recovered; first failure: {first}"
                )))
            } else if failures == timed_out_failures && cancelled_failures == 0 {
                Err(JobQueueStoreError::Db(format!(
                    "{timed_out_failures} timed-out prelaunch recovery candidate(s) failed after \
                     {changed} row(s) were recovered; first failure: {first}"
                )))
            } else {
                Err(JobQueueStoreError::Db(format!(
                    "{failures} recovery operation(s) failed after {changed} row(s) were recovered; \
                     first failure: {first}"
                )))
            };
        }
        Ok(changed)
    }

    pub async fn run(self) {
        loop {
            tokio::time::sleep(self.interval).await;
            match self.reap_once().await {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!(
                        "ci-controlplane reaper: reconciled {n} CI lifecycle row(s) in region \
                         `{}` (prelaunch sealing, active requeue, timeout retirement, or \
                         cancelled settlement)",
                        self.region
                    );
                }
                Err(e) => {
                    eprintln!(
                        "ci-controlplane reaper: sweep in region `{}` FAILED (will retry next \
                         interval): {e}",
                        self.region
                    );
                }
            }
        }
    }

    pub async fn run_until_shutdown(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                _ = tokio::time::sleep(self.interval) => {
                    match self.reap_once().await {
                        Ok(0) => {}
                        Ok(n) => {
                            eprintln!(
                                "ci-controlplane reaper: reconciled {n} CI lifecycle row(s) in \
                                 region `{}` (prelaunch sealing, active requeue, timeout \
                                 retirement, or cancelled settlement)",
                                self.region
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "ci-controlplane reaper: sweep in region `{}` FAILED (will retry \
                                 next interval): {e}",
                                self.region
                            );
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn parse_id(field: &'static str, value: &str) -> Result<Uuid, JobQueueStoreError> {
    Uuid::parse_str(value).map_err(|_| JobQueueStoreError::BadId {
        field,
        value: value.to_string(),
    })
}

pub(crate) fn trust_token(t: TrustTier) -> &'static str {
    match t {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
        TrustTier::SelfHosted => "self_hosted",
    }
}

pub(crate) fn trust_from_token(token: &str) -> Result<TrustTier, JobQueueStoreError> {
    match token {
        "trusted" => Ok(TrustTier::Trusted),
        "untrusted_fork" => Ok(TrustTier::UntrustedFork),
        "self_hosted" => Ok(TrustTier::SelfHosted),
        other => Err(JobQueueStoreError::CorruptRow(format!(
            "unknown trust_tier token `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_tier_tokens_round_trip_and_reject_unknown() {
        for t in [
            TrustTier::Trusted,
            TrustTier::UntrustedFork,
            TrustTier::SelfHosted,
        ] {
            let token = trust_token(t);
            assert_eq!(trust_from_token(token).unwrap(), t);
        }
        assert_eq!(trust_token(TrustTier::UntrustedFork), "untrusted_fork");
        assert!(matches!(
            trust_from_token("root"),
            Err(JobQueueStoreError::CorruptRow(_))
        ));
    }

    #[test]
    fn a_non_uuid_id_is_a_loud_refusal() {
        let e = parse_id("job_id", "not-a-uuid").unwrap_err();
        assert!(matches!(
            e,
            JobQueueStoreError::BadId {
                field: "job_id",
                ..
            }
        ));
        assert!(parse_id("run_id", "00000000-0000-0000-0000-000000000001").is_ok());
    }

    #[test]
    fn lane_tokens_round_trip() {
        for l in [Lane::Interactive, Lane::Batch, Lane::Deploy] {
            assert_eq!(Lane::from_token(l.as_str()), Some(l));
        }
        assert_eq!(Lane::from_token("nonsense"), None);
    }
}
