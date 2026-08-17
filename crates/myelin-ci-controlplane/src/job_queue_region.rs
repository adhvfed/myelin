use std::future::Future;
use std::pin::Pin;

use myelin_ci_sandbox::TrustTier;
use myelin_storage::PgError;
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::job_queue_store::{trust_from_token, trust_token, JobQueueStoreError, LeasedJob};
use crate::scheduler::{Lane, CLAIM_QUERY, REAP_QUERY};

#[derive(Clone)]
pub struct CiRegionQueueStore {
    pool: PgPool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AbandonedCancelledJob {
    pub tenant_id: String,
    pub wf_run_id: String,
    pub job_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryCursor {
    pub tenant_id: String,
    pub job_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivePrelaunchJob {
    pub tenant_id: String,
    pub wf_run_id: String,
    pub job_id: String,
}

pub(crate) const MAX_ABANDONED_CANCELLED_RECOVERY_BATCH: i64 = 64;
pub(crate) const MAX_ACTIVE_PRELAUNCH_RECOVERY_BATCH: i64 = 64;
pub(crate) const MAX_PRELAUNCH_USAGE_SEAL_BATCH: i64 = 64;

impl CiRegionQueueStore {
    pub(crate) fn with_pg(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn claim(
        &self,
        region: &str,
        runner_labels: &[String],
        runner_allowed_tiers: &[TrustTier],
        lease_owner: &str,
        lease_secs: u64,
    ) -> Result<Option<LeasedJob>, JobQueueStoreError> {
        claim_region_scoped(
            &self.pool,
            region,
            runner_labels,
            runner_allowed_tiers,
            lease_owner,
            lease_secs,
        )
        .await
    }

    pub async fn reap(&self, region: &str) -> Result<u64, JobQueueStoreError> {
        reap_region_scoped(&self.pool, region).await
    }

    pub async fn seal_expired_prelaunch_usage(
        &self,
        region: &str,
    ) -> Result<u64, JobQueueStoreError> {
        seal_expired_prelaunch_usage_region_scoped(&self.pool, region).await
    }

    pub(crate) async fn abandoned_cancelled(
        &self,
        region: &str,
        after: Option<&RecoveryCursor>,
    ) -> Result<Vec<AbandonedCancelledJob>, JobQueueStoreError> {
        abandoned_cancelled_region_scoped(&self.pool, region, after).await
    }

    pub(crate) async fn active_prelaunch(
        &self,
        region: &str,
        after: Option<&RecoveryCursor>,
    ) -> Result<Vec<ActivePrelaunchJob>, JobQueueStoreError> {
        active_prelaunch_region_scoped(&self.pool, region, after).await
    }

    pub async fn count_non_terminal_null_stage_jobs(
        &self,
        region: &str,
    ) -> Result<i64, JobQueueStoreError> {
        count_non_terminal_null_stage_jobs_region_scoped(&self.pool, region).await
    }

    pub async fn count_non_terminal_null_claim_window_jobs(
        &self,
        region: &str,
    ) -> Result<i64, JobQueueStoreError> {
        count_non_terminal_null_claim_window_jobs_region_scoped(&self.pool, region).await
    }
}

async fn active_prelaunch_region_scoped(
    pool: &PgPool,
    region: &str,
    after: Option<&RecoveryCursor>,
) -> Result<Vec<ActivePrelaunchJob>, JobQueueStoreError> {
    let region_owned = region.to_owned();
    let after_tenant = after.map(|cursor| cursor.tenant_id.clone());
    let after_job = after
        .map(|cursor| Uuid::parse_str(&cursor.job_id))
        .transpose()
        .map_err(|_| JobQueueStoreError::CorruptRow("invalid queued-recovery cursor".into()))?;
    let rows = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "SELECT q.tenant_id, q.run_id::text AS wf_run_id, q.job_id::text AS job_id
                 FROM job_queue q
                 JOIN workflow_run w
                   ON w.tenant_id = q.tenant_id AND w.region = q.region
                  AND w.run_id = q.run_id::text
                 JOIN ci_run c
                   ON c.tenant_id = q.tenant_id AND c.region = q.region
                  AND c.wf_run_id = q.run_id
                 WHERE q.region = $1 AND q.state IN ('queued', 'leased')
                   AND w.state = 'waiting' AND c.state = 'running'
                   AND (
                     $2::text IS NULL
                     OR q.tenant_id > $2
                     OR (q.tenant_id = $2 AND q.job_id > $3::uuid)
                   )
                 ORDER BY q.tenant_id, q.job_id
                 LIMIT $4",
            )
            .bind(&region_owned)
            .bind(after_tenant)
            .bind(after_job)
            .bind(MAX_ACTIVE_PRELAUNCH_RECOVERY_BATCH)
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ActivePrelaunchJob {
            tenant_id: row.get("tenant_id"),
            wf_run_id: row.get("wf_run_id"),
            job_id: row.get("job_id"),
        })
        .collect())
}

async fn seal_expired_prelaunch_usage_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<u64, JobQueueStoreError> {
    let region_owned = region.to_owned();
    let sealed = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "WITH candidates AS MATERIALIZED (
                   SELECT tenant_id, region, job_id, lease_epoch, claim_nonce, phase
                   FROM ci_job_prelaunch_usage
                   WHERE region = $1 AND status = 'started'
                     AND seal_after IS NOT NULL
                     AND seal_after <= statement_timestamp()
                   ORDER BY seal_after, tenant_id, job_id, lease_epoch, claim_nonce, phase
                   FOR UPDATE SKIP LOCKED
                   LIMIT $2
                 )
                 UPDATE ci_job_prelaunch_usage u
                 SET status = 'sealed_ceiling', resolved_at = statement_timestamp()
                 FROM candidates c
                 WHERE u.tenant_id = c.tenant_id AND u.region = c.region
                   AND u.job_id = c.job_id AND u.lease_epoch = c.lease_epoch
                   AND u.claim_nonce = c.claim_nonce AND u.phase = c.phase
                   AND u.status = 'started'
                 RETURNING u.job_id",
            )
            .bind(&region_owned)
            .bind(MAX_PRELAUNCH_USAGE_SEAL_BATCH)
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await?;
    u64::try_from(sealed.len())
        .map_err(|_| JobQueueStoreError::CorruptRow("prelaunch seal count overflowed".into()))
}

