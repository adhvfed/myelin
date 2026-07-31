//! CT-007 slice 5b.3-6d STEP 3: the **dormant** control-plane composition for the sandbox checkout
//! orchestrator.
//!
//! This module supplies the REAL, durable implementations of the sandbox-side capability vocabulary
//! that CT-007 slice 5b.3-6c drove with FAKE authorities:
//!
//! - the V2 phase-credential store factory ([`v2_phase_credential_store`]);
//! - the V2 resolver seam ([`V2CheckoutComposition::mint_initial_phase_credential`] +
//!   [`initial_phase_purpose`]) — a checkout job's first generation is `CheckoutAdvertise`, a compute
//!   job's is `Workload`;
//! - the durable [`AttemptAuthority`] adapter ([`DurableAttemptAuthority`]) bridging the sandbox's SYNC
//!   trait onto the ASYNC control plane;
//! - the parent-attempt reserve hook ([`V2CheckoutComposition::parent_attempt_reserve_hook`]) mapping
//!   [`admit_parent_attempt`](crate::ci_prelaunch_usage_journal::CiPrelaunchUsageJournal::admit_parent_attempt)
//!   to the sandbox [`ParentAttemptAdmission`], in ONE tenant transaction.
//!
//! **Dormant by construction.** No production composition root constructs [`V2CheckoutComposition`] or
//! the V2 phase-store, and production `RunnerHooks` never install a [`ParentAttemptReserveHook`], so
//! the whole path stays unreachable until 5b.3-6e's single activating cutover. The credential-store
//! dormancy scan (`integration_ci_credential_generation.rs`) pins this file's exact marker occurrences
//! as new DEFINITION sites while every composition-root zero stays zero.

use std::sync::Arc;

use myelin_ci_sandbox::checkout_orchestration::{
    AttemptAuthority, AttemptAuthorityError, ParentAttemptAdmission, PhaseCredentialCarrier,
    WorkloadCredentialCarrier,
};
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CheckoutAuthorizationScope, CheckoutPhase, HookError,
    JobSpec, PreparationLeaseCheckpoint, PreparationLeaseLost, PreparationPhase, ReserveHandle,
    ResourceUsage, RunTokenAuthorizationContext,
};

use crate::ci_credential_generation::{
    CiCredentialPurpose, CiJobCredentialGenerationStore, CiJobCredentialWriteVersion,
    CiPhaseCredentialMinter, MintedPhaseCredential,
};
use crate::ci_identity_adapter::ci_job_phase_authorization_context;
use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_prelaunch_usage_journal::{
    CiJobParentAttempt, CiParentAttemptAdmission, CiPrelaunchUsageJournal, CiPrelaunchUsagePhase,
};
use crate::job_queue_store::{CiJobLaunchClaim, CiJobQueueStore};
// **The async→sync bridge is the crate's ONE shared off-runtime helper** (`runner_bind::bridge`) — the
// SAME one `DurableLeaseAdapter`/`DurablePreparationLeaseCheckpoint` use, NOT a fork. Its precondition
// (never a current-thread Tokio runtime; drive on a dedicated OS thread) holds on BOTH sync entry
// points, which run on two DISTINCT off-runtime OS threads: (1) the parent-attempt reserve hook +
// initial-credential resolution run on the runner's own `CiRunnerLoop::spawn` OS thread (inside
// `run_one`), and (2) the `DurableAttemptAuthority` phase/lease/mint calls run on the sandbox's
// scoped-launch OS thread (`std::thread::scope`, off the async runtime) into which the backend drives
// the checkout orchestrator. Neither is a Tokio worker, so `block_on` runs directly and the
// `try_current`/`block_in_place` fallback (which would PANIC on a current-thread runtime) is only ever
// taken by the multi-thread live-PG tests that legally call these helpers on a Tokio worker.
use crate::runner_bind::{bridge, DurablePreparationLeaseCheckpoint};

/// The V2 phase-credential store factory — the ONLY sanctioned way this slice constructs a
/// `V2PhaseBound` generation store, pairing the durable insert-or-replay store with the phase minter.
/// Production passes `Arc::new(`[`IdentityCiJobCredentialMinter`](crate::ci_identity_adapter::IdentityCiJobCredentialMinter)`::new(..))`
/// (the SAME minter type composition already builds at `ci_runner_composition.rs:734` for V1); the
/// trait-object seam lets a live-PG test inject a call-counting wrapper to prove Identity invocation.
pub fn v2_phase_credential_store(
    pool: sqlx::PgPool,
    region: impl Into<String>,
    minter: Arc<dyn CiPhaseCredentialMinter>,
) -> CiJobCredentialGenerationStore {
    CiJobCredentialGenerationStore::with_pg_and_write_version(
        pool,
        region,
        minter,
        CiJobCredentialWriteVersion::V2PhaseBound,
    )
}

