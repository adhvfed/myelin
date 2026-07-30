//! # `job_queue_region` — CT-004c.1: the REGION-scoped, CROSS-TENANT scheduler claim + reaper
//!
//! **This file is a NAMED, LOUD `tenant-predicate` exclusion (see `myelin-lints` `EXCLUDED_SUBSTRINGS`).**
//! The scheduler pull-lease claim (arch 02 §2.1, [`crate::scheduler::CLAIM_QUERY`]), active-job
//! dead-runner reaper ([`crate::scheduler::REAP_QUERY`]), and bounded cancelled-launch discovery
//! are CROSS-TENANT BY DESIGN: a hosted runner claims the next eligible job across ALL tenants in
//! its region, and the DRR fairness
//! (`fair_deficit.deficit DESC`) is explicitly cross-tenant ("prevents one tenant's matrix from
//! starving every OTHER tenant", arch 02 §2.2). These queries filter by `region` only and carry NO
//! `tenant_id` predicate — so the `tenant-predicate` IDOR fingerprint (`sqlx::query` without a
//! `tenant_id`) flags them FALSELY, exactly the same control-plane-routing posture as
//! `myelin-storage/src/placement_durable.rs` (the cell-placement registry: "which cell homes tenant
//! X?" for any X). The `region` column is the ROUTING/RESIDENCY key here, not an RLS predicate.
//!
//! **The tenant-store queries stay FULLY linted.** Only cross-tenant SERVICE queries live in
//! this excluded file; the PER-TENANT ops (`enqueue`/`cancel_superseded`/`complete`/`heartbeat`) stay
//! in [`crate::job_queue_store`] (NOT excluded), each binding `tenant_id` through the MR-022
//! `with_tenant_tx` convention — so the tenant-predicate lint reads the IDOR guard on every one. This
//! is a SCOPED exclusion of genuinely cross-tenant routing/recovery reads, never a whole-store waiver.
//!
//! **Residency + no bleed (in-band).** Both queries run under [`with_region_tx`]: acquire → BEGIN →
//! set the `region` GUC transaction-scoped (residency pin, in-band — the same posture the
//! `residency-pin` lint expects, recorded via `@residency-cell-pinned:file`) + clear the tenant GUC →
//! op → COMMIT (the GUC discarded on commit, so the pooled connection carries no residual scope).
//!
//! **Dedicated capability boundary.** [`CiRegionQueueStore`] must be constructed over the constrained
//! region-scheduler pool. PostgreSQL admits rows only when the authenticated `session_user` mapping,
//! the row region, and this transaction's region GUC agree while the tenant GUC is empty. The type
//! exposes only claim/reap/recovery discovery; tenant writes remain impossible through this
//! capability surface.
//!
//! @residency-cell-pinned:file — the region is pinned in-band on every op via the transaction-scoped
//! `myelin.region` GUC (there is no region-less pool construction in this file; the pool is injected).

use std::future::Future;
use std::pin::Pin;

use myelin_ci_sandbox::TrustTier;
use myelin_storage::PgError;
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::job_queue_store::{trust_from_token, trust_token, JobQueueStoreError, LeasedJob};
use crate::scheduler::{Lane, CLAIM_QUERY, REAP_QUERY};

/// Region-wide scheduler capability over its dedicated constrained PostgreSQL pool. This type is
/// intentionally separate from [`crate::CiJobQueueStore`]: a tenant application pool can enqueue,
/// heartbeat, complete, and cancel, but cannot express claim/reap in the Rust API; the scheduler pool
/// can claim/reap/discover cancelled recovery candidates and exposes no tenant mutation verbs.
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
pub(crate) struct AbandonedCancelledCursor {
    pub tenant_id: String,
    pub job_id: String,
}

pub(crate) const MAX_ABANDONED_CANCELLED_RECOVERY_BATCH: i64 = 64;
pub(crate) const MAX_PRELAUNCH_USAGE_SEAL_BATCH: i64 = 64;

