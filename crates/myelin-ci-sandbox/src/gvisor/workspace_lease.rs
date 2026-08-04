//! Workspace + user-namespace integration for an enabled launch: acquiring the pair, binding the
//! lease to a live container, and settling (or unwinding) both together.

use super::*;
use crate::hardening::HardeningProfile;
use crate::user_namespace::{
    RunscInvocationMode, UserNamespaceAllocator, UserNamespaceBindError, UserNamespaceLease,
    UserNamespaceQuiescenceProof, UserNamespaceRefusal,
};
use crate::workspace_manager::{
    CapacityLease, CapacityRefusal, DeleteWorkspaceError, ManagedWorkspace, WorkspaceManager,
    WorkspaceProvisionError,
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
pub(super) fn settle_enabled_workspace_and_lease(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::gvisor::test_fixtures::*;
    #[cfg(feature = "test-support")]
    use crate::workspace_manager::WorkspaceStorageMode;
    #[cfg(feature = "test-support")]
    use std::sync::Arc;

    use crate::workspace_manager::DeleteWorkspaceError;

    use crate::workspace_storage::WorkspaceStorageError;

    // ───── CT-007 slice 3, piece 7c: pure decision-logic tests (no real objects at all) ─────

    #[test]
    fn classify_workspace_deletion_ok_proves_absence_with_no_diagnostic() {
        let outcome = classify_workspace_deletion(Ok(()));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::ProvenAbsent { diagnostic: None }
        ));
    }

    #[test]
    fn classify_workspace_deletion_internal_invariant_violated_proves_absence_but_surfaces_it() {
        let outcome =
            classify_workspace_deletion(Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "bookkeeping corruption".to_string(),
            }));
        match outcome {
            WorkspaceDeletionOutcome::ProvenAbsent {
                diagnostic: Some(diagnostic),
            } => assert!(diagnostic.contains("bookkeeping corruption")),
            other => {
                panic!("expected ProvenAbsent with a diagnostic, got a different shape: {other:?}")
            }
        }
    }

    #[test]
    fn classify_workspace_deletion_storage_failure_does_not_prove_absence() {
        let outcome = classify_workspace_deletion(Err(DeleteWorkspaceError::Storage(
            WorkspaceStorageError::ZeroQuota,
        )));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::NotProvenAbsent { .. }
        ));
    }

    /// Integrated (not just pure-`classify_workspace_deletion`-level) coverage: a REAL lease flows
    /// all the way through `delete_workspace_then_release_lease_if_absent` (the exact helper
    /// `cleanup_pre_bind_failure`'s `Allocated` arm calls), with only the `delete_workspace`
    /// operation injected as synthetic. `InternalInvariantViolated` means deletion actually
    /// succeeded (capacity was released, subvolume gone) despite a bookkeeping bug -- disk absence
    /// IS proven, so the real lease must still be releasable via `release_unused()`, and the
    /// failure must still be surfaced as a diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_releases_a_real_lease_on_internal_invariant_violated(
    ) {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-invariant-violated")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-invariant-violated")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-invariant-violated-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();

        // Sol's round-2 review: a synthetic error alone would make the "disk absence proven"
        // premise false (the real subvolume would still be sitting there) -- genuinely call the
        // REAL `delete_workspace` first (so the subvolume really is gone and real capacity really
        // is released), THEN report the synthetic `InternalInvariantViolated` as if some OTHER
        // bookkeeping check inside a real `delete_workspace` call had separately failed atop an
        // otherwise-successful deletion -- exactly the scenario this variant models.
        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |w| {
            workspace_manager.delete_workspace(w).expect(
                "the real delete must succeed for this test to model a genuine invariant \
                 violation atop an otherwise-successful deletion",
            );
            Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "synthetic bookkeeping corruption for this test".to_string(),
            })
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("synthetic bookkeeping corruption"));
        assert!(
            !host_path.exists(),
            "the real subvolume must genuinely be gone -- this variant's whole premise is that \
             disk absence IS proven, just alongside a separately-surfaced bookkeeping failure"
        );
        assert!(
            userns_allocator.is_healthy(),
            "InternalInvariantViolated proves disk absence -- the real lease must have released \
             cleanly, not been quarantined"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The `Storage`/sync-failure counterpart: disk absence is NOT proven, so the real lease must
    /// be left unreleased (dropped -- quarantined by `Drop`, never `release_unused()`), and the
    /// failure must still be surfaced as a diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_quarantines_a_real_lease_on_a_storage_failure()
    {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-storage-failure-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();

        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |_w| {
            Err(DeleteWorkspaceError::Storage(
                WorkspaceStorageError::ZeroQuota,
            ))
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("delete/sync failed"));
        assert!(
            !userns_allocator.is_healthy(),
            "a Storage failure does NOT prove disk absence -- the real lease must be quarantined, \
             never released"
        );

        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    // ───────── CT-007 slice 3, piece 7c: `bind_enabled_lease_given` classification matrix ─────────
    //
    // `bind_enabled_lease_given` was extracted specifically so this matrix is coverable with a real
    // (cheap, non-privileged) `UserNamespaceLease` and a bare `LeaseBindState` value — no
    // `ManagedWorkspace`/`CAP_SYS_ADMIN` involved at all, unlike the acquire/settle tests above.

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_binds_and_records_bound_on_success() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-ok") else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "bind-ok-container",
            cgroup_identity,
            || Ok(expected_root_identity),
        );
        assert!(result.is_ok());
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity,
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Never calls `lease.bind` at all when the live identity revalidation disagrees with what was
    /// expected — this is exactly the check that lets the caller (`run_production_container_streaming`)
    /// refuse BEFORE ever calling `run_and_capture`, without having mutated the lease or its durable
    /// marker in any way.
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_refuses_before_touching_the_lease_when_identity_drifted() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-identity-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            (11, 22),
            "bind-drift-container",
            (33, 44),
            || Ok((99, 99)), // the live revalidation disagrees with the expected identity.
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "an identity-drift refusal must never touch bind_state -- the lease was never bound"
        );
        // The lease itself was never mutated -- it can still be released as a plain unused lease.
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_an_invalid_container_id_as_still_allocated() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-invalid-id")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "", // empty container_id -> UserNamespaceBindError::InvalidContainerId
            (33, 44),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "InvalidContainerId is a caller bug, not a global-trust failure -- nothing touched \
             disk, so the lease remains safely Allocated and reusable"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_a_marker_mismatch_as_unreleasable() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-marker-mismatch")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        // Bind it once for real, durably transitioning the on-disk marker to Bound -- a SECOND
        // bind attempt against the same lease will then find the marker no longer `Allocated`,
        // which is exactly the `MarkerMismatch` path (poisoning the allocator).
        lease
            .bind(
                "already-bound".to_string(),
                expected_root_identity,
                (33, 44),
            )
            .expect("the first bind against a fresh Allocated lease must succeed");
        let mut bind_state = LeaseBindState::Bound {
            container_id: "already-bound".to_string(),
            runsc_root_identity: expected_root_identity,
            cgroup_identity: (33, 44),
        };
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "second-bind-attempt",
            (55, 66),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Unreleasable,
            "MarkerMismatch means the on-disk state no longer agrees with this in-memory lease -- \
             ambiguous and never safe to release"
        );
        assert!(
            !allocator.is_healthy(),
            "MarkerMismatch must globally poison the allocator (a global-trust failure, not a \
             caller bug)"
        );
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    // ───── CT-007 slice 3, piece 7c: `bind_then_continue` — the bind-then-capture composition ─────
    //
    // Sol's round-2 review: `bind_enabled_lease_given` proves the classification table is correct,
    // but leaves the crucial decision -- never invoking the capture/spawn continuation after a
    // failed/unconfirmed bind -- to its caller. These tests exercise that COMPOSITION directly with
    // a bare counting closure standing in for `run_and_capture` -- no real runsc spawn, no
    // privileged Btrfs, just a real (cheap) `UserNamespaceLease` where `Enabled` coverage needs one.

    #[test]
    fn bind_then_continue_always_invokes_the_continuation_when_rootless() {
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            None,
            "rootless-container",
            (33, 44),
            || panic!("Rootless must never need to revalidate a root identity"),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(calls.get(), 1);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_invokes_the_continuation_exactly_once_after_a_successful_bind() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-ok")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, expected_root_identity)),
            "bind-then-continue-ok-container",
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(
            calls.get(),
            1,
            "a successful bind must invoke the continuation exactly once"
        );
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-then-continue-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity: (33, 44),
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-then-continue-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((33, 44)),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// The security property this whole piece rests on: a live identity-drift refusal must leave
    /// the continuation-call count at zero -- no exec may ever follow a failed/unconfirmed bind.
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_when_identity_drifted() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, (11, 22))),
            "bind-then-continue-drift-container",
            (33, 44),
            || Ok((99, 99)), // disagrees with the expected (11, 22).
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "an identity-drift refusal must NEVER invoke the capture/spawn continuation"
        );
        assert_eq!(bind_state, LeaseBindState::Allocated);
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Same property, for a real durable bind failure (not merely a live-identity refusal).
    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_on_a_real_bind_failure() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-bind-fail")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((
                &mut lease,
                &mut LeaseBindState::Allocated,
                expected_root_identity,
            )),
            "", // empty container_id -> UserNamespaceBindError::InvalidContainerId
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "a real bind failure must NEVER invoke the capture/spawn continuation"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug bind refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_then_settle_releases_cleanly_on_a_matching_evidence() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("acquire-settle-ok")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("acquire-settle-ok")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "acquire-settle-ok-container";
        let (cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed against a healthy real manager/allocator");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(context.lease.config())
        );
        assert_eq!(context.bind_state, LeaseBindState::Allocated);

        // Simulate what `run_production_container_streaming` does: bind, THEN finalize.
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed for a fresh Allocated lease");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence)
            .expect("settling a matching evidence against a Bound lease must succeed");
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn settle_enabled_workspace_and_lease_refuses_evidence_disagreeing_with_the_recorded_binding() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("settle-mismatch")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("settle-mismatch")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "settle-mismatch-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().to_path_buf();
        // Evidence claims a DIFFERENT cgroup identity than what was actually bound.
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((99, 99)),
        );
        let result = settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence);
        assert!(
            result.is_err(),
            "evidence disagreeing with the recorded binding must refuse, not silently release"
        );
        // Neither the workspace nor the lease were touched by `settle_enabled_workspace_and_lease`
        // -- refusing dropped both, which poisons `workspace_manager` (real subvolume abandoned,
        // exactly like `dropping_a_managed_workspace_without_deleting_poisons_the_manager_with_one_incident`
        // in workspace_manager.rs) and quarantines the userns slot. The real subvolume is still on
        // disk here; `remove_dir_all` CANNOT remove a Btrfs subvolume (it needs a privileged
        // `btrfs subvolume delete`). Sol's review: exercise the ACTUAL claimed crash-recovery path
        // instead of leaking it — drop the poisoned manager (releasing its lock), open a FRESH
        // manager on the same base, and let ITS OWN boot-time reconciliation delete the orphan for
        // real before `remove_dir_all` is safe to call on the (now subvolume-free) base directory.
        assert!(
            host_path.exists(),
            "the abandoned subvolume must still be real and on disk"
        );
        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(fresh.is_healthy());
        assert!(
            !host_path.exists(),
            "boot reconciliation must have deleted the abandoned subvolume for real"
        );
        drop(fresh);
        // Quarantined userns markers are NEVER deleted by design (boot reconciliation only ever
        // quarantines a surviving marker, never removes it) -- `leases_dir` holds only plain JSON
        // marker files, not a Btrfs primitive, so `remove_dir_all` here is genuinely safe.
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's round-1 review, blocker 2: a `Bound` outer error is structurally impossible in correct
    /// code (a successful bind means the runner always returns the `RuntimeFinalization` envelope
    /// from that point on), but `cleanup_pre_bind_failure` must still handle it conservatively if a
    /// future regression ever reaches it -- abandoning BOTH resources rather than acting on either,
    /// and ALWAYS surfacing a non-empty invariant-violation diagnostic.
    #[cfg(feature = "test-support")]
    #[test]
    fn cleanup_pre_bind_failure_abandons_both_resources_when_bind_state_is_bound() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("bound-abandons-both")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("bound-abandons-both")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "bound-abandons-both-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().to_path_buf();

        // The "structurally impossible" outer failure: some future regression calls the pre-bind
        // cleanup path even though bind already durably succeeded.
        let diagnostics = cleanup_pre_bind_failure(context, &workspace_manager);

        assert_eq!(
            diagnostics.len(),
            1,
            "a Bound outer error must always surface exactly one invariant-violation diagnostic, \
             never an empty vec: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("structurally impossible"));
        assert!(
            host_path.exists(),
            "the workspace must be ABANDONED, not deleted, when bind_state was Bound"
        );
        assert!(
            !workspace_manager.is_healthy(),
            "abandoning the workspace without deleting it must poison the manager"
        );
        assert!(
            !userns_allocator.is_healthy(),
            "abandoning a Bound lease without releasing it must poison the allocator too"
        );

        // Clean up for real: drop the poisoned manager, let a fresh one's boot reconciliation
        // delete the orphaned subvolume (never `remove_dir_all` on a real Btrfs subvolume).
        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_refuses_when_capacity_is_exhausted_and_touches_nothing_else() {
        // Capacity is exhausted BEFORE `acquire_enabled_workspace` is ever called, so
        // `create_workspace`'s `btrfs qgroup limit` step is never reached — no `CAP_SYS_ADMIN`
        // needed, so this test runs (rather than skips) even without that privilege.
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("capacity-exhausted")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("capacity-exhausted")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        // Exhaust the 1 GiB ceiling with an unrelated hold, so `acquire_capacity` refuses cleanly
        // BEFORE ever touching the userns allocator.
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("the fresh manager's own full ceiling must be leasable once");
        let mut command_spec = spec(vec![]);
        command_spec.limits.disk_bytes = 1; // any nonzero request now exceeds the exhausted ceiling
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace(
            &command_spec,
            &profile,
            "capacity-exhausted-container",
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        );
        assert!(
            result.is_err(),
            "an exhausted ceiling must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "acquire_enabled_workspace must never have leased (and left quarantined) a userns \
             slot when capacity refused first: {:?}",
            userns_allocator.quarantined_slots()
        );
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    /// Sol's required-tests list: "userns refusal releases capacity." Uses the injectable
    /// `_given` seam with a REAL capacity lease (from a real, lightweight manager — no
    /// `CAP_SYS_ADMIN` needed for `acquire_capacity` itself) and a SYNTHETIC userns refusal (no
    /// real allocator needed at all), proving the capacity lease is released back to the pool
    /// rather than left dangling.
    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_capacity_when_userns_lease_is_refused() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("userns-refused")
        else {
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-userns-refused",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || Err(UserNamespaceRefusal::PoolExhausted { pool_size: 0 }),
            |_, _, _, _, _| {
                panic!("create_workspace must never run when the lease is refused first")
            },
            |_| panic!("delete_workspace must never run on this path"),
        );
        assert!(
            result.is_err(),
            "a refused userns lease must refuse acquisition"
        );
        // If capacity was genuinely released, the full ceiling is leasable again.
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("capacity must have been released back to the pool after the userns refusal");
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
    }

    /// Sol's required-tests list: "recoverable provisioning failure releases unused lease" and
    /// "`UnrecoverableLeak` quarantines it." Both use a REAL capacity lease + REAL userns lease
    /// (from a real, lightweight allocator — no privileged operation needed for `lease()` itself)
    /// and a SYNTHETIC `create_workspace` failure, so neither needs `CAP_SYS_ADMIN`.
    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_the_lease_on_a_recoverable_provisioning_failure() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("recoverable-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("recoverable-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-recoverable-failure",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, capacity: CapacityLease| {
                // Mirrors the REAL `WorkspaceManager::create_workspace`'s own contract for a
                // non-`UnrecoverableLeak` `Storage` error: "capacity was already released
                // internally" — a synthetic closure standing in for it must honor the same
                // contract, or this test would (as it initially did) observe an incident from
                // `CapacityLease::drop` that has nothing to do with what's under test.
                capacity.release();
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::ZeroQuota,
                ))
            },
            |_| panic!("delete_workspace must never run — no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "a recoverable provisioning failure must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "a recoverable failure must release_unused() the lease, not quarantine it: {:?}",
            userns_allocator.quarantined_slots()
        );
        assert!(
            workspace_manager.is_healthy(),
            "a recoverable failure must leave the workspace manager healthy (capacity released \
             cleanly, not abandoned)"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_quarantines_the_lease_on_an_unrecoverable_leak() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("unrecoverable-leak")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("unrecoverable-leak")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-unrecoverable-leak",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, _capacity| {
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::UnrecoverableLeak {
                        path: PathBuf::from("/fake/leaked/path"),
                        subvol_id: None,
                        provisioning_error: "synthetic provisioning error".to_string(),
                        cleanup_error: "synthetic cleanup error".to_string(),
                    },
                ))
            },
            |_| panic!("delete_workspace must never run — no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "an unrecoverable leak must refuse acquisition"
        );
        assert_eq!(
            userns_allocator.quarantined_slots().len(),
            1,
            "an UnrecoverableLeak must quarantine (never release_unused()) the lease: {:?}",
            userns_allocator.quarantined_slots()
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }
}
