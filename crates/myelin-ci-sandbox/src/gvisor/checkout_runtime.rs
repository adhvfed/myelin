//! CT-007 slice 5b.3-6a: the checkout provenance capsule, isolated in its OWN module so that **Rust's
//! module privacy — not a syntactic guard — enforces field inseparability** (Sol's r4 lesson: a syn
//! walk cannot soundly enforce this while the fields are private to the enormous `gvisor` module,
//! because any descendant module / free function / macro within `gvisor.rs` could name them).
//!
//! The two capsule types bundle ONE checkout-bearing job's entire sandbox identity — the stable
//! workload `container_id` (`myelin-prod-*`), the exact [`CheckoutAuthorizationScope`] derived from the
//! job, the single [`ManagedWorkspace`](crate::workspace_manager::ManagedWorkspace) +
//! [`UserNamespaceLease`](crate::user_namespace::UserNamespaceLease) the whole preparation→workload
//! sequence shares (via [`EnabledLaunchContext`]), the [`CheckoutPreparationSession`] state machine
//! tracking that lease's durable two-phase lifecycle, and the workload's own [`OciConfig`] (the
//! workspace host mount + user-namespace mapping) — into ONE value that cannot be pried apart. Their
//! fields are **module-private**: NOTHING outside this module — no sibling, no free function, no macro
//! expansion, and no descendant module (there are NONE inside here) — can NAME `workload_cfg` /
//! `enabled_context` / `session` / `acquired` / `prepared_checkout_evidence`. The five approved
//! accessors below hand out only whole capsules or borrows obtained INSIDE this module.
//!
//! `ManagedWorkspace::job_key()` is exactly `workload_container_id` (the constructor mints the id and
//! hands the SAME string to `acquire_enabled_workspace`), so the workload's stable identity and its
//! workspace can never diverge. `enabled_context.bind_state` stays `Allocated` throughout Hop B — the
//! SESSION drives the lease's `PreparationBound`/`Prepared` transitions — and flips to `Bound` only at
//! the final workload bind, so the EXISTING `settle_enabled_workspace_and_lease` takes the successful
//! workload over unchanged. The types are deliberately NOT `Clone` (duplicating a capsule would
//! duplicate a live lease/workspace). Deliberately dormant (`#[allow(dead_code)]`): 5b.3-6a lands the
//! capsule and the reshaped Hop B entry; wiring it into `launch_with` is 5b.3-6b/6c.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    acquire_enabled_workspace, checkout_cleanup_plan, execute_cleanup_plan,
    resolve_checkout_preparation_permit, run_checkout_preparation_inner,
    settle_enabled_finalization, unique_suffix, AcquisitionFailure, BoundWorkloadRefusal,
    CheckoutPreparationError, CheckoutPreparationSpec, ContainerRun, EnabledLaunchContext,
    LeaseBindState, OciConfig, PreparedCheckoutEvidence, RealCheckoutCleanupExecutor,
    RetainedWorkloadOutcome, RunFailure, RuntimeFinalization, WorkloadRotatedSpec,
};
// `RuntimePreparation` + `LaunchPermit` are named only by the `#[cfg(test)]` injectable-executor seam
// (production reaches the workload runner solely through the sealed wrapper's fixed-runner method).
#[cfg(test)]
use super::RuntimePreparation;
// CT-007 slice 5b.3-6e.1b/6e.2: the deterministic `test-support` execution seam (below) names the
// REAL workload-bind helper + the synthetic-finalization builder + the evidence-provenance mode from
// the parent module.
#[cfg(any(test, feature = "test-support"))]
use super::{
    bind_prepared_lease_given, finalized_for_test_support, CgroupQuiescenceEvidence,
    RuntimeNamespaceQuiescence, RuntimeQuiescenceEvidence, SubstitutedEvidenceMode,
};
use crate::checkout_orchestration::AttemptAuthority;
use crate::hardening::HardeningProfile;
use crate::runner::PreparationPhase;
use crate::user_namespace::{
    CheckoutPreparationSession, UserNamespaceAllocator, UserNamespaceLease,
};
use crate::workspace_manager::WorkspaceManager;
#[cfg(test)]
use crate::LaunchPermit;
use crate::{
    CheckoutAuthorizationScope, JobSpec, PhaseAuthorization, RunTokenCredential, RunnerHooks,
    SandboxCancellation, SandboxOutputSink,
};

/// The inseparable provenance capsule. Fields are MODULE-PRIVATE (see the module doc) — the language,
/// not a test, forbids naming them anywhere else.
#[allow(dead_code)]
pub(crate) struct AcquiredCheckoutRuntime {
    workload_container_id: String,
    checkout_scope: CheckoutAuthorizationScope,
    enabled_context: EnabledLaunchContext,
    session: CheckoutPreparationSession,
    workload_cfg: OciConfig,
}

/// The type-state successor of [`AcquiredCheckoutRuntime`], reachable ONLY as the `Ok` of
/// [`run_checkout_preparation_v2`] — the Hop B run and the `Acquired → Prepared` transition are FUSED,
/// so the carried [`PreparedCheckoutEvidence`] can only ever be the evidence Hop B just produced
/// against THIS capsule. Wrapping (not re-destructuring) the acquired capsule keeps the trio
/// inseparable across the transition too.
#[allow(dead_code)]
pub(crate) struct PreparedCheckoutRuntime {
    acquired: AcquiredCheckoutRuntime,
    prepared_checkout_evidence: PreparedCheckoutEvidence,
}

