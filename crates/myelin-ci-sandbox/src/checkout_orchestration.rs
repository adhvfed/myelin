//! CT-007 slice 5b.3-6c: the sandbox-side capability vocabulary for the **dormant** checkout
//! orchestrator.
//!
//! No control-plane dependency crosses the crate boundary here. Every durable authority the
//! orchestrator drives — parent-attempt admission, the phase journal, credential minting, the
//! preparation-lease renewal — is an INJECTED trait object the sandbox only *drives*; CT-007 slice
//! 5b.3-6d supplies the real control-plane adapter that implements these traits over
//! `CiJobParentAttempt`/`CiJobCredentialGenerationStore`/`DurablePreparationLeaseCheckpoint`. Because
//! production `RunnerHooks` never install a [`ParentAttemptReserveHook`](crate::ParentAttemptReserveHook)
//! (every constructor keeps it `None`), and no production composition root constructs any of these
//! authorities, the whole orchestrator stays unreachable until 5b.3-6e's single activating cutover.

use crate::runner::{
    PreparationAttemptDisposition, PreparationLeaseCheckpoint, PreparationLeaseLost,
    PreparationPhase, PreparationReportClaim, PreparationTerminalDisposition,
};
use crate::{
    CheckoutPhase, JobSpec, ReserveHandle, ResourceUsage, RunTokenAuthorizationContext,
    RunTokenCredential, SandboxLaunch,
};

/// **CT-007 slice 5b.3-6c: per-phase `JobSpec` rotation.** Produces the phase-local spec that differs
/// from `base` in EXACTLY the credential and its ephemeral authorization context (`run_token` +
/// `run_token_authorization`) — every other immutable `JobSpec` field is preserved verbatim. Each
/// phase boundary authorizes against, and runs under, its OWN rotated spec, so a real 6d/6e adapter's
/// per-generation Identity verification passes (the advertise/fetch/materialization/workload
/// generations each carry their own JTI + binding, not the stale advertise context).
pub(crate) fn rotate_spec_for_generation(
    base: &JobSpec,
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
) -> JobSpec {
    let mut spec = base.clone();
    spec.run_token = credential;
    spec.run_token_authorization = Some(authorization_context);
    spec
}

/// **The UNTRUSTED PREPARATION-phase credential carrier the attempt authority mints** (CT-007 5b.3-6c).
///
/// Bundles the freshly minted run-token credential, the REAL typed ephemeral authorization context the
/// control plane's own phase gate will verify, and the durable generation id it was minted against. It
/// is deliberately UNTRUSTED on the sandbox side: the sandbox only carries it into a phase-local
/// [`JobSpec`] (via [`Self::phase_local_spec`]) which it hands to
/// [`RunnerHooks::authorize_checkout_phase`](crate::RunnerHooks::authorize_checkout_phase) and the leg
/// transport, where the credential slice's Identity verification actually happens.
///
/// It is DISTINCT from [`WorkloadCredentialCarrier`] by TYPE: a workload credential can never be passed
/// to a preparation authorization API, nor a preparation credential to the workload permit path — the
/// two mint operations return two different, non-interchangeable carrier types.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct PhaseCredentialCarrier {
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
    generation_id: String,
}

impl PhaseCredentialCarrier {
    /// Build a carrier from the three parts one preparation-phase mint produced together.
    #[allow(dead_code)]
    pub fn new(
        credential: RunTokenCredential,
        authorization_context: RunTokenAuthorizationContext,
        generation_id: impl Into<String>,
    ) -> Self {
        Self {
            credential,
            authorization_context,
            generation_id: generation_id.into(),
        }
    }

    /// The carried (still-unverified) run-token credential.
    #[allow(dead_code)]
    pub fn credential(&self) -> &RunTokenCredential {
        &self.credential
    }

    /// The carried real typed ephemeral authorization context.
    #[allow(dead_code)]
    pub fn authorization_context(&self) -> &RunTokenAuthorizationContext {
        &self.authorization_context
    }

    /// The durable generation id this carrier was minted at — threaded into
    /// `authorize_checkout_phase` so the phase authorization is fused to the exact generation.
    #[allow(dead_code)]
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    /// **The phase-local spec** — `base` with ONLY the credential + ephemeral authorization context
    /// rotated in. Authorize against THIS (not the advertise-bound base) so the phase authorization
    /// retains this generation's JTI, and thread [`Self::into_credential`] into the same leg.
    #[allow(dead_code)]
    pub(crate) fn phase_local_spec(&self, base: &JobSpec) -> JobSpec {
        rotate_spec_for_generation(
            base,
            self.credential.clone(),
            self.authorization_context.clone(),
        )
    }

    /// Consume the carrier into just its credential — threaded into the transport/preparation leg once
    /// its phase authorization has been minted from the SAME generation (matching JTIs).
    #[allow(dead_code)]
    pub fn into_credential(self) -> RunTokenCredential {
        self.credential
    }
}

