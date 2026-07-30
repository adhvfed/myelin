//! Durable parent-attempt admission and checkout-preparation usage journaling.
//!
//! A parent attempt is admitted only after the exact scheduler generation, immutable manifest,
//! durable launch template, v2 reservation handle, reservation amount, and reservation lifecycle
//! all agree in one tenant transaction. Checkout phases then move monotonically from `started` to
//! either exact `measured` usage or the conservative `sealed_ceiling` fallback.

use myelin_ci_sandbox::ResourceUsage;
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Row};

use crate::ci_claim_token_issuer::{
    authority_from_durable_claim, runtime_authorities_from_durable_claim, verify_locked_claim,
};
use crate::ci_drive_manifest::CiDriveManifestStore;
use crate::ci_launch_authority::{
    raw_execution_usage_ceiling, verify_v2_operational_reservation, CiAttemptBudgetPolicy,
    CiJobRuntimeAuthorityRequest,
};
use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_pipeline_driver::{
    TIER_P_OPERATIONAL_RESERVATION_PREFIX, TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX,
};
use crate::ci_run_store::CiRunStore;
use crate::job_queue_store::CiJobQueueStore;
use crate::job_spec_store::CiJobSpecStore;

/// One checkout-preparation phase represented by `ci_job_prelaunch_usage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchUsagePhase {
    CheckoutTransport,
    CheckoutMaterialization,
}

impl CiPrelaunchUsagePhase {
    fn token(self) -> &'static str {
        match self {
            Self::CheckoutTransport => "checkout_transport",
            Self::CheckoutMaterialization => "checkout_materialization",
        }
    }

    fn execution_count(self) -> u64 {
        match self {
            Self::CheckoutTransport => 2,
            Self::CheckoutMaterialization => 1,
        }
    }
}

/// Whether a durable journal operation created/transitioned a row or recovered an exact replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchJournalOutcome {
    Applied,
    Replayed,
}

/// Typed, non-secret refusal from the prelaunch-usage journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchUsageJournalError {
    InvalidClaim,
    WrongRegion,
    ClaimUnavailable,
    DurableAuthorityUnavailable,
    ManifestReserveHandleMismatch,
    DispatchedReserveHandleMismatch,
    LegacyReservation,
    ReservationUnavailable,
    ReservationAuthorityMismatch,
    ReservationNotLaunchable,
    ParentAttemptLimitExceeded,
    ParentAttemptDivergence,
    PhaseUnavailable,
    PhaseDivergence,
    IllegalPhaseTransition,
    UsageOverflow,
    Database,
}

impl std::fmt::Display for CiPrelaunchUsageJournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::InvalidClaim => "the parent-attempt claim is invalid",
            Self::WrongRegion => "the parent-attempt claim is outside this cell region",
            Self::ClaimUnavailable => "the exact scheduler claim generation is not live",
            Self::DurableAuthorityUnavailable => {
                "durable run, manifest, or launch authority is unavailable"
            }
            Self::ManifestReserveHandleMismatch => {
                "the claim reservation differs from the immutable manifest"
            }
            Self::DispatchedReserveHandleMismatch => {
                "the claim reservation differs from the durable dispatched spec"
            }
            Self::LegacyReservation => "v1 reservations cannot enter the capped attempt journal",
            Self::ReservationUnavailable => "the exact durable reservation is unavailable",
            Self::ReservationAuthorityMismatch => {
                "the durable v2 reservation authority or amount diverged"
            }
            Self::ReservationNotLaunchable => {
                "the durable reservation is neither reserved nor inflight"
            }
            Self::ParentAttemptLimitExceeded => "the durable parent-attempt limit is exhausted",
            Self::ParentAttemptDivergence => "the parent-attempt replay differs from durable facts",
            Self::PhaseUnavailable => "the requested prelaunch phase has not begun",
            Self::PhaseDivergence => "the prelaunch phase replay differs from durable facts",
            Self::IllegalPhaseTransition => "the prelaunch phase transition is not legal",
            Self::UsageOverflow => "the prelaunch phase usage ceiling overflowed",
            Self::Database => "the durable prelaunch-usage transaction failed",
        };
        write!(f, "CI prelaunch usage journal refused: {detail}")
    }
}

impl std::error::Error for CiPrelaunchUsageJournalError {}

impl From<PgError> for CiPrelaunchUsageJournalError {
    fn from(_: PgError) -> Self {
        Self::Database
    }
}