#[allow(dead_code)]
impl AcquiredCheckoutRuntime {
    /// The ONE constructor: DERIVE the checkout scope from `spec` itself (via the sanctioned
    /// [`crate::derive_checkout_authorization_scope`] facade — Sol's r1 blocker 1, never a
    /// caller-supplied scope that could disagree with the job), mint the stable workload
    /// `container_id`, and in a SINGLE [`acquire_enabled_workspace`] call create the one
    /// [`ManagedWorkspace`](crate::workspace_manager::ManagedWorkspace)+[`UserNamespaceLease`](crate::user_namespace::UserNamespaceLease)
    /// this whole checkout sequence shares — keyed by that same id. The workload's [`OciConfig`] is
    /// retained INSIDE the capsule (never returned detached, Sol's r1 blocker 2). Every acquisition
    /// failure is a single accumulated diagnostic; nothing is leaked half-acquired.
    pub(crate) fn acquire(
        spec: &JobSpec,
        profile: &HardeningProfile,
        absolute_rootfs: PathBuf,
        workspace_manager: &WorkspaceManager,
        userns_allocator: &UserNamespaceAllocator,
    ) -> Result<AcquiredCheckoutRuntime, AcquisitionFailure> {
        // Blocker 1: the capsule's scope is DERIVED from the same `spec` Hop B will run against — it is
        // not an independent argument that a caller could set to scope A while handing a scope-B spec.
        // A malformed/non-checkout spec is a CLEAN refusal — nothing was ever acquired.
        let checkout_scope =
            match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
                Ok(Some(scope)) => scope,
                Ok(None) => {
                    return Err(AcquisitionFailure::clean(
                        "AcquiredCheckoutRuntime::acquire called for a non-checkout job — its \
                         workspace names neither repo_ref nor commit"
                            .to_string(),
                    ))
                }
                Err(reason) => return Err(AcquisitionFailure::clean(format!(
                    "deriving the checkout authorization scope from the job spec failed: {reason}"
                ))),
            };
        // The stable workload identity — `myelin-prod-*`, EXACTLY the form `launch_with` mints for an
        // ordinary job (this is the workload's id, distinct from Hop B's own `myelin-checkout-*` id
        // minted inside `run_checkout_preparation_inner`). Handed straight to the workspace as its
        // `job_key`, so the two can never diverge.
        let workload_container_id =
            format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());
        let (workload_cfg, enabled_context) = acquire_enabled_workspace(
            spec,
            profile,
            &workload_container_id,
            absolute_rootfs,
            workspace_manager,
            userns_allocator,
        )?;
        let runtime = AcquiredCheckoutRuntime {
            workload_container_id,
            checkout_scope,
            enabled_context,
            session: CheckoutPreparationSession::new(),
            workload_cfg,
        };
        // Blocker 4: the workspace-key/workload-id equality is the load-bearing identity invariant —
        // enforce it as a HARD, fallible check (never `debug_assert`, which release builds strip). On
        // a mismatch (a future acquisition-helper regression), the just-acquired workspace+lease are
        // safely disposed through the SAME disposition machinery — the session is `NotStarted`, so
        // that is delete + `release_unused` — and the acquisition refuses.
        if runtime.enabled_context.workspace.job_key() != runtime.workload_container_id {
            let observed = runtime.enabled_context.workspace.job_key().to_string();
            let expected = runtime.workload_container_id.clone();
            // NotStarted disposal (delete + release_unused): EMPTY diagnostics = the rollback was proven
            // clean → ordinary refusal; ANY diagnostic = a quarantine → reconciliation-required.
            let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
            return Err(AcquisitionFailure::from_rollback_diagnostics(
                format!(
                    "acquired workspace job_key {observed:?} does not equal the stable workload \
                     container id {expected:?} — the just-acquired workspace and lease were disposed"
                ),
                diagnostics,
            ));
        }
        Ok(runtime)
    }

    /// CT-007 slice 5b.3-6a: dispose the capsule's workspace+lease along the ONE safe path its
    /// session's current durable state permits, via the pure [`checkout_cleanup_plan`] mapping — the
    /// type state chooses the release method, so the WRONG one (`release_unused` on a `Prepared`
    /// lease, or `release_prepared` on a never-bound one) is unreachable, not merely avoided by care.
    /// Returns every accumulated diagnostic — empty only when disposal was fully clean.
    pub(crate) fn dispose_checkout_runtime(
        self,
        workspace_manager: &WorkspaceManager,
    ) -> Vec<String> {
        // Read the one authoritative disposition BEFORE moving anything out, and resolve its plan.
        let plan = checkout_cleanup_plan(self.session.cleanup_disposition());
        // Reuse the EXISTING context destructure; the workload container id / cfg are no longer needed
        // once we are disposing. Hand the disassembled resources to the REAL executor and let the one
        // shared `execute_cleanup_plan` drive the exact operation sequence — the same code the
        // always-run trace test exercises.
        let EnabledLaunchContext {
            workspace,
            lease,
            bind_state: _,
        } = self.enabled_context;
        let mut executor = RealCheckoutCleanupExecutor {
            workspace: Some(workspace),
            lease: Some(lease),
            checkout_session: Some(self.session),
            workspace_manager,
        };
        execute_cleanup_plan(plan, &mut executor)
    }
}

