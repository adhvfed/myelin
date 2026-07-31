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

use std::path::PathBuf;

use super::{
    acquire_enabled_workspace, checkout_cleanup_plan, execute_cleanup_plan, join_diagnostics,
    resolve_checkout_preparation_permit, run_checkout_preparation_inner, unique_suffix,
    CheckoutPreparationError, CheckoutPreparationSpec, EnabledLaunchContext, LeaseBindState,
    OciConfig, PreparedCheckoutEvidence, RealCheckoutCleanupExecutor,
};
use crate::hardening::HardeningProfile;
use crate::user_namespace::{
    CheckoutPreparationSession, UserNamespaceAllocator, UserNamespaceBindError,
};
use crate::workspace_manager::WorkspaceManager;
use crate::{CheckoutAuthorizationScope, JobSpec, PhaseAuthorization, RunTokenCredential};

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
    ) -> Result<AcquiredCheckoutRuntime, String> {
        // Blocker 1: the capsule's scope is DERIVED from the same `spec` Hop B will run against — it is
        // not an independent argument that a caller could set to scope A while handing a scope-B spec.
        let checkout_scope =
            match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
                Ok(Some(scope)) => scope,
                Ok(None) => {
                    return Err(
                        "AcquiredCheckoutRuntime::acquire called for a non-checkout job — its \
                         workspace names neither repo_ref nor commit"
                            .to_string(),
                    )
                }
                Err(reason) => {
                    return Err(format!(
                        "deriving the checkout authorization scope from the job spec failed: {reason}"
                    ))
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
            let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
            return Err(join_diagnostics(
                format!(
                    "acquired workspace job_key {observed:?} does not equal the stable workload \
                     container id {expected:?} — the just-acquired workspace and lease were disposed"
                ),
                &diagnostics,
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
    /// The prepared-workload bind helper: durably transition the shared lease `Prepared → Bound` under
    /// the stable workload `container_id`, then record the ordinary [`LeaseBindState::Bound`] so the
    /// EXISTING settle path takes over. The `WorkloadBindingIdentity` the session returns is consumed
    /// IN PLACE via `into_parts` and never escapes this method.
    pub(crate) fn bind_workload(
        &mut self,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        let identity = self.acquired.session.bind_workload(
            &mut self.acquired.enabled_context.lease,
            self.acquired.workload_container_id.clone(),
            runsc_root_identity,
            cgroup_identity,
        )?;
        let (container_id, runsc_root_identity, cgroup_identity) = identity.into_parts();
        self.acquired.enabled_context.bind_state = LeaseBindState::Bound {
            container_id,
            runsc_root_identity,
            cgroup_identity,
        };
        Ok(())
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
#[allow(dead_code, clippy::result_large_err)]
pub(crate) fn run_checkout_preparation_v2(
    mut runtime: AcquiredCheckoutRuntime,
    spec: CheckoutPreparationSpec,
    run_token: RunTokenCredential,
    authorization: PhaseAuthorization,
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
    );
    match outcome {
        Ok(evidence) => Ok(PreparedCheckoutRuntime {
            acquired: runtime,
            prepared_checkout_evidence: evidence,
        }),
        Err(error) => Err((runtime, error)),
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
}