/// Opaque proof that one exact parent attempt passed the durable admission boundary.
#[derive(Clone, Debug)]
pub struct CiJobParentAttempt {
    tenant_id: String,
    region: String,
    job_id: String,
    lease_epoch: i64,
    claim_nonce: String,
    authority: CiJobRuntimeAuthorityRequest,
}

impl CiJobParentAttempt {
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn lease_epoch(&self) -> i64 {
        self.lease_epoch
    }

    pub fn claim_nonce(&self) -> &str {
        &self.claim_nonce
    }
}

/// PostgreSQL-backed state machine for the parent-attempt and phase journal pair.
#[derive(Clone)]
pub struct CiPrelaunchUsageJournal {
    pool: PgPool,
    region: String,
}

impl CiPrelaunchUsageJournal {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
    ) -> Result<Self, CiPrelaunchUsageJournalError> {
        let region = region.into();
        if region.is_empty()
            || region.len() > 128
            || !region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(CiPrelaunchUsageJournalError::WrongRegion);
        }
        Ok(Self { pool, region })
    }

    /// Admit one exact live claim generation and create its immutable parent-attempt row.
    ///
    /// Exact replay returns the existing row. A distinct generation is admitted only while the
    /// policy embedded in the verified v2 handle still has capacity.
    pub async fn begin_parent_attempt(
        &self,
        claim: &CiJobTokenRequest,
        reserve_handle: &str,
    ) -> Result<(CiJobParentAttempt, CiPrelaunchJournalOutcome), CiPrelaunchUsageJournalError> {
        claim
            .validate()
            .map_err(|_| CiPrelaunchUsageJournalError::InvalidClaim)?;
        if claim.region != self.region {
            return Err(CiPrelaunchUsageJournalError::WrongRegion);
        }
        if reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_PREFIX) {
            return Err(CiPrelaunchUsageJournalError::LegacyReservation);
        }
        if !reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)
            || reserve_handle.len() > 512
            || reserve_handle.chars().any(char::is_control)
        {
            return Err(CiPrelaunchUsageJournalError::ReservationAuthorityMismatch);
        }

        let claim = claim.clone();
        let reserve_handle = reserve_handle.to_owned();
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        let manifest_store = CiDriveManifestStore::new(
            self.pool.clone(),
            TenantId(tenant_id.clone()),
            Region(region.clone()),
        )
        .map_err(|_| CiPrelaunchUsageJournalError::InvalidClaim)?;
        let spec_store = CiJobSpecStore::with_pg(self.pool.clone());

        with_tenant_tx_error(
            &self.pool,
            &tenant_id.clone(),
            &region.clone(),
            move |connection| {
                Box::pin(async move {
                    let locked_claim = CiJobQueueStore::lock_for_token_mint_on_conn(
                        connection,
                        &claim.tenant_id,
                        &claim.region,
                        &claim.job_id,
                        &claim.wf_run_id,
                    )
                    .await
                    .map_err(|_| CiPrelaunchUsageJournalError::ClaimUnavailable)?
                    .ok_or(CiPrelaunchUsageJournalError::ClaimUnavailable)?;
                    verify_locked_claim(&claim, &locked_claim)
                        .map_err(|_| CiPrelaunchUsageJournalError::ClaimUnavailable)?;

                    let run = CiRunStore::lock_for_token_mint_on_conn(
                        connection,
                        &claim.tenant_id,
                        &claim.region,
                        &claim.ci_run_id,
                        &claim.wf_run_id,
                    )
                    .await
                    .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?
                    .ok_or(CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    let (manifest, _) = manifest_store
                        .load_by_identity_on_conn(connection, &claim.wf_run_id, &claim.ci_run_id)
                        .await
                        .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?
                        .ok_or(CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    let launch = spec_store
                        .get_launch_template_on_conn(connection, &claim.tenant_id, &claim.job_id)
                        .await
                        .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    let current_authority = authority_from_durable_claim(
                        &claim, &run, &manifest, &launch,
                    )
                    .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    let authorities = runtime_authorities_from_durable_claim(
                        &claim, &run, &manifest,
                    )
                    .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    let manifest_job = manifest
                        .jobs
                        .iter()
                        .find(|job| job.job_id == claim.job_id)
                        .ok_or(CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
                    if manifest_job.reserve_handle != reserve_handle {
                        return Err(CiPrelaunchUsageJournalError::ManifestReserveHandleMismatch);
                    }
                    if launch.spec.meter_to.reserve_id != reserve_handle {
                        return Err(CiPrelaunchUsageJournalError::DispatchedReserveHandleMismatch);
                    }

                    let reservation = sqlx::query(
                        "SELECT reserved, state FROM cost_reservation
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3
                     FOR UPDATE",
                    )
                    .bind(&claim.tenant_id)
                    .bind(&claim.region)
                    .bind(&reserve_handle)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(map_sql_error)?
                    .ok_or(CiPrelaunchUsageJournalError::ReservationUnavailable)?;
                    let durable_amount: i64 = reservation
                        .try_get("reserved")
                        .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
                    let reservation_state: String = reservation
                        .try_get("state")
                        .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
                    let policy = verify_v2_operational_reservation(
                        &authorities,
                        &claim.ci_run_id,
                        &claim.job_id,
                        &reserve_handle,
                        durable_amount,
                    )
                    .map_err(|_| CiPrelaunchUsageJournalError::ReservationAuthorityMismatch)?;
                    match reservation_state.as_str() {
                        "inflight" => {}
                        "reserved" => {
                            let updated = sqlx::query(
                                "UPDATE cost_reservation SET state = 'inflight'
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3
                               AND state = 'reserved'",
                            )
                            .bind(&claim.tenant_id)
                            .bind(&claim.region)
                            .bind(&reserve_handle)
                            .execute(&mut *connection)
                            .await
                            .map_err(map_sql_error)?;
                            if updated.rows_affected() != 1 {
                                return Err(CiPrelaunchUsageJournalError::ReservationNotLaunchable);
                            }
                        }
                        _ => {
                            return Err(CiPrelaunchUsageJournalError::ReservationNotLaunchable);
                        }
                    }

                    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                        .bind(format!(
                            "myelin.ci.parent-attempt.v1:{}:{}:{}",
                            claim.tenant_id, claim.region, claim.job_id
                        ))
                        .execute(&mut *connection)
                        .await
                        .map_err(map_sql_error)?;

                    let outcome = insert_or_replay_parent_attempt(
                        connection,
                        &claim,
                        &reserve_handle,
                        policy,
                    )
                    .await?;
                    Ok((
                        CiJobParentAttempt {
                            tenant_id: claim.tenant_id,
                            region: claim.region,
                            job_id: claim.job_id,
                            lease_epoch: claim.lease_epoch,
                            claim_nonce: claim.claim_nonce,
                            authority: current_authority,
                        },
                        outcome,
                    ))
                })
            },
        )
        .await
    }

    /// Begin one checkout phase with the ceiling derived from the verified durable job authority.
    pub async fn begin_phase(
        &self,
        attempt: &CiJobParentAttempt,
        phase: CiPrelaunchUsagePhase,
    ) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
        self.require_attempt_scope(attempt)?;
        if attempt.authority.checkout.is_none() {
            return Err(CiPrelaunchUsageJournalError::PhaseUnavailable);
        }
        let ceiling = phase_ceiling(&attempt.authority, phase)?;
        let attempt = attempt.clone();
        let region = self.region.clone();
        with_tenant_tx_error(
            &self.pool,
            &attempt.tenant_id.clone(),
            &region,
            move |connection| {
                Box::pin(async move {
                    require_parent_attempt(connection, &attempt).await?;
                    let inserted = sqlx::query(
                        "INSERT INTO ci_job_prelaunch_usage (
                       tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
                       ceiling_cpu_seconds, ceiling_mem_byte_seconds
                     ) VALUES ($1, $2, $3::uuid, $4, $5::uuid, $6, 'started',
                               $7::text::numeric, $8::text::numeric)
                     ON CONFLICT DO NOTHING",
                    )
                    .bind(&attempt.tenant_id)
                    .bind(&attempt.region)
                    .bind(&attempt.job_id)
                    .bind(attempt.lease_epoch)
                    .bind(&attempt.claim_nonce)
                    .bind(phase.token())
                    .bind(ceiling.cpu_seconds.to_string())
                    .bind(ceiling.mem_byte_seconds.to_string())
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sql_error)?;
                    if inserted.rows_affected() == 1 {
                        return Ok(CiPrelaunchJournalOutcome::Applied);
                    }
                    let row = load_phase(connection, &attempt, phase)
                        .await?
                        .ok_or(CiPrelaunchUsageJournalError::PhaseDivergence)?;
                    if row.ceiling == ceiling {
                        Ok(CiPrelaunchJournalOutcome::Replayed)
                    } else {
                        Err(CiPrelaunchUsageJournalError::PhaseDivergence)
                    }
                })
            },
        )
        .await
    }

    /// Complete a begun phase with exact measured usage. Exact acknowledgement-loss replay is
    /// accepted; a different measurement or a previously sealed phase is refused.
    pub async fn complete_phase(
        &self,
        attempt: &CiJobParentAttempt,
        phase: CiPrelaunchUsagePhase,
        usage: ResourceUsage,
    ) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
        self.resolve_phase(attempt, phase, PhaseResolution::Measured(usage))
            .await
    }

    /// Seal a begun phase at its immutable ceiling when teardown/measurement cannot be proven.
    pub async fn seal_phase(
        &self,
        attempt: &CiJobParentAttempt,
        phase: CiPrelaunchUsagePhase,
    ) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
        self.resolve_phase(attempt, phase, PhaseResolution::Sealed)
            .await
    }

    async fn resolve_phase(
        &self,
        attempt: &CiJobParentAttempt,
        phase: CiPrelaunchUsagePhase,
        resolution: PhaseResolution,
    ) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
        self.require_attempt_scope(attempt)?;
        let attempt = attempt.clone();
        let region = self.region.clone();
        with_tenant_tx_error(
            &self.pool,
            &attempt.tenant_id.clone(),
            &region,
            move |connection| {
                Box::pin(async move {
                    require_parent_attempt(connection, &attempt).await?;
                    let (status, cpu, mem) = match resolution {
                        PhaseResolution::Measured(usage) => (
                            "measured",
                            Some(usage.cpu_seconds.to_string()),
                            Some(usage.mem_byte_seconds.to_string()),
                        ),
                        PhaseResolution::Sealed => ("sealed_ceiling", None, None),
                    };
                    let updated = sqlx::query(
                        "UPDATE ci_job_prelaunch_usage
                     SET status = $7,
                         exact_cpu_seconds = $8::text::numeric,
                         exact_mem_byte_seconds = $9::text::numeric,
                         resolved_at = statement_timestamp()
                     WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                       AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6
                       AND status = 'started'",
                    )
                    .bind(&attempt.tenant_id)
                    .bind(&attempt.region)
                    .bind(&attempt.job_id)
                    .bind(attempt.lease_epoch)
                    .bind(&attempt.claim_nonce)
                    .bind(phase.token())
                    .bind(status)
                    .bind(cpu)
                    .bind(mem)
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sql_error)?;
                    if updated.rows_affected() == 1 {
                        return Ok(CiPrelaunchJournalOutcome::Applied);
                    }
                    let existing = load_phase(connection, &attempt, phase)
                        .await?
                        .ok_or(CiPrelaunchUsageJournalError::PhaseUnavailable)?;
                    match (resolution, existing.status.as_str(), existing.exact) {
                        (PhaseResolution::Measured(expected), "measured", Some(actual))
                            if expected == actual =>
                        {
                            Ok(CiPrelaunchJournalOutcome::Replayed)
                        }
                        (PhaseResolution::Sealed, "sealed_ceiling", None) => {
                            Ok(CiPrelaunchJournalOutcome::Replayed)
                        }
                        (PhaseResolution::Measured(_), "measured", Some(_)) => {
                            Err(CiPrelaunchUsageJournalError::PhaseDivergence)
                        }
                        _ => Err(CiPrelaunchUsageJournalError::IllegalPhaseTransition),
                    }
                })
            },
        )
        .await
    }

    fn require_attempt_scope(
        &self,
        attempt: &CiJobParentAttempt,
    ) -> Result<(), CiPrelaunchUsageJournalError> {
        if attempt.region != self.region {
            Err(CiPrelaunchUsageJournalError::WrongRegion)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum PhaseResolution {
    Measured(ResourceUsage),
    Sealed,
}

struct DurablePhase {
    status: String,
    ceiling: ResourceUsage,
    exact: Option<ResourceUsage>,
}

async fn insert_or_replay_parent_attempt(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    policy: CiAttemptBudgetPolicy,
) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
    let rows = sqlx::query(
        "SELECT wf_run_id::text AS wf_run_id, ci_run_id::text AS ci_run_id,
                reserve_handle, lease_owner, lease_epoch, claim_nonce::text AS claim_nonce,
                claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
                budget_revision, max_parent_attempts
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND (lease_epoch = $4 OR claim_nonce = $5::uuid)",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if !rows.is_empty() {
        if rows.len() != 1 || !parent_row_matches(&rows[0], claim, reserve_handle, policy)? {
            return Err(CiPrelaunchUsageJournalError::ParentAttemptDivergence);
        }
        return Ok(CiPrelaunchJournalOutcome::Replayed);
    }

    let existing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if u64::try_from(existing)
        .ok()
        .is_none_or(|count| count >= u64::from(policy.max_parent_attempts().get()))
    {
        return Err(CiPrelaunchUsageJournalError::ParentAttemptLimitExceeded);
    }

    sqlx::query(
        "INSERT INTO ci_job_parent_attempt (
           tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
           lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
           budget_revision, max_parent_attempts
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid, $10, $11, $12, $13)",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .bind(i16::from(policy.revision().tag()))
    .bind(i64::from(policy.max_parent_attempts().get()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(CiPrelaunchJournalOutcome::Applied)
}

fn parent_row_matches(
    row: &sqlx::postgres::PgRow,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    policy: CiAttemptBudgetPolicy,
) -> Result<bool, CiPrelaunchUsageJournalError> {
    Ok(row
        .try_get::<String, _>("wf_run_id")
        .map_err(|_| CiPrelaunchUsageJournalError::Database)?
        == claim.wf_run_id
        && row
            .try_get::<String, _>("ci_run_id")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.ci_run_id
        && row
            .try_get::<String, _>("reserve_handle")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == reserve_handle
        && row
            .try_get::<String, _>("lease_owner")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.lease_owner
        && row
            .try_get::<i64, _>("lease_epoch")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.lease_epoch
        && row
            .try_get::<String, _>("claim_nonce")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.claim_nonce
        && row
            .try_get::<i64, _>("claim_started_at_epoch_secs")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.claim_started_at_epoch_secs
        && row
            .try_get::<i64, _>("claim_expires_at_epoch_secs")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == claim.claim_expires_at_epoch_secs
        && row
            .try_get::<i16, _>("budget_revision")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == i16::from(policy.revision().tag())
        && row
            .try_get::<i64, _>("max_parent_attempts")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            == i64::from(policy.max_parent_attempts().get()))
}

async fn require_parent_attempt(
    connection: &mut sqlx::PgConnection,
    attempt: &CiJobParentAttempt,
) -> Result<(), CiPrelaunchUsageJournalError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM ci_job_parent_attempt
           WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
             AND lease_epoch = $4 AND claim_nonce = $5::uuid
         )",
    )
    .bind(&attempt.tenant_id)
    .bind(&attempt.region)
    .bind(&attempt.job_id)
    .bind(attempt.lease_epoch)
    .bind(&attempt.claim_nonce)
    .fetch_one(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if exists {
        Ok(())
    } else {
        Err(CiPrelaunchUsageJournalError::ParentAttemptDivergence)
    }
}

async fn load_phase(
    connection: &mut sqlx::PgConnection,
    attempt: &CiJobParentAttempt,
    phase: CiPrelaunchUsagePhase,
) -> Result<Option<DurablePhase>, CiPrelaunchUsageJournalError> {
    let row = sqlx::query(
        "SELECT status,
                ceiling_cpu_seconds::text AS ceiling_cpu_seconds,
                ceiling_mem_byte_seconds::text AS ceiling_mem_byte_seconds,
                exact_cpu_seconds::text AS exact_cpu_seconds,
                exact_mem_byte_seconds::text AS exact_mem_byte_seconds
         FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6",
    )
    .bind(&attempt.tenant_id)
    .bind(&attempt.region)
    .bind(&attempt.job_id)
    .bind(attempt.lease_epoch)
    .bind(&attempt.claim_nonce)
    .bind(phase.token())
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    row.map(|row| {
        let status = row
            .try_get::<String, _>("status")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
        let ceiling = ResourceUsage {
            cpu_seconds: parse_usage_column(&row, "ceiling_cpu_seconds")?
                .ok_or(CiPrelaunchUsageJournalError::Database)?,
            mem_byte_seconds: parse_usage_column(&row, "ceiling_mem_byte_seconds")?
                .ok_or(CiPrelaunchUsageJournalError::Database)?,
        };
        let exact_cpu = parse_usage_column(&row, "exact_cpu_seconds")?;
        let exact_mem = parse_usage_column(&row, "exact_mem_byte_seconds")?;
        let exact = match (exact_cpu, exact_mem) {
            (Some(cpu_seconds), Some(mem_byte_seconds)) => Some(ResourceUsage {
                cpu_seconds,
                mem_byte_seconds,
            }),
            (None, None) => None,
            _ => return Err(CiPrelaunchUsageJournalError::Database),
        };
        Ok(DurablePhase {
            status,
            ceiling,
            exact,
        })
    })
    .transpose()
}

fn parse_usage_column(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<u64>, CiPrelaunchUsageJournalError> {
    row.try_get::<Option<String>, _>(column)
        .map_err(|_| CiPrelaunchUsageJournalError::Database)?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CiPrelaunchUsageJournalError::Database)
        })
        .transpose()
}

