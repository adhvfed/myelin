//! Workspace + user-namespace integration for an enabled launch: acquiring the pair, binding the
//! lease to a live container, and settling (or unwinding) both together.

use super::*;
use crate::hardening::HardeningProfile;
use crate::user_namespace::{
    RunscInvocationMode, UserNamespaceAllocator,
    UserNamespaceBindError, UserNamespaceLease, UserNamespaceQuiescenceProof,
    UserNamespaceRefusal,
};
use crate::workspace_manager::{
    CapacityLease, CapacityRefusal, DeleteWorkspaceError, ManagedWorkspace, WorkspaceManager, WorkspaceProvisionError,
};
use crate::workspace_storage::WorkspaceStorageError;
use crate::JobSpec;
use std::path::PathBuf;

/// Backend-level workspace/user-namespace integration — a SINGLE enum, never two independent
/// `Option` fields: workspace-mount support REQUIRES explicit user-namespace support (the guest's
/// unprivileged uid is unmapped under plain `--rootless`), so the two are only ever meaningful
/// together. Do not add a way to enable one without the other — if explicit-userns-without-
/// workspace ever becomes a legitimate need, add a deliberate third variant then, rather than
/// letting an invalid combination be constructed and pushing validation to a later layer.
pub(super) enum WorkspaceIntegration {
    /// The production floor until slice 4: no workspace mount is ever offered, and no
    /// [`UserNamespaceAllocator`] is even constructed — avoids forcing every existing caller (most
    /// of which never asked for this) to satisfy its real `/etc/subuid`/`/etc/subgid` hardening
    /// requirements.
    Disabled,
    Enabled {
        workspace_manager: WorkspaceManager,
        userns_allocator: UserNamespaceAllocator,
    },
}

/// The Enabled workspace/explicit-userns path's local memory of how far a lease's durable
/// `Allocated -> Bound` transition has progressed (CT-007 slice 3, piece 7c) — `launch_with` needs
/// this to decide EXACTLY how to release/quarantine the lease after `run()` returns, since `bind`
/// happens deep inside the run closure (not `launch_with` itself), which only ever BORROWS the
/// lease, never returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LeaseBindState {
    /// `bind` was never attempted, or it refused for a caller-fixable reason
    /// (`InvalidContainerId`/`MarkerTooLarge`) that leaves the durable marker untouched — the lease
    /// is genuinely still `Allocated`, so `release_unused()` is safe.
    Allocated,
    /// `bind` succeeded — the durable marker durably records this EXACT identity triple.
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
    /// `bind` failed for a reason that already poisoned the allocator
    /// (`MarkerMismatch`/`Poisoned`) — the durable marker's phase is no longer trustworthy;
    /// neither `release_unused()` nor `release()` may be called on it. The lease's own `Drop`
    /// quarantines it, which is exactly the right outcome for an already-poisoned allocator.
    Unreleasable,
}

/// Everything the Enabled workspace/explicit-userns path owns for the duration of ONE launch
/// (CT-007 slice 3, piece 7c). `launch_with` owns this value for the whole call; only a mutable
/// borrow crosses into the run closure (via [`RuntimeBinding::Enabled`]), so `launch_with` retains
/// the workspace/lease across the closure call and can perform the exact cleanup this piece
/// specifies once `run()` returns — regardless of whether it returned an outer `Err` (a pre-bind
/// failure) or a [`RuntimeFinalization`] envelope.
pub(super) struct EnabledLaunchContext {
    pub(super) workspace: ManagedWorkspace,
    pub(super) lease: UserNamespaceLease,
    pub(super) bind_state: LeaseBindState,
}

