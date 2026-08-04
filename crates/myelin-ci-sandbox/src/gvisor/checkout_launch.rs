//! The checkout orchestration seam on [`GvisorBackend`]: the outer orchestrator (steps 1-14), the
//! continuation that owns the acquired capsule (steps 15-25), and the failure classification that
//! routes each phase's refusal to a typed continuation outcome.

use super::*;
use crate::hardening::HardeningProfile;
use crate::runner::{
    PreparationPhase,
    RetryableAttemptCause,
};
use crate::workspace_manager::WorkspaceManager;
use crate::{
    CheckoutAuthorizationScope, HookError, JobSpec, PhaseAuthorization, ResourceUsage,
    RunTokenCredential, RunnerHooks, SandboxCancellation,
    SandboxHandle, SandboxLaunch, SandboxOutputSink,
};
use std::path::Path;
use std::sync::Arc;

/// **CT-007 slice 5b.3-6c (Sol's r5 finding 1): an RAII guard that disposes a still-retained NotStarted
/// checkout capsule SAFELY on ANY early exit or unwind before Hop B.** The 6a capsule's bare `Drop`
/// POISONS the workspace manager + quarantines the userns slot; this guard instead performs the SAFE
/// NotStarted cleanup (delete workspace + `release_unused`) via `dispose_checkout_runtime`, converting
/// poison-on-bare-drop into safe-cleanup-on-any-exit. The continuation moves the capsule INTO the guard
/// immediately; the success path [`disarm`](NotStartedCapsuleGuard::disarm)s it into `hop_b`; every
/// other `return`/`?`/`panic!` before Hop B runs this `Drop` — the resource-safety property is
/// RAII-enforced (no syntactic pin can be evaded). `Drop` cannot return diagnostics, so the EXPLICIT
/// failure paths `disarm` and dispose explicitly to produce the typed `ReconciliationRequired`/requeue
/// outcome; this `Drop` is the BACKSTOP for genuinely-unexpected exits/unwinds.
struct NotStartedCapsuleGuard<'a> {
    capsule: Option<checkout_runtime::AcquiredCheckoutRuntime>,
    workspace_manager: &'a WorkspaceManager,
}

impl<'a> NotStartedCapsuleGuard<'a> {
    pub(super) fn new(
        capsule: checkout_runtime::AcquiredCheckoutRuntime,
        workspace_manager: &'a WorkspaceManager,
    ) -> Self {
        Self {
            capsule: Some(capsule),
            workspace_manager,
        }
    }

    /// Take the (still-NotStarted) capsule out — for the success path (move into Hop B) or the explicit
    /// failure path (dispose explicitly to produce the typed outcome). The consumed guard's `Drop` then
    /// runs with `None` (harmless).
    fn disarm(mut self) -> checkout_runtime::AcquiredCheckoutRuntime {
        self.capsule
            .take()
            .expect("the guard still holds the capsule")
    }
}

impl Drop for NotStartedCapsuleGuard<'_> {
    fn drop(&mut self) {
        if let Some(capsule) = self.capsule.take() {
            // SAFE NotStarted cleanup (delete workspace + release_unused) — NEVER the poisoning bare
            // drop. An unexpected early-exit/unwind before Hop B lands here. Diagnostics cannot be
            // returned from `Drop`; the explicit failure paths already produced the typed outcome, so
            // this is the backstop for genuinely-unexpected exits only.
            let _diagnostics = capsule.dispose_checkout_runtime(self.workspace_manager);
        }
    }
}