#[allow(dead_code)]
impl PreparedCheckoutRuntime {
    /// **CT-007 slice 5b.3-6c: the ONE closed workload transition (steps 19–25).** Consumes the whole
    /// prepared capsule and — WITHOUT ever handing out its retained `OciConfig`/lease/session/workspace
    /// (folding in the old free-standing `bind_workload` synthetic-identity helper, which no longer
    /// exists as an independently callable seam) — drives, in order:
    ///
    /// - **19** complete the materialization journal phase with the RETAINED Hop B usage (read through
    ///   a borrow; the [`PreparedCheckoutEvidence`] itself is never exposed);
    /// - **20** renew the preparation lease before the workload spawns;
    /// - **22** acquire the workload launch permit;
    /// - **23–24** durably bind `Prepared → Bound` at the REAL cgroup identity (via the session, inside
    ///   the workload runner) and spawn the workload against the capsule's own retained context;
    /// - **25** settle the workspace/lease through the shared audited
    ///   [`settle_enabled_finalization`](super::settle_enabled_finalization) tail.
    ///
    /// EVERY failure branch disposes the capsule along its exact session disposition BEFORE returning,
    /// so no live workspace/lease ever leaks. It returns ONLY the owned [`RetainedWorkloadOutcome`] —
    /// never a reference or capability that could outlive the call.
    ///
    /// Production calls the FIXED real workload runner
    /// ([`run_production_container_streaming`](super::run_production_container_streaming)); the
    /// `#[cfg(test)]` `run_retained_workload_given` seam injects a fake spawn. Neither entry accepts an
    /// arbitrary `&OciConfig` callback: the executor is constructed HERE from the plain
    /// cancellation/output the caller passed, so the capsule's cloneable `OciConfig` can never be
    /// detached by a caller-supplied closure.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_retained_workload(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        cancellation: SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> RetainedWorkloadOutcome {
        // Sol's r6 finding 2: the `run_workload` closure receives the SEALED `WorkloadRotatedSpec`
        // wrapper (NOT a `&JobSpec`), and production selects the wrapper's FIXED-runner method
        // `acquire_permit_and_run` — which calls `run_production_container_streaming` internally with its
        // own private `&self.spec`. So no production code holds a `&JobSpec` to clone/substitute.
        self.run_retained_workload_inner(
            authority,
            spec,
            workspace_manager,
            move |workload_spec, cfg, container_id, lease, session, bind_state| {
                workload_spec.acquire_permit_and_run(
                    hooks,
                    cfg,
                    container_id,
                    rootfs,
                    lease,
                    session,
                    bind_state,
                    output,
                    cancellation,
                )
            },
        )
    }

    /// The shared body of [`Self::run_retained_workload`] over a `run_workload` selector. PRIVATE (never
    /// in the audited accessor surface). The selector receives the SEALED [`WorkloadRotatedSpec`]
    /// wrapper — NEVER a `&JobSpec` — so it cannot clone/substitute the spec: production selects the
    /// fixed-runner `acquire_permit_and_run`; tests select the `#[cfg(test)]`
    /// `acquire_permit_and_run_given`. `base_spec` is used EXACTLY ONCE (the rotation into the wrapper).
    #[allow(clippy::too_many_arguments, private_interfaces, private_bounds)]
    fn run_retained_workload_inner<R>(
        mut self,
        authority: &dyn AttemptAuthority,
        base_spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        run_workload: R,
    ) -> RetainedWorkloadOutcome
    where
        R: FnOnce(
            &WorkloadRotatedSpec,
            &OciConfig,
            &str,
            &mut UserNamespaceLease,
            &mut CheckoutPreparationSession,
            &mut LeaseBindState,
        ) -> Result<
            Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
            BoundWorkloadRefusal,
        >,
    {
        // Step 19: complete the materialization phase with the RETAINED Hop B usage (borrow only).
        let materialization_usage = self.prepared_checkout_evidence.preparation_usage();
        if let Err(error) = authority.complete_phase(
            PreparationPhase::CheckoutMaterialization,
            materialization_usage,
        ) {
            let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
            return RetainedWorkloadOutcome::PhaseAuthorityFailed {
                error,
                disposal_diagnostics,
            };
        }
        // Step 20: renew the preparation lease before the workload spawns.
        if let Err(lost) = authority.renew_preparation_lease() {
            let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
            return RetainedWorkloadOutcome::LeaseLost {
                lost,
                disposal_diagnostics,
            };
        }
        // Step 21: mint the WORKLOAD credential — a SEPARATE, type-distinct operation from the
        // preparation mints, then rotate it into the workload's OWN phase-local spec (base spec with
        // ONLY the credential + ephemeral authorization context replaced). The permit is acquired
        // against, and the workload runs under, THIS spec — not the stale advertise-bound base.
        let workload_carrier = match authority.mint_workload_credential() {
            Ok(carrier) => carrier,
            Err(error) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::PhaseAuthorityFailed {
                    error,
                    disposal_diagnostics,
                };
            }
        };
        // Rotate ONCE onto the workload generation into the SEALED wrapper. This is the ONLY use of
        // `base_spec` on the workload path; from here only the `WorkloadRotatedSpec` wrapper travels, and
        // its inner `JobSpec` NEVER escapes (no `as_job_spec`, no `Clone`) — so no code can clone or
        // substitute the spec. The `run_workload` selector receives the WRAPPER (never a `&JobSpec`) and
        // the disjoint capsule borrows; production selects the fixed-runner method, tests the cfg(test)
        // one (Sol's r6 finding 2).
        let workload_spec = WorkloadRotatedSpec::from_carrier(&workload_carrier, base_spec);
        // Steps 22–24: acquire the permit + revalidate + build the EnabledPrepared binding + spawn — ALL
        // inside the selected wrapper method, which can ONLY see its own private rotated spec.
        let outer_result = match run_workload(
            &workload_spec,
            &self.acquired.workload_cfg,
            &self.acquired.workload_container_id,
            &mut self.acquired.enabled_context.lease,
            &mut self.acquired.session,
            &mut self.acquired.enabled_context.bind_state,
        ) {
            Ok(outer_result) => outer_result,
            Err(BoundWorkloadRefusal::PermitRefused(message)) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::PermitRefused {
                    message,
                    disposal_diagnostics,
                };
            }
            Err(BoundWorkloadRefusal::PrepModeMismatch(message)) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::RunFailed {
                    failure: RunFailure::uncommitted(message),
                    disposal_diagnostics,
                };
            }
        };
        // The `prep` borrows (held inside the helper) have ended.
        match outer_result {
            Err(failure) => {
                // A pre-finalization failure (bundle staging, cgroup, the durable workload bind, or a
                // spawn failure before a trustworthy result). Dispose the capsule along its session
                // disposition — a failed bind left it Prepared/Unreleasable (Bound is unreachable here,
                // exactly like the compute pre-bind branch).
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                RetainedWorkloadOutcome::RunFailed {
                    failure,
                    disposal_diagnostics,
                }
            }
            Ok(finalization) => {
                // Step 25: settle the capsule's OWN enabled context through the shared audited tail.
                // Move the enabled context out of the capsule (the now-`Done` session drops harmlessly).
                let PreparedCheckoutRuntime { acquired, .. } = self;
                let AcquiredCheckoutRuntime {
                    enabled_context, ..
                } = acquired;
                let settled = settle_enabled_finalization(
                    finalization,
                    Some(enabled_context),
                    Some(workspace_manager),
                );
                RetainedWorkloadOutcome::Ran(settled)
            }
        }
    }

    /// Dispose a prepared-but-not-workload-bound capsule (e.g. the workload's own launch permit was
    /// refused): delegate to the inner capsule's [`AcquiredCheckoutRuntime::dispose_checkout_runtime`].
    pub(crate) fn dispose_checkout_runtime(
        self,
        workspace_manager: &WorkspaceManager,
    ) -> Vec<String> {
        self.acquired.dispose_checkout_runtime(workspace_manager)
    }
}