async fn abandoned_cancelled_region_scoped(
    pool: &PgPool,
    region: &str,
    after: Option<&RecoveryCursor>,
) -> Result<Vec<AbandonedCancelledJob>, JobQueueStoreError> {
    let region_owned = region.to_owned();
    let after_tenant = after.map(|cursor| cursor.tenant_id.clone());
    let after_job = after
        .map(|cursor| Uuid::parse_str(&cursor.job_id))
        .transpose()
        .map_err(|_| JobQueueStoreError::CorruptRow("invalid cancelled-recovery cursor".into()))?;
    let rows = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query(
                "SELECT q.tenant_id, q.run_id::text AS wf_run_id, q.job_id::text AS job_id
                 FROM job_queue q
                 JOIN workflow_run w
                   ON w.tenant_id = q.tenant_id AND w.region = q.region
                  AND w.run_id = q.run_id::text
                 JOIN ci_run c
                   ON c.tenant_id = q.tenant_id AND c.region = q.region
                  AND c.wf_run_id = q.run_id
                 WHERE q.region = $1 AND q.state = 'running' AND q.lease_expires < now()
                   AND w.state = 'terminated' AND c.state = 'cancelled'
                   AND (
                     $2::text IS NULL
                     OR q.tenant_id > $2
                     OR (q.tenant_id = $2 AND q.job_id > $3::uuid)
                   )
                 ORDER BY q.tenant_id, q.job_id
                 LIMIT $4",
            )
            .bind(&region_owned)
            .bind(after_tenant)
            .bind(after_job)
            .bind(MAX_ABANDONED_CANCELLED_RECOVERY_BATCH)
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AbandonedCancelledJob {
            tenant_id: row.get("tenant_id"),
            wf_run_id: row.get("wf_run_id"),
            job_id: row.get("job_id"),
        })
        .collect())
}

pub(crate) async fn claim_region_scoped(
    pool: &PgPool,
    region: &str,
    runner_labels: &[String],
    runner_allowed_tiers: &[TrustTier],
    lease_owner: &str,
    lease_secs: u64,
) -> Result<Option<LeasedJob>, JobQueueStoreError> {
    let labels: Vec<String> = runner_labels.to_vec();
    let tiers: Vec<String> = runner_allowed_tiers
        .iter()
        .map(|t| trust_token(*t).to_string())
        .collect();
    let owner = lease_owner.to_string();
    let ttl = lease_secs.to_string();
    let region_owned = region.to_string();
    let row = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query(CLAIM_QUERY)
                .bind(&region_owned)
                .bind(&labels)
                .bind(&tiers)
                .bind(&owner)
                .bind(&ttl)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))
        })
    })
    .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(leased_from_row(&r, lease_owner)?)),
    }
}

const RESET_REAPED_CI_JOB_SURFACE_QUERY: &str = "\
UPDATE ci_job
SET state = 'queued'
WHERE state = 'running'
  AND (tenant_id, job_id) IN (
    SELECT * FROM UNNEST($1::text[], $2::uuid[])
  )";

const SEAL_REAPED_PRELAUNCH_USAGE_QUERY: &str = "\
UPDATE ci_job_prelaunch_usage u
SET status = 'sealed_ceiling',
    resolved_at = GREATEST(statement_timestamp(), u.started_at)
FROM UNNEST($1::text[], $2::uuid[], $3::bigint[], $4::uuid[])
  AS reaped(tenant_id, job_id, lease_epoch, claim_nonce)
WHERE u.tenant_id = reaped.tenant_id
  AND u.region = $5
  AND u.job_id = reaped.job_id
  AND u.lease_epoch = reaped.lease_epoch
  AND u.claim_nonce = reaped.claim_nonce
  AND u.status = 'started'
RETURNING u.job_id";

