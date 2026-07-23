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

use myelin_ci_sandbox::RunTokenCredential;
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;

use crate::ci_drive_manifest::{CiDriveManifestStore, CiDriveManifestV1, CiManifestTrustTierV1};
use crate::ci_launch_authority::{CiJobRuntimeAuthorityRequest, ManifestBoundCiJobTokenAuthority};
use crate::ci_manifest_job_runner::{
    CiJobTokenIssueError, CiJobTokenIssuer, CiJobTokenRequest, MAX_CI_JOB_TOKEN_TTL_SECS,
};
use crate::ci_run_store::{CiRunRecord, CiRunStore};
use crate::job_queue_store::{CiJobQueueStore, LockedJobClaim};

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
                    let authority = authority_from_durable_claim(&request, &run, &manifest)?;
                    if locked_claim.stage.as_deref() != Some(authority.stage.as_str())
                        || locked_claim.trust_tier != authority.trust_tier
                    {
                        return Err(refused(
                            "scheduler claim authority differs from the immutable manifest",
                        ));
                    }
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

fn verify_locked_claim(
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

impl From<PgError> for CiJobTokenIssueError {
    fn from(_error: PgError) -> Self {
        refused("durable claim-time token transaction failed")
    }
}

fn authority_from_durable_claim(
    claim: &CiJobTokenRequest,
    run: &CiRunRecord,
    manifest: &CiDriveManifestV1,
) -> Result<CiJobRuntimeAuthorityRequest, CiJobTokenIssueError> {
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
    let job = manifest
        .jobs
        .iter()
        .find(|job| job.job_id == claim.job_id)
        .ok_or_else(|| refused("claimed job is absent from the immutable CI manifest"))?;
    if job.token_authority_handle != claim.token_authority_handle {
        return Err(refused("claimed token authority differs from the manifest"));
    }
    let snapshot = myelin_refs::parse_scoped(&manifest.source_snapshot_ref)
        .map_err(|_| refused("manifest source snapshot authority is invalid"))?;
    let source_snapshot_digest = snapshot
        .id
        .strip_prefix("snapshot-")
        .ok_or_else(|| refused("manifest source snapshot authority is invalid"))?
        .to_owned();
    let authority = CiJobRuntimeAuthorityRequest {
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
        source_snapshot_digest,
        workflow_definition_version: manifest.workflow_definition_version,
        workflow_code_hash: manifest.workflow_code_hash.clone(),
        policy_revision: manifest.authority_policy_revision.clone(),
        limits: job.limits.clone(),
    };
    if !ManifestBoundCiJobTokenAuthority::verifies(&authority, &claim.token_authority_handle) {
        return Err(refused(
            "manifest-bound CI token authority verification failed",
        ));
    }
    Ok(authority)
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
            commit_oid: Some("0123456789abcdef".into()),
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
                    repo_ref: run.repo_ref.clone().unwrap(),
                    commit_oid: run.commit_oid.clone().unwrap(),
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
            claim_is_live: true,
        }
    }

    #[test]
    fn reconstructs_and_verifies_the_complete_manifest_bound_authority() {
        let run = run();
        let (manifest, claim) = manifest_and_claim();
        let authority = authority_from_durable_claim(&claim, &run, &manifest).unwrap();
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
        assert!(authority_from_durable_claim(&claim, &run, &manifest).is_err());
    }

    #[test]
    fn refuses_when_any_reconstructed_authority_family_changes() {
        let (manifest, claim) = manifest_and_claim();

        let mut changed_run = run();
        changed_run.project_id = "88888888-8888-8888-8888-888888888888".into();
        assert!(authority_from_durable_claim(&claim, &changed_run, &manifest).is_err());

        let mut changed_trigger = run();
        changed_trigger.trigger_kind = "manual".into();
        assert!(authority_from_durable_claim(&claim, &changed_trigger, &manifest).is_err());

        let mut changed_workflow = manifest.clone();
        changed_workflow.workflow_code_hash = format!("blake3:{}", "e".repeat(64));
        assert!(authority_from_durable_claim(&claim, &run(), &changed_workflow).is_err());

        let mut changed_limits = manifest;
        changed_limits.jobs[0].limits.cpu_millis += 1;
        assert!(authority_from_durable_claim(&claim, &run(), &changed_limits).is_err());
    }

    #[test]
    fn refuses_terminal_or_cross_trust_run_state() {
        let (manifest, claim) = manifest_and_claim();
        let mut terminal = run();
        terminal.state = "succeeded".into();
        assert!(authority_from_durable_claim(&claim, &terminal, &manifest).is_err());

        let mut divergent = run();
        divergent.trust_tier = "untrusted_fork".into();
        assert!(authority_from_durable_claim(&claim, &divergent, &manifest).is_err());
    }

    #[test]
    fn refuses_a_claim_for_a_job_absent_from_the_manifest() {
        let run = run();
        let (manifest, mut claim) = manifest_and_claim();
        claim.job_id = "77777777-7777-7777-7777-777777777777".into();
        assert!(authority_from_durable_claim(&claim, &run, &manifest).is_err());
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
}