/// Which runtime-binding capability a run closure was given for ONE launch (CT-007 slice 3, piece
/// 7c) — `Rootless` carries nothing to bind; `Enabled` carries the mutable lease/bind-state
/// capability plus the identity the lease's OCI mapping was validated against at construction
/// time (re-revalidated again, live, immediately before the actual `bind` call — this value is
/// what that later check compares against, not a substitute for it).
pub(super) enum RuntimeBinding<'a> {
    Rootless,
    Enabled {
        expected_root_identity: (u64, u64),
        context: &'a mut EnabledLaunchContext,
    },
    /// CT-007 slice 5b.3-6c: the checkout WORKLOAD binding, over a lease already durably `Prepared`
    /// by Hop B. Carries only disjoint, scoped mutable borrows of the capsule's own lease, session,
    /// and bind-state (never the whole `EnabledLaunchContext`, never the `OciConfig`) — the borrows
    /// are constructed INSIDE [`checkout_runtime::PreparedCheckoutRuntime::run_retained_workload`] and
    /// never outlive that closed transition. At the real cgroup boundary the bind is
    /// `session.bind_workload` (durable `Prepared → Bound`), NOT `lease.bind` (which is `Allocated →
    /// Bound` and would refuse a `Prepared` lease). Compute never constructs this arm, so the ordinary
    /// compute path is byte-unchanged.
    EnabledPrepared {
        expected_root_identity: (u64, u64),
        lease: &'a mut crate::user_namespace::UserNamespaceLease,
        session: &'a mut crate::user_namespace::CheckoutPreparationSession,
        bind_state: &'a mut LeaseBindState,
    },
}

/// The single validated runtime-preparation handle threaded into the run closure (CT-007 slice 3,
/// piece 7c). Constructing one calls [`require_oci_layout_matches_prepared_mode`], so downstream
/// code (`run_and_capture`, `SpawnedRunsc`, `retire_container`, `finalize_runtime`) can never
/// independently reconstruct — and potentially disagree on — the mode.
pub(super) struct RuntimePreparation<'a> {
    pub(super) prepared_mode: PreparedRuntimeMode,
    pub(super) mode: RunscInvocationMode,
    pub(super) binding: RuntimeBinding<'a>,
}

impl<'a> RuntimePreparation<'a> {
    pub(super) fn new(cfg: &OciConfig, binding: RuntimeBinding<'a>) -> Result<Self, String> {
        let prepared_mode = match &binding {
            RuntimeBinding::Rootless => PreparedRuntimeMode::Rootless,
            RuntimeBinding::Enabled {
                expected_root_identity,
                context,
            } => PreparedRuntimeMode::ExplicitUserNamespace {
                config: context.lease.config(),
                expected_root_identity: *expected_root_identity,
            },
            // CT-007 slice 5b.3-6c: the checkout workload's OCI layout is the SAME
            // ExplicitUserNamespaceWithWorkspace mode the capsule's retained `workload_cfg` was built
            // in — the config comes from the same lease, so the mode/layout check matches exactly.
            RuntimeBinding::EnabledPrepared {
                expected_root_identity,
                lease,
                ..
            } => PreparedRuntimeMode::ExplicitUserNamespace {
                config: lease.config(),
                expected_root_identity: *expected_root_identity,
            },
        };
        let mode = require_oci_layout_matches_prepared_mode(cfg, &prepared_mode)?;
        Ok(RuntimePreparation {
            prepared_mode,
            mode,
            binding,
        })
    }
}

/// Whether a [`DeleteWorkspaceError`] outcome PROVES the disk-backed workspace is genuinely gone
/// (CT-007 slice 3, piece 7c — Sol's cleanup subtlety #1: reissuing the leased subordinate uid
/// while the chowned workspace survives would violate isolation even though nothing ever
/// executed). Pure decision logic, pulled out of [`delete_workspace_then_release_lease_if_absent`]
/// (Sol's round-1 review: "generic-operation seams for the decision logic") so the FULL
/// classification matrix — which delete outcomes prove absence, which don't, and what diagnostic
/// each produces — is unit-testable with synthetic [`DeleteWorkspaceError`] values, without any
/// real `WorkspaceManager`/Btrfs/`CAP_SYS_ADMIN` privilege at all.
#[derive(Debug)]
pub(super) enum WorkspaceDeletionOutcome {
    /// Disk absence is proven — safe to release the paired lease. `diagnostic` is `Some` only for
    /// the `InternalInvariantViolated` case (disk absence still proven, but the corruption must
    /// still be surfaced).
    ProvenAbsent { diagnostic: Option<String> },
    /// Disk absence is NOT proven — the paired lease must NOT be released.
    NotProvenAbsent { diagnostic: String },
}