impl CiRegionQueueStore {
    /// Bind the dedicated, startup-probed region-scheduler pool.
    pub(crate) fn with_pg(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Lease one eligible row across tenants in the mapped region.
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

    /// Re-queue expired leases whose authoritative Flow and CI owners remain active.
    pub async fn reap(&self, region: &str) -> Result<u64, JobQueueStoreError> {
        reap_region_scoped(&self.pool, region).await
    }

    /// Seal one bounded, skip-locked page of prelaunch phases whose immutable topology-aware
    /// deadline has elapsed. NULL legacy deadlines are deliberately invisible: the reaper never
    /// guesses abandonment from the flat scheduler lease.
    pub async fn seal_expired_prelaunch_usage(
        &self,
        region: &str,
    ) -> Result<u64, JobQueueStoreError> {
        seal_expired_prelaunch_usage_region_scoped(&self.pool, region).await
    }

    /// Discover at most one bounded keyset page of identities for exact-tenant cancelled-launch
    /// reconciliation. The caller rotates the cursor across sweeps so a persistent poison row
    /// cannot monopolize the region.
    pub(crate) async fn abandoned_cancelled(
        &self,
        region: &str,
        after: Option<&AbandonedCancelledCursor>,
    ) -> Result<Vec<AbandonedCancelledJob>, JobQueueStoreError> {
        abandoned_cancelled_region_scoped(&self.pool, region, after).await
    }

    /// Refuse runner activation while an old non-terminal row lacks completion stage authority.
    pub async fn count_non_terminal_null_stage_jobs(
        &self,
        region: &str,
    ) -> Result<i64, JobQueueStoreError> {
        count_non_terminal_null_stage_jobs_region_scoped(&self.pool, region).await
    }

    /// **The CHECKOUT-COMPOSITION activation guard (CT-007 lease/topology reconciliation).** Counts
    /// non-terminal rows dispatched before the claim-window expand. Such a row is claimed under the
    /// flat execution-lease fallback — CORRECT for the workload-only topology it was dispatched
    /// under, which is why this deliberately does NOT gate the ordinary runner lane the way the
    /// null-stage backlog does: a legacy row still runs its workload fine, so refusing to start a
    /// runner over it would be a self-inflicted outage. It can never hold a four-execution checkout
    /// topology, and per-job enforcement already refuses exactly that (the resolver before mint, the
    /// issuer inside the locked mint). 5b.3-6 calls this before enabling checkout composition, so
    /// the coarse fleet-convergence check exists alongside the per-job one rather than instead of it.
    pub async fn count_non_terminal_null_claim_window_jobs(
        &self,
        region: &str,
    ) -> Result<i64, JobQueueStoreError> {
        count_non_terminal_null_claim_window_jobs_region_scoped(&self.pool, region).await
    }
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
    after: Option<&AbandonedCancelledCursor>,
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

/// **The pull-lease claim raw execution (arch 02 §2.1; [`CLAIM_QUERY`]) — region-scoped, cross-tenant.**
/// Runs [`CLAIM_QUERY`] under [`with_region_tx`] and rebuilds the leased row. See the module note: the
/// `region` filter is the residency/routing key, NOT a tenant predicate (a hosted runner claims across
/// all tenants in its region — the DRR fairness spans tenants).
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
                .bind(&region_owned) // $1 cell_region (RESIDENCY, not a tenant predicate)
                .bind(&labels) // $2 runner_labels text[]
                .bind(&tiers) // $3 runner_allowed_tiers text[]
                .bind(&owner) // $4 lease_owner
                .bind(&ttl) // $5 lease_ttl_seconds (text → interval)
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

/// BUG FIX (investigation, 2026-07-25): [`REAP_QUERY`] re-queues `job_queue` (running/leased ->
/// queued) for a dead runner, but never touches the `ci_job` DAG surface `AUTHORIZE_JOB_LAUNCH_QUERY`
/// also crosses to `running` in the SAME statement as the launch CAS. Without this reset, a job whose
/// launch CAS committed (`ci_job.state = 'running'`) and then crashed BEFORE completing — the exact
/// "paused live continuation" scenario `tests/integration_ci_drive_manifest_store.rs`
/// (`store_replays_exact_bytes_and_refuses_divergent_authority`) proves — is reaped at the `job_queue`
/// layer (re-queued, freshly re-claimable) but can NEVER re-win the launch fence: `ci_job.state` stays
/// stuck at `'running'` forever, and `AUTHORIZE_JOB_LAUNCH_QUERY`'s surface-crossing UPDATE requires
/// `surface.state IN ('queued', 'leased')` (deliberately, per the pinned
/// `job_queue_store.rs` unit test) — so every subsequent relaunch attempt matches zero rows and the
/// job is permanently stranded. This is added here — a NEW, separate statement inside
/// [`reap_region_scoped`] — rather than folded into [`REAP_QUERY`] itself, because
/// `tests/integration_ci_p12_scheduler_claim.rs` and
/// `tests/integration_ci_p28_ct004_durability.rs` both run [`REAP_QUERY`]'s literal text directly
/// (via `.replace("job_queue", ...)`) against isolated synthetic fixtures that have no `ci_job` table
/// at all; folding the reset into the shared constant would break those tests. Only jobs REAP_QUERY
/// actually re-queued (the exact-generation set it just committed) are reset, and only from
/// `'running'` (a job already `'succeeded'`/`'failed'`/`'cancelled'`/`'reaped'` is a terminal DAG
/// fact and must never be reopened).
const RESET_REAPED_CI_JOB_SURFACE_QUERY: &str = "\
UPDATE ci_job
SET state = 'queued'
WHERE state = 'running'
  AND (tenant_id, job_id) IN (
    SELECT * FROM UNNEST($1::text[], $2::uuid[])
  )";

/// **CT-007 lease/topology reconciliation: seal the reaped generation's unresolved prelaunch phases
/// in the SAME transaction as the requeue.** A heartbeat-lapsed generation whose claim window is
/// still open must not become freshly claimable while its `started` journal rows remain unresolved:
/// the replacement claim would admit a new parent attempt whose settlement could not honestly
/// account for the old generation's accrued preparation usage until the independent `seal_after`
/// deadline eventually elapsed.
///
/// The predicate names the EXACT generation [`REAP_QUERY`] just requeued (tenant, region, job,
/// `lease_epoch`, `claim_nonce`) and only `started` rows — a neighbouring generation's phases, and
/// any phase a worker already `measured`, are untouched. `GREATEST(statement_timestamp(),
/// started_at)` closes the one edge the table CHECK would otherwise reject: a backward host-clock
/// step cannot produce `resolved_at < started_at`.
///
/// This is the only transition it performs (`started -> sealed_ceiling`), which the transition
/// trigger admits unconditionally: it refuses only a non-`started` OLD status, a `started` NEW
/// status, or a mutation of identity/ceiling/`started_at`/`seal_after`. So no reachable journal
/// state makes this refuse while the queue row is reapable — a failure here is infrastructural, and
/// rolling the whole sweep back is the honest response.
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

/// **The dead-runner reaper raw execution (arch 02 §2.1; [`REAP_QUERY`]) — region-scoped, cross-tenant.**
/// Runs [`REAP_QUERY`] under [`with_region_tx`], re-queuing every expired lease with an active
/// Flow/CI owner in place (no INSERT → 0 duplicate enqueues). Cancelled owners are excluded and
/// handled through exact-tenant accounting reconciliation. Also resets the matching `ci_job` DAG
/// surface row(s) back to `'queued'` for exactly the jobs just re-queued (see
/// [`RESET_REAPED_CI_JOB_SURFACE_QUERY`]), so a freshly re-claimed generation can win the launch fence
/// again. Returns the count re-queued.
///
/// **All three effects commit atomically (CT-007 lease/topology reconciliation).** Between the
/// requeue and the surface reset, the exact reaped generation's unresolved prelaunch phases are
/// sealed to their stored ceiling ([`SEAL_REAPED_PRELAUNCH_USAGE_QUERY`]); the surface is reset only
/// after that sealing succeeds. A seal failure rolls the WHOLE sweep back, so there is never a
/// committed "freshly claimable but old phase unresolved" state — a persistently failing seal
/// deliberately pins the row and reports loudly rather than making preparation usage it cannot
/// settle honestly disappear. The independent `seal_after` deadline sealer remains necessary and
/// untouched: it covers abandonment paths with no active-owner lease reap at all.
pub(crate) async fn reap_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<u64, JobQueueStoreError> {
    let region_owned = region.to_string();
    let seal_region = region.to_string();
    let rows = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            let reaped = sqlx::query(REAP_QUERY)
                .bind(&region_owned) // $1 region (RESIDENCY, not a tenant predicate)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            if !reaped.is_empty() {
                // The exact generation each row carried BEFORE the requeue cleared its nonce. A
                // legacy row with no nonce names no journal generation, so it is excluded here
                // rather than widening the seal predicate.
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

/// **The runner-lane pre-activation guard (CT-004d.2 — the ROLLING-UPGRADE FLOOR), region-scoped +
/// cross-tenant.** Counts, across every tenant in `region`, `job_queue` rows that are still non-terminal
/// but whose queue-authority `stage` is NULL (a pre-rewire historical dispatch a rolling upgrade left).
/// The runner-lane activation path refuses to start while this is non-zero — a CHECKED invariant that
/// the reporter is never asked to complete an unattributable live job. A healthy deploy
/// (CI has never been production-activated) returns 0. Runs under [`with_region_tx`] (the cross-tenant
/// service read the region scheduler already uses), NOT a per-tenant scope — so it lives in this NAMED
/// `tenant-predicate`-excluded module, never a per-tenant store.
pub(crate) async fn count_non_terminal_null_stage_jobs_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<i64, JobQueueStoreError> {
    let region_owned = region.to_string();
    with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query_scalar::<_, i64>(crate::job_spec_store::NON_TERMINAL_NULL_STAGE_JOBS_QUERY)
                .bind(&region_owned) // $1 region (RESIDENCY, not a tenant predicate — cross-tenant read)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))
        })
    })
    .await
}