/// **The UNTRUSTED WORKLOAD credential carrier** (CT-007 5b.3-6c, step 21) — deliberately a DISTINCT
/// type from [`PhaseCredentialCarrier`] so the sandbox can never drive a preparation authorization API
/// with a workload credential (`CheckoutPhase` excludes workload) nor a preparation credential with the
/// workload launch permit. Minted ONLY by [`AttemptAuthority::mint_workload_credential`]; consumed ONLY
/// by the closed workload transition, which rotates it into the workload's own phase-local spec before
/// acquiring the launch permit.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct WorkloadCredentialCarrier {
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
    generation_id: String,
}

impl WorkloadCredentialCarrier {
    /// Build a workload carrier from the three parts one workload mint produced together.
    #[allow(dead_code)]
    pub fn new(
        credential: RunTokenCredential,
        authorization_context: RunTokenAuthorizationContext,
        generation_id: impl Into<String>,
    ) -> Self {
        Self {
            credential,
            authorization_context,
            generation_id: generation_id.into(),
        }
    }

    /// The durable workload generation id.
    #[allow(dead_code)]
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    /// The carried workload run-token credential.
    #[allow(dead_code)]
    pub fn credential(&self) -> &RunTokenCredential {
        &self.credential
    }

    /// The workload-local spec — `base` with ONLY the workload credential + its authorization context
    /// rotated in. The workload launch permit is acquired against, and the workload runs under, THIS.
    #[allow(dead_code)]
    pub(crate) fn workload_local_spec(&self, base: &JobSpec) -> JobSpec {
        rotate_spec_for_generation(
            base,
            self.credential.clone(),
            self.authorization_context.clone(),
        )
    }
}

/// A structural failure from an injected [`AttemptAuthority`] operation (CT-007 5b.3-6c). The
/// orchestrator never parses `0`; diagnostics are diagnostics, not a routing protocol.
#[derive(Clone, Debug)]
pub struct AttemptAuthorityError(pub String);

impl std::fmt::Display for AttemptAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempt authority operation failed: {}", self.0)
    }
}

impl std::error::Error for AttemptAuthorityError {}

/// **CT-007 slice 5b.3-6c (Sol's finding 1): authorize a PREPARATION phase against its OWN generation.**
/// Rotates `carrier` into a phase-local spec (base with ONLY the credential + ephemeral authorization
/// context replaced), authorizes THAT spec through the phase hook (so the returned [`PhaseAuthorization`]
/// retains this generation's JTI — not the stale advertise-bound base's), and hands back the SAME
/// credential to thread into the leg. Because both the authorization and the threaded credential come
/// from the one `carrier`, their JTIs match and the leg's permit consumption succeeds. Extracted so the
/// exact threading is deterministically unit-testable without a real workspace capsule.
#[allow(dead_code)]
pub(crate) fn authorize_phase_generation(
    hooks: &crate::RunnerHooks,
    base_spec: &JobSpec,
    scope: &crate::CheckoutAuthorizationScope,
    phase: CheckoutPhase,
    carrier: PhaseCredentialCarrier,
) -> Result<(RunTokenCredential, crate::PhaseAuthorization), crate::HookError> {
    let phase_spec = carrier.phase_local_spec(base_spec);
    let authorization =
        hooks.authorize_checkout_phase(&phase_spec, scope.clone(), phase, carrier.generation_id())?;
    Ok((carrier.into_credential(), authorization))
}

/// **The opaque, INJECTED parent-attempt authority for ONE checkout attempt** (CT-007 5b.3-6c).
///
/// It owns that attempt's durable parent-attempt journal, its per-phase credential minting, and its
/// preparation-lease renewal. The sandbox drives it through this narrow trait and never names the
/// control-plane types behind it; 5b.3-6d supplies the real adapter over `CiJobParentAttempt` +
/// `CiJobCredentialGenerationStore` + `DurablePreparationLeaseCheckpoint`. Object-safe so the
/// admission result can carry it as a `Box<dyn AttemptAuthority>`.
pub trait AttemptAuthority: Send + Sync {
    /// Open the durable journal row for `phase` (idempotent replay on the exact parent attempt).
    fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError>;

    /// Complete `phase` with the EXACT measured usage (overflow is the caller's problem — this
    /// receives an already-checked total, never a wrapped one).
    fn complete_phase(
        &self,
        phase: PreparationPhase,
        usage: ResourceUsage,
    ) -> Result<(), AttemptAuthorityError>;

    /// Seal `phase` at its durable ceiling — used ONLY when the exact usage can no longer be
    /// honestly represented (a checked addition overflowed). Never manufactures an exact figure.
    fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError>;

    /// Renew the exact preparation lease for this generation, or refuse because it is no longer ours.
    /// Composes with the [`PreparationLeaseCheckpoint`] seam via
    /// [`AttemptAuthorityLeaseCheckpoint`].
    fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost>;