pub(super) fn classify_workspace_deletion(
    result: Result<(), DeleteWorkspaceError>,
) -> WorkspaceDeletionOutcome {
    match result {
        Ok(()) => WorkspaceDeletionOutcome::ProvenAbsent { diagnostic: None },
        Err(DeleteWorkspaceError::InternalInvariantViolated { reason }) => {
            WorkspaceDeletionOutcome::ProvenAbsent {
                diagnostic: Some(format!(
                    "workspace delete succeeded despite an internal invariant violation: {reason}"
                )),
            }
        }
        Err(DeleteWorkspaceError::Storage(e)) => WorkspaceDeletionOutcome::NotProvenAbsent {
            diagnostic: format!(
                "workspace delete/sync failed ({e}) — the userns lease is left unreleased \
                 (quarantined) since disk absence is not proven"
            ),
        },
        Err(DeleteWorkspaceError::WrongManager { .. }) => {
            WorkspaceDeletionOutcome::NotProvenAbsent {
                diagnostic:
                    "workspace delete refused (WrongManager — structurally unexpected) — the \
                         userns lease is left unreleased (quarantined) since disk absence is not \
                         proven"
                        .to_string(),
            }
        }
    }
}

/// Delete `workspace` (via the injected `delete_workspace` operation — Sol's round-1 review:
/// "generic-operation seams for the decision logic") and release `lease` if (and only if)
/// [`classify_workspace_deletion`] proves the disk-backed workspace is genuinely gone. Used for
/// every pre-bind failure that has a real workspace+lease pair to clean up. Returns every
/// diagnostic accumulated along the way — empty only if both steps succeeded cleanly.
fn delete_workspace_then_release_lease_if_absent(
    workspace: ManagedWorkspace,
    lease: UserNamespaceLease,
    delete_workspace: impl FnOnce(ManagedWorkspace) -> Result<(), DeleteWorkspaceError>,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    match classify_workspace_deletion(delete_workspace(workspace)) {
        WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
            diagnostics.extend(diagnostic);
            if let Err(e) = lease.release_unused() {
                diagnostics.push(format!("releasing the unused userns lease failed: {e}"));
            }
        }
        WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => {
            diagnostics.push(diagnostic);
            drop(lease);
        }
    }
    diagnostics
}

/// Join accumulated diagnostics onto a base message, `" AND "`-separated — the same
/// compound-message shape [`dispose_run_failure`]/[`augment_run_failure_with_teardown`] already
/// use elsewhere in this file.
pub(super) fn join_diagnostics(base: String, diagnostics: &[String]) -> String {
    diagnostics
        .iter()
        .fold(base, |acc, d| format!("{acc} AND {d}"))
}

/// CT-007 slice 5b.3-6c (Sol's r2 finding 1): a typed workspace/lease acquisition failure that carries
/// whether the acquisition's OWN rollback was PROVEN clean. `reconciliation_required` is `true` iff a
/// workspace could not be proven deleted, a lease could not be proven released, or a rollback failed —
/// i.e. the manager may be poisoned and/or the userns slot quarantined, so the caller MUST route to
/// `ReconciliationRequired` (never ordinary retry/exhaustion, which would strand a live resource with
/// no typed signal). A clean refusal (capacity/lease refused, or a provisioning failure whose rollback
/// was proven) is ordinary-retryable.
#[derive(Debug)]
pub(crate) struct AcquisitionFailure {
    pub message: String,
    pub reconciliation_required: bool,
}

impl AcquisitionFailure {
    pub(super) fn clean(message: String) -> Self {
        Self {
            message,
            reconciliation_required: false,
        }
    }
    fn reconcile(message: String) -> Self {
        Self {
            message,
            reconciliation_required: true,
        }
    }
    /// A post-provisioning rollback (`delete_workspace_then_release_lease_if_absent`): EMPTY diagnostics
    /// = the delete + release were proven clean → ordinary refusal; ANY diagnostic = a quarantine /
    /// unproven-absence → reconciliation-required.
    pub(super) fn from_rollback_diagnostics(base: String, diagnostics: Vec<String>) -> Self {
        let reconciliation_required = !diagnostics.is_empty();
        Self {
            message: join_diagnostics(base, &diagnostics),
            reconciliation_required,
        }
    }
}

