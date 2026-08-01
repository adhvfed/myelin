//! Policy-owned launch grants for the first executable CI profile.
//!
//! Customer V2 plans may request `linux-small-v1`, but they carry no runtime authority. This module
//! turns that request into fixed, server-owned isolation and scheduling terms. It delegates the
//! durable capacity reservation to one explicit provider, while deriving a content-bound token-authority
//! reference locally; the existing claim-time `CiJobTokenIssuer` remains the only bearer-mint seam.
//! Keeping those capabilities separate matters for Tier P: Identity can become real without
//! inventing the Commercial wallet that remains deliberately deferred. There is no budget-provider
//! default and no caller-controlled egress, limits, labels, or lane.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;

use crate::ci_pipeline_driver::{
    TIER_P_OPERATIONAL_PRICING_REVISION, TIER_P_OPERATIONAL_RESERVATION_PREFIX,
    TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX,
};
use crate::{
    ci_job_id_v2, CiExecutionProfileV1, CiJobAccountingPricer, CiJobLaunchGrantV1,
    CiJobPricingError, CiLaunchAuthorityError, CiLaunchAuthorityMaterializer, CiLaunchAuthorityV1,
    CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiRunRecord,
    CiWorkflowDefinitionPin, PreparedRunPlanV2, PricedCiJobUsage,
};
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CheckoutAuthorizationScope, JobKind, ResourceUsage,
    WorkspaceSpec,
};
use myelin_flow::MinorUnits;
use myelin_storage::{with_tenant_tx, PgError};
use sqlx::{PgPool, Row};

pub const LINUX_SMALL_V1_POLICY_REVISION: &str = "linux-small-v1:1";
/// Exact scheduler labels emitted by the production policy and advertised by its runner pool.
pub const LINUX_SMALL_V1_RUNNER_LABELS: [&str; 2] = ["linux", "linux-small-v1"];
/// Production-for-one ceiling on durable Tier-P reservations for one tenant and region.
///
/// Reservations cover every queued DAG job, so this is deliberately distinct from the measured
/// scheduler cap, which covers only leased/running jobs. An idle tenant can reserve one largest
/// valid run; additional runs fail closed until enough earlier reservations settle.
pub const TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING: u32 = 1_024;
const CI_OPERATIONAL_RESERVATION_V1_DOMAIN: &[u8] = b"myelin.ci.operational-reservation.v1\0";
const CI_OPERATIONAL_BATCH_V1_DOMAIN: &[u8] = b"myelin.ci.operational-reservation-batch.v1\0";
const GIB_BYTES: u64 = 1_073_741_824;

/// Complete immutable request shared by the budget reservation and token-reference derivation. All
/// identity comes from the locked `ci_run` and validated plan; neither path can replace executable
/// or scheduling terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobRuntimeAuthorityRequest {
    pub tenant_id: String,
    pub region: String,
    pub ci_run_id: String,
    pub wf_run_id: String,
    pub project_id: String,
    pub job_id: String,
    pub stage: String,
    pub concrete_name: String,
    pub trigger_kind: String,
    pub trust_tier: String,
    pub source_snapshot_digest: String,
    pub workflow_definition_version: i32,
    pub workflow_code_hash: String,
    pub policy_revision: String,
    pub limits: CiManifestLimitsV1,
    /// CT-007 slice 5b.3-2b: the job's checkout target, when it has one. `None` for an ordinary
    /// compute job (never present in this workspace's CI jobs today, since `ci_run.repo_ref`/
    /// `.commit_oid` are always non-empty for a CI run — see `run_plan::parse_snapshot_ref`
    /// — but kept genuinely optional to mirror `myelin_ci_sandbox::WorkspaceIntent`'s own
    /// Compute/Checkout duality rather than assuming CI is the only caller). Derived ONLY via
    /// [`myelin_ci_sandbox::derive_checkout_authorization_scope`] — never hand-constructed — so the
    /// authority digest and the real `JobSpec` a launch builds always agree on how a workspace is
    /// parsed. Hashed into the `v2` token-authority digest (`token_authority_digest_v2`); the
    /// frozen `v1` digest (`token_authority_digest`) never hashes this field, by design — a `v1`
    /// handle cannot prove checkout authority (see [`ManifestBoundCiJobTokenAuthority`]).
    pub checkout: Option<CheckoutAuthorizationScope>,
}

/// External reservation boundary used by the server policy. The complete job set is one
/// all-or-nothing operation: an implementation must commit every reservation or none, preserve input
/// order in its result, and return the same handles on an exact retry. A wrong cardinality, empty,
/// overlong, control-bearing, or duplicate handle is a provider refusal and must commit nothing.
/// This prevents a later job or malformed response from stranding reservations before the manifest
/// exists.
pub trait CiJobBudgetReservationProvider: Send + Sync {
    fn reserve_batch<'a>(
        &'a self,
        requests: Vec<CiJobRuntimeAuthorityRequest>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>;

    /// Reserve on the starter's already tenant-scoped transaction. External providers may retain
    /// the default exact-retry call, but an in-database implementation overrides this so reservation,
    /// immutable manifest, workflow start, and CI job ledger share one commit.
    fn reserve_batch_in_tx<'a>(
        &'a self,
        _conn: &'a mut sqlx::PgConnection,
        requests: Vec<CiJobRuntimeAuthorityRequest>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>
    {
        self.reserve_batch(requests)
    }
}

/// Durable Tier-P operational reservation source.
///
/// This is deliberately not a wallet. It enforces one explicit per-tenant ceiling on outstanding
/// CI job reservations and writes the existing Storage-owned `cost_reservation` rows that the
/// launch hook and terminal accounting path advance. One complete manifest batch commits in one
/// tenant-scoped PostgreSQL transaction. The transaction takes a tenant/region advisory lock so
/// concurrent fresh batches cannot both observe spare capacity.
///
/// Handles bind the complete immutable runtime-authority request. An exact acknowledgement-loss
/// retry returns the existing handles in input order, regardless of how far their reservation
/// lifecycle has subsequently advanced. Reusing a job id with changed authority or observing a
/// partial prior batch is refused instead of creating a second reservation.
#[derive(Clone)]
pub struct PgTierPCiJobBudgetReservation {
    pool: PgPool,
    region: String,
    max_outstanding_jobs_per_tenant: u32,
    /// Stored explicitly (never re-derived ad hoc) so every fresh `v2` write this provider ever
    /// mints agrees on the same policy. Read-side replay of an EXISTING durable `v2` row never
    /// consults this -- it reconstructs the policy from that row's own embedded descriptor instead,
    /// since a durable row may have been minted under an older policy than whatever is configured
    /// now (see `replay_v2_batch`).
    budget_policy: CiAttemptBudgetPolicy,
    /// CT-007 slice 5b.3-4a.1c: gates whether a genuinely fresh batch (no durable rows yet) mints
    /// `v1` or `v2`. See [`OperationalReservationWriteVersion`].
    write_version: OperationalReservationWriteVersion,
}

/// Completion-side policy paired with [`PgTierPCiJobBudgetReservation`].
///
/// Tier P records internal operational minor-units, not a customer price: one unit per measured
/// CPU-second and one per measured memory-GiB-second, with zero markup. The reservation upper bound
/// uses the exact same dimensions at the server-owned job limits. A future Commercial pricer must
/// not be composed with Tier-P reservation rows; changing unit policy requires a new revision and a
/// coordinated reservation/settlement migration.
#[derive(Clone, Debug, Default)]
pub struct TierPOperationalCiJobPricer;

impl CiJobAccountingPricer for TierPOperationalCiJobPricer {
    fn price(&self, usage: ResourceUsage) -> Result<PricedCiJobUsage, CiJobPricingError> {
        let memory_gb_seconds = usage.mem_byte_seconds.div_ceil(GIB_BYTES);
        usage
            .cpu_seconds
            .checked_add(memory_gb_seconds)
            .ok_or(CiJobPricingError::InvalidOutput)?;
        Ok(PricedCiJobUsage {
            pricing_revision: TIER_P_OPERATIONAL_PRICING_REVISION.into(),
            memory_gb_seconds,
            cpu_wholesale: MinorUnits(usage.cpu_seconds),
            cpu_markup: MinorUnits::ZERO,
            memory_wholesale: MinorUnits(memory_gb_seconds),
            memory_markup: MinorUnits::ZERO,
        })
    }
}

impl PgTierPCiJobBudgetReservation {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        max_outstanding_jobs_per_tenant: u32,
        budget_policy: CiAttemptBudgetPolicy,
        write_version: OperationalReservationWriteVersion,
    ) -> Result<Self, CiLaunchAuthorityError> {
        let region = region.into();
        if !valid_machine_token(&region) {
            return Err(refused("operational reservation region is invalid"));
        }
        if max_outstanding_jobs_per_tenant == 0 {
            return Err(refused("operational reservation ceiling must be positive"));
        }
        Ok(Self {
            pool,
            region,
            max_outstanding_jobs_per_tenant,
            budget_policy,
            write_version,
        })
    }
}

impl CiJobBudgetReservationProvider for PgTierPCiJobBudgetReservation {
    fn reserve_batch<'a>(
        &'a self,
        requests: Vec<CiJobRuntimeAuthorityRequest>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>
    {
        Box::pin(async move {
            let prepared = prepare_operational_batch(
                &self.region,
                self.max_outstanding_jobs_per_tenant,
                requests,
            )?;
            let tenant = prepared[0].request.tenant_id.clone();
            let region = self.region.clone();
            let ceiling = self.max_outstanding_jobs_per_tenant;
            let budget_policy = self.budget_policy;
            let write_version = self.write_version;
            let tx_tenant = tenant.clone();
            let tx_region = region.clone();
            let result = with_tenant_tx(&self.pool, &tenant, &region, move |conn| {
                Box::pin(async move {
                    reserve_operational_batch_on_conn(
                        conn,
                        &tx_tenant,
                        &tx_region,
                        ceiling,
                        &budget_policy,
                        write_version,
                        &prepared,
                    )
                    .await
                })
            })
            .await
            .map_err(|_| refused("durable operational reservation did not commit"))?;
            result
        })
    }

    fn reserve_batch_in_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        requests: Vec<CiJobRuntimeAuthorityRequest>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>
    {
        Box::pin(async move {
            let prepared = prepare_operational_batch(
                &self.region,
                self.max_outstanding_jobs_per_tenant,
                requests,
            )?;
            let tenant = prepared[0].request.tenant_id.clone();
            reserve_operational_batch_on_conn(
                conn,
                &tenant,
                &self.region,
                self.max_outstanding_jobs_per_tenant,
                &self.budget_policy,
                self.write_version,
                &prepared,
            )
            .await
            .map_err(|_| refused("durable operational reservation did not commit"))?
        })
    }
}

/// CT-007 slice 5b.3-4a.1b: fields are named `v1_*` (not just `handle`/`amount`) because this type
/// is now shared with `v2` replay-decision code elsewhere in this module -- but this struct itself
/// carries ONLY `v1` data. Sol's round-1 review: an earlier draft also eagerly computed a `v2`
/// candidate here under the provider's CURRENT policy on every batch, even though nothing writes
/// `v2` this slice -- that meant a fresh `v1` write (or an exact `v1` replay) could fail solely
/// because unrelated, unused `v2` ceiling arithmetic overflowed for the current policy. `v2`
/// candidates are still never computed here; every `v2` computation this module ever does is one of
/// exactly two callers (CT-007 slice 5b.3-4a.1c), each with its own policy:
/// - a genuinely FRESH batch (no durable rows at all) that this provider is configured to WRITE
///   `v2` for -- computed lazily in `reserve_operational_batch_on_conn`'s fresh branch, using the
///   provider's CURRENTLY configured policy, only after replay has already confirmed there is
///   nothing durable to recover instead;
/// - an existing DURABLE `v2` row being replayed -- computed lazily in `replay_v2_batch`, using a
///   policy RECONSTRUCTED from that row's own embedded descriptor, never the current one.
///
/// Neither case ever runs for a `v1`-shaped fresh write or an exact `v1` replay -- those two paths
/// never touch `v2` arithmetic in any form.
#[derive(Clone)]
struct PreparedOperationalReservation {
    request: CiJobRuntimeAuthorityRequest,
    v1_handle: String,
    v1_amount: i64,
}

fn prepare_operational_batch(
    provider_region: &str,
    ceiling: u32,
    requests: Vec<CiJobRuntimeAuthorityRequest>,
) -> Result<Vec<PreparedOperationalReservation>, CiLaunchAuthorityError> {
    let Some(first) = requests.first() else {
        return Err(refused("operational reservation batch is empty"));
    };
    let batch_tenant = first.tenant_id.clone();
    let batch_run = first.ci_run_id.clone();
    if !valid_machine_token(&batch_tenant)
        || first.region != provider_region
        || sqlx::types::Uuid::parse_str(&batch_run).is_err()
    {
        return Err(refused("operational reservation scope is invalid"));
    }
    if requests.len() > ceiling as usize {
        return Err(refused(
            "operational reservation batch exceeds the tenant ceiling",
        ));
    }

    let mut job_ids = BTreeSet::new();
    let mut validated = Vec::with_capacity(requests.len());
    for request in requests {
        if request.tenant_id != batch_tenant
            || request.region != provider_region
            || request.ci_run_id != batch_run
            || sqlx::types::Uuid::parse_str(&request.job_id).is_err()
            || !job_ids.insert(request.job_id.clone())
        {
            return Err(refused(
                "operational reservation batch has divergent scope or duplicate jobs",
            ));
        }
        let amount = operational_reservation_amount(&request.limits)?;
        validated.push((request, amount));
    }
    let batch_digest = operational_batch_digest(&validated);
    let mut prepared = Vec::with_capacity(validated.len());
    for (request, v1_amount) in validated {
        let request_digest =
            runtime_authority_digest(CI_OPERATIONAL_RESERVATION_V1_DOMAIN, &request);
        let v1_handle = format!(
            "{TIER_P_OPERATIONAL_RESERVATION_PREFIX}{}:{batch_digest}:{}:{request_digest}",
            request.ci_run_id, request.job_id
        );
        validate_handle("reserve", &v1_handle)?;
        prepared.push(PreparedOperationalReservation {
            request,
            v1_handle,
            v1_amount,
        });
    }
    Ok(prepared)
}

