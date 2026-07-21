//! # `job_queue_region` — CT-004c.1: the REGION-scoped, CROSS-TENANT scheduler claim + reaper
//!
//! **This file is a NAMED, LOUD `tenant-predicate` exclusion (see `myelin-lints` `EXCLUDED_SUBSTRINGS`).**
//! The scheduler pull-lease claim (arch 02 §2.1, [`crate::scheduler::CLAIM_QUERY`]) and the
//! dead-runner reaper ([`crate::scheduler::REAP_QUERY`]) are CROSS-TENANT BY DESIGN: a hosted runner
//! claims the next eligible job across ALL tenants in its region, and the DRR fairness
//! (`fair_deficit.deficit DESC`) is explicitly cross-tenant ("prevents one tenant's matrix from
//! starving every OTHER tenant", arch 02 §2.2). These queries filter by `region` only and carry NO
//! `tenant_id` predicate — so the `tenant-predicate` IDOR fingerprint (`sqlx::query` without a
//! `tenant_id`) flags them FALSELY, exactly the same control-plane-routing posture as
//! `myelin-storage/src/placement_durable.rs` (the cell-placement registry: "which cell homes tenant
//! X?" for any X). The `region` column is the ROUTING/RESIDENCY key here, not an RLS predicate.
//!
//! **The tenant-store queries stay FULLY linted.** Only the two cross-tenant SERVICE queries live in
//! this excluded file; the PER-TENANT ops (`enqueue`/`cancel_superseded`/`complete`/`heartbeat`) stay
//! in [`crate::job_queue_store`] (NOT excluded), each binding `tenant_id` through the MR-022
//! `with_tenant_tx` convention — so the tenant-predicate lint reads the IDOR guard on every one. This
//! is a SCOPED exclusion of the two genuinely-cross-tenant reads, never a whole-store waiver.
//!
//! **Residency + no bleed (in-band).** Both queries run under [`with_region_tx`]: acquire → BEGIN →
//! set the `region` GUC transaction-scoped (residency pin, in-band — the same posture the
//! `residency-pin` lint expects, recorded via `@residency-cell-pinned:file`) + clear the tenant GUC →
//! op → COMMIT (the GUC discarded on commit, so the pooled connection carries no residual scope).
//!
//! **Dedicated capability boundary.** [`CiRegionQueueStore`] must be constructed over the constrained
//! region-scheduler pool. PostgreSQL admits rows only when the authenticated `session_user` mapping,
//! the row region, and this transaction's region GUC agree while the tenant GUC is empty. The type
//! exposes only claim/reap; tenant writes remain impossible through this capability surface.
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
/// can claim/reap and exposes no tenant mutation verbs.
#[derive(Clone)]
pub struct CiRegionQueueStore {
    pool: PgPool,
}

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

    /// Re-queue expired leases across tenants in the mapped region.
    pub async fn reap(&self, region: &str) -> Result<u64, JobQueueStoreError> {
        reap_region_scoped(&self.pool, region).await
    }

    /// Refuse runner activation while an old non-terminal row lacks completion stage authority.
    pub async fn count_non_terminal_null_stage_jobs(
        &self,
        region: &str,
    ) -> Result<i64, JobQueueStoreError> {
        count_non_terminal_null_stage_jobs_region_scoped(&self.pool, region).await
    }
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
        Some(r) => Ok(Some(leased_from_row(&r)?)),
    }
}

/// **The dead-runner reaper raw execution (arch 02 §2.1; [`REAP_QUERY`]) — region-scoped, cross-tenant.**
/// Runs [`REAP_QUERY`] under [`with_region_tx`], re-queuing every expired lease in the region in place
/// (no INSERT → 0 duplicate enqueues). Returns the count re-queued.
pub(crate) async fn reap_region_scoped(
    pool: &PgPool,
    region: &str,
) -> Result<u64, JobQueueStoreError> {
    let region_owned = region.to_string();
    let rows = with_region_tx(pool, region, move |conn| {
        Box::pin(async move {
            sqlx::query(REAP_QUERY)
                .bind(&region_owned) // $1 region (RESIDENCY, not a tenant predicate)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))
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

/// **The REGION-scoped transaction seam (the cross-tenant claim/reap path).** Acquire → BEGIN → set
/// the `region` GUC transaction-scoped (residency pin, in-band) + clear the tenant GUC → run `op` →
/// COMMIT (the GUC discarded on commit — no bleed). Mirrors `myelin_storage::with_tenant_tx`'s
/// mechanism but for a region-wide, cross-tenant SERVICE read.
async fn with_region_tx<R, F>(pool: &PgPool, region: &str, op: F) -> Result<R, JobQueueStoreError>
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
fn leased_from_row(r: &sqlx::postgres::PgRow) -> Result<LeasedJob, JobQueueStoreError> {
    let tenant_id: String = r.get("tenant_id");
    let job_id: Uuid = r.get("job_id");
    let run_id: Uuid = r.get("run_id");
    let lane_token: String = r.get("lane");
    let concurrency_group: Option<String> = r.get("concurrency_group");
    let fair_key: String = r.get("fair_key");
    let trust_token_str: String = r.get("trust_tier");
    let lease_epoch: i64 = r.get("lease_epoch");
    let claim_nonce: String = r.get("claim_nonce");
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
        lease_epoch,
        claim_nonce,
    })
}