/// CT-007 slice 3, piece 7c: acquire capacity, a userns lease, and a real disk-backed workspace,
/// then build the `ExplicitUserNamespaceWithWorkspace` OCI layout from them — the Enabled path's
/// counterpart to `OciConfig::from_spec`'s plain Rootless construction. Every failure hands back a
/// single accumulated diagnostic message; nothing is silently dropped without at least an attempt
/// to release/quarantine it correctly (see the matrix in this function's own review history).
pub(super) fn acquire_enabled_workspace(
    spec: &JobSpec,
    profile: &HardeningProfile,
    container_id: &str,
    absolute_rootfs: PathBuf,
    workspace_manager: &WorkspaceManager,
    userns_allocator: &UserNamespaceAllocator,
    cargo_vendor: Option<crate::asset_registry::VerifiedCargoVendor>,
) -> Result<(OciConfig, EnabledLaunchContext), AcquisitionFailure> {
    let (cfg, context) = acquire_enabled_workspace_given(
        spec,
        profile,
        container_id,
        absolute_rootfs,
        |bytes| workspace_manager.acquire_capacity(bytes),
        || userns_allocator.lease(),
        |job_key, quota, uid, gid, capacity| {
            workspace_manager.create_workspace(job_key, quota, uid, gid, capacity)
        },
        |workspace| workspace_manager.delete_workspace(workspace),
    )?;
    let Some(cargo_vendor) = cargo_vendor else {
        return Ok((cfg, context));
    };
    match cfg.with_cargo_vendor(cargo_vendor) {
        Ok(cfg) => Ok((cfg, context)),
        Err(reason) => {
            let diagnostics = cleanup_pre_bind_failure(context, workspace_manager);
            Err(AcquisitionFailure::from_rollback_diagnostics(
                format!("attaching the structured Cargo vendor boundary failed: {reason}"),
                diagnostics,
            ))
        }
    }
}