    /// Mint a fresh PREPARATION phase-credential carrier for `phase` — UNTRUSTED until the Identity +
    /// durable phase gates verify it downstream. `phase` is a [`CheckoutPhase`], which structurally
    /// EXCLUDES the workload — the workload has its own separate mint below.
    fn mint_phase_credential(
        &self,
        phase: CheckoutPhase,
    ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError>;

    /// **Mint the WORKLOAD credential (step 21)** — a SEPARATE operation returning a distinct
    /// [`WorkloadCredentialCarrier`] type, so a workload credential can never be handed to a
    /// preparation authorization API and vice versa. Called once, immediately before the workload
    /// launch permit is acquired against the workload's own phase-local spec.
    fn mint_workload_credential(
        &self,
    ) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError>;

    /// Whether a nonterminal outcome should be requeued (another parent attempt is permitted) rather
    /// than terminalized as [`PreparationTerminalDisposition::AttemptsExhausted`].
    fn should_requeue(&self) -> bool;
}

/// Presents an injected [`AttemptAuthority`] as the [`PreparationLeaseCheckpoint`] the Hop A
/// transport and Hop B preparation already accept (CT-007 5b.3-6c) — the ONE renewal seam, so a
/// phase boundary reconciles the immutable claim window with the heartbeat-extendable execution lease
/// exactly once. Threaded as `Some(&AttemptAuthorityLeaseCheckpoint(authority))` where those paths
/// today pass `None`.
pub(crate) struct AttemptAuthorityLeaseCheckpoint<'a>(pub &'a dyn AttemptAuthority);

impl PreparationLeaseCheckpoint for AttemptAuthorityLeaseCheckpoint<'_> {
    fn renew(&self) -> Result<(), PreparationLeaseLost> {
        self.0.renew_preparation_lease()
    }
}

/// **The result of the parent-attempt reserve/admission transaction** (CT-007 5b.3-6c) — the new
/// reservation mode that lands alongside the legacy [`ReserveHook`](crate::ReserveHook). The reserve
/// handle is present in BOTH arms: even an exhausted attempt still holds the operational reservation
/// that its terminal `AttemptsExhausted` report must settle against.
pub enum ParentAttemptAdmission {
    /// The exact claim was validated, the reservation transitioned to inflight, and a parent-attempt
    /// row was inserted/replayed — the attempt may proceed under `attempt_authority`. `claim` is the
    /// preparation REPORTING identity (CT-007 5b.3-6d STEP 4) carried into any preparation outcome the
    /// orchestrator/continuation produces for this attempt.
    Admitted {
        claim: PreparationReportClaim,
        reserve: ReserveHandle,
        attempt_authority: Box<dyn AttemptAuthority>,
    },
    /// The reservation exists but the durable parent-attempt budget is already exhausted — nothing
    /// may spawn; the caller terminalizes `AttemptsExhausted` and settles `reserve`. `claim` carries
    /// the reporting identity even though there is no attempt authority: an exhausted attempt STILL
    /// reports its terminal (CT-007 5b.3-6d STEP 4).
    AttemptsExhausted {
        claim: PreparationReportClaim,
        reserve: ReserveHandle,
    },
}

/// A structural failure of the dormant checkout orchestration that is NOT an ordinary preparation
/// disposition (CT-007 slice 5b.3-6c) — a configured hook refusing, or an injected authority's
/// journal/credential op failing. The queue/report routing for these is 5b.3-6d/6e's job; the sandbox
/// only surfaces them structurally.
#[derive(Debug)]
pub enum CheckoutOrchestrationError {
    /// A configured hook (admission, phase authorization) refused.
    Hook(crate::HookError),
    /// An injected attempt-authority journal/credential op failed.
    Authority(AttemptAuthorityError),
    /// The exact preparation-lease generation was lost (another worker now owns it) — the runner must
    /// abort this cycle before spawning; the capsule was already disposed.
    LeaseLost(PreparationLeaseLost),
}

impl std::fmt::Display for CheckoutOrchestrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hook(e) => write!(f, "checkout orchestration hook refused: {e}"),
            Self::Authority(e) => write!(f, "checkout orchestration authority failed: {e}"),
            Self::LeaseLost(e) => write!(f, "checkout orchestration lease lost: {e}"),
        }
    }
}

impl std::error::Error for CheckoutOrchestrationError {}

impl From<AttemptAuthorityError> for CheckoutOrchestrationError {
    fn from(e: AttemptAuthorityError) -> Self {
        Self::Authority(e)
    }
}

impl From<crate::HookError> for CheckoutOrchestrationError {
    fn from(e: crate::HookError) -> Self {
        Self::Hook(e)
    }
}

