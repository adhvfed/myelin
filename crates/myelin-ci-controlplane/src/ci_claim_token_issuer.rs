//! Claim-time verification for CI job credentials.
//!
//! A queued launch template carries only a public, content-bound authority handle. The executable
//! bearer is minted later, under one exact scheduler claim. This module makes that mint conditional
//! on durable truth: it locks the scheduler claim and run-of-record, reloads the immutable manifest
//! in the same tenant-scoped transaction, reconstructs the complete authority request, verifies the
//! handle, and only then invokes the raw Identity credential minter while both locks remain held.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_ci_sandbox::{derive_checkout_authorization_scope, JobKind, RunTokenCredential};
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;

use crate::ci_drive_manifest::{
    CiDriveManifestStore, CiDriveManifestV1, CiManifestTrustTierV1, CiManifestWorkspaceV1,
};
use crate::ci_launch_authority::{CiJobRuntimeAuthorityRequest, ManifestBoundCiJobTokenAuthority};
use crate::ci_manifest_job_runner::{
    CiJobTokenIssueError, CiJobTokenIssuer, CiJobTokenRequest, MAX_CI_JOB_TOKEN_TTL_SECS,
};
use crate::ci_run_store::{CiRunRecord, CiRunStore};
use crate::job_queue_store::{CiJobQueueStore, LockedJobClaim};
use crate::job_spec_store::{CiJobSpecStore, DurableCiJobLaunchTemplate};
use crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;

/// The narrow Identity-facing mint seam. Implementations receive only server-reconstructed durable
/// authority plus the exact live claim. They must be exact-retry stable while the deterministic
/// token generation is live, refuse that same claim after the generation expires, use the claim's
/// absolute expiry as a ceiling, and report a lifetime no greater than either the complete claim
/// lifetime or [`MAX_CI_JOB_TOKEN_TTL_SECS`].
pub trait CiJobCredentialMinter: Send + Sync {
    fn mint_verified<'a>(
        &'a self,
        claim: CiJobTokenRequest,
        authority: CiJobRuntimeAuthorityRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>;
}

/// Production claim-time issuer. Its raw minter is unreachable until the durable scheduler claim,
/// run, and manifest have been locked, cross-checked, and reduced to the exact authority digest
/// stored on the job.
#[derive(Clone)]
pub struct LockedManifestCiJobTokenIssuer {
    pool: PgPool,
    region: String,
    credential_minter: Arc<dyn CiJobCredentialMinter>,
}

impl LockedManifestCiJobTokenIssuer {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        credential_minter: Arc<dyn CiJobCredentialMinter>,
    ) -> Self {
        Self {
            pool,
            region: region.into(),
            credential_minter,
        }
    }
}

