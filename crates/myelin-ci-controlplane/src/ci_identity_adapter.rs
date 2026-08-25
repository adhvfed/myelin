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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiCredentialExpectation {
    LegacyClaimBound,
    Phase(CiCredentialPurpose),
}

pub const CI_JOB_PRINCIPAL_ID: &str = "svc:ci";

pub const CI_JOB_REQUIRED_CAPABILITIES: [&str; 2] = ["job.launch", "artifact.write"];

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

    fn verify_live(&self, context: &CiJobAuthorizationContext) -> Result<(), String>;

    fn phase_permit(&self, gate: CiPhaseGenerationGate) -> Result<LaunchPermit, String>;

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

    pub fn hook(self: Arc<Self>) -> AttributeHook {
        Box::new(move |spec| self.authorize(spec))
    }

    pub fn launch_fence_hook(self: Arc<Self>) -> LaunchFenceHook {
        Box::new(move |spec| self.authorize_retained(spec))
    }

    pub fn authorize(&self, spec: &JobSpec) -> Result<(), HookError> {
        self.authorize_retained(spec)?.commit_and_release()
    }

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

    pub fn authorize_retained(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
        let context = self.verify_ci_job_signed(spec, CiCredentialExpectation::LegacyClaimBound)?;
        self.claim_gate
            .permit(context)
            .map_err(|error| HookError(format!("durable scheduler launch fence failed: {error}")))
    }

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

    pub fn authorize_checkout_advertise_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<LaunchPermit, HookError> {
        self.authorize_preparation_retained(spec, scope, CiCredentialPurpose::CheckoutAdvertise)
    }

    pub fn authorize_checkout_fetch_retained(
        &self,
        spec: &JobSpec,
        scope: &CheckoutAuthorizationScope,
    ) -> Result<LaunchPermit, HookError> {
        self.authorize_preparation_retained(spec, scope, CiCredentialPurpose::CheckoutFetch)
    }

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
    let claim_lifetime =
        u64::try_from(claim.claim_expires_at_epoch_secs - claim.claim_started_at_epoch_secs)
            .map_err(|_| refused("claim lifetime is outside the supported range"))?;
    let topology_ceiling = if authority.checkout.is_some() {
        crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS
    } else {
        crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS_U64
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

    #[derive(Default)]
    struct SpyClaimGateCalls {
        verify_live_calls: usize,
        permit_calls: usize,
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
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let minted_scope = checkout_scope();
        let substituted_workspace = WorkspaceSpec {
            repo_ref: checkout_workspace().repo_ref,
            commit: Some("c".repeat(40)),
        };
        let rederived_from_substituted_workspace =
            derive_checkout_authorization_scope(JobKind::Ci, &substituted_workspace)
                .unwrap()
                .unwrap();
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

        let missing = checkout_job(credential.clone(), &claim, checkout_workspace(), None);
        assert!(boundary.authorize_checkout(&missing, &scope).is_err());
        assert!(boundary.authorize(&missing).is_err());

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
        )
        .expect("record CI credential teardown");
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
        let (credential, claim, cell, s7) = checkout_test_rig().await;
        let scope = checkout_scope();
        let boundary = checkout_boundary(&cell, s7, Arc::new(AllowClaimGate));
        let spec = checkout_job(credential, &claim, checkout_workspace(), Some(&scope));
        let Some(RunTokenAuthorizationContext::CiJob(context)) =
            spec.run_token_authorization.as_ref()
        else {
            panic!("CI context")
        };
        assert_eq!(context.checkout_scope.as_ref(), Some(&scope));
        assert!(context
            .required_capabilities
            .contains(&"repo:myelin://acme/git/repo/widgets#pull".to_string()));
        boundary
            .authorize_checkout(&spec, &scope)
            .expect("checkout authorization succeeds");
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
        boundary
            .authorize(&spec)
            .expect("the real workload permit is still acquirable and committable");
    }

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
        s7.tear_down_run_token(&scope, &credential.jti, timestamp_from_epoch(NOW).unwrap())
            .expect("record CI credential teardown");
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

    #[tokio::test]
    async fn the_raw_minter_seam_enforces_the_topology_specific_claim_ceiling() {
        let store = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[5_u8; 32], &[6_u8; 32]).unwrap());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell).with_clock(|| NOW));
        let minter = RunTokenMinter::with_signer_and_tuples(store, None, signer);
        let adapter = IdentityCiJobCredentialMinter::new(minter).with_clock(|| NOW);

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

        adapter
            .mint_verified(over_execution_bound, checkout_authority())
            .await
            .expect("a checkout-bearing authority justifies a multi-execution claim");

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

    const ALL_PURPOSES: [CiCredentialPurpose; 4] = [
        CiCredentialPurpose::CheckoutAdvertise,
        CiCredentialPurpose::CheckoutFetch,
        CiCredentialPurpose::CheckoutMaterialization,
        CiCredentialPurpose::Workload,
    ];

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

    #[tokio::test]
    async fn v1_and_v2_credentials_never_satisfy_each_other_s_boundary() {
        let (adapter, claim, cell, s7) = phase_rig().await;
        let boundary = checkout_boundary(&cell, s7.clone(), Arc::new(AllowClaimGate));

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
        boundary
            .authorize_retained(&v1_spec)
            .expect("the shipped V1 path is untouched by this slice");
    }

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
