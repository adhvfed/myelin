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

const PRELAUNCH_PHASE_DEADLINE_HEADROOM_SECS: u64 = 600;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchJournalOutcome {
    Applied,
    Replayed,
}

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
    MissingParentAttempt,
    SettlementIdentityMismatch,
    UnresolvedPhase,
    DeadlineUnavailable,
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
            Self::MissingParentAttempt => {
                "the v2 reservation has no durable parent-attempt journal"
            }
            Self::SettlementIdentityMismatch => {
                "the prelaunch journal differs from the settlement identity"
            }
            Self::UnresolvedPhase => "a prelaunch phase remains unresolved",
            Self::DeadlineUnavailable => {
                "the prelaunch phase has no verified topology-aware sealing deadline"
            }
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

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CiParentAttemptAdmission {
    Admitted {
        attempt: CiJobParentAttempt,
        outcome: CiPrelaunchJournalOutcome,
    },
    AttemptsExhausted {
        reserve_handle: String,
    },
}

#[derive(Clone)]
pub struct CiPrelaunchUsageJournal {
    pool: PgPool,
    region: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchParentExpectation {
    Required,
    OptionalBeforeLaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiPrelaunchUnresolvedPolicy {
    Refuse,
    SealToCeiling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiPrelaunchUsageAccrual {
    pub usage: ResourceUsage,
    pub parent_attempts: u64,
    pub measured_phases: u64,
    pub sealed_phases: u64,
}

impl Default for CiPrelaunchUsageAccrual {
    fn default() -> Self {
        Self {
            usage: ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
            parent_attempts: 0,
            measured_phases: 0,
            sealed_phases: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CiPrelaunchSettlementIdentity<'a> {
    pub tenant_id: &'a str,
    pub region: &'a str,
    pub job_id: &'a str,
    pub wf_run_id: &'a str,
    pub ci_run_id: &'a str,
    pub reserve_handle: &'a str,
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

    pub async fn begin_parent_attempt(
        &self,
        claim: &CiJobTokenRequest,
        reserve_handle: &str,
    ) -> Result<(CiJobParentAttempt, CiPrelaunchJournalOutcome), CiPrelaunchUsageJournalError> {
        let (claim, reserve_handle, manifest_store, spec_store) =
            self.prepare_admission(claim, reserve_handle)?;
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        with_tenant_tx_error(&self.pool, &tenant_id, &region, move |connection| {
            Box::pin(async move {
                match run_parent_admission_on_conn(
                    connection,
                    &claim,
                    &reserve_handle,
                    &manifest_store,
                    &spec_store,
                )
                .await?
                {
                    ParentAdmissionTxOutcome::Admitted(attempt, outcome) => Ok((attempt, outcome)),
                    ParentAdmissionTxOutcome::Exhausted => {
                        Err(CiPrelaunchUsageJournalError::ParentAttemptLimitExceeded)
                    }
                }
            })
        })
        .await
    }

    pub async fn admit_parent_attempt(
        &self,
        claim: &CiJobTokenRequest,
        reserve_handle: &str,
    ) -> Result<CiParentAttemptAdmission, CiPrelaunchUsageJournalError> {
        let (claim, reserve_handle, manifest_store, spec_store) =
            self.prepare_admission(claim, reserve_handle)?;
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        with_tenant_tx_error(&self.pool, &tenant_id, &region, move |connection| {
            Box::pin(async move {
                match run_parent_admission_on_conn(
                    connection,
                    &claim,
                    &reserve_handle,
                    &manifest_store,
                    &spec_store,
                )
                .await?
                {
                    ParentAdmissionTxOutcome::Admitted(attempt, outcome) => {
                        Ok(CiParentAttemptAdmission::Admitted { attempt, outcome })
                    }
                    ParentAdmissionTxOutcome::Exhausted => {
                        Ok(CiParentAttemptAdmission::AttemptsExhausted { reserve_handle })
                    }
                }
            })
        })
        .await
    }

    fn prepare_admission(
        &self,
        claim: &CiJobTokenRequest,
        reserve_handle: &str,
    ) -> Result<
        (
            CiJobTokenRequest,
            String,
            CiDriveManifestStore,
            CiJobSpecStore,
        ),
        CiPrelaunchUsageJournalError,
    > {
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
        let manifest_store = CiDriveManifestStore::new(
            self.pool.clone(),
            TenantId(claim.tenant_id.clone()),
            Region(claim.region.clone()),
        )
        .map_err(|_| CiPrelaunchUsageJournalError::InvalidClaim)?;
        let spec_store = CiJobSpecStore::with_pg(self.pool.clone());
        Ok((claim, reserve_handle.to_owned(), manifest_store, spec_store))
    }

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
        let seal_window_secs = phase_seal_window_secs(&attempt.authority, phase)?;
        let attempt = attempt.clone();
        let region = self.region.clone();
        with_tenant_tx_error(
            &self.pool,
            &attempt.tenant_id.clone(),
            &region,
            move |connection| {
                Box::pin(async move {
                    require_live_parent_attempt_on_conn(connection, &attempt).await?;
                    lock_parent_attempt_job_on_conn(
                        connection,
                        &attempt.tenant_id,
                        &attempt.region,
                        &attempt.job_id,
                    )
                    .await?;
                    let inserted = sqlx::query(
                        "INSERT INTO ci_job_prelaunch_usage (
                       tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
                       ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, seal_after
                     ) VALUES ($1, $2, $3::uuid, $4, $5::uuid, $6, 'started',
                               $7::text::numeric, $8::text::numeric, statement_timestamp(),
                               statement_timestamp() + ($9::text || ' seconds')::interval)
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
                    .bind(seal_window_secs.to_string())
                    .execute(&mut *connection)
                    .await
                    .map_err(map_sql_error)?;
                    if inserted.rows_affected() == 1 {
                        return Ok(CiPrelaunchJournalOutcome::Applied);
                    }
                    let row = load_phase(connection, &attempt, phase)
                        .await?
                        .ok_or(CiPrelaunchUsageJournalError::PhaseDivergence)?;
                    if row.ceiling == ceiling && row.seal_window_secs == Some(seal_window_secs) {
                        Ok(CiPrelaunchJournalOutcome::Replayed)
                    } else {
                        Err(CiPrelaunchUsageJournalError::PhaseDivergence)
                    }
                })
            },
        )
        .await
    }

    pub async fn complete_phase(
        &self,
        attempt: &CiJobParentAttempt,
        phase: CiPrelaunchUsagePhase,
        usage: ResourceUsage,
    ) -> Result<CiPrelaunchJournalOutcome, CiPrelaunchUsageJournalError> {
        self.resolve_phase(attempt, phase, PhaseResolution::Measured(usage))
            .await
    }

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
                    require_live_parent_attempt_on_conn(connection, &attempt).await?;
                    lock_parent_attempt_job_on_conn(
                        connection,
                        &attempt.tenant_id,
                        &attempt.region,
                        &attempt.job_id,
                    )
                    .await?;
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

    pub async fn parent_attempt_retry_permitted(
        &self,
        claim: &CiJobTokenRequest,
        reserve_handle: &str,
    ) -> Result<bool, CiPrelaunchUsageJournalError> {
        claim
            .validate()
            .map_err(|_| CiPrelaunchUsageJournalError::InvalidClaim)?;
        if claim.region != self.region {
            return Err(CiPrelaunchUsageJournalError::WrongRegion);
        }
        let claim = claim.clone();
        let reserve_handle = reserve_handle.to_owned();
        let region = self.region.clone();
        with_tenant_tx_error(
            &self.pool,
            &claim.tenant_id.clone(),
            &region,
            move |connection| {
                Box::pin(async move {
                    lock_parent_attempt_job_on_conn(
                        connection,
                        &claim.tenant_id,
                        &claim.region,
                        &claim.job_id,
                    )
                    .await?;
                    let policies = sqlx::query(
                        "SELECT budget_revision, max_parent_attempts
                         FROM ci_job_parent_attempt
                         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid AND reserve_handle = $6
                         GROUP BY budget_revision, max_parent_attempts",
                    )
                    .bind(&claim.tenant_id)
                    .bind(&claim.region)
                    .bind(&claim.job_id)
                    .bind(&claim.wf_run_id)
                    .bind(&claim.ci_run_id)
                    .bind(&reserve_handle)
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(map_sql_error)?;
                    if policies.len() != 1 {
                        return Ok(false);
                    }
                    let revision: i16 = policies[0]
                        .try_get("budget_revision")
                        .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
                    let maximum: i64 = policies[0]
                        .try_get("max_parent_attempts")
                        .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
                    let count: i64 = sqlx::query_scalar(
                        "SELECT count(*)
                         FROM ci_job_parent_attempt
                         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid AND reserve_handle = $6
                           AND budget_revision = $7 AND max_parent_attempts = $8",
                    )
                    .bind(&claim.tenant_id)
                    .bind(&claim.region)
                    .bind(&claim.job_id)
                    .bind(&claim.wf_run_id)
                    .bind(&claim.ci_run_id)
                    .bind(&reserve_handle)
                    .bind(revision)
                    .bind(maximum)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(map_sql_error)?;
                    Ok(count < maximum)
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

pub async fn resolve_prelaunch_usage_on_conn(
    connection: &mut sqlx::PgConnection,
    identity: CiPrelaunchSettlementIdentity<'_>,
    parent_expectation: CiPrelaunchParentExpectation,
    unresolved_policy: CiPrelaunchUnresolvedPolicy,
) -> Result<CiPrelaunchUsageAccrual, CiPrelaunchUsageJournalError> {
    let queue = sqlx::query_scalar::<_, String>(
        "SELECT job_id::text
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND run_id = $4::uuid
         FOR UPDATE",
    )
    .bind(identity.tenant_id)
    .bind(identity.region)
    .bind(identity.job_id)
    .bind(identity.wf_run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if queue.is_none() && parent_expectation == CiPrelaunchParentExpectation::Required {
        return Err(CiPrelaunchUsageJournalError::SettlementIdentityMismatch);
    }

    lock_parent_attempt_job_on_conn(
        connection,
        identity.tenant_id,
        identity.region,
        identity.job_id,
    )
    .await?;

    let is_v1 = identity
        .reserve_handle
        .starts_with(TIER_P_OPERATIONAL_RESERVATION_PREFIX);
    let is_v2 = identity
        .reserve_handle
        .starts_with(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX);
    if !is_v1 && !is_v2 {
        return Err(CiPrelaunchUsageJournalError::SettlementIdentityMismatch);
    }

    let parents = sqlx::query(
        "SELECT wf_run_id::text AS wf_run_id, ci_run_id::text AS ci_run_id, reserve_handle
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
         ORDER BY begun_at, lease_epoch, claim_nonce",
    )
    .bind(identity.tenant_id)
    .bind(identity.region)
    .bind(identity.job_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if queue.is_none() && !parents.is_empty() {
        return Err(CiPrelaunchUsageJournalError::SettlementIdentityMismatch);
    }
    if is_v1 {
        return if parents.is_empty() {
            Ok(CiPrelaunchUsageAccrual::default())
        } else {
            Err(CiPrelaunchUsageJournalError::SettlementIdentityMismatch)
        };
    }
    if parents.is_empty() {
        return match parent_expectation {
            CiPrelaunchParentExpectation::Required => {
                Err(CiPrelaunchUsageJournalError::MissingParentAttempt)
            }
            CiPrelaunchParentExpectation::OptionalBeforeLaunch => {
                Ok(CiPrelaunchUsageAccrual::default())
            }
        };
    }
    for parent in &parents {
        if parent
            .try_get::<String, _>("wf_run_id")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            != identity.wf_run_id
            || parent
                .try_get::<String, _>("ci_run_id")
                .map_err(|_| CiPrelaunchUsageJournalError::Database)?
                != identity.ci_run_id
            || parent
                .try_get::<String, _>("reserve_handle")
                .map_err(|_| CiPrelaunchUsageJournalError::Database)?
                != identity.reserve_handle
        {
            return Err(CiPrelaunchUsageJournalError::SettlementIdentityMismatch);
        }
    }

    if unresolved_policy == CiPrelaunchUnresolvedPolicy::SealToCeiling {
        sqlx::query(
            "UPDATE ci_job_prelaunch_usage u
             SET status = 'sealed_ceiling', resolved_at = statement_timestamp()
             FROM ci_job_parent_attempt p
             WHERE u.tenant_id = p.tenant_id AND u.region = p.region AND u.job_id = p.job_id
               AND u.lease_epoch = p.lease_epoch AND u.claim_nonce = p.claim_nonce
               AND u.tenant_id = $1 AND u.region = $2 AND u.job_id = $3::uuid
               AND p.wf_run_id = $4::uuid AND p.ci_run_id = $5::uuid
               AND p.reserve_handle = $6 AND u.status = 'started'",
        )
        .bind(identity.tenant_id)
        .bind(identity.region)
        .bind(identity.job_id)
        .bind(identity.wf_run_id)
        .bind(identity.ci_run_id)
        .bind(identity.reserve_handle)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }

    let phases = sqlx::query(
        "SELECT u.status,
                u.ceiling_cpu_seconds::text AS ceiling_cpu_seconds,
                u.ceiling_mem_byte_seconds::text AS ceiling_mem_byte_seconds,
                u.exact_cpu_seconds::text AS exact_cpu_seconds,
                u.exact_mem_byte_seconds::text AS exact_mem_byte_seconds,
                u.seal_after IS NOT NULL AS has_seal_deadline
         FROM ci_job_prelaunch_usage u
         JOIN ci_job_parent_attempt p
           ON p.tenant_id = u.tenant_id AND p.region = u.region AND p.job_id = u.job_id
          AND p.lease_epoch = u.lease_epoch AND p.claim_nonce = u.claim_nonce
         WHERE u.tenant_id = $1 AND u.region = $2 AND u.job_id = $3::uuid
           AND p.wf_run_id = $4::uuid AND p.ci_run_id = $5::uuid
           AND p.reserve_handle = $6
         ORDER BY p.begun_at, u.phase",
    )
    .bind(identity.tenant_id)
    .bind(identity.region)
    .bind(identity.job_id)
    .bind(identity.wf_run_id)
    .bind(identity.ci_run_id)
    .bind(identity.reserve_handle)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sql_error)?;

    let mut accrual = CiPrelaunchUsageAccrual {
        parent_attempts: u64::try_from(parents.len())
            .map_err(|_| CiPrelaunchUsageJournalError::UsageOverflow)?,
        ..CiPrelaunchUsageAccrual::default()
    };
    for phase in phases {
        let status = phase
            .try_get::<String, _>("status")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
        let ceiling =
            usage_from_columns(&phase, "ceiling_cpu_seconds", "ceiling_mem_byte_seconds")?
                .ok_or(CiPrelaunchUsageJournalError::Database)?;
        let exact = usage_from_columns(&phase, "exact_cpu_seconds", "exact_mem_byte_seconds")?;
        let contribution = match status.as_str() {
            "started" => {
                let has_deadline = phase
                    .try_get::<bool, _>("has_seal_deadline")
                    .map_err(|_| CiPrelaunchUsageJournalError::Database)?;
                if !has_deadline {
                    return Err(CiPrelaunchUsageJournalError::DeadlineUnavailable);
                }
                return Err(CiPrelaunchUsageJournalError::UnresolvedPhase);
            }
            "measured" => {
                accrual.measured_phases = accrual
                    .measured_phases
                    .checked_add(1)
                    .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?;
                exact.ok_or(CiPrelaunchUsageJournalError::Database)?
            }
            "sealed_ceiling" => {
                if exact.is_some() {
                    return Err(CiPrelaunchUsageJournalError::Database);
                }
                accrual.sealed_phases = accrual
                    .sealed_phases
                    .checked_add(1)
                    .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?;
                ceiling
            }
            _ => return Err(CiPrelaunchUsageJournalError::Database),
        };
        accrual.usage = checked_add_usage(accrual.usage, contribution)?;
    }
    Ok(accrual)
}

struct DurablePhase {
    status: String,
    ceiling: ResourceUsage,
    exact: Option<ResourceUsage>,
    seal_window_secs: Option<u64>,
}

#[allow(clippy::large_enum_variant)]
enum ParentAdmissionTxOutcome {
    Admitted(CiJobParentAttempt, CiPrelaunchJournalOutcome),
    Exhausted,
}

enum InsertOrReplayOutcome {
    Journaled(CiPrelaunchJournalOutcome),
    Exhausted,
}

async fn run_parent_admission_on_conn(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    manifest_store: &CiDriveManifestStore,
    spec_store: &CiJobSpecStore,
) -> Result<ParentAdmissionTxOutcome, CiPrelaunchUsageJournalError> {
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
    verify_locked_claim(claim, &locked_claim)
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
    let current_authority = authority_from_durable_claim(claim, &run, &manifest, &launch)
        .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
    let authorities = runtime_authorities_from_durable_claim(claim, &run, &manifest)
        .map_err(|_| CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
    let manifest_job = manifest
        .jobs
        .iter()
        .find(|job| job.job_id == claim.job_id)
        .ok_or(CiPrelaunchUsageJournalError::DurableAuthorityUnavailable)?;
    if manifest_job.reserve_handle.as_str() != reserve_handle {
        return Err(CiPrelaunchUsageJournalError::ManifestReserveHandleMismatch);
    }
    if launch.spec.meter_to.reserve_id.as_str() != reserve_handle {
        return Err(CiPrelaunchUsageJournalError::DispatchedReserveHandleMismatch);
    }

    let reservation = sqlx::query(
        "SELECT reserved, state FROM cost_reservation
     WHERE tenant_id = $1 AND region = $2 AND run_id = $3
     FOR UPDATE",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(reserve_handle)
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
        reserve_handle,
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
            .bind(reserve_handle)
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

    lock_parent_attempt_job_on_conn(connection, &claim.tenant_id, &claim.region, &claim.job_id)
        .await?;

    match insert_or_replay_parent_attempt(connection, claim, reserve_handle, policy).await? {
        InsertOrReplayOutcome::Journaled(outcome) => Ok(ParentAdmissionTxOutcome::Admitted(
            CiJobParentAttempt {
                tenant_id: claim.tenant_id.clone(),
                region: claim.region.clone(),
                job_id: claim.job_id.clone(),
                lease_epoch: claim.lease_epoch,
                claim_nonce: claim.claim_nonce.clone(),
                authority: current_authority,
            },
            outcome,
        )),
        InsertOrReplayOutcome::Exhausted => Ok(ParentAdmissionTxOutcome::Exhausted),
    }
}

async fn insert_or_replay_parent_attempt(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    policy: CiAttemptBudgetPolicy,
) -> Result<InsertOrReplayOutcome, CiPrelaunchUsageJournalError> {
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
        return Ok(InsertOrReplayOutcome::Journaled(
            CiPrelaunchJournalOutcome::Replayed,
        ));
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
        return Ok(InsertOrReplayOutcome::Exhausted);
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
    Ok(InsertOrReplayOutcome::Journaled(
        CiPrelaunchJournalOutcome::Applied,
    ))
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

async fn lock_parent_attempt_job_on_conn(
    connection: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    job_id: &str,
) -> Result<(), CiPrelaunchUsageJournalError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.parent-attempt.v1:{tenant_id}:{region}:{job_id}"
        ))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    Ok(())
}

async fn require_live_parent_attempt_on_conn(
    connection: &mut sqlx::PgConnection,
    attempt: &CiJobParentAttempt,
) -> Result<(), CiPrelaunchUsageJournalError> {
    let live = sqlx::query_scalar::<_, String>(
        "SELECT q.job_id::text
         FROM ci_job_parent_attempt p
         JOIN job_queue q
           ON q.tenant_id = p.tenant_id AND q.region = p.region AND q.job_id = p.job_id
          AND q.run_id = p.wf_run_id
         WHERE p.tenant_id = $1 AND p.region = $2 AND p.job_id = $3::uuid
           AND p.lease_epoch = $4 AND p.claim_nonce = $5::uuid
           AND q.state IN ('leased', 'running')
           AND q.lease_owner = p.lease_owner
           AND q.lease_epoch = p.lease_epoch
           AND q.claim_nonce = p.claim_nonce
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint =
               p.claim_started_at_epoch_secs
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint =
               p.claim_expires_at_epoch_secs
           AND q.claim_expires_at > statement_timestamp()
           AND q.completion_receipt IS NULL
         FOR UPDATE OF q",
    )
    .bind(&attempt.tenant_id)
    .bind(&attempt.region)
    .bind(&attempt.job_id)
    .bind(attempt.lease_epoch)
    .bind(&attempt.claim_nonce)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if live.is_some() {
        Ok(())
    } else {
        Err(CiPrelaunchUsageJournalError::ClaimUnavailable)
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
                exact_mem_byte_seconds::text AS exact_mem_byte_seconds,
                EXTRACT(EPOCH FROM (seal_after - started_at))::bigint::text AS seal_window_secs
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
        let seal_window_secs = row
            .try_get::<Option<String>, _>("seal_window_secs")
            .map_err(|_| CiPrelaunchUsageJournalError::Database)?
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| CiPrelaunchUsageJournalError::Database)
            })
            .transpose()?;
        Ok(DurablePhase {
            status,
            ceiling,
            exact,
            seal_window_secs,
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

fn usage_from_columns(
    row: &sqlx::postgres::PgRow,
    cpu_column: &str,
    mem_column: &str,
) -> Result<Option<ResourceUsage>, CiPrelaunchUsageJournalError> {
    match (
        parse_usage_column(row, cpu_column)?,
        parse_usage_column(row, mem_column)?,
    ) {
        (Some(cpu_seconds), Some(mem_byte_seconds)) => Ok(Some(ResourceUsage {
            cpu_seconds,
            mem_byte_seconds,
        })),
        (None, None) => Ok(None),
        _ => Err(CiPrelaunchUsageJournalError::Database),
    }
}

fn checked_add_usage(
    left: ResourceUsage,
    right: ResourceUsage,
) -> Result<ResourceUsage, CiPrelaunchUsageJournalError> {
    Ok(ResourceUsage {
        cpu_seconds: left
            .cpu_seconds
            .checked_add(right.cpu_seconds)
            .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?,
        mem_byte_seconds: left
            .mem_byte_seconds
            .checked_add(right.mem_byte_seconds)
            .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)?,
    })
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

fn phase_seal_window_secs(
    authority: &CiJobRuntimeAuthorityRequest,
    phase: CiPrelaunchUsagePhase,
) -> Result<u64, CiPrelaunchUsageJournalError> {
    u64::from(authority.limits.timeout_secs)
        .checked_mul(phase.execution_count())
        .and_then(|seconds| seconds.checked_add(PRELAUNCH_PHASE_DEADLINE_HEADROOM_SECS))
        .ok_or(CiPrelaunchUsageJournalError::UsageOverflow)
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
            reserve_id: Some("ci-reserve:v2:fixture".into()),
            checkout_commit: checkout.as_ref().map(|scope| scope.commit_hex().to_owned()),
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

    #[test]
    fn phase_seal_windows_cover_the_full_topology_plus_headroom() {
        let mut authority = authority(true);
        authority.limits.timeout_secs = 21_600;
        assert_eq!(
            phase_seal_window_secs(&authority, CiPrelaunchUsagePhase::CheckoutTransport).unwrap(),
            43_800,
            "two full six-hour transport executions plus ten minutes"
        );
        assert_eq!(
            phase_seal_window_secs(&authority, CiPrelaunchUsagePhase::CheckoutMaterialization)
                .unwrap(),
            22_200,
            "one full six-hour materialization execution plus ten minutes"
        );
    }
}
