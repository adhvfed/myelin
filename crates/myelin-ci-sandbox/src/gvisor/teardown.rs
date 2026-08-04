use super::*;
use crate::launch_gate::DirectChildRetirement;
use crate::user_namespace::RunscInvocationMode;
use crate::ResourceUsage;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub(super) const RUNTIME_QUIESCE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeNamespaceQuiescence {
    Rootless,
    ExplicitUserNamespace { runsc_root_identity: (u64, u64) },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeQuiescenceEvidence {
    pub(super) container_id: String,
    pub(super) namespace: RuntimeNamespaceQuiescence,
    pub(super) cgroup: CgroupQuiescenceEvidence,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RuntimeTeardownIssue {
    ChildNotConfirmedReaped(String),
    NamespaceIdentityDrifted(String),
    ContainerNotConfirmedDeleted(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeTeardownError {
    pub(super) issues: Vec<RuntimeTeardownIssue>,
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

    if namespace.is_some() {
        if let Err(reason) = retire_container_fn(bin, container_id, prepared_mode.invocation_mode())
        {
            issues.push(RuntimeTeardownIssue::ContainerNotConfirmedDeleted(reason));
        }
    }

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

#[derive(Debug)]
pub(super) struct FinalizedRun<T> {
    pub(super) primary: T,
    pub(super) evidence: RuntimeQuiescenceEvidence,
}

#[derive(Debug)]
pub(super) enum RuntimeFinalization<T> {
    Finalized(FinalizedRun<T>),
    Failed {
        primary: T,
        teardown: RuntimeTeardownError,
    },
}

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

pub(super) fn augment_run_failure_message(
    failure: RunFailure,
    extra: impl std::fmt::Display,
) -> RunFailure {
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

pub(super) fn augment_run_failure_with_teardown(
    failure: RunFailure,
    teardown: &RuntimeTeardownError,
) -> RunFailure {
    augment_run_failure_message(failure, format!("runtime teardown failed ({teardown})"))
}

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

pub(super) fn read_proc_cpu_seconds(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::user_namespace::UserNamespaceConfig;

    use crate::ResourceUsage;

    use std::sync::Arc;

    use crate::gvisor::test_fixtures::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn augment_settled_result_with_enabled_cleanup_failure_converts_a_clean_success_to_executed() {
        let usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 4096,
        };
        let discarded = std::cell::Cell::new(false);
        let result: Result<u64, RunFailure> = Ok(42);
        let result = augment_settled_result_with_enabled_cleanup_failure(
            result,
            move |_: &u64| usage,
            |value: u64| {
                assert_eq!(value, 42);
                discarded.set(true);
            },
            "workspace delete/sync failed".to_string(),
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, usage);
                assert!(message.contains("workspace/userns-lease cleanup failed"));
                assert!(message.contains("workspace delete/sync failed"));
            }
            other => panic!("expected RunFailure::Executed, got {other:?}"),
        }
    }

    #[test]
    fn augment_settled_result_with_enabled_cleanup_failure_augments_an_existing_failure() {
        let original_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 8192,
        };
        let result: Result<u64, RunFailure> =
            Err(RunFailure::executed("original failure", original_usage));
        let result = augment_settled_result_with_enabled_cleanup_failure(
            result,
            |_: &u64| panic!("usage_of must not be called when the primary already failed"),
            |_: u64| panic!("on_discarded_success must not run when the primary already failed"),
            "lease release failed".to_string(),
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, original_usage);
                assert!(message.contains("original failure"));
                assert!(message.contains("workspace/userns-lease cleanup also failed"));
                assert!(message.contains("lease release failed"));
            }
            other => panic!("expected an augmented RunFailure::Executed, got {other:?}"),
        }
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn finalize_runtime_mints_evidence_on_a_clean_rootless_teardown() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let cgroup_identity = cg.identity();
        let evidence = finalize_runtime(
            Path::new("/bin/true"),
            "container-does-not-matter-for-bin-true",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
        )
        .expect(
            "a confirmed-reaped child, a successful delete, and a clean cgroup must mint evidence",
        );
        assert_eq!(evidence.namespace, RuntimeNamespaceQuiescence::Rootless);
        assert_eq!(evidence.cgroup.cgroup_identity(), cgroup_identity);
        assert!(
            !dir.exists(),
            "finalize_runtime must remove the cgroup on success"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn finalize_runtime_refuses_when_the_direct_child_was_not_confirmed_reaped() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let result = finalize_runtime(
            Path::new("/bin/true"),
            "container-does-not-matter-for-bin-true",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Unconfirmed("wait() returned ECHILD".to_string()),
        );
        let error = result.expect_err("an unconfirmed direct-child reap must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::ChildNotConfirmedReaped(_)
        ));
        assert!(
            !dir.exists(),
            "quiesce must still run (and succeed) even though evidence was refused"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn finalize_runtime_refuses_when_the_container_delete_is_not_confirmed() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let result = finalize_runtime(
            Path::new("/bin/false"),
            "container-does-not-matter-for-bin-false",
            &PreparedRuntimeMode::Rootless,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
        );
        let error = result.expect_err("a non-zero delete exit must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::ContainerNotConfirmedDeleted(_)
        ));
        assert!(
            !dir.exists(),
            "cgroup quiescence must still run (and succeed) despite the delete failure"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn finalize_runtime_skips_the_delete_but_still_quiesces_when_namespace_identity_drifts() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir().to_path_buf();
        let expected_root_identity = (11, 22);
        let drifted_identity = (11, 99);
        let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
            config: UserNamespaceConfig::for_tests(1000, 1000, 200000, 200000),
            expected_root_identity,
        };
        let result = finalize_runtime_given(
            Path::new("/bin/false"),
            "container-must-not-be-deleted",
            &prepared_mode,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
            move || Ok(drifted_identity),
            |_bin, _container_id, _mode| {
                panic!("retire_container must never be invoked after a namespace-identity drift")
            },
        );
        let error = result.expect_err("a drifted namespace identity must refuse evidence");
        assert_eq!(error.issues.len(), 1, "{error:?}");
        assert!(matches!(
            error.issues[0],
            RuntimeTeardownIssue::NamespaceIdentityDrifted(_)
        ));
        assert!(
            !dir.exists(),
            "cgroup quiescence must still run (and succeed) even though the delete was skipped"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn finalize_runtime_mints_explicit_userns_evidence_when_the_identity_still_matches() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let cgroup_identity = cg.identity();
        let identity = (33, 44);
        let userns_config = UserNamespaceConfig::for_tests(1000, 1000, 200000, 200000);
        let prepared_mode = PreparedRuntimeMode::ExplicitUserNamespace {
            config: userns_config,
            expected_root_identity: identity,
        };
        let delete_calls = std::cell::Cell::new(0u32);
        let seen_mode = std::cell::RefCell::new(None);
        let evidence = finalize_runtime_given(
            Path::new("/bin/true"),
            "container-xyz",
            &prepared_mode,
            cg,
            Duration::from_secs(2),
            DirectChildRetirement::Reaped,
            move || Ok(identity),
            |_bin, container_id, mode| {
                delete_calls.set(delete_calls.get() + 1);
                *seen_mode.borrow_mut() = Some(mode);
                assert_eq!(container_id, "container-xyz");
                Ok(())
            },
        )
        .expect("a matching identity, successful delete, and clean cgroup must mint evidence");

        assert_eq!(delete_calls.get(), 1, "delete must be invoked exactly once");
        assert_eq!(
            seen_mode.into_inner(),
            Some(RunscInvocationMode::ExplicitUserNamespace(userns_config)),
            "the derived explicit invocation mode must be used"
        );
        assert_eq!(evidence.container_id, "container-xyz");
        assert_eq!(
            evidence.namespace,
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: identity
            }
        );
        assert_eq!(evidence.cgroup.cgroup_identity(), cgroup_identity);
    }

    #[test]
    fn discard_container_run_after_teardown_failure_removes_bundle_and_best_effort_kills_on_a_non_drift_issue(
    ) {
        let killed = Arc::new(AtomicBool::new(false));
        let (run, bundle_dir) = container_run_with_real_bundle_dir(killed.clone());
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                "exited 1".to_string(),
            )],
        };
        discard_container_run_after_teardown_failure(run, &teardown);
        assert!(
            !bundle_dir.exists(),
            "the staged bundle must be removed even when the run is discarded"
        );
        assert!(
            killed.load(Ordering::SeqCst),
            "a non-drift issue must still attempt the best-effort deferred kill"
        );
    }

    #[test]
    fn discard_container_run_after_teardown_failure_skips_kill_but_still_removes_bundle_on_namespace_drift(
    ) {
        let killed = Arc::new(AtomicBool::new(false));
        let (run, bundle_dir) = container_run_with_real_bundle_dir(killed.clone());
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::NamespaceIdentityDrifted(
                "expected (1, 2), found (1, 3)".to_string(),
            )],
        };
        discard_container_run_after_teardown_failure(run, &teardown);
        assert!(
            !bundle_dir.exists(),
            "the staged bundle must still be removed even when kill is skipped"
        );
        assert!(
            !killed.load(Ordering::SeqCst),
            "a namespace-identity drift must NOT trigger the path-based deferred kill"
        );
    }

    #[test]
    fn settle_finalization_returns_the_primary_unchanged_when_finalized() {
        let evidence = RuntimeQuiescenceEvidence {
            container_id: "c".to_string(),
            namespace: RuntimeNamespaceQuiescence::Rootless,
            cgroup: CgroupQuiescenceEvidence::assert_for_tests((1, 2)),
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Finalized(FinalizedRun {
                primary: Ok(42),
                evidence,
            });
        let result = settle_finalization(
            finalization,
            |_: &u64| ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
            |_: u64, _: &RuntimeTeardownError| {
                panic!("on_discarded_success must not run when finalization succeeded")
            },
        );
        assert!(matches!(result, Ok(42)), "{result:?}");
    }

    #[test]
    fn settle_finalization_converts_a_clean_success_into_executed_when_teardown_fails() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::ContainerNotConfirmedDeleted(
                "exited 1".to_string(),
            )],
        };
        let usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 4096,
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Failed {
                primary: Ok(42),
                teardown,
            };
        let discarded = std::cell::Cell::new(false);
        let result = settle_finalization(
            finalization,
            move |_: &u64| usage,
            |value: u64, _: &RuntimeTeardownError| {
                assert_eq!(value, 42);
                discarded.set(true);
            },
        );
        assert!(
            discarded.get(),
            "on_discarded_success must run for a discarded successful primary"
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, usage);
                assert!(message.contains("runtime teardown failed"));
            }
            other => panic!("expected RunFailure::Executed, got {other:?}"),
        }
    }

    #[test]
    fn settle_finalization_augments_an_existing_run_failure_without_losing_its_phase_or_usage() {
        let teardown = RuntimeTeardownError {
            issues: vec![RuntimeTeardownIssue::Cgroup(
                CgroupQuiescenceError::StillPopulated {
                    waited: Duration::from_secs(2),
                },
            )],
        };
        let original_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 8192,
        };
        let finalization: RuntimeFinalization<Result<u64, RunFailure>> =
            RuntimeFinalization::Failed {
                primary: Err(RunFailure::executed("original failure", original_usage)),
                teardown,
            };
        let result = settle_finalization(
            finalization,
            |_: &u64| panic!("usage_of must not be called when the primary already failed"),
            |_: u64, _: &RuntimeTeardownError| {
                panic!("on_discarded_success must not run when the primary already failed")
            },
        );
        match result {
            Err(RunFailure::Executed {
                usage: got_usage,
                message,
            }) => {
                assert_eq!(got_usage, original_usage);
                assert!(message.contains("original failure"));
                assert!(message.contains("runtime teardown failed"));
            }
            other => panic!("expected an augmented RunFailure::Executed, got {other:?}"),
        }
    }
}
