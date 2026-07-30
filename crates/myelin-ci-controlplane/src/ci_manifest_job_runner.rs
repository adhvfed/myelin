//! Exact manifest-job translation into the durable scheduler and sandbox.
//!
//! The workflow dispatch target is the manifest's immutable job UUID. This adapter preserves that
//! UUID through `job_queue` and `ci_job_spec`, persists the stable token-authority handle without an
//! expiring token, and translates every executable/scheduling field from the validated manifest.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_ci_sandbox::{
    EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind as SandboxJobKind,
    JobSpecTemplate as SandboxJobSpecTemplate, MeterTarget, ResourceLimits, RunTokenCredential,
    SecretRef, TrustTier as SandboxTrustTier, WorkspaceSpec,
};
use myelin_flow::{
    ActivityError, JobKind, JobRunner, JobSpec as FlowJobSpec, PgFlowWorker, PgResolvedDriveInput,
    PgWorkerError,
};

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

/// Owned, retry-stable authority request for one exact live scheduler claim. The epoch + nonce are
/// the activity generation: acknowledgement-loss retries reuse it, while a reaper/new claim changes
/// it and may legitimately remint after expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobTokenRequest {
    pub tenant_id: String,
    pub region: String,
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

/// A token authority refusal. The detail must be structural and safe to journal as an activity
/// failure; token material is never returned through this error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobTokenIssueError(pub String);

/// Maximum fail-static window accepted from Identity for a claim-bound CI credential.
pub const MAX_CI_JOB_TOKEN_TTL_SECS: u64 = 300;

impl CiJobTokenRequest {
    pub fn validate(&self) -> Result<(), CiJobTokenIssueError> {
        for (name, value) in [
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
        // CT-007 lease/topology reconciliation: the accepted ceiling is the CLAIM window's maximum
        // (four executions at the job-timeout ceiling), not the one-execution lease TTL — a
        // legitimately long checkout-bearing claim must not be refused here. The credential's own
        // lifetime stays capped at `MAX_CI_JOB_TOKEN_TTL_SECS`; this bounds only the claim generation
        // a credential may be bound to.
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

/// Explicit short-lived token mint. Implementations must be retry-safe for the complete request:
/// repeated calls for the same claim generation resolve the same credential while its deterministic
/// short token window is live and refuse after that window expires. A reaped and newly claimed job
/// carries a new epoch/nonce and may receive a fresh token. There is no permissive default.
pub trait CiJobTokenIssuer: Send + Sync {
    fn mint(
        &self,
        request: CiJobTokenRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + '_>>;
}

/// Per-run durable adapter. The validated manifest is the sole source of executable and scheduling
/// authority; the Flow spec contributes only the engine-minted idempotency token and exact job UUID
/// target.
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

/// Register the strict resolver, manifest-native DAG, and exact durable queue-template writer as one
/// production definition. Token minting belongs to the later live claim/launch boundary.
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
            // NOTE (ResourceLimits split): mirrored from `disk_bytes`, NOT a new independent
            // default, deliberately. Two reasons: (1) this is exactly the value that sized the
            // `/tmp` tmpfs before the split, so mirroring it keeps runtime tmpfs sizing
            // byte-for-byte unchanged; (2) `ci_runner_composition::manifest_matches_template`
            // asserts `template.limits.disk_bytes == job.limits.disk_bytes` as a fail-closed
            // dispatch invariant — giving `disk_bytes` here its own independent
            // ephemeral-workspace default (as the manifest's declared value no longer directly
            // sizes anything at runtime) would NOT break that invariant, since it only compares
            // `disk_bytes`. But there is no manifest-carried tmpfs value to source
            // `tmpfs_bytes` from, so mirroring `job.limits.disk_bytes` is the only
            // behavior-preserving choice available without a manifest schema change (a later
            // step, alongside actually wiring up the disk-backed workspace mount).
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
    // The immutable claim ceiling comes from the SAME template this dispatch persists, so
    // `co_persist_dispatch`'s recomputation can only ever agree (it refuses fail-closed otherwise).
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
    };
    Ok((
        enqueue,
        DurableCiJobLaunchTemplate {
            spec,
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

        // A legitimately long checkout-bearing claim is ACCEPTED up to the claim-window maximum, and
        // refused one second past it. The one-execution lease TTL is no longer the bound here.
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

    /// **The credential ceiling is unchanged by the longer claim window.** A 300-second token
    /// remains the credential bound regardless of how long the generation it is bound to lives.
    #[test]
    fn the_token_ttl_ceiling_is_independent_of_the_claim_window() {
        assert_eq!(MAX_CI_JOB_TOKEN_TTL_SECS, 300);
        assert!(
            (MAX_CI_JOB_TOKEN_TTL_SECS as i64)
                < crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS
        );
    }
}