fn operational_batch_digest(validated: &[(CiJobRuntimeAuthorityRequest, i64)]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_OPERATIONAL_BATCH_V1_DOMAIN);
    hasher.update(&(validated.len() as u64).to_be_bytes());
    for (request, amount) in validated {
        hasher.update(
            runtime_authority_digest(CI_OPERATIONAL_RESERVATION_V1_DOMAIN, request).as_bytes(),
        );
        hasher.update(&amount.to_be_bytes());
    }
    hasher.finalize()
}

/// CT-007 slice 5b.3-4a.1b: the `v2` sibling of [`operational_batch_digest`] -- recomputes each
/// per-request `v2` digest internally from `(request, policy)` rather than accepting it as an input,
/// for the same reason [`operational_reservation_digest_v2`] does: no authority encoder accepts an
/// independently forgeable derived fact.
fn operational_batch_digest_v2(
    validated: &[(&CiJobRuntimeAuthorityRequest, i64)],
    policy: &CiAttemptBudgetPolicy,
) -> Result<blake3::Hash, CiLaunchAuthorityError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_OPERATIONAL_BATCH_V2_DOMAIN);
    hasher.update(&(validated.len() as u64).to_be_bytes());
    for (request, amount) in validated {
        hasher.update(operational_reservation_digest_v2(request, policy)?.as_bytes());
        hasher.update(&amount.to_be_bytes());
    }
    Ok(hasher.finalize())
}

/// The plain-text `a<N>` segment embedded in a durable `v2` handle for `max_parent_attempts` -- kept
/// alongside the revision tag so a reader can reconstruct the exact policy an already-durable row
/// was minted under (CT-007 slice 5b.3-4a.1b), never the provider's currently configured value.
fn attempts_handle_tag(max_parent_attempts: NonZeroU32) -> String {
    format!("a{}", max_parent_attempts.get())
}

/// The inverse of [`attempts_handle_tag`]. Returns `None` for anything that isn't an `a` followed by
/// a positive integer -- an unrecognized or zero value must refuse replay, never guess or clamp.
fn parse_attempts_handle_tag(tag: &str) -> Option<NonZeroU32> {
    let digits = tag.strip_prefix('a')?;
    let value: u32 = digits.parse().ok()?;
    NonZeroU32::new(value)
}

struct V2Candidate {
    handle: String,
    amount: i64,
}

/// The `v2` handle+amount every request in `requests` would durably mint under exactly `policy` --
/// factored out of [`prepare_operational_batch`] (CT-007 slice 5b.3-4a.1b) so replay validation can
/// call it a SECOND time with a policy reconstructed from an already-durable row's own embedded
/// descriptor, never the provider's currently configured policy, which may have since changed.
fn build_v2_candidates(
    requests: &[CiJobRuntimeAuthorityRequest],
    policy: &CiAttemptBudgetPolicy,
) -> Result<Vec<V2Candidate>, CiLaunchAuthorityError> {
    let amounts = requests
        .iter()
        .map(|request| operational_reservation_amount_v2(request, policy))
        .collect::<Result<Vec<i64>, _>>()?;
    let pairs = requests
        .iter()
        .zip(amounts.iter())
        .map(|(request, amount)| (request, *amount))
        .collect::<Vec<_>>();
    let batch_digest_v2 = operational_batch_digest_v2(&pairs, policy)?;
    let attempts_tag = attempts_handle_tag(policy.max_parent_attempts());
    let budget_tag = policy.revision().handle_tag();
    requests
        .iter()
        .zip(amounts)
        .map(|(request, amount)| {
            let request_digest_v2 = operational_reservation_digest_v2(request, policy)?;
            let handle = format!(
                "{TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX}{}:{budget_tag}:{attempts_tag}:{batch_digest_v2}:{}:{request_digest_v2}",
                request.ci_run_id, request.job_id
            );
            validate_handle("reserve", &handle)?;
            Ok(V2Candidate { handle, amount })
        })
        .collect()
}

/// The fields embedded in a durable `v2` handle after its prefix, parsed back apart. Used ONLY
/// during replay (CT-007 slice 5b.3-4a.1b) to recover which policy an already-durable row was
/// minted under -- never trusted on its own; the caller must still recompute the expected candidate
/// from these parsed fields and compare it byte-for-byte against the durable row.
struct ParsedV2Handle {
    ci_run_id: String,
    revision: CiAttemptBudgetRevision,
    max_parent_attempts: NonZeroU32,
    job_id: String,
}

/// Parses `<run_id>:<budget_tag>:<attempts_tag>:<batch_digest>:<job_id>:<request_digest>` (the
/// suffix after [`TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX`]). Returns `None` for any handle that
/// doesn't have exactly this shape or carries an unrecognized revision/attempts tag -- malformed or
/// forward-versioned handles must refuse replay, never be guessed at.
fn parse_v2_handle(handle: &str) -> Option<ParsedV2Handle> {
    let suffix = handle.strip_prefix(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)?;
    let mut parts = suffix.split(':');
    let ci_run_id = parts.next()?;
    let budget_tag = parts.next()?;
    let attempts_tag = parts.next()?;
    let _batch_digest = parts.next()?;
    let job_id = parts.next()?;
    let _request_digest = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let revision = CiAttemptBudgetRevision::from_handle_tag(budget_tag)?;
    let max_parent_attempts = parse_attempts_handle_tag(attempts_tag)?;
    Some(ParsedV2Handle {
        ci_run_id: ci_run_id.to_owned(),
        revision,
        max_parent_attempts,
        job_id: job_id.to_owned(),
    })
}

/// The durable reservation-writer marker carried by one `job_queue` dispatch.
///
/// This is derived from the reserve handle's exact wire shape rather than supplied as an unrelated
/// integer beside it. A legacy/V1 (or malformed/unknown) handle produces `NULL`; only an exactly
/// parseable V2 handle produces the protocol descriptor's marker `2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservationWriteVersionMarker(Option<i16>);

impl ReservationWriteVersionMarker {
    /// Derive the queue marker from the actual durable reserve handle.
    pub fn derive_from_reserve_handle(handle: &str) -> Self {
        if parse_v2_handle(handle).is_some() {
            Self(Some(
                crate::ci_pipeline_protocol::PRODUCTION_RESERVATION_QUEUE_MARKER,
            ))
        } else {
            Self::legacy()
        }
    }

    /// Historical/V1 dispatch marker (`NULL`).
    pub const fn legacy() -> Self {
        Self(None)
    }

    /// The SQL value persisted in `job_queue.reservation_write_version`.
    pub const fn value(self) -> Option<i16> {
        self.0
    }
}

/// Verify one durable v2 reservation against the complete immutable authority batch.
///
/// Parsing only recovers the policy descriptor; it grants no authority. This resolver reconstructs
/// that policy, rebuilds every candidate (thereby binding the batch digest), then requires the
/// current job's complete handle and durable amount to match byte-for-byte.
pub(crate) fn verify_v2_operational_reservation(
    requests: &[CiJobRuntimeAuthorityRequest],
    expected_ci_run_id: &str,
    expected_job_id: &str,
    durable_handle: &str,
    durable_amount: i64,
) -> Result<CiAttemptBudgetPolicy, CiLaunchAuthorityError> {
    let parsed = parse_v2_handle(durable_handle)
        .ok_or_else(|| refused("operational reservation is not a valid v2 handle"))?;
    if parsed.ci_run_id != expected_ci_run_id || parsed.job_id != expected_job_id {
        return Err(refused(
            "operational v2 reservation identity differs from the durable claim",
        ));
    }
    let policy = CiAttemptBudgetPolicy::new(parsed.revision, parsed.max_parent_attempts);
    let candidates = build_v2_candidates(requests, &policy)?;
    let candidate = requests
        .iter()
        .zip(candidates.iter())
        .find_map(|(request, candidate)| {
            (request.ci_run_id == expected_ci_run_id && request.job_id == expected_job_id)
                .then_some(candidate)
        })
        .ok_or_else(|| refused("claimed job is absent from the operational authority batch"))?;
    if candidate.handle != durable_handle || candidate.amount != durable_amount {
        return Err(refused(
            "durable operational v2 reservation authority diverged",
        ));
    }
    Ok(policy)
}

/// Reserve the maximum CPU-seconds plus memory-GiB-seconds the server-owned limits can consume.
/// These are operational capacity units, not a customer price. Terminal accounting uses the same
/// two measured dimensions and refunds the unused upper bound.
fn operational_reservation_amount(
    limits: &CiManifestLimitsV1,
) -> Result<i64, CiLaunchAuthorityError> {
    if limits.cpu_millis == 0
        || limits.mem_bytes == 0
        || limits.timeout_secs == 0
        || limits.pids_max == 0
        || limits.disk_bytes == 0
    {
        return Err(refused(
            "operational reservation limits must all be positive",
        ));
    }
    let timeout = u128::from(limits.timeout_secs);
    let cpu_millis_seconds = u128::from(limits.cpu_millis)
        .checked_mul(timeout)
        .ok_or_else(|| refused("operational CPU reservation overflow"))?;
    let memory_byte_seconds = u128::from(limits.mem_bytes)
        .checked_mul(timeout)
        .ok_or_else(|| refused("operational memory reservation overflow"))?;
    let cpu_seconds = cpu_millis_seconds.div_ceil(1_000);
    let memory_gib_seconds = memory_byte_seconds.div_ceil(u128::from(GIB_BYTES));
    let total = cpu_seconds
        .checked_add(memory_gib_seconds)
        .ok_or_else(|| refused("operational reservation overflow"))?;
    i64::try_from(total).map_err(|_| refused("operational reservation exceeds durable range"))
}