/// The actual decision logic behind [`acquire_enabled_workspace`], taking the four external
/// operations (capacity acquisition, userns leasing, workspace creation, and the workspace
/// deletion used ONLY by this function's own post-creation-failure cleanup) as injectable closures
/// (Sol's round-1 review: "generic-operation seams for the decision logic") — mirrors this file's
/// established `_given` pattern. A closure still needs to return a REAL `CapacityLease`/
/// `UserNamespaceLease`/`ManagedWorkspace` for its OWN success case (there is no fake-object
/// constructor for any of them, deliberately), but a FAILURE case is freely constructible synthetic
/// data — so this seam lets a test exercise e.g. "the userns lease is refused" or "workspace
/// creation hits an `UnrecoverableLeak`" deterministically, without needing `CAP_SYS_ADMIN`/real
/// Btrfs quota privilege for the specific operation under test. `delete_workspace` is `FnOnce`
/// despite two syntactic call sites below — they are mutually exclusive match arms, so at most one
/// ever actually runs.
#[allow(clippy::too_many_arguments)]
fn acquire_enabled_workspace_given(
    spec: &JobSpec,
    profile: &HardeningProfile,
    container_id: &str,
    absolute_rootfs: PathBuf,
    acquire_capacity: impl FnOnce(u64) -> Result<CapacityLease, CapacityRefusal>,
    lease_fn: impl FnOnce() -> Result<UserNamespaceLease, UserNamespaceRefusal>,
    create_workspace: impl FnOnce(
        &str,
        u64,
        u32,
        u32,
        CapacityLease,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError>,
    delete_workspace: impl FnOnce(ManagedWorkspace) -> Result<(), DeleteWorkspaceError>,
) -> Result<(OciConfig, EnabledLaunchContext), AcquisitionFailure> {
    let capacity = acquire_capacity(spec.limits.disk_bytes).map_err(|refusal| {
        AcquisitionFailure::clean(format!("workspace capacity refused: {refusal}"))
    })?;
    let lease = match lease_fn() {
        Ok(lease) => lease,
        Err(refusal) => {
            capacity.release();
            return Err(AcquisitionFailure::clean(format!(
                "userns lease refused: {refusal}"
            )));
        }
    };
    let workspace = match create_workspace(
        container_id,
        spec.limits.disk_bytes,
        lease.host_uid(),
        lease.host_gid(),
        capacity,
    ) {
        Ok(workspace) => workspace,
        Err(WorkspaceProvisionError::Refused(refusal)) => {
            let message = format!("workspace creation refused: {refusal}");
            refusal.into_capacity().release();
            // Release proven clean → ordinary refusal; a FAILED release means the slot may still be
            // live → reconciliation-required (Sol's finding 1).
            return Err(match lease.release_unused() {
                Ok(()) => AcquisitionFailure::clean(message),
                Err(e) => AcquisitionFailure::reconcile(format!(
                    "{message} AND releasing the unused userns lease also failed: {e}"
                )),
            });
        }
        Err(WorkspaceProvisionError::Storage(WorkspaceStorageError::UnrecoverableLeak {
            path,
            ..
        })) => {
            // Capacity was already abandoned internally (poisoning the workspace manager); the
            // subvolume may still exist on disk, so the lease must NOT be released — drop it
            // (quarantine) rather than risk reissuing this subordinate uid over live data. The
            // rollback is UNPROVEN → reconciliation-required.
            drop(lease);
            return Err(AcquisitionFailure::reconcile(format!(
                "workspace creation left an unrecoverable leak at {path:?} — the userns lease is \
                 left unreleased (quarantined) since disk absence is not proven"
            )));
        }
        Err(WorkspaceProvisionError::Storage(e)) => {
            // A real attempt failed, but `WorkspaceManager`'s own rollback already proved no
            // subvolume survives — capacity was already released internally.
            let message = format!("workspace-storage provisioning failed: {e}");
            return Err(match lease.release_unused() {
                Ok(()) => AcquisitionFailure::clean(message),
                Err(release_error) => AcquisitionFailure::reconcile(format!(
                    "{message} AND releasing the unused userns lease also failed: {release_error}"
                )),
            });
        }
        Err(WorkspaceProvisionError::InternalInvariantViolated { reason, workspace }) => {
            let diagnostics =
                delete_workspace_then_release_lease_if_absent(*workspace, lease, delete_workspace);
            // Non-empty diagnostics = the delete/release could not be PROVEN → reconciliation-required.
            return Err(AcquisitionFailure::from_rollback_diagnostics(
                format!("workspace creation violated an internal invariant: {reason}"),
                diagnostics,
            ));
        }
    };
    let cfg = match OciConfig::from_spec(spec, profile).with_explicit_user_namespace_and_workspace(
        lease.config(),
        OciWorkspaceMount::from_managed_workspace(&workspace),
        absolute_rootfs,
    ) {
        Ok(cfg) => cfg,
        Err(reason) => {
            let diagnostics =
                delete_workspace_then_release_lease_if_absent(workspace, lease, delete_workspace);
            return Err(AcquisitionFailure::from_rollback_diagnostics(
                format!("building the explicit-userns workspace OCI layout failed: {reason}"),
                diagnostics,
            ));
        }
    };
    Ok((
        cfg,
        EnabledLaunchContext {
            workspace,
            lease,
            bind_state: LeaseBindState::Allocated,
        },
    ))
}

/// CT-007 slice 3, piece 7c: clean up a real workspace+lease after a PRE-BIND failure (the run
/// closure returned an outer `Err` before `run_and_capture` was ever called — the ONE path where
/// `lease.bind()` may or may not have even been attempted). Consults the locally-recorded
/// [`LeaseBindState`] to decide the correct disposition: `Allocated` deletes the workspace then
/// releases the lease if disk absence is proven; `Unreleasable` never releases the lease (bind
/// already poisoned/quarantined it) but still deletes the workspace if disk absence is proven.
/// `Bound` should be structurally unreachable here (a successful bind means the runner always
/// returns the `RuntimeFinalization` envelope, never an outer `Err`) — if it is ever reached anyway,
/// BOTH the workspace and the lease are abandoned (never deleted, never released), since no
/// finalization evidence exists proving the runtime cannot still access them.
pub(super) fn cleanup_pre_bind_failure(
    context: EnabledLaunchContext,
    workspace_manager: &WorkspaceManager,
) -> Vec<String> {
    let EnabledLaunchContext {
        workspace,
        lease,
        bind_state,
    } = context;
    match bind_state {
        LeaseBindState::Allocated => {
            delete_workspace_then_release_lease_if_absent(workspace, lease, |w| {
                workspace_manager.delete_workspace(w)
            })
        }
        LeaseBindState::Unreleasable => {
            // `bind()` itself already set `released = true` and poisoned the allocator for
            // `MarkerMismatch`/`Poisoned` — dropping `lease` here is a genuine no-op (no
            // duplicate incident; `Drop` sees `released == true` and does nothing). There is no
            // `Allocated` marker left to `release_unused()` against, so never attempt to release
            // it — but the workspace is still safe to delete (deletion never touches the lease's
            // own durable marker) if disk absence can be proven.
            drop(lease);
            match classify_workspace_deletion(workspace_manager.delete_workspace(workspace)) {
                WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
                    diagnostic.into_iter().collect()
                }
                WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => vec![diagnostic],
            }
        }
        LeaseBindState::Bound { .. } => {
            // STRUCTURALLY IMPOSSIBLE in correct code (Sol's round-1 review, blocker 2): a
            // successful bind means `run_production_container_streaming` always returns the
            // `RuntimeFinalization` envelope from that point on, never an outer `Err`. If this is
            // ever reached anyway (a future regression), there is NO finalization evidence proving
            // the runtime cannot still access the workspace — deleting the workspace or releasing/
            // reissuing the subordinate uid would both be unsafe. Abandon BOTH: drop them, letting
            // their own `Drop` poison the workspace manager and quarantine the lease, and ALWAYS
            // surface this as an invariant violation, regardless of anything else.
            drop(workspace);
            drop(lease);
            vec![
                "an outer launch failure occurred AFTER a successful userns lease bind — this \
                 should be structurally impossible; the workspace and lease are both abandoned \
                 (quarantined) rather than acted on, since no finalization evidence exists \
                 proving the runtime cannot still access them"
                    .to_string(),
            ]
        }
    }
}

