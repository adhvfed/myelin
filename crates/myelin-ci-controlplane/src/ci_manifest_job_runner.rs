use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_ci_sandbox::{
    EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind as SandboxJobKind, JobSpec,
    JobSpecTemplate as SandboxJobSpecTemplate, MeterTarget, ResourceLimits,
    RunTokenAuthorizationContext, RunTokenCredential, SecretRef, TrustTier as SandboxTrustTier,
    WorkspaceSpec,
};
use myelin_flow::{
    ActivityError, JobKind, JobRunner, JobSpec as FlowJobSpec, PgFlowWorker, PgResolvedDriveInput,
    PgWorkerError,
};
use myelin_identity::{
    AuthzError, Consistency, DataRole, IdentityService, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use crate::ci_drive_manifest::{
    CiDriveManifestV1, CiManifestLaneV1, CiManifestTrustTierV1, GrantedCiJobV1,
};
use crate::ci_manifest_pipeline::{
    decode_resolved_ci_manifest, run_ci_manifest_pipeline, CiManifestInputResolver,
};
use crate::ci_run_store::CiRunFinalizer;
use crate::job_queue_store::DurableEnqueue;
use crate::job_spec_store::{CiJobSpecStore, DurableCiJobLaunchTemplate};
use crate::scheduler::Lane;
use crate::CI_PIPELINE_WF_TYPE;
use crate::{
    SecretBroker, SecretCapability, SecretLaunchError, WithheldSecret, WithholdReason,
};

pub type CiJobSecretResolver =
    Arc<dyn Fn(&TenantId, JobSpec) -> Result<JobSpec, SecretLaunchError> + Send + Sync>;

pub(crate) fn secret_withhold_machine_reason(withheld: &[WithheldSecret]) -> String {
    let mut reason = String::from("secret_withheld:");
    for (index, item) in withheld.iter().enumerate() {
        if index != 0 {
            reason.push(',');
        }
        reason.push_str(&item.name);
        reason.push('=');
        reason.push_str(item.reason.as_token());
    }
    reason
}

pub fn unavailable_ci_job_secret_resolver() -> CiJobSecretResolver {
    Arc::new(|_tenant, spec| {
        if spec.secret_refs.is_empty() {
            return spec
                .with_resolved_secrets(Vec::new())
                .map_err(SecretLaunchError::Injection);
        }
        let reason = if spec.trust_tier == SandboxTrustTier::UntrustedFork {
            WithholdReason::UntrustedFork
        } else {
            WithholdReason::CapabilityUnavailable
        };
        Err(SecretLaunchError::Withheld(
            spec.secret_refs
                .iter()
                .map(|secret| WithheldSecret {
                    name: secret.name.clone(),
                    reason,
                })
                .collect(),
        ))
    })
}

pub(crate) fn claim_secret_subject(
    tenant: &TenantId,
    context: &myelin_ci_sandbox::CiJobAuthorizationContext,
) -> Result<Principal, SecretLaunchError> {
    if context.tenant_id != tenant.as_str() {
        return Err(SecretLaunchError::Authorization(AuthzError::FailClosed(
            "CI secret resolution tenant does not match the authorized claim".into(),
        )));
    }
    if sqlx::types::Uuid::parse_str(&context.project_id).is_err()
        || sqlx::types::Uuid::parse_str(&context.job_id).is_err()
    {
        return Err(SecretLaunchError::Authorization(AuthzError::FailClosed(
            "CI secret resolution requires canonical project and job identities".into(),
        )));
    }
    Ok(Principal::new(
        tenant.clone(),
        Region::new(context.region.clone()),
        PrincipalId(format!(
            "svc:ci:project:{}:job:{}",
            context.project_id, context.job_id
        )),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    ))
}

pub fn secret_broker_ci_job_resolver<C, I>(
    capability: Arc<C>,
    identity: Arc<I>,
    consistency: Consistency,
) -> CiJobSecretResolver
where
    C: SecretCapability + Send + Sync + 'static,
    I: IdentityService + Send + Sync + 'static,
{
    Arc::new(move |tenant, spec| {
        let context = match spec.run_token_authorization.as_ref() {
            Some(RunTokenAuthorizationContext::CiJob(context)) => context.clone(),
            None => {
                return Err(SecretLaunchError::Authorization(AuthzError::FailClosed(
                    "CI secret resolution requires a claim authorization context".into(),
                )));
            }
        };
        let subject = claim_secret_subject(tenant, &context)?;
        SecretBroker::new(capability.as_ref(), identity.as_ref()).resolve_for_launch(
            spec,
            &subject,
            |secret| ArtifactRef(secret.handle.clone()),
            &consistency,
        )
    })
}

pub(crate) fn resolve_claim_launch_secrets(
    tenant: &TenantId,
    template: SandboxJobSpecTemplate,
    run_token: RunTokenCredential,
    authorization: RunTokenAuthorizationContext,
    secrets: &CiJobSecretResolver,
) -> Result<JobSpec, SecretLaunchError> {
    secrets(
        tenant,
        template.resolve_with_authorization(run_token, Some(authorization)),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobTokenRequest {
    pub tenant_id: String,
    pub region: String,
    pub project_id: String,
    pub wf_run_id: String,
    pub ci_run_id: String,
    pub job_id: String,
    pub token_authority_handle: String,
    pub idem_token: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobTokenIssueError(pub String);

pub const MAX_CI_JOB_TOKEN_TTL_SECS: u64 = 300;

impl CiJobTokenRequest {
    pub fn validate(&self) -> Result<(), CiJobTokenIssueError> {
        for (name, value) in [
            ("project", self.project_id.as_str()),
            ("workflow run", self.wf_run_id.as_str()),
            ("CI run", self.ci_run_id.as_str()),
            ("job", self.job_id.as_str()),
            ("claim nonce", self.claim_nonce.as_str()),
        ] {
            sqlx::types::Uuid::parse_str(value).map_err(|_| {
                CiJobTokenIssueError(format!("{name} identity is not a canonical UUID"))
            })?;
        }
        if self.tenant_id.trim().is_empty()
            || self.region.trim().is_empty()
            || self.token_authority_handle.trim().is_empty()
            || self.idem_token.trim().is_empty()
            || self.lease_owner.trim().is_empty()
            || self.lease_epoch <= 0
            || self.claim_started_at_epoch_secs <= 0
            || self.claim_expires_at_epoch_secs <= self.claim_started_at_epoch_secs
        {
            return Err(CiJobTokenIssueError(
                "claim-bound token request has invalid scope or lifetime".into(),
            ));
        }
        let claim_lifetime =
            u64::try_from(self.claim_expires_at_epoch_secs - self.claim_started_at_epoch_secs)
                .map_err(|_| {
                    CiJobTokenIssueError("claim lifetime is outside the supported range".into())
                })?;
        if claim_lifetime > crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS {
            return Err(CiJobTokenIssueError(
                "claim lifetime exceeds the production claim-window bound".into(),
            ));
        }
        Ok(())
    }
}

impl std::fmt::Display for CiJobTokenIssueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CI job token mint refused: {}", self.0)
    }
}

impl std::error::Error for CiJobTokenIssueError {}

pub trait CiJobTokenIssuer: Send + Sync {
    fn mint(
        &self,
        request: CiJobTokenRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + '_>>;
}

pub struct CiManifestDurableJobRunner {
    manifest: Arc<CiDriveManifestV1>,
    store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
}

impl CiManifestDurableJobRunner {
    pub fn new(
        manifest: Arc<CiDriveManifestV1>,
        store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, String> {
        manifest.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            manifest,
            store,
            rt,
        })
    }

    fn manifest_job<'a>(&'a self, flow: &FlowJobSpec) -> Result<&'a GrantedCiJobV1, ActivityError> {
        if flow.kind != JobKind::Ci {
            return Err(ActivityError(
                "manifest CI runner refused a non-CI job".into(),
            ));
        }
        if !flow
            .idem_token
            .strip_prefix(&self.manifest.wf_run_id)
            .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(ActivityError(
                "manifest CI runner refused an idempotency token from another workflow run".into(),
            ));
        }
        self.manifest
            .jobs
            .iter()
            .find(|job| job.job_id == flow.target)
            .ok_or_else(|| {
                ActivityError(
                    "manifest CI runner refused a dispatch target absent from the immutable manifest"
                        .into(),
                )
            })
    }
}

