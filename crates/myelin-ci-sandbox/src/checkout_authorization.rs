//! CT-007 slice 5b.3-2a: the [`CheckoutAuthorizationProof`] capability and the
//! [`crate::RunnerHooks::authorize_checkout`] implementation that mints it; plus CT-007's
//! phase-credential [`PhaseAuthorization`].
//!
//! Deliberately its OWN sibling module of the crate root (never inline in `lib.rs`, Sol's review):
//! Rust's privacy rules make a private field visible to every DESCENDANT module of its defining
//! module. If these types were defined at the crate root, `crate::gvisor` (a descendant of the crate
//! root, like every module in this crate) could forge one via a bare struct literal, bypassing the
//! hook entirely and defeating the whole capability guarantee. Living here instead means only THIS
//! module — the one that actually invokes the hooks — can ever construct one; `crate::gvisor` and
//! everything else can only CONSUME an already-minted capability.

use crate::workspace_intent::ExpectedGitCommitId;
use crate::{
    CheckoutAuthorizationScope, HookError, JobSpec, LaunchPermit, RunTokenCredential, RunnerHooks,
};

/// **CT-007 phase-credential generations: which preparation boundary an authorization was minted
/// at.** The sandbox's own vocabulary — it deliberately does NOT know the control plane's `workload`
/// purpose, because no sandbox preparation API may ever be driven by a workload credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckoutPhase {
    Advertise,
    Fetch,
    Materialization,
}

impl CheckoutPhase {
    /// The exact durable purpose vocabulary token the control plane persists.
    pub fn purpose_token(self) -> &'static str {
        match self {
            Self::Advertise => "checkout_advertise",
            Self::Fetch => "checkout_fetch",
            Self::Materialization => "checkout_materialization",
        }
    }
}

/// An unforgeable, one-shot proof that `RunnerHooks::authorize_checkout` genuinely succeeded for an
/// EXACT [`CheckoutAuthorizationScope`] AND an exact token generation (CT-007 slice 5b.3-2a, Sol's
/// review): binding the `run_token.jti` the authorization was actually checked against, not just the
/// scope, is what prevents a proof minted for one claim generation from being detached and paired
/// with a DIFFERENT attempt's transport/accounting inputs. Fields are private to this module — the
/// only way to obtain one is a real successful `authorize_checkout` call.
///
/// This is the LEGACY (V1 claim-bound) capability and is deliberately UNCHANGED by the
/// phase-credential slice: the V2 boundaries take [`PhaseAuthorization`] instead, and no V2 entry
/// point accepts this type at all.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct CheckoutAuthorizationProof {
    scope: CheckoutAuthorizationScope,
    run_token_jti: String,
}

impl CheckoutAuthorizationProof {
    #[allow(dead_code)]
    pub(crate) fn scope(&self) -> &CheckoutAuthorizationScope {
        &self.scope
    }

    #[allow(dead_code)]
    pub(crate) fn run_token_jti(&self) -> &str {
        &self.run_token_jti
    }
}

/// **CT-007 round-1 blocker 2: ONE opaque, non-constructible object that owns the proof AND the
/// permit from the SAME hook invocation.**
///
/// The earlier shape returned `(proof, permit)` as a detachable pair, so nothing proved the two came
/// from the same invocation, claim, or generation. The concrete bypass that enabled: obtain a
/// still-cryptographically-valid proof for requeued/superseded claim A and a live permit for claim
/// B, then present A's proof/credential with B's permit — the proof checks A, the permit
/// independently authorizes B, and A's container spawns.
///
/// This type closes that by construction:
///
/// - fields are private to this module and there is NO public constructor — only
///   [`RunnerHooks::authorize_checkout_phase`] builds one, from one hook invocation;
/// - it is deliberately **not** `Clone`, so an authorization cannot be duplicated across legs;
/// - the permit is unreachable except through a CONSUMING `into_*_permit` method that first
///   re-verifies the phase, the retained run-token JTI, and the scope against the request actually
///   being authorized. Proof and permit therefore cannot be separated at all: you cannot obtain the
///   permit without passing the proof's own checks against the same inputs.
///
/// `run_token_jti` is the JTI of the `JobSpec` the hook was invoked with. Every consumption compares
/// the caller's in-hand credential against it, which is what makes the credential inseparable from
/// this object.
#[allow(dead_code)]
pub(crate) struct PhaseAuthorization {
    scope: CheckoutAuthorizationScope,
    run_token_jti: String,
    phase: CheckoutPhase,
    generation_id: String,
    permit: LaunchPermit,
}

