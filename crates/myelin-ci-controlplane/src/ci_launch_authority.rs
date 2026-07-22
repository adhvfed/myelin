//! Policy-owned launch grants for the first executable CI profile.
//!
//! Customer V2 plans may request `linux-small-v1`, but they carry no runtime authority. This module
//! turns that request into fixed, server-owned isolation and scheduling terms. It delegates the
//! durable money reservation to one explicit provider, while deriving a content-bound token-authority
//! reference locally; the existing claim-time `CiJobTokenIssuer` remains the only bearer-mint seam.
//! Keeping those capabilities separate matters for Tier P: Identity can become real without
//! inventing the Commercial wallet that remains deliberately deferred. There is no budget-provider
//! default and no caller-controlled egress, limits, labels, or lane.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{
    ci_job_id_v2, CiExecutionProfileV1, CiJobLaunchGrantV1, CiLaunchAuthorityError,
    CiLaunchAuthorityMaterializer, CiLaunchAuthorityV1, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiRunRecord, CiWorkflowDefinitionPin, PreparedRunPlanV2,
};

pub const LINUX_SMALL_V1_POLICY_REVISION: &str = "linux-small-v1:1";

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

/// External money-reservation boundary used by the server policy. The complete job set is one
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
}

/// Explicit fail-closed money placeholder used by the dormant production composition. Keeping this
/// one layer below the real policy adapter means the composed starter exercises the fixed policy
/// mapping while the Tier-P operational quota/cost reservation source remains the exact visible
/// blocker. This is a safety/metering floor, not billing, Stripe, or a Commercial wallet, and it
/// never fabricates a reservation handle.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnavailableCiJobBudgetReservation;

impl CiJobBudgetReservationProvider for UnavailableCiJobBudgetReservation {
    fn reserve_batch<'a>(
        &'a self,
        _requests: Vec<CiJobRuntimeAuthorityRequest>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CiLaunchAuthorityError>> + Send + 'a>>
    {
        Box::pin(async {
            Err(refused(
                "durable CI budget reservation authority is not composed",
            ))
        })
    }
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
            let run_id = validate_run_scope(record, prepared)?;
            if prepared.plan().execution.profile != CiExecutionProfileV1::LinuxSmallV1 {
                return Err(refused("unsupported CI execution profile"));
            }
            let limits = linux_small_limits();
            let mut requests = Vec::with_capacity(prepared.plan().jobs.len());
            for job in &prepared.plan().jobs {
                let job_id = ci_job_id_v2(
                    prepared.tenant(),
                    run_id,
                    &job.stage,
                    &job.name,
                    &job.matrix_identity(),
                )
                .to_string();
                let request = CiJobRuntimeAuthorityRequest {
                    tenant_id: record.tenant_id.clone(),
                    region: record.region.clone(),
                    ci_run_id: record.run_id.clone(),
                    wf_run_id: record.wf_run_id.clone(),
                    project_id: record.project_id.clone(),
                    job_id,
                    stage: job.stage.clone(),
                    concrete_name: job.name.clone(),
                    trigger_kind: record.trigger_kind.clone(),
                    trust_tier: record.trust_tier.clone(),
                    source_snapshot_digest: prepared.content_hash().to_multihash_string(),
                    workflow_definition_version: definition.version(),
                    workflow_code_hash: definition.code_hash().into(),
                    policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
                    limits: limits.clone(),
                };
                requests.push(request);
            }

            // The only external side effect receives the complete set and must commit all-or-none.
            let reserve_handles = self
                .budget_reservations
                .reserve_batch(requests.clone())
                .await?;
            if reserve_handles.len() != requests.len() {
                return Err(refused(
                    "budget authority returned the wrong reservation cardinality",
                ));
            }
            let mut unique_reservations = BTreeSet::new();
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
                // Pure local derivation follows the committed exact batch. Bearer minting is a
                // separate claim-time seam and can never widen this reservation request.
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
                        labels: vec!["linux".into(), "linux-small-v1".into()],
                        concurrency_group: None,
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
        })
    }
}

const CI_TOKEN_AUTHORITY_V1_DOMAIN: &[u8] = b"myelin.ci.token-authority.v1\0";

fn token_authority_digest(request: &CiJobRuntimeAuthorityRequest) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_TOKEN_AUTHORITY_V1_DOMAIN);
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
                vec!["linux".to_string(), "linux-small-v1".to_string()]
            );
            assert_eq!(
                grant.scheduling.fair_key,
                "project:20000000-0000-0000-0000-000000000001"
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

    #[tokio::test]
    async fn dormant_budget_provider_never_fabricates_reservation_handles() {
        let (record, prepared) = fixture();
        let policy = policy(Arc::new(UnavailableCiJobBudgetReservation));
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap_err();
        assert!(error.0.contains("not composed"));
    }
}
