//! Policy-owned launch grants for the first executable CI profile.
//!
//! Customer V2 plans may request `linux-small-v1`, but they carry no runtime authority. This module
//! turns that request into fixed, server-owned isolation and scheduling terms while delegating only
//! the two durable external capabilities—budget reservation and token-mint authority—to an explicit
//! provider. There is no provider default and no caller-controlled egress, limits, labels, or lane.

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

/// Complete immutable request to the durable budget/token authority. All identity comes from the
/// locked `ci_run` and validated plan; the provider cannot replace executable or scheduling terms.
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

/// Stable handles returned after durable reservation and token-authority materialization. These are
/// references, never bearer material. Exact retry of the same request must return the same handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobRuntimeAuthority {
    pub reserve_handle: String,
    pub token_authority_handle: String,
}

/// External side-effect boundary used by the server policy. Implementations must be retry-safe for
/// the full request because PostgreSQL rollback can occur after a successful external call.
pub trait CiJobRuntimeAuthorityProvider: Send + Sync {
    fn materialize<'a>(
        &'a self,
        request: CiJobRuntimeAuthorityRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiJobRuntimeAuthority, CiLaunchAuthorityError>> + Send + 'a>,
    >;
}

/// Server policy for the only V2 execution profile currently accepted. Resource limits, default
/// deny egress, batch scheduling, and fair-share identity are constants owned here—not plan fields.
#[derive(Clone)]
pub struct LinuxSmallV1LaunchAuthority {
    runtime_authority: Arc<dyn CiJobRuntimeAuthorityProvider>,
}

impl LinuxSmallV1LaunchAuthority {
    pub fn new(runtime_authority: Arc<dyn CiJobRuntimeAuthorityProvider>) -> Self {
        Self { runtime_authority }
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
            let mut grants = Vec::with_capacity(prepared.plan().jobs.len());
            let mut reserve_handles = BTreeSet::new();

            for job in &prepared.plan().jobs {
                let job_id = ci_job_id_v2(
                    prepared.tenant(),
                    run_id,
                    &job.stage,
                    &job.name,
                    &job.matrix_identity(),
                )
                .to_string();
                let authority = self
                    .runtime_authority
                    .materialize(CiJobRuntimeAuthorityRequest {
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
                    })
                    .await?;
                validate_handle("reserve", &authority.reserve_handle)?;
                validate_handle("token authority", &authority.token_authority_handle)?;
                if !reserve_handles.insert(authority.reserve_handle.clone()) {
                    return Err(refused(
                        "runtime authority reused one reservation across jobs",
                    ));
                }
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
                    reserve_handle: authority.reserve_handle,
                    token_authority_handle: authority.token_authority_handle,
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
    struct RecordingAuthority {
        requests: Mutex<Vec<CiJobRuntimeAuthorityRequest>>,
        duplicate_reservation: bool,
        refusal: Option<String>,
    }

    impl CiJobRuntimeAuthorityProvider for RecordingAuthority {
        fn materialize<'a>(
            &'a self,
            request: CiJobRuntimeAuthorityRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<CiJobRuntimeAuthority, CiLaunchAuthorityError>>
                    + Send
                    + 'a,
            >,
        > {
            self.requests.lock().unwrap().push(request.clone());
            if let Some(detail) = self.refusal.clone() {
                return Box::pin(async move { Err(refused(&detail)) });
            }
            let reserve_handle = if self.duplicate_reservation {
                "reserve:duplicate".into()
            } else {
                format!("reserve:{}", request.job_id)
            };
            Box::pin(async move {
                Ok(CiJobRuntimeAuthority {
                    reserve_handle,
                    token_authority_handle: format!("token-authority:{}", request.job_id),
                })
            })
        }
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
        let runtime = Arc::new(RecordingAuthority::default());
        let policy = LinuxSmallV1LaunchAuthority::new(runtime.clone());
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
        }
        let requests = runtime.requests.lock().unwrap();
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
    async fn duplicate_job_reservations_are_refused_before_manifest_creation() {
        let (record, prepared) = fixture();
        let runtime = Arc::new(RecordingAuthority {
            requests: Mutex::new(Vec::new()),
            duplicate_reservation: true,
            refusal: None,
        });
        let policy = LinuxSmallV1LaunchAuthority::new(runtime);
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
        let runtime = Arc::new(RecordingAuthority::default());
        let policy = LinuxSmallV1LaunchAuthority::new(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        record.tenant_id = "other".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        record.tenant_id = "acme".into();
        record.state = "running".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        assert!(runtime.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_locked_scope_is_refused_before_external_calls() {
        let (mut record, prepared) = fixture();
        let runtime = Arc::new(RecordingAuthority::default());
        let policy = LinuxSmallV1LaunchAuthority::new(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        record.project_id = "not-a-uuid".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        record.project_id = "20000000-0000-0000-0000-000000000001".into();
        record.trigger_kind = "customer-defined".into();
        assert!(policy.materialize(&record, &prepared, &pin).await.is_err());
        assert!(runtime.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_authority_refusal_is_propagated_without_a_partial_manifest() {
        let (record, prepared) = fixture();
        let runtime = Arc::new(RecordingAuthority {
            requests: Mutex::new(Vec::new()),
            duplicate_reservation: false,
            refusal: Some("budget unavailable".into()),
        });
        let policy = LinuxSmallV1LaunchAuthority::new(runtime.clone());
        let pin = CiWorkflowDefinitionPin::new(1, "ci-body-v1").unwrap();

        let error = policy
            .materialize(&record, &prepared, &pin)
            .await
            .unwrap_err();
        assert_eq!(error.0, "budget unavailable");
        assert_eq!(runtime.requests.lock().unwrap().len(), 1);
    }
}
