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
use std::pin::Pin;
use std::sync::Arc;

use crate::ci_pipeline_driver::{
    TIER_P_OPERATIONAL_PRICING_REVISION, TIER_P_OPERATIONAL_RESERVATION_PREFIX,
};
use crate::{
    ci_job_id_v2, CiExecutionProfileV1, CiJobAccountingPricer, CiJobLaunchGrantV1,
    CiJobPricingError, CiLaunchAuthorityError, CiLaunchAuthorityMaterializer, CiLaunchAuthorityV1,
    CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiRunRecord,
    CiWorkflowDefinitionPin, PreparedRunPlanV2, PricedCiJobUsage,
};
use myelin_ci_sandbox::ResourceUsage;
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
            let tx_tenant = tenant.clone();
            let tx_region = region.clone();
            let result = with_tenant_tx(&self.pool, &tenant, &region, move |conn| {
                Box::pin(async move {
                    reserve_operational_batch_on_conn(
                        conn, &tx_tenant, &tx_region, ceiling, &prepared,
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
                &prepared,
            )
            .await
            .map_err(|_| refused("durable operational reservation did not commit"))?
        })
    }
}

#[derive(Clone)]
struct PreparedOperationalReservation {
    request: CiJobRuntimeAuthorityRequest,
    handle: String,
    amount: i64,
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
    for (request, amount) in validated {
        let request_digest =
            runtime_authority_digest(CI_OPERATIONAL_RESERVATION_V1_DOMAIN, &request);
        let handle = format!(
            "{TIER_P_OPERATIONAL_RESERVATION_PREFIX}{}:{batch_digest}:{}:{request_digest}",
            request.ci_run_id, request.job_id
        );
        validate_handle("reserve", &handle)?;
        prepared.push(PreparedOperationalReservation {
            request,
            handle,
            amount,
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
    prepared: &[PreparedOperationalReservation],
) -> Result<Result<Vec<String>, CiLaunchAuthorityError>, PgError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.operational-reservation.v1:{tenant_id}:{region}"
        ))
        .execute(&mut *conn)
        .await
        .map_err(pg_query)?;

    let run_prefix = format!(
        "{TIER_P_OPERATIONAL_RESERVATION_PREFIX}{}:",
        prepared[0].request.ci_run_id
    );
    let rows = sqlx::query(
        "SELECT run_id, reserved FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 AND run_id LIKE ($3 || '%')",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(run_prefix)
    .fetch_all(&mut *conn)
    .await
    .map_err(pg_query)?;
    if !rows.is_empty() {
        let expected = prepared
            .iter()
            .map(|item| (item.handle.as_str(), item.amount))
            .collect::<BTreeMap<_, _>>();
        let mut durable = BTreeMap::new();
        for row in rows {
            let handle = row.try_get::<String, _>("run_id").map_err(pg_row)?;
            let amount = row.try_get::<i64, _>("reserved").map_err(pg_row)?;
            if durable.insert(handle, amount).is_some() {
                return Ok(Err(refused(
                    "operational reservation run has duplicate durable authority",
                )));
            }
        }
        let exact = durable.len() == expected.len()
            && durable
                .iter()
                .all(|(handle, amount)| expected.get(handle.as_str()) == Some(amount));
        if exact {
            return Ok(Ok(prepared
                .iter()
                .map(|item| item.handle.clone())
                .collect()));
        }
        return Ok(Err(refused(
            "operational reservation run authority diverged from its durable batch",
        )));
    }

    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cost_reservation \
         WHERE tenant_id = $1 AND region = $2 \
           AND run_id LIKE ($3 || '%') AND state IN ('reserved', 'inflight')",
    )
    .bind(tenant_id)
    .bind(region)
    .bind(TIER_P_OPERATIONAL_RESERVATION_PREFIX)
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

    for item in prepared {
        sqlx::query(
            "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state) \
             VALUES ($1, $2, $3, $4, 'reserved')",
        )
        .bind(tenant_id)
        .bind(region)
        .bind(&item.handle)
        .bind(item.amount)
        .execute(&mut *conn)
        .await
        .map_err(pg_query)?;
    }
    Ok(Ok(prepared
        .iter()
        .map(|item| item.handle.clone())
        .collect()))
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

/// Content-addressed token-authority reference. The immutable manifest persists this handle, and a
/// later claim-bound issuer can reload the manifest and recompute it before minting. The hash binds
/// every locked identity, source, workflow, policy, and limit field; it contains no secret and grants
/// no authority by itself.
#[derive(Clone, Debug, Default)]
pub struct ManifestBoundCiJobTokenAuthority;

impl ManifestBoundCiJobTokenAuthority {
    pub fn handle_for(request: &CiJobRuntimeAuthorityRequest) -> String {
        format!("ci-token-authority:v1:{}", token_authority_digest(request))
    }

    /// Recompute the public authority reference from server-resolved facts. This is not bearer
    /// verification; the claim-bound issuer uses it before asking Identity to mint a credential.
    pub fn verifies(request: &CiJobRuntimeAuthorityRequest, handle: &str) -> bool {
        Self::handle_for(request) == handle
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
    ) {
        ("pull_request", Some(group)) if crate::ci_run_store::valid_pr_concurrency_group(group) => {
            Ok(Some(group.to_owned()))
        }
        ("pull_request", _) => Err(refused(
            "pull-request run lacks canonical concurrency identity",
        )),
        (_, None) => Ok(None),
        (_, Some(_)) => Err(refused(
            "non-pull-request run carries PR concurrency identity",
        )),
    }
}

const CI_TOKEN_AUTHORITY_V1_DOMAIN: &[u8] = b"myelin.ci.token-authority.v1\0";

fn token_authority_digest(request: &CiJobRuntimeAuthorityRequest) -> blake3::Hash {
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

fn linux_small_limits() -> CiManifestLimitsV1 {
    CiManifestLimitsV1 {
        cpu_millis: 1_000,
        mem_bytes: 256 * 1024 * 1024,
        disk_bytes: 1024 * 1024 * 1024,
        pids_max: 128,
        timeout_secs: 600,
    }
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
            commit_oid: Some("deadbeef".into()),
            cause_event_id: None,
            cause_depth: 0,
            caused_by: None,
            definition_snapshot: format!(
                "myelin://acme/ci/snapshot/{}",
                hash.to_multihash_string()
            ),
            trigger_kind: "push".into(),
            concurrency_group: None,
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
                .starts_with("ci-token-authority:v1:"));
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
        let budget = Arc::new(RecordingBudget::default());
        let policy = policy(budget.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let authority = policy.materialize(&record, &prepared, &pin).await.unwrap();
        assert!(authority
            .jobs
            .iter()
            .all(|job| { job.scheduling.concurrency_group.as_deref() == Some("pr:team/core:42") }));
        assert_eq!(budget.batches.lock().unwrap().len(), 1);

        record.concurrency_group = None;
        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .expect_err("a legacy PR row without its event-derived identity fails closed");
        assert!(error.0.contains("lacks canonical concurrency identity"));
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
