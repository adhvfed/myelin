use super::run::StagedProductionBundle;
use super::*;
use crate::hardening::HardeningProfile;
use crate::launch_gate::{DirectChildRetirement, SandboxChild, SandboxCommand};
use crate::user_namespace::RunscInvocationMode;
use crate::{EgressPolicy, ResourceLimits};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const MIN_SESSION_MEMORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SESSION_MEMORY_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MIN_SESSION_TMPFS_BYTES: u64 = 1024 * 1024;
const MAX_SESSION_TMPFS_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SESSION_CPU_MILLIS: u32 = 16_000;
const MAX_SESSION_PIDS: u32 = 1024;
const MAX_SESSION_DURATION: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_EXEC_COMMAND_BYTES: usize = 64 * 1024;

pub trait VerifiedWorkspaceMount: Send + Sync {
    fn revalidated_mount_source(&self) -> Result<&Path, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceSessionCommand {
    Shell,
    Exec(String),
}

impl WorkspaceSessionCommand {
    fn argv(&self) -> Result<Vec<String>, WorkspaceSessionError> {
        match self {
            Self::Shell => Ok(vec!["/bin/sh".into(), "-i".into()]),
            Self::Exec(command)
                if !command.is_empty()
                    && command.len() <= MAX_EXEC_COMMAND_BYTES
                    && !command.contains('\0') =>
            {
                Ok(vec!["/bin/sh".into(), "-c".into(), command.clone()])
            }
            Self::Exec(_) => Err(WorkspaceSessionError::InvalidRequest(
                "an SSH exec command must contain 1..=65536 bytes and no NUL".into(),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceSessionLimits {
    cpu_millis: u32,
    memory_bytes: u64,
    tmpfs_bytes: u64,
    pids: u32,
    max_duration: Duration,
}

impl WorkspaceSessionLimits {
    pub fn new(
        cpu_millis: u32,
        memory_bytes: u64,
        tmpfs_bytes: u64,
        pids: u32,
        max_duration: Duration,
    ) -> Result<Self, WorkspaceSessionError> {
        if !(1..=MAX_SESSION_CPU_MILLIS).contains(&cpu_millis)
            || !(MIN_SESSION_MEMORY_BYTES..=MAX_SESSION_MEMORY_BYTES).contains(&memory_bytes)
            || !(MIN_SESSION_TMPFS_BYTES..=MAX_SESSION_TMPFS_BYTES).contains(&tmpfs_bytes)
            || !(1..=MAX_SESSION_PIDS).contains(&pids)
            || max_duration.is_zero()
            || max_duration > MAX_SESSION_DURATION
        {
            return Err(WorkspaceSessionError::InvalidRequest(
                "workspace session limits are outside the supported safety envelope".into(),
            ));
        }
        Ok(Self {
            cpu_millis,
            memory_bytes,
            tmpfs_bytes,
            pids,
            max_duration,
        })
    }

    fn resource_limits(self) -> ResourceLimits {
        ResourceLimits {
            cpu_millis: self.cpu_millis,
            mem_bytes: self.memory_bytes,
            disk_bytes: 0,
            tmpfs_bytes: self.tmpfs_bytes,
            pids_max: self.pids,
            timeout_secs: u32::try_from(self.max_duration.as_secs())
                .expect("a workspace session is capped at one day"),
        }
    }
}

impl Default for WorkspaceSessionLimits {
    fn default() -> Self {
        Self {
            cpu_millis: 2_000,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            tmpfs_bytes: 256 * 1024 * 1024,
            pids: 256,
            max_duration: Duration::from_secs(8 * 60 * 60),
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceSessionError {
    InvalidRequest(String),
    RuntimeUnavailable(String),
    Launch(String),
    Teardown(String),
}

impl core::fmt::Display for WorkspaceSessionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidRequest(reason) => {
                write!(formatter, "invalid workspace session: {reason}")
            }
            Self::RuntimeUnavailable(reason) => {
                write!(formatter, "workspace confinement unavailable: {reason}")
            }
            Self::Launch(reason) => write!(formatter, "launch confined workspace: {reason}"),
            Self::Teardown(reason) => write!(formatter, "retire confined workspace: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceSessionError {}

pub struct ConfinedWorkspaceSessionIo {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfinedWorkspaceSessionExit {
    pub code: Option<i32>,
    pub timed_out: bool,
}

pub struct ConfinedWorkspaceSession {
    child: Option<SandboxChild>,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    error: Option<ChildStderr>,
    runtime: Option<WorkspaceRuntime>,
    started_at: Instant,
    max_duration: Duration,
}

struct WorkspaceRuntime {
    bin: PathBuf,
    container_id: String,
    bundle: StagedProductionBundle,
    cgroup: MemoryCgroup,
}

impl ConfinedWorkspaceSession {
    pub fn launch(
        workspace: &impl VerifiedWorkspaceMount,
        command: WorkspaceSessionCommand,
        limits: WorkspaceSessionLimits,
    ) -> Result<Self, WorkspaceSessionError> {
        let rootfs =
            verified_gvisor_git_rootfs().map_err(WorkspaceSessionError::RuntimeUnavailable)?;
        let plan = WorkspaceSessionPlan::build(workspace, command, limits, rootfs)?;
        plan.launch(workspace)
    }

    pub fn take_io(&mut self) -> Result<ConfinedWorkspaceSessionIo, WorkspaceSessionError> {
        let stdin = self.input.take();
        let stdout = self.output.take();
        let stderr = self.error.take();
        match (stdin, stdout, stderr) {
            (Some(stdin), Some(stdout), Some(stderr)) => Ok(ConfinedWorkspaceSessionIo {
                stdin,
                stdout,
                stderr,
            }),
            _ => Err(WorkspaceSessionError::InvalidRequest(
                "workspace session I/O was already claimed".into(),
            )),
        }
    }

    pub fn wait(mut self) -> Result<ConfinedWorkspaceSessionExit, WorkspaceSessionError> {
        self.input.take();
        self.output.take();
        self.error.take();
        let (status, timed_out, retirement) = loop {
            let child = self
                .child
                .as_mut()
                .expect("a live workspace session retains its direct child");
            match child.try_wait() {
                Ok(Some(status)) => {
                    let timed_out = child.watchdog_deadline_expired();
                    break (Some(status), timed_out, DirectChildRetirement::Reaped);
                }
                Ok(None) if self.started_at.elapsed() < self.max_duration => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => break (None, true, child.kill_and_wait()),
                Err(error) => {
                    let retirement = child.kill_and_wait();
                    self.finish(retirement)?;
                    return Err(WorkspaceSessionError::Teardown(format!(
                        "wait for runsc: {error}"
                    )));
                }
            }
        };
        self.child.take();
        self.finish(retirement)?;
        Ok(ConfinedWorkspaceSessionExit {
            code: status.and_then(|value: ExitStatus| value.code()),
            timed_out,
        })
    }

    fn finish(&mut self, retirement: DirectChildRetirement) -> Result<(), WorkspaceSessionError> {
        let Some(mut runtime) = self.runtime.take() else {
            return Ok(());
        };
        let teardown = finalize_runtime(
            &runtime.bin,
            &runtime.container_id,
            &PreparedRuntimeMode::Rootless,
            runtime.cgroup,
            RUNTIME_QUIESCE_TIMEOUT,
            retirement,
        );
        let cleanup = runtime.bundle.cleanup();
        match (teardown, cleanup) {
            (Ok(_), Ok(())) => Ok(()),
            (Err(teardown), Ok(())) => Err(WorkspaceSessionError::Teardown(teardown.to_string())),
            (Ok(_), Err(cleanup)) => Err(WorkspaceSessionError::Teardown(cleanup)),
            (Err(teardown), Err(cleanup)) => Err(WorkspaceSessionError::Teardown(format!(
                "{teardown}; {cleanup}"
            ))),
        }
    }
}

impl Drop for ConfinedWorkspaceSession {
    fn drop(&mut self) {
        self.input.take();
        self.output.take();
        self.error.take();
        let retirement = self.child.as_mut().map_or(
            DirectChildRetirement::NoChildReturned,
            SandboxChild::kill_and_wait,
        );
        self.child.take();
        if let Err(error) = self.finish(retirement) {
            eprintln!("[workspace session cleanup incident] {error}");
        }
    }
}

struct WorkspaceSessionPlan {
    config: OciConfig,
    rootfs: PathBuf,
    mount_source: PathBuf,
    limits: WorkspaceSessionLimits,
}

impl WorkspaceSessionPlan {
    fn build(
        workspace: &impl VerifiedWorkspaceMount,
        command: WorkspaceSessionCommand,
        limits: WorkspaceSessionLimits,
        rootfs: PathBuf,
    ) -> Result<Self, WorkspaceSessionError> {
        let mount_source = workspace
            .revalidated_mount_source()
            .map_err(WorkspaceSessionError::RuntimeUnavailable)?
            .to_path_buf();
        let resource_limits = limits.resource_limits();
        let profile = HardeningProfile::for_execution(&resource_limits, &EgressPolicy::deny_all());
        profile
            .assert_enforced()
            .map_err(WorkspaceSessionError::RuntimeUnavailable)?;
        let mount = OciWorkspaceMount::from_revalidated_source(&mount_source)
            .map_err(WorkspaceSessionError::InvalidRequest)?;
        let config =
            OciConfig::for_fixed_command(command.argv()?, resource_limits.mem_bytes, &profile)
                .with_extra_env(vec![
                    "HOME=/workspace".into(),
                    "SHELL=/bin/sh".into(),
                    "MYELIN_WORKSPACE=1".into(),
                ])
                .with_rootless_workspace(rootfs.clone(), mount)
                .map_err(WorkspaceSessionError::InvalidRequest)?;
        Ok(Self {
            config,
            rootfs,
            mount_source,
            limits,
        })
    }

    fn launch(
        self,
        workspace: &impl VerifiedWorkspaceMount,
    ) -> Result<ConfinedWorkspaceSession, WorkspaceSessionError> {
        let resource_limits = self.limits.resource_limits();
        let cgroup = MemoryCgroup::create(resource_limits.mem_bytes, resource_limits.cpu_millis)
            .map_err(WorkspaceSessionError::RuntimeUnavailable)?;
        let mut bundle = stage_production_bundle(&self.config, &self.rootfs)
            .map_err(WorkspaceSessionError::Launch)?;
        let current_source = workspace
            .revalidated_mount_source()
            .map_err(WorkspaceSessionError::RuntimeUnavailable)?;
        if current_source != self.mount_source {
            let cleanup = bundle.cleanup();
            return Err(WorkspaceSessionError::RuntimeUnavailable(match cleanup {
                Ok(()) => "workspace mount source changed before runsc spawn".into(),
                Err(cleanup) => {
                    format!("workspace mount source changed before runsc spawn; {cleanup}")
                }
            }));
        }

        let bin = runsc_bin().to_path_buf();
        let container_id = format!(
            "myelin-workspace-{}-{}",
            std::process::id(),
            unique_suffix()
        );
        let mut command = SandboxCommand::guarded(&bin, self.limits.max_duration)
            .map_err(|error| WorkspaceSessionError::Launch(error.to_string()))?;
        {
            let process = command.command_mut();
            apply_runsc_invocation_policy(process, &bin, RunscInvocationMode::Rootless)
                .map_err(WorkspaceSessionError::RuntimeUnavailable)?;
            process
                .arg("--network=none")
                .arg("run")
                .arg("-bundle")
                .arg(&bundle.path)
                .arg(&container_id)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            cgroup
                .place_child(process)
                .map_err(|error| WorkspaceSessionError::Launch(error.to_string()))?;
        }
        command
            .kill_cgroup_on_liveness_loss(
                cgroup
                    .kill_file()
                    .map_err(|error| WorkspaceSessionError::Launch(error.to_string()))?,
            )
            .map_err(|error| WorkspaceSessionError::Launch(error.to_string()))?;
        let mut child = command
            .spawn()
            .map_err(|error| WorkspaceSessionError::Launch(error.to_string()))?;
        let input = child.stdin().take();
        let output = child.stdout().take();
        let error = child.stderr().take();
        let (Some(input), Some(output), Some(error)) = (input, output, error) else {
            let retirement = child.kill_and_wait();
            let teardown = finalize_runtime(
                &bin,
                &container_id,
                &PreparedRuntimeMode::Rootless,
                cgroup,
                RUNTIME_QUIESCE_TIMEOUT,
                retirement,
            );
            let cleanup = bundle.cleanup();
            return Err(WorkspaceSessionError::Launch(format!(
                "runsc did not expose all three session pipes; teardown={teardown:?}; \
                 cleanup={cleanup:?}"
            )));
        };

        Ok(ConfinedWorkspaceSession {
            child: Some(child),
            input: Some(input),
            output: Some(output),
            error: Some(error),
            runtime: Some(WorkspaceRuntime {
                bin,
                container_id,
                bundle,
                cgroup,
            }),
            started_at: Instant::now(),
            max_duration: self.limits.max_duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Mount {
        path: PathBuf,
        calls: AtomicUsize,
    }

    impl VerifiedWorkspaceMount for Mount {
        fn revalidated_mount_source(&self) -> Result<&Path, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(&self.path)
        }
    }

    #[test]
    fn a_workspace_shell_has_one_fixed_writable_mount_and_no_network() {
        let mount = Mount {
            path: PathBuf::from("/srv/myelin/workspaces/opaque/workspace"),
            calls: AtomicUsize::new(0),
        };
        let plan = WorkspaceSessionPlan::build(
            &mount,
            WorkspaceSessionCommand::Shell,
            WorkspaceSessionLimits::default(),
            PathBuf::from("/srv/myelin/rootfs/small-v1"),
        )
        .unwrap();
        let json = plan.config.to_json().unwrap();

        assert!(json.contains("\"args\": [\"/bin/sh\", \"-i\"]"));
        assert!(json.contains("\"user\": { \"uid\": 0, \"gid\": 0 }"));
        assert!(json.contains("\"cwd\": \"/workspace\""));
        assert!(json.contains("\"destination\": \"/workspace\""));
        assert!(json.contains("\"source\": \"/srv/myelin/workspaces/opaque/workspace\""));
        assert!(json.contains("[\"bind\", \"rw\", \"nosuid\", \"nodev\"]"));
        assert!(json.contains("\"type\": \"network\", \"path\": \"\""));
        assert!(json.contains("\"readonly\": true"));
        assert!(!json.contains("/repo"));
        assert!(!json.contains("/quarantine"));
        assert_eq!(mount.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exec_uses_the_guest_shell_without_interpolating_into_host_arguments() {
        let mount = Mount {
            path: PathBuf::from("/srv/myelin/workspaces/opaque/workspace"),
            calls: AtomicUsize::new(0),
        };
        let plan = WorkspaceSessionPlan::build(
            &mount,
            WorkspaceSessionCommand::Exec("printf '%s\\n' hello > marker".into()),
            WorkspaceSessionLimits::default(),
            PathBuf::from("/srv/myelin/rootfs/small-v1"),
        )
        .unwrap();

        assert_eq!(
            plan.config.args,
            ["/bin/sh", "-c", "printf '%s\\n' hello > marker"]
        );
    }

    #[test]
    fn unsafe_limits_commands_and_mount_sources_are_refused_before_runtime_work() {
        assert!(WorkspaceSessionLimits::new(
            0,
            2 * 1024 * 1024 * 1024,
            256 * 1024 * 1024,
            256,
            Duration::from_secs(60),
        )
        .is_err());
        assert!(WorkspaceSessionCommand::Exec(String::new()).argv().is_err());

        let mount = Mount {
            path: PathBuf::from("relative/workspace"),
            calls: AtomicUsize::new(0),
        };
        assert!(WorkspaceSessionPlan::build(
            &mount,
            WorkspaceSessionCommand::Shell,
            WorkspaceSessionLimits::default(),
            PathBuf::from("/srv/myelin/rootfs/small-v1"),
        )
        .is_err());
    }
}
