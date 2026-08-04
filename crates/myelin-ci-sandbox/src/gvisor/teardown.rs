//! Checked runtime teardown: `runsc delete -force`, cgroup + namespace quiescence evidence, and the
//! rules for merging a teardown failure into whatever result the run produced.

use super::*;
use crate::launch_gate::DirectChildRetirement;
use crate::user_namespace::RunscInvocationMode;
use crate::ResourceUsage;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// How long [`finalize_runtime`]'s call to [`MemoryCgroup::quiesce`] polls `cgroup.events` before
/// giving up. By the time `finalize_runtime` runs, `run_and_capture` has already confirmed (or
/// force-killed) the whole container — this only bounds the wait for the kernel to finish
/// unwinding an already-terminated cgroup, not the workload's own execution.
pub(super) const RUNTIME_QUIESCE_TIMEOUT: Duration = Duration::from_secs(2);

/// What [`RuntimeQuiescenceEvidence`] vouches for regarding the runsc state-root namespace: either
/// there was none to check (`Rootless`), or the pinned state root's identity was re-confirmed
/// unchanged immediately before minting this evidence. `pub(crate)` (CT-007 slice 3, piece 7c) so
/// [`crate::user_namespace::UserNamespaceQuiescenceProof::from_runtime_evidence`] can match on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeNamespaceQuiescence {
    Rootless,
    ExplicitUserNamespace { runsc_root_identity: (u64, u64) },
}

/// Non-forgeable (outside tests) proof that ONE runtime instance — its direct child process, its
/// exact `container_id`, its runsc-state-root namespace (if any), and its [`MemoryCgroup`] — was
/// independently checked torn down, produced ONLY by a successful [`finalize_runtime`]. `pub(crate)`
/// (CT-007 slice 3, piece 7c) so [`crate::user_namespace::UserNamespaceQuiescenceProof::from_runtime_evidence`]
/// can mint a real proof from it — fields stay private; only the accessors below are exposed, and
/// `finalize_runtime` remains the sole production minting path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeQuiescenceEvidence {
    container_id: String,
    namespace: RuntimeNamespaceQuiescence,
    cgroup: CgroupQuiescenceEvidence,
}

impl RuntimeQuiescenceEvidence {
    pub(crate) fn container_id(&self) -> &str {
        &self.container_id
    }

    pub(crate) fn namespace(&self) -> RuntimeNamespaceQuiescence {
        self.namespace
    }

    pub(crate) fn cgroup(&self) -> CgroupQuiescenceEvidence {
        self.cgroup
    }

    // CT-007 slice 5b.3-6e.1: also available to the `test-support` runsc-driver seam.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn assert_for_tests(
        container_id: String,
        namespace: RuntimeNamespaceQuiescence,
        cgroup: CgroupQuiescenceEvidence,
    ) -> Self {
        RuntimeQuiescenceEvidence {
            container_id,
            namespace,
            cgroup,
        }
    }
}

/// One independent thing [`finalize_runtime`] found wrong. Deliberately NOT a single first-error
/// enum (Sol's review): the direct-child reap, the namespace-identity revalidation, the checked
/// container delete, and the cgroup quiescence are four INDEPENDENT checks, and more than one can
/// genuinely fail on the same run — collapsing them into "whichever failed first" would silently
/// discard the others.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeTeardownIssue {
    /// The direct `runsc` child's `wait()` did not confirm a reap (its fate is unknown).
    ChildNotConfirmedReaped(String),
    /// The runsc state-root identity no longer matches what the run was validated against —
    /// `ExplicitUserNamespace` mode only; SKIPS the checked container delete below it (acting on a
    /// path-based policy that may no longer name the trusted state root is not safe).
    NamespaceIdentityDrifted(String),
    /// `runsc delete -force <container_id>` did not confirm the container is gone.
    ContainerNotConfirmedDeleted(String),
    /// [`MemoryCgroup::quiesce`] failed to independently verify + remove the cgroup.
    Cgroup(CgroupQuiescenceError),
}

