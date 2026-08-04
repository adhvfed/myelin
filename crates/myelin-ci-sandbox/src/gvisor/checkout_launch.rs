//! The checkout orchestration seam on [`GvisorBackend`]: the outer orchestrator (steps 1-14), the
//! continuation that owns the acquired capsule (steps 15-25), and the failure classification that
//! routes each phase's refusal to a typed continuation outcome.

use super::*;
use crate::hardening::HardeningProfile;
use crate::runner::{PreparationPhase, PreparationTerminalDisposition, RetryableAttemptCause};
use crate::workspace_manager::WorkspaceManager;
use crate::{
    CheckoutAuthorizationScope, HookError, JobSpec, PhaseAuthorization, ResourceUsage,
    RunTokenCredential, RunnerHooks, SandboxCancellation, SandboxHandle, SandboxLaunch,
    SandboxOutputSink,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::runner::PreparationAttemptDisposition;
    #[cfg(feature = "test-support")]
    use std::sync::Mutex;

    use crate::runner::PreparationPhase;

    use std::sync::Arc;

    use crate::gvisor::test_fixtures::*;
    use crate::{
        CompletionSettlementOwner, HookError, LaunchPermit, ReserveHandle, ResourceUsage,
        RunnerHooks, SandboxBackend, SandboxCancellation, SandboxLaunchError, SandboxOutputSink,
    };

    /// The legacy-mode `RunnerHooks` (every production constructor) selects the legacy reserve and
    /// REFUSES parent-attempt admission — the dormancy gate that keeps the V2 path unreachable.
    #[test]
    fn reserve_parent_attempt_refuses_in_legacy_mode() {
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        );
        assert!(matches!(
            hooks.reserve_parent_attempt(&spec(vec![])),
            Err(HookError(_))
        ));
    }

    /// Once the V2 reservation mode is installed, admission returns the injected
    /// [`ParentAttemptAdmission`] (both arms carry the reserve handle).
    #[test]
    fn reserve_parent_attempt_returns_the_installed_admission() {
        use crate::checkout_orchestration::ParentAttemptAdmission;
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_parent_attempt_reservation(Box::new(|_spec| {
            Ok(ParentAttemptAdmission::Admitted {
                claim: report_claim(),
                reserve: ReserveHandle("ci-reserve:v2:a".to_string()),
                attempt_authority: Box::new(FakeAttemptAuthority::new(true)),
            })
        }));
        match hooks
            .reserve_parent_attempt(&spec(vec![]))
            .expect("admitted")
        {
            ParentAttemptAdmission::Admitted { reserve, .. } => {
                assert_eq!(reserve.0, "ci-reserve:v2:a")
            }
            ParentAttemptAdmission::AttemptsExhausted { .. } => panic!("expected Admitted"),
        }
    }

    // ───────── CT-007 slice 5b.3-6c: workload failure-phase + begun-phase routing (always-run) ─────────

    /// Sol's finding 5 + finding 6(c): the workload failure-phase matrix. `EnabledPrepared` binds the
    /// lease BEFORE the launch CAS commits, so a pre-CAS `Uncommitted` failure leaves the row `leased`
    /// → PREPARATION requeue, NEVER a running-claim workload attempt. Only `CommittedButNotExecuted` and
    /// `Executed` (a committed running claim) go to the reporter-owned workload path.
    #[test]
    fn classify_bound_workload_failure_splits_pre_and_post_cas() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);

        // Uncommitted (pre-CAS, row still leased) → preparation requeue.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(
            matches!(
                out,
                CheckoutContinuationOutcome::PreparationRetryable {
                    phase: PreparationPhase::CheckoutMaterialization,
                    ..
                }
            ),
            "a pre-CAS Uncommitted workload failure is a preparation requeue, got {out:?}"
        );

        // CommitOutcomeUnknown → reconciliation (never guessed).
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::commit_outcome_unknown("ambiguous"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::ReconciliationRequired { .. }
        ));

        // CommittedButNotExecuted (running claim) → workload retryable, zero usage.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::committed_but_not_executed("never execed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0
                },
                ..
            }
        ));

        // Executed (running claim, real usage) → workload retryable carrying that usage.
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::executed(
                "teardown infra failed",
                ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6,
                },
            ),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6
                },
                ..
            }
        ));
    }

    /// Sol's finding 5: when the parent-attempt budget is exhausted, a pre-CAS `Uncommitted` workload
    /// failure terminalizes `AttemptsExhausted` rather than requeueing.
    #[test]
    fn classify_uncommitted_terminalizes_when_attempts_are_exhausted() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(false);
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: crate::runner::PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
    }

    /// Sol's finding 3: an advertise mint/authorization failure after `begin_phase(CheckoutTransport)`
    /// must COMPLETE the begun transport phase with zero (never leave it started for the sealer) and
    /// route requeue/exhaustion.
    #[test]
    fn resolve_begun_transport_failure_completes_zero_then_requeues() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);
        let out = resolve_begun_transport_failure(&authority, &report_claim());
        assert_eq!(
            authority.ops.lock().unwrap().clone(),
            vec!["complete:CheckoutTransport:0"],
            "the begun transport phase is completed with zero"
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_routes_a_terminal_hop_b_failure_and_disposes_the_prepared_capsule() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-terminal-hopb")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        // The V2 phase-authorization hook returns the retained (here immediate) permit.
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                // Injected Hop B: a terminal materialization failure that hands the capsule back.
                |runtime, _spec, _run_token, _authorization| {
                    Err((
                        runtime,
                        CheckoutPreparationError::RejectedAfterQuiescence {
                            message: "injected terminal checkout rejection".to_string(),
                            usage: ResourceUsage {
                                cpu_seconds: 4,
                                mem_byte_seconds: 8,
                            },
                            disposition: PreparationAttemptDisposition::Terminal(
                                PreparationTerminalDisposition::Failed {
                                    phase: PreparationPhase::CheckoutMaterialization,
                                },
                            ),
                        },
                    ))
                },
                // The workload runner op must never be reached on a Hop B failure.
                |_prepared, _authority, _hooks, _spec, _wm, _rootfs| {
                    panic!("the workload transition must not run after a Hop B failure")
                },
            )
            .expect("the continuation routes a terminal Hop B failure without a structural error");

        assert!(
            matches!(
                outcome,
                CheckoutContinuationOutcome::PreparationTerminal {
                    disposition: PreparationTerminalDisposition::Failed {
                        phase: PreparationPhase::CheckoutMaterialization
                    },
                    diagnostic: Some(ref diagnostic),
                    ..
                }
                if diagnostic == "injected terminal checkout rejection"
            ),
            "a terminal Hop B failure retains its diagnostic in the preparation-terminal outcome, got {outcome:?}"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string())
                && ops.contains(&"complete:CheckoutMaterialization:4".to_string()),
            "the continuation began, authorized, and completed the materialization phase, got {ops:?}"
        );
        // The capsule was disposed along its session disposition BEFORE the error crossed back — the
        // slot is reusable and the managers stay healthy (this fake hands the capsule back in its
        // as-acquired NotStarted state, so disposal is the delete + release_unused path).
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "disposing the capsule must return the slot to the pool"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// **Sol's r5 finding 1 proof: the RAII guard disposes a NotStarted capsule SAFELY on any early
    /// exit before Hop B.** Simulates Sol's evasion (an early `return`/`?`/`panic!` that implicitly drops
    /// the capsule) by creating the guard and letting it drop WITHOUT disarming — the guard's `Drop` must
    /// run the SAFE NotStarted cleanup (delete workspace + `release_unused`), leaving the workspace
    /// manager HEALTHY and the userns slot REUSABLE — NOT the capsule's poison-on-bare-drop. Gated like
    /// the 6a dispose matrix (real Btrfs+userns); soft-skips otherwise.
    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disposes_safely_on_any_early_exit() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-early-exit")
        else {
            return;
        };
        // An early exit before Hop B: the guard is created and DROPPED without `disarm` (exactly what an
        // injected `if cond { return Ok(...); }` / `?` / panic would do). Its Drop performs safe cleanup.
        {
            let _guard = NotStartedCapsuleGuard::new(runtime, &workspace_manager);
        }
        assert!(
            workspace_manager.is_healthy(),
            "the guard's Drop must NOT poison the manager — it performs the safe NotStarted cleanup"
        );
        assert!(
            userns_allocator.lease().is_ok(),
            "the guard's Drop must release_unused the slot — the pool slot is reusable"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The success path DISARMS the guard: `disarm` hands the capsule back (Drop then a no-op), so the
    /// capsule survives to be moved into Hop B — no double-dispose. Proven by disposing the disarmed
    /// capsule explicitly and observing the slot free exactly once.
    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disarm_hands_back_the_capsule() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-disarm")
        else {
            return;
        };
        let runtime = NotStartedCapsuleGuard::new(runtime, &workspace_manager).disarm();
        // The disarmed guard dropped harmlessly; the capsule is intact — dispose it exactly once.
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean NotStarted disposal, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's finding 6(a): the DETERMINISTIC full-success continuation sequence — begin materialization,
    /// mint + authorize the MATERIALIZATION generation (asserting the phase hook is handed the ROTATED
    /// materialization spec, not the advertise base — finding 1), fused Hop B → Prepared, then a workload
    /// that launches → `WorkloadLaunched`. Uses a real capsule (gated like the 6a matrix) but a synthetic
    /// workload op (no `runsc`/userns policy needed); the workload's OWN generation threading is proven
    /// separately by `run_retained_workload_given` below.
    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_full_success_threads_materialization_generation_and_launches() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-full-success")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let seen_materialization_jti = Arc::new(Mutex::new(None::<String>));
        let seen = seen_materialization_jti.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(move |s, _scope, phase| {
            if phase == crate::CheckoutPhase::Materialization {
                *seen.lock().unwrap() = Some(s.run_token.jti.clone());
            }
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                // Hop B success: drive the real session/lease to Prepared, wrapping test evidence.
                |runtime, _spec, _run_token, _authorization| {
                    Ok(runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(
                        ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                    )))
                },
                // Synthetic workload success: dispose the Prepared capsule CLEANLY (no runsc/userns
                // policy) and report a launched workload. The continuation maps Ran(Ok) → WorkloadLaunched.
                |prepared, _authority, _hooks, _spec, wm, _rootfs| {
                    let diagnostics = prepared.dispose_checkout_runtime(wm);
                    assert!(
                        diagnostics.is_empty(),
                        "the Prepared capsule disposes cleanly (release_prepared), got {diagnostics:?}"
                    );
                    RetainedWorkloadOutcome::Ran(Ok(fake_run()))
                },
            )
            .expect("the full-success continuation returns a launched workload");

        assert!(
            matches!(outcome, CheckoutContinuationOutcome::WorkloadLaunched(_)),
            "the full success sequence launches the workload, got {outcome:?}"
        );
        // Finding 1: the materialization phase hook was handed the ROTATED materialization generation.
        assert_eq!(
            seen_materialization_jti.lock().unwrap().as_deref(),
            Some("jti-Materialization"),
            "the materialization phase authorized against its OWN rotated spec, not the advertise base"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string()),
            "the continuation began + minted the materialization generation, got {ops:?}"
        );
        // The workload launched, then the synthetic op disposed the capsule cleanly → slot reusable.
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        drop(backend);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's finding 2 + 6(b): a post-acquisition authority failure (begin_phase OR mint) must DISPOSE
    /// the NotStarted capsule cleanly (delete workspace + release_unused) rather than dropping it (which
    /// would poison the manager + quarantine the slot), and return a clean typed requeue outcome — never
    /// permanently halt workspace admission.
    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_disposes_capsule_on_authority_failure_without_poisoning() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        for (label, authority) in [
            ("begin_phase", FakeAttemptAuthority::failing_begin_phase()),
            ("mint_phase", FakeAttemptAuthority::failing_mint_phase()),
        ] {
            let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
                acquire_real_checkout_capsule(&format!("continuation-authfail-{label}"))
            else {
                return;
            };
            let backend = GvisorBackend::new(test_registry());
            let spec = checkout_spec();
            let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
                .unwrap()
                .unwrap();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::TerminalReporter,
                Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
                Box::new(|_, _, _| Ok(())),
                Box::new(|_| Ok(())),
                Box::new(|_| Ok(())),
            )
            .with_checkout_phase_authorization(Box::new(|_s, _scope, _phase| {
                Ok(LaunchPermit::immediate())
            }));
            let preparation_spec = CheckoutPreparationSpec::new(
                crate::workspace_intent::ExpectedGitCommitId::new(
                    scope.commit_hex().to_string(),
                    scope.commit_format(),
                )
                .unwrap(),
                PrefetchedCheckoutPack::for_tests(),
                spec.limits,
            )
            .unwrap();

            let outcome = backend
                .launch_checkout_continuation_given(
                    &spec,
                    &hooks,
                    &authority,
                    &report_claim(),
                    &scope,
                    runtime,
                    preparation_spec,
                    &workspace_manager,
                    std::path::Path::new("/abs/staged-rootfs"),
                    |_runtime, _spec, _rt, _auth| {
                        panic!("Hop B must not run after an authority failure")
                    },
                    |_prepared, _a, _h, _s, _wm, _r| panic!("the workload must not run"),
                )
                .unwrap_or_else(|e| {
                    panic!("{label}: authority failure must be a typed outcome, not {e:?}")
                });

            // The capsule was disposed cleanly (NotStarted → delete + release_unused) — NOT poisoned.
            assert!(
                matches!(
                    outcome,
                    CheckoutContinuationOutcome::PreparationRetryable { .. }
                        | CheckoutContinuationOutcome::PreparationTerminal { .. }
                ),
                "{label}: a clean-disposal authority failure yields a typed requeue/terminal, got {outcome:?}"
            );
            assert!(
                workspace_manager.is_healthy(),
                "{label}: the manager must NOT be poisoned by a dropped capsule"
            );
            assert!(
                userns_allocator.lease().is_ok(),
                "{label}: the slot must be released (not quarantined) — workspace admission stays open"
            );
            drop(backend);
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }
    }

    /// **CT-007 slice 5b.3-6e.2 Stage A: the composed active-path proofs (PG-free).** These drive the
    /// REAL outer checkout orchestrator (`launch_checkout_orchestrated_with_given`, steps 1–14
    /// single-sourced) through the hardware-independent runsc-driver seam, substituting ONLY the
    /// hardware (the Hop-A git-container execution + the workload runsc spawn), and prove the active
    /// path settles cleanly before Stage B ever selects it — with NO control-plane.
    mod orchestrated_active_path_6e2 {
        use super::*;
        use crate::checkout_orchestration::ParentAttemptAdmission;
        use crate::gvisor::checkout_transport_test_support::{
            checkout_spec_for_backend, deterministic_enabled_backend_for_tests,
        };

        fn unique_root(tag: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!(
                "myelin-6e2-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        /// Hooks whose `reserve_parent_attempt` admits with the no-op test-support authority, and whose
        /// checkout + per-phase authorizations pass — the minimal V2 wiring the dormant orchestrator
        /// needs to progress the whole advertise → fetch → materialization → workload sequence PG-free.
        fn admitting_hooks() -> RunnerHooks {
            ok_hooks()
                .with_checkout_authorization(Box::new(|_spec, _scope| Ok(())))
                .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
                    Ok(LaunchPermit::immediate())
                }))
                .with_parent_attempt_reservation(Box::new(|_spec| {
                    Ok(ParentAttemptAdmission::Admitted {
                        claim: report_claim(),
                        reserve: ReserveHandle("ci-reserve:v2:6e2".to_string()),
                        attempt_authority: Box::new(NoOpTestSupportAuthority),
                    })
                }))
        }

        /// The §4 composed CHECKOUT-SUCCESS proof (PG-free variant): the REAL orchestrator drives the
        /// two gated transport hops, the real capsule acquisition, the real Hop-B durable transitions,
        /// and the real materialization/renewal/workload-credential/settle tail — all the way to a clean
        /// workload launch. Substituting ONLY the runsc executions means every composition seam (the
        /// admission handoff, transport-phase begin/complete, advertise→fetch generation ordering, the
        /// two renewals, capsule acquisition) runs for real.
        ///
        /// Gated `test-support`: the runsc-driver seam it exercises lives in the
        /// `#[cfg(feature = "test-support")]` `runsc_driver` module, so this proof EXECUTES under
        /// `--features test-support` (the deterministic substrate this whole slice rests on).
        #[cfg(feature = "test-support")]
        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests"
        )]
        fn orchestrated_checkout_drives_two_gated_hops_to_a_clean_workload_launch() {
            use crate::checkout_orchestration::CheckoutContinuationOutcome;
            use crate::gvisor::checkout_transport_test_support::stage_checkout_repo_root;
            let root = unique_root("orchestrated");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let repo_root = stage_checkout_repo_root(&root.join("repos"));
            let spec = checkout_spec_for_backend(image);
            let hooks = admitting_hooks();

            let (result, recorded) = backend.drive_checkout_cycle_with_substituted_runsc_given(
                &spec,
                &hooks,
                &repo_root,
                "checkout.sentinel",
                b"6e2-provenance-sentinel",
            );

            // Exactly the two scripted transport legs ran — no unused step, and the executor panics on a
            // third call, so a masked extra spawn could not pass silently.
            assert_eq!(
                recorded.len(),
                2,
                "exactly two transport hops must spawn (advertise then fetch): {recorded:?}"
            );
            // Each leg spawned under its OWN durable credential (distinct jti) ...
            assert_ne!(
                recorded[0].0, recorded[1].0,
                "advertise and fetch must spawn under DISTINCT jtis: {recorded:?}"
            );
            // ... and BOTH phase permits committed at the spawn boundary.
            assert!(
                recorded[0].1 && recorded[1].1,
                "both transport permits must commit: {recorded:?}"
            );

            // The full steps 1–25 sequence progressed to a clean workload launch — i.e. the real settle
            // tail succeeded (a failed settle would surface as a non-`WorkloadLaunched` outcome).
            match result {
                Ok(CheckoutContinuationOutcome::WorkloadLaunched(launch)) => {
                    assert!(
                        launch.output_complete,
                        "the substituted workload must complete cleanly"
                    );
                    assert_eq!(
                        launch.result.usage,
                        crate::ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                        "the settled workload carries exactly the substituted workload usage"
                    );
                }
                other => panic!("expected a clean WorkloadLaunched, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        /// **The dormant typed-cycle SELECTOR routes on workspace shape BEFORE any reserve or spawn.** A
        /// checkout-bearing job on a checkout-`disabled()` backend fails closed; a malformed workspace is
        /// refused as neither compute nor checkout; a compute job reaches the compute arm (whose FIRST
        /// admission step — `reserve_parent_attempt` — refuses under legacy hooks, proving the arm was
        /// selected without spawning). Each arm returns a DISTINCT fail-closed diagnostic.
        #[test]
        fn run_cycle_selects_the_gvisor_arm_on_workspace_shape_before_reserve_or_spawn() {
            let root = unique_root("selector");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let sink: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());

            // (Some, Some) checkout-bearing on a checkout-`disabled()` backend → fail closed before
            // reserve/spawn (the deterministic Enabled backend leaves `checkout` disabled()).
            let checkout_spec = checkout_spec_for_backend(image.clone());
            let err = backend
                .run_cycle(
                    &checkout_spec,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a checkout job on a checkout-disabled backend fails closed");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("enabled checkout repository root"),
                    "checkout arm selected; got: {msg}"
                ),
                other => panic!("expected the checkout-arm fail-closed refusal, got {other:?}"),
            }

            // A malformed workspace (repo_ref present, commit absent) → refused as neither compute nor a
            // valid checkout, before reserve/spawn.
            let mut malformed = checkout_spec_for_backend(image.clone());
            malformed.workspace.commit = None;
            let err = backend
                .run_cycle(
                    &malformed,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a malformed workspace is refused");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("malformed workspace"),
                    "malformed arm selected; got: {msg}"
                ),
                other => panic!("expected the malformed-workspace refusal, got {other:?}"),
            }

            // (None, None) compute: the compute arm is reached; its first admission step
            // (`reserve_parent_attempt`) refuses under legacy hooks (no parent-attempt reservation),
            // proving the arm was selected without spawning. The image resolves so preflight passes.
            let mut compute_spec = checkout_spec_for_backend(image);
            compute_spec.workspace = crate::WorkspaceSpec::default();
            let err = backend
                .run_cycle(&compute_spec, &ok_hooks(), sink, SandboxCancellation::new())
                .expect_err("compute under legacy hooks refuses at parent-attempt admission");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("parent-attempt"),
                    "compute arm selected (reached reserve_parent_attempt); got: {msg}"
                ),
                other => panic!("expected the compute-arm reserve refusal, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