fn phase_ceiling(
    authority: &CiJobRuntimeAuthorityRequest,
    phase: CiPrelaunchUsagePhase,
) -> Result<ResourceUsage, CiPrelaunchUsageJournalError> {
    let raw = raw_execution_usage_ceiling(authority)
        .map_err(|_| CiPrelaunchUsageJournalError::UsageOverflow)?;
    let factor = phase.execution_count();
    Ok(ResourceUsage {
        cpu_seconds: raw
            .cpu_seconds
            .checked_mul(factor)
            .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?,
        mem_byte_seconds: raw
            .mem_byte_seconds
            .checked_mul(factor)
            .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?,
    })
}

fn map_sql_error(error: sqlx::Error) -> CiPrelaunchUsageJournalError {
    if error
        .as_database_error()
        .and_then(|database| database.code())
        .as_deref()
        == Some("P0001")
    {
        CiPrelaunchUsageJournalError::IllegalPhaseTransition
    } else {
        CiPrelaunchUsageJournalError::Database
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CiManifestLimitsV1;
    use myelin_ci_sandbox::{derive_checkout_authorization_scope, JobKind, WorkspaceSpec};

    fn authority(checkout: bool) -> CiJobRuntimeAuthorityRequest {
        let checkout = checkout.then(|| {
            derive_checkout_authorization_scope(
                JobKind::Ci,
                &WorkspaceSpec {
                    repo_ref: Some("myelin://acme/git/repo/core".into()),
                    commit: Some("a".repeat(40)),
                },
            )
            .unwrap()
            .unwrap()
        });
        CiJobRuntimeAuthorityRequest {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            ci_run_id: "11111111-1111-1111-1111-111111111111".into(),
            wf_run_id: "22222222-2222-2222-2222-222222222222".into(),
            project_id: "33333333-3333-3333-3333-333333333333".into(),
            job_id: "44444444-4444-4444-4444-444444444444".into(),
            stage: "build".into(),
            concrete_name: "build".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            source_snapshot_digest: "a".repeat(64),
            workflow_definition_version: 1,
            workflow_code_hash: "b".repeat(64),
            policy_revision: "linux-small-v1:1".into(),
            limits: CiManifestLimitsV1 {
                cpu_millis: 1_000,
                mem_bytes: 1024,
                disk_bytes: 1024,
                pids_max: 1,
                timeout_secs: 10,
            },
            checkout,
        }
    }

    #[test]
    fn phase_ceilings_use_two_transport_executions_and_one_materialization_execution() {
        let authority = authority(true);
        assert_eq!(
            phase_ceiling(&authority, CiPrelaunchUsagePhase::CheckoutTransport).unwrap(),
            ResourceUsage {
                cpu_seconds: 20,
                mem_byte_seconds: 20_480,
            }
        );
        assert_eq!(
            phase_ceiling(&authority, CiPrelaunchUsagePhase::CheckoutMaterialization).unwrap(),
            ResourceUsage {
                cpu_seconds: 10,
                mem_byte_seconds: 10_240,
            }
        );
    }

    #[test]
    fn phase_tokens_are_the_schema_vocabulary() {
        assert_eq!(
            CiPrelaunchUsagePhase::CheckoutTransport.token(),
            "checkout_transport"
        );
        assert_eq!(
            CiPrelaunchUsagePhase::CheckoutMaterialization.token(),
            "checkout_materialization"
        );
    }
}