/// **The typed sandbox result of one dormant checkout orchestration** (CT-007 5b.3-6c). Wraps the
/// existing [`PreparationTerminalDisposition`]/[`PreparationAttemptDisposition`] vocabulary rather
/// than inventing a parallel one; the outer orchestrator routes each variant to the injected
/// authority's queue/report path (5b.3-6d/6e install the real control-plane routing).
#[derive(Debug)]
#[allow(dead_code)]
pub enum CheckoutContinuationOutcome {
    /// Preparation fully succeeded and the workload launched — the ordinary [`SandboxLaunch`] path
    /// takes over. `report_preparation_terminal` must NEVER be called for this outcome.
    WorkloadLaunched(SandboxLaunch),
    /// A durable running claim existed for the workload but the sandbox has been fully killed/reaped
    /// — the existing reporter-owned retryable-attempt accounting takes over (never a preparation
    /// terminal report).
    WorkloadRetryable {
        cause: crate::runner::RetryableAttemptCause,
        usage: ResourceUsage,
        message: String,
    },
    /// Preparation reached a terminal disposition (`Failed`/`TimedOut`/`AttemptsExhausted`) before
    /// any workload launched — the caller reports `report_preparation_terminal(claim, disposition)`.
    /// `claim` is the reporting identity carried UNCHANGED from the parent-attempt admission (CT-007
    /// 5b.3-6d STEP 4).
    PreparationTerminal {
        claim: PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        /// Retained operator-safe detail from the checkout failure, if this terminal has one.
        diagnostic: Option<String>,
    },
    /// A nonterminal preparation failure that is a RETRY REQUEST, not a completed requeue: it is
    /// produced after the ADVISORY `should_requeue()` check (budget not yet exhausted). The caller
    /// MUST dispatch `report_preparation_retry(claim)`, whose authoritative leased-generation CAS
    /// decides `Requeued` (the generation was actually re-queued) vs `NoOp` (already
    /// requeued/reclaimed/stale) — an activation author must NOT assume a requeue happened and skip the
    /// reporter, or the generation is left leased. When the budget IS exhausted the orchestrator
    /// surfaces [`Self::PreparationTerminal`] with `AttemptsExhausted` instead, never this variant.
    /// `claim` is the reporting identity carried UNCHANGED from the parent-attempt admission (CT-007
    /// 5b.3-6d STEP 4).
    PreparationRetryable {
        claim: PreparationReportClaim,
        phase: PreparationPhase,
    },
    /// An invariant requires reconciliation — a teardown could not be proven and/or exact usage is
    /// unrepresentable, so resources may still be live. No ordinary terminal/requeue report; the
    /// reaper/reconciliation owner takes over.
    ReconciliationRequired {
        phase: PreparationPhase,
        teardown_unproven: bool,
        usage_unrepresentable: bool,
        quarantine_required: bool,
    },
}

/// Route a nonterminal/terminal preparation disposition to the correct typed outcome, performing the
/// journal side-effect (complete/seal the phase) each row demands (CT-007 5b.3-6c, Sol's failure
/// matrix). `usage` is the exact, already-checked measured total up to the failure point; it is only
/// submitted on the completion (not the seal) branches.
#[allow(dead_code)]
pub(crate) fn route_preparation_disposition(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposition: PreparationAttemptDisposition,
    usage: ResourceUsage,
    diagnostic: Option<String>,
) -> Result<CheckoutContinuationOutcome, AttemptAuthorityError> {
    match disposition {
        PreparationAttemptDisposition::Terminal(terminal) => {
            // Terminal failed/timed out: complete the active phase with the exact measured usage,
            // then report the preparation terminal.
            authority.complete_phase(terminal_phase(terminal), usage)?;
            Ok(CheckoutContinuationOutcome::PreparationTerminal {
                claim: claim.clone(),
                disposition: terminal,
                diagnostic,
            })
        }
        PreparationAttemptDisposition::RefusedBeforeExecution { phase } => {
            // Refused before execution: complete the begun phase with ZERO, then requeue-or-exhausted.
            authority.complete_phase(
                phase,
                ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
            )?;
            Ok(requeue_or_exhausted(authority, claim, phase))
        }
        PreparationAttemptDisposition::RetryableInfrastructure { phase } => {
            // Retryable infrastructure: complete with the exact measured usage, then requeue-or-exhausted.
            authority.complete_phase(phase, usage)?;
            Ok(requeue_or_exhausted(authority, claim, phase))
        }
        PreparationAttemptDisposition::ReconciliationRequired {
            phase,
            teardown_unproven,
            usage_unrepresentable,
            quarantine_required,
        } => {
            // Usage unrepresentable / teardown unproven: SEAL at the ceiling (never manufacture exact
            // usage), then hand to reconciliation.
            authority.seal_phase(phase)?;
            Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                phase,
                teardown_unproven,
                usage_unrepresentable,
                quarantine_required,
            })
        }
    }
}

/// **CT-007 slice 5b.3-6c (Sol's finding 4): honour capsule-disposal diagnostics.** When 6a disposal
/// returns diagnostics, a workspace/lease could not be proven released — it was QUARANTINED and the
/// resource may still be live. In that case the ONLY honest outcome is [`Self::ReconciliationRequired`]
/// (the reaper/reconciliation owns it), NEVER an ordinary requeue/terminal alongside an unreconciled
/// quarantined resource. An empty diagnostics vector means the disposal was fully clean → the intended
/// outcome stands.
#[allow(dead_code)]
pub(crate) fn route_after_disposal(
    disposal_diagnostics: Vec<String>,
    phase: PreparationPhase,
    clean: CheckoutContinuationOutcome,
) -> CheckoutContinuationOutcome {
    if disposal_diagnostics.is_empty() {
        clean
    } else {
        CheckoutContinuationOutcome::ReconciliationRequired {
            phase,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        }
    }
}