impl std::fmt::Display for RuntimeTeardownIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeTeardownIssue::ChildNotConfirmedReaped(e) => {
                write!(f, "the direct child was not confirmed reaped: {e}")
            }
            RuntimeTeardownIssue::NamespaceIdentityDrifted(e) => {
                write!(f, "the runsc state-root identity drifted: {e}")
            }
            RuntimeTeardownIssue::ContainerNotConfirmedDeleted(e) => {
                write!(f, "the container was not confirmed deleted: {e}")
            }
            RuntimeTeardownIssue::Cgroup(e) => write!(f, "cgroup quiescence failed: {e}"),
        }
    }
}

/// Every independent [`RuntimeTeardownIssue`] [`finalize_runtime`] found — always non-empty (a
/// caller never constructs this with zero issues; see [`finalize_runtime`]'s own contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeTeardownError {
    issues: Vec<RuntimeTeardownIssue>,
}

impl std::fmt::Display for RuntimeTeardownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let joined = self
            .issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        write!(f, "{joined}")
    }
}

/// Checked container retirement — confirms the exit status of `runsc delete -force`, rather than
/// silently discarding it like the pre-existing best-effort [`delete_container`] (which remains,
/// unchanged, as the DEFERRED/idempotent teardown [`SpawnedRunsc::kill`] uses — a legitimately
/// separate, later, best-effort safety net, not the authoritative check this function is).
fn retire_container(
    bin: &Path,
    container_id: &str,
    mode: RunscInvocationMode,
) -> Result<(), String> {
    let mut cmd = Command::new(bin);
    apply_runsc_invocation_policy(&mut cmd, bin, mode)
        .map_err(|e| format!("apply runsc invocation policy for delete: {e}"))?;
    let output = cmd
        .arg("delete")
        .arg("-force")
        .arg(container_id)
        .output()
        .map_err(|e| format!("run `runsc delete -force {container_id}`: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`runsc delete -force {container_id}` exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// **The authoritative, checked teardown for ONE runtime instance (CT-007 slice 3, piece 7b).**
/// Consumes `cgroup` (ownership was hoisted out of [`run_and_capture`] into the caller precisely so
/// this function can own the final `quiesce()`). Never returns early on a non-cgroup failure —
/// EVERY check below always runs, and every issue found is collected, because a caller must never
/// assume "the first thing that failed" is the only thing that failed:
///
/// 1. The direct child's [`DirectChildRetirement`] (already known from `run_and_capture`/
///    `SandboxCommand::spawn`'s own real `wait()` outcome — nothing to re-check here).
/// 2. For `ExplicitUserNamespace` mode: re-confirm the runsc state-root identity via
///    [`revalidated_explicit_userns_root_identity`]. A drift here means the path-based policy this
///    process would otherwise `runsc delete` through may no longer name the trusted state root —
///    so step 3 is SKIPPED, but step 4 (cgroup quiescence, identified independently by its own
///    `(dev, ino)`, never by this path) still always runs.
/// 3. Checked `runsc delete -force` ([`retire_container`]) — skipped per (2) above.
/// 4. `cgroup.quiesce()` — ALWAYS attempted, regardless of whether (1)-(3) found anything wrong.
///
/// Evidence is minted ONLY when every check that ran found nothing wrong.
pub(super) fn finalize_runtime(
    bin: &Path,
    container_id: &str,
    prepared_mode: &PreparedRuntimeMode,
    cgroup: MemoryCgroup,
    quiesce_timeout: Duration,
    child_retirement: DirectChildRetirement,
) -> Result<RuntimeQuiescenceEvidence, RuntimeTeardownError> {
    finalize_runtime_given(
        bin,
        container_id,
        prepared_mode,
        cgroup,
        quiesce_timeout,
        child_retirement,
        revalidated_explicit_userns_root_identity,
        retire_container,
    )
}

/// The actual decision logic behind [`finalize_runtime`], taking the namespace-identity
/// revalidation AND the checked container retirement as injectable closures rather than calling
/// the process-global [`revalidated_explicit_userns_root_identity`]/[`retire_container`] directly
/// — mirrors this file's own established `_given` pattern (see
/// [`apply_runsc_invocation_policy_given`]): once ANY test in the same test binary's process
/// installs the real global `EXPLICIT_USERNS_POLICY`, it stays installed for every other test
/// sharing that process, making the drift-detection AND identity-match branches impossible to
/// test deterministically without this seam (Sol's round-2 review: the identity-match branch
/// specifically needs `retire_container` injectable too, since it runs immediately after a
/// matching identity and would otherwise hit the same global-policy dependency).
#[allow(clippy::too_many_arguments)]
fn finalize_runtime_given(
    bin: &Path,
    container_id: &str,
    prepared_mode: &PreparedRuntimeMode,
    cgroup: MemoryCgroup,
    quiesce_timeout: Duration,
    child_retirement: DirectChildRetirement,
    revalidate_namespace_identity: impl FnOnce() -> Result<(u64, u64), String>,
    retire_container_fn: impl FnOnce(&Path, &str, RunscInvocationMode) -> Result<(), String>,
) -> Result<RuntimeQuiescenceEvidence, RuntimeTeardownError> {
    let mut issues = Vec::new();

    if let DirectChildRetirement::Unconfirmed(reason) = child_retirement {
        issues.push(RuntimeTeardownIssue::ChildNotConfirmedReaped(reason));
    }

    let namespace = match prepared_mode {
        PreparedRuntimeMode::Rootless => Some(RuntimeNamespaceQuiescence::Rootless),
        PreparedRuntimeMode::ExplicitUserNamespace {
            expected_root_identity,
            ..
        } => match revalidate_namespace_identity() {
            Ok(current) if current == *expected_root_identity => {
                Some(RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: current,
                })
            }
            Ok(current) => {
                issues.push(RuntimeTeardownIssue::NamespaceIdentityDrifted(format!(
                    "expected {expected_root_identity:?}, found {current:?}"
                )));
                None
            }
            Err(reason) => {
                issues.push(RuntimeTeardownIssue::NamespaceIdentityDrifted(reason));
                None
            }
        },
    };

    // Only attempt the checked delete when the namespace identity is trusted (or not applicable) —
    // acting on a path-based policy that may no longer name the trusted state root is not safe.
    if namespace.is_some() {
        if let Err(reason) = retire_container_fn(bin, container_id, prepared_mode.invocation_mode())
        {
            issues.push(RuntimeTeardownIssue::ContainerNotConfirmedDeleted(reason));
        }
    }

    // ALWAYS attempt cgroup quiescence, regardless of anything found above — the cgroup is
    // identified by its own (device, inode), independent of the container/namespace checks.
    let cgroup_evidence = match cgroup.quiesce(quiesce_timeout) {
        Ok(evidence) => Some(evidence),
        Err(e) => {
            issues.push(RuntimeTeardownIssue::Cgroup(e));
            None
        }
    };

    match (namespace, cgroup_evidence, issues.is_empty()) {
        (Some(namespace), Some(cgroup), true) => Ok(RuntimeQuiescenceEvidence {
            container_id: container_id.to_string(),
            namespace,
            cgroup,
        }),
        _ => Err(RuntimeTeardownError { issues }),
    }
}

/// A run's primary disposition (`T`, typically `Result<ContainerRun, RunFailure>`-shaped) PLUS
/// independently-verified proof its runtime instance was fully torn down — [`finalize_runtime`]'s
/// only production constructor.
#[derive(Debug)]
pub(super) struct FinalizedRun<T> {
    pub(super) primary: T,
    pub(super) evidence: RuntimeQuiescenceEvidence,
}

/// The TOTAL result of finalizing a runtime instance (CT-007 slice 3, piece 7b, Sol's design
/// review): a caller must never be able to lose the primary run disposition OR the teardown
/// result — a bare `Result<FinalizedRun<T>, RuntimeTeardownError>` would let a `?` silently
/// discard `T` the moment teardown failed, even though the primary run may have succeeded (or
/// failed for a completely different, already-informative reason).
#[derive(Debug)]
pub(super) enum RuntimeFinalization<T> {
    Finalized(FinalizedRun<T>),
    Failed {
        primary: T,
        teardown: RuntimeTeardownError,
    },
}

/// Run [`finalize_runtime`] and merge its outcome with `primary`, never losing either.
pub(super) fn finalize_and_merge<T>(
    primary: T,
    bin: &Path,
    container_id: &str,
    prepared_mode: &PreparedRuntimeMode,
    cgroup: MemoryCgroup,
    quiesce_timeout: Duration,
    child_retirement: DirectChildRetirement,
) -> RuntimeFinalization<T> {
    match finalize_runtime(
        bin,
        container_id,
        prepared_mode,
        cgroup,
        quiesce_timeout,
        child_retirement,
    ) {
        Ok(evidence) => RuntimeFinalization::Finalized(FinalizedRun { primary, evidence }),
        Err(teardown) => RuntimeFinalization::Failed { primary, teardown },
    }
}

/// Append `extra` to an existing [`RunFailure`]'s message, preserving its exact phase/usage —
/// mirrors [`GvisorBackend::dispose_run_failure`]'s established "augment, never replace"
/// compound-message pattern, generalized (CT-007 slice 3, piece 7c, Sol's round-1 review, blocker
/// 3) so every later augmentation (runtime teardown, workspace/lease cleanup, ...) shares ONE
/// mechanism instead of each hand-rolling the same four-variant match.
pub(super) fn augment_run_failure_message(failure: RunFailure, extra: impl std::fmt::Display) -> RunFailure {
    match failure {
        RunFailure::Uncommitted { message } => RunFailure::Uncommitted {
            message: format!("{message} AND {extra}"),
        },
        RunFailure::CommitOutcomeUnknown { message } => RunFailure::CommitOutcomeUnknown {
            message: format!("{message} AND {extra}"),
        },
        RunFailure::CommittedButNotExecuted { message } => RunFailure::CommittedButNotExecuted {
            message: format!("{message} AND {extra}"),
        },
        RunFailure::Executed { message, usage } => RunFailure::Executed {
            message: format!("{message} AND {extra}"),
            usage,
        },
    }
}

/// Append a teardown failure to an existing [`RunFailure`], preserving its exact phase/usage and
/// its original message — mirrors [`GvisorBackend::dispose_run_failure`]'s established "augment,
/// never replace" compound-message pattern, extended here to runtime-teardown failures.
pub(super) fn augment_run_failure_with_teardown(
    failure: RunFailure,
    teardown: &RuntimeTeardownError,
) -> RunFailure {
    augment_run_failure_message(failure, format!("runtime teardown failed ({teardown})"))
}

/// CT-007 slice 3, piece 7c (Sol's round-1 review, blocker 3): a workspace-deletion or
/// userns-lease-release failure discovered AFTER `finalize_runtime` itself already succeeded is a
/// SEPARATE safety domain from runtime teardown (container delete, cgroup quiescence) — folding it
/// into [`RuntimeTeardownError`] would produce a misleading "runtime teardown failed" message even
/// when the runtime tore down perfectly cleanly (or was never created at all, for a pre-permit
/// failure). This shares the SAME augment-never-replace mechanics via
/// [`augment_run_failure_message`], under its own distinct message, applied to an
/// ALREADY-SETTLED `Result<S, RunFailure>` (i.e. after [`settle_finalization`] has already run, for
/// the post-`Finalized` case — never folded back into a [`RuntimeFinalization`]).
pub(super) fn augment_settled_result_with_enabled_cleanup_failure<S>(
    settled: Result<S, RunFailure>,
    usage_of: impl FnOnce(&S) -> ResourceUsage,
    on_discarded_success: impl FnOnce(S),
    diagnostic: String,
) -> Result<S, RunFailure> {
    match settled {
        Ok(success) => {
            let usage = usage_of(&success);
            on_discarded_success(success);
            Err(RunFailure::executed(
                format!(
                    "workspace/userns-lease cleanup failed after a successful run: {diagnostic}"
                ),
                usage,
            ))
        }
        Err(failure) => Err(augment_run_failure_message(
            failure,
            format!("workspace/userns-lease cleanup also failed: {diagnostic}"),
        )),
    }
}

/// Settle a [`RuntimeFinalization`] of a `Result<S, RunFailure>`-shaped primary into the actual
/// `Result<S, RunFailure>` a run-closure returns (CT-007 slice 3, piece 7b, Sol's review point 4):
/// a teardown failure is NEVER incident-only — it must change what the caller (and, downstream,
/// the job's billing/retry disposition) sees. `usage_of` extracts the measured [`ResourceUsage`]
/// from a successful `S`, needed to convert a clean success into a `RunFailure::Executed` when
/// teardown fails after it (the run itself still consumed real resources — settling it as if
/// nothing happened would keep treating the job as successful while the new safety invariant this
/// piece adds — "teardown is independently verified" — was actually false).
///
/// `on_discarded_success` (Sol's round-2 review, blocker 3): a successful `S` this function is
/// about to convert into a failure and DROP would otherwise leak whatever resources `S` owned —
/// for a [`ContainerRun`], its staged `bundle_dir` (which would never be entered into `self.live`
/// for later teardown, since this whole call is returning `Err`). Called with the discarded `S`
/// and the `RuntimeTeardownError` that caused the discard, so the caller can make an informed
/// decision (e.g. skip a path-based cleanup step the teardown itself found unsafe to trust).
pub(super) fn settle_finalization<S>(
    finalization: RuntimeFinalization<Result<S, RunFailure>>,
    usage_of: impl FnOnce(&S) -> ResourceUsage,
    on_discarded_success: impl FnOnce(S, &RuntimeTeardownError),
) -> Result<S, RunFailure> {
    match finalization {
        RuntimeFinalization::Finalized(FinalizedRun { primary, .. }) => primary,
        RuntimeFinalization::Failed {
            primary: Ok(success),
            teardown,
        } => {
            let usage = usage_of(&success);
            on_discarded_success(success, &teardown);
            Err(RunFailure::executed(
                format!("runtime teardown failed after a successful run ({teardown})"),
                usage,
            ))
        }
        RuntimeFinalization::Failed {
            primary: Err(failure),
            teardown,
        } => Err(augment_run_failure_with_teardown(failure, &teardown)),
    }
}

/// Clean up a [`ContainerRun`] that a teardown failure is forcing [`settle_finalization`] to
/// discard instead of returning to its caller (Sol's round-2 review, blocker 3): BEST-EFFORT
/// removes the staged bundle dir (a leaked temp dir, not a leaked live container/cgroup — this
/// function does not surface a removal failure, matching how bundle-dir cleanup is treated
/// EVERYWHERE ELSE in this file's ordinary error paths), and best-effort re-attempts the deferred
/// [`SpawnedRunsc::kill`] as one more defense-in-depth try — UNLESS the teardown issues include a
/// namespace-identity drift, in which case the finalizer already determined the path-based `runsc
/// delete`/`kill` this would perform is not safe to trust (acting on it here would be exactly the
/// mistake `finalize_runtime` itself refused to make).
/// Discard an owned [`ContainerRun`] that will never be returned to a caller as a success — best-
/// effort kill (unless `skip_path_kill`, e.g. a confirmed namespace-identity drift where the child
/// may not be safely killable via the ordinary path) and remove the staged bundle. Sol's round-2
/// review: factored out of `discard_container_run_after_teardown_failure` so the Enabled workspace/
/// lease cleanup path (a run that already finalized VALIDLY, but must still be discarded because a
/// cleanup step failed afterward) can discard a `ContainerRun` without fabricating an invalid empty
/// `RuntimeTeardownError` just to reuse that function's signature.
pub(super) fn discard_container_run(mut run: ContainerRun, skip_path_kill: bool) {
    if !skip_path_kill {
        let _ = run.child.kill();
    }
    let _ = std::fs::remove_dir_all(&run.bundle_dir);
}

pub(super) fn discard_container_run_after_teardown_failure(
    run: ContainerRun,
    teardown: &RuntimeTeardownError,
) {
    let namespace_drifted = teardown
        .issues
        .iter()
        .any(|issue| matches!(issue, RuntimeTeardownIssue::NamespaceIdentityDrifted(_)));
    discard_container_run(run, namespace_drifted);
}

/// Read the `runsc` process's cumulative CPU time (utime+stime) from `/proc/<pid>/stat`, in whole
/// seconds (USER_HZ = 100 on Linux). Mirrors the Firecracker backend's measurement (a small,
/// backend-specific `/proc` read of the spawned runtime's pid). `None` if `/proc` is unavailable or
/// unparseable (then the caller falls back to a wall-clock figure — a real run never under-meters to 0).
pub(super) fn read_proc_cpu_seconds(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field (field 2) is parenthesised and may contain spaces/`)`; skip past the LAST ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm: state(3) ppid(4) ... utime(14) stime(15) ... ⇒ rest indices 11/12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100)
}
