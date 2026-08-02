//! Identity-backed CI claim credential minting and final pre-launch authorization.
//!
//! The durable claim wrapper proves that a mint request names one exact live scheduler generation.
//! This module completes the other half of that boundary: it mints a real signed CI credential with
//! an absolute expiry no later than that generation, then re-verifies the signed facts and durable
//! S7 liveness immediately before the sandbox backend starts untrusted code.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, AttributeHook, CheckoutAuthorizationScope,
    CiJobAuthorizationContext, CiJobCredentialBinding, HookError, JobKind, JobSpec,
    LaunchFenceHook, LaunchOwnership, LaunchPermit, RunTokenAuthorizationContext,
    RunTokenCredential, ValidatedLaunchOwnership,
};
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, RunId, RunToken,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{Authority, DelegationInput, MachineKind, RunTokenMinter};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

use crate::ci_claim_token_issuer::CiJobCredentialMinter;
use crate::ci_credential_generation::CiPhaseGenerationGate;
use crate::ci_credential_generation::{
    phase_generation_id, CiCredentialPurpose, CiPhaseCredentialBinding,
    CiPhaseCredentialMintRequest, CiPhaseCredentialMinter, CiPhaseGenerationInputs,
    CI_PHASE_CREDENTIAL_BINDING_V1,
};
use crate::ci_launch_authority::CiJobRuntimeAuthorityRequest;
use crate::ci_manifest_job_runner::{
    CiJobTokenIssueError, CiJobTokenRequest, MAX_CI_JOB_TOKEN_TTL_SECS,
};
use crate::{CiJobLaunchClaim, CiJobQueueStore};

/// **CT-007 phase-credential generations: which credential SHAPE a boundary requires.** Passed in by
/// the caller, never inferred from the presented context, so the two shapes can never satisfy each
/// other's boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiCredentialExpectation {
    /// The V1 production shape: signed `run_id == job_id`, one credential per claim.
    LegacyClaimBound,
    /// The V2 shape: signed `run_id` == the recomputed generation id for exactly this purpose.
    Phase(CiCredentialPurpose),
}

/// The platform CI service principal cryptographically bound into every hosted-job token.
pub const CI_JOB_PRINCIPAL_ID: &str = "svc:ci";

/// Narrow authority the hosted runner requires at this boundary. Broader Git/check mutation remains
/// behind its own public-endpoint authorization rather than being implied by sandbox launch.
pub const CI_JOB_REQUIRED_CAPABILITIES: [&str; 2] = ["job.launch", "artifact.write"];

/// CT-007 slice 5b.3-2c: the ONE place the exact-repo capability grant is derived, shared by
/// minting, context construction, and verification, so all three can never disagree about what a
/// job's capability vector should be. Every job gets `reserve:<exact id>#consume`; a
/// checkout-bearing job additionally gets
/// `repo:<canonical ArtifactRef>#pull` (Sol's review: the full canonical ref, never a bare
/// `repo_id`, so the grant is unambiguous without relying on capability evaluation to separately
/// incorporate tenant). This capability proves repo-READ authority only -- NOT the exact commit;
/// checkout scope and reserve id are also exact-compared via [`CiJobAuthorizationContext`]. The
/// commit attestation capability is deliberately non-operational: it grants no Git verb, but makes
/// the exact commit part of Identity's signed capability vector for every checkout-bearing phase.
fn reserve_capability(reserve_id: &str) -> String {
    format!("reserve:{reserve_id}#consume")
}

fn checkout_commit_capability(scope: &CheckoutAuthorizationScope) -> String {
    let format = match scope.commit_format() {
        myelin_ci_sandbox::GitObjectFormat::Sha1 => "sha1",
        myelin_ci_sandbox::GitObjectFormat::Sha256 => "sha256",
    };
    format!("checkout-commit:{format}:{}#attest", scope.commit_hex())
}

fn required_ci_capabilities(
    reserve_id: &str,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> Vec<String> {
    let mut capabilities: Vec<String> = CI_JOB_REQUIRED_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect();
    capabilities.push(reserve_capability(reserve_id));
    if let Some(scope) = checkout {
        capabilities.push(format!("repo:{}#pull", scope.repo_ref().0));
        capabilities.push(checkout_commit_capability(scope));
    }
    capabilities
}

/// **CT-007 phase-credential generations: the purpose-attenuated capability vectors.** The ONE place
/// a V2 phase credential's authority is derived, shared by minting, context construction, and
/// verification exactly like [`required_ci_capabilities`] is for V1.
///
/// The attenuation is the point: a preparation credential must not carry `artifact.write`, so an
/// escaped Hop A/B container holding a live preparation bearer cannot write artifacts; and the
/// workload credential must not carry `repo:<ref>#pull`, so a workload bearer cannot re-enter the
/// git wire. Only advertise/fetch (the two git-wire executions) get the repo grant.
pub fn phase_ci_capabilities(
    purpose: CiCredentialPurpose,
    reserve_id: &str,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> Vec<String> {
    let mut capabilities = match purpose {
        CiCredentialPurpose::CheckoutAdvertise | CiCredentialPurpose::CheckoutFetch => {
            let mut capabilities = vec!["job.launch".to_string()];
            if let Some(scope) = checkout {
                capabilities.push(format!("repo:{}#pull", scope.repo_ref().0));
            }
            capabilities
        }
        CiCredentialPurpose::CheckoutMaterialization => vec!["job.launch".to_string()],
        CiCredentialPurpose::Workload => {
            vec!["job.launch".to_string(), "artifact.write".to_string()]
        }
    };
    if let Some(scope) = checkout {
        capabilities.push(checkout_commit_capability(scope));
    }
    capabilities.push(reserve_capability(reserve_id));
    capabilities
}

/// The deterministic JTI Identity will return for one exact generation. Identity derives it from
/// `(principal, run_id, mint_instant)`, and a V2 mint supplies the generation id as the run id and
/// the persisted issuance anchor as the mint instant — so the control plane can compute the expected
/// value BEFORE calling Identity and store it as durable evidence.
pub fn expected_phase_jti(
    generation_id: &str,
    issued_at_epoch_secs: i64,
) -> Result<String, CiJobTokenIssueError> {
    let minted_at = timestamp_from_epoch(issued_at_epoch_secs)?;
    Ok(myelin_identity_service::run_token_jti(
        &PrincipalId(CI_JOB_PRINCIPAL_ID.into()),
        &RunId(generation_id.to_owned()),
        &minted_at,
    ))
}

/// Build the non-secret expected facts carried from a locked scheduler claim to final launch.
/// `checkout` must be the SAME checkout scope (or lack thereof) the claim was actually minted
/// against (CT-007 slice 5b.3-2c) -- see callers for how each derives it.
pub fn ci_job_authorization_context(
    claim: &CiJobTokenRequest,
    reserve_id: &str,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> RunTokenAuthorizationContext {
    RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        principal_id: CI_JOB_PRINCIPAL_ID.into(),
        project_id: claim.project_id.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        reserve_id: reserve_id.to_string(),
        required_capabilities: required_ci_capabilities(reserve_id, checkout),
        checkout_scope: checkout.cloned(),
        credential_binding: None,
    })
}

/// CT-007 phase-credential generations: the V2 analogue of [`ci_job_authorization_context`]. Carries
/// the exact durable generation so the launch boundary can recompute the signed generation id.
pub fn ci_job_phase_authorization_context(
    claim: &CiJobTokenRequest,
    reserve_id: &str,
    checkout: Option<&CheckoutAuthorizationScope>,
    binding: &CiPhaseCredentialBinding,
) -> RunTokenAuthorizationContext {
    RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        principal_id: CI_JOB_PRINCIPAL_ID.into(),
        project_id: claim.project_id.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        reserve_id: reserve_id.to_string(),
        required_capabilities: phase_ci_capabilities(binding.purpose, reserve_id, checkout),
        checkout_scope: checkout.cloned(),
        credential_binding: Some(CiJobCredentialBinding {
            binding_version: binding.binding_version,
            purpose: binding.purpose.token().to_string(),
            generation_id: binding.generation_id.clone(),
            issued_at_epoch_secs: binding.issued_at_epoch_secs,
            expires_at_epoch_secs: binding.expires_at_epoch_secs,
            ci_run_id: claim.ci_run_id.clone(),
            token_authority_handle: claim.token_authority_handle.clone(),
            idem_token: claim.idem_token.clone(),
        }),
    })
}

/// Production Identity adapter for one exact, server-reconstructed scheduler claim.
#[derive(Clone)]
pub struct IdentityCiJobCredentialMinter {
    minter: RunTokenMinter,
    now_epoch_secs: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl IdentityCiJobCredentialMinter {
    pub fn new(minter: RunTokenMinter) -> Self {
        Self {
            minter,
            now_epoch_secs: Arc::new(system_now_epoch_secs),
        }
    }

    /// Inject a deterministic clock for claim-race and acknowledgement-loss proofs.
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now_epoch_secs = Arc::new(now);
        self
    }
}

impl CiJobCredentialMinter for IdentityCiJobCredentialMinter {
    fn mint_verified<'a>(
        &'a self,
        claim: CiJobTokenRequest,
        authority: CiJobRuntimeAuthorityRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>
    {
        Box::pin(async move {
            claim.validate()?;
            validate_claim_authority(&claim, &authority)?;
            let now = (self.now_epoch_secs)();
            let token_expires_at = deterministic_token_expiry(&claim)?;
            token_expires_at
                .checked_sub(now)
                .filter(|remaining| *remaining > 0)
                .ok_or_else(|| refused("claim token generation expired before Identity mint"))?;
            let minter = self.minter.clone();
            let mint_claim = claim.clone();
            let checkout = authority.checkout.clone();
            let reserve_id = authority
                .reserve_id
                .ok_or_else(|| refused("durable CI authority lacks a reservation binding"))?;
            let task = tokio::task::spawn_blocking(move || {
                mint_claim_credential(minter, mint_claim, &reserve_id, checkout.as_ref())
            });
            task.await
                .map_err(|_| refused("Identity mint worker terminated"))?
        })
    }
}

impl CiPhaseCredentialMinter for IdentityCiJobCredentialMinter {
    /// **CT-007 phase-credential generations: the raw V2 Identity seam.** Signs exactly the supplied
    /// generation id as `CredentialPurpose::CiJob.run_id`, anchors the token at exactly the
    /// PERSISTED issuance instant (never a process clock, so an exact retry is byte-stable), and
    /// attenuates authority to exactly this purpose's vector.
    ///
    /// The liveness check is against the persisted expiry, not a recomputed one: a generation whose
    /// window has closed refuses here as well as in the store, so even a direct caller of this seam
    /// cannot resurrect a dead phase.
    fn mint_phase<'a>(
        &'a self,
        request: CiPhaseCredentialMintRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>
    {
        Box::pin(async move {
            request.claim.validate()?;
            validate_phase_mint_request(&request)?;
            let now = (self.now_epoch_secs)();
            request
                .expires_at_epoch_secs
                .checked_sub(now)
                .filter(|remaining| *remaining > 0)
                .ok_or_else(|| {
                    refused("phase credential generation expired before Identity mint")
                })?;
            let minter = self.minter.clone();
            let task = tokio::task::spawn_blocking(move || sign_phase_credential(minter, request));
            task.await
                .map_err(|_| refused("Identity mint worker terminated"))?
        })
    }
}

