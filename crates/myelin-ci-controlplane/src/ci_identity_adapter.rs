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
    AttributeHook, CiJobAuthorizationContext, HookError, JobKind, JobSpec, LaunchFenceHook,
    LaunchOwnership, LaunchPermit, RunTokenAuthorizationContext, RunTokenCredential,
    ValidatedLaunchOwnership,
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

/// Build the non-secret expected facts carried from a locked scheduler claim to final launch.
pub fn ci_job_authorization_context(claim: &CiJobTokenRequest) -> RunTokenAuthorizationContext {
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
        required_capabilities: CI_JOB_REQUIRED_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect(),
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
            let task =
                tokio::task::spawn_blocking(move || mint_claim_credential(minter, mint_claim));
            task.await
                .map_err(|_| refused("Identity mint worker terminated"))?
        })
    }
}

fn mint_claim_credential(
    minter: RunTokenMinter,
    claim: CiJobTokenRequest,
) -> Result<RunTokenCredential, CiJobTokenIssueError> {
    let token_expires_at = deterministic_token_expiry(&claim)?;
    let lifetime_secs = u64::try_from(token_expires_at - claim.claim_started_at_epoch_secs)
        .map_err(|_| refused("claim token lifetime is outside the supported range"))?;
    let minted_at = timestamp_from_epoch(claim.claim_started_at_epoch_secs)?;
    let principal = ci_principal(&claim.tenant_id, &claim.region);
    let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
    let authority = Authority::of(CI_JOB_REQUIRED_CAPABILITIES);
    let input = DelegationInput {
        agent_policy: authority.clone(),
        delegation: authority.clone(),
        tenant_policy: authority.clone(),
        trigger_actor_held: authority,
    };
    let caveats = DelegationCaveats(
        CI_JOB_REQUIRED_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect(),
    );
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

trait CiJobLaunchClaimGate: Send + Sync {
    fn permit(&self, context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String>;
}

#[derive(Clone)]
struct PgCiJobLaunchClaimGate {
    store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
}

impl CiJobLaunchClaimGate for PgCiJobLaunchClaimGate {
    fn permit(&self, context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
        let claim = CiJobLaunchClaim {
            tenant_id: context.tenant_id.clone(),
            region: context.region.clone(),
            wf_run_id: context.wf_run_id.clone(),
            job_id: context.job_id.clone(),
            lease_owner: context.lease_owner.clone(),
            lease_epoch: context.lease_epoch,
            claim_nonce: context.claim_nonce.clone(),
            claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
        };
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

    /// Reauthorize signed facts and return a lazy durable permit. The exact CAS runs only after the
    /// sandbox launch guard is spawned and armed.
    pub fn authorize_retained(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
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
        let required: Vec<String> = CI_JOB_REQUIRED_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect();
        if context.principal_id != CI_JOB_PRINCIPAL_ID
            || context.required_capabilities != required
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
        self.claim_gate
            .permit(context)
            .map_err(|error| HookError(format!("durable scheduler launch fence failed: {error}")))
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
    }

    struct RefuseClaimGate;

    impl CiJobLaunchClaimGate for RefuseClaimGate {
        fn permit(&self, _context: &CiJobAuthorizationContext) -> Result<LaunchPermit, String> {
            Ok(LaunchPermit::retained(|| {
                Err(HookError("durable scheduler claim was refused".into()))
            }))
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
        spec.run_token_authorization = Some(ci_job_authorization_context(claim));
        spec
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