/// **The V2 (phase-bound) Hop B entry point, FUSED with the type transition.** Consumes the opaque
/// [`PhaseAuthorization`] — which can only have come from one `authorize_checkout_phase` invocation —
/// into the retained durable permit the preparation container spawns under, then, on success, returns
/// the [`PreparedCheckoutRuntime`] directly. Because the Hop B run and the `Acquired → Prepared`
/// transition are ONE call (Sol's r1 blocker 3), the prepared capsule's evidence can only ever be the
/// evidence Hop B just produced against THIS capsule — there is no free-standing `into_prepared` that
/// would take caller-supplied evidence from a different acquisition.
///
/// Consumption checks the `Materialization` phase, the authorization's privately-retained run-token
/// JTI against `run_token`, AND (Sol's r1 blocker 1) the authorization's ENTIRE scope against the
/// capsule's own derived `checkout_scope` — a capsule acquired for scope A refuses an authorization
/// minted for scope B before any spawn. It reaches the capsule's lease, session, and workspace ONLY
/// through DISJOINT field borrows (never three independently pairable arguments). On any failure it
/// hands the capsule back intact so the caller can dispose it along its session's disposition.
///
/// There is deliberately NO legacy/immediate option here: this entry point cannot construct or accept
/// one, so "a V2 caller cannot select the legacy arm" is enforced by the type system.
// The `Err` variant deliberately carries the WHOLE capsule back so the caller can dispose it along its
// session's disposition — that is the point (blocker 3's fusion). Boxing it would only add an
// allocation on the failure path; the `Ok` capsule is equally large, so nothing is saved.
#[allow(dead_code, clippy::result_large_err, clippy::too_many_arguments)]
pub(crate) fn run_checkout_preparation_v2(
    mut runtime: AcquiredCheckoutRuntime,
    spec: CheckoutPreparationSpec,
    run_token: RunTokenCredential,
    authorization: PhaseAuthorization,
    // CT-007 slice 5b.3-6c: the SAME cancellation object + output sink threaded through Hop A / Hop B
    // / workload — Hop B no longer runs under `NEVER_CANCELLED` once composed.
    cancellation: &std::sync::atomic::AtomicBool,
    output: Option<Arc<dyn SandboxOutputSink>>,
) -> Result<PreparedCheckoutRuntime, (AcquiredCheckoutRuntime, CheckoutPreparationError)> {
    // Resolve the permit BEFORE any staging, cgroup, or durable bind — a refusal leaves the capsule's
    // lease, session, and workspace completely untouched, and we hand the whole capsule back.
    let permit = match resolve_checkout_preparation_permit(
        authorization,
        &run_token,
        &runtime.checkout_scope,
        &spec.expected_commit,
    ) {
        Ok(permit) => permit,
        Err(error) => return Err((runtime, error)),
    };
    // Disjoint field borrows off the ONE capsule: `enabled_context.lease` (mut) and
    // `enabled_context.workspace` (shared) are distinct fields of the context, `session` (mut) a
    // distinct field of the capsule — the borrow checker splits them, and no caller could have
    // supplied a mismatched trio. The borrows end when `outcome` is computed, so the capsule can then
    // be moved into the fused transition. (Passing borrows obtained INSIDE this module across the
    // module boundary to `run_checkout_preparation_inner` is fine — privacy is about NAMING the field,
    // not about borrows legitimately obtained here.)
    let outcome = run_checkout_preparation_inner(
        &mut runtime.enabled_context.lease,
        &mut runtime.session,
        &runtime.enabled_context.workspace,
        spec,
        permit,
        cancellation,
        output,
    );
    match outcome {
        Ok(evidence) => Ok(PreparedCheckoutRuntime {
            acquired: runtime,
            prepared_checkout_evidence: evidence,
        }),
        Err(error) => Err((runtime, error)),
    }
}