/// CT-007 slice 3, piece 7c: the Enabled path's post-`Finalized` cleanup — validate the evidence
/// against the LOCALLY recorded binding (never trust evidence alone; it must agree with what THIS
/// process durably bound), mint a real [`UserNamespaceQuiescenceProof`], delete+sync the
/// workspace, and only once disk absence is proven, release the bound lease. Returns `Ok(())`
/// only if every step succeeded; any failure's message is a single diagnostic the caller applies
/// to the ALREADY-SETTLED result via [`augment_settled_result_with_enabled_cleanup_failure`] —
/// deliberately NOT folded into a [`RuntimeTeardownError`] (Sol's round-1 review, blocker 3: that
/// would misleadingly claim "runtime teardown failed" for a failure in a different safety domain).
fn settle_enabled_workspace_and_lease(
    context: EnabledLaunchContext,
    workspace_manager: &WorkspaceManager,
    evidence: &RuntimeQuiescenceEvidence,
) -> Result<(), String> {
    let EnabledLaunchContext {
        workspace,
        lease,
        bind_state,
    } = context;
    let LeaseBindState::Bound {
        container_id,
        runsc_root_identity,
        cgroup_identity,
    } = bind_state
    else {
        return Err(format!(
            "the runtime finalized, but this lease's locally-recorded bind state was \
             {bind_state:?} (not Bound) — refusing to trust evidence against an unrecorded binding"
        ));
    };
    let expected_namespace = RuntimeNamespaceQuiescence::ExplicitUserNamespace {
        runsc_root_identity,
    };
    if evidence.container_id() != container_id || evidence.namespace() != expected_namespace {
        return Err(format!(
            "runtime quiescence evidence ({:?}, {:?}) does not match the recorded binding \
             ({container_id:?}, {expected_namespace:?})",
            evidence.container_id(),
            evidence.namespace()
        ));
    }
    if evidence.cgroup().cgroup_identity() != cgroup_identity {
        return Err(format!(
            "runtime quiescence evidence's cgroup identity {:?} does not match the recorded \
             bind-time cgroup identity {cgroup_identity:?}",
            evidence.cgroup().cgroup_identity()
        ));
    }
    let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, evidence)
        .map_err(|e| format!("failed to mint a userns quiescence proof: {e}"))?;
    match classify_workspace_deletion(workspace_manager.delete_workspace(workspace)) {
        WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
            if let Err(e) = lease.release(proof) {
                let base = diagnostic.unwrap_or_else(|| "workspace deleted".to_string());
                return Err(format!(
                    "{base}, but releasing the userns lease also failed: {e}"
                ));
            }
            match diagnostic {
                Some(diagnostic) => Err(diagnostic),
                None => Ok(()),
            }
        }
        WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => {
            drop(lease);
            Err(diagnostic)
        }
    }
}

