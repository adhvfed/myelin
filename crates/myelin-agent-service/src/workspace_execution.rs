use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use myelin_ci_sandbox::gvisor::{
    ConfinedWorkspaceSession, ConfinedWorkspaceSessionHandle, ConfinedWorkspaceSessionIo,
    WorkspaceSessionCommand, WorkspaceSessionLimits,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::workspace::LocalDevelopmentWorkspaceProvisioner;

pub const DEFAULT_WORKSPACE_COMMAND_TIMEOUT_SECS: u64 = 60;
pub const MAX_WORKSPACE_COMMAND_TIMEOUT_SECS: u64 = 300;
pub const MAX_WORKSPACE_COMMAND_BYTES: usize = 16 * 1024;
pub const MAX_WORKSPACE_COMMAND_OUTPUT_BYTES: usize = 32 * 1024;

const WORKSPACE_COMMAND_CPU_MILLIS: u32 = 2_000;
const WORKSPACE_COMMAND_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const WORKSPACE_COMMAND_TMPFS_BYTES: u64 = 256 * 1024 * 1024;
const WORKSPACE_COMMAND_PIDS: u32 = 256;

#[derive(Default)]
struct SharedOutputBudget {
    observed: AtomicUsize,
}

impl SharedOutputBudget {
    fn admitted_bytes(&self, requested: usize) -> usize {
        let offset = self.observed.fetch_add(requested, Ordering::AcqRel);
        requested.min(MAX_WORKSPACE_COMMAND_OUTPUT_BYTES.saturating_sub(offset))
    }

    fn exceeded(&self) -> bool {
        self.observed.load(Ordering::Acquire) > MAX_WORKSPACE_COMMAND_OUTPUT_BYTES
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCommandRequest {
    command: String,
    timeout: Duration,
}

impl WorkspaceCommandRequest {
    pub fn new(
        command: impl Into<String>,
        timeout_seconds: Option<u64>,
    ) -> Result<Self, WorkspaceExecutionError> {
        let command = command.into();
        let timeout_seconds = timeout_seconds.unwrap_or(DEFAULT_WORKSPACE_COMMAND_TIMEOUT_SECS);
        if command.is_empty()
            || command.len() > MAX_WORKSPACE_COMMAND_BYTES
            || command.contains('\0')
        {
            return Err(WorkspaceExecutionError::InvalidRequest(format!(
                "`command` must contain 1..={MAX_WORKSPACE_COMMAND_BYTES} bytes and no NUL"
            )));
        }
        if !(1..=MAX_WORKSPACE_COMMAND_TIMEOUT_SECS).contains(&timeout_seconds) {
            return Err(WorkspaceExecutionError::InvalidRequest(format!(
                "`timeout_seconds` must be between 1 and {MAX_WORKSPACE_COMMAND_TIMEOUT_SECS}"
            )));
        }
        Ok(Self {
            command,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }

    pub fn identity_hash(&self) -> String {
        let mut digest = blake3::Hasher::new();
        digest.update(b"myelin.workspace-command.request.v1\0");
        digest.update(&(self.command.len() as u64).to_be_bytes());
        digest.update(self.command.as_bytes());
        digest.update(&self.timeout.as_secs().to_be_bytes());
        digest.finalize().to_hex().to_string()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WorkspaceCommandResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub output_limit_exceeded: bool,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceExecutionError {
    InvalidRequest(String),
    WorkspaceUnavailable,
    ConfinementUnavailable,
    Io(String),
}

impl core::fmt::Display for WorkspaceExecutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidRequest(reason) => {
                write!(formatter, "invalid workspace command: {reason}")
            }
            Self::WorkspaceUnavailable => formatter.write_str("workspace is unavailable"),
            Self::ConfinementUnavailable => {
                formatter.write_str("workspace command confinement is unavailable")
            }
            Self::Io(reason) => write!(formatter, "workspace command I/O failed: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceExecutionError {}

pub trait AgentWorkspaceExecutor: Send + Sync {
    fn execute(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        storage_locator: &str,
        request: WorkspaceCommandRequest,
        lease: &dyn WorkspaceExecutionLease,
    ) -> Result<WorkspaceCommandResult, WorkspaceExecutionError>;
}

pub trait WorkspaceExecutionLease: Send + Sync {
    fn is_live(&self) -> bool;
}

impl AgentWorkspaceExecutor for LocalDevelopmentWorkspaceProvisioner {
    fn execute(
        &self,
        tenant: &str,
        workspace_id: Uuid,
        storage_locator: &str,
        request: WorkspaceCommandRequest,
        lease: &dyn WorkspaceExecutionLease,
    ) -> Result<WorkspaceCommandResult, WorkspaceExecutionError> {
        let workspace = self
            .open_verified_directory(tenant, workspace_id, storage_locator)
            .map_err(|_| WorkspaceExecutionError::WorkspaceUnavailable)?;
        let limits = WorkspaceSessionLimits::new(
            WORKSPACE_COMMAND_CPU_MILLIS,
            WORKSPACE_COMMAND_MEMORY_BYTES,
            WORKSPACE_COMMAND_TMPFS_BYTES,
            WORKSPACE_COMMAND_PIDS,
            request.timeout,
        )
        .map_err(|_| WorkspaceExecutionError::ConfinementUnavailable)?;
        let confined = ConfinedWorkspaceSession::launch(
            &workspace,
            WorkspaceSessionCommand::Exec(request.command),
            limits,
            None,
        )
        .map_err(|_| WorkspaceExecutionError::ConfinementUnavailable)?;
        capture_command(confined, lease)
    }
}

fn capture_command(
    mut confined: ConfinedWorkspaceSession,
    lease: &dyn WorkspaceExecutionLease,
) -> Result<WorkspaceCommandResult, WorkspaceExecutionError> {
    let ConfinedWorkspaceSessionIo::Pipes {
        stdin,
        stdout,
        stderr,
    } = confined
        .take_io()
        .map_err(|error| WorkspaceExecutionError::Io(error.to_string()))?
    else {
        return Err(WorkspaceExecutionError::Io(
            "a non-interactive command unexpectedly received a terminal".into(),
        ));
    };
    drop(stdin);

    let started = Instant::now();
    let handle = confined.handle();
    let output_budget = Arc::new(SharedOutputBudget::default());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (stdout, stderr, exit) = std::thread::scope(|scope| {
        let stdout_handle = handle.clone();
        let stdout_budget = Arc::clone(&output_budget);
        let stdout = scope
            .spawn(move || read_bounded_output(stdout, stdout_budget, stdout_handle, "stdout"));
        let stderr_handle = handle.clone();
        let stderr_budget = Arc::clone(&output_budget);
        let stderr = scope
            .spawn(move || read_bounded_output(stderr, stderr_budget, stderr_handle, "stderr"));
        let wait = scope.spawn(move || confined.wait());
        let monitor_handle = handle.clone();
        let monitor_finished = Arc::clone(&finished);
        let monitor_cancelled = Arc::clone(&cancelled);
        let monitor = scope.spawn(move || {
            while !monitor_finished.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(50));
                if !lease.is_live() {
                    monitor_cancelled.store(true, Ordering::Release);
                    monitor_handle.terminate();
                    return;
                }
            }
        });

        let stdout = stdout
            .join()
            .map_err(|_| WorkspaceExecutionError::Io("workspace stdout reader terminated".into()));
        let stderr = stderr
            .join()
            .map_err(|_| WorkspaceExecutionError::Io("workspace stderr reader terminated".into()));
        let exit = wait
            .join()
            .map_err(|_| WorkspaceExecutionError::Io("workspace wait worker terminated".into()));
        finished.store(true, Ordering::Release);
        let monitor = monitor
            .join()
            .map_err(|_| WorkspaceExecutionError::Io("workspace lease monitor terminated".into()));
        monitor?;
        Ok::<_, WorkspaceExecutionError>((
            stdout??,
            stderr??,
            exit?.map_err(|error| WorkspaceExecutionError::Io(error.to_string()))?,
        ))
    })?;
    let output_limit_exceeded = output_budget.exceeded();
    let cancelled = cancelled.load(Ordering::Acquire);

    Ok(WorkspaceCommandResult {
        exit_code: exit.code,
        stdout,
        stderr,
        timed_out: exit.timed_out && !output_limit_exceeded && !cancelled,
        cancelled,
        output_limit_exceeded,
        elapsed: started.elapsed(),
    })
}

fn read_bounded_output(
    mut source: impl Read,
    output_budget: Arc<SharedOutputBudget>,
    session: ConfinedWorkspaceSessionHandle,
    stream: &'static str,
) -> Result<Vec<u8>, WorkspaceExecutionError> {
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                session.terminate();
                return Err(WorkspaceExecutionError::Io(format!(
                    "read command {stream}: {error}"
                )));
            }
        };
        if read == 0 {
            return Ok(captured);
        }
        let retained = output_budget.admitted_bytes(read);
        captured.extend_from_slice(&buffer[..retained]);
        if retained < read {
            session.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_request_has_one_small_obvious_safety_envelope() {
        let request = WorkspaceCommandRequest::new("printf ready", None).unwrap();
        assert_eq!(
            request.timeout,
            Duration::from_secs(DEFAULT_WORKSPACE_COMMAND_TIMEOUT_SECS)
        );
        assert_eq!(request.identity_hash(), request.identity_hash());
        assert_ne!(
            request.identity_hash(),
            WorkspaceCommandRequest::new("printf changed", None)
                .unwrap()
                .identity_hash()
        );
        assert_ne!(
            request.identity_hash(),
            WorkspaceCommandRequest::new("printf ready", Some(1))
                .unwrap()
                .identity_hash()
        );
        for command in [
            String::new(),
            "x\0y".into(),
            "x".repeat(MAX_WORKSPACE_COMMAND_BYTES + 1),
        ] {
            assert!(matches!(
                WorkspaceCommandRequest::new(command, None),
                Err(WorkspaceExecutionError::InvalidRequest(_))
            ));
        }
        for timeout in [0, MAX_WORKSPACE_COMMAND_TIMEOUT_SECS + 1] {
            assert!(matches!(
                WorkspaceCommandRequest::new("true", Some(timeout)),
                Err(WorkspaceExecutionError::InvalidRequest(_))
            ));
        }
    }

    #[test]
    fn output_from_both_streams_shares_one_bound() {
        let budget = SharedOutputBudget::default();
        assert_eq!(
            budget.admitted_bytes(MAX_WORKSPACE_COMMAND_OUTPUT_BYTES - 8),
            MAX_WORKSPACE_COMMAND_OUTPUT_BYTES - 8
        );
        assert_eq!(
            budget.admitted_bytes(16),
            8,
            "the other stream receives only the shared remainder"
        );
        assert_eq!(
            budget.admitted_bytes(16),
            0,
            "no stream can retain bytes after the common bound"
        );
        assert!(budget.exceeded());
    }
}