/// CT-007 slice 5b.3-6c (Sol's finding 6): the `#[cfg(test)]` injectable-executor seam for the closed
/// workload transition. Production reaches the shared inner ONLY through the fixed-real-runner
/// [`PreparedCheckoutRuntime::run_retained_workload`] (which cannot accept an arbitrary `&OciConfig`
/// callback); this lets the DETERMINISTIC full-success + workload-failure-phase tests drive steps
/// 19–25 (including the real `Prepared → Bound` session bind against synthetic identities) with a FAKE
/// spawn — no `runsc` — while still proving the per-phase workload credential/context threading.
#[cfg(test)]
impl PreparedCheckoutRuntime {
    // The `#[cfg(test)]` execution seam's `F` bound names gvisor-module-private RuntimePreparation /
    // RuntimeFinalization — legitimate for a test-only injection point (production `run_retained_workload`
    // exposes none of them).
    #[allow(clippy::too_many_arguments, private_interfaces, private_bounds)]
    pub(crate) fn run_retained_workload_given<F>(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        execute: F,
    ) -> RetainedWorkloadOutcome
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        // The `run_workload` selector receives the SEALED wrapper and selects its `#[cfg(test)]`
        // `acquire_permit_and_run_given` — the ONLY place a `&JobSpec`-receiving `execute` closure lives.
        self.run_retained_workload_inner(
            authority,
            spec,
            workspace_manager,
            move |workload_spec, cfg, container_id, lease, session, bind_state| {
                workload_spec.acquire_permit_and_run_given(
                    hooks,
                    cfg,
                    container_id,
                    rootfs,
                    lease,
                    session,
                    bind_state,
                    execute,
                )
            },
        )
    }
}

/// Test-only session driver (Sol's r4: the behavioral dispose matrix lives in `gvisor::tests`, a
/// SIBLING of this module, so it can no longer reach the private `session`/`enabled_context` fields
/// directly — module privacy is now the enforcer). This helper, defined INSIDE the module, drives a
/// freshly-acquired capsule's session to the requested disposition using the session's OWN real
/// transitions (no `runsc` needed), so the sibling matrix can then observe disposal's durable effect.
#[cfg(test)]
impl AcquiredCheckoutRuntime {
    pub(crate) fn drive_session_for_tests(
        &mut self,
        target: crate::user_namespace::CheckoutSessionCleanup,
    ) {
        use crate::user_namespace::{CheckoutSessionCleanup, PreparationQuiescenceProof};
        let container = "myelin-checkout-drive".to_string();
        let root = (11_u64, 22_u64);
        let cgroup = (33_u64, 44_u64);
        if target == CheckoutSessionCleanup::NeverBound {
            return;
        }
        self.session
            .bind_preparation(
                &mut self.enabled_context.lease,
                container.clone(),
                root,
                cgroup,
            )
            .expect("bind_preparation must succeed on a fresh Allocated lease");
        if target == CheckoutSessionCleanup::TeardownUnproven {
            return;
        }
        let nonce = self.enabled_context.lease.nonce_for_tests();
        let proof =
            PreparationQuiescenceProof::assert_for_tests(nonce, container.clone(), root, cgroup);
        self.session
            .confirm_prepared(&mut self.enabled_context.lease, proof)
            .expect("confirm_prepared must succeed with a matching proof");
        if target == CheckoutSessionCleanup::Prepared {
            return;
        }
        if target == CheckoutSessionCleanup::WorkloadBound {
            self.session
                .bind_workload(
                    &mut self.enabled_context.lease,
                    "myelin-prod-workload".to_string(),
                    root,
                    cgroup,
                )
                .expect("bind_workload must succeed from Prepared");
        }
    }