/// **CT-007 slice 5b.3-6c (Sol's r2 findings 2/4): the pure Hop-B-failure router.** The materialization
/// journal row is `started`, so it MUST be resolved: `route_preparation_disposition` completes it
/// (measured usage) or SEALS it at ceiling per the disposition — including IMMEDIATE sealing for
/// teardown-unproven/unreleasable (finding 2, never a later sealer sweep). If disposal ALSO quarantined
/// a resource, the OUTCOME is `ReconciliationRequired` (finding 4), but the phase is resolved either
/// way. Extracted so both invariants are deterministically unit-testable without a real capsule.
#[allow(dead_code)]
pub(crate) fn resolve_hop_b_failure(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposition: PreparationAttemptDisposition,
    usage: ResourceUsage,
    diagnostic: Option<String>,
    disposal_diagnostics: Vec<String>,
) -> Result<CheckoutContinuationOutcome, AttemptAuthorityError> {
    let routed =
        route_preparation_disposition(authority, claim, disposition, usage, diagnostic)?;
    if disposal_diagnostics.is_empty() {
        Ok(routed)
    } else {
        Ok(CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        })
    }
}

/// **CT-007 slice 5b.3-6c (Sol's r2 finding 4): the pure post-acquisition authority-failure router.**
/// The materialization capsule was already disposed by the caller; this routes the typed outcome from
/// the disposal diagnostics + whether the phase was begun. If the begun phase cannot be completed, or a
/// resource was quarantined, reconciliation owns it; otherwise a clean disposal requeues/exhausts.
/// Extracted so the begin/mint/AUTHORIZE-refusal paths are deterministically testable without a real
/// capsule (finding 4a).
#[allow(dead_code)]
pub(crate) fn route_post_acquisition_authority_failure(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposal_diagnostics: Vec<String>,
    phase_was_begun: bool,
) -> CheckoutContinuationOutcome {
    let quarantined = !disposal_diagnostics.is_empty();
    if phase_was_begun
        && authority
            .complete_phase(
                PreparationPhase::CheckoutMaterialization,
                ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
            )
            .is_err()
    {
        return CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: quarantined,
            usage_unrepresentable: false,
            quarantine_required: quarantined,
        };
    }
    if quarantined {
        CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        }
    } else {
        requeue_or_exhausted(authority, claim, PreparationPhase::CheckoutMaterialization)
    }
}

/// Requeue the exact leased generation when another parent attempt is permitted, else terminalize
/// `AttemptsExhausted` (CT-007 5b.3-6c). The parent-attempt journal — via
/// [`AttemptAuthority::should_requeue`] — is the sole retry authority; this never appends to workload
/// retry attempts. `claim` is carried UNCHANGED into whichever preparation outcome is produced (CT-007
/// 5b.3-6d STEP 4).
#[allow(dead_code)]
pub(crate) fn requeue_or_exhausted(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    phase: PreparationPhase,
) -> CheckoutContinuationOutcome {
    if authority.should_requeue() {
        CheckoutContinuationOutcome::PreparationRetryable {
            claim: claim.clone(),
            phase,
        }
    } else {
        CheckoutContinuationOutcome::PreparationTerminal {
            claim: claim.clone(),
            disposition: PreparationTerminalDisposition::AttemptsExhausted,
            diagnostic: None,
        }
    }
}