/// **The V2 resolver seam's SELECTION rule.** A checkout-bearing job's first (resolver-minted)
/// generation is `CheckoutAdvertise`; a compute job's is `Workload`. Both are mintable at claim
/// resolution before any parent attempt — the store admits advertise (its parent/journal gates live
/// in the execution boundary) and a compute job's workload as the FIRST generation of its claim.
pub fn initial_phase_purpose(checkout: Option<&CheckoutAuthorizationScope>) -> CiCredentialPurpose {
    match checkout {
        Some(_) => CiCredentialPurpose::CheckoutAdvertise,
        None => CiCredentialPurpose::Workload,
    }
}

/// The sandbox preparation phase → durable journal phase mapping — the ONE place the two vocabularies
/// meet, so a phase can never be journaled through a hand-written mismatched token.
fn journal_phase(phase: PreparationPhase) -> CiPrelaunchUsagePhase {
    match phase {
        PreparationPhase::CheckoutTransport => CiPrelaunchUsagePhase::CheckoutTransport,
        PreparationPhase::CheckoutMaterialization => CiPrelaunchUsagePhase::CheckoutMaterialization,
    }
}

/// The sandbox checkout phase → durable credential purpose mapping (preparation purposes only; the
/// workload is a SEPARATE mint, so `CheckoutPhase` — which structurally excludes the workload —
/// maps only onto the three preparation purposes).
fn phase_purpose(phase: CheckoutPhase) -> CiCredentialPurpose {
    match phase {
        CheckoutPhase::Advertise => CiCredentialPurpose::CheckoutAdvertise,
        CheckoutPhase::Fetch => CiCredentialPurpose::CheckoutFetch,
        CheckoutPhase::Materialization => CiCredentialPurpose::CheckoutMaterialization,
    }
}