    /// **CT-007 slice 5b.3-6c: the test-only consuming transition to a `PreparedCheckoutRuntime`.**
    /// Drives the REAL session/lease state machine to `Prepared` (via the same real `bind_preparation
    /// → confirm_prepared` transitions [`drive_session_for_tests`](Self::drive_session_for_tests)
    /// uses), asserts the durable disposition, consumes the whole acquired capsule, and wraps it with
    /// the supplied test evidence. It drives the REAL durable lease state — wrapping a `NotStarted`
    /// capsule would make a workload success test dishonest (the later `Prepared → Bound` bind would
    /// fail). Fabricated evidence is acceptable ONLY here: `#[cfg(test)]` is absent from every ordinary
    /// build (including `test-support`), so the production fused Hop B transition
    /// (`run_checkout_preparation_v2`) remains the ONLY production constructor of a prepared capsule.
    pub(crate) fn into_prepared_for_tests(
        mut self,
        evidence: PreparedCheckoutEvidence,
    ) -> PreparedCheckoutRuntime {
        use crate::user_namespace::CheckoutSessionCleanup;
        self.drive_session_for_tests(CheckoutSessionCleanup::Prepared);
        // Read the disposition into a LOCAL before asserting: the closed-world macro scan flags any
        // capsule-field ident (`session`) appearing INSIDE a macro invocation, so keep `self.session`
        // out of the `assert_eq!` token stream.
        let disposition = self.session.cleanup_disposition();
        assert_eq!(
            disposition,
            CheckoutSessionCleanup::Prepared,
            "into_prepared_for_tests must leave the session durably Prepared"
        );
        PreparedCheckoutRuntime {
            acquired: self,
            prepared_checkout_evidence: evidence,
        }
    }
}

/// **CT-007 slice 5b.3-6e.1b/6e.2: the deliberately-audited `test-support` EXECUTION seam (Hop B
/// half).** Unlike [`AcquiredCheckoutRuntime::into_prepared_for_tests`] (which fabricates the
/// transition with caller-supplied evidence), this seam performs the REAL durable session/lease
/// transitions `Allocated → PreparationBound → Prepared` while SUBSTITUTING only the Hop B runtime
/// EXECUTION (a checked byte-accounted sentinel write in place of the real `git` fetch/checkout), so
/// the checkout-capsule preparation lifecycle RUNS on a host with no Btrfs/subuid/KVM/runsc. It is
/// a SEPARATE `#[cfg(any(test, feature = "test-support"))]` impl (never the `#[cfg(test)]` driver
/// impl): the closed-world audit inventories it as test-support-only WITHOUT enlarging the five-entry
/// production accessor surface.
///
/// HONEST SCOPING: what is substituted here is ONLY the Hop B `git` execution + its measured usage;
/// the materialization permit and durable `Allocated → PreparationBound → Prepared` transitions are
/// REAL. On the workload leg, the launch permit is likewise acquired and committed for real; only
/// explicit-userns policy revalidation and the `runsc` spawn are hardware-gated and deferred to the
/// real-`runsc` drill 5b.3-7.
#[cfg(any(test, feature = "test-support"))]
impl AcquiredCheckoutRuntime {
    /// Drive the REAL Hop B durable transitions with a substituted execution and return the fused
    /// [`PreparedCheckoutRuntime`] plus two owned observations (the checked-write outcome and the
    /// byte-accounted checkpoint) — never a workspace path, lease, session, `OciConfig`, or evidence.
    /// In order: (2a) real `bind_preparation`; (3) the substituted Hop B writes a sentinel into the
    /// capsule's REAL workspace via the CHECKED byte-accounted test-quota op; (2b) real
    /// `confirm_prepared` from a matching synthetic preparation proof. The fused capsule carries a
    /// synthetic (substituted-Hop-B) `PreparedCheckoutEvidence` usage the workload leg then completes
    /// the materialization phase against.
    #[allow(clippy::result_large_err)]
    pub(crate) fn substituted_hop_b_for_test_support(
        mut self,
        run_token: &RunTokenCredential,
        authorization: PhaseAuthorization,
        expected_commit: &crate::workspace_intent::ExpectedGitCommitId,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        injected_disposition: Option<crate::runner::PreparationAttemptDisposition>,
    ) -> Result<
        (PreparedCheckoutRuntime, bool, u64),
        (AcquiredCheckoutRuntime, CheckoutPreparationError),
    > {
        use crate::user_namespace::PreparationQuiescenceProof;

        // Step 17 (REAL, not faked): resolve the materialization launch permit by CONSUMING the fused
        // `PhaseAuthorization` against THIS capsule's own derived scope + the run token + the expected
        // commit — the SAME `resolve_checkout_preparation_permit` production's `run_checkout_preparation_v2`
        // runs BEFORE any staging or durable bind. A mismatched materialization credential (wrong JTI,
        // scope, or commit) is rejected HERE, and the untouched capsule is handed back so the real
        // continuation disposes + routes it exactly as production does. Only the `git` spawn is faked.
        let permit = match resolve_checkout_preparation_permit(
            authorization,
            run_token,
            &self.checkout_scope,
            expected_commit,
        ) {
            Ok(permit) => permit,
            Err(error) => return Err((self, error)),
        };

        // Synthetic-but-CONSISTENT preparation identities: reused for BOTH the real durable
        // transition and the matching synthetic preparation proof, so Hop B validates with no runsc.
        let prep_container = format!("myelin-checkout-substituted-{}", self.workload_container_id);
        let prep_root = (0x0011_u64, 0x0022_u64);
        let prep_cgroup = (0x0033_u64, 0x0044_u64);

        // Step 2a: REAL Allocated -> PreparationBound (durable lease + session transition).
        self.session
            .bind_preparation(
                &mut self.enabled_context.lease,
                prep_container.clone(),
                prep_root,
                prep_cgroup,
            )
            .expect("bind_preparation must succeed on a fresh Allocated lease");

        // Commit the resolved materialization permit at the substituted spawn boundary, BEFORE the
        // fake execution and BEFORE quiescence is confirmed — the same order `run_and_capture` uses
        // for the production Hop-B child. Thus a phase CAS/permit regression cannot be masked by a
        // sentinel write or by prematurely moving the durable session to Prepared.
        if let Err(error) = permit.commit_and_release() {
            // No child was spawned, so the synthetic substrate can prove quiescence immediately and
            // leave the already-bound session releasable. Production likewise routes a launch-boundary
            // failure only after finalization has established quiescence.
            let prep_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                prep_container.clone(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: prep_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
            );
            let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(
                &self.enabled_context.lease,
                &prep_evidence,
            )
            .expect("a matching preparation evidence mints a proof");
            self.session
                .confirm_prepared(&mut self.enabled_context.lease, prep_proof)
                .expect("confirm_prepared with matching evidence must succeed");
            return Err((
                self,
                CheckoutPreparationError::RejectedAfterQuiescence {
                    message: format!("materialization launch permit commit failed: {error}"),
                    usage: crate::ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    },
                    disposition:
                        crate::runner::PreparationAttemptDisposition::RetryableInfrastructure {
                            phase: crate::runner::PreparationPhase::CheckoutMaterialization,
                        },
                },
            ));
        }