async fn reserve_operational_batch_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    ceiling: u32,
    budget_policy: &CiAttemptBudgetPolicy,
    write_version: OperationalReservationWriteVersion,
    prepared: &[PreparedOperationalReservation],
) -> Result<Result<Vec<String>, CiLaunchAuthorityError>, PgError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.operational-reservation.v1:{tenant_id}:{region}"
        ))
        .execute(&mut *conn)
        .await
        .map_err(pg_query)?;

    let v1_run_prefix = format!(
        "{TIER_P_OPERATIONAL_RESERVATION_PREFIX}{}:",
        prepared[0].request.ci_run_id
    );
    let v2_run_prefix = format!(
        "{TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX}{}:",
        prepared[0].request.ci_run_id
    );
    let rows = sqlx::query(
        "SELECT run_id, reserved FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 \
           AND (run_id LIKE ($3 || '%') OR run_id LIKE ($4 || '%'))",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(&v1_run_prefix)
    .bind(&v2_run_prefix)
    .fetch_all(&mut *conn)
    .await
    .map_err(pg_query)?;
    let rows = rows
        .into_iter()
        .map(|row| {
            let handle = row.try_get::<String, _>("run_id").map_err(pg_row)?;
            let amount = row.try_get::<i64, _>("reserved").map_err(pg_row)?;
            Ok::<_, PgError>((handle, amount))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(result) = resolve_durable_replay(rows, &v1_run_prefix, &v2_run_prefix, prepared) {
        return Ok(result);
    }

    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 \
           AND (run_id LIKE ($3 || '%') OR run_id LIKE ($4 || '%')) \
           AND state IN ('reserved', 'inflight')",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(TIER_P_OPERATIONAL_RESERVATION_PREFIX)
    .bind(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)
    .fetch_one(&mut *conn)
    .await
    .map_err(pg_query)?;
    let requested = i64::try_from(prepared.len())
        .map_err(|_| PgError::Query("operational reservation cardinality overflow".into()))?;
    if active
        .checked_add(requested)
        .is_none_or(|total| total > i64::from(ceiling))
    {
        return Ok(Err(refused(
            "operational reservation tenant ceiling is exhausted",
        )));
    }

    // Genuinely fresh (no durable rows exist for this run yet): mint whichever version this
    // provider is currently configured to WRITE. `v2` candidates are computed here for the first
    // time in this whole call -- CT-007 slice 5b.3-4a.1b's round-1 bug was eagerly computing them
    // for every batch regardless of need; this is the one place they're actually needed. Building
    // the complete candidate `Vec` before the insert loop starts means a `v2` overflow anywhere in
    // the batch refuses before any row is written -- never a partial insert.
    let fresh: Vec<(String, i64)> = match write_version {
        OperationalReservationWriteVersion::V1 => prepared
            .iter()
            .map(|item| (item.v1_handle.clone(), item.v1_amount))
            .collect(),
        OperationalReservationWriteVersion::V2 => {
            let requests: Vec<CiJobRuntimeAuthorityRequest> =
                prepared.iter().map(|item| item.request.clone()).collect();
            match build_v2_candidates(&requests, budget_policy) {
                Ok(candidates) => candidates
                    .into_iter()
                    .map(|candidate| (candidate.handle, candidate.amount))
                    .collect(),
                Err(error) => return Ok(Err(error)),
            }
        }
    };

    for (handle, amount) in &fresh {
        sqlx::query(
            "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state) \
             VALUES ($1, $2, $3, $4, 'reserved')",
        )
        .bind(tenant_id)
        .bind(region)
        .bind(handle)
        .bind(amount)
        .execute(&mut *conn)
        .await
        .map_err(pg_query)?;
    }
    Ok(Ok(fresh.into_iter().map(|(handle, _)| handle).collect()))
}

/// The pure replay/divergence decision over already-fetched durable rows (CT-007 slice 5b.3-4a.1b) --
/// factored out of [`reserve_operational_batch_on_conn`] so every branch (exact `v1` replay, exact
/// `v2` replay, mixed-version refusal, tampered/partial/divergent refusal) is directly unit-testable
/// without a database. Returns `None` when there is nothing durable for this run yet (the caller
/// should proceed to the active-ceiling check and a fresh `v1` insert); `Some(Ok(handles))` or
/// `Some(Err(_))` otherwise.
fn resolve_durable_replay(
    rows: Vec<(String, i64)>,
    v1_run_prefix: &str,
    v2_run_prefix: &str,
    prepared: &[PreparedOperationalReservation],
) -> Option<Result<Vec<String>, CiLaunchAuthorityError>> {
    if rows.is_empty() {
        return None;
    }
    // A durable run may hold `v1` OR `v2` rows (never both -- a batch is written entirely under
    // one version), so split by prefix before replaying either.
    let mut durable_v1 = BTreeMap::new();
    let mut durable_v2 = BTreeMap::new();
    for (handle, amount) in rows {
        let bucket = if handle.starts_with(v1_run_prefix) {
            &mut durable_v1
        } else if handle.starts_with(v2_run_prefix) {
            &mut durable_v2
        } else {
            return Some(Err(refused(
                "operational reservation run authority matched neither known prefix",
            )));
        };
        if bucket.insert(handle, amount).is_some() {
            return Some(Err(refused(
                "operational reservation run has duplicate durable authority",
            )));
        }
    }
    if !durable_v1.is_empty() && !durable_v2.is_empty() {
        return Some(Err(refused(
            "operational reservation run mixes v1 and v2 durable authority",
        )));
    }
    if !durable_v1.is_empty() {
        let expected = prepared
            .iter()
            .map(|item| (item.v1_handle.as_str(), item.v1_amount))
            .collect::<BTreeMap<_, _>>();
        let exact = durable_v1.len() == expected.len()
            && durable_v1
                .iter()
                .all(|(handle, amount)| expected.get(handle.as_str()) == Some(amount));
        if exact {
            return Some(Ok(prepared
                .iter()
                .map(|item| item.v1_handle.clone())
                .collect()));
        }
        return Some(Err(refused(
            "operational reservation run authority diverged from its durable batch",
        )));
    }
    Some(replay_v2_batch(prepared, &durable_v2))
}

/// Validates an exact acknowledgement-loss replay against durably-found `v2` rows (CT-007 slice
/// 5b.3-4a.1b). The durable rows may have been minted under an OLDER policy than the provider's
/// CURRENTLY configured one, so this recovers the actual policy from the rows' own embedded
/// descriptor instead -- a `v2` batch created under one attempt cap must still replay correctly
/// after the provider is reconfigured with another.
fn replay_v2_batch(
    prepared: &[PreparedOperationalReservation],
    durable_v2: &BTreeMap<String, i64>,
) -> Result<Vec<String>, CiLaunchAuthorityError> {
    let mut descriptor: Option<(CiAttemptBudgetRevision, NonZeroU32)> = None;
    for handle in durable_v2.keys() {
        let parsed = parse_v2_handle(handle)
            .ok_or_else(|| refused("operational reservation v2 durable handle is unparseable"))?;
        let this = (parsed.revision, parsed.max_parent_attempts);
        match descriptor {
            None => descriptor = Some(this),
            Some(previous) if previous == this => {}
            Some(_) => {
                return Err(refused(
                    "operational reservation v2 durable rows disagree on budget policy descriptor",
                ));
            }
        }
    }
    let (revision, max_parent_attempts) =
        descriptor.expect("durable_v2 is non-empty, checked by the caller");
    let reconstructed_policy = CiAttemptBudgetPolicy::new(revision, max_parent_attempts);

    let requests: Vec<CiJobRuntimeAuthorityRequest> =
        prepared.iter().map(|item| item.request.clone()).collect();
    let candidates = build_v2_candidates(&requests, &reconstructed_policy)?;
    let expected = candidates
        .iter()
        .map(|candidate| (candidate.handle.as_str(), candidate.amount))
        .collect::<BTreeMap<_, _>>();

    let exact = durable_v2.len() == expected.len()
        && durable_v2
            .iter()
            .all(|(handle, amount)| expected.get(handle.as_str()) == Some(amount));
    if !exact {
        return Err(refused(
            "operational reservation run authority diverged from its durable v2 batch",
        ));
    }
    Ok(candidates
        .into_iter()
        .map(|candidate| candidate.handle)
        .collect())
}

fn valid_machine_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn pg_query(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

fn pg_row(error: sqlx::Error) -> PgError {
    PgError::Query(error.to_string())
}

const CI_TOKEN_AUTHORITY_V1_HANDLE_PREFIX: &str = "ci-token-authority:v1:";
const CI_TOKEN_AUTHORITY_V2_HANDLE_PREFIX: &str = "ci-token-authority:v2:";

/// Content-addressed token-authority reference. The immutable manifest persists this handle, and a
/// later claim-bound issuer can reload the manifest and recompute it before minting. The hash binds
/// every locked identity, source, workflow, policy, and limit field; it contains no secret and grants
/// no authority by itself.
///
/// CT-007 slice 5b.3-2b: every NEWLY minted handle is `v2` (`token_authority_digest_v2`), which
/// additionally binds [`CiJobRuntimeAuthorityRequest::checkout`] — `v1` never hashed this field at
/// all (it did not exist), so a `v1` handle can never be reinterpreted as proof of checkout
/// authority. `v1` verification stays frozen (byte-identical to before this slice) and is accepted
/// ONLY for a request with `checkout: None`, purely so runs whose manifest already persisted a `v1`
/// handle before this slice landed keep resolving; any already-persisted `v1` handle on a
/// checkout-bearing job is refused outright and must be regenerated/requeued under `v2` — a `v1`
/// digest never bound a checkout target, so there is no way to "upgrade" one after the fact.
#[derive(Clone, Debug, Default)]
pub struct ManifestBoundCiJobTokenAuthority;

impl ManifestBoundCiJobTokenAuthority {
    pub fn handle_for(request: &CiJobRuntimeAuthorityRequest) -> String {
        format!(
            "{CI_TOKEN_AUTHORITY_V2_HANDLE_PREFIX}{}",
            token_authority_digest_v2(request)
        )
    }

    /// Recompute the public authority reference from server-resolved facts. This is not bearer
    /// verification; the claim-bound issuer uses it before asking Identity to mint a credential.
    /// Dispatches on the persisted handle's own version prefix rather than assuming `v2`, so a
    /// pre-existing `v1` manifest handle (minted before this slice) still verifies for a compute
    /// job.
    pub fn verifies(request: &CiJobRuntimeAuthorityRequest, handle: &str) -> bool {
        if let Some(digest_hex) = handle.strip_prefix(CI_TOKEN_AUTHORITY_V2_HANDLE_PREFIX) {
            return digest_hex == token_authority_digest_v2(request).to_string();
        }
        if let Some(digest_hex) = handle.strip_prefix(CI_TOKEN_AUTHORITY_V1_HANDLE_PREFIX) {
            if request.checkout.is_some() {
                return false;
            }
            return digest_hex == token_authority_digest(request).to_string();
        }
        false
    }
}

/// Server policy for the only V2 execution profile currently accepted. Resource limits, default
/// deny egress, batch scheduling, and fair-share identity are constants owned here—not plan fields.
#[derive(Clone)]
pub struct LinuxSmallV1LaunchAuthority {
    budget_reservations: Arc<dyn CiJobBudgetReservationProvider>,
}

impl LinuxSmallV1LaunchAuthority {
    pub fn new(budget_reservations: Arc<dyn CiJobBudgetReservationProvider>) -> Self {
        Self {
            budget_reservations,
        }
    }
}

impl CiLaunchAuthorityMaterializer for LinuxSmallV1LaunchAuthority {
    fn materialize<'a>(
        &'a self,
        record: &'a CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (limits, requests) = prepare_linux_small_requests(record, prepared, definition)?;
            let reserve_handles = self
                .budget_reservations
                .reserve_batch(requests.clone())
                .await?;
            finish_linux_small_authority(record, prepared, limits, &requests, reserve_handles)
        })
    }

    fn materialize_in_tx<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        record: &'a CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (limits, requests) = prepare_linux_small_requests(record, prepared, definition)?;
            let reserve_handles = self
                .budget_reservations
                .reserve_batch_in_tx(conn, requests.clone())
                .await?;
            finish_linux_small_authority(record, prepared, limits, &requests, reserve_handles)
        })
    }
}

fn prepare_linux_small_requests(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
    definition: &CiWorkflowDefinitionPin,
) -> Result<(CiManifestLimitsV1, Vec<CiJobRuntimeAuthorityRequest>), CiLaunchAuthorityError> {
    let run_id = validate_run_scope(record, prepared)?;
    launch_concurrency_group(record)?;
    if prepared.plan().execution.profile != CiExecutionProfileV1::LinuxSmallV1 {
        return Err(refused("unsupported CI execution profile"));
    }
    let limits = linux_small_limits();
    let checkout = checkout_scope_for_run(record)?;
    let mut requests = Vec::with_capacity(prepared.plan().jobs.len());
    for job in &prepared.plan().jobs {
        requests.push(CiJobRuntimeAuthorityRequest {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            ci_run_id: record.run_id.clone(),
            wf_run_id: record.wf_run_id.clone(),
            project_id: record.project_id.clone(),
            job_id: ci_job_id_v2(
                prepared.tenant(),
                run_id,
                &job.stage,
                &job.name,
                &job.matrix_identity(),
            )
            .to_string(),
            stage: job.stage.clone(),
            concrete_name: job.name.clone(),
            trigger_kind: record.trigger_kind.clone(),
            trust_tier: record.trust_tier.clone(),
            source_snapshot_digest: prepared.content_hash().to_multihash_string(),
            workflow_definition_version: definition.version(),
            workflow_code_hash: definition.code_hash().into(),
            policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
            limits: limits.clone(),
            checkout: checkout.clone(),
        });
    }
    Ok((limits, requests))
}

fn finish_linux_small_authority(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
    limits: CiManifestLimitsV1,
    requests: &[CiJobRuntimeAuthorityRequest],
    reserve_handles: Vec<String>,
) -> Result<CiLaunchAuthorityV1, CiLaunchAuthorityError> {
    if reserve_handles.len() != requests.len() {
        return Err(refused(
            "budget authority returned the wrong reservation cardinality",
        ));
    }
    let mut unique_reservations = BTreeSet::new();
    let concurrency_group = launch_concurrency_group(record)?;
    let mut grants = Vec::with_capacity(prepared.plan().jobs.len());
    for ((job, request), reserve_handle) in prepared
        .plan()
        .jobs
        .iter()
        .zip(requests.iter())
        .zip(reserve_handles)
    {
        validate_handle("reserve", &reserve_handle)?;
        if !unique_reservations.insert(reserve_handle.clone()) {
            return Err(refused(
                "runtime authority reused one reservation across jobs",
            ));
        }
        let token_authority_handle = ManifestBoundCiJobTokenAuthority::handle_for(request);
        validate_handle("token authority", &token_authority_handle)?;
        grants.push(CiJobLaunchGrantV1 {
            concrete_name: job.name.clone(),
            env: BTreeMap::new(),
            secret_handles: BTreeMap::new(),
            egress_allow: Vec::new(),
            limits: limits.clone(),
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: LINUX_SMALL_V1_RUNNER_LABELS
                    .iter()
                    .map(|label| (*label).to_owned())
                    .collect(),
                concurrency_group: concurrency_group.clone(),
                fair_key: format!("project:{}", record.project_id),
            },
            reserve_handle,
            token_authority_handle,
        });
    }
    Ok(CiLaunchAuthorityV1 {
        policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
        jobs: grants,
        merge_waiter: None,
    })
}

fn launch_concurrency_group(
    record: &CiRunRecord,
) -> Result<Option<String>, CiLaunchAuthorityError> {
    match (
        record.trigger_kind.as_str(),
        record.concurrency_group.as_deref(),
        record.pr_head_generation,
    ) {
        ("pull_request", Some(group), Some(generation))
            if crate::ci_run_store::valid_pr_concurrency_group(group) && generation > 0 =>
        {
            Ok(Some(group.to_owned()))
        }
        ("pull_request", Some(group), None)
            if crate::ci_run_store::valid_pr_concurrency_group(group) =>
        {
            // Rows written by the immediately preceding dispatcher during a rolling deploy do not
            // have this additive column. They remain runnable but are permanently legacy-oldest;
            // the run-supersession transaction must never let NULL cancel a positive generation.
            Ok(Some(group.to_owned()))
        }
        ("pull_request", _, _) => Err(refused(
            "pull-request run lacks canonical concurrency identity or producer generation",
        )),
        (_, None, None) => Ok(None),
        (_, _, _) => Err(refused(
            "non-pull-request run carries PR concurrency identity",
        )),
    }
}