impl JobRunner for CiManifestDurableJobRunner {
    fn dispatch(&self, flow: &FlowJobSpec) -> Result<(), ActivityError> {
        let job = self.manifest_job(flow)?;
        let (enqueue, spec) = manifest_dispatch_parts(&self.manifest, job, flow)?;
        bridge(
            &self.rt,
            self.store
                .co_persist_active_flow_dispatch(&enqueue, &spec, &job.name),
        )
        .map_err(|error| ActivityError(format!("durable manifest dispatch refused: {error}")))?;
        Ok(())
    }
}

pub fn register_durable_ci_manifest_pipeline(
    worker: &mut PgFlowWorker,
    resolver: CiManifestInputResolver,
    store: CiJobSpecStore,
    finalizer: Arc<dyn CiRunFinalizer>,
    rt: tokio::runtime::Handle,
) -> Result<(), PgWorkerError> {
    let version = resolver.definition().version();
    let code_hash = resolver.definition().code_hash().to_owned();
    worker.register_definition_with_input_resolver(
        CI_PIPELINE_WF_TYPE,
        version,
        &code_hash,
        resolver,
        move |input: &PgResolvedDriveInput, ctx| {
            let manifest = decode_resolved_ci_manifest(input)?;
            let runner = CiManifestDurableJobRunner::new(
                Arc::new(manifest.clone()),
                store.clone(),
                rt.clone(),
            )?;
            run_ci_manifest_pipeline(ctx, &manifest, &runner, finalizer.as_ref())
                .map_err(|error| format!("manifest CI workflow failed: {error:?}"))?;
            Ok(vec![myelin_refs::ArtifactRef(manifest.run_ref)])
        },
    )
}