pub(crate) async fn reap_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<u64, JobQueueStoreError> {
    let region_owned = region.to_string();
    let seal_region = region.to_string();
    let rows = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            let reaped = sqlx::query(REAP_QUERY)
                .bind(&region_owned)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            if !reaped.is_empty() {
                let mut seal_tenants: Vec<String> = Vec::with_capacity(reaped.len());
                let mut seal_jobs: Vec<Uuid> = Vec::with_capacity(reaped.len());
                let mut seal_epochs: Vec<i64> = Vec::with_capacity(reaped.len());
                let mut seal_nonces: Vec<Uuid> = Vec::with_capacity(reaped.len());
                for row in &reaped {
                    let Some(nonce) = row.get::<Option<String>, _>("reaped_claim_nonce") else {
                        continue;
                    };
                    let Ok(nonce) = Uuid::parse_str(&nonce) else {
                        return Err(PgError::Query(
                            "reaped generation carries a non-uuid claim nonce".into(),
                        ));
                    };
                    seal_tenants.push(row.get::<String, _>("tenant_id"));
                    seal_jobs.push(row.get::<Uuid, _>("job_id"));
                    seal_epochs.push(row.get::<i64, _>("reaped_lease_epoch"));
                    seal_nonces.push(nonce);
                }
                if !seal_tenants.is_empty() {
                    sqlx::query(SEAL_REAPED_PRELAUNCH_USAGE_QUERY)
                        .bind(&seal_tenants)
                        .bind(&seal_jobs)
                        .bind(&seal_epochs)
                        .bind(&seal_nonces)
                        .bind(&seal_region)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                }
                let tenant_ids: Vec<String> = reaped
                    .iter()
                    .map(|r| r.get::<String, _>("tenant_id"))
                    .collect();
                let job_ids: Vec<Uuid> =
                    reaped.iter().map(|r| r.get::<Uuid, _>("job_id")).collect();
                sqlx::query(RESET_REAPED_CI_JOB_SURFACE_QUERY)
                    .bind(&tenant_ids)
                    .bind(&job_ids)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
            }
            Ok(reaped)
        })
    })
    .await?;
    Ok(rows.len() as u64)
}

pub(crate) async fn count_non_terminal_null_stage_jobs_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<i64, JobQueueStoreError> {
    let region_owned = region.to_string();
    with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>(crate::job_spec_store::NON_TERMINAL_NULL_STAGE_JOBS_QUERY)
                .bind(&region_owned)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))
        })
    })
    .await
}

pub(crate) async fn count_non_terminal_null_claim_window_jobs_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<i64, JobQueueStoreError> {
    let region_owned = region.to_string();
    with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>(
                crate::job_spec_store::NON_TERMINAL_NULL_CLAIM_WINDOW_JOBS_QUERY,
            )
            .bind(&region_owned)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
        })
    })
    .await
}

pub(crate) async fn with_region_tx<R, F>(
    pool: &PgPool,
    region: &str,
    op: F,
) -> Result<R, JobQueueStoreError>
where
    F: for<'c> FnOnce(
            &'c mut sqlx::PgConnection,
        ) -> Pin<Box<dyn Future<Output = Result<R, PgError>> + Send + 'c>>
        + Send,
    R: Send,
{
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| JobQueueStoreError::Db(format!("begin region-scoped transaction: {e}")))?;
    sqlx::query(
        "SELECT set_config('myelin.region', $1, true), set_config('myelin.tenant_id', '', true)",
    )
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| JobQueueStoreError::Db(format!("set region-scoped GUC: {e}")))?;
    let out = op(&mut tx).await.map_err(JobQueueStoreError::from_pg)?;
    tx.commit()
        .await
        .map_err(|e| JobQueueStoreError::Db(format!("commit region-scoped transaction: {e}")))?;
    Ok(out)
}

fn leased_from_row(
    r: &sqlx::postgres::PgRow,
    lease_owner: &str,
) -> Result<LeasedJob, JobQueueStoreError> {
    let tenant_id: String = r.get("tenant_id");
    let job_id: Uuid = r.get("job_id");
    let run_id: Uuid = r.get("run_id");
    let lane_token: String = r.get("lane");
    let concurrency_group: Option<String> = r.get("concurrency_group");
    let fair_key: String = r.get("fair_key");
    let trust_token_str: String = r.get("trust_tier");
    let lease_epoch: i64 = r.get("lease_epoch");
    let claim_nonce: String = r.get("claim_nonce");
    let claim_started_at_epoch_secs: i64 = r.get("claim_started_at_epoch_secs");
    let claim_expires_at_epoch_secs: i64 = r.get("claim_expires_at_epoch_secs");
    let claim_window_secs: Option<i64> = r.get("claim_window_secs");
    let lane = Lane::from_token(&lane_token).ok_or_else(|| {
        JobQueueStoreError::CorruptRow(format!("unknown lane token `{lane_token}`"))
    })?;
    let trust_tier = trust_from_token(&trust_token_str)?;
    Ok(LeasedJob {
        tenant_id,
        job_id,
        run_id,
        lane,
        concurrency_group,
        fair_key,
        trust_tier,
        lease_owner: lease_owner.to_string(),
        lease_epoch,
        claim_nonce,
        claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs,
        claim_window_secs,
    })
}
