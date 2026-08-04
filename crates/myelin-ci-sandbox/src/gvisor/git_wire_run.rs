use super::*;
use crate::hardening::HardeningProfile;
use crate::redaction::RedactionPlan;
use crate::{
    CompletionSettlementOwner, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, LaunchPermit,
    MeterTarget, ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks,
    SandboxBackend, SandboxHandle, SandboxLaunch, TrustTier, WorkspaceSpec,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const WIRE_REPO_MOUNT: &str = "/repo";
pub const WIRE_QUARANTINE_MOUNT: &str = "/quarantine";

pub const RECEIVE_PACK_INGEST_SCRIPT: &str = "set -e
export HOME=/tmp
export GIT_CONFIG_NOSYSTEM=1
export GIT_EXEC_PATH=/usr/lib/git-core
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=safe.directory
export GIT_CONFIG_VALUE_0=*
export GIT_DIR=/tmp/q
rm -rf /tmp/q
git init --bare -q /tmp/q
printf '%s\\n' /repo/objects > /tmp/q/objects/info/alternates
git index-pack --stdin --fix-thin 1>&2
git verify-pack -v /tmp/q/objects/pack/pack-*.idx | awk '$2 ~ /^(commit|tree|blob|tag)$/ {print $1}' > /tmp/oids
git cat-file --batch < /tmp/oids
";
pub const WIRE_STDIN_BOUND: usize = 64 * 1024 * 1024;
pub const WIRE_STDOUT_BOUND: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct GitWireSpec {
    repo_host_path: PathBuf,
    root: PathBuf,
    pub(super) git_argv: Vec<String>,
    pub(super) stdin: Vec<u8>,
    env: Vec<String>,
    quarantine_host_path: Option<PathBuf>,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    meter_to: MeterTarget,
    idem_token: IdemToken,
}

impl GitWireSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn for_repo(
        root: &Path,
        tenant: &str,
        region: &str,
        repo: &str,
        git_argv: Vec<String>,
        stdin: Vec<u8>,
        env: Vec<String>,
        quarantine_host_path: Option<PathBuf>,
        limits: ResourceLimits,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<GitWireSpec, WireError> {
        let repo_host_path = resolve_bare_repo_path(root, tenant, region, repo)?;
        Ok(GitWireSpec {
            repo_host_path,
            root: root.to_path_buf(),
            git_argv,
            stdin,
            env,
            quarantine_host_path,
            limits,
            run_token,
            meter_to,
            idem_token,
        })
    }

    pub fn repo_host_path(&self) -> &Path {
        &self.repo_host_path
    }
}

impl GvisorBackend {
    pub fn launch_git_wire(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    pub fn launch_git_wire_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    pub fn launch_git_receive_pack(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    pub fn launch_git_receive_pack_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    fn launch_git_command(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        command: Vec<String>,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let job = build_git_wire_job(spec, command, cancellation)?;

        if hooks.completion_settlement_owner() == CompletionSettlementOwner::TerminalReporter {
            return Err(WireError::Runtime(
                "git-wire launch requires Hook-owned completion settlement - it is a direct \
                 synchronous path with no terminal reporter above it to defer a retryable-attempt \
                 accounting to"
                    .to_string(),
            ));
        }

        hooks.enforce_isolation_floor(&job)?;
        let (cfg, rootfs) = build_git_wire_oci_config(&job, spec)?;
        let reserve = hooks.reserve(&job)?;
        let launch_permit = match hooks.acquire_launch_permit(&job) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                hooks.release_unused(&job, &reserve)?;
                return Err(attribute_error.into());
            }
        };

        let (
            ContainerRun {
                child,
                bundle_dir,
                result,
                run_error,
            },
            stdout_truncated,
        ) = match run_git_wire_container(
            &job,
            &cfg,
            spec.stdin.clone(),
            &rootfs,
            cancellation,
            launch_permit,
        ) {
            Ok(run_and_truncated) => run_and_truncated,
            Err(run_failure) => {
                return Err(dispose_git_wire_run_failure(
                    hooks,
                    &job,
                    &reserve,
                    run_failure,
                ))
            }
        };

        if stdout_truncated {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            if let Err(settle_error) = hooks.settle_completed(&job, &reserve, result.usage) {
                return Err(WireError::Runtime(format!(
                    "response exceeded the wire cap AND settling its measured usage also failed \
                     ({settle_error}) - reservation may be leaked"
                )));
            }
            return Err(WireError::OutputTooLarge {
                cap: job.limits.disk_bytes as usize,
            });
        }

        let guest_id = format!("runsc-gitwire-{}", job.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        if let Err(error) = hooks.settle_completed(&job, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(error.into());
        }

        if let Some(error) = run_error {
            let _ = self.kill(&SandboxHandle { guest_id });
            return Err(WireError::Runtime(error));
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: true,
        })
    }
}

pub(super) fn build_git_wire_job(
    spec: &GitWireSpec,
    command: Vec<String>,
    cancellation: &AtomicBool,
) -> Result<JobSpec, WireError> {
    if cancellation.load(Ordering::Acquire) {
        return Err(WireError::Runtime(
            "Git wire launch cancelled by process shutdown".into(),
        ));
    }
    if spec.stdin.len() > WIRE_STDIN_BOUND {
        return Err(WireError::StdinTooLarge {
            len: spec.stdin.len(),
            cap: WIRE_STDIN_BOUND,
        });
    }
    assert_repo_under_root(&spec.root, &spec.repo_host_path)?;
    let image = ImageRef::pinned(
        "sandbox/git-wire@sha256:0000000000000000000000000000000000000000000000000000000000000000",
    )
    .map_err(|e| WireError::Runtime(e.to_string()))?;
    JobSpec::new(
        JobKind::Ci,
        image,
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        spec.limits,
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        spec.run_token.clone(),
        spec.meter_to.clone(),
        spec.idem_token.clone(),
    )
    .map_err(|e| WireError::Runtime(e.to_string()))
}

pub(super) fn build_git_wire_oci_config(
    job: &JobSpec,
    spec: &GitWireSpec,
) -> Result<(OciConfig, PathBuf), WireError> {
    let profile = HardeningProfile::derive(job);
    profile.assert_enforced().map_err(WireError::Hardening)?;

    let rootfs = verified_gvisor_git_rootfs().map_err(WireError::Runtime)?;
    let cfg = OciConfig::from_spec(job, &profile)
        .with_extra_env(spec.env.clone())
        .with_rootless_host_mounts(
            rootfs.clone(),
            spec.repo_host_path.clone(),
            spec.quarantine_host_path.clone(),
        )
        .map_err(WireError::Runtime)?;
    Ok((cfg, rootfs))
}

fn dispose_git_wire_run_failure(
    hooks: &RunnerHooks,
    job: &JobSpec,
    reserve: &ReserveHandle,
    run_failure: RunFailure,
) -> WireError {
    let message = run_failure.to_string();
    match run_failure {
        RunFailure::Uncommitted { .. } => {
            if let Err(settle_error) = hooks.release_unused(job, reserve) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (uncommitted: {message}) AND \
                     release_unused also failed ({settle_error}) - reservation may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::CommitOutcomeUnknown { .. } => {
            WireError::Runtime(format!(
                "durable launch commit outcome unknown, needs reconciliation: {message}"
            ))
        }
        RunFailure::CommittedButNotExecuted { .. } => {
            let zero = ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            };
            if let Err(settle_error) = hooks.settle_completed(job, reserve, zero) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (committed but not executed: {message}) \
                     AND its zero-usage settlement also failed ({settle_error}) - reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::Executed { usage, .. } => {
            if let Err(settle_error) = hooks.settle_completed(job, reserve, usage) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (executed: {message}) AND its \
                     conservative-usage settlement also failed ({settle_error}) - reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
    }
}

fn run_git_wire_container(
    job: &JobSpec,
    cfg: &OciConfig,
    stdin: Vec<u8>,
    rootfs: &Path,
    cancellation: &AtomicBool,
    launch_permit: LaunchPermit,
) -> Result<(ContainerRun, bool), RunFailure> {
    let (finalization, _bundle_cleanup_proof) =
        run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, launch_permit);
    settle_finalization(
        finalization?,
        |(run, _): &(ContainerRun, bool)| run.result.usage,
        |(run, _): (ContainerRun, bool), teardown| {
            discard_container_run_after_teardown_failure(run, teardown)
        },
    )
}

pub(super) type GitWireHopFinalization =
    RuntimeFinalization<Result<(ContainerRun, bool), RunFailure>>;

pub(super) type BundleCleanupProof = Result<(), String>;

pub(super) fn run_git_wire_container_raw(
    job: &JobSpec,
    cfg: &OciConfig,
    stdin: Vec<u8>,
    rootfs: &Path,
    cancellation: &AtomicBool,
    launch_permit: LaunchPermit,
) -> (
    Result<GitWireHopFinalization, RunFailure>,
    BundleCleanupProof,
) {
    let bin = runsc_bin();
    if !rootfs.exists() {
        return (
            Err(RunFailure::uncommitted(format!(
                "staged gVisor git rootfs absent: {} (the git wire REQUIRES a real `git` in the \
                 guest - stage a git-bearing rootfs and point {ENV_GVISOR_GIT_ROOTFS} at it; see \
                 tests/git_wire_prod_exec_test.rs)",
                rootfs.display()
            ))),
            Ok(()),
        );
    }
    let prepared_mode = PreparedRuntimeMode::Rootless;
    let mode = match require_oci_layout_matches_prepared_mode(cfg, &prepared_mode) {
        Ok(mode) => mode,
        Err(e) => return (Err(RunFailure::uncommitted(e)), Ok(())),
    };

    let bundle_dir = match stage_git_wire_bundle(cfg) {
        Ok(dir) => dir,
        Err(e) => {
            let cleanup_proof = if e.leaked {
                Err(e.message.clone())
            } else {
                Ok(())
            };
            return (Err(RunFailure::uncommitted(e.message)), cleanup_proof);
        }
    };
    let container_id = format!("myelin-gitwire-{}-{}", std::process::id(), unique_suffix());

    let cgroup = match MemoryCgroup::create(job.limits.mem_bytes, job.limits.cpu_millis) {
        Ok(cgroup) => cgroup,
        Err(e) => {
            let cleanup_proof = std::fs::remove_dir_all(&bundle_dir)
                .map_err(|re| format!("bundle dir {bundle_dir:?} removal failed: {re}"));
            return (Err(RunFailure::uncommitted(e)), cleanup_proof);
        }
    };

    let timeout = Duration::from_secs(job.limits.timeout_secs as u64);
    let wire_cap = job.limits.disk_bytes as usize;
    let (result, child_retirement) = run_and_capture(
        bin,
        &bundle_dir,
        &container_id,
        timeout,
        job.limits.mem_bytes,
        RunCaptureOptions {
            stdin: Some(StdinSource::Bytes(stdin)),
            stdout_mode: StdoutMode::StreamToFile { bound: wire_cap },
            cancellation,
            redaction: RedactionPlan::none(),
            output: None,
        },
        Some(launch_permit),
        mode,
        &cgroup,
    );
    let (primary, cleanup_proof): (Result<(ContainerRun, bool), RunFailure>, BundleCleanupProof) =
        match result {
            Ok(outcome) => {
                let stdout_truncated = outcome.stdout_truncated;
                let result = build_result(job, &outcome, &RedactionPlan::none());
                (
                    Ok((
                        ContainerRun {
                            child: Box::new(SpawnedRunsc {
                                bin,
                                container_id: container_id.clone(),
                                mode,
                            }),
                            bundle_dir,
                            result,
                            run_error: outcome.stream_error,
                        },
                        stdout_truncated,
                    )),
                    Ok(()),
                )
            }
            Err(e) => {
                let cleanup_proof = std::fs::remove_dir_all(&bundle_dir)
                    .map_err(|re| format!("bundle dir {bundle_dir:?} removal failed: {re}"));
                (Err(e), cleanup_proof)
            }
        };
    (
        Ok(finalize_and_merge(
            primary,
            bin,
            &container_id,
            &prepared_mode,
            cgroup,
            RUNTIME_QUIESCE_TIMEOUT,
            child_retirement,
        )),
        cleanup_proof,
    )
}

pub(super) struct StageBundleError {
    pub(super) message: String,
    leaked: bool,
}

impl std::fmt::Display for StageBundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub(super) fn stage_config_only_bundle(
    cfg: &OciConfig,
    label: &str,
) -> Result<PathBuf, StageBundleError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let bundle = std::env::temp_dir().join(format!(
        "myelin-{label}-bundle-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&bundle)
        .map_err(|e| StageBundleError {
            message: format!("create bundle dir {bundle:?}: {e}"),
            leaked: false,
        })?;
    let json = cfg.to_json_zeroizing().map_err(|message| {
        let (message, leaked) = match std::fs::remove_dir_all(&bundle) {
            Ok(()) => (message, false),
            Err(cleanup) => (
                format!(
                    "{message} AND cleaning up the partially-staged bundle dir also failed: \
                     {cleanup}"
                ),
                true,
            ),
        };
        StageBundleError { message, leaked }
    })?;
    let write_result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(bundle.join("config.json"))
        .and_then(|mut file| {
            file.write_all(json.as_bytes())
                .and_then(|()| file.sync_all())
        });
    if let Err(e) = write_result {
        return Err(match std::fs::remove_dir_all(&bundle) {
            Ok(()) => StageBundleError {
                message: format!("write config.json: {e}"),
                leaked: false,
            },
            Err(cleanup_err) => StageBundleError {
                message: format!(
                    "write config.json: {e} AND cleaning up the partially-staged bundle dir also \
                     failed: {cleanup_err}"
                ),
                leaked: true,
            },
        });
    }
    Ok(bundle)
}

fn stage_git_wire_bundle(cfg: &OciConfig) -> Result<PathBuf, StageBundleError> {
    stage_config_only_bundle(cfg, "gitwire")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::gvisor::test_fixtures::*;
    use crate::{
        CompletionSettlementOwner, IdemToken, MeterTarget, ReserveHandle, ResourceLimits,
        ResourceUsage, RunTokenCredential, RunnerHooks,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn cancelled_git_wire_refuses_before_reserve_or_spawn() {
        let cancelled = AtomicBool::new(true);
        let spec = GitWireSpec {
            repo_host_path: PathBuf::from("/absent/repo.git"),
            root: PathBuf::from("/absent"),
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "cancel", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "cancel".into(),
            },
            idem_token: IdemToken("cancel".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &ok_hooks(),
            &cancelled,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(message)) if message.contains("cancelled by process shutdown"))
        );
    }

    #[test]
    fn git_wire_refuses_reporter_owned_hooks_before_reserve() {
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-reporter-owned-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let repo = root.join("acme").join("fr-par").join("widgets.git");
        std::fs::create_dir_all(&repo).unwrap();

        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let spec = GitWireSpec {
            repo_host_path: repo,
            root,
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "reporter-owned", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "reporter-owned".into(),
            },
            idem_token: IdemToken("reporter-owned".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &hooks,
            &NEVER_CANCELLED,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(ref message)) if message.contains("requires Hook-owned")),
            "expected a Hook-ownership refusal, got {result:?}"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "reporter-owned hooks must refuse before reserve is ever called"
        );
    }

    #[test]
    fn dispose_git_wire_run_failure_dispatches_all_four_phases() {
        fn recording_hooks() -> (RunnerHooks, Arc<Mutex<Vec<ResourceUsage>>>) {
            let settled = Arc::new(Mutex::new(Vec::new()));
            let settled_at = settled.clone();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::Hook,
                Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
                Box::new(move |_spec, _h, usage| {
                    settled_at.lock().unwrap().push(usage);
                    Ok(())
                }),
                Box::new(|_t| Ok(())),
                Box::new(|_s| Ok(())),
            );
            (hooks, settled)
        }
        let job = spec(vec![]);
        let reserve = ReserveHandle(job.meter_to.reserve_id.clone());
        let zero = ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        };

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::uncommitted("injected uncommitted"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected uncommitted")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![zero],
            "release_unused settles zero"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::commit_outcome_unknown("injected outcome unknown"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("needs reconciliation")));
        assert!(
            settled.lock().unwrap().is_empty(),
            "commit-outcome-unknown must never release or settle"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::committed_but_not_executed("injected committed but not executed"),
        );
        assert!(
            matches!(error, WireError::Runtime(m) if m.contains("injected committed but not executed"))
        );
        assert_eq!(*settled.lock().unwrap(), vec![zero]);

        let (hooks, settled) = recording_hooks();
        let fallback_usage = ResourceUsage {
            cpu_seconds: 4,
            mem_byte_seconds: 400,
        };
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::executed("injected executed", fallback_usage),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected executed")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![fallback_usage],
            "executed must settle the carried conservative usage, never zero"
        );
    }

    #[test]
    fn symlinked_repo_path_is_refused_before_mount() {
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-symlink-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let outside = tmp.join("outside-the-tree");
        std::fs::create_dir_all(root.join("acme").join("fr-par")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let real = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();
        std::fs::create_dir_all(&real).unwrap();
        assert!(
            assert_repo_under_root(&root, &real).is_ok(),
            "a real directory under the root must be admitted"
        );

        let evil = resolve_bare_repo_path(&root, "acme", "fr-par", "evil").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &evil).unwrap();
        let r = assert_repo_under_root(&root, &evil);
        assert!(
            matches!(r, Err(WireError::Path(_))),
            "a symlinked repo path escaping the tree must be refused, got {r:?}"
        );

        let root2 = tmp.join("git-root-2");
        std::fs::create_dir_all(&root2).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root2.join("acme")).unwrap();
        let leak_parent = outside.join("fr-par");
        std::fs::create_dir_all(leak_parent.join("widgets.git")).unwrap();
        let via_symlinked_component =
            resolve_bare_repo_path(&root2, "acme", "fr-par", "widgets").unwrap();
        let r2 = assert_repo_under_root(&root2, &via_symlinked_component);
        assert!(
            matches!(r2, Err(WireError::Path(_))),
            "a symlinked intermediate component leaving the root must be refused, got {r2:?}"
        );

        let absent = resolve_bare_repo_path(&root, "acme", "fr-par", "ghost").unwrap();
        assert!(matches!(
            assert_repo_under_root(&root, &absent),
            Err(WireError::Path(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