/// CT-007 slice 5b.3-6b: the dormant checkout-continuation seam lives in its OWN `impl` block, kept
/// OUT of the compute `impl` above. The closed-world dormancy pins scope `launch_with`/
/// `launch_compute_with` by source SPAN (signature → the impl's column-0 close), so a capsule-naming
/// method sharing that impl would be swept into the "compute path names no capsule" assertion and
/// break it. A separate impl keeps the compute span capsule-free while still proving this seam
/// dormant with its own pins.
impl GvisorBackend {
    /// CT-007 slice 5b.3-6b: the DORMANT checkout-continuation seam — 5b.3-6c implements its body (the
    /// dormant sandbox orchestrator) and 5b.3-6e selects/activates it — NOT wired into
    /// [`Self::launch_with`]'s dispatch, ZERO callers, so it introduces no reachable behavior (a
    /// checkout-bearing job still runs the ordinary compute path via `launch_with` today). It accepts
    /// the 5b.3-6a provenance capsule ([`checkout_runtime::AcquiredCheckoutRuntime`]) BY VALUE — the
    /// single `ManagedWorkspace`+`UserNamespaceLease`+`CheckoutPreparationSession` the whole
    /// preparation→workload sequence shares — so 6e can drive Hop B through the capsule
    /// (`Acquired → Prepared`), bind the workload lease (`Prepared → Bound`), and spawn the workload
    /// under `run` against the capsule's OWN retained workspace/`OciConfig`, rather than acquiring a
    /// fresh one (the very workspace/lease collision `launch_compute_with`'s own acquisition would
    /// otherwise create for a checkout job).
    ///
    /// It is deliberately a PURE STUB in 6b. Fleshing it out requires reaching the capsule's retained
    /// `enabled_context`/`workload_cfg`/`session` to run the workload core against a PRE-BUILT context
    /// — i.e. a workload-core extraction (splitting `launch_compute_with`'s run/settle/finalize tail
    /// from its own workspace-acquisition head) plus, potentially, a new capsule accessor. Both are
    /// 5b.3-6c/6e changes that would touch the 6a capsule's closed-world inseparability surface; doing
    /// them here would exceed this behavior-preserving slice. So 6b pins ONLY the seam's SIGNATURE and
    /// leaves the body to 6c (the orchestrator), with 6e selecting it — nothing here NAMES a capsule
    /// field, so 6a's module-private inseparability guarantee (and its module-shape audit) is untouched.
    /// CT-007 slice 5b.3-6c: **the Hop-B-onward checkout continuation (steps 15–25), DORMANT.** Takes
    /// the post-Hop-A capsule BY VALUE and, lending the parent-attempt `authority` the outer
    /// orchestrator retains, drives: begin the materialization phase (15); mint+authorize the
    /// materialization credential (16); run Hop B fused into `Acquired → Prepared` (17–18); then the
    /// ONE closed capsule workload transition (19–25). It is production-shaped (fixed real Hop B +
    /// fixed real workload runner, threading the SAME `cancellation` + output sink), but has ZERO
    /// production callers until 5b.3-6e selects it. `launch_with` still never shape-diverts.
    #[allow(dead_code, clippy::too_many_arguments, clippy::result_large_err)]
    fn launch_checkout_continuation(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        authority: &dyn crate::checkout_orchestration::AttemptAuthority,
        report_claim: &crate::runner::PreparationReportClaim,
        scope: &CheckoutAuthorizationScope,
        runtime: checkout_runtime::AcquiredCheckoutRuntime,
        preparation_spec: CheckoutPreparationSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        cancellation: &SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    > {
        self.launch_checkout_continuation_given(
            spec,
            hooks,
            authority,
            report_claim,
            scope,
            runtime,
            preparation_spec,
            workspace_manager,
            rootfs,
            // Fixed real Hop B — the fused V2 preparation, threading the SAME cancellation + sink.
            |runtime, prep_spec, run_token, authorization| {
                checkout_runtime::run_checkout_preparation_v2(
                    runtime,
                    prep_spec,
                    run_token,
                    authorization,
                    cancellation.as_atomic(),
                    output.clone(),
                )
            },
            // Fixed real workload runner — the closed capsule op, threading the SAME cancellation + sink.
            |prepared, authority, hooks, spec, workspace_manager, rootfs| {
                prepared.run_retained_workload(
                    authority,
                    hooks,
                    spec,
                    workspace_manager,
                    rootfs,
                    cancellation.clone(),
                    output.clone(),
                )
            },
        )
    }

    /// The shared body of [`Self::launch_checkout_continuation`] over an injectable Hop B op and an
    /// injectable workload runner op — both with the EXACT production ownership signature. Production
    /// hardwires the two ops to the fused V2 preparation + the closed capsule workload transition; the
    /// deterministic unit tests inject fakes (a capsule test transition + a fake workload spawn) so the
    /// full steps 15–25 sequence runs with no `runsc`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_checkout_continuation_given<HopB, RunWorkload>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        authority: &dyn crate::checkout_orchestration::AttemptAuthority,
        report_claim: &crate::runner::PreparationReportClaim,
        scope: &CheckoutAuthorizationScope,
        runtime: checkout_runtime::AcquiredCheckoutRuntime,
        preparation_spec: CheckoutPreparationSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        hop_b: HopB,
        run_workload: RunWorkload,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    >
    where
        HopB: FnOnce(
            checkout_runtime::AcquiredCheckoutRuntime,
            CheckoutPreparationSpec,
            RunTokenCredential,
            PhaseAuthorization,
        ) -> Result<
            checkout_runtime::PreparedCheckoutRuntime,
            (
                checkout_runtime::AcquiredCheckoutRuntime,
                CheckoutPreparationError,
            ),
        >,
        RunWorkload: FnOnce(
            checkout_runtime::PreparedCheckoutRuntime,
            &dyn crate::checkout_orchestration::AttemptAuthority,
            &RunnerHooks,
            &JobSpec,
            &WorkspaceManager,
            &Path,
        ) -> RetainedWorkloadOutcome,
    {
        use crate::checkout_orchestration::{
            requeue_or_exhausted, route_after_disposal, CheckoutContinuationOutcome,
            CheckoutOrchestrationError,
        };
        use crate::runner::PreparationPhase;
        use crate::CheckoutPhase;

        const MATERIALIZATION: PreparationPhase = PreparationPhase::CheckoutMaterialization;

        // **Sol's r5 finding 1 (RAII GUARD — the terminal, language-enforced fence).** Move the acquired
        // NotStarted capsule INTO an RAII guard IMMEDIATELY. From here, EVERY early `return`/`?`/`panic!`
        // before Hop B runs the guard's `Drop`, which performs the SAFE NotStarted cleanup (delete
        // workspace + release_unused) — the manager stays healthy, the slot reusable — instead of the
        // capsule's poison-on-bare-drop. No syntactic pin is required and none can be evaded: even
        // Sol's `if cond { return Ok(...); }` (an implicit drop with no `drop(runtime)` token) now
        // disposes safely, and any `drop(runtime)` is a use-after-move compile error (the capsule was
        // moved into the guard). `runtime` is no longer separately owned by this scope.
        let capsule_guard = NotStartedCapsuleGuard::new(runtime, workspace_manager);

        // The fallible pre-Hop-B authority steps run in a closure that BORROWS `authority`/`hooks`/
        // `spec`/`scope` (it never names the guard/capsule). `Err(bool)` carries `phase_was_begun` for
        // the disposal routing (begin failure → false; mint/authorize → true).
        let prepare_materialization =
            || -> Result<(RunTokenCredential, PhaseAuthorization), bool> {
                authority.begin_phase(MATERIALIZATION).map_err(|_| false)?;
                let carrier = authority
                    .mint_phase_credential(CheckoutPhase::Materialization)
                    .map_err(|_| true)?;
                // Authorize against, and thread, the MATERIALIZATION generation's OWN phase-local spec —
                // never the stale advertise-bound base, whose JTI a real Identity gate would reject.
                crate::checkout_orchestration::authorize_phase_generation(
                    hooks,
                    spec,
                    scope,
                    CheckoutPhase::Materialization,
                    carrier,
                )
                .map_err(|_| true)
            };
        let (run_token, authorization) = match prepare_materialization() {
            Ok(pair) => pair,
            Err(phase_was_begun) => {
                // Explicit failure disposal producing the typed outcome (with diagnostics →
                // reconciliation). `disarm()` is consumed DIRECTLY as the argument expression (Sol's r6
                // finding 1): there is NO intervening bare capsule local, so no early-exit can be
                // inserted between the disarm and the consume to drop a bare capsule.
                return Ok(resolve_post_acquisition_authority_failure(
                    authority,
                    report_claim,
                    capsule_guard.disarm(),
                    workspace_manager,
                    phase_was_begun,
                ));
            }
        };

        // Steps 17–18: run Hop B, fused with the `Acquired → Prepared` type transition, threading the
        // SAME materialization credential its authorization retained. SUCCESS: `capsule_guard.disarm()`
        // is consumed DIRECTLY as `hop_b`'s first argument — no intermediate bare local, no window.
        let prepared = match hop_b(
            capsule_guard.disarm(),
            preparation_spec,
            run_token,
            authorization,
        ) {
            Ok(prepared) => prepared,
            Err((runtime, error)) => {
                // Hop B failed — dispose the capsule along its exact session disposition FIRST (no
                // error crosses back until it is safely disposed/quarantined). The materialization
                // journal row is `started`, so it MUST be resolved: `route_preparation_disposition`
                // completes it (measured usage) or SEALS it at ceiling per the disposition — including
                // the teardown-unproven/unreleasable Hop B errors (Sol's finding 2: those must seal
                // IMMEDIATELY, not wait for a later sealer sweep). If disposal ALSO quarantined a
                // resource, the resource may still be live, so the OUTCOME is `ReconciliationRequired`
                // (Sol's finding 4) — but the phase is resolved either way.
                let disposition = error.attempt_disposition();
                let usage = checkout_preparation_error_usage(&error);
                let diagnostic = checkout_preparation_error_diagnostic(&error).to_owned();
                let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
                return Ok(crate::checkout_orchestration::resolve_hop_b_failure(
                    authority,
                    report_claim,
                    disposition,
                    usage,
                    Some(diagnostic),
                    diagnostics,
                )?);
            }
        };

        // Steps 19–25: the closed capsule workload transition (which mints/rotates the workload
        // generation internally — finding 1). Every failure branch already disposed the capsule and
        // carries its disposal diagnostics; honour a quarantine as reconciliation (finding 4).
        let outcome = run_workload(prepared, authority, hooks, spec, workspace_manager, rootfs);
        match outcome {
            RetainedWorkloadOutcome::Ran(Ok(container_run)) => {
                // Success into workload: the ordinary SandboxLaunch takes over. NEVER call
                // preparation-terminal reporting — the existing workload finalization/reporter (step
                // 26) settles workload usage + every parent-attempt journal row once.
                let ContainerRun {
                    child,
                    bundle_dir,
                    result,
                    run_error,
                } = container_run;
                let guest_id = format!("runsc-{}", spec.idem_token.0);
                self.live
                    .lock()
                    .unwrap()
                    .insert(guest_id.clone(), RunscProc { child, bundle_dir });
                Ok(CheckoutContinuationOutcome::WorkloadLaunched(
                    SandboxLaunch {
                        handle: SandboxHandle { guest_id },
                        result,
                        output_complete: run_error.is_none(),
                    },
                ))
            }
            RetainedWorkloadOutcome::Ran(Err(run_failure)) => {
                // The workload bound + ran, then a post-settle failure surfaced. Classify by whether the
                // launch CAS committed (finding 5): pre-CAS `Uncommitted` → preparation requeue; a
                // committed running claim → the reporter-owned workload retry path.
                Ok(classify_bound_workload_failure(
                    authority,
                    report_claim,
                    run_failure,
                ))
            }
            RetainedWorkloadOutcome::RunFailed {
                failure,
                disposal_diagnostics,
            } => {
                // A pre-finalization workload failure — the capsule was disposed. If disposal
                // quarantined, reconcile (finding 4); else classify the failure (finding 5): a pre-CAS
                // `Uncommitted` (the row is still leased) is a PREPARATION requeue, never a workload
                // running-claim attempt.
                Ok(route_after_disposal(
                    disposal_diagnostics,
                    MATERIALIZATION,
                    classify_bound_workload_failure(authority, report_claim, failure),
                ))
            }
            RetainedWorkloadOutcome::PermitRefused {
                disposal_diagnostics,
                ..
            } => {
                // The workload launch permit was refused after materialization completed — the workload
                // never launched. Reconcile on a quarantined disposal, else requeue-or-exhausted.
                Ok(route_after_disposal(
                    disposal_diagnostics,
                    MATERIALIZATION,
                    requeue_or_exhausted(authority, report_claim, MATERIALIZATION),
                ))
            }
            RetainedWorkloadOutcome::PhaseAuthorityFailed {
                error,
                disposal_diagnostics,
            } => {
                // A workload journal/mint op failed structurally, capsule disposed. A quarantined
                // disposal reconciles (finding 4); an otherwise-clean disposal surfaces the structural
                // authority error.
                if disposal_diagnostics.is_empty() {
                    Err(CheckoutOrchestrationError::Authority(error))
                } else {
                    Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: MATERIALIZATION,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    })
                }
            }
            RetainedWorkloadOutcome::LeaseLost {
                lost,
                disposal_diagnostics,
            } => {
                if disposal_diagnostics.is_empty() {
                    Err(CheckoutOrchestrationError::LeaseLost(lost))
                } else {
                    Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: MATERIALIZATION,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    })
                }
            }
        }
    }
}