        // Step 3: the SUBSTITUTED Hop B either injects a post-spawn result or writes the sentinel into
        // the capsule's REAL workspace through the checked byte-accounted test-quota operation. Both
        // arms have already consumed and committed the real materialization authorization above.
        let hopb_write_ok = injected_disposition.is_none()
            && self
                .enabled_context
                .workspace
                .checked_test_quota_write(sentinel_name, sentinel_bytes)
                .is_ok();
        let used_after_hopb = self
            .enabled_context
            .workspace
            .scan_used_bytes()
            .unwrap_or(0);

        // Step 2b: REAL PreparationBound -> Prepared, confirmed with matching synthetic evidence only
        // after the substituted execution boundary.
        let prep_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            prep_container.clone(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: prep_root,
            },
            CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
        );
        let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(
            &self.enabled_context.lease,
            &prep_evidence,
        )
        .expect("a matching preparation evidence mints a proof");
        self.session
            .confirm_prepared(&mut self.enabled_context.lease, prep_proof)
            .expect("confirm_prepared with a matching proof must succeed");

        if let Some(disposition) = injected_disposition {
            return Err((
                self,
                CheckoutPreparationError::RejectedAfterQuiescence {
                    message: "6e.2 injected Hop-B failure (test-support)".to_string(),
                    usage: crate::ResourceUsage {
                        cpu_seconds: 2,
                        mem_byte_seconds: 5,
                    },
                    disposition,
                },
            ));
        }

        // Fuse into the prepared capsule (wrapping, not re-destructuring — the trio stays inseparable
        // across the transition). The synthetic preparation usage is the substituted Hop B's measured
        // cost; the workload leg completes the materialization phase against it (step 19).
        let prepared = PreparedCheckoutRuntime {
            acquired: self,
            prepared_checkout_evidence: PreparedCheckoutEvidence::for_tests(crate::ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 5,
            }),
        };
        Ok((prepared, hopb_write_ok, used_after_hopb))
    }
}