/// CT-007 slice 5b.3-6c: the shared finalization→settle tail, extracted BYTE-FOR-BYTE out of the old
/// inline `launch_compute_with` `Ok(finalization)` body so the checkout workload can settle the
/// CAPSULE's own [`EnabledLaunchContext`] through the exact same audited path. Workspace/lease cleanup
/// is a SEPARATE safety domain from `finalize_runtime`'s own runtime-teardown checks: its outcome is
/// captured here and applied to the ALREADY-SETTLED `Result`, never folded back into a
/// `RuntimeFinalization`/`RuntimeTeardownError` (which would misleadingly claim "runtime teardown
/// failed" even when the runtime tore down perfectly cleanly). `enabled_context` is `Some` only for
/// the Enabled workspace path; `workspace_manager` MUST then be `Some` (the same-integration
/// invariant compute already relied on).
pub(super) fn settle_enabled_finalization(
    finalization: RuntimeFinalization<Result<ContainerRun, RunFailure>>,
    enabled_context: Option<EnabledLaunchContext>,
    workspace_manager: Option<&WorkspaceManager>,
) -> Result<ContainerRun, RunFailure> {
    let enabled_cleanup_failure = match (enabled_context, &finalization) {
        (Some(context), RuntimeFinalization::Finalized(finalized)) => {
            let workspace_manager = workspace_manager.expect(
                "an enabled context is only ever present alongside its workspace manager (Enabled)",
            );
            settle_enabled_workspace_and_lease(context, workspace_manager, &finalized.evidence)
                .err()
        }
        (Some(context), RuntimeFinalization::Failed { .. }) => {
            // Runtime finalization itself failed (no full quiescence evidence exists) — never delete
            // the workspace or release the lease; let both quarantine/poison through their own
            // existing `Drop` machinery.
            drop(context);
            None
        }
        (None, _) => None,
    };

    let settled = settle_finalization(
        finalization,
        |run: &ContainerRun| run.result.usage,
        discard_container_run_after_teardown_failure,
    );
    match enabled_cleanup_failure {
        None => settled,
        Some(diagnostic) => augment_settled_result_with_enabled_cleanup_failure(
            settled,
            |run: &ContainerRun| run.result.usage,
            // This run already finalized VALIDLY (namespace never drifted) -- discard it directly via
            // `discard_container_run`, never a fabricated empty `RuntimeTeardownError`.
            |run: ContainerRun| discard_container_run(run, false),
            diagnostic,
        ),
    }
}