/// CT-007 slice 5b.3-6c (Sol's findings 2/3/4): resolve a post-acquisition materialization-phase
/// authority failure that owns an acquired capsule. Disposes the capsule ALWAYS (finding 2 — never a
/// bare `?` that would drop+poison it); if the phase was begun, completes it with zero so the sealer
/// never charges a ceiling (finding 3); routes a quarantined disposal to reconciliation, an otherwise-
/// clean disposal to the structural error's typed outcome (finding 4). A `complete_phase` that itself
/// fails leaves the phase unresolvable → reconciliation.
/// CT-007 slice 5b.3-6c (Sol's finding 3): resolve a begun CheckoutTransport phase when advertise
/// mint/authorization fails BEFORE Hop A acquires anything (no capsule exists). Completes the begun
/// phase with zero so the sealer never charges a ceiling, then routes requeue/exhaustion. A
/// `complete_phase` that itself fails leaves the phase unresolvable → reconciliation.
fn resolve_begun_transport_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    use crate::checkout_orchestration::CheckoutContinuationOutcome;
    use crate::runner::PreparationPhase;
    if authority
        .complete_phase(
            PreparationPhase::CheckoutTransport,
            ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
        )
        .is_err()
    {
        return CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutTransport,
            teardown_unproven: false,
            usage_unrepresentable: false,
            quarantine_required: false,
        };
    }
    crate::checkout_orchestration::requeue_or_exhausted(
        authority,
        report_claim,
        PreparationPhase::CheckoutTransport,
    )
}