/// Structural preconditions the raw phase seam enforces itself, so a direct caller (not only the
/// locked store) cannot bind a credential to an unbounded or claim-overlong window.
fn validate_phase_mint_request(
    request: &CiPhaseCredentialMintRequest,
) -> Result<(), CiJobTokenIssueError> {
    let claim = &request.claim;
    if request.issued_at_epoch_secs <= 0
        || request.issued_at_epoch_secs < claim.claim_started_at_epoch_secs
        || request.expires_at_epoch_secs <= request.issued_at_epoch_secs
        || request.expires_at_epoch_secs > claim.claim_expires_at_epoch_secs
        || request.reserve_id.trim().is_empty()
    {
        return Err(refused("phase credential window is outside its claim"));
    }
    let lifetime = u64::try_from(request.expires_at_epoch_secs - request.issued_at_epoch_secs)
        .map_err(|_| refused("phase credential lifetime is outside the supported range"))?;
    if lifetime > MAX_CI_JOB_TOKEN_TTL_SECS {
        return Err(refused(
            "phase credential lifetime exceeds the CI token ceiling",
        ));
    }
    if request.purpose.is_preparation() && request.checkout.is_none() {
        return Err(refused(
            "a preparation credential requires durable checkout authority",
        ));
    }
    // The signed generation id must be the digest of exactly these inputs — a caller cannot supply
    // an arbitrary `run_id` and have Identity sign it.
    let expected = phase_generation_id(CiPhaseGenerationInputs {
        tenant_id: &claim.tenant_id,
        region: &claim.region,
        wf_run_id: &claim.wf_run_id,
        ci_run_id: &claim.ci_run_id,
        job_id: &claim.job_id,
        token_authority_handle: &claim.token_authority_handle,
        idem_token: &claim.idem_token,
        lease_owner: &claim.lease_owner,
        lease_epoch: claim.lease_epoch,
        claim_nonce: &claim.claim_nonce,
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        purpose: request.purpose,
        issued_at_epoch_secs: request.issued_at_epoch_secs,
        expires_at_epoch_secs: request.expires_at_epoch_secs,
        binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
    });
    if expected != request.generation_id {
        return Err(refused(
            "phase credential generation id is not the digest of its own binding",
        ));
    }
    Ok(())
}

/// The blocking Identity signing step. Named distinctly from
/// [`CiJobCredentialGenerationStore::mint_phase_credential`](crate::CiJobCredentialGenerationStore)
/// — that one is the DURABLE mint (insert-or-replay + Identity + exact validation, all inside the
/// locked transaction); this is only the signature.
fn sign_phase_credential(
    minter: RunTokenMinter,
    request: CiPhaseCredentialMintRequest,
) -> Result<RunTokenCredential, CiJobTokenIssueError> {
    let lifetime_secs = u64::try_from(request.expires_at_epoch_secs - request.issued_at_epoch_secs)
        .map_err(|_| refused("phase credential lifetime is outside the supported range"))?;
    let minted_at = timestamp_from_epoch(request.issued_at_epoch_secs)?;
    let principal = ci_principal(&request.claim.tenant_id, &request.claim.region);
    let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
    let required_capabilities = phase_ci_capabilities(
        request.purpose,
        &request.reserve_id,
        request.checkout.as_ref(),
    );
    let authority = Authority::of(required_capabilities.clone());
    let input = DelegationInput {
        agent_policy: authority.clone(),
        delegation: authority.clone(),
        tenant_policy: authority.clone(),
        trigger_actor_held: authority,
    };
    let caveats = DelegationCaveats(required_capabilities);
    let token = minter
        .mint_run_token(
            &scope,
            &principal.principal_id,
            &RunId(request.generation_id),
            &principal,
            &principal,
            &input,
            &caveats,
            MachineKind::Ci,
            &FailStaticBound {
                static_max_secs: lifetime_secs,
            },
            &minted_at,
        )
        .map_err(|_| refused("Identity refused the phase-bound CI credential"))?;
    RunTokenCredential::new(token.token.clone(), token.jti.clone(), lifetime_secs)
        .map_err(|_| refused("Identity returned an invalid CI credential carrier"))
}

