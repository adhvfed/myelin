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
    resolve_checkout_preparation_permit, run_checkout_preparation_inner, settle_enabled_finalization,
    unique_suffix, AcquisitionFailure, BoundWorkloadRefusal, CheckoutPreparationError,
    CheckoutPreparationSpec, ContainerRun, EnabledLaunchContext, LeaseBindState, OciConfig,
    PreparedCheckoutEvidence, RealCheckoutCleanupExecutor, RetainedWorkloadOutcome, RunFailure,
    RuntimeFinalization, WorkloadRotatedSpec,
};
// `RuntimePreparation` + `LaunchPermit` are named only by the `#[cfg(test)]` injectable-executor seam
// (production reaches the workload runner solely through the sealed wrapper's fixed-runner method).
#[cfg(test)]
use super::RuntimePreparation;
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
                Err(reason) => {
                    return Err(AcquisitionFailure::clean(format!(
                        "deriving the checkout authorization scope from the job spec failed: {reason}"
                    )))
                }
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
        if let Err(error) = authority
            .complete_phase(PreparationPhase::CheckoutMaterialization, materialization_usage)
        {
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
