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
    CiJobAuthorizationContext, HookError, JobKind, JobSpec, LaunchFenceHook, LaunchOwnership,
    LaunchPermit, RunTokenAuthorizationContext, RunTokenCredential, ValidatedLaunchOwnership,
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
use crate::ci_launch_authority::CiJobRuntimeAuthorityRequest;
use crate::ci_manifest_job_runner::{
    CiJobTokenIssueError, CiJobTokenRequest, MAX_CI_JOB_TOKEN_TTL_SECS,
};
use crate::{CiJobLaunchClaim, CiJobQueueStore};

/// The platform CI service principal cryptographically bound into every hosted-job token.
pub const CI_JOB_PRINCIPAL_ID: &str = "svc:ci";

/// Narrow authority the hosted runner requires at this boundary. Broader Git/check mutation remains
/// behind its own public-endpoint authorization rather than being implied by sandbox launch.
pub const CI_JOB_REQUIRED_CAPABILITIES: [&str; 2] = ["job.launch", "artifact.write"];

/// CT-007 slice 5b.3-2c: the ONE place the exact-repo capability grant is derived, shared by
/// minting, context construction, and verification, so all three can never disagree about what a
/// checkout-bearing job's capability vector should be. Compute jobs (`checkout: None`) get exactly
/// the original two capabilities, unchanged. A checkout-bearing job additionally gets
/// `repo:<canonical ArtifactRef>#pull` (Sol's review: the full canonical ref, never a bare
/// `repo_id`, so the grant is unambiguous without relying on capability evaluation to separately
/// incorporate tenant). This capability proves repo-READ authority only -- NOT the exact commit;
/// the exact commit is bound separately via `CiJobAuthorizationContext.checkout_scope`.
fn required_ci_capabilities(checkout: Option<&CheckoutAuthorizationScope>) -> Vec<String> {
    let mut capabilities: Vec<String> = CI_JOB_REQUIRED_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect();
    if let Some(scope) = checkout {
        capabilities.push(format!("repo:{}#pull", scope.repo_ref().0));
    }
    capabilities
}

/// Build the non-secret expected facts carried from a locked scheduler claim to final launch.
/// `checkout` must be the SAME checkout scope (or lack thereof) the claim was actually minted
/// against (CT-007 slice 5b.3-2c) -- see callers for how each derives it.
pub fn ci_job_authorization_context(
    claim: &CiJobTokenRequest,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> RunTokenAuthorizationContext {
    RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        principal_id: CI_JOB_PRINCIPAL_ID.into(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        required_capabilities: required_ci_capabilities(checkout),
        checkout_scope: checkout.cloned(),
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
            let task = tokio::task::spawn_blocking(move || {
                mint_claim_credential(minter, mint_claim, checkout.as_ref())
            });
            task.await
                .map_err(|_| refused("Identity mint worker terminated"))?
        })
    }
}

fn mint_claim_credential(
    minter: RunTokenMinter,
    claim: CiJobTokenRequest,
    checkout: Option<&CheckoutAuthorizationScope>,
) -> Result<RunTokenCredential, CiJobTokenIssueError> {
    let token_expires_at = deterministic_token_expiry(&claim)?;
    let lifetime_secs = u64::try_from(token_expires_at - claim.claim_started_at_epoch_secs)
        .map_err(|_| refused("claim token lifetime is outside the supported range"))?;
    let minted_at = timestamp_from_epoch(claim.claim_started_at_epoch_secs)?;
    let principal = ci_principal(&claim.tenant_id, &claim.region);
    let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
    let required_capabilities = required_ci_capabilities(checkout);
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

trait CiJobLaunchClaimGate: Send + Sync {
    fn permit(&self, context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String>;

    /// CT-007 slice 5b.3-2c: confirm the durable claim is still live (`state = 'leased'`, every
    /// generation fact matching) WITHOUT performing the `leased -> running` CAS `permit` commits.
    /// The pre-Hop-A checkout-authorization hook uses this — it must never itself transition the
    /// real workload's state.
    fn verify_live(&self, context: &CiJobAuthorizationContext) -> Result<(), String>;
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
    /// review). Checks: CI job kind + context shape, the EXACT capability vector (the fixed two
    /// PLUS, only if the job is checkout-bearing, the one matching `repo:<ref>#pull`), that the
    /// checkout scope re-derived from the in-hand `spec.workspace` agrees EXACTLY with what the
    /// server-resolved authorization context claims (catching a commit substituted after mint even
    /// though the repo-read capability alone would not), the cryptographic bearer (JTI/tenant/
    /// region/principal/job/capabilities all bound into Identity's own verification), and that the
    /// signed credential's expiry never outlives the durable claim. Does NOT check durable claim
    /// liveness or perform any
    /// state transition -- callers do that themselves (`permit`'s CAS, or `verify_live`'s read-only
    /// check).
    fn verify_ci_job_signed<'a>(
        &self,
        spec: &'a JobSpec,
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
        let required = required_ci_capabilities(rederived_checkout.as_ref());
        if context.principal_id != CI_JOB_PRINCIPAL_ID
            || context.required_capabilities != required
            || context.checkout_scope != rederived_checkout
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
                &context.job_id,
                &token,
                &required,
            )
            .map_err(|error| HookError(format!("Identity refused CI launch: {error}")))?;
        if verified.exp_unix > context.claim_expires_at_epoch_secs {
            return Err(HookError(
                "signed CI credential outlives its durable scheduler claim".into(),
            ));
        }
        Ok(context)
    }

    /// Reauthorize signed facts and return a lazy durable permit. The exact CAS runs only after the
    /// sandbox launch guard is spawned and armed.
    pub fn authorize_retained(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
        let context = self.verify_ci_job_signed(spec)?;
        self.claim_gate
            .permit(context)
            .map_err(|error| HookError(format!("durable scheduler launch fence failed: {error}")))
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
        let context = self.verify_ci_job_signed(spec)?;
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
    {
        return Err(refused(
            "durable CI authority does not match the scheduler claim",
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
    }

    fn claim() -> CiJobTokenRequest {
        CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "eu-west".into(),
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
        spec.run_token_authorization = Some(ci_job_authorization_context(claim, None));
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
        spec.run_token_authorization = Some(ci_job_authorization_context(claim, checkout));
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
    fn compute_mint_capabilities_are_exactly_the_original_two() {
        assert_eq!(
            required_ci_capabilities(None),
            vec!["job.launch".to_string(), "artifact.write".to_string()]
        );
    }

    #[test]
    fn checkout_mint_capabilities_add_exactly_one_repo_pull_capability() {
        let caps = required_ci_capabilities(Some(&checkout_scope()));
        assert_eq!(
            caps,
            vec![
                "job.launch".to_string(),
                "artifact.write".to_string(),
                "repo:myelin://acme/git/repo/widgets#pull".to_string(),
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
        assert_eq!(
            required_ci_capabilities(Some(&scope)),
            required_ci_capabilities(Some(&checkout_scope())),
            "the repo capability names only the repo, never the commit"
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
                    caps.pop();
                }),
            ),
            (
                "wrong repo capability",
                Box::new(|caps: &mut Vec<String>| {
                    *caps.last_mut().unwrap() = "repo:myelin://acme/git/repo/other#pull".into();
                }),
            ),
            (
                "duplicate repo capability",
                Box::new(|caps: &mut Vec<String>| {
                    let last = caps.last().unwrap().clone();
                    caps.push(last);
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
            START + crate::runner_bind::CI_RUNNER_LEASE_TTL_SECS;

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
}