fn resolve_post_acquisition_authority_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
    runtime: checkout_runtime::AcquiredCheckoutRuntime,
    workspace_manager: &WorkspaceManager,
    phase_was_begun: bool,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    // Dispose the capsule FIRST — the capsule is NotStarted (never bound), so this is the clean
    // delete + release_unused path unless the delete/release cannot be proven — then delegate to the
    // pure (deterministically-tested) router.
    let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
    crate::checkout_orchestration::route_post_acquisition_authority_failure(
        authority,
        report_claim,
        diagnostics,
        phase_was_begun,
    )
}

/// CT-007 slice 5b.3-6c: the exact measured usage a Hop B [`CheckoutPreparationError`] carries (zero
/// for the genuinely-free / pre-spawn variants). The disposition routing settles this exact figure.
fn checkout_preparation_error_usage(error: &CheckoutPreparationError) -> ResourceUsage {
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match error {
        CheckoutPreparationError::Refused(_) => zero,
        CheckoutPreparationError::Unreleasable { usage, .. } => usage.unwrap_or(zero),
        CheckoutPreparationError::TeardownUnproven { usage, .. } => *usage,
        CheckoutPreparationError::RejectedAfterQuiescence { usage, .. } => *usage,
    }
}

/// Retain the checkout materialization diagnostic across the typed disposition seam. These messages
/// are produced by bounded checkout/runtime or host-side verification code; credential carriers are
/// never formatted into them.
fn checkout_preparation_error_diagnostic(error: &CheckoutPreparationError) -> &str {
    match error {
        CheckoutPreparationError::Refused(message)
        | CheckoutPreparationError::Unreleasable { message, .. }
        | CheckoutPreparationError::TeardownUnproven { message, .. }
        | CheckoutPreparationError::RejectedAfterQuiescence { message, .. } => message,
    }
}