fn mint_claim_credential(
    minter: RunTokenMinter,
    claim: CiJobTokenRequest,
    reserve_id: &str,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> Result<RunTokenCredential, CiJobTokenIssueError> {
    let token_expires_at = deterministic_token_expiry(&claim)?;
    let lifetime_secs = u64::try_from(token_expires_at - claim.claim_started_at_epoch_secs)
        .map_err(|_| refused("claim token lifetime is outside the supported range"))?;
    let minted_at = timestamp_from_epoch(claim.claim_started_at_epoch_secs)?;
    let principal = ci_principal(&claim.tenant_id, &claim.region);
    let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
    let required_capabilities = required_ci_capabilities(reserve_id, checkout);
    let authority = Authority::of(required_capabilities.clone());
    let input = DelegationInput {
        agent_policy: authority.clone(),
        delegation: authority.clone(),
        tenant_policy: authority.clone(),
        trigger_actor_held: authority,
    };
    let caveats = DelegationCaveats(required_capabilities);
    let token = minter
        .mint_run_token(
            &scope,
            &principal.principal_id,
            &RunId(claim.job_id),
            &principal,
            &principal,
            &input,
            &caveats,
            MachineKind::Ci,
            &FailStaticBound {
                static_max_secs: lifetime_secs,
            },
            &minted_at,
        )
        .map_err(|_| refused("Identity refused the claim-bound CI credential"))?;
    RunTokenCredential::new(token.token.clone(), token.jti.clone(), lifetime_secs)
        .map_err(|_| refused("Identity returned an invalid CI credential carrier"))
}

fn deterministic_token_expiry(claim: &CiJobTokenRequest) -> Result<i64, CiJobTokenIssueError> {
    let ceiling = i64::try_from(MAX_CI_JOB_TOKEN_TTL_SECS)
        .map_err(|_| refused("CI token ceiling is outside the supported range"))?;
    claim
        .claim_started_at_epoch_secs
        .checked_add(ceiling)
        .map(|token_ceiling| token_ceiling.min(claim.claim_expires_at_epoch_secs))
        .filter(|expiry| *expiry > claim.claim_started_at_epoch_secs)
        .ok_or_else(|| refused("scheduler claim lifetime is outside the supported range"))
}

/// CT-007 slice 5b.3-2c: build the durable-claim lookup key shared by both the CAS-performing
/// [`CiJobLaunchClaimGate::permit`] and the read-only [`CiJobLaunchClaimGate::verify_live`].
fn claim_from_context(context: &CiJobAuthorizationContext) -> CiJobLaunchClaim {
    CiJobLaunchClaim {
        tenant_id: context.tenant_id.clone(),
        region: context.region.clone(),
        wf_run_id: context.wf_run_id.clone(),
        job_id: context.job_id.clone(),
        lease_owner: context.lease_owner.clone(),
        lease_epoch: context.lease_epoch,
        claim_nonce: context.claim_nonce.clone(),
        claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
    }
}

/// CT-007 phase-credential generations: build the durable gate input from server-resolved facts.
fn phase_gate_from_context(
    context: &CiJobAuthorizationContext,
    binding: &CiJobCredentialBinding,
    purpose: CiCredentialPurpose,
) -> CiPhaseGenerationGate {
    CiPhaseGenerationGate {
        tenant_id: context.tenant_id.clone(),
        region: context.region.clone(),
        wf_run_id: context.wf_run_id.clone(),
        ci_run_id: binding.ci_run_id.clone(),
        job_id: context.job_id.clone(),
        token_authority_handle: binding.token_authority_handle.clone(),
        idem_token: binding.idem_token.clone(),
        checkout_commit: context
            .checkout_scope
            .as_ref()
            .map(|scope| scope.commit_hex().to_owned()),
        lease_owner: context.lease_owner.clone(),
        lease_epoch: context.lease_epoch,
        claim_nonce: context.claim_nonce.clone(),
        claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
        purpose,
        binding_version: binding.binding_version,
        generation_id: binding.generation_id.clone(),
        jti: crate::ci_identity_adapter::expected_phase_jti(
            &binding.generation_id,
            binding.issued_at_epoch_secs,
        )
        .unwrap_or_default(),
        issued_at_epoch_secs: binding.issued_at_epoch_secs,
        expires_at_epoch_secs: binding.expires_at_epoch_secs,
    }
}

trait CiJobLaunchClaimGate: Send + Sync {
    fn permit(&self, context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String>;

    /// CT-007 slice 5b.3-2c: confirm the durable claim is still live (`state = 'leased'`, every
    /// generation fact matching) WITHOUT performing the `leased -> running` CAS `permit` commits.
    /// The pre-Hop-A checkout-authorization hook uses this — it must never itself transition the
    /// real workload's state.
    fn verify_live(&self, context: &CiJobAuthorizationContext) -> Result<(), String>;

    /// CT-007 phase-credential generations: a LAZY, retained preparation permit. The durable
    /// predicate re-runs when the permit is committed — at the spawn boundary — never at mint time.
    /// It performs no state transition: there is no CAS for a preparation phase to win, so the
    /// returned ownership is immediate once the durable generation is proven still current.
    fn phase_permit(&self, gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String>;

    /// CT-007 phase-credential generations: the V2 workload permit — the SAME `leased -> running`
    /// CAS, with the current `workload` generation predicate folded into its transaction.
    fn workload_v2_permit(
        &self,
        context: &CiJobAuthorizationContext,
        gate: CiPhaseGenerationGate,
    ) -> Result<LaunchPermit, String>;
}

#[derive(Clone)]
struct PgCiJobLaunchClaimGate {
    store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
}

impl CiJobLaunchClaimGate for PgCiJobLaunchClaimGate {
    fn permit(&self, context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
        let claim = claim_from_context(context);
        let store = self.store.clone();
        let rt = self.rt.clone();
        Ok(LaunchPermit::retained(move || {
            let launch = bridge(&rt, store.authorize_launch_retained(&claim))
                .map_err(|error| HookError(error.to_string()))?
                .ok_or_else(|| {
                    HookError(
                        "durable scheduler claim was canceled, reaped, expired, or already launched"
                            .into(),
                    )
                })?;
            let release_rt = rt.clone();
            Ok(LaunchOwnership::retained(move || {
                let mut launch = launch;
                bridge(&release_rt, launch.validate())
                    .map_err(|error| HookError(error.to_string()))?;
                let release_rt = release_rt.clone();
                Ok(ValidatedLaunchOwnership::retained(move || {
                    bridge(&release_rt, launch.release())
                        .map_err(|error| HookError(error.to_string()))
                }))
            }))
        }))
    }

    fn verify_live(&self, context: &CiJobAuthorizationContext) -> Result<(), String> {
        let claim = claim_from_context(context);
        let store = self.store.clone();
        let rt = self.rt.clone();
        let live =
            bridge(&rt, store.verify_launch_live(&claim)).map_err(|error| error.to_string())?;
        if !live {
            return Err(
                "durable scheduler claim is not live (canceled, reaped, expired, or already \
                 launched)"
                    .into(),
            );
        }
        Ok(())
    }

    fn phase_permit(&self, gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String> {
        let pool = self.store.pool().clone();
        let rt = self.rt.clone();
        Ok(LaunchPermit::retained(move || {
            // Round-1 blocker 1: RETAINED durable ownership, not a read-only probe. The returned
            // handle holds an open transaction carrying a `FOR SHARE` lock on the exact `job_queue`
            // row, so a requeue / successor mint / launch CAS cannot invalidate this generation
            // between the check and the gated child release — it either waits or wins first (in
            // which case acquisition returns `None` and nothing spawns).
            let owned = bridge(
                &rt,
                crate::ci_credential_generation::acquire_phase_generation_ownership(&pool, &gate),
            )
            .map_err(|error| HookError(error.to_string()))?
            .ok_or_else(|| {
                HookError(
                    "the durable phase credential generation is not current, has expired, or its \
                     claim/journal predicate no longer holds"
                        .into(),
                )
            })?;
            let release_rt = rt.clone();
            Ok(LaunchOwnership::retained(move || {
                let mut owned = owned;
                bridge(&release_rt, owned.validate())
                    .map_err(|error| HookError(error.to_string()))?;
                let release_rt = release_rt.clone();
                Ok(ValidatedLaunchOwnership::retained(move || {
                    bridge(&release_rt, owned.release())
                        .map_err(|error| HookError(error.to_string()))
                }))
            }))
        }))
    }

    fn workload_v2_permit(
        &self,
        context: &CiJobAuthorizationContext,
        gate: CiPhaseGenerationGate,
    ) -> Result<LaunchPermit, String> {
        let claim = claim_from_context(context);
        let store = self.store.clone();
        let rt = self.rt.clone();
        Ok(LaunchPermit::retained(move || {
            let launch = bridge(&rt, store.authorize_launch_v2_retained(&claim, &gate))
                .map_err(|error| HookError(error.to_string()))?
                .ok_or_else(|| {
                    HookError(
                        "durable scheduler claim was canceled, reaped, expired, already launched, \
                         or its workload credential generation is not current"
                            .into(),
                    )
                })?;
            let release_rt = rt.clone();
            Ok(LaunchOwnership::retained(move || {
                let mut launch = launch;
                bridge(&release_rt, launch.validate())
                    .map_err(|error| HookError(error.to_string()))?;
                let release_rt = release_rt.clone();
                Ok(ValidatedLaunchOwnership::retained(move || {
                    bridge(&release_rt, launch.release())
                        .map_err(|error| HookError(error.to_string()))
                }))
            }))
        }))
    }
}

fn bridge<F: Future>(rt: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

/// Final backend hook over real Identity verification plus the shared durable S7 lifecycle.
#[derive(Clone)]
pub struct IdentityCiJobLaunchAuthorizer {
    authorizer: RunTokenAuthorizer,
    region: Region,
    claim_gate: Arc<dyn CiJobLaunchClaimGate>,
}

impl IdentityCiJobLaunchAuthorizer {
    pub fn new(
        authorizer: RunTokenAuthorizer,
        store: CiJobQueueStore,
        region: Region,
        rt: tokio::runtime::Handle,
    ) -> Self {
        Self {
            authorizer,
            region,
            claim_gate: Arc::new(PgCiJobLaunchClaimGate { store, rt }),
        }
    }

    #[cfg(test)]
    fn with_claim_gate(
        authorizer: RunTokenAuthorizer,
        region: Region,
        claim_gate: Arc<dyn CiJobLaunchClaimGate>,
    ) -> Self {
        Self {
            authorizer,
            region,
            claim_gate,
        }
    }

    /// Turn this verifier into the runner's guarantee-#2 hook.
    pub fn hook(self: Arc<Self>) -> AttributeHook {
        Box::new(move |spec| self.authorize(spec))
    }

    /// Turn this verifier into a retained production launch fence.
    pub fn launch_fence_hook(self: Arc<Self>) -> LaunchFenceHook {
        Box::new(move |spec| self.authorize_retained(spec))
    }

    /// Reauthorize all signed and durable facts immediately before sandbox launch.
    pub fn authorize(&self, spec: &JobSpec) -> Result<(), HookError> {
        self.authorize_retained(spec)?.commit_and_release()
    }

    /// CT-007 slice 5b.3-2c: the shared, READ-ONLY verification core both `authorize_retained` (the
    /// real workload launch boundary) and `authorize_checkout` (the pre-Hop-A checkout-authorization
    /// hook) call — a genuine re-verification each time, never a cached prior success (Sol's
    /// review). Checks: CI job kind + context shape, the EXACT capability vector (including the
    /// reservation and, for checkout-bearing jobs, canonical repo + exact commit attestation), that
    /// the SIGNED authority is exactly that set (never merely a superset), that the checkout scope
    /// re-derived from the in-hand `spec.workspace` agrees EXACTLY with what the server-resolved
    /// authorization context claims, the cryptographic bearer (JTI/tenant/region/principal/job/
    /// capabilities all bound into Identity's own verification), and that the signed credential's
    /// expiry never outlives the durable claim. Exact signed-authority equality is what prevents a
    /// checkout credential from being shape-downgraded into a compute/None request whose smaller
    /// vector would otherwise pass Identity's contains-every-required-capability check. Does NOT
    /// check durable claim liveness or perform any
    /// state transition -- callers do that themselves (`permit`'s CAS, or `verify_live`'s read-only
    /// check).
    fn verify_ci_job_signed<'a>(
        &self,
        spec: &'a JobSpec,
        expected_purpose: CiCredentialExpectation,
    ) -> Result<&'a CiJobAuthorizationContext, HookError> {
        if spec.kind != JobKind::Ci {
            return Err(HookError(
                "CI Identity launch authorizer received a non-CI job".into(),
            ));
        }
        let Some(RunTokenAuthorizationContext::CiJob(context)) =
            spec.run_token_authorization.as_ref()
        else {
            return Err(HookError(
                "CI launch is missing its claim-resolved authorization context".into(),
            ));
        };
        let rederived_checkout = derive_checkout_authorization_scope(JobKind::Ci, &spec.workspace)
            .map_err(|error| {
                HookError(format!("CI launch workspace intent is invalid: {error}"))
            })?;
        // CT-007 phase-credential generations: the expected credential SHAPE is decided by the
        // caller's boundary, never inferred from whatever the in-hand context happens to carry — so
        // a V2 phase context can never satisfy a legacy boundary, and a legacy context can never
        // satisfy a phase boundary.
        let (required, expected_run_id) =
            match (expected_purpose, context.credential_binding.as_ref()) {
                (CiCredentialExpectation::LegacyClaimBound, None) => (
                    required_ci_capabilities(
                        &spec.meter_to.reserve_id,
                        rederived_checkout.as_ref(),
                    ),
                    context.job_id.clone(),
                ),
                (CiCredentialExpectation::Phase(purpose), Some(binding)) => {
                    let recomputed = self.verify_phase_binding(context, binding, purpose)?;
                    (
                        phase_ci_capabilities(
                            purpose,
                            &spec.meter_to.reserve_id,
                            rederived_checkout.as_ref(),
                        ),
                        recomputed,
                    )
                }
                (CiCredentialExpectation::LegacyClaimBound, Some(_)) => return Err(HookError(
                    "a phase-bound CI credential was presented at a legacy claim-bound boundary"
                        .into(),
                )),
                (CiCredentialExpectation::Phase(_), None) => {
                    return Err(HookError(
                        "a legacy claim-bound CI credential was presented at a phase boundary"
                            .into(),
                    ))
                }
            };
        if context.principal_id != CI_JOB_PRINCIPAL_ID
            || context.required_capabilities != required
            || context.checkout_scope != rederived_checkout
            || context.reserve_id != spec.meter_to.reserve_id
            || context.reserve_id.trim().is_empty()
            || context.tenant_id.trim().is_empty()
            || context.region != self.region.0
            || context.wf_run_id.trim().is_empty()
            || context.job_id.trim().is_empty()
            || context.lease_owner.trim().is_empty()
            || context.lease_epoch <= 0
            || context.claim_nonce.trim().is_empty()
            || context.claim_started_at_epoch_secs <= 0
            || context.claim_expires_at_epoch_secs <= 0
            || context.claim_expires_at_epoch_secs <= context.claim_started_at_epoch_secs
        {
            return Err(HookError(
                "CI launch authorization context diverged from server policy".into(),
            ));
        }
        let principal = ci_principal(&context.tenant_id, &context.region);
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let token = RunToken {
            token: spec.run_token.expose_bearer().to_owned(),
            jti: spec.run_token.jti.clone(),
        };
        let verified = self
            .authorizer
            .authorize_ci_job(
                &scope,
                &principal.principal_id,
                &expected_run_id,
                &token,
                &required,
            )
            .map_err(|error| HookError(format!("Identity refused CI launch: {error}")))?;
        // `RunTokenAuthorizer` intentionally implements general capability authorization (the
        // signed authority may be a superset of `required`). A CI launch credential is narrower:
        // its complete authority shape is itself signed context. Requiring set equality here makes
        // checkout presence/absence and exact commit fail closed independently of the V2 workload
        // CAS backstop.
        if verified.authority != Authority::of(required.iter().cloned()) {
            return Err(HookError(
                "signed CI credential authority differs from the exact launch authority".into(),
            ));
        }
        if verified.exp_unix > context.claim_expires_at_epoch_secs {
            return Err(HookError(
                "signed CI credential outlives its durable scheduler claim".into(),
            ));
        }
        // For V2 the expiry check is EXACT, not merely an upper bound: the cryptographically
        // verified expiry must equal the persisted generation expiry, and the carrier's reported TTL
        // must equal `expiry - anchor`.
        if let CiCredentialExpectation::Phase(_) = expected_purpose {
            let binding = context
                .credential_binding
                .as_ref()
                .ok_or_else(|| HookError("phase binding disappeared mid-verification".into()))?;
            let expected_ttl = binding
                .expires_at_epoch_secs
                .checked_sub(binding.issued_at_epoch_secs)
                .and_then(|ttl| u64::try_from(ttl).ok())
                .ok_or_else(|| HookError("phase credential window is invalid".into()))?;
            if verified.exp_unix != binding.expires_at_epoch_secs
                || spec.run_token.ttl_secs() != expected_ttl
            {
                return Err(HookError(
                    "signed CI credential expiry differs from its durable generation".into(),
                ));
            }
            // The carrier JTI must be the DETERMINISTIC one Identity produces for exactly this
            // generation and anchor. Identity's own boundary already proves carrier == signed; this
            // additionally proves both equal the value the control plane persisted as evidence.
            let expected_jti = expected_phase_jti(&expected_run_id, binding.issued_at_epoch_secs)
                .map_err(|error| {
                HookError(format!(
                    "CI phase credential JTI is underivable: {}",
                    error.0
                ))
            })?;
            if spec.run_token.jti != expected_jti {
                return Err(HookError(
                    "CI phase credential JTI is not the deterministic one for its generation"
                        .into(),
                ));
            }
        }
        Ok(context)
    }

    /// Recompute the generation id from server-resolved facts and require the carrier's JTI to be
    /// the deterministic one Identity must have produced for it. Returns the RECOMPUTED generation
    /// id, which is what the signed `run_id` is then required to equal — the context's own
    /// `generation_id` field is never trusted as the comparison value.
    fn verify_phase_binding(
        &self,
        context: &CiJobAuthorizationContext,
        binding: &CiJobCredentialBinding,
        expected_purpose: CiCredentialPurpose,
    ) -> Result<String, HookError> {
        if binding.binding_version != CI_PHASE_CREDENTIAL_BINDING_V1 {
            return Err(HookError(
                "CI phase credential carries an unsupported binding version".into(),
            ));
        }
        if binding.purpose != expected_purpose.token() {
            return Err(HookError(format!(
                "CI phase credential was minted for purpose {:?}, but this boundary requires {:?}",
                binding.purpose,
                expected_purpose.token()
            )));
        }
        if binding.issued_at_epoch_secs <= 0
            || binding.issued_at_epoch_secs < context.claim_started_at_epoch_secs
            || binding.expires_at_epoch_secs <= binding.issued_at_epoch_secs
            || binding.expires_at_epoch_secs > context.claim_expires_at_epoch_secs
            || binding.ci_run_id.trim().is_empty()
            || binding.token_authority_handle.trim().is_empty()
            || binding.idem_token.trim().is_empty()
        {
            return Err(HookError(
                "CI phase credential binding is outside its durable claim".into(),
            ));
        }
        let recomputed = phase_generation_id(CiPhaseGenerationInputs {
            tenant_id: &context.tenant_id,
            region: &context.region,
            wf_run_id: &context.wf_run_id,
            ci_run_id: &binding.ci_run_id,
            job_id: &context.job_id,
            token_authority_handle: &binding.token_authority_handle,
            idem_token: &binding.idem_token,
            lease_owner: &context.lease_owner,
            lease_epoch: context.lease_epoch,
            claim_nonce: &context.claim_nonce,
            claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
            purpose: expected_purpose,
            issued_at_epoch_secs: binding.issued_at_epoch_secs,
            expires_at_epoch_secs: binding.expires_at_epoch_secs,
            binding_version: binding.binding_version,
        });
        if recomputed != binding.generation_id {
            return Err(HookError(
                "CI phase credential generation id is not the digest of its own binding".into(),
            ));
        }
        Ok(recomputed)
    }

    /// Reauthorize signed facts and return a lazy durable permit. The shared verifier's exact
    /// signed-authority equality is the legacy path's checkout shape/commit predicate; a mutated
    /// compute/None request cannot discard a checkout commit carried by the signed credential. The
    /// exact durable CAS runs only after the sandbox launch guard is spawned and armed.
    pub fn authorize_retained(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
        let context = self.verify_ci_job_signed(spec, CiCredentialExpectation::LegacyClaimBound)?;
        self.claim_gate
            .permit(context)
            .map_err(|error| HookError(format!("durable scheduler launch fence failed: {error}")))
    }

    /// **CT-007 phase-credential generations: the V2 workload launch boundary.** Verifies the signed
    /// credential as a `workload` phase credential (so any preparation credential is refused here by
    /// purpose, generation id, AND capability vector) and returns the retained permit whose CAS
    /// folds the current-generation predicate into the launch transaction itself.
    pub fn authorize_workload_v2_retained(
        &self,
        spec: &JobSpec,
    ) -> Result<LaunchPermit, HookError> {
        let (context, gate) = self.phase_boundary(spec, CiCredentialPurpose::Workload)?;
        self.claim_gate
            .workload_v2_permit(context, gate)
            .map_err(|error| {
                HookError(format!("durable V2 scheduler launch fence failed: {error}"))
            })
    }

    /// Hop A step 1 (`git upload-pack --advertise-refs`).
    pub fn authorize_checkout_advertise_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<LaunchPermit, HookError> {
        self.authorize_preparation_retained(spec, scope, CiCredentialPurpose::CheckoutAdvertise)
    }

    /// Hop A step 2 (`git upload-pack`, the pack fetch).
    pub fn authorize_checkout_fetch_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<LaunchPermit, HookError> {
        self.authorize_preparation_retained(spec, scope, CiCredentialPurpose::CheckoutFetch)
    }

    /// Hop B (the checkout-preparation runtime).
    pub fn authorize_checkout_materialization_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<LaunchPermit, HookError> {
        self.authorize_preparation_retained(
            spec,
            scope,
            CiCredentialPurpose::CheckoutMaterialization,
        )
    }

    /// The shared preparation-boundary core. Every preparation gate ALSO re-checks that the caller's
    /// own checkout scope agrees with the server-resolved context (the same anti-substitution check
    /// `authorize_checkout` performs), because a caller holding a valid bearer for one repo/commit
    /// must not be able to drive a spawn against another.
    fn authorize_preparation_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
        purpose: CiCredentialPurpose,
    ) -> Result<LaunchPermit, HookError> {
        let (context, gate) = self.phase_boundary(spec, purpose)?;
        if context.checkout_scope.as_ref() != Some(scope) {
            return Err(HookError(
                "checkout scope handed to the phase authorization boundary differs from the \
                 server-resolved authorization context"
                    .into(),
            ));
        }
        self.claim_gate
            .phase_permit(gate)
            .map_err(|error| HookError(format!("durable phase credential gate failed: {error}")))
    }

    fn phase_boundary<'a>(
        &self,
        spec: &'a JobSpec,
        purpose: CiCredentialPurpose,
    ) -> Result<(&'a CiJobAuthorizationContext, CiPhaseGenerationGate), HookError> {
        let context = self.verify_ci_job_signed(spec, CiCredentialExpectation::Phase(purpose))?;
        let binding = context.credential_binding.as_ref().ok_or_else(|| {
            HookError("verified phase context carries no credential binding".into())
        })?;
        let gate = phase_gate_from_context(context, binding, purpose);
        if gate.jti != spec.run_token.jti {
            return Err(HookError(
                "the durable phase gate's expected JTI differs from the presented carrier".into(),
            ));
        }
        Ok((context, gate))
    }

    /// CT-007 slice 5b.3-2c: the real `CheckoutAuthorizationHook` implementation — a READ-ONLY,
    /// pre-Hop-A check (never a state transition) that the job's durably authorized claim actually
    /// grants read access to the EXACT repo/commit `scope` names. Runs the SAME shared verification
    /// core `authorize_retained` runs (signed bearer, capability vector, workspace-scope agreement),
    /// PLUS confirms the scope the caller handed in agrees with the server-resolved authorization
    /// context (so a caller that passed a different scope than the one actually minted is refused
    /// even if the bearer itself checks out), PLUS a read-only durable-claim-liveness check
    /// (`verify_live` — never the `leased -> running` CAS `authorize_retained` alone may commit).
    pub fn authorize_checkout(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<(), HookError> {
        let context = self.verify_ci_job_signed(spec, CiCredentialExpectation::LegacyClaimBound)?;
        if context.checkout_scope.as_ref() != Some(scope) {
            return Err(HookError(
                "checkout scope handed to the authorization hook differs from the server-resolved \
                 authorization context"
                    .into(),
            ));
        }
        self.claim_gate
            .verify_live(context)
            .map_err(|error| HookError(format!("durable scheduler claim is not live: {error}")))
    }
}

fn validate_claim_authority(
    claim: &CiJobTokenRequest,
    authority: &CiJobRuntimeAuthorityRequest,
) -> Result<(), CiJobTokenIssueError> {
    if authority.tenant_id != claim.tenant_id
        || authority.region != claim.region
        || authority.ci_run_id != claim.ci_run_id
        || authority.wf_run_id != claim.wf_run_id
        || authority.job_id != claim.job_id
        || authority
            .reserve_id
            .as_deref()
            .is_none_or(|reserve_id| reserve_id.trim().is_empty())
    {
        return Err(refused(
            "durable CI authority does not match the scheduler claim",
        ));
    }
    // CT-007 lease/topology reconciliation: `CiJobTokenRequest::validate` can only bound the claim
    // lifetime by the GLOBAL maximum (88,800s), because it has no topology context. This is the
    // boundary that does have it — the server-reconstructed authority says whether the job is
    // checkout-bearing — so the topology-specific ceiling is enforced HERE, at the raw Identity
    // seam. Without it, a caller reaching `mint_verified` directly (the exported minter, not the
    // locked issuer whose `verify_claim_window` protects the composed path) could bind a credential
    // to a non-checkout claim four times longer than that job's topology can justify.
    let claim_lifetime =
        u64::try_from(claim.claim_expires_at_epoch_secs - claim.claim_started_at_epoch_secs)
            .map_err(|_| refused("claim lifetime is outside the supported range"))?;
    let topology_ceiling = if authority.checkout.is_some() {
        crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS
    } else {
        u64::try_from(crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
            .expect("the execution-lease bound is positive")
    };
    if claim_lifetime > topology_ceiling {
        return Err(refused(
            "claim lifetime exceeds what this job's durable topology can justify",
        ));
    }
    Ok(())
}

fn ci_principal(tenant: &str, region: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId(CI_JOB_PRINCIPAL_ID.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn timestamp_from_epoch(epoch_secs: i64) -> Result<Timestamp, CiJobTokenIssueError> {
    DateTime::<Utc>::from_timestamp(epoch_secs, 0)
        .map(|instant| Timestamp(instant.to_rfc3339_opts(SecondsFormat::Secs, true)))
        .ok_or_else(|| refused("scheduler claim timestamp is outside the supported range"))
}

fn system_now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(i64::MIN)
}

fn refused(detail: impl Into<String>) -> CiJobTokenIssueError {
    CiJobTokenIssueError(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::{
        EgressPolicy, IdemToken, ImageRef, MeterTarget, ResourceLimits, TrustTier, WorkspaceSpec,
    };
    use myelin_identity_service::{
        CellTokenAuthority, PasetoCapabilitySigner, PasetoCapabilityVerifier, RevocationStore,
    };

    const START: i64 = 1_785_000_000;
    const NOW: i64 = START + 10;
    const EXPIRY: i64 = START + 30;
    const WF_RUN_ID: &str = "11111111-1111-4111-8111-111111111111";
    const CI_RUN_ID: &str = "22222222-2222-4222-8222-222222222222";
    const JOB_ID: &str = "33333333-3333-4333-8333-333333333333";
    const NONCE: &str = "44444444-4444-4444-8444-444444444444";

    struct AllowClaimGate;

    impl CiJobLaunchClaimGate for AllowClaimGate {
        fn permit(&self, _context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::immediate())
        }

        fn verify_live(&self, _context: &CiJobAuthorizationContext) -> Result<(), String> {
            Ok(())
        }

        fn phase_permit(&self, _gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::immediate())
        }

        fn workload_v2_permit(
            &self,
            _context: &CiJobAuthorizationContext,
            _gate: CiPhaseGenerationGate,
        ) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::immediate())
        }
    }

    struct RefuseClaimGate;

    impl CiJobLaunchClaimGate for RefuseClaimGate {
        fn permit(&self, _context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::retained(|| {
                Err(HookError("durable scheduler claim was refused".into()))
            }))
        }

        fn verify_live(&self, _context: &CiJobAuthorizationContext) -> Result<(), String> {
            Err("durable scheduler claim was refused".into())
        }

        fn phase_permit(&self, _gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::retained(|| {
                Err(HookError("durable phase generation was refused".into()))
            }))
        }

        fn workload_v2_permit(
            &self,
            _context: &CiJobAuthorizationContext,
            _gate: CiPhaseGenerationGate,
        ) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::retained(|| {
                Err(HookError("durable scheduler claim was refused".into()))
            }))
        }
    }

    fn claim() -> CiJobTokenRequest {
        CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "eu-west".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: WF_RUN_ID.into(),
            ci_run_id: CI_RUN_ID.into(),
            job_id: JOB_ID.into(),
            token_authority_handle: "ci-token-authority:v1:test".into(),
            idem_token: "idem-job".into(),
            lease_owner: "runner-1".into(),
            lease_epoch: 1,
            claim_nonce: NONCE.into(),
            claim_started_at_epoch_secs: START,
            claim_expires_at_epoch_secs: EXPIRY,
        }
    }

    fn authority() -> CiJobRuntimeAuthorityRequest {
        CiJobRuntimeAuthorityRequest {
            tenant_id: "acme".into(),
            region: "eu-west".into(),
            ci_run_id: CI_RUN_ID.into(),
            wf_run_id: WF_RUN_ID.into(),
            project_id: "project-1".into(),
            job_id: JOB_ID.into(),
            stage: "test".into(),
            concrete_name: "test".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            source_snapshot_digest: "digest".into(),
            workflow_definition_version: 1,
            workflow_code_hash: "code-hash".into(),
            policy_revision: "policy-1".into(),
            limits: crate::CiManifestLimitsV1 {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 30,
            },
            reserve_id: Some("reserve-1".into()),
            checkout_commit: None,
            checkout: None,
        }
    }

    fn job(credential: RunTokenCredential, claim: &CiJobTokenRequest) -> JobSpec {
        let mut spec = JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.invalid/ci@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["true".into()],
            Vec::new(),
            Vec::new(),
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 30,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            credential,
            MeterTarget {
                reserve_id: "reserve-1".into(),
            },
            IdemToken("idem-job".into()),
        )
        .unwrap();
        spec.run_token_authorization = Some(ci_job_authorization_context(claim, "reserve-1", None));
        spec
    }

    fn checkout_workspace() -> WorkspaceSpec {
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".into()),
            commit: Some("a".repeat(40)),
        }
    }

    fn checkout_scope() -> CheckoutAuthorizationScope {
        derive_checkout_authorization_scope(JobKind::Ci, &checkout_workspace())
            .unwrap()
            .unwrap()
    }

    fn checkout_authority() -> CiJobRuntimeAuthorityRequest {
        CiJobRuntimeAuthorityRequest {
            checkout_commit: Some("a".repeat(40)),
            checkout: Some(checkout_scope()),
            ..authority()
        }
    }

    /// Like `job()`, but with an arbitrary `workspace` and an explicit `checkout` scope for the
    /// authorization context — the checkout-authorization test fixture.
    fn checkout_job(
        credential: RunTokenCredential,
        claim: &CiJobTokenRequest,
        workspace: WorkspaceSpec,
        checkout: Option<&CheckoutAuthorizationScope>,
    ) -> JobSpec {
        let mut spec = JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.invalid/ci@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["true".into()],
            Vec::new(),
            Vec::new(),
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 30,
            },
            workspace,
            TrustTier::Trusted,
            credential,
            MeterTarget {
                reserve_id: "reserve-1".into(),
            },
            IdemToken("idem-job".into()),
        )
        .unwrap();
        spec.run_token_authorization =
            Some(ci_job_authorization_context(claim, "reserve-1", checkout));
        spec
    }

    fn mut_context(spec: &mut JobSpec) -> &mut CiJobAuthorizationContext {
        let Some(RunTokenAuthorizationContext::CiJob(context)) =
            spec.run_token_authorization.as_mut()
        else {
            panic!("CI context")
        };
        context
    }

    /// Records which `CiJobLaunchClaimGate` methods were invoked, wrapping an always-succeeding
    /// inner behavior — proves `authorize_checkout` calls `verify_live` and NEVER `permit` (the
    /// real workload's `leased -> running` CAS stays untouched by checkout authorization).
    #[derive(Default)]
    struct SpyClaimGateCalls {
        verify_live_calls: usize,
        permit_calls: usize,
        /// Every phase gate this boundary handed down, in order — the V2 tests assert exactly which
        /// purpose/generation reached the durable layer.
        phase_gates: Vec<CiPhaseGenerationGate>,
        workload_v2_gates: Vec<CiPhaseGenerationGate>,
    }

    struct SpyClaimGate(std::sync::Mutex<SpyClaimGateCalls>);

    impl CiJobLaunchClaimGate for SpyClaimGate {
        fn permit(&self, _context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
            self.0.lock().unwrap().permit_calls += 1;
            Ok(LaunchPermit::immediate())
        }

        fn verify_live(&self, _context: &CiJobAuthorizationContext) -> Result<(), String> {
            self.0.lock().unwrap().verify_live_calls += 1;
            Ok(())
        }

        fn phase_permit(&self, gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String> {
            self.0.lock().unwrap().phase_gates.push(gate);
            Ok(LaunchPermit::immediate())
        }

        fn workload_v2_permit(
            &self,
            _context: &CiJobAuthorizationContext,
            gate: CiPhaseGenerationGate,
        ) -> Result<LaunchPermit, String> {
            self.0.lock().unwrap().workload_v2_gates.push(gate);
            Ok(LaunchPermit::immediate())
        }
    }

    /// Mint a real signed checkout-bearing credential + build the matching boundary, for the
    /// checkout-authorization test suite below.
    async fn checkout_test_rig() -> (
        RunTokenCredential,
        CiJobTokenRequest,
        Arc<CellTokenAuthority>,
        RevocationStore,
    ) {
        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[8_u8; 32], &[10_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(s7.clone(), None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);
        let claim = claim();
        let credential = adapter
            .mint_verified(claim.clone(), checkout_authority())
            .await
            .expect("checkout-bearing claim mints");
        (credential, claim, cell, s7)
    }

    fn checkout_boundary(
        cell: &CellTokenAuthority,
        s7: RevocationStore,
        claim_gate: Arc<dyn CiJobLaunchClaimGate>,
    ) -> IdentityCiJobLaunchAuthorizer {
        let verifier = PasetoCapabilityVerifier::new(cell.trust_anchor()).with_clock(|| NOW);
        IdentityCiJobLaunchAuthorizer::with_claim_gate(
            RunTokenAuthorizer::new(Arc::new(verifier), s7)
                .with_clock(|| timestamp_from_epoch(NOW).unwrap()),
            Region("eu-west".into()),
            claim_gate,
        )
    }

    #[test]
    fn compute_mint_capabilities_add_exactly_the_reservation_binding() {
        assert_eq!(
            required_ci_capabilities("reserve-1", None),
            vec![
                "job.launch".to_string(),
                "artifact.write".to_string(),
                "reserve:reserve-1#consume".to_string(),
            ]
        );
    }

    #[test]
    fn checkout_mint_capabilities_bind_repo_and_exact_commit() {
        let caps = required_ci_capabilities("reserve-1", Some(&checkout_scope()));
        assert_eq!(
            caps,
            vec![
                "job.launch".to_string(),
                "artifact.write".to_string(),
                "reserve:reserve-1#consume".to_string(),
                "repo:myelin://acme/git/repo/widgets#pull".to_string(),
                format!("checkout-commit:sha1:{}#attest", "a".repeat(40)),
            ]
        );
    }

    #[test]
    fn sha256_scope_derives_the_same_repo_capability_and_retains_the_full_commit() {
        let scope = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/widgets".into()),
                commit: Some("b".repeat(64)),
            },
        )
        .unwrap()
        .unwrap();
        let sha256 = required_ci_capabilities("reserve-1", Some(&scope));
        let sha1 = required_ci_capabilities("reserve-1", Some(&checkout_scope()));
        assert_eq!(sha256[3], sha1[3], "the repo grant remains canonical");
        assert_ne!(sha256[4], sha1[4], "the exact commit attestation is signed");
        assert_eq!(
            sha256[4],
            format!("checkout-commit:sha256:{}#attest", "b".repeat(64))
        );
        assert_eq!(scope.commit_hex(), "b".repeat(64));
    }

    #[tokio::test]
    async fn checkout_authorization_succeeds_for_the_exact_minted_scope_without_touching_leased() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let calls = Arc::new(SpyClaimGate(std::sync::Mutex::new(Default::default())));
        let boundary = checkout_boundary(&cell, s7, calls.clone());
        let scope = checkout_scope();
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));

        boundary
            .authorize_checkout(&spec, &scope)
            .expect("the exact minted checkout scope is authorized");

        let recorded = calls.0.lock().unwrap();
        assert_eq!(recorded.verify_live_calls, 1);
        assert_eq!(
            recorded.permit_calls, 0,
            "checkout authorization must never touch the real workload's leased->running CAS"
        );
    }

    #[tokio::test]
    async fn checkout_authorization_then_the_workload_permit_can_still_be_acquired_and_committed() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));

        boundary
            .authorize_checkout(&spec, &scope)
            .expect("checkout authorization succeeds");
        boundary
            .authorize(&spec)
            .expect("the subsequent real workload permit is still acquirable and committable");
    }

    #[tokio::test]
    async fn checkout_a_credential_is_refused_for_compute_absent_launch() {
        // Shape-downgrade mutation: every unsigned carrier field consistently says compute/None,
        // while the signed checkout-A credential still carries repo + exact-commit authority.
        // The standalone verifier must reject that unused signed superset before the durable gate.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let compute_spec = job(credential, &claim);

        assert!(
            boundary.authorize_retained(&compute_spec).is_err(),
            "a checkout-bearing signed authority must not authorize a compute/None launch"
        );
    }

    #[tokio::test]
    async fn compute_credential_is_refused_for_checkout_launch() {
        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[11_u8; 32], &[12_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(s7.clone(), None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);
        let claim = claim();
        let credential = adapter
            .mint_verified(claim.clone(), authority())
            .await
            .expect("compute claim mints");
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let checkout_spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));

        assert!(
            boundary.authorize_retained(&checkout_spec).is_err(),
            "a compute signed authority must not authorize a checkout-bearing launch"
        );
    }

    #[tokio::test]
    async fn commit_substitution_after_mint_fails_at_the_context_scope_comparison() {
        // The exact production attack shape (Sol's review): mint for `widgets@A`, keep the
        // server-resolved authorization context's own scope as `widgets@A` (the claim-time-
        // validated value -- server-produced but NOT signed itself; only the bearer is signed, and
        // it is never re-derived after mint), but the in-hand JobSpec's OWN workspace now
        // names a DIFFERENT commit `widgets@B` on the SAME repo (e.g. a worker that re-resolved its
        // checkout target after the token was already minted). The hook is handed the scope
        // re-derived from THIS workspace (`widgets@B`) -- exactly what a real caller would compute
        // from the JobSpec it actually has. This must fail specifically at
        // `context.checkout_scope != rederived_checkout` inside `verify_ci_job_signed`, since the
        // dynamic `repo:widgets#pull` capability alone says nothing about which commit.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let minted_scope = checkout_scope(); // widgets@A -- what the context carries
        let substituted_workspace = WorkspaceSpec {
            repo_ref: checkout_workspace().repo_ref, // same repo: widgets
            commit: Some("c".repeat(40)),            // different commit: B
        };
        let rederived_from_substituted_workspace =
            derive_checkout_authorization_scope(JobKind::Ci, &substituted_workspace)
                .unwrap()
                .unwrap();
        // Context still carries the ORIGINAL minted scope (`Some(&minted_scope)`) -- only the
        // JobSpec's own `workspace` field changed.
        let spec = checkout_job(
            credential,
            &claim,
            substituted_workspace,
            Some(&minted_scope),
        );
        assert!(boundary
            .authorize_checkout(&spec, &rederived_from_substituted_workspace)
            .is_err());
    }

    #[tokio::test]
    async fn post_mint_signed_context_checkout_commit_substitution_is_refused() {
        // Make workspace, context, hook scope, and expected vector consistently name commit B.
        // Structural equality therefore passes; only Identity's signature over the exact commit
        // attestation can distinguish this from the credential minted for commit A.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let substituted_workspace = WorkspaceSpec {
            repo_ref: checkout_workspace().repo_ref,
            commit: Some("c".repeat(40)),
        };
        let substituted_scope =
            derive_checkout_authorization_scope(JobKind::Ci, &substituted_workspace)
                .unwrap()
                .unwrap();
        let spec = checkout_job(
            credential,
            &claim,
            substituted_workspace,
            Some(&substituted_scope),
        );
        assert!(boundary
            .authorize_checkout(&spec, &substituted_scope)
            .is_err());
    }

    #[tokio::test]
    async fn repository_substitution_fails_signed_token_verification() {
        // The other production attack shape (Sol's review): workspace, context, AND the hook's own
        // argument all consistently name a DIFFERENT repo (`other`) than what was actually minted
        // (`widgets`). Every STRUCTURAL comparison this slice added (capability-vector shape,
        // scope-agreement) passes, because all three inputs agree with each other -- but Identity's
        // own cryptographic verification must still fail, because the signed token was minted with
        // `repo:widgets#pull`, never `repo:other#pull`.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let other_workspace = WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/other".into()),
            commit: checkout_workspace().commit,
        };
        let other_scope = derive_checkout_authorization_scope(JobKind::Ci, &other_workspace)
            .unwrap()
            .unwrap();
        let spec = checkout_job(credential, &claim, other_workspace, Some(&other_scope));
        assert!(boundary.authorize_checkout(&spec, &other_scope).is_err());
    }

    #[tokio::test]
    async fn post_mint_reserve_id_substitution_fails_at_the_context_comparison() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let mut spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        spec.meter_to.reserve_id = "reserve-substituted".into();
        assert!(boundary.authorize_checkout(&spec, &scope).is_err());
    }

    #[tokio::test]
    async fn reserve_id_substitution_fails_signed_token_verification() {
        // Make spec, context, and expected vector consistently name a substituted reservation so
        // every structural comparison passes. The real PASETO was minted for reserve-1 and must be
        // the check that refuses this attack shape.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let mut spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        let substituted = "reserve-substituted";
        spec.meter_to.reserve_id = substituted.into();
        let context = mut_context(&mut spec);
        context.reserve_id = substituted.into();
        context.required_capabilities = required_ci_capabilities(substituted, Some(&scope));
        assert!(boundary.authorize_checkout(&spec, &scope).is_err());
    }

    #[tokio::test]
    async fn hook_scope_differing_from_the_in_hand_jobspec_scope_is_refused() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        let different_repo_scope = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/other".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap()
        .unwrap();
        assert!(boundary
            .authorize_checkout(&spec, &different_repo_scope)
            .is_err());
    }

    #[tokio::test]
    async fn context_checkout_scope_missing_or_differing_is_refused() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();

        // Context carries NO checkout scope even though the workspace is checkout-bearing.
        let missing = checkout_job(credential.clone(), &claim, checkout_workspace(), None);
        assert!(boundary.authorize_checkout(&missing, &scope).is_err());
        assert!(boundary.authorize(&missing).is_err());

        // Context carries a DIFFERENT checkout scope than what the workspace itself re-derives to.
        let other_scope = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/other".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap()
        .unwrap();
        let divergent = checkout_job(credential, &claim, checkout_workspace(), Some(&other_scope));
        assert!(boundary.authorize_checkout(&divergent, &scope).is_err());
        assert!(boundary.authorize(&divergent).is_err());
    }

    #[tokio::test]
    async fn capability_vector_mismatches_are_refused() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();

        type CapabilityMutation = Box<dyn Fn(&mut Vec<String>)>;
        let mutations: Vec<(&str, CapabilityMutation)> = vec![
            (
                "missing the repo capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps.remove(3);
                }),
            ),
            (
                "wrong repo capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps[3] = "repo:myelin://acme/git/repo/other#pull".into();
                }),
            ),
            (
                "wrong checkout commit capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps[4] = format!("checkout-commit:sha1:{}#attest", "f".repeat(40));
                }),
            ),
            (
                "wrong reserve capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps[2] = "reserve:reserve-substituted#consume".into();
                }),
            ),
            (
                "duplicate repo capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps.push(caps[3].clone());
                }),
            ),
            (
                "reordered capabilities",
                Box::new(|caps: &mut Vec<String>| {
                    caps.swap(0, 1);
                }),
            ),
            (
                "extra unrelated capability",
                Box::new(|caps: &mut Vec<String>| {
                    caps.push("job.extra".into());
                }),
            ),
        ];
        for (label, mutate) in mutations {
            let mut spec = checkout_job(
                credential.clone(),
                &claim,
                checkout_workspace(),
                Some(&scope),
            );
            mutate(&mut mut_context(&mut spec).required_capabilities);
            assert!(
                boundary.authorize_checkout(&spec, &scope).is_err(),
                "{label} must be refused"
            );
        }
    }

    #[tokio::test]
    async fn revoked_bearer_fails_pre_hop_a_checkout_authorization() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7.clone(), Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let principal = ci_principal(&claim.tenant_id, &claim.region);
        let tenant_scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        s7.tear_down_run_token(
            &tenant_scope,
            &credential.jti,
            timestamp_from_epoch(NOW).unwrap(),
        );
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        assert!(boundary.authorize_checkout(&spec, &scope).is_err());
    }

    #[tokio::test]
    async fn expired_context_fails_pre_hop_a_checkout_authorization() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let scope = checkout_scope();
        let mut spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        mut_context(&mut spec).claim_expires_at_epoch_secs = EXPIRY - 1;
        assert!(boundary.authorize_checkout(&spec, &scope).is_err());
    }

    #[tokio::test]
    async fn malformed_workspace_intent_fails_before_any_claim_lookup() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let calls = Arc::new(SpyClaimGate(std::sync::Mutex::new(Default::default())));
        let boundary = checkout_boundary(&cell, s7, calls.clone());
        let scope = checkout_scope();
        // Mixed workspace (repo_ref Some, commit None) -- syntactically invalid, refused by
        // `derive_checkout_authorization_scope` itself before any durable-claim gate is consulted.
        let mixed_workspace = WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".into()),
            commit: None,
        };
        let spec = checkout_job(credential, &claim, mixed_workspace, Some(&scope));
        assert!(boundary.authorize_checkout(&spec, &scope).is_err());
        assert!(boundary.authorize(&spec).is_err());
        let recorded = calls.0.lock().unwrap();
        assert_eq!(
            recorded.verify_live_calls + recorded.permit_calls,
            0,
            "a malformed workspace must be refused before ever reaching the durable claim gate"
        );
    }

    #[tokio::test]
    async fn stale_claim_refuses_checkout_authorization_without_touching_leased() {
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(RefuseClaimGate));
        let scope = checkout_scope();
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        assert!(
            boundary.authorize_checkout(&spec, &scope).is_err(),
            "a stale/canceled/reaped durable claim must refuse checkout authorization"
        );
    }

    #[tokio::test]
    async fn complete_mint_to_hook_agreement_sequence() {
        // 1. Mint grants the exact repository capability.
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let scope = checkout_scope();
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        // 2. Context carries the exact repo and commit scope.
        let Some(RunTokenAuthorizationContext::CiJob(context)) =
            spec.run_token_authorization.as_ref()
        else {
            panic!("CI context")
        };
        assert_eq!(context.checkout_scope.as_ref(), Some(&scope));
        assert!(context
            .required_capabilities
            .contains(&"repo:myelin://acme/git/repo/widgets#pull".to_string()));
        // 3. Checkout authorization succeeds without changing `leased`.
        boundary
            .authorize_checkout(&spec, &scope)
            .expect("checkout authorization succeeds");
        // 4. A different repo or same-repo different commit fails.
        let different_repo = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/other".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap()
        .unwrap();
        assert!(boundary.authorize_checkout(&spec, &different_repo).is_err());
        let different_commit = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: checkout_workspace().repo_ref,
                commit: Some("f".repeat(40)),
            },
        )
        .unwrap()
        .unwrap();
        assert!(boundary
            .authorize_checkout(&spec, &different_commit)
            .is_err());
        // 5. The subsequent workload permit can still be acquired and committed once.
        boundary
            .authorize(&spec)
            .expect("the real workload permit is still acquirable and committable");
    }

    // Note: "an ordinary unconfigured RunnerHooks still refuses checkout" is NOT re-tested here --
    // `RunnerHooks::authorize_checkout` is `pub(crate)` to `myelin-ci-sandbox` and inaccessible from
    // this crate, and 5b.3-2a's own `authorize_checkout_refuses_when_no_hook_is_configured` test (in
    // that crate) already covers it. The live-PostgreSQL integration suite
    // (`integration_ci_drive_manifest_store.rs`) directly exercises
    // `IdentityCiJobLaunchAuthorizer::authorize_checkout` against real durable state; that
    // `ci_runner_composition::ci_runner_hooks` correctly WIRES this into `RunnerHooks` is
    // compile-covered only -- actual invocation THROUGH the installed `RunnerHooks` hook remains
    // deferred until the sandbox launch path itself consumes checkout authorization (5b.3-3+).

    #[tokio::test]
    async fn real_paseto_claim_round_trip_reauthorizes_and_binds_absolute_expiry() {
        let s7 = RevocationStore::new();
        let cell = Arc::new(
            CellTokenAuthority::from_seed(&[7_u8; 32], &[9_u8; 32]).expect("cell authority"),
        );
        let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(s7.clone(), None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);
        let claim = claim();
        let (first, concurrent_retry) = tokio::join!(
            adapter.mint_verified(claim.clone(), authority()),
            adapter.mint_verified(claim.clone(), authority()),
        );
        let credential = first.expect("exact live claim mints");
        let concurrent_retry = concurrent_retry.expect("concurrent exact retry mints");
        assert_eq!(credential, concurrent_retry);
        assert_eq!(credential.ttl_secs(), 30);

        let verifier = PasetoCapabilityVerifier::new(cell.trust_anchor()).with_clock(|| NOW);
        let boundary = IdentityCiJobLaunchAuthorizer::with_claim_gate(
            RunTokenAuthorizer::new(Arc::new(verifier), s7.clone())
                .with_clock(|| timestamp_from_epoch(NOW).unwrap()),
            Region("eu-west".into()),
            Arc::new(AllowClaimGate),
        );
        let spec = job(credential.clone(), &claim);
        boundary
            .authorize(&spec)
            .expect("signed exact-scope token is live immediately before launch");

        let forged = job(
            RunTokenCredential::new(
                format!("{}x", credential.expose_bearer()),
                credential.jti.clone(),
                credential.ttl_secs(),
            )
            .unwrap(),
            &claim,
        );
        assert!(boundary.authorize(&forged).is_err());

        assert!(boundary.authorize(&job(credential.clone(), &claim)).is_ok());
        let signed = job(spec.run_token.clone(), &claim);
        let mut overlong_context = signed;
        let Some(RunTokenAuthorizationContext::CiJob(context)) =
            overlong_context.run_token_authorization.as_mut()
        else {
            panic!("CI context")
        };
        context.claim_expires_at_epoch_secs = EXPIRY - 1;
        assert!(boundary.authorize(&overlong_context).is_err());

        let principal = ci_principal(&claim.tenant_id, &claim.region);
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        s7.tear_down_run_token(&scope, &credential.jti, timestamp_from_epoch(NOW).unwrap());
        let retry_after_teardown = adapter
            .mint_verified(claim.clone(), authority())
            .await
            .expect("acknowledgement-loss retry remains deterministic");
        assert_eq!(credential, retry_after_teardown);
        assert!(boundary
            .authorize(&job(retry_after_teardown, &claim))
            .is_err());
    }

    #[tokio::test]
    async fn mint_refuses_divergent_authority_and_expired_claim_before_signing() {
        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[1_u8; 32], &[2_u8; 32]).unwrap());
        let minter = RunTokenMinter::with_signer_and_tuples(
            s7,
            None,
            Arc::new(PasetoCapabilitySigner::new(cell)),
        );
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);
        let mut divergent = authority();
        divergent.job_id = "55555555-5555-4555-8555-555555555555".into();
        assert!(adapter.mint_verified(claim(), divergent).await.is_err());

        let expired =
            IdentityCiJobCredentialMinter::new(adapter.minter.clone()).with_clock(|| EXPIRY);
        assert!(expired.mint_verified(claim(), authority()).await.is_err());
    }

    #[tokio::test]
    async fn production_lease_gets_one_deterministic_short_generation_and_launch_needs_durable_cas()
    {
        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[3_u8; 32], &[4_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(s7.clone(), None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter.clone()).with_clock(|| NOW);
        let mut long_claim = claim();
        long_claim.claim_expires_at_epoch_secs =
            START + crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;

        let credential = adapter
            .mint_verified(long_claim.clone(), authority())
            .await
            .expect("the production lease mints a bounded token generation");
        assert_eq!(credential.ttl_secs(), MAX_CI_JOB_TOKEN_TTL_SECS);

        let verifier = PasetoCapabilityVerifier::new(cell.trust_anchor()).with_clock(|| NOW);
        let boundary = IdentityCiJobLaunchAuthorizer::with_claim_gate(
            RunTokenAuthorizer::new(Arc::new(verifier), s7)
                .with_clock(|| timestamp_from_epoch(NOW).unwrap()),
            Region("eu-west".into()),
            Arc::new(RefuseClaimGate),
        );
        assert!(
            boundary.authorize(&job(credential, &long_claim)).is_err(),
            "a valid signed token cannot bypass a refused durable launch CAS"
        );

        let late = IdentityCiJobCredentialMinter::new(minter)
            .with_clock(|| START + i64::try_from(MAX_CI_JOB_TOKEN_TTL_SECS).unwrap());
        assert!(
            late.mint_verified(long_claim, authority()).await.is_err(),
            "the same long claim cannot remint after its deterministic token generation expires"
        );
    }

    /// **The topology ceiling is enforced at the RAW seam, not only at the locked issuer.**
    /// `CiJobTokenRequest::validate` cannot bound a claim by topology (it has no authority), and the
    /// locked issuer's `verify_claim_window` only guards the composed path — so `mint_verified`,
    /// which is exported and reachable directly, must refuse a non-checkout authority carrying a
    /// checkout-length claim.
    #[tokio::test]
    async fn the_raw_minter_seam_enforces_the_topology_specific_claim_ceiling() {
        let store = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[5_u8; 32], &[6_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(store, None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);

        // A NON-checkout job may claim exactly one execution slot — and not one second more.
        let mut at_execution_bound = claim();
        at_execution_bound.claim_expires_at_epoch_secs =
            START + crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
        adapter
            .mint_verified(at_execution_bound.clone(), authority())
            .await
            .expect("a non-checkout claim at the execution-lease bound mints");

        let mut over_execution_bound = claim();
        over_execution_bound.claim_expires_at_epoch_secs =
            START + crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS + 1;
        let refused = adapter
            .mint_verified(over_execution_bound.clone(), authority())
            .await
            .expect_err("a non-checkout job may not bind a checkout-length claim");
        assert!(
            refused.0.contains("durable topology"),
            "unexpected refusal: {refused:?}"
        );

        // The SAME over-bound claim mints once the authority is genuinely checkout-bearing — proving
        // the ceiling tracks topology, not merely a lowered global constant.
        adapter
            .mint_verified(over_execution_bound, checkout_authority())
            .await
            .expect("a checkout-bearing authority justifies a multi-execution claim");

        // A checkout-bearing job is still bounded, at the four-execution maximum.
        let mut at_claim_window_bound = claim();
        at_claim_window_bound.claim_expires_at_epoch_secs =
            START + crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS as i64;
        adapter
            .mint_verified(at_claim_window_bound.clone(), checkout_authority())
            .await
            .expect("a checkout claim at the topology maximum mints");
        let mut over_claim_window_bound = at_claim_window_bound;
        over_claim_window_bound.claim_expires_at_epoch_secs += 1;
        assert!(
            adapter
                .mint_verified(over_claim_window_bound, checkout_authority())
                .await
                .is_err(),
            "no authority justifies a claim past the four-execution maximum"
        );
    }

    // =============================================================================================
    // CT-007 phase-credential generations.
    // =============================================================================================

    const ALL_PURPOSES: [CiCredentialPurpose; 4] = [
        CiCredentialPurpose::CheckoutAdvertise,
        CiCredentialPurpose::CheckoutFetch,
        CiCredentialPurpose::CheckoutMaterialization,
        CiCredentialPurpose::Workload,
    ];

    /// **Purpose attenuation is the containment property.** A preparation credential must not carry
    /// `artifact.write` (an escaped Hop A/B container cannot write artifacts) and the workload
    /// credential must not carry the repo grant (a workload bearer cannot re-enter the git wire).
    #[test]
    fn phase_capability_vectors_are_attenuated_per_purpose() {
        let scope = checkout_scope();
        assert_eq!(
            phase_ci_capabilities(
                CiCredentialPurpose::CheckoutAdvertise,
                "reserve-1",
                Some(&scope),
            ),
            vec![
                "job.launch".to_string(),
                "repo:myelin://acme/git/repo/widgets#pull".to_string(),
                format!("checkout-commit:sha1:{}#attest", "a".repeat(40)),
                "reserve:reserve-1#consume".to_string(),
            ]
        );
        assert_eq!(
            phase_ci_capabilities(
                CiCredentialPurpose::CheckoutFetch,
                "reserve-1",
                Some(&scope)
            ),
            phase_ci_capabilities(
                CiCredentialPurpose::CheckoutAdvertise,
                "reserve-1",
                Some(&scope),
            )
        );
        assert_eq!(
            phase_ci_capabilities(
                CiCredentialPurpose::CheckoutMaterialization,
                "reserve-1",
                Some(&scope),
            ),
            vec![
                "job.launch".to_string(),
                format!("checkout-commit:sha1:{}#attest", "a".repeat(40)),
                "reserve:reserve-1#consume".to_string(),
            ]
        );
        assert_eq!(
            phase_ci_capabilities(CiCredentialPurpose::Workload, "reserve-1", Some(&scope)),
            vec![
                "job.launch".to_string(),
                "artifact.write".to_string(),
                format!("checkout-commit:sha1:{}#attest", "a".repeat(40)),
                "reserve:reserve-1#consume".to_string(),
            ]
        );
        for purpose in ALL_PURPOSES {
            let caps = phase_ci_capabilities(purpose, "reserve-1", Some(&scope));
            assert_eq!(
                caps.contains(&"artifact.write".to_string()),
                purpose == CiCredentialPurpose::Workload,
                "only the workload credential may write artifacts ({purpose:?})"
            );
            assert_eq!(
                caps.iter().any(|c| c.starts_with("repo:")),
                matches!(
                    purpose,
                    CiCredentialPurpose::CheckoutAdvertise | CiCredentialPurpose::CheckoutFetch
                ),
                "only the two git-wire executions may pull the repo ({purpose:?})"
            );
        }
    }

    #[test]
    fn the_expected_phase_jti_is_deterministic_and_generation_bound() {
        let a = expected_phase_jti("ci-credential:v1:aaaa", 1_785_000_100).unwrap();
        assert_eq!(
            a,
            expected_phase_jti("ci-credential:v1:aaaa", 1_785_000_100).unwrap()
        );
        assert!(a.starts_with("runtok:svc:ci:ci-credential:v1:aaaa:"));
        assert_ne!(
            a,
            expected_phase_jti("ci-credential:v1:bbbb", 1_785_000_100).unwrap()
        );
        assert_ne!(
            a,
            expected_phase_jti("ci-credential:v1:aaaa", 1_785_000_101).unwrap()
        );
    }

    fn phase_binding_for(
        claim: &CiJobTokenRequest,
        purpose: CiCredentialPurpose,
        issued: i64,
        expires: i64,
    ) -> CiPhaseCredentialBinding {
        let generation_id = phase_generation_id(CiPhaseGenerationInputs {
            tenant_id: &claim.tenant_id,
            region: &claim.region,
            wf_run_id: &claim.wf_run_id,
            ci_run_id: &claim.ci_run_id,
            job_id: &claim.job_id,
            token_authority_handle: &claim.token_authority_handle,
            idem_token: &claim.idem_token,
            lease_owner: &claim.lease_owner,
            lease_epoch: claim.lease_epoch,
            claim_nonce: &claim.claim_nonce,
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
            purpose,
            issued_at_epoch_secs: issued,
            expires_at_epoch_secs: expires,
            binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
        });
        CiPhaseCredentialBinding {
            binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
            purpose,
            jti: expected_phase_jti(&generation_id, issued).unwrap(),
            generation_id,
            issued_at_epoch_secs: issued,
            expires_at_epoch_secs: expires,
        }
    }

    /// Mint a REAL signed V2 credential for `purpose`, plus the matching ephemeral context, using
    /// the same anchor/expiry a durable generation row would have carried.
    async fn phase_credential(
        adapter: &IdentityCiJobCredentialMinter,
        claim: &CiJobTokenRequest,
        purpose: CiCredentialPurpose,
    ) -> (RunTokenCredential, CiPhaseCredentialBinding) {
        let binding = phase_binding_for(claim, purpose, NOW, EXPIRY);
        let credential = adapter
            .mint_phase(CiPhaseCredentialMintRequest {
                claim: claim.clone(),
                reserve_id: "reserve-1".into(),
                checkout: Some(checkout_scope()),
                purpose,
                generation_id: binding.generation_id.clone(),
                issued_at_epoch_secs: binding.issued_at_epoch_secs,
                expires_at_epoch_secs: binding.expires_at_epoch_secs,
            })
            .await
            .expect("the phase credential mints");
        assert_eq!(credential.jti, binding.jti, "the JTI is deterministic");
        (credential, binding)
    }

    fn phase_job(
        credential: RunTokenCredential,
        claim: &CiJobTokenRequest,
        binding: &CiPhaseCredentialBinding,
    ) -> JobSpec {
        let scope = checkout_scope();
        let mut spec = JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.invalid/ci@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["true".into()],
            Vec::new(),
            Vec::new(),
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 30,
            },
            checkout_workspace(),
            TrustTier::Trusted,
            credential,
            MeterTarget {
                reserve_id: "reserve-1".into(),
            },
            IdemToken("idem-job".into()),
        )
        .unwrap();
        spec.run_token_authorization = Some(ci_job_phase_authorization_context(
            claim,
            "reserve-1",
            Some(&scope),
            binding,
        ));
        spec
    }

    /// The boundary entry point for each purpose, so the substitution matrix can be driven
    /// uniformly.
    fn authorize_at(
        boundary: &IdentityCiJobLaunchAuthorizer,
        spec: &JobSpec,
        purpose: CiCredentialPurpose,
    ) -> Result<LaunchPermit, HookError> {
        let scope = checkout_scope();
        match purpose {
            CiCredentialPurpose::CheckoutAdvertise => {
                boundary.authorize_checkout_advertise_retained(spec, &scope)
            }
            CiCredentialPurpose::CheckoutFetch => {
                boundary.authorize_checkout_fetch_retained(spec, &scope)
            }
            CiCredentialPurpose::CheckoutMaterialization => {
                boundary.authorize_checkout_materialization_retained(spec, &scope)
            }
            CiCredentialPurpose::Workload => boundary.authorize_workload_v2_retained(spec),
        }
    }

    async fn phase_rig() -> (
        IdentityCiJobCredentialMinter,
        CiJobTokenRequest,
        Arc<CellTokenAuthority>,
        RevocationStore,
    ) {
        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[11_u8; 32], &[12_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(s7.clone(), None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);
        (adapter, claim(), cell, s7)
    }

    /// **THE SUBSTITUTION MATRIX.** Every purpose's credential is accepted at exactly its own
    /// boundary and refused at all three others — including "any preparation credential presented at
    /// workload authorization" and "the workload credential presented at a preparation boundary".
    #[tokio::test]
    async fn every_phase_credential_is_accepted_only_at_its_own_boundary() {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        for minted_purpose in ALL_PURPOSES {
            let (credential, binding) = phase_credential(&adapter, &claim, minted_purpose).await;
            let spec = phase_job(credential, &claim, &binding);
            for boundary_purpose in ALL_PURPOSES {
                let outcome = authorize_at(&boundary, &spec, boundary_purpose);
                assert_eq!(
                    outcome.is_ok(),
                    minted_purpose == boundary_purpose,
                    "a {minted_purpose:?} credential at the {boundary_purpose:?} boundary"
                );
            }
        }
    }

    /// **Dual-read rollout, both directions.** A legacy V1 context cannot satisfy any phase
    /// boundary, and a V2 phase context cannot satisfy the legacy workload boundary — so a mixed
    /// fleet can never accidentally cross-accept.
    #[tokio::test]
    async fn v1_and_v2_credentials_never_satisfy_each_other_s_boundary() {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let boundary = checkout_boundary(&cell, s7.clone(), Arc::new(AllowClaimGate));

        // V2 -> legacy boundary.
        let (credential, binding) =
            phase_credential(&adapter, &claim, CiCredentialPurpose::Workload).await;
        let v2_spec = phase_job(credential, &claim, &binding);
        let error = boundary
            .authorize_retained(&v2_spec)
            .err()
            .expect("a phase-bound credential may not satisfy the legacy boundary");
        assert!(
            error
                .0
                .contains("phase-bound CI credential was presented at a legacy"),
            "message was: {}",
            error.0
        );

        // V1 -> phase boundary. Mint through the legacy claim-bound seam.
        let legacy = adapter
            .mint_verified(claim.clone(), checkout_authority())
            .await
            .expect("the legacy claim-bound credential mints");
        let scope = checkout_scope();
        let v1_spec = checkout_job(legacy, &claim, checkout_workspace(), Some(&scope));
        for purpose in ALL_PURPOSES {
            let error = authorize_at(&boundary, &v1_spec, purpose)
                .err()
                .expect("a legacy credential may not satisfy a phase boundary");
            assert!(
                error
                    .0
                    .contains("legacy claim-bound CI credential was presented at a phase"),
                "message was: {}",
                error.0
            );
        }
        // ...and the legacy credential still works at the legacy boundary, unchanged.
        boundary
            .authorize_retained(&v1_spec)
            .expect("the shipped V1 path is untouched by this slice");
    }

    /// Every field of the binding is load-bearing: mutating any of them breaks the recomputed
    /// generation id (or the exactness checks) and the credential is refused.
    #[tokio::test]
    async fn a_mutated_phase_binding_is_refused() {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let (credential, binding) =
            phase_credential(&adapter, &claim, CiCredentialPurpose::CheckoutAdvertise).await;

        type Mutation = (&'static str, fn(&mut CiJobCredentialBinding));
        let mutations: [Mutation; 8] = [
            ("generation id", |b| {
                b.generation_id = format!("ci-credential:v1:{}", "0".repeat(64))
            }),
            ("purpose", |b| b.purpose = "checkout_fetch".into()),
            ("binding version", |b| b.binding_version = 2),
            ("issuance anchor", |b| b.issued_at_epoch_secs += 1),
            ("expiry", |b| b.expires_at_epoch_secs -= 1),
            ("CI run", |b| {
                b.ci_run_id = "99999999-9999-4999-8999-999999999999".into()
            }),
            ("authority handle", |b| {
                b.token_authority_handle = "ci-token-authority:v2:tampered".into()
            }),
            ("idem token", |b| b.idem_token = "idem-other".into()),
        ];
        for (label, mutate) in mutations {
            let mut spec = phase_job(credential.clone(), &claim, &binding);
            let context = mut_context(&mut spec);
            mutate(
                context
                    .credential_binding
                    .as_mut()
                    .expect("a V2 context carries its binding"),
            );
            assert!(
                boundary
                    .authorize_checkout_advertise_retained(&spec, &checkout_scope())
                    .is_err(),
                "a mutated {label} must be refused"
            );
        }
    }

    /// The durable phase gate receives the RECOMPUTED generation and the deterministic JTI, and the
    /// preparation boundaries never touch the workload's `leased -> running` CAS.
    #[tokio::test]
    async fn a_preparation_boundary_hands_the_exact_generation_down_and_never_runs_the_launch_cas()
    {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let calls = Arc::new(SpyClaimGate(std::sync::Mutex::new(Default::default())));
        let boundary = checkout_boundary(&cell, s7, calls.clone());
        let (credential, binding) = phase_credential(
            &adapter,
            &claim,
            CiCredentialPurpose::CheckoutMaterialization,
        )
        .await;
        let spec = phase_job(credential, &claim, &binding);
        boundary
            .authorize_checkout_materialization_retained(&spec, &checkout_scope())
            .expect("the materialization boundary authorizes");
        let recorded = calls.0.lock().unwrap();
        assert_eq!(recorded.permit_calls, 0, "no legacy launch CAS may run");
        assert_eq!(recorded.workload_v2_gates.len(), 0);
        assert_eq!(recorded.phase_gates.len(), 1);
        let gate = &recorded.phase_gates[0];
        assert_eq!(gate.purpose, CiCredentialPurpose::CheckoutMaterialization);
        assert_eq!(gate.generation_id, binding.generation_id);
        assert_eq!(gate.jti, binding.jti);
        assert_eq!(gate.ci_run_id, claim.ci_run_id);
        assert_eq!(gate.token_authority_handle, claim.token_authority_handle);
        assert_eq!(gate.idem_token, claim.idem_token);
        assert_eq!(gate.issued_at_epoch_secs, binding.issued_at_epoch_secs);
        assert_eq!(gate.expires_at_epoch_secs, binding.expires_at_epoch_secs);
    }

    /// A caller holding a valid phase credential for one repo/commit cannot drive a preparation
    /// spawn against another.
    #[tokio::test]
    async fn a_preparation_boundary_refuses_a_substituted_checkout_scope() {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let (credential, binding) =
            phase_credential(&adapter, &claim, CiCredentialPurpose::CheckoutFetch).await;
        let spec = phase_job(credential, &claim, &binding);
        let other = derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/other".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap()
        .unwrap();
        assert!(boundary
            .authorize_checkout_fetch_retained(&spec, &other)
            .is_err());
    }

    /// **The raw V2 Identity seam is not a bypass.** A caller reaching `mint_phase` directly cannot
    /// have an arbitrary `run_id` signed, cannot bind a window outside its claim, cannot exceed the
    /// 300-second ceiling, and cannot mint a preparation credential without checkout authority.
    #[tokio::test]
    async fn the_raw_phase_seam_refuses_a_forged_binding() {
        let (adapter, claim, _cell, _s7) = phase_rig().await;
        let good = phase_binding_for(&claim, CiCredentialPurpose::Workload, NOW, EXPIRY);
        let request = |generation_id: String,
                       issued: i64,
                       expires: i64,
                       purpose: CiCredentialPurpose,
                       checkout: Option<CheckoutAuthorizationScope>| {
            CiPhaseCredentialMintRequest {
                claim: claim.clone(),
                reserve_id: "reserve-1".into(),
                checkout,
                purpose,
                generation_id,
                issued_at_epoch_secs: issued,
                expires_at_epoch_secs: expires,
            }
        };
        adapter
            .mint_phase(request(
                good.generation_id.clone(),
                NOW,
                EXPIRY,
                CiCredentialPurpose::Workload,
                None,
            ))
            .await
            .expect("the honest binding mints");

        // A run_id that is not the digest of its own binding.
        assert!(adapter
            .mint_phase(request(
                "ci-credential:v1:forged".into(),
                NOW,
                EXPIRY,
                CiCredentialPurpose::Workload,
                None,
            ))
            .await
            .is_err());
        // A window that outlives the claim.
        assert!(adapter
            .mint_phase(request(
                good.generation_id.clone(),
                NOW,
                EXPIRY + 1,
                CiCredentialPurpose::Workload,
                None,
            ))
            .await
            .is_err());
        // A preparation purpose without checkout authority.
        let preparation =
            phase_binding_for(&claim, CiCredentialPurpose::CheckoutAdvertise, NOW, EXPIRY);
        assert!(adapter
            .mint_phase(request(
                preparation.generation_id,
                NOW,
                EXPIRY,
                CiCredentialPurpose::CheckoutAdvertise,
                None,
            ))
            .await
            .is_err());
        // An anchor before the claim started.
        let early = phase_binding_for(&claim, CiCredentialPurpose::Workload, START - 1, EXPIRY);
        assert!(adapter
            .mint_phase(request(
                early.generation_id,
                START - 1,
                EXPIRY,
                CiCredentialPurpose::Workload,
                None,
            ))
            .await
            .is_err());
    }

    /// A generation whose five-minute window has already closed refuses at the raw seam too — the
    /// claim never remints a phase.
    #[tokio::test]
    async fn an_expired_phase_generation_refuses_rather_than_rotating() {
        let (adapter, claim, _cell, _s7) = phase_rig().await;
        let binding = phase_binding_for(&claim, CiCredentialPurpose::Workload, NOW, EXPIRY);
        let late = IdentityCiJobCredentialMinter::new(adapter.minter.clone()).with_clock(|| EXPIRY);
        assert!(late
            .mint_phase(CiPhaseCredentialMintRequest {
                claim: claim.clone(),
                reserve_id: "reserve-1".into(),
                checkout: None,
                purpose: CiCredentialPurpose::Workload,
                generation_id: binding.generation_id,
                issued_at_epoch_secs: binding.issued_at_epoch_secs,
                expires_at_epoch_secs: binding.expires_at_epoch_secs,
            })
            .await
            .is_err());
    }

    /// **Dormancy.** The V1 context construction path — the one the production resolver uses —
    /// never carries a credential binding, so nothing in production can reach a phase boundary.
    #[test]
    fn the_production_context_constructor_never_carries_a_phase_binding() {
        let claim = claim();
        let scope = checkout_scope();
        let RunTokenAuthorizationContext::CiJob(context) =
            ci_job_authorization_context(&claim, "reserve-1", Some(&scope));
        assert!(
            context.credential_binding.is_none(),
            "the V1 production resolver context is claim-bound, never phase-bound"
        );
        assert_eq!(
            context.required_capabilities,
            required_ci_capabilities("reserve-1", Some(&scope)),
            "the V1 capability vector includes the exact reserve binding"
        );
    }
}