/// Durably bind `context`'s lease to `container_id`/`cgroup_identity`, classifying any bind
/// failure into the correct [`LeaseBindState`] — extracted out of
/// `run_production_container_streaming` as a deterministic `_given` seam: `revalidate_root_identity`
/// stands in for the live `revalidated_explicit_userns_root_identity()` syscall, so this can be unit
/// tested against a real [`UserNamespaceLease`] (cheap, no `CAP_SYS_ADMIN` needed) without depending
/// on this host's actual runsc-root ownership. Never calls anything beyond `context.lease.bind` —
/// the caller alone decides whether a bind failure means `run_and_capture` must never be reached.
fn bind_enabled_lease_given(
    lease: &mut UserNamespaceLease,
    bind_state: &mut LeaseBindState,
    expected_root_identity: (u64, u64),
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
) -> Result<(), String> {
    let current = revalidate_root_identity()?;
    if current != expected_root_identity {
        return Err(format!(
            "runsc-root identity drifted before bind (expected {expected_root_identity:?}, \
             found {current:?})"
        ));
    }
    match lease.bind(container_id.to_string(), current, cgroup_identity) {
        Ok(()) => {
            *bind_state = LeaseBindState::Bound {
                container_id: container_id.to_string(),
                runsc_root_identity: current,
                cgroup_identity,
            };
            Ok(())
        }
        Err(bind_error) => {
            *bind_state = match bind_error {
                UserNamespaceBindError::InvalidContainerId
                | UserNamespaceBindError::MarkerTooLarge => LeaseBindState::Allocated,
                UserNamespaceBindError::MarkerMismatch | UserNamespaceBindError::Poisoned => {
                    LeaseBindState::Unreleasable
                }
            };
            Err(format!("durable lease bind failed: {bind_error}"))
        }
    }
}

/// CT-007 slice 5b.3-6c: durably bind the checkout WORKLOAD to `container_id`/`cgroup_identity`
/// through the capsule's session (`Prepared → Bound`), classifying any bind failure into the correct
/// [`LeaseBindState`] — the `EnabledPrepared` counterpart to [`bind_enabled_lease_given`]. The
/// `LeaseBindState::Bound` is constructed EXCLUSIVELY from the returned
/// [`WorkloadBindingIdentity::into_parts`](crate::user_namespace::WorkloadBindingIdentity::into_parts),
/// never by re-deriving the arguments, so the in-memory bind state can never diverge from what the
/// durable rewrite wrote. On a caller-fixable failure the session stays `Prepared` (so prepared
/// cleanup stays valid); on a poisoning failure the local bind state becomes `Unreleasable`.
pub(super) fn bind_prepared_lease_given(
    lease: &mut crate::user_namespace::UserNamespaceLease,
    session: &mut crate::user_namespace::CheckoutPreparationSession,
    bind_state: &mut LeaseBindState,
    expected_root_identity: (u64, u64),
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
) -> Result<(), String> {
    let current = revalidate_root_identity()?;
    if current != expected_root_identity {
        return Err(format!(
            "runsc-root identity drifted before workload bind (expected \
             {expected_root_identity:?}, found {current:?})"
        ));
    }
    match session.bind_workload(lease, container_id.to_string(), current, cgroup_identity) {
        Ok(identity) => {
            let (container_id, runsc_root_identity, cgroup_identity) = identity.into_parts();
            *bind_state = LeaseBindState::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            };
            Ok(())
        }
        Err(bind_error) => {
            *bind_state = match bind_error {
                crate::user_namespace::UserNamespaceBindError::InvalidContainerId
                | crate::user_namespace::UserNamespaceBindError::MarkerTooLarge => {
                    // Caller-fixable — the lease/session are provably still Prepared and untouched.
                    LeaseBindState::Allocated
                }
                crate::user_namespace::UserNamespaceBindError::MarkerMismatch
                | crate::user_namespace::UserNamespaceBindError::Poisoned => {
                    LeaseBindState::Unreleasable
                }
            };
            Err(format!("durable workload lease bind failed: {bind_error}"))
        }
    }
}

/// The exact composition boundary this piece's whole security property rests on: bind (if
/// `Enabled`; a no-op for `Rootless`) and invoke `continuation` ONLY if that succeeds — a bind
/// failure (or a live identity-drift refusal) must NEVER be followed by the capture/spawn
/// continuation. Generic over `T` so this can be unit tested with a bare counting closure instead
/// of `run_and_capture` — no real runsc spawn or privileged Btrfs involved (Sol's round-2 review).
pub(super) fn bind_then_continue<T>(
    enabled: Option<(&mut UserNamespaceLease, &mut LeaseBindState, (u64, u64))>,
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
    continuation: impl FnOnce() -> T,
) -> Result<T, String> {
    if let Some((lease, bind_state, expected_root_identity)) = enabled {
        bind_enabled_lease_given(
            lease,
            bind_state,
            expected_root_identity,
            container_id,
            cgroup_identity,
            revalidate_root_identity,
        )?;
    }
    Ok(continuation())
}