/// **Reconstruct the exact durable claim from a resolved [`JobSpec`]'s phase authorization context.**
///
/// The [`ParentAttemptReserveHook`] receives only `&JobSpec`, but
/// [`admit_parent_attempt`](CiPrelaunchUsageJournal::admit_parent_attempt) needs the full
/// [`CiJobTokenRequest`] — including `ci_run_id`, `token_authority_handle`, and `idem_token`, which the
/// base [`CiJobAuthorizationContext`](myelin_ci_sandbox::CiJobAuthorizationContext) does NOT itself
/// carry. They live on the V2 resolver's [`CiJobCredentialBinding`](myelin_ci_sandbox::CiJobCredentialBinding),
/// which `ci_job_phase_authorization_context` installs at resolve time. Every one of the 12
/// `CiJobTokenRequest` fields is present between the context and its binding, so this reconstruction is
/// EXACT — proven by `claim.validate()` here and by the durable admission re-verifying the claim under
/// the queue row lock. `reserve_handle` comes separately from `spec.meter_to.reserve_id`.
///
/// A spec resolved under the legacy V1 shape (no binding) is refused — this seam is only reachable for
/// a V2-resolved checkout/compute job.
///
/// **The spec is fully cross-checked against the context, not merely projected from it** (Sol's r1
/// blocker 1). The trusted authority is `spec.workspace` + `spec.kind` + `spec.idem_token` — the
/// caller-facing execution surface. So this:
/// 1. re-derives the checkout scope from `spec.workspace` via the ONE sanctioned facade and requires
///    EXACT equality with the context's `checkout_scope` (a substituted commit from the same repo
///    would carry the same `repo:<ref>#pull` capability, so scope equality — not capability presence —
///    is what closes the substitution the checkout-authorization chain exists to prevent);
/// 2. requires the binding's `purpose` to be the EXPECTED INITIAL purpose for this job shape
///    (`CheckoutAdvertise` for a checkout job, `Workload` for a compute job) — a workload binding can
///    never be smuggled into a checkout job's admission and vice versa;
/// 3. requires the binding's `idem_token` to equal `spec.idem_token` — the reconstructed claim's idem
///    is fused to the exact dispatched job, not merely echoed from a claimed context.
fn reconstruct_claim(
    spec: &JobSpec,
) -> Result<(CiJobTokenRequest, Option<CheckoutAuthorizationScope>), HookError> {
    let context = match &spec.run_token_authorization {
        Some(RunTokenAuthorizationContext::CiJob(context)) => context,
        None => {
            return Err(HookError(
                "V2 parent-attempt admission requires a resolved CI-job authorization context".into(),
            ))
        }
    };
    let binding = context.credential_binding.as_ref().ok_or_else(|| {
        HookError(
            "V2 parent-attempt admission requires a V2 phase-credential binding (the reconstructed \
             claim's ci_run_id/token_authority_handle/idem_token live there); a legacy V1 context is \
             refused"
                .into(),
        )
    })?;
    // (1) Re-derive the checkout scope from the spec's OWN workspace and require exact equality with
    // the context — never trust the context's scope on its own.
    let derived_scope = derive_checkout_authorization_scope(spec.kind, &spec.workspace)
        .map_err(|reason| HookError(format!("deriving the spec's checkout scope failed: {reason}")))?;
    if derived_scope != context.checkout_scope {
        return Err(HookError(
            "V2 parent-attempt admission refused: the checkout scope derived from the spec's \
             workspace does not equal the resolved authorization context's scope (substitution)"
                .into(),
        ));
    }
    // (2) The binding must be the EXPECTED INITIAL generation for this job shape.
    let expected_initial = initial_phase_purpose(derived_scope.as_ref());
    if CiCredentialPurpose::from_token(&binding.purpose) != Some(expected_initial) {
        return Err(HookError(format!(
            "V2 parent-attempt admission refused: the phase-credential binding purpose `{}` is not \
             this job shape's expected initial purpose `{}`",
            binding.purpose,
            expected_initial.token()
        )));
    }
    // (3) The binding's idem token must be exactly the dispatched job's.
    if binding.idem_token != spec.idem_token.0 {
        return Err(HookError(
            "V2 parent-attempt admission refused: the phase-credential binding's idem token does not \
             equal the dispatched spec's idem token"
                .into(),
        ));
    }
    let claim = CiJobTokenRequest {
        tenant_id: context.tenant_id.clone(),
        region: context.region.clone(),
        wf_run_id: context.wf_run_id.clone(),
        ci_run_id: binding.ci_run_id.clone(),
        job_id: context.job_id.clone(),
        token_authority_handle: binding.token_authority_handle.clone(),
        idem_token: binding.idem_token.clone(),
        lease_owner: context.lease_owner.clone(),
        lease_epoch: context.lease_epoch,
        claim_nonce: context.claim_nonce.clone(),
        claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
    };
    claim.validate().map_err(|error| {
        HookError(format!(
            "reconstructed V2 claim failed validation (the resolved context is malformed): {}",
            error.0
        ))
    })?;
    Ok((claim, derived_scope))
}