impl std::fmt::Debug for PhaseAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhaseAuthorization")
            .field("phase", &self.phase)
            .field("generation_id", &self.generation_id)
            .field("run_token_jti", &self.run_token_jti)
            .field("permit", &"<retained durable permit>")
            .finish()
    }
}

impl PhaseAuthorization {
    #[allow(dead_code)]
    pub(crate) fn phase(&self) -> CheckoutPhase {
        self.phase
    }

    /// The exact durable generation this authorization was minted against. Borrowing, for
    /// cross-leg provenance checks and tests — never a way to reach the permit.
    #[allow(dead_code)]
    pub(crate) fn generation_id(&self) -> &str {
        &self.generation_id
    }

    #[allow(dead_code)]
    pub(crate) fn run_token_jti(&self) -> &str {
        &self.run_token_jti
    }

    /// The shared provenance check every consumption runs first.
    fn verify_provenance(
        &self,
        expected_phase: CheckoutPhase,
        run_token: &RunTokenCredential,
    ) -> Result<(), HookError> {
        if self.phase != expected_phase {
            return Err(HookError(format!(
                "phase authorization was minted for the {:?} boundary, but this is the \
                 {expected_phase:?} boundary -- refusing before any spawn",
                self.phase
            )));
        }
        if self.run_token_jti != run_token.jti {
            return Err(HookError(format!(
                "phase authorization was minted against run-token jti {:?}, but this boundary is \
                 running under jti {:?} -- refusing before any spawn",
                self.run_token_jti, run_token.jti
            )));
        }
        if self.generation_id.trim().is_empty() {
            return Err(HookError(
                "phase authorization carries no durable generation id".to_string(),
            ));
        }
        Ok(())
    }

    /// **Consume this authorization into the git-wire launch permit.** The permit is returned ONLY
    /// if the phase, the retained run-token JTI, and the complete checkout scope all agree with the
    /// transport request being authorized.
    #[allow(dead_code)]
    pub(crate) fn into_transport_permit(
        self,
        expected_phase: CheckoutPhase,
        run_token: &RunTokenCredential,
        tenant: &str,
        repo: &str,
        expected: &ExpectedGitCommitId,
    ) -> Result<LaunchPermit, HookError> {
        self.verify_provenance(expected_phase, run_token)?;
        if self.scope.tenant().0 != tenant {
            return Err(HookError(format!(
                "phase authorization was minted for tenant {:?}, but this transport is requesting \
                 tenant {tenant:?}",
                self.scope.tenant().0
            )));
        }
        if self.scope.repo_id() != repo {
            return Err(HookError(format!(
                "phase authorization was minted for repo {:?}, but this transport is requesting \
                 repo {repo:?}",
                self.scope.repo_id()
            )));
        }
        if self.scope.commit_hex() != expected.as_str()
            || self.scope.commit_format() != expected.format()
        {
            return Err(HookError(format!(
                "phase authorization was minted for commit {:?} ({:?}), but this transport is \
                 requesting {:?} ({:?})",
                self.scope.commit_hex(),
                self.scope.commit_format(),
                expected.as_str(),
                expected.format()
            )));
        }
        Ok(self.permit)
    }