/// **The claim-window activation guard (CT-007 lease/topology reconciliation), region-scoped +
/// cross-tenant.** Counts, across every tenant in `region`, non-terminal `job_queue` rows whose
/// `claim_window_secs` is still NULL — see
/// [`NON_TERMINAL_NULL_CLAIM_WINDOW_JOBS_QUERY`](crate::job_spec_store::NON_TERMINAL_NULL_CLAIM_WINDOW_JOBS_QUERY).
/// Lives in this NAMED `tenant-predicate`-excluded module for the same reason its null-stage sibling
/// does: it is a cross-tenant service read under [`with_region_tx`], never a per-tenant store op.
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
            .bind(&region_owned) // $1 region (RESIDENCY, not a tenant predicate — cross-tenant read)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
        })
    })
    .await
}

/// **The REGION-scoped transaction seam (the cross-tenant scheduler path).** Acquire → BEGIN → set
/// the `region` GUC transaction-scoped (residency pin, in-band) + clear the tenant GUC → run `op` →
/// COMMIT (the GUC discarded on commit — no bleed). Mirrors `myelin_storage::with_tenant_tx`'s
/// mechanism but for a region-wide, cross-tenant SERVICE read.
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
    // Set the region GUC TRANSACTION-scoped and clear the tenant GUC (a region-wide claim is not a
    // single tenant's op) — discarded at COMMIT, so the pooled connection carries no residue.
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

/// Rebuild a [`LeasedJob`] from a claim `RETURNING` row (`tenant_id, job_id, run_id, lane,
/// concurrency_group, fair_key, trust_tier`). A `lane`/`trust_tier` token outside the frozen set is a
/// loud [`JobQueueStoreError::CorruptRow`].
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