fn bridge<F: Future>(rt: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

fn manifest_dispatch_parts(
    manifest: &CiDriveManifestV1,
    job: &GrantedCiJobV1,
    flow: &FlowJobSpec,
) -> Result<(DurableEnqueue, DurableCiJobLaunchTemplate), ActivityError> {
    let trust_tier = sandbox_trust(manifest.trust_tier);
    let spec = SandboxJobSpecTemplate::new(
        SandboxJobKind::Ci,
        ImageRef::pinned(job.image.clone()).map_err(|error| ActivityError(error.to_string()))?,
        job.command.clone(),
        job.env
            .iter()
            .map(|(name, value)| EnvVar {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        job.secret_handles
            .iter()
            .map(|(name, handle)| SecretRef {
                name: name.clone(),
                handle: handle.clone(),
            })
            .collect(),
        EgressPolicy {
            allow: job.egress_allow.clone(),
        },
        ResourceLimits {
            cpu_millis: job.limits.cpu_millis,
            mem_bytes: job.limits.mem_bytes,
            disk_bytes: job.limits.disk_bytes,
            tmpfs_bytes: job.limits.disk_bytes,
            pids_max: job.limits.pids_max,
            timeout_secs: job.limits.timeout_secs,
        },
        WorkspaceSpec {
            repo_ref: Some(job.workspace.repo_ref.clone()),
            commit: Some(job.workspace.commit_oid.clone()),
        },
        trust_tier,
        MeterTarget {
            reserve_id: job.reserve_handle.clone(),
        },
        IdemToken(flow.idem_token.clone()),
    )
    .map_err(|error| ActivityError(error.to_string()))?;
    let claim_window_secs = crate::ci_claim_window::claim_window_secs_for_template(&spec)
        .map_err(|error| ActivityError(error.to_string()))?;
    let enqueue = DurableEnqueue {
        tenant_id: manifest.tenant_id.clone(),
        region: manifest.region.clone(),
        job_id: job.job_id.clone(),
        run_id: manifest.wf_run_id.clone(),
        lane: manifest_lane(job.scheduling.lane),
        labels: job.scheduling.labels.clone(),
        trust_tier,
        concurrency_group: job.scheduling.concurrency_group.clone(),
        fair_key: job.scheduling.fair_key.clone(),
        idem_token: flow.idem_token.clone(),
        stage: job.name.clone(),
        claim_window_secs,
        reservation_write_version: crate::ReservationWriteVersionMarker::derive_from_reserve_handle(
            &job.reserve_handle,
        ),
    };
    Ok((
        enqueue,
        DurableCiJobLaunchTemplate {
            spec,
            project_id: manifest.project_id.clone(),
            ci_run_id: manifest.ci_run_id.clone(),
            token_authority_handle: job.token_authority_handle.clone(),
        },
    ))
}

fn sandbox_trust(trust: CiManifestTrustTierV1) -> SandboxTrustTier {
    match trust {
        CiManifestTrustTierV1::Trusted => SandboxTrustTier::Trusted,
        CiManifestTrustTierV1::UntrustedFork => SandboxTrustTier::UntrustedFork,
        CiManifestTrustTierV1::SelfHosted => SandboxTrustTier::SelfHosted,
    }
}

fn manifest_lane(lane: CiManifestLaneV1) -> Lane {
    match lane {
        CiManifestLaneV1::Interactive => Lane::Interactive,
        CiManifestLaneV1::Batch => Lane::Batch,
        CiManifestLaneV1::Deploy => Lane::Deploy,
    }
}

pub(crate) fn validate_run_token(
    token: &RunTokenCredential,
    token_authority_handle: &str,
) -> Result<(), ActivityError> {
    if token.jti == token_authority_handle || token.ttl_secs() > MAX_CI_JOB_TOKEN_TTL_SECS {
        return Err(ActivityError(
            "CI token issuer returned an overlong-lived credential or copied the authority handle"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_template() -> SandboxJobSpecTemplate {
        SandboxJobSpecTemplate::new(
            SandboxJobKind::Ci,
            ImageRef::pinned(format!("registry.example/job@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["/bin/true".into()],
            Vec::new(),
            vec![SecretRef {
                name: "DEPLOY_KEY".into(),
                handle: "myelin://acme/ci/secret/deploy".into(),
            }],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 64 * 1024 * 1024,
                pids_max: 64,
                timeout_secs: 30,
            },
            WorkspaceSpec::default(),
            SandboxTrustTier::Trusted,
            MeterTarget {
                reserve_id: "reserve:secret-test".into(),
            },
            IdemToken("idem:secret-test".into()),
        )
        .unwrap()
    }

    fn authorization() -> RunTokenAuthorizationContext {
        RunTokenAuthorizationContext::CiJob(myelin_ci_sandbox::CiJobAuthorizationContext {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            principal_id: "ci-job".into(),
            project_id: "11111111-1111-4111-8111-111111111111".into(),
            wf_run_id: "wf".into(),
            job_id: "job".into(),
            lease_owner: "runner".into(),
            lease_epoch: 1,
            claim_nonce: "nonce".into(),
            claim_started_at_epoch_secs: 1,
            claim_expires_at_epoch_secs: 2,
            reserve_id: "reserve:secret-test".into(),
            required_capabilities: Vec::new(),
            checkout_scope: None,
            credential_binding: None,
        })
    }

    #[test]
    fn maps_every_manifest_lane_and_trust_tier_exactly() {
        assert_eq!(
            manifest_lane(CiManifestLaneV1::Interactive),
            Lane::Interactive
        );
        assert_eq!(manifest_lane(CiManifestLaneV1::Batch), Lane::Batch);
        assert_eq!(manifest_lane(CiManifestLaneV1::Deploy), Lane::Deploy);
        assert_eq!(
            sandbox_trust(CiManifestTrustTierV1::Trusted),
            SandboxTrustTier::Trusted
        );
        assert_eq!(
            sandbox_trust(CiManifestTrustTierV1::UntrustedFork),
            SandboxTrustTier::UntrustedFork
        );
        assert_eq!(
            sandbox_trust(CiManifestTrustTierV1::SelfHosted),
            SandboxTrustTier::SelfHosted
        );
    }

    #[test]
    fn token_credential_must_be_short_lived_and_distinct_from_authority_handle() {
        assert!(validate_run_token(
            &RunTokenCredential::new("bearer", "jti:1", MAX_CI_JOB_TOKEN_TTL_SECS).unwrap(),
            "mint:1"
        )
        .is_ok());
        let copied = RunTokenCredential::new("bearer", "mint:1", 1).unwrap();
        assert!(validate_run_token(&copied, "mint:1").is_err());
        let overlong =
            RunTokenCredential::new("bearer", "jti:overlong", MAX_CI_JOB_TOKEN_TTL_SECS + 1)
                .unwrap();
        assert!(validate_run_token(&overlong, "mint:1").is_err());
    }

    #[test]
    fn token_request_binds_one_bounded_durable_claim_generation() {
        let request = CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: "10000000-0000-0000-0000-000000000001".into(),
            ci_run_id: "20000000-0000-0000-0000-000000000001".into(),
            job_id: "30000000-0000-0000-0000-000000000001".into(),
            token_authority_handle: "authority:job".into(),
            idem_token: "idem:job".into(),
            lease_owner: "runner:1".into(),
            lease_epoch: 1,
            claim_nonce: "40000000-0000-0000-0000-000000000001".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        };
        request.validate().unwrap();

        let mut longest_legal = request.clone();
        longest_legal.claim_expires_at_epoch_secs = longest_legal.claim_started_at_epoch_secs
            + crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS as i64;
        longest_legal.validate().unwrap();
        let mut checkout_length = request.clone();
        checkout_length.claim_expires_at_epoch_secs = checkout_length.claim_started_at_epoch_secs
            + crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS
            + 1;
        checkout_length.validate().unwrap();

        let mut overlong = request.clone();
        overlong.claim_expires_at_epoch_secs = overlong.claim_started_at_epoch_secs
            + crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS as i64
            + 1;
        assert!(overlong.validate().is_err());
        let mut malformed = request;
        malformed.claim_nonce = "not-a-uuid".into();
        assert!(malformed.validate().is_err());
    }

    #[test]
    fn the_token_ttl_ceiling_is_independent_of_the_claim_window() {
        assert_eq!(MAX_CI_JOB_TOKEN_TTL_SECS, 300);
        assert!(
            (MAX_CI_JOB_TOKEN_TTL_SECS as i64)
                < crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS
        );
    }

    #[test]
    fn unavailable_secret_stack_admits_non_secret_and_terminally_withholds_secret_job() {
        let resolver: CiJobSecretResolver = Arc::new(|tenant, spec| {
            assert_eq!(tenant.0, "acme");
            spec.with_resolved_secrets(vec![myelin_ci_sandbox::ResolvedSecretEnv::new(
                "DEPLOY_KEY",
                "production-material",
            )])
            .map_err(SecretLaunchError::Injection)
        });
        let injected = resolve_claim_launch_secrets(
            &TenantId::from_token("acme"),
            secret_template(),
            RunTokenCredential::new("bearer", "jti", 60).unwrap(),
            authorization(),
            &resolver,
        )
        .unwrap();
        assert_eq!(injected.resolved_secret_count(), 1);
        assert!(injected.validate_secret_coverage().is_ok());

        let mut non_secret_template = secret_template();
        non_secret_template.secret_refs.clear();
        let non_secret = resolve_claim_launch_secrets(
            &TenantId::from_token("acme"),
            non_secret_template,
            RunTokenCredential::new("bearer", "jti", 60).unwrap(),
            authorization(),
            &crate::ci_runner_composition::optional_secret_resolver(None),
        )
        .expect("an unavailable secret stack must not block a non-secret job");
        assert_eq!(non_secret.resolved_secret_count(), 0);
        assert!(non_secret.validate_secret_coverage().is_ok());

        let withheld = resolve_claim_launch_secrets(
            &TenantId::from_token("acme"),
            secret_template(),
            RunTokenCredential::new("bearer", "jti", 60).unwrap(),
            authorization(),
            &crate::ci_runner_composition::optional_secret_resolver(None),
        )
        .unwrap_err();
        assert_eq!(
            withheld.to_string(),
            "secret launch withheld: DEPLOY_KEY=capability_unavailable"
        );
        let SecretLaunchError::Withheld(withheld) = withheld else {
            panic!("unavailable capability must produce a typed withhold");
        };
        assert_eq!(
            secret_withhold_machine_reason(&withheld),
            "secret_withheld:DEPLOY_KEY=capability_unavailable"
        );
    }
}