/// CT-007 slice 5b.3-6c: the exact measured usage a Hop A [`CheckoutTransportError`] carries.
fn checkout_transport_error_usage(error: &CheckoutTransportError) -> ResourceUsage {
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match error {
        CheckoutTransportError::Refused { .. } => zero,
        CheckoutTransportError::Failed { usage, .. }
        | CheckoutTransportError::TeardownUnproven { usage, .. }
        | CheckoutTransportError::UsageUnrepresentable { usage, .. } => *usage,
    }
}

/// CT-007 slice 5b.3-6c (Sol's finding 5): classify a workload `RunFailure` by whether the workload
/// launch CAS committed. `EnabledPrepared` binds the lease BEFORE `run_and_capture` commits the permit,
/// so a pre-CAS failure (`Uncommitted` — gate construction, runsc spawn, readiness) leaves the queue
/// row `leased`: it belongs to PREPARATION requeue/exhaustion, NOT the workload reporter (whose
/// running-generation accounting requires a durable `running` claim). Only `CommittedButNotExecuted`
/// and `Executed` — where the CAS committed a running claim — go to workload reporting.
fn classify_bound_workload_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
    failure: RunFailure,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    use crate::checkout_orchestration::{requeue_or_exhausted, CheckoutContinuationOutcome};
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match failure {
        // Pre-CAS: the workload CAS never committed, so the row is still leased — this is a PREPARATION
        // requeue/exhaustion, never a running-claim workload attempt.
        RunFailure::Uncommitted { .. } => requeue_or_exhausted(
            authority,
            report_claim,
            PreparationPhase::CheckoutMaterialization,
        ),
        RunFailure::CommitOutcomeUnknown { .. } => {
            CheckoutContinuationOutcome::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: false,
            }
        }
        // Post-CAS (a durable running claim exists) → the existing reporter-owned workload retry path.
        RunFailure::Executed { usage, message } => CheckoutContinuationOutcome::WorkloadRetryable {
            cause: RetryableAttemptCause::SandboxInfrastructure,
            usage,
            message,
        },
        RunFailure::CommittedButNotExecuted { message } => {
            CheckoutContinuationOutcome::WorkloadRetryable {
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: zero,
                message,
            }
        }
    }
}