/// **CT-007 slice 5b.3-6e.2: the workload half of the audited `test-support` EXECUTION seam.** Drives
/// the REAL [`PreparedCheckoutRuntime::run_retained_workload_inner`] body — so the injected
/// [`AttemptAuthority`]'s materialization completion (step 19), preparation-lease renewal (step 20),
/// and workload-credential mint + rotation (step 21) all RUN for real, producing the real durable
/// rows — while the injected workload closure binds directly via the REAL
/// [`bind_prepared_lease_given`](super::bind_prepared_lease_given) (`Prepared → Bound`) and hands back
/// a canned exit-0 finalization, so the REAL [`settle_enabled_finalization`](super::settle_enabled_finalization)
/// tail then runs. HONEST SCOPING: the closure acquires and commits the REAL workload launch permit;
/// it substitutes only explicit-userns policy revalidation and the `runsc` spawn, which are the
/// hardware boundary covered by the real-`runsc` drill 5b.3-7. It substitutes NOTHING about
/// provenance: the workload runtime evidence is DERIVED from the bind's OWN `Bound` output
/// ([`LeaseBindState::Bound`]'s recorded `runsc_root_identity`/`cgroup_identity`), never from an
/// argument, so the settle tail's evidence-vs-recorded-binding provenance check is exercised LIVE.
#[cfg(any(test, feature = "test-support"))]
impl PreparedCheckoutRuntime {
    /// Drive steps 19–25 with a substituted workload execution. Returns the REAL owned
    /// [`RetainedWorkloadOutcome`](super::RetainedWorkloadOutcome) (so the §4 continuation driver can
    /// route it exactly as production does) PLUS the three owned side-observations
    /// `(used_at_workload_checkpoint, mount_source_matched_workspace, sentinel_read_through_mount)` —
    /// never a workspace path, lease, session, `OciConfig`, or evidence. `evidence_mode` selects
    /// whether the workload runtime evidence is DERIVED from the bind's own `Bound` output (the honest
    /// positive path) or deliberately MISMATCHED on `runsc_root_identity` (the negative path proving the
    /// settle-tail provenance check is LIVE, not vacuous). The ONE sealed workload seam both the 6e.1b
    /// in-lib test and the §4 continuation driver share (the private `run_retained_workload_inner` it
    /// drives is unreachable from any sibling module).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn substituted_workload_for_test_support(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        base_spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        evidence_mode: SubstitutedEvidenceMode,
    ) -> (RetainedWorkloadOutcome, u64, bool, bool) {
        // Read the workspace-derived observations BEFORE the capsule is consumed by the inner body.
        let workspace_host = self
            .acquired
            .enabled_context
            .workspace
            .host_path()
            .to_path_buf();
        let used_at_workload_checkpoint = self
            .acquired
            .enabled_context
            .workspace
            .scan_used_bytes()
            .unwrap_or(0);
        // The closure runs synchronously inside `run_retained_workload_inner`; it writes its two OCI-
        // mount observations back through these cells (it cannot return them — its type is fixed).
        let mount_source_matched_workspace = std::cell::Cell::new(false);
        let sentinel_read_through_mount = std::cell::Cell::new(false);

        let outcome = self.run_retained_workload_inner(
            authority,
            base_spec,
            workspace_manager,
            |workload_spec, cfg, container_id, lease, session, bind_state| {
                // Step 22: acquire the REAL workload launch permit against the ROTATED spec — the
                // hardware-INDEPENDENT control-plane fence (for V2: `authorize_workload_v2_retained` +
                // the queue→running launch CAS the permit commits). This is NOT faked: a broken workload
                // authorizer or wrong-purpose credential is refused HERE. The ONLY things this seam
                // fakes are the explicit-userns revalidation (`|| Ok(synth_root)`) and the runsc spawn —
                // both hardware, drilled by 5b.3-7.
                let permit = match workload_spec.acquire_launch_permit_for_test_support(hooks) {
                    Ok(permit) => permit,
                    Err(refusal) => return Err(refusal),
                };
                // Synthetic identities the substituted workload binds to. The bind's revalidation is
                // faked (`|| Ok(synth_root)`) — that plus the runsc spawn are the ONLY things skipped vs
                // `run_production_container_streaming` (its live explicit-userns revalidation is the
                // hardware boundary drilled by 5b.3-7; the permit fence above runs for real).
                let synth_root = (0x0111_u64, 0x0222_u64);
                let synth_cgroup = (0x0333_u64, 0x0444_u64);

                // Step 5: the SUBSTITUTED workload READS the sentinel THROUGH the retained OCI config's
                // recorded workspace mount source — proving Hop B and the workload shared one capsule.
                let mount_source = cfg.workspace_host_source_for_tests().map(Path::to_path_buf);
                mount_source_matched_workspace
                    .set(mount_source.as_deref() == Some(workspace_host.as_path()));
                sentinel_read_through_mount.set(match &mount_source {
                    Some(src) => std::fs::read(src.join(sentinel_name))
                        .map(|bytes| bytes == sentinel_bytes)
                        .unwrap_or(false),
                    None => false,
                });

                // Steps 23–24: the REAL `Prepared → Bound` durable workload bind, faking ONLY the
                // runsc-root revalidation. `bind_state` becomes `Bound` from the bind's OWN
                // `WorkloadBindingIdentity::into_parts()` output.
                if let Err(message) = bind_prepared_lease_given(
                    lease,
                    session,
                    bind_state,
                    synth_root,
                    container_id,
                    synth_cgroup,
                    || Ok(synth_root),
                ) {
                    return Ok(Err(RunFailure::uncommitted(message)));
                }
                // DERIVE the workload evidence from the bind's OWN recorded `Bound` output — never
                // from the synthetic input arguments. This is the honesty line: identity flows
                // real-bind → evidence; only the runsc execution is faked.
                let (bound_container, bound_root, bound_cgroup) = match &*bind_state {
                    LeaseBindState::Bound {
                        container_id,
                        runsc_root_identity,
                        cgroup_identity,
                    } => (container_id.clone(), *runsc_root_identity, *cgroup_identity),
                    other => {
                        return Ok(Err(RunFailure::uncommitted(format!(
                            "substituted workload bind did not reach Bound: {other:?}"
                        ))))
                    }
                };
                let evidence_root = match evidence_mode {
                    // The honest path: evidence's runsc-root IS the bind's recorded root.
                    SubstitutedEvidenceMode::DerivedFromBind => bound_root,
                    // The negative path: deliberately diverge from the recorded binding so the settle
                    // tail's provenance check MUST reject it (and only it — container/cgroup stay
                    // correct, isolating the runsc-root provenance comparison).
                    SubstitutedEvidenceMode::MismatchedRunscRoot => {
                        (bound_root.0 ^ 0xDEAD_BEEF, bound_root.1 ^ 0x0F0F)
                    }
                };
                let workload_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                    bound_container,
                    RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                        runsc_root_identity: evidence_root,
                    },
                    CgroupQuiescenceEvidence::assert_for_tests(bound_cgroup),
                );
                // Commit the launch permit exactly as the production `run` closure does at the spawn
                // boundary (mirroring `drive_compute_cycle_with_substituted_runsc`) — for V2 this is the
                // queue→running CAS. The spawn itself is the ONLY step faked past this point.
                if let Err(error) = permit.commit_and_release() {
                    return Ok(Err(RunFailure::uncommitted(error.to_string())));
                }
                Ok(Ok(finalized_for_test_support(workload_evidence)))
            },
        );

        // Return the REAL outcome (the caller maps it to settled_ok/settle_error, or — in the §4
        // driver — routes it through the continuation exactly as production does) plus the three
        // side-observations captured during the substituted execution.
        (
            outcome,
            used_at_workload_checkpoint,
            mount_source_matched_workspace.get(),
            sentinel_read_through_mount.get(),
        )
    }
}