/// The journal phase a terminal disposition completes against.
#[allow(dead_code)]
fn terminal_phase(disposition: PreparationTerminalDisposition) -> PreparationPhase {
    match disposition {
        PreparationTerminalDisposition::Failed { phase }
        | PreparationTerminalDisposition::TimedOut { phase } => phase,
        // AttemptsExhausted is never produced from a measured phase failure here — it is the
        // requeue-exhausted terminal. Default to the transport phase (its completion is a no-op the
        // caller never reaches for this variant).
        PreparationTerminalDisposition::AttemptsExhausted => PreparationPhase::CheckoutTransport,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A recording fake [`AttemptAuthority`] — records every journal op so a routing test can assert
    /// the EXACT complete/seal side-effects, and returns a configurable `should_requeue`.
    struct RecordingAuthority {
        ops: Mutex<Vec<String>>,
        should_requeue: bool,
    }

    impl RecordingAuthority {
        fn new(should_requeue: bool) -> Self {
            Self {
                ops: Mutex::new(Vec::new()),
                should_requeue,
            }
        }
        fn ops(&self) -> Vec<String> {
            self.ops.lock().unwrap().clone()
        }
    }

    impl AttemptAuthority for RecordingAuthority {
        fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("begin:{phase:?}"));
            Ok(())
        }
        fn complete_phase(
            &self,
            phase: PreparationPhase,
            usage: ResourceUsage,
        ) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!(
                "complete:{phase:?}:{}:{}",
                usage.cpu_seconds, usage.mem_byte_seconds
            ));
            Ok(())
        }
        fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("seal:{phase:?}"));
            Ok(())
        }
        fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost> {
            self.ops.lock().unwrap().push("renew".to_string());
            Ok(())
        }
        fn mint_phase_credential(
            &self,
            phase: CheckoutPhase,
        ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("mint:{phase:?}"));
            Ok(PhaseCredentialCarrier::new(
                RunTokenCredential::new("bearer", format!("jti-{phase:?}"), 300).unwrap(),
                test_authorization_context(),
                format!("gen-{phase:?}"),
            ))
        }
        fn mint_workload_credential(
            &self,
        ) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError> {
            self.ops.lock().unwrap().push("mint:Workload".to_string());
            Ok(WorkloadCredentialCarrier::new(
                RunTokenCredential::new("bearer", "jti-Workload", 300).unwrap(),
                test_authorization_context(),
                "gen-Workload",
            ))
        }
        fn should_requeue(&self) -> bool {
            self.should_requeue
        }
    }

    fn test_authorization_context() -> RunTokenAuthorizationContext {
        RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "fr-par".to_string(),
            principal_id: "p".to_string(),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf".to_string(),
            job_id: "j".to_string(),
            lease_owner: "o".to_string(),
            lease_epoch: 1,
            claim_nonce: "n".to_string(),
            claim_started_at_epoch_secs: 0,
            claim_expires_at_epoch_secs: 1,
            reserve_id: "r".to_string(),
            required_capabilities: vec![],
            checkout_scope: None,
            credential_binding: None,
        })
    }

    /// An authorization context with facts DELIBERATELY DISTINCT from both the base spec
    /// (`checkout_job_spec_for_tests`) and `test_authorization_context` — so a rotation regression that
    /// retains the advertise context (rather than installing the carrier's) is caught (Sol's r2 finding 3).
    fn distinct_authorization_context(generation: &str) -> RunTokenAuthorizationContext {
        RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "us-west-2".to_string(),
            principal_id: format!("principal-{generation}"),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf-rotated".to_string(),
            job_id: format!("job-{generation}"),
            lease_owner: "rotated-owner".to_string(),
            lease_epoch: 99,
            claim_nonce: format!("nonce-{generation}"),
            claim_started_at_epoch_secs: 1000,
            claim_expires_at_epoch_secs: 2000,
            reserve_id: format!("reserve-{generation}"),
            required_capabilities: vec!["repo:widgets#pull".to_string()],
            checkout_scope: None,
            credential_binding: None,
        })
    }

    fn usage(cpu: u64, mem: u64) -> ResourceUsage {
        ResourceUsage {
            cpu_seconds: cpu,
            mem_byte_seconds: mem,
        }
    }

    /// A well-formed preparation reporting identity for the routing tests (CT-007 5b.3-6d STEP 4). The
    /// routers carry it UNCHANGED into whichever preparation outcome they build.
    fn report_claim() -> PreparationReportClaim {
        PreparationReportClaim {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
            ci_run_id: "44444444-4444-4444-4444-444444444444".into(),
            job_id: "22222222-2222-2222-2222-222222222222".into(),
            token_authority_handle: "tah-xyz".into(),
            idem_token: "11111111-1111-1111-1111-111111111111/build".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 7,
            claim_nonce: "33333333-3333-3333-3333-333333333333".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        }
    }

    #[test]
    fn phase_credential_carrier_rotates_only_the_credential_and_context() {
        let base = crate::checkout_job_spec_for_tests();
        let carrier_context = distinct_authorization_context("materialization");
        let carrier_credential = RunTokenCredential::new("bearer", "the-jti", 300).unwrap();
        let carrier = PhaseCredentialCarrier::new(
            carrier_credential.clone(),
            carrier_context.clone(),
            "gen-42",
        );
        assert_eq!(carrier.generation_id(), "gen-42");

        // Sanity: the base spec's credential + context are DISTINCT from the carrier's, so a regression
        // that retained the base would be observable.
        assert_ne!(base.run_token, carrier_credential);
        assert_ne!(base.run_token_authorization, Some(carrier_context.clone()));

        let phase_spec = carrier.phase_local_spec(&base);
        // The rotated context equals the CARRIER's complete context (not the base's advertise context).
        assert_eq!(phase_spec.run_token, carrier_credential);
        assert_eq!(
            phase_spec.run_token_authorization,
            Some(carrier_context.clone())
        );
        // And the ENTIRE spec equals `base.clone()` with EXACTLY the two fields replaced — proving every
        // other immutable field is byte-identical AND both rotated fields actually changed.
        let mut expected = base.clone();
        expected.run_token = carrier_credential.clone();
        expected.run_token_authorization = Some(carrier_context);
        assert_eq!(phase_spec, expected);
        assert_eq!(carrier.into_credential(), carrier_credential);
    }

    #[test]
    fn workload_carrier_is_type_separate_and_rotates_its_own_spec() {
        // Sol's r2 finding 4b: the workload runs under its OWN rotated generation — credential AND
        // context — distinct from the advertise base, proven always-run.
        let base = crate::checkout_job_spec_for_tests();
        let carrier_context = distinct_authorization_context("workload");
        let carrier_credential = RunTokenCredential::new("bearer", "wl-jti", 300).unwrap();
        let carrier = WorkloadCredentialCarrier::new(
            carrier_credential.clone(),
            carrier_context.clone(),
            "gen-wl",
        );
        assert_ne!(base.run_token, carrier_credential);
        assert_ne!(base.run_token_authorization, Some(carrier_context.clone()));

        let workload_spec = carrier.workload_local_spec(&base);
        assert_eq!(workload_spec.run_token, carrier_credential);
        assert_eq!(
            workload_spec.run_token_authorization,
            Some(carrier_context.clone())
        );
        let mut expected = base.clone();
        expected.run_token = carrier_credential;
        expected.run_token_authorization = Some(carrier_context);
        assert_eq!(workload_spec, expected);
        assert_eq!(carrier.generation_id(), "gen-wl");
    }

    /// **Sol's finding 1, deterministic catch.** `authorize_phase_generation` must authorize against
    /// the phase's OWN rotated spec (credential + context replaced), NOT the stale advertise-bound base,
    /// and must thread back the SAME credential the authorization retained. If the wiring regressed to
    /// authorizing the base spec, the hook would observe the base JTI and this fails.
    #[test]
    fn authorize_phase_generation_rotates_and_threads_the_matching_credential() {
        use std::sync::Arc;
        #[allow(clippy::type_complexity)]
        let seen: Arc<Mutex<Option<(String, Option<RunTokenAuthorizationContext>)>>> =
            Arc::new(Mutex::new(None));
        let recorder = seen.clone();
        let hooks = crate::RunnerHooks::new(
            crate::CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(crate::ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(move |spec, _scope, _phase| {
            *recorder.lock().unwrap() = Some((
                spec.run_token.jti.clone(),
                spec.run_token_authorization.clone(),
            ));
            Ok(crate::LaunchPermit::immediate())
        }));
        let base = crate::checkout_job_spec_for_tests();
        assert_eq!(base.run_token.jti, "advertise-jti", "the base carries the advertise generation");
        let scope = crate::derive_checkout_authorization_scope(base.kind, &base.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let carrier_context = distinct_authorization_context("fetch");
        let carrier = PhaseCredentialCarrier::new(
            RunTokenCredential::new("bearer", "fetch-jti-xyz", 300).unwrap(),
            carrier_context.clone(),
            "gen-fetch",
        );
        let (credential, authorization) =
            authorize_phase_generation(&hooks, &base, &scope, CheckoutPhase::Fetch, carrier)
                .expect("authorizes");
        // The hook saw the ROTATED fetch generation — its JTI AND its complete authorization context —
        // never the stale advertise-bound base (Sol's r2 finding 3: a regression that rotated only the
        // JTI while retaining the advertise context is now caught).
        let (seen_jti, seen_ctx) = seen.lock().unwrap().clone().expect("the hook was invoked");
        assert_eq!(
            seen_jti, "fetch-jti-xyz",
            "the phase hook must be handed the rotated credential, not the advertise base"
        );
        assert_eq!(
            seen_ctx,
            Some(carrier_context),
            "the phase hook must be handed the rotated authorization context, not the advertise base's"
        );
        // The threaded credential and the authorization both carry the fetch JTI (they MATCH, so the
        // leg's permit consumption succeeds).
        assert_eq!(credential.jti, "fetch-jti-xyz");
        assert_eq!(authorization.run_token_jti(), "fetch-jti-xyz");
    }

    /// Sol's r2 finding 2, DETERMINISTIC: a teardown-unproven materialization Hop-B failure whose
    /// disposal quarantines must SEAL the started phase at ceiling AND return `ReconciliationRequired`.
    #[test]
    fn resolve_hop_b_failure_seals_and_reconciles_a_quarantined_teardown_unproven() {
        let authority = RecordingAuthority::new(true);
        let outcome = resolve_hop_b_failure(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            },
            usage(2, 2),
            None,
            vec!["slot quarantined; workspace manager poisoned".to_string()],
        )
        .expect("routes");
        // The started materialization phase is SEALED at ceiling (immediate, not a later sweep)...
        assert_eq!(
            authority.ops(),
            vec!["seal:CheckoutMaterialization"],
            "the started materialization phase must be sealed immediately"
        );
        // ...AND the quarantined resource forces reconciliation.
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                quarantine_required: true,
                ..
            }
        ));
    }

    /// A terminal Hop-B failure whose disposal is CLEAN completes the phase with measured usage and
    /// terminalizes — no reconciliation.
    #[test]
    fn resolve_hop_b_failure_completes_terminal_on_a_clean_disposal() {
        let authority = RecordingAuthority::new(true);
        let outcome = resolve_hop_b_failure(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutMaterialization,
            }),
            usage(4, 4),
            Some("host-side HEAD re-verification disagreed: injected".to_string()),
            vec![],
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutMaterialization:4:4"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal { .. }
        ));
    }

    /// Sol's r2 finding 4a, DETERMINISTIC: a post-acquisition authority failure (begin/mint/AUTHORIZE)
    /// routes through capsule disposal to the correct typed outcome, everywhere (no Btrfs).
    #[test]
    fn post_acquisition_authority_failure_routing_matrix() {
        // begin_phase failure (phase NOT begun), clean disposal → requeue.
        let authority = RecordingAuthority::new(true);
        let out = route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], false);
        assert!(authority.ops().is_empty(), "no begun phase to complete");
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));

        // mint/authorize failure (phase begun), clean disposal → complete-zero then requeue.
        let authority = RecordingAuthority::new(true);
        let out = route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], true);
        assert_eq!(authority.ops(), vec!["complete:CheckoutMaterialization:0:0"]);
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));

        // authorize failure (phase begun), clean disposal, budget exhausted → complete-zero, terminalize.
        let authority = RecordingAuthority::new(false);
        let out = route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], true);
        assert_eq!(authority.ops(), vec!["complete:CheckoutMaterialization:0:0"]);
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));

        // quarantined disposal → reconciliation (regardless of the phase completing).
        let authority = RecordingAuthority::new(true);
        let out = route_post_acquisition_authority_failure(
            &authority,
            &report_claim(),
            vec!["slot quarantined".to_string()],
            true,
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::ReconciliationRequired {
                quarantine_required: true,
                ..
            }
        ));
    }

    #[test]
    fn requeue_or_exhausted_carries_the_exact_claim_into_both_outcomes() {
        // CT-007 5b.3-6d STEP 4: the reporting identity threads UNCHANGED into whichever preparation
        // outcome the router builds — a requeue (should_requeue == true) and an exhaustion terminal.
        let claim = report_claim();
        let requeued = requeue_or_exhausted(
            &RecordingAuthority::new(true),
            &claim,
            PreparationPhase::CheckoutTransport,
        );
        match requeued {
            CheckoutContinuationOutcome::PreparationRetryable { claim: carried, phase } => {
                assert_eq!(carried, claim, "the retry outcome carries the exact claim");
                assert_eq!(phase, PreparationPhase::CheckoutTransport);
            }
            other => panic!("expected a retryable, got {other:?}"),
        }
        let exhausted = requeue_or_exhausted(
            &RecordingAuthority::new(false),
            &claim,
            PreparationPhase::CheckoutTransport,
        );
        match exhausted {
            CheckoutContinuationOutcome::PreparationTerminal {
                claim: carried,
                disposition,
                ..
            } => {
                assert_eq!(carried, claim, "the exhausted terminal carries the exact claim");
                assert_eq!(disposition, PreparationTerminalDisposition::AttemptsExhausted);
            }
            other => panic!("expected an exhausted terminal, got {other:?}"),
        }
    }

    #[test]
    fn route_terminal_completes_the_active_phase_with_exact_usage() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutTransport,
            }),
            usage(7, 9),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:7:9"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutTransport
                },
                ..
            }
        ));
    }

    #[test]
    fn route_timed_out_completes_and_reports_terminal() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutMaterialization,
            }),
            usage(3, 3),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutMaterialization:3:3"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::TimedOut { .. },
                ..
            }
        ));
    }

    #[test]
    fn route_refused_completes_zero_then_requeues_when_permitted() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutTransport,
            },
            usage(5, 5), // ignored — a refusal completes the phase with ZERO.
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:0:0"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
    }

    #[test]
    fn route_refused_terminalizes_attempts_exhausted_when_not_permitted() {
        let authority = RecordingAuthority::new(false);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutTransport,
            },
            usage(0, 0),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:0:0"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
    }

    #[test]
    fn route_retryable_infrastructure_completes_with_exact_usage_then_requeues() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RetryableInfrastructure {
                phase: PreparationPhase::CheckoutMaterialization,
            },
            usage(11, 13),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutMaterialization:11:13"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutMaterialization,
                ..
            }
        ));
    }

    #[test]
    fn route_usage_unrepresentable_seals_at_the_ceiling_and_requires_reconciliation() {
        // The checked-usage overflow branch: SEAL (never manufacture an exact figure), reconcile.
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutTransport,
                teardown_unproven: false,
                usage_unrepresentable: true,
                quarantine_required: false,
            },
            usage(999, 999),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["seal:CheckoutTransport"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                usage_unrepresentable: true,
                teardown_unproven: false,
                ..
            }
        ));
    }

    #[test]
    fn route_after_disposal_reconciles_on_a_quarantined_disposal() {
        // Finding 4: non-empty disposal diagnostics (a quarantine / unproven release) must override an
        // ordinary requeue with ReconciliationRequired — never a requeued generation alongside a live
        // unreconciled resource.
        let clean = CheckoutContinuationOutcome::PreparationRetryable {
            claim: report_claim(),
            phase: PreparationPhase::CheckoutMaterialization,
        };
        let quarantined = route_after_disposal(
            vec!["slot quarantined; workspace manager poisoned".to_string()],
            PreparationPhase::CheckoutMaterialization,
            CheckoutContinuationOutcome::PreparationRetryable {
                claim: report_claim(),
                phase: PreparationPhase::CheckoutMaterialization,
            },
        );
        assert!(matches!(
            quarantined,
            CheckoutContinuationOutcome::ReconciliationRequired {
                quarantine_required: true,
                teardown_unproven: true,
                ..
            }
        ));
        // An empty diagnostics vector = a fully clean disposal → the intended outcome stands.
        let ok = route_after_disposal(
            vec![],
            PreparationPhase::CheckoutMaterialization,
            clean,
        );
        assert!(matches!(
            ok,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));
    }

    #[test]
    fn route_teardown_unproven_seals_and_requires_reconciliation() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            },
            usage(1, 1),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["seal:CheckoutMaterialization"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                teardown_unproven: true,
                quarantine_required: true,
                usage_unrepresentable: false,
                ..
            }
        ));
    }
}