const CI_TOKEN_AUTHORITY_V1_DOMAIN: &[u8] = b"myelin.ci.token-authority.v1\0";

pub(crate) fn token_authority_digest(request: &CiJobRuntimeAuthorityRequest) -> blake3::Hash {
    runtime_authority_digest(CI_TOKEN_AUTHORITY_V1_DOMAIN, request)
}

fn runtime_authority_digest(domain: &[u8], request: &CiJobRuntimeAuthorityRequest) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    for value in [
        request.tenant_id.as_str(),
        request.region.as_str(),
        request.ci_run_id.as_str(),
        request.wf_run_id.as_str(),
        request.project_id.as_str(),
        request.job_id.as_str(),
        request.stage.as_str(),
        request.concrete_name.as_str(),
        request.trigger_kind.as_str(),
        request.trust_tier.as_str(),
        request.source_snapshot_digest.as_str(),
        request.workflow_code_hash.as_str(),
        request.policy_revision.as_str(),
    ] {
        hash_length_prefixed(&mut hasher, value.as_bytes());
    }
    hasher.update(&request.workflow_definition_version.to_be_bytes());
    hasher.update(&request.limits.cpu_millis.to_be_bytes());
    hasher.update(&request.limits.mem_bytes.to_be_bytes());
    hasher.update(&request.limits.disk_bytes.to_be_bytes());
    hasher.update(&request.limits.pids_max.to_be_bytes());
    hasher.update(&request.limits.timeout_secs.to_be_bytes());
    hasher.finalize()
}

fn hash_length_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

const CI_TOKEN_AUTHORITY_V2_DOMAIN: &[u8] = b"myelin.ci.token-authority.v2\0";

/// CT-007 slice 5b.3-2b: the `v2` token-authority digest. A wholly SEPARATE encoder from
/// [`token_authority_digest`] (Sol's review) — never a conditional variant of the shared
/// [`runtime_authority_digest`] helper, which stays byte-frozen because it is ALSO used by the two
/// budget-reservation domains ([`CI_OPERATIONAL_RESERVATION_V1_DOMAIN`],
/// [`CI_OPERATIONAL_BATCH_V1_DOMAIN`]), which have no reason to know about checkout scope at all.
/// Hashes every field the `v1` digest hashes, under a DIFFERENT domain separator (so a `v2` digest
/// can never collide with a `v1` one even for a request with `checkout: None`), plus an explicit
/// present/absent discriminator byte and — only when `Some` — the checkout scope's own canonical
/// fields (tenant, repo ref, repo id, exact commit hex, object format).
fn token_authority_digest_v2(request: &CiJobRuntimeAuthorityRequest) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_TOKEN_AUTHORITY_V2_DOMAIN);
    for value in [
        request.tenant_id.as_str(),
        request.region.as_str(),
        request.ci_run_id.as_str(),
        request.wf_run_id.as_str(),
        request.project_id.as_str(),
        request.job_id.as_str(),
        request.stage.as_str(),
        request.concrete_name.as_str(),
        request.trigger_kind.as_str(),
        request.trust_tier.as_str(),
        request.source_snapshot_digest.as_str(),
        request.workflow_code_hash.as_str(),
        request.policy_revision.as_str(),
    ] {
        hash_length_prefixed(&mut hasher, value.as_bytes());
    }
    hasher.update(&request.workflow_definition_version.to_be_bytes());
    hasher.update(&request.limits.cpu_millis.to_be_bytes());
    hasher.update(&request.limits.mem_bytes.to_be_bytes());
    hasher.update(&request.limits.disk_bytes.to_be_bytes());
    hasher.update(&request.limits.pids_max.to_be_bytes());
    hasher.update(&request.limits.timeout_secs.to_be_bytes());
    match &request.checkout {
        None => {
            hasher.update(&[0u8]);
        }
        Some(scope) => {
            hasher.update(&[1u8]);
            hash_length_prefixed(&mut hasher, scope.tenant().0.as_bytes());
            hash_length_prefixed(&mut hasher, scope.repo_ref().0.as_bytes());
            hash_length_prefixed(&mut hasher, scope.repo_id().as_bytes());
            hash_length_prefixed(&mut hasher, scope.commit_hex().as_bytes());
            let format_tag: u8 = match scope.commit_format() {
                myelin_ci_sandbox::GitObjectFormat::Sha1 => 1,
                myelin_ci_sandbox::GitObjectFormat::Sha256 => 2,
            };
            hasher.update(&[format_tag]);
        }
    }
    hasher.finalize()
}

fn linux_small_limits() -> CiManifestLimitsV1 {
    CiManifestLimitsV1 {
        cpu_millis: 1_000,
        mem_bytes: 256 * 1024 * 1024,
        disk_bytes: 1024 * 1024 * 1024,
        pids_max: 128,
        timeout_secs: 600,
    }
}

/// CT-007 slice 5b.3-2b: derive the checkout scope every job in `record`'s run shares (a `ci_run`'s
/// `repo_ref`/`commit_oid` are per-RUN, not per-job — every `GrantedCiJobV1.workspace` this run's
/// manifest ever builds copies the SAME pair, see `pg_pipeline_starter.rs`'s manifest-building code).
/// Goes through [`derive_checkout_authorization_scope`] ONLY — never hand-parses `repo_ref`/
/// `commit_oid` itself — so this authority-side derivation can never silently diverge from what
/// `ci_manifest_job_runner.rs` does when it builds the real launched `JobSpec.workspace` from the
/// identical two strings.
fn checkout_scope_for_run(
    record: &CiRunRecord,
) -> Result<Option<CheckoutAuthorizationScope>, CiLaunchAuthorityError> {
    let workspace = WorkspaceSpec {
        repo_ref: record.repo_ref.clone(),
        commit: record.commit_oid.clone(),
    };
    derive_checkout_authorization_scope(JobKind::Ci, &workspace)
        .map_err(|detail| refused(&format!("ci_run checkout target is invalid: {detail}")))
}

fn validate_run_scope(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
) -> Result<sqlx::types::Uuid, CiLaunchAuthorityError> {
    if record.tenant_id != prepared.tenant().0 {
        return Err(refused(
            "prepared plan tenant differs from the locked ci_run",
        ));
    }
    if record.state != "queued" {
        return Err(refused("launch authority requires a queued ci_run"));
    }
    if record.region.trim().is_empty() {
        return Err(refused("ci_run region is empty"));
    }
    let run_id = parse_uuid("run_id", &record.run_id)?;
    parse_uuid("project_id", &record.project_id)?;
    parse_uuid("wf_run_id", &record.wf_run_id)?;
    if !matches!(
        record.trigger_kind.as_str(),
        "push" | "pull_request" | "issue_transition" | "manual" | "agent" | "schedule"
    ) {
        return Err(refused(
            "ci_run trigger kind is outside the frozen vocabulary",
        ));
    }
    if !matches!(
        record.trust_tier.as_str(),
        "trusted" | "untrusted_fork" | "self_hosted"
    ) {
        return Err(refused(
            "ci_run trust tier is outside the frozen vocabulary",
        ));
    }
    Ok(run_id)
}

fn parse_uuid(field: &str, value: &str) -> Result<sqlx::types::Uuid, CiLaunchAuthorityError> {
    sqlx::types::Uuid::parse_str(value)
        .map_err(|_| refused(&format!("ci_run.{field} is not a UUID")))
}

fn validate_handle(kind: &str, value: &str) -> Result<(), CiLaunchAuthorityError> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(refused(&format!(
            "runtime authority returned an invalid {kind} handle"
        )));
    }
    Ok(())
}

fn refused(detail: &str) -> CiLaunchAuthorityError {
    CiLaunchAuthorityError(detail.into())
}

// =================================================================================================
// CT-007 slice 5b.3-4a.1a — the `v2` operational-reservation budget authority (design locked with
// Sol, 2026-07-29): pure calculator + digest-encoder machinery, NOT wired into any live reservation
// path yet (5b.3-4a.1b activates it at `PgTierPCiJobBudgetReservation` materialization). `v1` stays
// completely untouched — every function/constant above this section is byte-frozen.
//
// The core fact this machinery prices: a checkout-bearing parent attempt sequentially runs Hop A's
// two nested git-wire executions (advertise-refs, fetch) plus Hop B's checkout-materialization run,
// BEFORE the workload's own execution — four sequential executions total, each independently
// enforcing the SAME `spec.limits` ceiling (Sol's review: peak resources stay one container's
// limits since the four are sequential, never concurrent; metered resource-TIME is additive across
// them). The existing `v1` single-workload reservation has no concept of this and must not be
// reinterpreted to cover it — `v2` adds a separate, ADDITIVE parent-attempt budget instead of
// dividing the existing per-container ceiling four ways (which would silently weaken every
// individual execution's own timeout/resource allowance).
// =================================================================================================

/// Named execution counts within one checkout-bearing parent attempt — never a bare `4` scattered
/// through the ceiling math. Hop A's two nested git-wire executions (advertise-refs, fetch) plus Hop
/// B's one checkout-materialization run, additive with the workload's own one execution.
const CHECKOUT_TRANSPORT_EXECUTIONS: u64 = 2;
const CHECKOUT_MATERIALIZATION_EXECUTIONS: u64 = 1;
const WORKLOAD_EXECUTIONS: u64 = 1;

const CI_OPERATIONAL_RESERVATION_V2_DOMAIN: &[u8] = b"myelin.ci.operational-reservation.v2\0";
/// CT-007 slice 5b.3-4a.1b: the `v2` sibling of [`CI_OPERATIONAL_BATCH_V1_DOMAIN`], used by
/// [`operational_batch_digest_v2`] so a `v2` batch's aggregate digest can never collide with a `v1`
/// batch's, even over an identical set of per-request digests.
const CI_OPERATIONAL_BATCH_V2_DOMAIN: &[u8] = b"myelin.ci.operational-reservation-batch.v2\0";

/// The revision of the attempt-budget CALCULATION topology/algorithm a `v2` reservation was priced
/// under — distinct from [`CiAttemptBudgetPolicy::max_parent_attempts`] (a policy VALUE), this
/// identifies HOW ceilings combine (how many executions per phase, how they're aggregated). Hashed
/// separately into the `v2` digest so a future topology change (e.g. Hop B splitting into two
/// executions) can never silently collide with today's `V1` revision's handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiAttemptBudgetRevision {
    V1,
}

impl CiAttemptBudgetRevision {
    pub(crate) fn tag(self) -> u8 {
        match self {
            CiAttemptBudgetRevision::V1 => 1,
        }
    }

    /// The plain-text segment embedded in a durable `v2` handle (CT-007 slice 5b.3-4a.1b) so a
    /// reader can recover which topology revision priced an already-durable row without trusting
    /// the provider's CURRENTLY configured revision -- an old row must replay under the revision it
    /// was actually written with, even if production has since moved on.
    fn handle_tag(self) -> &'static str {
        match self {
            CiAttemptBudgetRevision::V1 => "budget-v1",
        }
    }

    /// The inverse of [`Self::handle_tag`]. Returns `None` for anything not durably minted by this
    /// binary's known revisions -- an unrecognized tag must refuse replay, never guess.
    fn from_handle_tag(tag: &str) -> Option<Self> {
        match tag {
            "budget-v1" => Some(CiAttemptBudgetRevision::V1),
            _ => None,
        }
    }
}

/// The durable policy governing how many times one job may be attempted before its `v2`
/// reservation's upper bound is exhausted. Sol's round-2 review: a "parent attempt" is counted from
/// a durably-begun claim generation — before Hop A for a checkout-bearing job, or before workload
/// launch itself for a compute job (which has no Hop A/B preparation at all, yet still receives the
/// SAME 5-attempt `v2` ceiling — this cap counts attempts at the job, not at checkout preparation
/// specifically). CT-007 slice 5b.3-4a.2 must enforce this cap for BOTH shapes.
///
/// `max_parent_attempts` is a deliberate, REVISABLE policy value (CT-007 slice 5b.3-4, default `5`
/// — chosen 2026-07-29 as a cost/reliability trade-off, not derived from any technical constraint;
/// mirrors the same house convention as [`TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING`]). A
/// provider must STORE this explicitly (never re-derive it ad hoc) so every request it prices agrees
/// on the same policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiAttemptBudgetPolicy {
    revision: CiAttemptBudgetRevision,
    max_parent_attempts: NonZeroU32,
}

impl CiAttemptBudgetPolicy {
    /// The production default: revision `V1`, 5 attempts.
    pub fn production() -> Self {
        Self {
            revision: CiAttemptBudgetRevision::V1,
            max_parent_attempts: NonZeroU32::new(5).expect("5 is nonzero"),
        }
    }

    /// A checked constructor for tests and future non-default compositions.
    pub fn new(revision: CiAttemptBudgetRevision, max_parent_attempts: NonZeroU32) -> Self {
        Self {
            revision,
            max_parent_attempts,
        }
    }

    pub fn revision(&self) -> CiAttemptBudgetRevision {
        self.revision
    }

    pub fn max_parent_attempts(&self) -> NonZeroU32 {
        self.max_parent_attempts
    }
}