impl CiJobTokenIssuer for LockedManifestCiJobTokenIssuer {
    fn mint(
        &self,
        request: CiJobTokenRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + '_>>
    {
        Box::pin(async move {
            request.validate()?;
            if request.region != self.region {
                return Err(refused(
                    "scheduler claim region differs from the runner cell",
                ));
            }
            let tenant = request.tenant_id.clone();
            let region = request.region.clone();
            let manifest_store = CiDriveManifestStore::new(
                self.pool.clone(),
                TenantId(tenant.clone()),
                Region(region.clone()),
            )
            .map_err(|_| refused("claim scope is invalid"))?;
            let job_spec_store = CiJobSpecStore::with_pg(self.pool.clone());
            let credential_minter = self.credential_minter.clone();

            with_tenant_tx_error(&self.pool, &tenant, &region, move |connection| {
                Box::pin(async move {
                    // Global lock order for this boundary: scheduler row, then run-of-record. The
                    // reporter/reaper own only the first and finalization owns only the second.
                    let locked_claim = CiJobQueueStore::lock_for_token_mint_on_conn(
                        connection,
                        &request.tenant_id,
                        &request.region,
                        &request.job_id,
                        &request.wf_run_id,
                    )
                    .await
                    .map_err(|_| refused("durable scheduler claim is unavailable"))?
                    .ok_or_else(|| refused("durable scheduler claim is absent"))?;
                    verify_locked_claim(&request, &locked_claim)?;
                    let run = CiRunStore::lock_for_token_mint_on_conn(
                        connection,
                        &request.tenant_id,
                        &request.region,
                        &request.ci_run_id,
                        &request.wf_run_id,
                    )
                    .await
                    .map_err(|_| refused("durable CI run authority is unavailable"))?
                    .ok_or_else(|| refused("durable CI run authority is absent"))?;
                    let (manifest, _) = manifest_store
                        .load_by_identity_on_conn(
                            connection,
                            &request.wf_run_id,
                            &request.ci_run_id,
                        )
                        .await
                        .map_err(|_| refused("immutable CI manifest authority is unavailable"))?
                        .ok_or_else(|| refused("immutable CI manifest authority is absent"))?;
                    // CT-007 slice 5b.3-2b: load the durable `ci_job_spec` row in this SAME locked
                    // tx and cross-check its `workspace` against what the manifest granted this job
                    // — closing the gap where nothing previously verified that the `JobSpec` a
                    // launch will actually execute agrees with what was authorized.
                    let launch_template = job_spec_store
                        .get_launch_template_on_conn(
                            connection,
                            &request.tenant_id,
                            &request.job_id,
                        )
                        .await
                        .map_err(|_| {
                            refused("durable ci_job_spec launch template is unavailable")
                        })?;
                    let authority =
                        authority_from_durable_claim(&request, &run, &manifest, &launch_template)?;
                    if locked_claim.stage.as_deref() != Some(authority.stage.as_str())
                        || locked_claim.trust_tier != authority.trust_tier
                    {
                        return Err(refused(
                            "scheduler claim authority differs from the immutable manifest",
                        ));
                    }
                    // CT-007 lease/topology reconciliation, defense in depth: the durable spec
                    // resolver already refused a checkout-bearing legacy row before this mint was
                    // requested, but the credential boundary re-derives the window from the durable
                    // spec under the SAME row lock rather than trusting that earlier check.
                    verify_claim_window(&request, &locked_claim, &launch_template)?;
                    let credential = credential_minter
                        .mint_verified(request.clone(), authority)
                        .await?;
                    validate_minted_credential(&request, &credential)?;
                    Ok(credential)
                })
            })
            .await
        })
    }
}

pub(crate) fn verify_locked_claim(
    request: &CiJobTokenRequest,
    locked: &LockedJobClaim,
) -> Result<(), CiJobTokenIssueError> {
    if !matches!(locked.state.as_str(), "leased" | "running")
        || locked.idem_token != request.idem_token
        || locked.lease_owner.as_deref() != Some(request.lease_owner.as_str())
        || locked.lease_epoch != request.lease_epoch
        || locked.claim_nonce.as_deref() != Some(request.claim_nonce.as_str())
        || locked.claim_started_at_epoch_secs != Some(request.claim_started_at_epoch_secs)
        || locked.claim_expires_at_epoch_secs != Some(request.claim_expires_at_epoch_secs)
        || !locked.claim_is_live
    {
        return Err(refused(
            "presented scheduler claim is stale, expired, or divergent",
        ));
    }
    Ok(())
}

/// **CT-007 lease/topology reconciliation: the claim window's anti-forgery cross-check.** The
/// immutable `claim_expires_at - claim_started_at` difference is what every downstream authority
/// binds; this proves that difference really was sized from the durable window, and that the durable
/// window really is what the dispatched spec's own topology derives. The two directions matter
/// separately: the first catches a queue row whose timestamps were written under a different window
/// than the one it now carries; the second catches a queue row whose window disagrees with the spec
/// that will actually execute.
///
/// `claim_window_secs = NULL` is the legacy generation, claimed under the flat execution-lease
/// fallback:
/// - non-checkout → accepted only if the difference is exactly
///   [`CI_RUNNER_EXECUTION_LEASE_TTL_SECS`], i.e. the row really was claimed under that fallback;
/// - checkout-bearing → REFUSED outright. A four-execution topology under a one-execution ceiling
///   would have its claim expire mid-preparation, and no credential may be minted for it.
pub(crate) fn verify_claim_window(
    request: &CiJobTokenRequest,
    locked: &LockedJobClaim,
    launch_template: &DurableCiJobLaunchTemplate,
) -> Result<(), CiJobTokenIssueError> {
    let observed = request
        .claim_expires_at_epoch_secs
        .checked_sub(request.claim_started_at_epoch_secs)
        .ok_or_else(|| refused("claim lifetime is outside the supported range"))?;
    let derived = crate::ci_claim_window::claim_window_secs_for_template(&launch_template.spec)
        .map_err(|_| refused("durable launch template has no derivable claim window"))?;
    match locked.claim_window_secs {
        Some(window) => {
            if observed != window || window != derived {
                return Err(refused(
                    "durable claim window disagrees with the claim generation or the dispatched spec",
                ));
            }
        }
        None => {
            let checkout_bearing = crate::ci_claim_window::is_checkout_bearing(
                launch_template.spec.kind,
                &launch_template.spec.workspace,
            )
            .map_err(|_| refused("durable launch template has no derivable checkout intent"))?;
            if checkout_bearing {
                return Err(refused(
                    "checkout-bearing job carries no durable claim window (a legacy pre-expand \
                     dispatch); its claim would expire mid-preparation",
                ));
            }
            if observed != CI_RUNNER_EXECUTION_LEASE_TTL_SECS {
                return Err(refused(
                    "legacy null-window claim was not sized from the flat execution-lease TTL",
                ));
            }
        }
    }
    Ok(())
}

impl From<PgError> for CiJobTokenIssueError {
    fn from(_error: PgError) -> Self {
        refused("durable claim-time token transaction failed")
    }
}

pub(crate) fn authority_from_durable_claim(
    claim: &CiJobTokenRequest,
    run: &CiRunRecord,
    manifest: &CiDriveManifestV1,
    launch_template: &DurableCiJobLaunchTemplate,
) -> Result<CiJobRuntimeAuthorityRequest, CiJobTokenIssueError> {
    let authorities = runtime_authorities_from_durable_claim(claim, run, manifest)?;
    let authority = authorities
        .into_iter()
        .find(|authority| authority.job_id == claim.job_id)
        .ok_or_else(|| refused("claimed job is absent from the immutable CI manifest"))?;
    let job = manifest
        .jobs
        .iter()
        .find(|job| job.job_id == claim.job_id)
        .ok_or_else(|| refused("claimed job is absent from the immutable CI manifest"))?;
    if job.token_authority_handle != claim.token_authority_handle {
        return Err(refused("claimed token authority differs from the manifest"));
    }
    // CT-007 slice 5b.3-2b, Sol's review: the durable `ci_job_spec` row carries its OWN
    // authority-identity fields (`ci_run_id`, `token_authority_handle`) alongside the dispatched
    // `spec` -- verify BOTH against the same run/manifest/claim identity this function already
    // locked, not just the spec's workspace. A row whose wrapper identity diverged from its own
    // spec (or from the run/claim/manifest already verified above) is refused before it can ever
    // reach the credential minter.
    if launch_template.ci_run_id != run.run_id
        || launch_template.token_authority_handle != job.token_authority_handle
        || launch_template.spec.idem_token.0 != claim.idem_token
    {
        return Err(refused(
            "durable ci_job_spec launch template identity differs from the locked claim/run/manifest",
        ));
    }
    verify_dispatched_spec_matches_granted_workspace(&launch_template.spec, &job.workspace)?;
    if !ManifestBoundCiJobTokenAuthority::verifies(&authority, &claim.token_authority_handle) {
        return Err(refused(
            "manifest-bound CI token authority verification failed",
        ));
    }
    Ok(authority)
}

/// Reconstruct every immutable runtime-authority request in the exact canonical manifest order.
///
/// Both claim-time credential minting and the prelaunch-usage journal's v2 reservation verifier use
/// this one function. The latter must recompute the COMPLETE reservation batch because the durable
/// v2 batch digest binds every job, not only the currently claimed one.
pub(crate) fn runtime_authorities_from_durable_claim(
    claim: &CiJobTokenRequest,
    run: &CiRunRecord,
    manifest: &CiDriveManifestV1,
) -> Result<Vec<CiJobRuntimeAuthorityRequest>, CiJobTokenIssueError> {
    manifest
        .validate()
        .map_err(|_| refused("immutable CI manifest is invalid"))?;
    if run.state != "running" {
        return Err(refused("CI run is not live for token minting"));
    }
    if run.tenant_id != claim.tenant_id
        || run.region != claim.region
        || run.run_id != claim.ci_run_id
        || run.wf_run_id != claim.wf_run_id
        || manifest.tenant_id != run.tenant_id
        || manifest.region != run.region
        || manifest.ci_run_id != run.run_id
        || manifest.wf_run_id != run.wf_run_id
        || manifest.repo_ref != run.repo_ref.as_deref().unwrap_or_default()
        || manifest.commit_oid != run.commit_oid.as_deref().unwrap_or_default()
        || manifest_trust_token(manifest.trust_tier) != run.trust_tier
    {
        return Err(refused("durable CI run and manifest authority diverged"));
    }
    let snapshot = myelin_refs::parse_scoped(&manifest.source_snapshot_ref)
        .map_err(|_| refused("manifest source snapshot authority is invalid"))?;
    let source_snapshot_digest = snapshot
        .id
        .strip_prefix("snapshot-")
        .ok_or_else(|| refused("manifest source snapshot authority is invalid"))?
        .to_owned();
    let mut authorities = Vec::with_capacity(manifest.jobs.len());
    for job in &manifest.jobs {
        let checkout = checkout_scope_for_manifest_job(&job.workspace)?;
        verify_checkout_scope_tenant(&checkout, &run.tenant_id)?;
        authorities.push(CiJobRuntimeAuthorityRequest {
            tenant_id: run.tenant_id.clone(),
            region: run.region.clone(),
            ci_run_id: run.run_id.clone(),
            wf_run_id: run.wf_run_id.clone(),
            project_id: run.project_id.clone(),
            job_id: job.job_id.clone(),
            stage: job.stage.clone(),
            concrete_name: job.name.clone(),
            trigger_kind: run.trigger_kind.clone(),
            trust_tier: run.trust_tier.clone(),
            source_snapshot_digest: source_snapshot_digest.clone(),
            workflow_definition_version: manifest.workflow_definition_version,
            workflow_code_hash: manifest.workflow_code_hash.clone(),
            policy_revision: manifest.authority_policy_revision.clone(),
            limits: job.limits.clone(),
            checkout,
        });
    }
    Ok(authorities)
}

/// CT-007 slice 5b.3-2b: derive the checkout scope the manifest GRANTED this job, via the sandbox's
/// own [`derive_checkout_authorization_scope`] facade only (never hand-parsed here) — the exact
/// same derivation `checkout_scope_for_run` used at materialize time, so a claim-time digest
/// recomputation can only ever agree or disagree with the materialize-time one for a genuine reason
/// (a real divergence in the underlying repo_ref/commit_oid), never because the two sites parse
/// differently.
fn checkout_scope_for_manifest_job(
    granted_workspace: &CiManifestWorkspaceV1,
) -> Result<Option<myelin_ci_sandbox::CheckoutAuthorizationScope>, CiJobTokenIssueError> {
    let workspace = myelin_ci_sandbox::WorkspaceSpec {
        repo_ref: Some(granted_workspace.repo_ref.clone()),
        commit: Some(granted_workspace.commit_oid.clone()),
    };
    derive_checkout_authorization_scope(JobKind::Ci, &workspace)
        .map_err(|_| refused("manifest-granted checkout target is invalid"))
}

/// CT-007 slice 5b.3-2b: refuse a checkout scope whose repo-ref ArtifactRef encodes a DIFFERENT
/// tenant than the durable `ci_run`'s own tenant. This is DEFENSE IN DEPTH, not an independently
/// reachable authorization gap (Sol's correction of an earlier, inaccurate doc claim here):
/// `CiDriveManifestV1::validate`'s `validate_canonical_ref("repo_ref", ..., &self.tenant_id, "git",
/// "repo")` already requires `manifest.repo_ref`'s OWN embedded tenant to equal `manifest.tenant_id`,
/// and `authority_from_durable_claim` separately requires `manifest.tenant_id == run.tenant_id` — so
/// for any manifest that already passes `validate()`, this branch can never actually fire; a
/// cross-tenant repo_ref is refused earlier, by `validate()` itself. Kept anyway as an explicit,
/// directly-testable second check on the exact property this slice cares about (never trusting the
/// checkout scope's tenant without comparison), so a future change that weakens or removes the
/// `validate()` guarantee does not silently reopen this gap unnoticed.
fn verify_checkout_scope_tenant(
    checkout: &Option<myelin_ci_sandbox::CheckoutAuthorizationScope>,
    run_tenant_id: &str,
) -> Result<(), CiJobTokenIssueError> {
    if let Some(scope) = checkout {
        if scope.tenant().0 != run_tenant_id {
            return Err(refused(
                "checkout scope tenant differs from the durable CI run tenant",
            ));
        }
    }
    Ok(())
}

/// CT-007 slice 5b.3-2b: verify the durable `ci_job_spec` row's `workspace` — what a launch will
/// ACTUALLY execute — is byte-identical to what the manifest GRANTED this job. Before this slice,
/// nothing checked this; `ci_manifest_job_runner.rs` copies the manifest's `workspace` verbatim when
/// it persists the dispatch, so this should always hold in the happy path, but a claim-time check
/// closes the gap for any future write path (bug or otherwise) that could let the two diverge.
fn verify_dispatched_spec_matches_granted_workspace(
    dispatched_spec: &myelin_ci_sandbox::JobSpecTemplate,
    granted_workspace: &CiManifestWorkspaceV1,
) -> Result<(), CiJobTokenIssueError> {
    if dispatched_spec.kind != JobKind::Ci
        || dispatched_spec.workspace.repo_ref.as_deref()
            != Some(granted_workspace.repo_ref.as_str())
        || dispatched_spec.workspace.commit.as_deref()
            != Some(granted_workspace.commit_oid.as_str())
    {
        return Err(refused(
            "dispatched ci_job_spec workspace differs from the manifest-granted workspace",
        ));
    }
    Ok(())
}

fn validate_minted_credential(
    request: &CiJobTokenRequest,
    credential: &RunTokenCredential,
) -> Result<(), CiJobTokenIssueError> {
    let claim_lifetime =
        u64::try_from(request.claim_expires_at_epoch_secs - request.claim_started_at_epoch_secs)
            .map_err(|_| refused("claim lifetime is outside the supported range"))?;
    if credential.jti == request.token_authority_handle
        || credential.ttl_secs() > MAX_CI_JOB_TOKEN_TTL_SECS
        || credential.ttl_secs() > claim_lifetime
    {
        return Err(refused(
            "Identity returned a copied authority handle or claim-overlong credential",
        ));
    }
    Ok(())
}

fn manifest_trust_token(trust: CiManifestTrustTierV1) -> &'static str {
    match trust {
        CiManifestTrustTierV1::Trusted => "trusted",
        CiManifestTrustTierV1::UntrustedFork => "untrusted_fork",
        CiManifestTrustTierV1::SelfHosted => "self_hosted",
    }
}