/// CT-007 slice 5b.3-6c: the outer checkout orchestrator lives in its OWN impl block (kept
/// out of the compute impl, exactly like the continuation seam) so the compute closed-world span stays
/// capsule-free.
impl GvisorBackend {
    /// **The activated outer checkout orchestrator (steps 1–14).** It owns preflight, parent-attempt
    /// admission, Hop A (transport) with the shared renewal checkpoint, transport-phase completion, and
    /// — only AFTER Hop A succeeds — the ONE capsule acquisition, before transferring the capsule BY
    /// VALUE into [`Self::launch_checkout_continuation`] (steps 15–25). It RETAINS the parent-attempt
    /// authority and LENDS it to the continuation. Stage B selects it through `run_cycle`; the V2
    /// production hooks install the required parent-attempt reservation.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(super) fn launch_checkout_orchestrated_with(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &Path,
        cancellation: &SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    > {
        self.launch_checkout_orchestrated_with_given(
            spec,
            hooks,
            repo_root,
            cancellation,
            &|job, cfg, stdin, rootfs, cancellation, permit| {
                run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, permit)
            },
            // Production step 15: the REAL continuation, byte-identical to the prior inline call —
            // same live `authority`/`report_claim`/`runtime`, threading this backend's `cancellation`
            // + `output`. `move` so the `output` sink is owned by the one-shot continuation.
            move |authority,
                  report_claim,
                  scope,
                  runtime,
                  preparation_spec,
                  workspace_manager,
                  rootfs| {
                self.launch_checkout_continuation(
                    spec,
                    hooks,
                    authority,
                    report_claim,
                    scope,
                    runtime,
                    preparation_spec,
                    workspace_manager,
                    rootfs,
                    cancellation,
                    output,
                )
            },
        )
    }

    /// The shared body over an injectable Hop A transport executor AND an injectable step-15
    /// CONTINUATION (CT-007 slice 5b.3-6e.2, the same behavior-preserving extraction pattern as 6b).
    /// Steps 1–14 (preflight, parent-attempt admission, Hop A transport + phase, the ONE capsule
    /// acquisition) are SINGLE-SOURCED here; ONLY the continuation differs. Production
    /// ([`Self::launch_checkout_orchestrated_with`]) passes a `continue_with` that calls the real
    /// [`Self::launch_checkout_continuation`] with the SAME live `authority`/`report_claim`/`runtime`
    /// at exactly step 15; the deterministic test-support driver passes one that calls
    /// [`Self::launch_checkout_continuation_given`] with fake Hop B / workload — so a §4 composed test
    /// drives the identical steps 1–14 and the same live durable authority, substituting ONLY runsc.
    /// The generic monomorphizes (zero-cost); nothing on the production path boxes or `dyn`s.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_checkout_orchestrated_with_given<Continue>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &Path,
        cancellation: &SandboxCancellation,
        transport_execute: GitWireHopExecutor,
        continue_with: Continue,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    >
    where
        Continue: FnOnce(
            &dyn crate::checkout_orchestration::AttemptAuthority,
            &crate::runner::PreparationReportClaim,
            &CheckoutAuthorizationScope,
            checkout_runtime::AcquiredCheckoutRuntime,
            CheckoutPreparationSpec,
            &WorkspaceManager,
            &Path,
        ) -> Result<
            crate::checkout_orchestration::CheckoutContinuationOutcome,
            crate::checkout_orchestration::CheckoutOrchestrationError,
        >,
    {
        use crate::checkout_orchestration::{
            authorize_phase_generation, requeue_or_exhausted, route_after_disposal,
            route_preparation_disposition, AttemptAuthorityLeaseCheckpoint,
            CheckoutContinuationOutcome, CheckoutOrchestrationError, ParentAttemptAdmission,
        };
        use crate::runner::{PreparationPhase, PreparationTerminalDisposition};
        use crate::CheckoutPhase;

        // Step 1/5: preflight — isolation floor, hardening assert, image resolution, workspace health.
        // Checkout REQUIRES the Enabled workspace integration (its capsule owns a workspace+lease).
        hooks
            .enforce_isolation_floor(spec)
            .map_err(CheckoutOrchestrationError::Hook)?;
        spec.validate_secret_coverage().map_err(|error| {
            CheckoutOrchestrationError::Hook(HookError(format!(
                "secret injection refused: {error}"
            )))
        })?;
        let profile = HardeningProfile::derive(spec);
        profile
            .assert_enforced()
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e.to_string())))?;
        let registry = self.registry.as_ref().ok_or_else(|| {
            CheckoutOrchestrationError::Hook(HookError(
                "checkout orchestration requires an asset registry for the workload image"
                    .to_string(),
            ))
        })?;
        let verified_rootfs = registry
            .resolve(&spec.image)
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e.to_string())))?;
        // CT-007 #26/#27: derive THIS job's per-job CoW guest root up front (same mechanism the
        // compute path uses in `launch_compute_common_body`). With a rootfs overlay manager installed,
        // `checkout_guest_root.path()` is a fresh per-job OverlayFS merged view (verified base as the
        // read-only lower) — so the checkout workload's `root.path` and its mount-target precreation
        // land in the per-job upper, never the shared pinned base. The guard is held across the whole
        // orchestration and torn down on drop; a failure here (or any early return below) cleans it up.
        // Without a manager it is the verified base itself — the exact pre-integration behavior.
        let checkout_guest_root = self
            .materialize_job_guest_root(
                verified_rootfs,
                &format!("checkout-{}-{}", std::process::id(), unique_suffix()),
            )
            .map_err(|e| {
                CheckoutOrchestrationError::Hook(HookError(format!("per-job rootfs overlay: {e}")))
            })?;
        let cargo_vendor = selected_cargo_vendor(spec, registry)
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e)))?;
        let (workspace_manager, userns_allocator) = match &self.workspace_integration {
            WorkspaceIntegration::Enabled {
                workspace_manager,
                userns_allocator,
            } => {
                workspace_manager.check_health().map_err(|e| {
                    CheckoutOrchestrationError::Hook(HookError(format!(
                        "workspace manager health check failed: {e}"
                    )))
                })?;
                userns_allocator.check_identity().map_err(|e| {
                    CheckoutOrchestrationError::Hook(HookError(format!(
                        "userns allocator identity check failed: {e}"
                    )))
                })?;
                (workspace_manager, userns_allocator)
            }
            WorkspaceIntegration::Disabled => {
                return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration requires the Enabled workspace integration".to_string(),
                )))
            }
        };

        // Step 2: derive the checkout scope from the SAME spec Hop A/Hop B run against; refuse a
        // non-checkout job. The region comes from the resolved authorization context.
        let scope = match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
            Ok(Some(scope)) => scope,
            Ok(None) => {
                return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration called for a non-checkout job".to_string(),
                )))
            }
            Err(reason) => {
                return Err(CheckoutOrchestrationError::Hook(HookError(format!(
                    "deriving the checkout scope failed: {reason}"
                ))))
            }
        };
        let region =
            match &spec.run_token_authorization {
                Some(crate::RunTokenAuthorizationContext::CiJob(ctx)) => ctx.region.clone(),
                None => return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration requires a resolved run-token authorization context \
                     (for the region)"
                        .to_string(),
                ))),
            };
        let tenant = scope.tenant().0.clone();
        let repo = scope.repo_id().to_string();
        let expected = crate::workspace_intent::ExpectedGitCommitId::new(
            scope.commit_hex().to_string(),
            scope.commit_format(),
        )
        .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e)))?;

        // Step 6: parent-attempt admission (reserve + inflight transition + parent row, one txn). The
        // admission carries the preparation REPORTING identity in BOTH arms (CT-007 5b.3-6d STEP 4):
        // an exhausted attempt has no authority but still needs the claim to report its terminal.
        let (report_claim, _reserve, attempt_authority) = match hooks
            .reserve_parent_attempt(spec)
            .map_err(
            CheckoutOrchestrationError::Hook,
        )? {
            ParentAttemptAdmission::Admitted {
                claim,
                reserve,
                attempt_authority,
            } => (claim, reserve, attempt_authority),
            ParentAttemptAdmission::AttemptsExhausted {
                claim,
                reserve: _reserve,
            } => {
                // The durable parent-attempt budget is already exhausted — terminalize, carrying the
                // admission's claim UNCHANGED into the report.
                return Ok(CheckoutContinuationOutcome::PreparationTerminal {
                    claim,
                    disposition: PreparationTerminalDisposition::AttemptsExhausted,
                    diagnostic: None,
                });
            }
        };
        let authority = attempt_authority.as_ref();
        let report_claim = &report_claim;

        // Step 7: begin the transport phase. A failure here begins nothing and owns no capsule.
        if authority
            .begin_phase(PreparationPhase::CheckoutTransport)
            .is_err()
        {
            return Ok(requeue_or_exhausted(
                authority,
                report_claim,
                PreparationPhase::CheckoutTransport,
            ));
        }

        // Step 8: mint + authorize the advertise credential, threading its OWN phase-local spec
        // (finding 1). A mint/authorize failure AFTER the transport phase began must RESOLVE that begun
        // phase (complete zero) and route the typed outcome — never leave it started for the sealer to
        // charge a ceiling (finding 3). No capsule exists yet.
        let advertise_carrier = match authority.mint_phase_credential(CheckoutPhase::Advertise) {
            Ok(carrier) => carrier,
            Err(_error) => return Ok(resolve_begun_transport_failure(authority, report_claim)),
        };
        let (advertise_credential, advertise_authorization) = match authorize_phase_generation(
            hooks,
            spec,
            &scope,
            CheckoutPhase::Advertise,
            advertise_carrier,
        ) {
            Ok(pair) => pair,
            Err(_error) => return Ok(resolve_begun_transport_failure(authority, report_claim)),
        };

        // Step 10: the fetch leg mints + authorizes its OWN generation mid-transport (only after the
        // advertisement retires and the lease renews — the transport drives that ordering internally),
        // against the fetch generation's OWN phase-local spec (finding 1).
        let mut fetch_leg = || -> Result<(RunTokenCredential, PhaseAuthorization), HookError> {
            let carrier = authority
                .mint_phase_credential(CheckoutPhase::Fetch)
                .map_err(|e| HookError(e.to_string()))?;
            authorize_phase_generation(hooks, spec, &scope, CheckoutPhase::Fetch, carrier)
        };

        // Step 9/13: the ONE renewal checkpoint composed from the attempt authority (replaces the
        // Hop A transport's historical `None`).
        let lease_checkpoint = AttemptAuthorityLeaseCheckpoint(authority);

        // Steps 8–11: Hop A V2 transport, under the SAME cancellation object threaded everywhere.
        let transport = fetch_checkout_pack_within_parent_attempt_v2_given(
            repo_root,
            &tenant,
            &region,
            &repo,
            &expected,
            spec.limits,
            advertise_credential,
            advertise_authorization,
            &mut fetch_leg,
            cancellation.as_atomic(),
            Some(&lease_checkpoint),
            transport_execute,
        );
        let (pack, transport_usage) = match transport {
            Ok(outcome) => outcome.into_parts(),
            Err(error) => {
                // Hop A failed — no capsule exists yet, so no capsule cleanup. Complete/seal the
                // transport phase per the disposition and route the typed outcome.
                let disposition = error.attempt_disposition();
                let usage = checkout_transport_error_usage(&error);
                return Ok(route_preparation_disposition(
                    authority,
                    report_claim,
                    disposition,
                    usage,
                    None,
                )?);
            }
        };

        // Step 12: complete the transport phase with the exact aggregate Hop A usage.
        authority.complete_phase(PreparationPhase::CheckoutTransport, transport_usage)?;
        // Step 13: renew before Hop B.
        if let Err(lost) = authority.renew_preparation_lease() {
            return Err(CheckoutOrchestrationError::LeaseLost(lost));
        }

        // Step 14: acquire the ONE workspace/lease capsule — AFTER Hop A (which never touches it).
        // `acquire` owns rollback of any partial acquisition; materialization has not begun yet.
        let runtime = match checkout_runtime::AcquiredCheckoutRuntime::acquire(
            spec,
            &profile,
            checkout_guest_root.path().to_path_buf(),
            workspace_manager,
            userns_allocator,
            cargo_vendor,
        ) {
            Ok(runtime) => runtime,
            Err(failure) => {
                // Sol's finding 1: `acquire` reports whether its OWN rollback was PROVEN clean. An
                // unproven rollback (workspace leak / failed lease release / failed rollback) means the
                // manager may be poisoned and/or the slot quarantined — the resource may still be live,
                // so route ReconciliationRequired, never an ordinary retry that would strand it.
                // Materialization has not begun, so there is no journal phase to resolve either way.
                if failure.reconciliation_required {
                    return Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: PreparationPhase::CheckoutMaterialization,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    });
                }
                return Ok(requeue_or_exhausted(
                    authority,
                    report_claim,
                    PreparationPhase::CheckoutMaterialization,
                ));
            }
        };

        let preparation_spec = match CheckoutPreparationSpec::new(expected, pack, spec.limits) {
            Ok(spec) => spec,
            Err(_reason) => {
                // The capsule is acquired (NotStarted) — dispose it, and honour a quarantined disposal
                // as reconciliation rather than an ordinary requeue (finding 4).
                let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
                return Ok(route_after_disposal(
                    diagnostics,
                    PreparationPhase::CheckoutMaterialization,
                    requeue_or_exhausted(
                        authority,
                        report_claim,
                        PreparationPhase::CheckoutMaterialization,
                    ),
                ));
            }
        };

        // Steps 15–25: transfer the capsule BY VALUE into the injected continuation with the SAME live
        // reporting claim / authority / runtime (CT-007 5b.3-6d STEP 4). Production's `continue_with`
        // calls the real continuation; the test driver's calls the `_given` continuation with fakes —
        // steps 1–14 above are identical for both.
        continue_with(
            authority,
            report_claim,
            &scope,
            runtime,
            preparation_spec,
            workspace_manager,
            checkout_guest_root.path(),
        )
    }
}