    /// **Consume this authorization into Hop B's launch permit, bound to a capsule's FULL checkout
    /// scope** (CT-007 slice 5b.3-6a, Sol's r1 blocker 1 / r2 blocker 1). This is the ONLY
    /// preparation-permit path: a commit-only variant would let an authorization minted for scope B be
    /// replayed against a workspace capsule acquired for scope A whenever both name commit B, so it was
    /// removed entirely — full-scope enforcement is structural, not conventional. This variant requires
    /// the authorization's ENTIRE scope (tenant, repo ref, repo id, commit, object format) to equal
    /// `expected_scope` (the capsule's own derived scope), AND requires that scope to name exactly the
    /// `expected_commit` this preparation is about to check out. A capsule for scope A therefore
    /// refuses an authorization for scope B before any spawn, by construction.
    #[allow(dead_code)]
    pub(crate) fn into_preparation_permit_for_scope(
        self,
        run_token: &RunTokenCredential,
        expected_scope: &CheckoutAuthorizationScope,
        expected_commit: &ExpectedGitCommitId,
    ) -> Result<LaunchPermit, HookError> {
        self.verify_provenance(CheckoutPhase::Materialization, run_token)?;
        if &self.scope != expected_scope {
            return Err(HookError(format!(
                "materialization authorization was minted for scope (tenant {:?}, repo {:?}, commit \
                 {:?}), but this preparation capsule was acquired for scope (tenant {:?}, repo {:?}, \
                 commit {:?}) -- refusing before any spawn",
                self.scope.tenant().0,
                self.scope.repo_id(),
                self.scope.commit_hex(),
                expected_scope.tenant().0,
                expected_scope.repo_id(),
                expected_scope.commit_hex()
            )));
        }
        if expected_scope.commit_hex() != expected_commit.as_str()
            || expected_scope.commit_format() != expected_commit.format()
        {
            return Err(HookError(format!(
                "the capsule's checkout scope names commit {:?} ({:?}), but this preparation is \
                 checking out {:?} ({:?})",
                expected_scope.commit_hex(),
                expected_scope.commit_format(),
                expected_commit.as_str(),
                expected_commit.format()
            )));
        }
        Ok(self.permit)
    }
}

impl RunnerHooks {
    /// CT-007 slice 5b.3-2: the pre-Hop-A checkout-authorization check. A READ-ONLY verification
    /// (never a state transition) that the job's durably authorized claim actually grants read
    /// access to the EXACT repo/commit `scope` names — refuses outright if no hook was configured,
    /// rather than silently treating "no hook" as "authorized." On success, mints the ONE
    /// `CheckoutAuthorizationProof` `fetch_checkout_pack` can consume for this attempt. `pub(crate)`,
    /// not `pub` — only the sandbox backend itself (same crate) ever calls this; external callers
    /// only ever SUPPLY the hook closure via `Self::with_checkout_authorization`.
    #[allow(dead_code)]
    pub(crate) fn authorize_checkout(
        &self,
        spec: &JobSpec,
        scope: CheckoutAuthorizationScope,
    ) -> Result<CheckoutAuthorizationProof, HookError> {
        match &self.checkout_authorization {
            Some(hook) => {
                hook(spec, &scope)?;
                Ok(CheckoutAuthorizationProof {
                    scope,
                    run_token_jti: spec.run_token.jti.clone(),
                })
            }
            None => Err(HookError(
                "checkout-bearing job requires a configured checkout-authorization hook, but \
                 none was provided"
                    .to_string(),
            )),
        }
    }

    /// **CT-007 phase-credential generations: the V2 per-phase authorization.** Invokes the
    /// phase-aware hook (the control plane's `authorize_checkout_*_retained` boundary), which
    /// verifies the phase-bound signed credential AND returns the RETAINED durable permit whose
    /// commit holds a row lock on the exact `job_queue` row. The proof facts and that permit are
    /// fused into ONE [`PhaseAuthorization`] here, at the single point where they are known to have
    /// come from the same invocation.
    ///
    /// Refuses outright when no phase hook is configured — a missing hook is never "authorized", and
    /// the legacy [`Self::authorize_checkout`] hook is deliberately NOT a fallback: a claim-bound
    /// proof carries no phase and must never satisfy a phase boundary.
    #[allow(dead_code)]
    pub(crate) fn authorize_checkout_phase(
        &self,
        spec: &JobSpec,
        scope: CheckoutAuthorizationScope,
        phase: CheckoutPhase,
        generation_id: &str,
    ) -> Result<PhaseAuthorization, HookError> {
        let Some(hook) = &self.checkout_phase_authorization else {
            return Err(HookError(format!(
                "the {phase:?} phase boundary requires a configured phase-authorization hook, but \
                 none was provided"
            )));
        };
        if generation_id.trim().is_empty() {
            return Err(HookError(
                "a phase authorization requires a non-empty durable generation id".to_string(),
            ));
        }
        let permit = hook(spec, &scope, phase)?;
        Ok(PhaseAuthorization {
            scope,
            run_token_jti: spec.run_token.jti.clone(),
            phase,
            generation_id: generation_id.to_owned(),
            permit,
        })
    }
}