/// The exact durable claim generation → [`CiJobLaunchClaim`] projection the preparation-lease
/// checkpoint renews against.
fn launch_claim(claim: &CiJobTokenRequest) -> CiJobLaunchClaim {
    CiJobLaunchClaim {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

/// **The dormant durable composition for one region's checkout orchestration.** Holds the shared
/// journal + phase-credential store + queue store + runtime handle; hands out the parent-attempt
/// reserve hook and the initial-credential resolver seam. Constructed by NO production composition
/// root (5b.3-6e does that).
#[derive(Clone)]
pub struct V2CheckoutComposition {
    journal: CiPrelaunchUsageJournal,
    credential_store: CiJobCredentialGenerationStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
}

impl V2CheckoutComposition {
    /// Compose the durable authorities from one region's pool + Identity minter + queue store.
    pub fn new(
        pool: sqlx::PgPool,
        region: impl Into<String>,
        minter: Arc<dyn CiPhaseCredentialMinter>,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, HookError> {
        let region = region.into();
        let journal = CiPrelaunchUsageJournal::new(pool.clone(), region.clone())
            .map_err(|error| HookError(format!("V2 checkout composition refused: {error}")))?;
        let credential_store = v2_phase_credential_store(pool, region, minter);
        Ok(Self {
            journal,
            credential_store,
            queue_store,
            rt,
        })
    }

    /// **The V2 resolver seam.** Mint (or replay) the job's INITIAL phase credential — `CheckoutAdvertise`
    /// for a checkout-bearing job, `Workload` for a compute job (see [`initial_phase_purpose`]) — and
    /// return it alongside the phase authorization context the resolved [`JobSpec`] carries. The
    /// caller rotates both into the spec (`resolve_with_authorization`), which the parent-attempt hook
    /// later reconstructs the claim from.
    pub fn mint_initial_phase_credential(
        &self,
        claim: &CiJobTokenRequest,
        checkout: Option<&CheckoutAuthorizationScope>,
    ) -> Result<(MintedPhaseCredential, RunTokenAuthorizationContext), AttemptAuthorityError> {
        let purpose = initial_phase_purpose(checkout);
        let minted = bridge(&self.rt, self.credential_store.mint_phase_credential(claim, purpose))
            .map_err(|error| AttemptAuthorityError(error.to_string()))?;
        let context = ci_job_phase_authorization_context(claim, checkout, &minted.binding);
        Ok((minted, context))
    }

    /// **The parent-attempt reserve hook.** Reconstructs the exact claim from the resolved spec,
    /// takes `reserve_handle` from `spec.meter_to.reserve_id`, and drives
    /// [`admit_parent_attempt`](CiPrelaunchUsageJournal::admit_parent_attempt) — reservation
    /// `reserved → inflight` and parent-row insertion in ONE tenant transaction. On admission it
    /// hands back a real [`DurableAttemptAuthority`]; on exhaustion the settleable reserve for the
    /// terminal `AttemptsExhausted` report.
    pub fn parent_attempt_reserve_hook(&self) -> myelin_ci_sandbox::ParentAttemptReserveHook {
        let this = self.clone();
        Box::new(move |spec: &JobSpec| this.admit(spec))
    }

    fn admit(&self, spec: &JobSpec) -> Result<ParentAttemptAdmission, HookError> {
        let (claim, checkout) = reconstruct_claim(spec)?;
        let reserve_handle = spec.meter_to.reserve_id.clone();
        match bridge(
            &self.rt,
            self.journal.admit_parent_attempt(&claim, &reserve_handle),
        )
        .map_err(|error| HookError(format!("V2 parent-attempt admission refused: {error}")))?
        {
            CiParentAttemptAdmission::Admitted { attempt, .. } => {
                let lease_checkpoint = DurablePreparationLeaseCheckpoint::new(
                    self.queue_store.clone(),
                    launch_claim(&claim),
                    self.rt.clone(),
                );
                let authority = DurableAttemptAuthority {
                    journal: self.journal.clone(),
                    credential_store: self.credential_store.clone(),
                    lease_checkpoint,
                    attempt,
                    claim,
                    checkout,
                    reserve_handle: reserve_handle.clone(),
                    rt: self.rt.clone(),
                };
                Ok(ParentAttemptAdmission::Admitted {
                    reserve: ReserveHandle(reserve_handle),
                    attempt_authority: Box::new(authority),
                })
            }
            CiParentAttemptAdmission::AttemptsExhausted { reserve_handle } => {
                Ok(ParentAttemptAdmission::AttemptsExhausted {
                    reserve: ReserveHandle(reserve_handle),
                })
            }
        }
    }
}

/// **The REAL durable [`AttemptAuthority`] for one admitted checkout attempt.** Every sandbox trait
/// method bridges onto its durable backing:
///
/// | method | durable backing |
/// |---|---|
/// | `begin_phase` / `complete_phase` / `seal_phase` | [`CiPrelaunchUsageJournal`] |
/// | `renew_preparation_lease` | [`DurablePreparationLeaseCheckpoint`] |
/// | `mint_phase_credential` / `mint_workload_credential` | [`CiJobCredentialGenerationStore::mint_phase_credential`] |
/// | `should_requeue` | [`CiPrelaunchUsageJournal::parent_attempt_retry_permitted`] (live `count < max`) |
///
/// The carrier's ephemeral authorization context is built by `ci_job_phase_authorization_context`
/// from the exact reconstructed `claim` + the durable binding the mint returned.
struct DurableAttemptAuthority {
    journal: CiPrelaunchUsageJournal,
    credential_store: CiJobCredentialGenerationStore,
    lease_checkpoint: DurablePreparationLeaseCheckpoint,
    attempt: CiJobParentAttempt,
    claim: CiJobTokenRequest,
    checkout: Option<CheckoutAuthorizationScope>,
    reserve_handle: String,
    rt: tokio::runtime::Handle,
}

impl DurableAttemptAuthority {
    /// Mint one preparation OR workload generation and build its carrier parts. The workload/checkout
    /// carrier split is enforced by the callers passing the matching purpose; both share this exact
    /// mint + context construction so a generation always carries its own binding, never a stale one.
    fn mint(
        &self,
        purpose: CiCredentialPurpose,
    ) -> Result<(myelin_ci_sandbox::RunTokenCredential, RunTokenAuthorizationContext, String), AttemptAuthorityError>
    {
        let minted = bridge(
            &self.rt,
            self.credential_store
                .mint_phase_credential(&self.claim, purpose),
        )
        .map_err(|error| AttemptAuthorityError(error.to_string()))?;
        let context =
            ci_job_phase_authorization_context(&self.claim, self.checkout.as_ref(), &minted.binding);
        Ok((minted.credential, context, minted.binding.generation_id))
    }
}

impl AttemptAuthority for DurableAttemptAuthority {
    fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal.begin_phase(&self.attempt, journal_phase(phase)),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn complete_phase(
        &self,
        phase: PreparationPhase,
        usage: ResourceUsage,
    ) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal
                .complete_phase(&self.attempt, journal_phase(phase), usage),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal.seal_phase(&self.attempt, journal_phase(phase)),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost> {
        // Delegates to the SAME already-proven durable checkpoint the transport/preparation legs
        // accept; a DB error is treated as lost ownership (fail-closed), never as success.
        PreparationLeaseCheckpoint::renew(&self.lease_checkpoint)
    }

    fn mint_phase_credential(
        &self,
        phase: CheckoutPhase,
    ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError> {
        let (credential, context, generation_id) = self.mint(phase_purpose(phase))?;
        Ok(PhaseCredentialCarrier::new(credential, context, generation_id))
    }

    fn mint_workload_credential(&self) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError> {
        let (credential, context, generation_id) = self.mint(CiCredentialPurpose::Workload)?;
        Ok(WorkloadCredentialCarrier::new(credential, context, generation_id))
    }

    fn should_requeue(&self) -> bool {
        // The live `count < max` policy. A durable error fails CLOSED to "exhausted" (terminalize
        // rather than requeue-forever): the durable requeue CAS would re-verify anyway, and the
        // conservative branch never strands a job in an endless requeue loop.
        bridge(
            &self.rt,
            self.journal
                .parent_attempt_retry_permitted(&self.claim, &self.reserve_handle),
        )
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::{
        CiJobAuthorizationContext, CiJobCredentialBinding, EgressPolicy, ImageRef, IdemToken, JobKind,
        MeterTarget, ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
    };

    fn checkout_workspace() -> WorkspaceSpec {
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".into()),
            commit: Some("a".repeat(40)),
        }
    }

    fn job_spec(
        workspace: WorkspaceSpec,
        context: Option<RunTokenAuthorizationContext>,
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
            RunTokenCredential::new("bearer", "advertise-jti", 300).unwrap(),
            MeterTarget {
                reserve_id: "ci-reserve:v2:reserve-1".into(),
            },
            IdemToken("11111111-1111-1111-1111-111111111111/build".into()),
        )
        .unwrap();
        spec.run_token_authorization = context;
        spec
    }

    fn checkout_scope() -> CheckoutAuthorizationScope {
        derive_checkout_authorization_scope(JobKind::Ci, &checkout_workspace())
            .expect("scope derives")
            .expect("the checkout workspace is checkout-bearing")
    }

    /// A CONSISTENT V2 checkout context: its `checkout_scope` equals the scope derived from the
    /// checkout workspace, its binding purpose is the initial `checkout_advertise`, and its binding
    /// idem token equals the spec's idem token — exactly what `reconstruct_claim` now cross-checks.
    fn v2_context() -> CiJobAuthorizationContext {
        CiJobAuthorizationContext {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            principal_id: "ci-job".into(),
            wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
            job_id: "22222222-2222-2222-2222-222222222222".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 7,
            claim_nonce: "33333333-3333-3333-3333-333333333333".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
            required_capabilities: vec![],
            checkout_scope: Some(checkout_scope()),
            credential_binding: Some(CiJobCredentialBinding {
                binding_version: 1,
                purpose: "checkout_advertise".into(),
                generation_id: "cigen:abc".into(),
                issued_at_epoch_secs: 1_000,
                expires_at_epoch_secs: 1_300,
                ci_run_id: "44444444-4444-4444-4444-444444444444".into(),
                token_authority_handle: "tah-xyz".into(),
                idem_token: "11111111-1111-1111-1111-111111111111/build".into(),
            }),
        }
    }

    fn spec_with_context(context: Option<RunTokenAuthorizationContext>) -> JobSpec {
        job_spec(checkout_workspace(), context)
    }

    #[test]
    fn reconstruct_claim_recovers_every_field_from_the_context_and_binding() {
        let context = v2_context();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context.clone())));
        let (claim, _checkout) = reconstruct_claim(&spec).expect("reconstructs an exact claim");
        // The context supplies the base claim identity...
        assert_eq!(claim.tenant_id, context.tenant_id);
        assert_eq!(claim.region, context.region);
        assert_eq!(claim.wf_run_id, context.wf_run_id);
        assert_eq!(claim.job_id, context.job_id);
        assert_eq!(claim.lease_owner, context.lease_owner);
        assert_eq!(claim.lease_epoch, context.lease_epoch);
        assert_eq!(claim.claim_nonce, context.claim_nonce);
        assert_eq!(
            claim.claim_started_at_epoch_secs,
            context.claim_started_at_epoch_secs
        );
        assert_eq!(
            claim.claim_expires_at_epoch_secs,
            context.claim_expires_at_epoch_secs
        );
        // ...and the three binding-only fields the base context does NOT carry come from the binding.
        let binding = context.credential_binding.as_ref().unwrap();
        assert_eq!(claim.ci_run_id, binding.ci_run_id);
        assert_eq!(claim.token_authority_handle, binding.token_authority_handle);
        assert_eq!(claim.idem_token, binding.idem_token);
        // The reconstructed claim is well-formed (the seam validates before use).
        claim.validate().expect("the reconstructed claim validates");
    }

    #[test]
    fn reconstruct_claim_refuses_a_legacy_v1_context_without_a_binding() {
        let mut context = v2_context();
        context.credential_binding = None;
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(reconstruct_claim(&spec).is_err(), "a V1 shape has no claim identity to reconstruct");
    }

    #[test]
    fn reconstruct_claim_refuses_a_missing_context() {
        let spec = spec_with_context(None);
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn reconstruct_claim_refuses_a_scope_substitution() {
        // The context claims a checkout scope, but the spec's OWN workspace is a plain compute
        // workspace — so the scope derived from the spec (None) does NOT equal the context's scope.
        // A substituted spec must be refused before any durable admission.
        let context = v2_context();
        let spec = job_spec(
            WorkspaceSpec::default(),
            Some(RunTokenAuthorizationContext::CiJob(context)),
        );
        assert!(
            reconstruct_claim(&spec).is_err(),
            "a spec whose workspace disagrees with the context's scope is a substitution and must refuse"
        );
    }

    #[test]
    fn reconstruct_claim_refuses_a_binding_purpose_that_is_not_the_initial() {
        // A workload binding smuggled into a checkout job's context (its expected initial purpose is
        // advertise) must refuse.
        let mut context = v2_context();
        context.credential_binding.as_mut().unwrap().purpose = "workload".into();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn reconstruct_claim_refuses_an_idem_token_mismatch() {
        // The binding's idem token must equal the dispatched spec's; a mismatch is refused.
        let mut context = v2_context();
        context.credential_binding.as_mut().unwrap().idem_token =
            "11111111-1111-1111-1111-111111111111/other".into();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn initial_phase_purpose_selects_advertise_for_checkout_and_workload_for_compute() {
        let scope = myelin_ci_sandbox::derive_checkout_authorization_scope(
            JobKind::Ci,
            &checkout_workspace(),
        )
        .expect("scope derives")
        .expect("the test spec is checkout-bearing");
        assert_eq!(
            initial_phase_purpose(Some(&scope)),
            CiCredentialPurpose::CheckoutAdvertise
        );
        assert_eq!(initial_phase_purpose(None), CiCredentialPurpose::Workload);
    }

    #[test]
    fn phase_and_purpose_mappings_are_total_and_disjoint() {
        assert_eq!(
            journal_phase(PreparationPhase::CheckoutTransport),
            CiPrelaunchUsagePhase::CheckoutTransport
        );
        assert_eq!(
            journal_phase(PreparationPhase::CheckoutMaterialization),
            CiPrelaunchUsagePhase::CheckoutMaterialization
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Advertise),
            CiCredentialPurpose::CheckoutAdvertise
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Fetch),
            CiCredentialPurpose::CheckoutFetch
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Materialization),
            CiCredentialPurpose::CheckoutMaterialization
        );
    }
}