/// CT-007 slice 5b.3-4a.1c: which reservation version a provider mints for a genuinely FRESH batch
/// (one with no durable rows for its run yet). This is orthogonal to [`CiAttemptBudgetPolicy`] --
/// that describes the `v2` AMOUNT topology; this describes whether fresh writes use it at all.
///
/// Sol's review: a committed 4a.1c binary is safe to write `v2` ONLY once every reader in the fleet
/// already understands `v2` (i.e. every binary has 5b.3-4a.1b's compatibility reads). A preceding
/// commit alone does not guarantee that during a rolling deployment -- an old binary mid-rollout
/// still cannot discover or count `v2` rows. Production MUST stay on `V1` until fleet convergence is
/// separately confirmed and this is deliberately flipped; existing durable rows always replay under
/// whichever version they were actually written with regardless of this setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalReservationWriteVersion {
    V1,
    V2,
}

/// Raw, unpriced resource-seconds — CPU-seconds and memory-byte-seconds kept SEPARATE (Sol's
/// review: "aggregate raw dimensions first, price once"). Distinct from `v1`'s
/// [`operational_reservation_amount`], which combines cpu-seconds + memory-GiB-seconds into one
/// already-converted `i64` at the very first step — `v2`'s math instead stays in raw dimensions
/// through every intermediate aggregation, converting to operational units exactly once, at the end
/// ([`operational_reservation_amount_v2`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceCeiling {
    cpu_seconds: u64,
    mem_byte_seconds: u64,
}

impl ResourceCeiling {
    fn checked_mul(self, factor: u64) -> Result<ResourceCeiling, CiLaunchAuthorityError> {
        Ok(ResourceCeiling {
            cpu_seconds: self
                .cpu_seconds
                .checked_mul(factor)
                .ok_or_else(|| refused("operational v2 cpu-seconds ceiling overflow"))?,
            mem_byte_seconds: self
                .mem_byte_seconds
                .checked_mul(factor)
                .ok_or_else(|| refused("operational v2 mem-byte-seconds ceiling overflow"))?,
        })
    }
}

/// The raw resource-seconds ceiling for ONE execution at `limits` — checked arithmetic, kept in raw
/// dimensions (never combined/converted here). Mirrors `v1`'s [`operational_reservation_amount`]
/// CPU/memory formulas exactly (same inputs, same validation), but returns the two dimensions
/// separately instead of one pre-converted `i64`.
fn raw_execution_ceiling(
    limits: &CiManifestLimitsV1,
) -> Result<ResourceCeiling, CiLaunchAuthorityError> {
    if limits.cpu_millis == 0
        || limits.mem_bytes == 0
        || limits.timeout_secs == 0
        || limits.pids_max == 0
        || limits.disk_bytes == 0
    {
        return Err(refused(
            "operational v2 reservation limits must all be positive",
        ));
    }
    let timeout = u128::from(limits.timeout_secs);
    let cpu_millis_seconds = u128::from(limits.cpu_millis)
        .checked_mul(timeout)
        .ok_or_else(|| refused("operational v2 CPU reservation overflow"))?;
    let mem_byte_seconds = u128::from(limits.mem_bytes)
        .checked_mul(timeout)
        .ok_or_else(|| refused("operational v2 memory reservation overflow"))?;
    let cpu_seconds = cpu_millis_seconds.div_ceil(1_000);
    Ok(ResourceCeiling {
        cpu_seconds: u64::try_from(cpu_seconds)
            .map_err(|_| refused("operational v2 cpu-seconds ceiling exceeds durable range"))?,
        mem_byte_seconds: u64::try_from(mem_byte_seconds).map_err(|_| {
            refused("operational v2 mem-byte-seconds ceiling exceeds durable range")
        })?,
    })
}

/// Raw ceiling for one sandbox execution, exposed only to the prelaunch journal so it can derive
/// phase ceilings from the same checked arithmetic used by the v2 reservation.
pub(crate) fn raw_execution_usage_ceiling(
    request: &CiJobRuntimeAuthorityRequest,
) -> Result<ResourceUsage, CiLaunchAuthorityError> {
    let ceiling = raw_execution_ceiling(&request.limits)?;
    Ok(ResourceUsage {
        cpu_seconds: ceiling.cpu_seconds,
        mem_byte_seconds: ceiling.mem_byte_seconds,
    })
}

/// One parent attempt's total raw ceiling: the workload alone for a compute job, or Hop A (2
/// executions) + Hop B (1) + the workload (1) for a checkout-bearing job. Checkout presence is
/// derived ONLY from `request.checkout` (Sol's review: never a caller-supplied bool, which could
/// disagree with the authority request and under-reserve it).
fn parent_attempt_ceiling(
    request: &CiJobRuntimeAuthorityRequest,
) -> Result<ResourceCeiling, CiLaunchAuthorityError> {
    let one_execution = raw_execution_ceiling(&request.limits)?;
    let executions = if request.checkout.is_some() {
        CHECKOUT_TRANSPORT_EXECUTIONS + CHECKOUT_MATERIALIZATION_EXECUTIONS + WORKLOAD_EXECUTIONS
    } else {
        WORKLOAD_EXECUTIONS
    };
    one_execution.checked_mul(executions)
}

/// The complete raw ceiling a job may durably accrue across every parent attempt its budget policy
/// allows — [`parent_attempt_ceiling`] times `policy.max_parent_attempts`, checked.
fn job_lifetime_ceiling(
    request: &CiJobRuntimeAuthorityRequest,
    policy: &CiAttemptBudgetPolicy,
) -> Result<ResourceCeiling, CiLaunchAuthorityError> {
    let per_attempt = parent_attempt_ceiling(request)?;
    per_attempt.checked_mul(u64::from(policy.max_parent_attempts.get()))
}

/// The final conversion from a raw [`ResourceCeiling`] to operational units — `cpu_seconds +
/// ceil(mem_byte_seconds / GiB)`, checked. Mirrors [`TierPOperationalCiJobPricer`]'s own conversion
/// formula exactly, so a job's `v2` reservation and its eventual settlement price always agree on
/// units. Extracted as its own function (Sol's round-2 review) so both overflow boundaries — the
/// `checked_add` and the `i64` range check — are directly testable without needing to construct
/// realistic-looking `CiManifestLimitsV1`/policy inputs that happen to trigger each one.
fn operational_amount_from_ceiling(
    ceiling: ResourceCeiling,
) -> Result<i64, CiLaunchAuthorityError> {
    let memory_gib_seconds = ceiling.mem_byte_seconds.div_ceil(GIB_BYTES);
    let total = ceiling
        .cpu_seconds
        .checked_add(memory_gib_seconds)
        .ok_or_else(|| refused("operational v2 reservation overflow"))?;
    i64::try_from(total).map_err(|_| refused("operational v2 reservation exceeds durable range"))
}

/// `v2`'s final conversion to operational units — aggregate raw dimensions first
/// ([`job_lifetime_ceiling`]), convert exactly once via [`operational_amount_from_ceiling`].
fn operational_reservation_amount_v2(
    request: &CiJobRuntimeAuthorityRequest,
    policy: &CiAttemptBudgetPolicy,
) -> Result<i64, CiLaunchAuthorityError> {
    let ceiling = job_lifetime_ceiling(request, policy)?;
    operational_amount_from_ceiling(ceiling)
}