fn refused(detail: &str) -> CiJobTokenIssueError {
    CiJobTokenIssueError(detail.into())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::ci_drive_manifest::{
        CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestWorkspaceV1,
        GrantedCiJobV1, CI_DRIVE_MANIFEST_SCHEMA_V1,
    };
    use crate::{ci_artifact_ref, ci_run_ref, CI_PIPELINE_WF_TYPE, RUN_PLAN_SCHEMA_V2};

    const WF_RUN_ID: &str = "11111111-1111-1111-1111-111111111111";
    const CI_RUN_ID: &str = "22222222-2222-2222-2222-222222222222";
    const PROJECT_ID: &str = "33333333-3333-3333-3333-333333333333";
    const PIPELINE_ID: &str = "44444444-4444-4444-4444-444444444444";
    const JOB_ID: &str = "55555555-5555-5555-5555-555555555555";

    /// The fixed checkout target `run()`/`manifest_and_claim()` always grant.
    fn default_granted_workspace() -> CiManifestWorkspaceV1 {
        let run = run();
        CiManifestWorkspaceV1 {
            repo_ref: run.repo_ref.unwrap(),
            commit_oid: run.commit_oid.unwrap(),
            read_only_root: true,
            tmpfs_scratch: true,
        }
    }

    /// The durable `ci_job_spec` row's complete wrapper — spec plus the two authority-identity
    /// fields (`ci_run_id`, `token_authority_handle`) `authority_from_durable_claim` now ALSO
    /// verifies (Sol's review) — for a `claim` whose `token_authority_handle`/`ci_run_id` this
    /// template must agree with to pass.
    fn launch_template_for(
        claim: &CiJobTokenRequest,
        granted_workspace: &CiManifestWorkspaceV1,
    ) -> DurableCiJobLaunchTemplate {
        DurableCiJobLaunchTemplate {
            spec: dispatched_spec_for(granted_workspace),
            ci_run_id: claim.ci_run_id.clone(),
            token_authority_handle: claim.token_authority_handle.clone(),
        }
    }

    fn default_launch_template(claim: &CiJobTokenRequest) -> DurableCiJobLaunchTemplate {
        launch_template_for(claim, &default_granted_workspace())
    }

    /// A minimal, valid [`myelin_ci_sandbox::JobSpecTemplate`] whose `workspace` matches
    /// `granted_workspace` exactly — the durable `ci_job_spec` row's dispatched spec, standing in
    /// for what `ci_manifest_job_runner.rs` actually persists at dispatch time.
    fn dispatched_spec_for(
        granted_workspace: &CiManifestWorkspaceV1,
    ) -> myelin_ci_sandbox::JobSpecTemplate {
        myelin_ci_sandbox::JobSpecTemplate {
            kind: JobKind::Ci,
            image: myelin_ci_sandbox::ImageRef {
                reference: format!("registry.example/runner@sha256:{}", "c".repeat(64)),
            },
            command: vec!["true".into()],
            env: Vec::new(),
            secret_refs: Vec::new(),
            egress: myelin_ci_sandbox::EgressPolicy::default(),
            limits: myelin_ci_sandbox::ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 600,
            },
            workspace: myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some(granted_workspace.repo_ref.clone()),
                commit: Some(granted_workspace.commit_oid.clone()),
            },
            trust_tier: myelin_ci_sandbox::TrustTier::Trusted,
            meter_to: myelin_ci_sandbox::MeterTarget {
                reserve_id: "reserve:test".into(),
            },
            idem_token: myelin_ci_sandbox::IdemToken("idem:test".into()),
        }
    }

    fn limits() -> CiManifestLimitsV1 {
        CiManifestLimitsV1 {
            cpu_millis: 1_000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            pids_max: 128,
            timeout_secs: 600,
        }
    }

    fn run() -> CiRunRecord {
        CiRunRecord {
            tenant_id: "acme".into(),
            run_id: CI_RUN_ID.into(),
            region: "fr-par".into(),
            project_id: PROJECT_ID.into(),
            pipeline_id: PIPELINE_ID.into(),
            wf_run_id: WF_RUN_ID.into(),
            repo_ref: Some("myelin://acme/git/repo/myelin".into()),
            commit_oid: Some("0123456789abcdef0123456789abcdef01234567".into()),
            cause_event_id: None,
            cause_depth: 0,
            caused_by: None,
            definition_snapshot: "snapshot".into(),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "running".into(),
            correlation_id: "correlation".into(),
        }
    }

    fn manifest_and_claim() -> (CiDriveManifestV1, CiJobTokenRequest) {
        let run = run();
        let snapshot_digest = format!("blake3:{}", "a".repeat(64));
        let granted_workspace = CiManifestWorkspaceV1 {
            repo_ref: run.repo_ref.clone().unwrap(),
            commit_oid: run.commit_oid.clone().unwrap(),
            read_only_root: true,
            tmpfs_scratch: true,
        };
        let checkout = checkout_scope_for_manifest_job(&granted_workspace).unwrap();
        let authority = CiJobRuntimeAuthorityRequest {
            tenant_id: run.tenant_id.clone(),
            region: run.region.clone(),
            ci_run_id: run.run_id.clone(),
            wf_run_id: run.wf_run_id.clone(),
            project_id: run.project_id.clone(),
            job_id: JOB_ID.into(),
            stage: "test".into(),
            concrete_name: "test".into(),
            trigger_kind: run.trigger_kind.clone(),
            trust_tier: run.trust_tier.clone(),
            source_snapshot_digest: snapshot_digest.clone(),
            workflow_definition_version: 7,
            workflow_code_hash: format!("blake3:{}", "d".repeat(64)),
            policy_revision: "linux-small-v1:1".into(),
            limits: limits(),
            checkout,
        };
        let token_authority_handle = ManifestBoundCiJobTokenAuthority::handle_for(&authority);
        let manifest = CiDriveManifestV1 {
            schema_version: CI_DRIVE_MANIFEST_SCHEMA_V1,
            tenant_id: run.tenant_id.clone(),
            region: run.region.clone(),
            wf_run_id: run.wf_run_id.clone(),
            ci_run_id: run.run_id.clone(),
            source_snapshot_ref: ci_artifact_ref(
                &run.tenant_id,
                &format!("snapshot-{snapshot_digest}"),
            )
            .0,
            source_plan_schema_version: RUN_PLAN_SCHEMA_V2,
            launch_request_digest: format!("blake3:{}", "b".repeat(64)),
            workflow_type: CI_PIPELINE_WF_TYPE.into(),
            workflow_definition_version: authority.workflow_definition_version,
            workflow_code_hash: authority.workflow_code_hash,
            authority_policy_revision: authority.policy_revision,
            repo_ref: run.repo_ref.clone().unwrap(),
            commit_oid: run.commit_oid.clone().unwrap(),
            run_ref: ci_run_ref(&run.tenant_id, &run.run_id).0,
            started_at: "2026-07-22T12:00:00.000000Z".into(),
            trust_tier: CiManifestTrustTierV1::Trusted,
            check_attempts: BTreeMap::from([("test".into(), 1)]),
            merge_waiter: None,
            jobs: vec![GrantedCiJobV1 {
                job_id: JOB_ID.into(),
                stage: "test".into(),
                name: "test".into(),
                check_context: "test".into(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: format!("registry.example/runner@sha256:{}", "c".repeat(64)),
                command: vec!["true".into()],
                env: BTreeMap::new(),
                secret_handles: BTreeMap::new(),
                egress_allow: Vec::new(),
                limits: limits(),
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: granted_workspace.repo_ref.clone(),
                    commit_oid: granted_workspace.commit_oid.clone(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: CiManifestSchedulingV1 {
                    lane: CiManifestLaneV1::Batch,
                    labels: vec!["linux".into(), "linux-small-v1".into()],
                    concurrency_group: None,
                    fair_key: format!("project:{}", run.project_id),
                },
                reserve_handle: "reserve:test".into(),
                token_authority_handle: token_authority_handle.clone(),
                continue_on_error: false,
            }],
        };
        let claim = CiJobTokenRequest {
            tenant_id: run.tenant_id,
            region: run.region,
            wf_run_id: run.wf_run_id,
            ci_run_id: run.run_id,
            job_id: JOB_ID.into(),
            token_authority_handle,
            idem_token: "idem:test".into(),
            lease_owner: "runner:test".into(),
            lease_epoch: 1,
            claim_nonce: "66666666-6666-6666-6666-666666666666".into(),
            claim_started_at_epoch_secs: 1_785_000_000,
            claim_expires_at_epoch_secs: 1_785_000_030,
        };
        (manifest, claim)
    }

    fn locked_claim(claim: &CiJobTokenRequest) -> LockedJobClaim {
        LockedJobClaim {
            state: "leased".into(),
            idem_token: claim.idem_token.clone(),
            stage: Some("test".into()),
            trust_tier: "trusted".into(),
            lease_owner: Some(claim.lease_owner.clone()),
            lease_epoch: claim.lease_epoch,
            claim_nonce: Some(claim.claim_nonce.clone()),
            claim_started_at_epoch_secs: Some(claim.claim_started_at_epoch_secs),
            claim_expires_at_epoch_secs: Some(claim.claim_expires_at_epoch_secs),
            claim_window_secs: Some(
                claim.claim_expires_at_epoch_secs - claim.claim_started_at_epoch_secs,
            ),
            claim_is_live: true,
        }
    }

    #[test]
    fn reconstructs_and_verifies_the_complete_manifest_bound_authority() {
        let run = run();
        let (manifest, claim) = manifest_and_claim();
        let authority =
            authority_from_durable_claim(&claim, &run, &manifest, &default_launch_template(&claim))
                .unwrap();
        assert_eq!(authority.project_id, PROJECT_ID);
        assert_eq!(
            authority.source_snapshot_digest,
            format!("blake3:{}", "a".repeat(64))
        );
        assert!(ManifestBoundCiJobTokenAuthority::verifies(
            &authority,
            &claim.token_authority_handle
        ));
    }

    #[test]
    fn refuses_a_handle_that_does_not_match_the_reloaded_manifest() {
        let run = run();
        let (manifest, mut claim) = manifest_and_claim();
        claim.token_authority_handle = format!("ci-token-authority:v1:{}", "0".repeat(64));
        assert!(authority_from_durable_claim(
            &claim,
            &run,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());
    }

    #[test]
    fn refuses_when_any_reconstructed_authority_family_changes() {
        let (manifest, claim) = manifest_and_claim();

        let mut changed_run = run();
        changed_run.project_id = "88888888-8888-8888-8888-888888888888".into();
        assert!(authority_from_durable_claim(
            &claim,
            &changed_run,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());

        let mut changed_trigger = run();
        changed_trigger.trigger_kind = "manual".into();
        assert!(authority_from_durable_claim(
            &claim,
            &changed_trigger,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());

        let mut changed_workflow = manifest.clone();
        changed_workflow.workflow_code_hash = format!("blake3:{}", "e".repeat(64));
        assert!(authority_from_durable_claim(
            &claim,
            &run(),
            &changed_workflow,
            &default_launch_template(&claim)
        )
        .is_err());

        let mut changed_limits = manifest;
        changed_limits.jobs[0].limits.cpu_millis += 1;
        assert!(authority_from_durable_claim(
            &claim,
            &run(),
            &changed_limits,
            &default_launch_template(&claim)
        )
        .is_err());
    }

    #[test]
    fn refuses_terminal_or_cross_trust_run_state() {
        let (manifest, claim) = manifest_and_claim();
        let mut terminal = run();
        terminal.state = "succeeded".into();
        assert!(authority_from_durable_claim(
            &claim,
            &terminal,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());

        let mut divergent = run();
        divergent.trust_tier = "untrusted_fork".into();
        assert!(authority_from_durable_claim(
            &claim,
            &divergent,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());
    }

    #[test]
    fn refuses_a_claim_for_a_job_absent_from_the_manifest() {
        let run = run();
        let (manifest, mut claim) = manifest_and_claim();
        claim.job_id = "77777777-7777-7777-7777-777777777777".into();
        assert!(authority_from_durable_claim(
            &claim,
            &run,
            &manifest,
            &default_launch_template(&claim)
        )
        .is_err());
    }

    #[test]
    fn scheduler_claim_verification_binds_every_generation_fact_and_liveness() {
        let (_, claim) = manifest_and_claim();
        assert!(verify_locked_claim(&claim, &locked_claim(&claim)).is_ok());

        let mutations: [fn(&mut LockedJobClaim); 8] = [
            |locked| locked.state = "queued".into(),
            |locked| locked.idem_token = "other-idem".into(),
            |locked| locked.lease_owner = Some("other-runner".into()),
            |locked| locked.lease_epoch += 1,
            |locked| locked.claim_nonce = Some("99999999-9999-9999-9999-999999999999".into()),
            |locked| locked.claim_started_at_epoch_secs = Some(1_785_000_001),
            |locked| locked.claim_expires_at_epoch_secs = Some(1_785_000_031),
            |locked| locked.claim_is_live = false,
        ];
        for mutate in mutations {
            let mut divergent = locked_claim(&claim);
            mutate(&mut divergent);
            assert!(verify_locked_claim(&claim, &divergent).is_err());
        }
    }

    // ---------------------------------------------------------------------------------------
    // CT-007 lease/topology reconciliation: the claim-window cross-check at the mint boundary.
    // ---------------------------------------------------------------------------------------

    /// A claim/locked pair whose generation really was sized from `window`.
    fn claim_of_window(window: i64) -> (CiJobTokenRequest, LockedJobClaim) {
        let (_, mut claim) = manifest_and_claim();
        claim.claim_expires_at_epoch_secs = claim.claim_started_at_epoch_secs + window;
        let mut locked = locked_claim(&claim);
        locked.claim_window_secs = Some(window);
        (claim, locked)
    }

    /// The checkout-bearing template every fixture here dispatches: timeout 600s, four execution
    /// slots → 4,800s.
    const FIXTURE_CHECKOUT_WINDOW_SECS: i64 = 4 * (600 + 600);

    fn compute_launch_template(claim: &CiJobTokenRequest) -> DurableCiJobLaunchTemplate {
        let mut template = default_launch_template(claim);
        template.spec.workspace = myelin_ci_sandbox::WorkspaceSpec::default();
        template
    }

    #[test]
    fn a_populated_window_must_match_both_the_generation_and_the_dispatched_spec() {
        let (claim, locked) = claim_of_window(FIXTURE_CHECKOUT_WINDOW_SECS);
        let template = default_launch_template(&claim);
        verify_claim_window(&claim, &locked, &template).unwrap();

        // The generation's timestamps say something different from the stored window.
        let (_, mut divergent_generation) = claim_of_window(FIXTURE_CHECKOUT_WINDOW_SECS);
        divergent_generation.claim_window_secs = Some(FIXTURE_CHECKOUT_WINDOW_SECS - 1);
        assert!(verify_claim_window(&claim, &divergent_generation, &template).is_err());

        // The stored window says something different from what the dispatched spec derives.
        let (short_claim, short_locked) = claim_of_window(30);
        assert!(verify_claim_window(
            &short_claim,
            &short_locked,
            &default_launch_template(&short_claim)
        )
        .is_err());
    }

    #[test]
    fn a_legacy_null_window_is_accepted_only_for_a_flat_window_non_checkout_generation() {
        let (claim, mut locked) = claim_of_window(CI_RUNNER_EXECUTION_LEASE_TTL_SECS);
        locked.claim_window_secs = None;
        verify_claim_window(&claim, &locked, &compute_launch_template(&claim)).unwrap();

        // A null-window row whose generation was NOT sized from the flat TTL is not the legacy
        // shape this fallback exists for.
        let (short_claim, mut short_locked) = claim_of_window(30);
        short_locked.claim_window_secs = None;
        assert!(verify_claim_window(
            &short_claim,
            &short_locked,
            &compute_launch_template(&short_claim)
        )
        .is_err());
    }

    #[test]
    fn a_legacy_null_window_is_refused_outright_for_a_checkout_bearing_spec() {
        let (claim, mut locked) = claim_of_window(CI_RUNNER_EXECUTION_LEASE_TTL_SECS);
        locked.claim_window_secs = None;
        let checkout = default_launch_template(&claim);
        assert!(crate::ci_claim_window::is_checkout_bearing(
            checkout.spec.kind,
            &checkout.spec.workspace
        )
        .unwrap());
        assert!(
            verify_claim_window(&claim, &locked, &checkout).is_err(),
            "a four-execution topology may never mint under a one-execution claim ceiling"
        );
    }

    #[test]
    fn refuses_copied_authority_jti_and_overlong_identity_credential() {
        let (_, claim) = manifest_and_claim();
        let copied = RunTokenCredential::new("bearer", &claim.token_authority_handle, 30).unwrap();
        assert!(validate_minted_credential(&claim, &copied).is_err());
        let overlong =
            RunTokenCredential::new("bearer", "jti", MAX_CI_JOB_TOKEN_TTL_SECS + 1).unwrap();
        assert!(validate_minted_credential(&claim, &overlong).is_err());
        let claim_overlong = RunTokenCredential::new("bearer", "jti", 31).unwrap();
        assert!(validate_minted_credential(&claim, &claim_overlong).is_err());
    }

    #[test]
    fn lock_query_pins_exact_scope_and_holds_a_write_lock() {
        let run_query = crate::ci_run_store::LOCK_CI_RUN_FOR_TOKEN_MINT_QUERY;
        let run_required: BTreeSet<&str> = [
            "tenant_id = $1",
            "region = $2",
            "run_id = $3::uuid",
            "wf_run_id = $4::uuid",
            "FOR UPDATE",
        ]
        .into_iter()
        .collect();
        for fragment in run_required {
            assert!(
                run_query.contains(fragment),
                "missing run-lock fragment: {fragment}"
            );
        }
        let claim_query = crate::job_queue_store::LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY;
        for fragment in [
            "tenant_id = $1",
            "region = $2",
            "job_id = $3::uuid",
            "run_id = $4::uuid",
            "claim_started_at",
            "claim_expires_at",
            "claim_is_live",
            "FOR UPDATE",
        ] {
            assert!(
                claim_query.contains(fragment),
                "missing claim-lock fragment: {fragment}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // CT-007 slice 5b.3-2b: the checkout-authority blocker-test list Sol required before landing.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn refuses_a_dispatched_spec_whose_repo_was_substituted() {
        let (manifest, claim) = manifest_and_claim();
        let mut substituted = default_granted_workspace();
        substituted.repo_ref = "myelin://acme/git/repo/other".into();
        let launch_template = launch_template_for(&claim, &substituted);
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn refuses_a_dispatched_spec_whose_commit_was_substituted() {
        let (manifest, claim) = manifest_and_claim();
        let mut substituted = default_granted_workspace();
        substituted.commit_oid = "f".repeat(40);
        let launch_template = launch_template_for(&claim, &substituted);
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn verify_checkout_scope_tenant_is_a_pure_defense_in_depth_check() {
        // A pure-function unit test of `verify_checkout_scope_tenant`'s own contract, called
        // directly (bypassing the full manifest/claim chain entirely). Per Sol's correction: this
        // branch is NOT independently reachable through `authority_from_durable_claim` for any
        // manifest that already passes `CiDriveManifestV1::validate` (see that function's doc) --
        // this test exists to pin the helper's own behavior, not to prove a reachable gap.
        let scope = myelin_ci_sandbox::derive_checkout_authorization_scope(
            JobKind::Ci,
            &myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some("myelin://globex/git/repo/core".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap();
        assert!(verify_checkout_scope_tenant(&scope, "acme").is_err());
        assert!(verify_checkout_scope_tenant(&scope, "globex").is_ok());
        assert!(verify_checkout_scope_tenant(&None, "acme").is_ok());
    }

    #[test]
    fn full_chain_refuses_a_cross_tenant_repo_ref_via_manifest_validate() {
        // The full-chain proof Sol asked for: a manifest whose `repo_ref` (and, to keep `job.workspace`
        // consistent with it, the job's own workspace) names a DIFFERENT tenant than `manifest.tenant_id`
        // is refused by `authority_from_durable_claim` -- not via `verify_checkout_scope_tenant` (which
        // never gets a chance to run), but because `CiDriveManifestV1::validate`'s
        // `validate_canonical_ref("repo_ref", ..., &self.tenant_id, "git", "repo")` refuses it first.
        let (mut manifest, claim) = manifest_and_claim();
        manifest.repo_ref = "myelin://globex/git/repo/core".into();
        manifest.jobs[0].workspace.repo_ref = manifest.repo_ref.clone();
        let launch_template = launch_template_for(&claim, &manifest.jobs[0].workspace);
        assert!(manifest.validate().is_err());
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn refuses_a_launch_template_whose_idem_token_diverges() {
        let (manifest, claim) = manifest_and_claim();
        let mut launch_template = default_launch_template(&claim);
        launch_template.spec.idem_token = myelin_ci_sandbox::IdemToken("some-other-idem".into());
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn refuses_a_launch_template_whose_durable_ci_run_id_diverges() {
        let (manifest, claim) = manifest_and_claim();
        let mut launch_template = default_launch_template(&claim);
        launch_template.ci_run_id = "99999999-9999-9999-9999-999999999999".into();
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn refuses_a_launch_template_whose_durable_token_authority_handle_diverges() {
        let (manifest, claim) = manifest_and_claim();
        let mut launch_template = default_launch_template(&claim);
        launch_template.token_authority_handle = "ci-token-authority:v2:tampered".into();
        assert!(authority_from_durable_claim(&claim, &run(), &manifest, &launch_template).is_err());
    }

    #[test]
    fn v1_and_v2_digests_never_collide_for_the_same_request() {
        let (manifest, claim) = manifest_and_claim();
        let authority = authority_from_durable_claim(
            &claim,
            &run(),
            &manifest,
            &default_launch_template(&claim),
        )
        .unwrap();
        let v1 = format!(
            "ci-token-authority:v1:{}",
            crate::ci_launch_authority::token_authority_digest(&authority)
        );
        let v2 = ManifestBoundCiJobTokenAuthority::handle_for(&authority);
        assert!(v2.starts_with("ci-token-authority:v2:"));
        assert_ne!(v1, v2);
    }

    /// A fully literal, fixed authority request — never built from `run()`/`manifest_and_claim()`,
    /// so nothing here can silently drift if those shared fixtures change.
    fn golden_compute_authority() -> CiJobRuntimeAuthorityRequest {
        CiJobRuntimeAuthorityRequest {
            tenant_id: "golden-tenant".into(),
            region: "golden-region".into(),
            ci_run_id: "10101010-1010-1010-1010-101010101010".into(),
            wf_run_id: "20202020-2020-2020-2020-202020202020".into(),
            project_id: "30303030-3030-3030-3030-303030303030".into(),
            job_id: "40404040-4040-4040-4040-404040404040".into(),
            stage: "golden-stage".into(),
            concrete_name: "golden-job".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            source_snapshot_digest: "0".repeat(64),
            workflow_definition_version: 42,
            workflow_code_hash: "1".repeat(64),
            policy_revision: "linux-small-v1:1".into(),
            limits: CiManifestLimitsV1 {
                cpu_millis: 1_000,
                mem_bytes: 268_435_456,
                disk_bytes: 1_073_741_824,
                pids_max: 128,
                timeout_secs: 600,
            },
            checkout: None,
        }
    }

    fn golden_checkout_scope() -> myelin_ci_sandbox::CheckoutAuthorizationScope {
        myelin_ci_sandbox::derive_checkout_authorization_scope(
            JobKind::Ci,
            &myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some("myelin://golden-tenant/git/repo/widgets".into()),
                commit: Some("2".repeat(40)),
            },
        )
        .unwrap()
        .unwrap()
    }

    fn golden_checkout_authority() -> CiJobRuntimeAuthorityRequest {
        CiJobRuntimeAuthorityRequest {
            checkout: Some(golden_checkout_scope()),
            ..golden_compute_authority()
        }
    }

    /// External golden pin (Sol's review): a self-referential test that computes the expected value
    /// via the same function it's testing stays green even if the encoding silently drifts. This
    /// test instead hard-codes the digest's OUTPUT for a fixed, fully literal request as a plain
    /// string literal, captured once and frozen here — any future change to what
    /// `token_authority_digest`/`token_authority_digest_v2` hash (field order, domain, framing) will
    /// change the computed value and fail this assertion, regardless of what the implementation
    /// itself currently believes is correct.
    #[test]
    fn golden_v1_digest_for_a_fixed_compute_request_is_pinned() {
        let digest =
            crate::ci_launch_authority::token_authority_digest(&golden_compute_authority());
        assert_eq!(
            digest.to_string(),
            "711d75a4042e7b755def20feda38c19c896cdaee32c5a7c33fe5253ae50eafb0"
        );
    }

    #[test]
    fn golden_v2_digest_for_a_fixed_compute_request_is_pinned() {
        let handle = ManifestBoundCiJobTokenAuthority::handle_for(&golden_compute_authority());
        assert_eq!(
            handle,
            "ci-token-authority:v2:7075eca6be655f765d28f4acdb51070cd7ac34826fbcc35be7b18214c94829f9"
        );
    }

    #[test]
    fn golden_v2_digest_for_a_fixed_checkout_request_is_pinned() {
        let handle = ManifestBoundCiJobTokenAuthority::handle_for(&golden_checkout_authority());
        assert_eq!(
            handle,
            "ci-token-authority:v2:aba79c69d035437e665480ce1d08e529d5d51dcf87f4204d071bb9207aa14dea"
        );
    }

    #[test]
    fn a_legacy_v1_handle_still_verifies_for_a_compute_only_request() {
        let (manifest, claim) = manifest_and_claim();
        let mut authority = authority_from_durable_claim(
            &claim,
            &run(),
            &manifest,
            &default_launch_template(&claim),
        )
        .unwrap();
        authority.checkout = None;
        let legacy_v1_handle = format!(
            "ci-token-authority:v1:{}",
            crate::ci_launch_authority::token_authority_digest(&authority)
        );
        assert!(ManifestBoundCiJobTokenAuthority::verifies(
            &authority,
            &legacy_v1_handle
        ));
    }

    #[test]
    fn a_legacy_v1_handle_is_refused_outright_for_a_checkout_bearing_request() {
        let (manifest, claim) = manifest_and_claim();
        let authority = authority_from_durable_claim(
            &claim,
            &run(),
            &manifest,
            &default_launch_template(&claim),
        )
        .unwrap();
        assert!(authority.checkout.is_some());
        let legacy_v1_handle = format!(
            "ci-token-authority:v1:{}",
            crate::ci_launch_authority::token_authority_digest(&authority)
        );
        assert!(!ManifestBoundCiJobTokenAuthority::verifies(
            &authority,
            &legacy_v1_handle
        ));
    }
}