/// The `v2` operational-reservation digest — a wholly SEPARATE encoder from
/// [`runtime_authority_digest`] (which stays byte-frozen; it is ALSO shared by the `v1` batch
/// domain, which has no reason to know about checkout scope, budget policy, or ceilings at all).
/// Mirrors [`token_authority_digest_v2`]'s checkout-hashing pattern exactly (same present/absent
/// discriminator + canonical fields), and additionally binds the budget-policy revision,
/// `max_parent_attempts`, and the resulting per-attempt/lifetime raw ceilings — so two jobs with
/// identical limits and checkout-presence, but a different budget policy (or a future topology
/// revision), can NEVER collide on the same `v2` handle, even if their reservation AMOUNT happens to
/// coincide. Sol's round-2 review: derives `per_attempt`/`lifetime` INTERNALLY from `request`/
/// `policy` rather than accepting them as caller-supplied parameters — no authority encoder may
/// accept independently forgeable derived facts; a caller could otherwise pass ceilings that
/// disagree with the request/policy they claim to describe.
fn operational_reservation_digest_v2(
    request: &CiJobRuntimeAuthorityRequest,
    policy: &CiAttemptBudgetPolicy,
) -> Result<blake3::Hash, CiLaunchAuthorityError> {
    let per_attempt = parent_attempt_ceiling(request)?;
    let lifetime = job_lifetime_ceiling(request, policy)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_OPERATIONAL_RESERVATION_V2_DOMAIN);
    for value in [
        request.tenant_id.as_str(),
        request.region.as_str(),
        request.ci_run_id.as_str(),
        request.wf_run_id.as_str(),
        request.project_id.as_str(),
        request.job_id.as_str(),
        request.stage.as_str(),
        request.concrete_name.as_str(),
        request.trigger_kind.as_str(),
        request.trust_tier.as_str(),
        request.source_snapshot_digest.as_str(),
        request.workflow_code_hash.as_str(),
        request.policy_revision.as_str(),
    ] {
        hash_length_prefixed(&mut hasher, value.as_bytes());
    }
    hasher.update(&request.workflow_definition_version.to_be_bytes());
    hasher.update(&request.limits.cpu_millis.to_be_bytes());
    hasher.update(&request.limits.mem_bytes.to_be_bytes());
    hasher.update(&request.limits.disk_bytes.to_be_bytes());
    hasher.update(&request.limits.pids_max.to_be_bytes());
    hasher.update(&request.limits.timeout_secs.to_be_bytes());
    match &request.checkout {
        None => {
            hasher.update(&[0u8]);
        }
        Some(scope) => {
            hasher.update(&[1u8]);
            hash_length_prefixed(&mut hasher, scope.tenant().0.as_bytes());
            hash_length_prefixed(&mut hasher, scope.repo_ref().0.as_bytes());
            hash_length_prefixed(&mut hasher, scope.repo_id().as_bytes());
            hash_length_prefixed(&mut hasher, scope.commit_hex().as_bytes());
            let format_tag: u8 = match scope.commit_format() {
                myelin_ci_sandbox::GitObjectFormat::Sha1 => 1,
                myelin_ci_sandbox::GitObjectFormat::Sha256 => 2,
            };
            hasher.update(&[format_tag]);
        }
    }
    hasher.update(&[policy.revision.tag()]);
    hasher.update(&policy.max_parent_attempts.get().to_be_bytes());
    hasher.update(&per_attempt.cpu_seconds.to_be_bytes());
    hasher.update(&per_attempt.mem_byte_seconds.to_be_bytes());
    hasher.update(&lifetime.cpu_seconds.to_be_bytes());
    hasher.update(&lifetime.mem_byte_seconds.to_be_bytes());
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load_launch_run_plan_v2, CiExecutionRequestV1, ResolvedJobV2, ResolvedRunPlanV2,
        EXECUTION_REQUEST_SCHEMA_V1, RUN_PLAN_SCHEMA_V2,
    };
    use myelin_storage::{BlobStore, FsBlobStore};
    use myelin_tenancy::TenantId;
    use std::sync::Mutex;

    const PINNED_IMAGE: &str =
        "registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[derive(Default)]
    struct RecordingBudget {
        batches: Mutex<Vec<Vec<CiJobRuntimeAuthorityRequest>>>,
        duplicate_reservation: bool,
        bad_cardinality: bool,
        refusal: Option<String>,
    }

    impl CiJobBudgetReservationProvider for RecordingBudget {
        fn reserve_batch<'a>(
            &'a self,
            requests: Vec<CiJobRuntimeAuthorityRequest>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>
        {
            self.batches.lock().unwrap().push(requests.clone());
            if let Some(detail) = self.refusal.clone() {
                return Box::pin(async move { Err(refused(&detail)) });
            }
            let mut handles = requests
                .iter()
                .map(|request| {
                    if self.duplicate_reservation {
                        "reserve:duplicate".into()
                    } else {
                        format!("reserve:{}", request.job_id)
                    }
                })
                .collect::<Vec<_>>();
            if self.bad_cardinality {
                handles.pop();
            }
            Box::pin(async move { Ok(handles) })
        }
    }

    fn policy(budget: Arc<dyn CiJobBudgetReservationProvider>) -> LinuxSmallV1LaunchAuthority {
        LinuxSmallV1LaunchAuthority::new(budget)
    }

    // =============================================================================================
    // CT-007 slice 5b.3-4a.1a — the `v2` operational-reservation budget authority. Pure
    // calculator/digest-encoder tests only; nothing here is wired into any live reservation path yet
    // (5b.3-4a.1b).
    // =============================================================================================
    mod v2_budget_authority_5b3_4a1a {
        use super::*;

        fn v2_request(
            checkout: Option<CheckoutAuthorizationScope>,
        ) -> CiJobRuntimeAuthorityRequest {
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
                source_snapshot_digest: "digest-fixture".into(),
                workflow_definition_version: 3,
                workflow_code_hash: "code-hash-fixture".into(),
                policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
                limits: linux_small_limits(),
                checkout,
            }
        }

        fn checkout_scope_fixture() -> CheckoutAuthorizationScope {
            myelin_ci_sandbox::derive_checkout_authorization_scope(
                myelin_ci_sandbox::JobKind::Ci,
                &myelin_ci_sandbox::WorkspaceSpec {
                    repo_ref: Some("myelin://acme/git/repo/widgets".into()),
                    commit: Some("a".repeat(40)),
                },
            )
            .unwrap()
            .unwrap()
        }

        #[test]
        fn v1_amount_is_completely_unchanged_by_this_slice() {
            assert_eq!(
                operational_reservation_amount(&linux_small_limits()).unwrap(),
                750
            );
        }

        #[test]
        fn golden_v1_digest_for_a_fixed_request_is_pinned() {
            // Frozen hard-coded v1 handle-digest vector (Sol's round-4a.1a review): proves this
            // slice's additions never touch `runtime_authority_digest`/`CI_OPERATIONAL_RESERVATION_V1_DOMAIN`
            // byte-for-byte, computed once and pasted as a literal (never compared via the same
            // function under test).
            let request = v2_request(None);
            let digest = runtime_authority_digest(CI_OPERATIONAL_RESERVATION_V1_DOMAIN, &request);
            assert_eq!(
                digest.to_hex().as_str(),
                "e7342bddca7b3b20491a47906abc128515486354aaab516bd68837be536a9592"
            );
        }

        #[test]
        fn raw_execution_ceiling_matches_v1s_own_formula_in_raw_dimensions() {
            let ceiling = raw_execution_ceiling(&linux_small_limits()).unwrap();
            assert_eq!(ceiling.cpu_seconds, 600);
            assert_eq!(ceiling.mem_byte_seconds, 161_061_273_600);
        }

        #[test]
        fn compute_job_parent_attempt_ceiling_is_exactly_one_execution() {
            let request = v2_request(None);
            let ceiling = parent_attempt_ceiling(&request).unwrap();
            let one = raw_execution_ceiling(&linux_small_limits()).unwrap();
            assert_eq!(ceiling, one);
        }

        #[test]
        fn checkout_job_parent_attempt_ceiling_is_exactly_four_executions() {
            let request = v2_request(Some(checkout_scope_fixture()));
            let ceiling = parent_attempt_ceiling(&request).unwrap();
            let one = raw_execution_ceiling(&linux_small_limits()).unwrap();
            assert_eq!(ceiling.cpu_seconds, one.cpu_seconds * 4);
            assert_eq!(ceiling.mem_byte_seconds, one.mem_byte_seconds * 4);
        }

        #[test]
        fn compute_job_lifetime_ceiling_and_amount_golden() {
            let policy = CiAttemptBudgetPolicy::production();
            let request = v2_request(None);
            let lifetime = job_lifetime_ceiling(&request, &policy).unwrap();
            assert_eq!(lifetime.cpu_seconds, 3_000);
            assert_eq!(lifetime.mem_byte_seconds, 805_306_368_000);
            assert_eq!(
                operational_reservation_amount_v2(&request, &policy).unwrap(),
                3_750
            );
        }

        #[test]
        fn checkout_job_lifetime_ceiling_and_amount_golden() {
            let policy = CiAttemptBudgetPolicy::production();
            let request = v2_request(Some(checkout_scope_fixture()));
            let lifetime = job_lifetime_ceiling(&request, &policy).unwrap();
            assert_eq!(lifetime.cpu_seconds, 12_000);
            assert_eq!(lifetime.mem_byte_seconds, 3_221_225_472_000);
            assert_eq!(
                operational_reservation_amount_v2(&request, &policy).unwrap(),
                15_000
            );
        }

        #[test]
        fn golden_v2_compute_digest_is_pinned() {
            let policy = CiAttemptBudgetPolicy::production();
            let request = v2_request(None);
            let digest = operational_reservation_digest_v2(&request, &policy).unwrap();
            assert_eq!(
                digest.to_hex().as_str(),
                "72d454ec72766e876206495631c7f01311e8e967461e0ce31b2a077d4c09e1ac"
            );
        }

        #[test]
        fn golden_v2_checkout_digest_is_pinned() {
            let policy = CiAttemptBudgetPolicy::production();
            let request = v2_request(Some(checkout_scope_fixture()));
            let digest = operational_reservation_digest_v2(&request, &policy).unwrap();
            assert_eq!(
                digest.to_hex().as_str(),
                "8f75d7cdca35c8646e98f865ae548283fde626ef11ebde6df1247fe8cf58fc3b"
            );
        }

        #[test]
        fn different_max_parent_attempts_yields_different_amount_and_handle() {
            let request = v2_request(None);
            let policy_5 = CiAttemptBudgetPolicy::production();
            let policy_3 = CiAttemptBudgetPolicy::new(
                CiAttemptBudgetRevision::V1,
                NonZeroU32::new(3).unwrap(),
            );
            let amount_5 = operational_reservation_amount_v2(&request, &policy_5).unwrap();
            let amount_3 = operational_reservation_amount_v2(&request, &policy_3).unwrap();
            assert_ne!(amount_5, amount_3);

            let digest_5 = operational_reservation_digest_v2(&request, &policy_5).unwrap();
            let digest_3 = operational_reservation_digest_v2(&request, &policy_3).unwrap();
            assert_ne!(digest_5, digest_3);
        }

        #[test]
        fn different_checkout_scope_same_limits_same_amount_different_handle() {
            let request_a = v2_request(Some(checkout_scope_fixture()));
            let other_scope = myelin_ci_sandbox::derive_checkout_authorization_scope(
                myelin_ci_sandbox::JobKind::Ci,
                &myelin_ci_sandbox::WorkspaceSpec {
                    repo_ref: Some("myelin://acme/git/repo/other".into()),
                    commit: Some("b".repeat(40)),
                },
            )
            .unwrap()
            .unwrap();
            let request_b = v2_request(Some(other_scope));

            let policy = CiAttemptBudgetPolicy::production();
            let amount_a = operational_reservation_amount_v2(&request_a, &policy).unwrap();
            let amount_b = operational_reservation_amount_v2(&request_b, &policy).unwrap();
            assert_eq!(
                amount_a, amount_b,
                "same limits, same execution count -> same amount"
            );

            let digest_a = operational_reservation_digest_v2(&request_a, &policy).unwrap();
            let digest_b = operational_reservation_digest_v2(&request_b, &policy).unwrap();
            assert_ne!(
                digest_a, digest_b,
                "different checkout scope must still produce a different handle"
            );
        }

        #[test]
        fn zero_attempts_cannot_be_constructed() {
            assert!(NonZeroU32::new(0).is_none());
        }

        #[test]
        fn resource_ceiling_checked_mul_overflow_refuses_loudly() {
            let ceiling = ResourceCeiling {
                cpu_seconds: u64::MAX,
                mem_byte_seconds: 1,
            };
            assert!(ceiling.checked_mul(2).is_err());

            let ceiling = ResourceCeiling {
                cpu_seconds: 1,
                mem_byte_seconds: u64::MAX,
            };
            assert!(ceiling.checked_mul(2).is_err());
        }

        #[test]
        fn raw_execution_ceiling_mem_byte_seconds_overflow_refuses_loudly() {
            let mut limits = linux_small_limits();
            limits.mem_bytes = u64::MAX;
            limits.timeout_secs = 2;
            let err = raw_execution_ceiling(&limits).unwrap_err();
            assert!(
                err.0.contains("overflow") || err.0.contains("exceeds"),
                "message was: {}",
                err.0
            );
        }

        #[test]
        fn job_lifetime_ceiling_overflow_refuses_loudly() {
            // A large-but-individually-valid raw ceiling whose CHECKOUT parent-attempt (x4) times
            // the production policy's max_parent_attempts (x5) overflows u64 mem-byte-seconds.
            let mut limits = linux_small_limits();
            limits.mem_bytes = u64::MAX / 15; // *4 (checkout) *5 (max_attempts) = *20 > u64::MAX
            limits.timeout_secs = 1;
            let request = v2_request(Some(checkout_scope_fixture()));
            let mut request = request;
            request.limits = limits;
            let policy = CiAttemptBudgetPolicy::production();
            let err = job_lifetime_ceiling(&request, &policy).unwrap_err();
            assert!(err.0.contains("overflow"), "message was: {}", err.0);
        }

        #[test]
        fn operational_amount_from_ceiling_add_overflow_refuses_loudly() {
            // cpu_seconds is already u64::MAX; adding even one memory-GiB-second overflows u64.
            let ceiling = ResourceCeiling {
                cpu_seconds: u64::MAX,
                mem_byte_seconds: GIB_BYTES,
            };
            let err = operational_amount_from_ceiling(ceiling).unwrap_err();
            assert!(err.0.contains("overflow"), "message was: {}", err.0);
        }

        #[test]
        fn operational_amount_from_ceiling_exceeding_i64_range_refuses_loudly() {
            // A total that fits comfortably in u64 (no checked_add overflow) but exceeds i64::MAX --
            // a DISTINCT failure mode from the checked_add overflow above.
            let ceiling = ResourceCeiling {
                cpu_seconds: i64::MAX as u64 + 100,
                mem_byte_seconds: 0,
            };
            let err = operational_amount_from_ceiling(ceiling).unwrap_err();
            assert!(err.0.contains("exceeds"), "message was: {}", err.0);
        }
    }

    /// CT-007 slice 5b.3-4a.1b: dual-version replay-compatibility tests. `resolve_durable_replay`
    /// and `replay_v2_batch` are pure, synchronous, in-memory decision functions -- no database
    /// required -- so every branch (exact v1/v2 replay, mixed-version refusal, tampered/partial
    /// divergence, policy-descriptor disagreement, replay surviving a provider reconfiguration) is
    /// covered here directly. Live-database coverage (active-ceiling counting across both prefixes,
    /// a real INSERT/SELECT round trip) lives in `tests/integration_ci_operational_reservation.rs`.
    mod handle_replay_5b3_4a1b {
        use super::*;

        fn fixture_request(job_id: &str) -> CiJobRuntimeAuthorityRequest {
            CiJobRuntimeAuthorityRequest {
                tenant_id: "acme".into(),
                region: "fr-par".into(),
                ci_run_id: "55555555-5555-5555-5555-555555555555".into(),
                wf_run_id: "66666666-6666-6666-6666-666666666666".into(),
                project_id: "77777777-7777-7777-7777-777777777777".into(),
                job_id: job_id.into(),
                stage: "build".into(),
                concrete_name: "build".into(),
                trigger_kind: "push".into(),
                trust_tier: "trusted".into(),
                source_snapshot_digest: "digest-fixture".into(),
                workflow_definition_version: 3,
                workflow_code_hash: "code-hash-fixture".into(),
                policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
                limits: linux_small_limits(),
                checkout: None,
            }
        }

        fn fixture_batch() -> Vec<CiJobRuntimeAuthorityRequest> {
            vec![
                fixture_request("88888888-8888-8888-8888-888888888881"),
                fixture_request("88888888-8888-8888-8888-888888888882"),
            ]
        }

        fn checkout_scope_fixture() -> CheckoutAuthorizationScope {
            myelin_ci_sandbox::derive_checkout_authorization_scope(
                myelin_ci_sandbox::JobKind::Ci,
                &myelin_ci_sandbox::WorkspaceSpec {
                    repo_ref: Some("myelin://acme/git/repo/widgets".into()),
                    commit: Some("a".repeat(40)),
                },
            )
            .unwrap()
            .unwrap()
        }

        fn prefixes(ci_run_id: &str) -> (String, String) {
            (
                format!("{TIER_P_OPERATIONAL_RESERVATION_PREFIX}{ci_run_id}:"),
                format!("{TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX}{ci_run_id}:"),
            )
        }

        #[test]
        fn no_durable_rows_returns_none_so_the_caller_proceeds_to_a_fresh_write() {
            let requests = fixture_batch();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            assert!(
                resolve_durable_replay(Vec::new(), &v1_prefix, &v2_prefix, &prepared).is_none()
            );
        }

        #[test]
        fn exact_v1_replay_still_returns_v1_handles() {
            let requests = fixture_batch();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let rows: Vec<(String, i64)> = prepared
                .iter()
                .map(|item| (item.v1_handle.clone(), item.v1_amount))
                .collect();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let result = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect("exact v1 replay must succeed");
            let expected: Vec<String> =
                prepared.iter().map(|item| item.v1_handle.clone()).collect();
            assert_eq!(result, expected);
        }

        #[test]
        fn v1_divergent_amount_refuses() {
            let requests = fixture_batch();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let mut rows: Vec<(String, i64)> = prepared
                .iter()
                .map(|item| (item.v1_handle.clone(), item.v1_amount))
                .collect();
            rows[0].1 += 1;
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("tampered v1 amount must refuse");
            assert!(err.0.contains("diverged"), "message was: {}", err.0);
        }

        #[test]
        fn exact_v2_replay_returns_the_persisted_v2_handles() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let rows: Vec<(String, i64)> = candidates
                .iter()
                .map(|candidate| (candidate.handle.clone(), candidate.amount))
                .collect();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let result = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect("exact v2 replay must succeed");
            let expected: Vec<String> = candidates
                .into_iter()
                .map(|candidate| candidate.handle)
                .collect();
            assert_eq!(result, expected);
        }

        #[test]
        fn tampered_v2_amount_refuses() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let mut rows: Vec<(String, i64)> = candidates
                .iter()
                .map(|candidate| (candidate.handle.clone(), candidate.amount))
                .collect();
            rows[0].1 += 1;
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("tampered v2 amount must refuse");
            assert!(err.0.contains("diverged"), "message was: {}", err.0);
        }

        #[test]
        fn tampered_v2_handle_digest_refuses() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let mut rows: Vec<(String, i64)> = candidates
                .iter()
                .map(|candidate| (candidate.handle.clone(), candidate.amount))
                .collect();
            // Flip the trailing hex character of the first row's digest suffix. It still PARSES
            // (the revision/attempts tags are untouched) but no longer matches any recomputed
            // candidate's handle string.
            let mut chars: Vec<char> = rows[0].0.chars().collect();
            let last = chars.len() - 1;
            chars[last] = if chars[last] == '0' { '1' } else { '0' };
            rows[0].0 = chars.into_iter().collect();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("tampered digest must refuse");
            assert!(err.0.contains("diverged"), "message was: {}", err.0);
        }

        #[test]
        fn mixed_v1_and_v2_rows_for_one_run_refuse() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let v2_candidates = build_v2_candidates(&requests, &policy).unwrap();
            let rows = vec![
                (prepared[0].v1_handle.clone(), prepared[0].v1_amount),
                (v2_candidates[1].handle.clone(), v2_candidates[1].amount),
            ];
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("mixed versions must refuse");
            assert!(err.0.contains("mixes v1 and v2"), "message was: {}", err.0);
        }

        #[test]
        fn v2_rows_disagreeing_on_policy_descriptor_refuse() {
            let requests = fixture_batch();
            let policy_a = CiAttemptBudgetPolicy::production();
            let policy_b = CiAttemptBudgetPolicy::new(
                CiAttemptBudgetRevision::V1,
                NonZeroU32::new(3).unwrap(),
            );
            let candidates_a = build_v2_candidates(&requests, &policy_a).unwrap();
            let candidates_b = build_v2_candidates(&requests, &policy_b).unwrap();
            let rows = vec![
                (candidates_a[0].handle.clone(), candidates_a[0].amount),
                (candidates_b[1].handle.clone(), candidates_b[1].amount),
            ];
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("disagreeing descriptors must refuse");
            assert!(
                err.0.contains("disagree on budget policy descriptor"),
                "message was: {}",
                err.0
            );
        }

        #[test]
        fn v2_batch_replays_correctly_under_a_non_default_written_policy() {
            // The durable rows were minted under a NON-default policy (3 attempts). Replay must
            // recover that from the rows' own embedded descriptor -- `prepared` carries no policy
            // at all any more (Sol's round-2 review: `prepare_operational_batch` must never touch
            // `v2` arithmetic eagerly), so there is nothing here for replay to fall back to except
            // what it parses from the durable handles themselves.
            let requests = fixture_batch();
            let written_policy = CiAttemptBudgetPolicy::new(
                CiAttemptBudgetRevision::V1,
                NonZeroU32::new(3).unwrap(),
            );
            let candidates = build_v2_candidates(&requests, &written_policy).unwrap();
            let rows: Vec<(String, i64)> = candidates
                .iter()
                .map(|candidate| (candidate.handle.clone(), candidate.amount))
                .collect();

            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let result = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect("must replay under the policy the rows were actually written with");
            let expected: Vec<String> = candidates
                .into_iter()
                .map(|candidate| candidate.handle)
                .collect();
            assert_eq!(result, expected);
        }

        #[test]
        fn fresh_v1_preparation_succeeds_even_when_a_v2_calculation_would_overflow() {
            // Sol's round-2 review: an earlier draft eagerly computed a v2 candidate inside
            // `prepare_operational_batch` under the provider's current policy, so a fresh v1 write
            // could fail purely because unused v2 ceiling arithmetic overflowed. `limits` below is
            // exactly the fixture the existing `job_lifetime_ceiling_overflow_refuses_loudly` test
            // uses to trigger that v2 overflow under the checkout-4x times production's 5-attempt
            // multiplier (mem_bytes = u64::MAX / 15, so 20x overflows u64) -- proving v1 preparation
            // never even evaluates that arithmetic.
            let mut request = fixture_request("88888888-8888-8888-8888-888888888881");
            request.limits.mem_bytes = u64::MAX / 15;
            request.limits.timeout_secs = 1;
            request.checkout = Some(checkout_scope_fixture());
            assert!(
                operational_reservation_amount_v2(&request, &CiAttemptBudgetPolicy::production())
                    .is_err(),
                "fixture must actually trigger a v2 overflow under the production policy"
            );

            let prepared = prepare_operational_batch("fr-par", 10, vec![request])
                .expect("v1 preparation must never evaluate v2 arithmetic, let alone fail from it");
            assert_eq!(prepared.len(), 1);
        }

        #[test]
        fn exact_v1_replay_succeeds_even_when_a_v2_calculation_would_overflow() {
            let mut request = fixture_request("88888888-8888-8888-8888-888888888881");
            request.limits.mem_bytes = u64::MAX / 15;
            request.limits.timeout_secs = 1;
            request.checkout = Some(checkout_scope_fixture());
            let requests = vec![request];

            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let rows: Vec<(String, i64)> = prepared
                .iter()
                .map(|item| (item.v1_handle.clone(), item.v1_amount))
                .collect();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let result = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect("v1 replay must never evaluate v2 arithmetic, let alone fail from it");
            assert_eq!(result, vec![prepared[0].v1_handle.clone()]);
        }

        #[test]
        fn exact_v2_replay_succeeds_under_a_safe_written_policy_when_production_would_overflow() {
            // Same overflow-triggering limits as above, but replayed as a v2 batch written under a
            // SAFE (1-attempt) policy instead of production's 5-attempt one. Proves replay recomputes
            // strictly under the policy parsed from the durable rows -- never under
            // `CiAttemptBudgetPolicy::production()`, which this test never even hands to replay.
            let mut request = fixture_request("88888888-8888-8888-8888-888888888881");
            request.limits.mem_bytes = u64::MAX / 15;
            request.limits.timeout_secs = 1;
            request.checkout = Some(checkout_scope_fixture());
            let requests = vec![request];

            assert!(
                operational_reservation_amount_v2(
                    &requests[0],
                    &CiAttemptBudgetPolicy::production()
                )
                .is_err(),
                "fixture must actually trigger a v2 overflow under the production policy"
            );
            let safe_policy = CiAttemptBudgetPolicy::new(
                CiAttemptBudgetRevision::V1,
                NonZeroU32::new(1).unwrap(),
            );
            assert!(
                operational_reservation_amount_v2(&requests[0], &safe_policy).is_ok(),
                "fixture must NOT overflow under the safe policy used to write these durable rows"
            );

            let candidates = build_v2_candidates(&requests, &safe_policy).unwrap();
            let rows: Vec<(String, i64)> = candidates
                .iter()
                .map(|candidate| (candidate.handle.clone(), candidate.amount))
                .collect();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let result = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect("v2 replay must succeed under the policy it was actually written with");
            let expected: Vec<String> = candidates
                .into_iter()
                .map(|candidate| candidate.handle)
                .collect();
            assert_eq!(result, expected);
        }

        #[test]
        fn partial_v2_batch_refuses() {
            // One durable row for a two-job batch: the module's design narrative promises "partial
            // divergence" refuses, but nothing previously exercised it directly for v2.
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let rows = vec![(candidates[0].handle.clone(), candidates[0].amount)];
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("a partial v2 batch must refuse, never partially replay");
            assert!(err.0.contains("diverged"), "message was: {}", err.0);
        }

        #[test]
        fn golden_v2_batch_digest_for_a_fixed_two_job_batch_is_pinned() {
            // External golden vector (Sol's round-2 review): the per-request
            // `operational_reservation_digest_v2` is already pinned by the 4a.1a goldens, but the
            // BATCH framing (domain, cardinality, ordering, amount) is new to this slice and was
            // previously only exercised by generating expectations through `build_v2_candidates`
            // itself -- a tautology. These are computed once and pasted as literals (never compared
            // via the same function under test).
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            assert_eq!(
                candidates[0].handle,
                "ci-reserve:v2:55555555-5555-5555-5555-555555555555:budget-v1:a5:1be02316af6d18a5375d858afaafba500d7d3ee3fc9a3ee2f41a29463f7879e9:88888888-8888-8888-8888-888888888881:681727ec3c89359a8cf69a95741a3a28b168fc058f124de03d68f681c8b34007"
            );
            assert_eq!(candidates[0].amount, 3_750);
            assert_eq!(
                candidates[1].handle,
                "ci-reserve:v2:55555555-5555-5555-5555-555555555555:budget-v1:a5:1be02316af6d18a5375d858afaafba500d7d3ee3fc9a3ee2f41a29463f7879e9:88888888-8888-8888-8888-888888888882:a7f2a99c7a388abbae3f691c6946ac0ef51c5967bde5c28742f9f71192ee194e"
            );
            assert_eq!(candidates[1].amount, 3_750);
        }

        #[test]
        fn unparseable_v2_handle_refuses_replay() {
            let requests = fixture_batch();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let rows = vec![(format!("{v2_prefix}not-enough-segments"), 100i64)];
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("unparseable handle must refuse");
            assert!(err.0.contains("unparseable"), "message was: {}", err.0);
        }

        #[test]
        fn row_matching_neither_prefix_refuses() {
            let requests = fixture_batch();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let rows = vec![("nonsense-run-id".to_string(), 1i64)];
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("must refuse");
            assert!(err.0.contains("matched neither"), "message was: {}", err.0);
        }

        #[test]
        fn duplicate_durable_v2_handle_refuses() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let prepared = prepare_operational_batch("fr-par", 10, requests.clone()).unwrap();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let rows = vec![
                (candidates[0].handle.clone(), candidates[0].amount),
                (candidates[0].handle.clone(), candidates[0].amount),
            ];
            let (v1_prefix, v2_prefix) = prefixes(&requests[0].ci_run_id);
            let err = resolve_durable_replay(rows, &v1_prefix, &v2_prefix, &prepared)
                .expect("durable rows present")
                .expect_err("duplicate rows must refuse");
            assert!(err.0.contains("duplicate"), "message was: {}", err.0);
        }

        #[test]
        fn attempts_handle_tag_round_trips() {
            let n = NonZeroU32::new(7).unwrap();
            assert_eq!(parse_attempts_handle_tag(&attempts_handle_tag(n)), Some(n));
        }

        #[test]
        fn attempts_handle_tag_rejects_garbage_and_zero() {
            assert_eq!(parse_attempts_handle_tag("a0"), None);
            assert_eq!(parse_attempts_handle_tag("5"), None);
            assert_eq!(parse_attempts_handle_tag("aX"), None);
            assert_eq!(parse_attempts_handle_tag(""), None);
        }

        #[test]
        fn budget_revision_handle_tag_round_trips() {
            assert_eq!(
                CiAttemptBudgetRevision::from_handle_tag(CiAttemptBudgetRevision::V1.handle_tag()),
                Some(CiAttemptBudgetRevision::V1)
            );
        }

        #[test]
        fn budget_revision_handle_tag_rejects_unknown() {
            assert_eq!(CiAttemptBudgetRevision::from_handle_tag("budget-v2"), None);
            assert_eq!(CiAttemptBudgetRevision::from_handle_tag(""), None);
        }

        #[test]
        fn parse_v2_handle_round_trips_a_real_candidate() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let parsed =
                parse_v2_handle(&candidates[0].handle).expect("must parse a real candidate");
            assert_eq!(parsed.ci_run_id, requests[0].ci_run_id);
            assert_eq!(parsed.revision, policy.revision());
            assert_eq!(parsed.max_parent_attempts, policy.max_parent_attempts());
            assert_eq!(parsed.job_id, requests[0].job_id);
        }

        #[test]
        fn narrow_v2_resolver_recomputes_the_complete_batch_and_policy() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::new(
                CiAttemptBudgetRevision::V1,
                NonZeroU32::new(3).unwrap(),
            );
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            let recovered = verify_v2_operational_reservation(
                &requests,
                &requests[0].ci_run_id,
                &requests[0].job_id,
                &candidates[0].handle,
                candidates[0].amount,
            )
            .unwrap();
            assert_eq!(recovered, policy);
        }

        #[test]
        fn narrow_v2_resolver_refuses_identity_digest_and_amount_divergence() {
            let requests = fixture_batch();
            let policy = CiAttemptBudgetPolicy::production();
            let candidates = build_v2_candidates(&requests, &policy).unwrap();
            assert!(verify_v2_operational_reservation(
                &requests,
                &requests[0].ci_run_id,
                &requests[1].job_id,
                &candidates[0].handle,
                candidates[0].amount,
            )
            .is_err());
            let mut tampered = candidates[0].handle.clone();
            let last = tampered.pop().unwrap();
            tampered.push(if last == '0' { '1' } else { '0' });
            assert!(verify_v2_operational_reservation(
                &requests,
                &requests[0].ci_run_id,
                &requests[0].job_id,
                &tampered,
                candidates[0].amount,
            )
            .is_err());
            assert!(verify_v2_operational_reservation(
                &requests,
                &requests[0].ci_run_id,
                &requests[0].job_id,
                &candidates[0].handle,
                candidates[0].amount + 1,
            )
            .is_err());
        }

        #[test]
        fn parse_v2_handle_rejects_wrong_prefix() {
            assert!(parse_v2_handle("ci-reserve:v1:run:budget-v1:a5:batch:job:digest").is_none());
        }

        #[test]
        fn parse_v2_handle_rejects_wrong_arity() {
            assert!(parse_v2_handle("ci-reserve:v2:run:budget-v1:a5:batch:job").is_none());
            assert!(
                parse_v2_handle("ci-reserve:v2:run:budget-v1:a5:batch:job:digest:extra").is_none()
            );
        }

        #[test]
        fn parse_v2_handle_rejects_unknown_tags() {
            assert!(parse_v2_handle("ci-reserve:v2:run:budget-v9:a5:batch:job:digest").is_none());
            assert!(parse_v2_handle("ci-reserve:v2:run:budget-v1:aX:batch:job:digest").is_none());
            assert!(parse_v2_handle("ci-reserve:v2:run:budget-v1:a0:batch:job:digest").is_none());
        }
    }

    #[test]
    fn tier_p_pricer_uses_the_same_operational_units_as_the_reservation_bound() {
        let priced = TierPOperationalCiJobPricer
            .price(ResourceUsage {
                cpu_seconds: 600,
                mem_byte_seconds: 256 * 1024 * 1024 * 600,
            })
            .unwrap();
        assert_eq!(priced.pricing_revision, TIER_P_OPERATIONAL_PRICING_REVISION);
        assert_eq!(priced.memory_gb_seconds, 150);
        assert_eq!(priced.cpu_wholesale, MinorUnits(600));
        assert_eq!(priced.memory_wholesale, MinorUnits(150));
        assert_eq!(priced.cpu_markup, MinorUnits::ZERO);
        assert_eq!(priced.memory_markup, MinorUnits::ZERO);
        assert_eq!(
            operational_reservation_amount(&linux_small_limits()).unwrap(),
            750
        );

        assert_eq!(
            TierPOperationalCiJobPricer
                .price(ResourceUsage {
                    cpu_seconds: u64::MAX,
                    mem_byte_seconds: GIB_BYTES,
                })
                .unwrap_err(),
            CiJobPricingError::InvalidOutput
        );
    }

    #[test]
    fn production_operational_ceiling_admits_one_largest_valid_run_when_idle() {
        assert_eq!(
            TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING as usize,
            crate::run_plan::MAX_RUN_PLAN_JOBS
        );
    }

    fn fixture() -> (CiRunRecord, PreparedRunPlanV2) {
        let tenant = TenantId("acme".into());
        let plan = ResolvedRunPlanV2 {
            schema_version: RUN_PLAN_SCHEMA_V2,
            execution: CiExecutionRequestV1 {
                schema_version: EXECUTION_REQUEST_SCHEMA_V1,
                profile: CiExecutionProfileV1::LinuxSmallV1,
            },
            jobs: vec![
                ResolvedJobV2 {
                    stage: "build".into(),
                    name: "build".into(),
                    image: PINNED_IMAGE.into(),
                    command: vec!["/bin/build".into()],
                    needs: Vec::new(),
                    is_generator: false,
                    matrix_key: BTreeMap::new(),
                },
                ResolvedJobV2 {
                    stage: "test".into(),
                    name: "test".into(),
                    image: PINNED_IMAGE.into(),
                    command: vec!["/bin/test".into()],
                    needs: vec!["build".into()],
                    is_generator: false,
                    matrix_key: BTreeMap::new(),
                },
            ],
        };
        let bytes = plan.canonical_bytes().unwrap();
        let blobs = FsBlobStore::new();
        let hash = blobs.put(&tenant, &bytes).unwrap();
        let record = CiRunRecord {
            tenant_id: tenant.0.clone(),
            run_id: "10000000-0000-0000-0000-000000000001".into(),
            region: "fr-par".into(),
            project_id: "20000000-0000-0000-0000-000000000001".into(),
            pipeline_id: "30000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "40000000-0000-0000-0000-000000000001".into(),
            repo_ref: Some("myelin://acme/git/repo/core".into()),
            commit_oid: Some("deadbeef00deadbeef00deadbeef00deadbeef00".into()),
            cause_event_id: None,
            cause_depth: 0,
            caused_by: None,
            definition_snapshot: format!(
                "myelin://acme/ci/snapshot/{}",
                hash.to_multihash_string()
            ),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "50000000-0000-0000-0000-000000000001".into(),
        };
        let prepared = load_launch_run_plan_v2(&blobs, &record).unwrap();
        (record, prepared)
    }

    #[tokio::test]
    async fn customer_profile_becomes_fixed_default_deny_server_grants() {
        let (record, prepared) = fixture();
        let runtime = Arc::new(RecordingBudget::default());
        let policy = policy(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();
        let authority = policy.materialize(&record, &prepared, &pin).await.unwrap();

        assert_eq!(authority.policy_revision, LINUX_SMALL_V1_POLICY_REVISION);
        assert_eq!(authority.jobs.len(), 2);
        assert!(authority.merge_waiter.is_none());
        for grant in &authority.jobs {
            assert!(grant.env.is_empty());
            assert!(grant.secret_handles.is_empty());
            assert!(grant.egress_allow.is_empty());
            assert_eq!(grant.limits, linux_small_limits());
            assert_eq!(grant.scheduling.lane, CiManifestLaneV1::Batch);
            assert_eq!(
                grant.scheduling.labels,
                LINUX_SMALL_V1_RUNNER_LABELS
                    .iter()
                    .map(|label| (*label).to_owned())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                grant.scheduling.fair_key,
                "project:20000000-0000-0000-0000-000000000001"
            );
            assert!(
                grant.scheduling.concurrency_group.is_none(),
                "push runs carry no PR supersession key"
            );
            assert!(grant
                .token_authority_handle
                .starts_with("ci-token-authority:v2:"));
        }
        assert_ne!(
            authority.jobs[0].token_authority_handle,
            authority.jobs[1].token_authority_handle
        );
        let batches = runtime.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        let requests = &batches[0];
        assert_eq!(requests.len(), 2);
        let run_id = sqlx::types::Uuid::parse_str(&record.run_id).unwrap();
        for (request, job) in requests.iter().zip(&prepared.plan().jobs) {
            assert_eq!(request.tenant_id, "acme");
            assert_eq!(request.region, "fr-par");
            assert_eq!(request.ci_run_id, record.run_id);
            assert_eq!(request.wf_run_id, record.wf_run_id);
            assert_eq!(request.project_id, record.project_id);
            assert_eq!(
                request.job_id,
                ci_job_id_v2(
                    prepared.tenant(),
                    run_id,
                    &job.stage,
                    &job.name,
                    &job.matrix_identity(),
                )
                .to_string()
            );
            assert_eq!(request.stage, job.stage);
            assert_eq!(request.concrete_name, job.name);
            assert_eq!(request.trigger_kind, "push");
            assert_eq!(request.trust_tier, "trusted");
            assert_eq!(
                request.source_snapshot_digest,
                prepared.content_hash().to_multihash_string()
            );
            assert_eq!(request.workflow_definition_version, 1);
            assert_eq!(request.workflow_code_hash, "ci-body-v1");
            assert_eq!(request.policy_revision, LINUX_SMALL_V1_POLICY_REVISION);
            assert_eq!(request.limits, linux_small_limits());
        }
    }

    #[tokio::test]
    async fn canonical_pr_identity_reaches_every_grant_and_missing_identity_refuses_pre_reserve() {
        let (mut record, prepared) = fixture();
        record.trigger_kind = "pull_request".into();
        record.concurrency_group = Some("pr:team/core:42".into());
        record.pr_head_generation = Some(7);
        let budget = Arc::new(RecordingBudget::default());
        let policy = policy(budget.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let authority = policy.materialize(&record, &prepared, &pin).await.unwrap();
        assert!(authority
            .jobs
            .iter()
            .all(|job| { job.scheduling.concurrency_group.as_deref() == Some("pr:team/core:42") }));
        assert_eq!(budget.batches.lock().unwrap().len(), 1);

        record.pr_head_generation = None;
        assert_eq!(
            launch_concurrency_group(&record).unwrap().as_deref(),
            Some("pr:team/core:42"),
            "an old-dispatcher row remains launchable during the rolling migration"
        );

        record.concurrency_group = None;
        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .expect_err("a legacy PR row without its event-derived identity fails closed");
        assert!(error
            .0
            .contains("lacks canonical concurrency identity or producer generation"));
        assert_eq!(
            budget.batches.lock().unwrap().len(),
            1,
            "invalid PR authority is refused before reserving operational capacity"
        );
    }

    #[tokio::test]
    async fn exact_retry_replays_one_identical_complete_budget_batch_and_authority() {
        let (record, prepared) = fixture();
        let budget = Arc::new(RecordingBudget::default());
        let policy = policy(budget.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let first = policy.materialize(&record, &prepared, &pin).await.unwrap();
        let second = policy.materialize(&record, &prepared, &pin).await.unwrap();

        assert_eq!(second, first);
        let batches = budget.batches.lock().unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), prepared.plan().jobs.len());
        assert_eq!(batches[1], batches[0]);
    }

    fn token_handle(request: &CiJobRuntimeAuthorityRequest) -> String {
        ManifestBoundCiJobTokenAuthority::handle_for(request)
    }

    #[tokio::test]
    async fn token_authority_handle_is_retry_stable_and_binds_every_request_field() {
        let (record, prepared) = fixture();
        let budget = Arc::new(RecordingBudget::default());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();
        policy(budget.clone())
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap();
        let base = budget.batches.lock().unwrap()[0][0].clone();
        let expected = token_handle(&base);
        assert_eq!(token_handle(&base), expected);
        assert!(ManifestBoundCiJobTokenAuthority::verifies(&base, &expected));

        let mut variants = Vec::new();
        macro_rules! changed {
            ($field:ident, $value:expr) => {{
                let mut request = base.clone();
                request.$field = $value;
                variants.push(request);
            }};
        }
        changed!(tenant_id, "globex".into());
        changed!(region, "nbg1".into());
        changed!(ci_run_id, "10000000-0000-0000-0000-000000000002".into());
        changed!(wf_run_id, "40000000-0000-0000-0000-000000000002".into());
        changed!(project_id, "20000000-0000-0000-0000-000000000002".into());
        changed!(job_id, "60000000-0000-0000-0000-000000000002".into());
        changed!(stage, "verify".into());
        changed!(concrete_name, "verify-linux".into());
        changed!(trigger_kind, "manual".into());
        changed!(trust_tier, "untrusted_fork".into());
        changed!(source_snapshot_digest, "bafkreidifferent".into());
        changed!(workflow_definition_version, 2);
        changed!(workflow_code_hash, "ci-body-v2".into());
        changed!(policy_revision, "linux-small-v1:2".into());
        let mut cpu_limits = base.limits.clone();
        cpu_limits.cpu_millis += 1;
        changed!(limits, cpu_limits);
        let mut memory_limits = base.limits.clone();
        memory_limits.mem_bytes += 1;
        changed!(limits, memory_limits);
        let mut disk_limits = base.limits.clone();
        disk_limits.disk_bytes += 1;
        changed!(limits, disk_limits);
        let mut pid_limits = base.limits.clone();
        pid_limits.pids_max += 1;
        changed!(limits, pid_limits);
        let mut timeout_limits = base.limits.clone();
        timeout_limits.timeout_secs += 1;
        changed!(limits, timeout_limits);
        changed!(checkout, None);
        let different_commit_scope = myelin_ci_sandbox::derive_checkout_authorization_scope(
            myelin_ci_sandbox::JobKind::Ci,
            &myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/core".into()),
                commit: Some("f".repeat(40)),
            },
        )
        .unwrap();
        changed!(checkout, different_commit_scope);

        for variant in variants {
            assert!(!ManifestBoundCiJobTokenAuthority::verifies(
                &variant, &expected
            ));
            assert_ne!(token_handle(&variant), expected);
        }
    }

    #[tokio::test]
    async fn duplicate_job_reservations_are_refused_before_manifest_creation() {
        let (record, prepared) = fixture();
        let runtime = Arc::new(RecordingBudget {
            batches: Mutex::new(Vec::new()),
            duplicate_reservation: true,
            bad_cardinality: false,
            refusal: None,
        });
        let policy = policy(runtime);
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();
        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap_err();
        assert!(error.0.contains("reused one reservation"));
    }

    #[tokio::test]
    async fn mismatched_tenant_and_nonqueued_runs_are_refused_without_external_calls() {
        let (mut record, prepared) = fixture();
        let runtime = Arc::new(RecordingBudget::default());
        let policy = policy(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        record.tenant_id = "other".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        record.tenant_id = "acme".into();
        record.state = "running".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        assert!(runtime.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_locked_scope_is_refused_before_external_calls() {
        let (mut record, prepared) = fixture();
        let runtime = Arc::new(RecordingBudget::default());
        let policy = policy(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        record.project_id = "not-a-uuid".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        record.project_id = "20000000-0000-0000-0000-000000000001".into();
        record.trigger_kind = "customer-defined".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        assert!(runtime.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_authority_refusal_is_propagated_without_a_partial_manifest() {
        let (record, prepared) = fixture();
        let runtime = Arc::new(RecordingBudget {
            batches: Mutex::new(Vec::new()),
            duplicate_reservation: false,
            bad_cardinality: false,
            refusal: Some("budget unavailable".into()),
        });
        let policy = policy(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap_err();
        assert_eq!(error.0, "budget unavailable");
        let batches = runtime.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), prepared.plan().jobs.len());
    }

    #[tokio::test]
    async fn wrong_budget_cardinality_is_refused_before_manifest_creation() {
        let (record, prepared) = fixture();
        let budget = Arc::new(RecordingBudget {
            batches: Mutex::new(Vec::new()),
            duplicate_reservation: false,
            bad_cardinality: true,
            refusal: None,
        });
        let policy = policy(budget);
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap_err();
        assert!(error.0.contains("wrong reservation cardinality"));
    }
}
