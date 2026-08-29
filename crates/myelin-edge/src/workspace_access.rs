use std::sync::Arc;

use chrono::Utc;
use myelin_agent_service::workspace::{
    validate_workspace_file_path, AgentWorkspaceStore, WorkspaceAccessError, WorkspaceFile,
    WrittenWorkspaceFile, MAX_WORKSPACE_FILE_BYTES,
};
use myelin_agent_service::workspace_execution::{
    AgentWorkspaceExecutor, WorkspaceCommandRequest, WorkspaceCommandResult,
    WorkspaceExecutionError, WorkspaceExecutionLease,
};
use myelin_storage::{
    AgentThreadRunBinding, AgentToolEffectStore, DurableAgentThreadBacking, ToolEffectBegin,
    ToolEffectCompletion, ToolEffectError,
};
use myelin_tenancy::TenantId;
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::runtime::drive_edge_future;
use crate::EdgeError;

#[derive(Clone)]
pub struct DurableAgentWorkspaceAccess {
    threads: DurableAgentThreadBacking,
    files: Arc<dyn AgentWorkspaceStore>,
    executor: Arc<dyn AgentWorkspaceExecutor>,
    effects: AgentToolEffectStore,
    runtime: Handle,
}

impl DurableAgentWorkspaceAccess {
    pub fn new(
        threads: DurableAgentThreadBacking,
        files: Arc<dyn AgentWorkspaceStore>,
        executor: Arc<dyn AgentWorkspaceExecutor>,
        effects: AgentToolEffectStore,
        runtime: Handle,
    ) -> Self {
        Self {
            threads,
            files,
            executor,
            effects,
            runtime,
        }
    }

    pub fn for_run(
        &self,
        tenant: &str,
        run_id: &str,
        agent_id: &str,
        token_jti: &str,
    ) -> Result<WorkspaceRunAccess, EdgeError> {
        Ok(WorkspaceRunAccess {
            service: self.clone(),
            tenant: tenant.to_string(),
            run_id: canonical_uuid("agent run", run_id)?,
            agent_id: canonical_uuid("agent", agent_id)?,
            token_jti: token_jti.to_string(),
        })
    }
}

#[derive(Clone)]
pub struct WorkspaceRunAccess {
    service: DurableAgentWorkspaceAccess,
    tenant: String,
    run_id: Uuid,
    agent_id: Uuid,
    token_jti: String,
}

pub struct ReadWorkspaceFile {
    pub binding: AgentThreadRunBinding,
    pub file: WorkspaceFile,
}

pub struct WrittenWorkspaceFileOutcome {
    pub binding: AgentThreadRunBinding,
    pub file: WrittenWorkspaceFile,
}

pub struct ExecutedWorkspaceCommandOutcome {
    pub binding: AgentThreadRunBinding,
    pub command: WorkspaceCommandResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceRunAccessError {
    InvalidPath(String),
    InvalidCommand(String),
    Indeterminate,
    NotFound,
    TooLarge,
    Unavailable,
}

impl WorkspaceRunAccess {
    pub fn read_file(&self, path: &str) -> Result<ReadWorkspaceFile, WorkspaceRunAccessError> {
        let (binding, workspace_id) = self.live_workspace()?;
        let file = self
            .service
            .files
            .read_file(&self.tenant, workspace_id, path)
            .map_err(map_file_error)?;
        Ok(ReadWorkspaceFile { binding, file })
    }

    pub fn write_file(
        &self,
        path: &str,
        bytes: &[u8],
        idempotency_key: &str,
        requested_by: &str,
    ) -> Result<WrittenWorkspaceFileOutcome, WorkspaceRunAccessError> {
        validate_workspace_file_path(path).map_err(map_file_error)?;
        if bytes.len() > MAX_WORKSPACE_FILE_BYTES {
            return Err(WorkspaceRunAccessError::TooLarge);
        }
        let (binding, workspace_id) = self.live_workspace()?;
        let effect_key = workspace_write_effect_key(idempotency_key);
        let request_hash = workspace_write_request_hash(path, bytes);
        match self
            .service
            .effects
            .begin_at_most_once(
                &TenantId(self.tenant.clone()),
                &self.run_id.to_string(),
                &effect_key,
                &request_hash,
                requested_by,
            )
            .map_err(map_effect_error)?
        {
            ToolEffectBegin::Completed(result) => return decode_write_replay(binding, &result),
            ToolEffectBegin::Indeterminate | ToolEffectBegin::Unreplayable => {
                return Err(WorkspaceRunAccessError::Indeterminate)
            }
            ToolEffectBegin::Execute => {}
        }
        let file = self
            .service
            .files
            .write_file(&self.tenant, workspace_id, path, bytes)
            .map_err(map_file_error)?;
        let encoded =
            serde_json::to_string(&file).map_err(|_| WorkspaceRunAccessError::Indeterminate)?;
        match self
            .service
            .effects
            .complete(
                &TenantId(self.tenant.clone()),
                &self.run_id.to_string(),
                &effect_key,
                &request_hash,
                requested_by,
                &encoded,
            )
            .map_err(|_| WorkspaceRunAccessError::Indeterminate)?
        {
            ToolEffectCompletion::Applied => Ok(WrittenWorkspaceFileOutcome { binding, file }),
            ToolEffectCompletion::Replayed(result) => decode_write_replay(binding, &result),
        }
    }

    pub fn execute(
        &self,
        request: WorkspaceCommandRequest,
        idempotency_key: &str,
        requested_by: &str,
    ) -> Result<ExecutedWorkspaceCommandOutcome, WorkspaceRunAccessError> {
        let (binding, workspace_id) = self.live_workspace()?;
        let effect_key = workspace_exec_effect_key(idempotency_key);
        let request_hash = request.identity_hash();
        match self
            .service
            .effects
            .begin_at_most_once(
                &TenantId(self.tenant.clone()),
                &self.run_id.to_string(),
                &effect_key,
                &request_hash,
                requested_by,
            )
            .map_err(map_effect_error)?
        {
            ToolEffectBegin::Completed(result) => return decode_execution_replay(binding, &result),
            ToolEffectBegin::Indeterminate | ToolEffectBegin::Unreplayable => {
                return Err(WorkspaceRunAccessError::Indeterminate)
            }
            ToolEffectBegin::Execute => {}
        }
        let command = tokio::task::block_in_place(|| {
            self.service.executor.execute(
                &self.tenant,
                workspace_id,
                &binding.workspace_storage_locator,
                request,
                self,
            )
        })
        .map_err(map_execution_error)?;
        let encoded =
            serde_json::to_string(&command).map_err(|_| WorkspaceRunAccessError::Indeterminate)?;
        match self
            .service
            .effects
            .complete(
                &TenantId(self.tenant.clone()),
                &self.run_id.to_string(),
                &effect_key,
                &request_hash,
                requested_by,
                &encoded,
            )
            .map_err(|_| WorkspaceRunAccessError::Indeterminate)?
        {
            ToolEffectCompletion::Applied => {
                Ok(ExecutedWorkspaceCommandOutcome { binding, command })
            }
            ToolEffectCompletion::Replayed(result) => decode_execution_replay(binding, &result),
        }
    }

    fn live_workspace(&self) -> Result<(AgentThreadRunBinding, Uuid), WorkspaceRunAccessError> {
        let result = drive_edge_future(
            &self.service.runtime,
            self.service.threads.live_binding_for_run(
                &self.tenant,
                self.run_id,
                self.agent_id,
                &self.token_jti,
                Utc::now(),
            ),
            "agent workspace access",
        )
        .map_err(|_| WorkspaceRunAccessError::Unavailable)?
        .map_err(|_| WorkspaceRunAccessError::Unavailable)?
        .ok_or(WorkspaceRunAccessError::NotFound)?;
        let workspace_id = Uuid::parse_str(&result.workspace_id)
            .ok()
            .filter(|id| id.to_string() == result.workspace_id)
            .ok_or(WorkspaceRunAccessError::Unavailable)?;
        Ok((result, workspace_id))
    }
}

fn workspace_exec_effect_key(idempotency_key: &str) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.workspace-command.effect.v1\0");
    digest.update(&(idempotency_key.len() as u64).to_be_bytes());
    digest.update(idempotency_key.as_bytes());
    format!("workspace.exec:{}", digest.finalize().to_hex())
}

fn workspace_write_effect_key(idempotency_key: &str) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.workspace-write.effect.v1\0");
    digest.update(&(idempotency_key.len() as u64).to_be_bytes());
    digest.update(idempotency_key.as_bytes());
    format!("workspace.write_file:{}", digest.finalize().to_hex())
}

fn workspace_write_request_hash(path: &str, bytes: &[u8]) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.workspace-write.request.v1\0");
    digest.update(&(path.len() as u64).to_be_bytes());
    digest.update(path.as_bytes());
    digest.update(&(bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().to_hex().to_string()
}

fn decode_write_replay(
    binding: AgentThreadRunBinding,
    encoded: &str,
) -> Result<WrittenWorkspaceFileOutcome, WorkspaceRunAccessError> {
    let file = serde_json::from_str(encoded).map_err(|_| WorkspaceRunAccessError::Unavailable)?;
    Ok(WrittenWorkspaceFileOutcome { binding, file })
}

fn decode_execution_replay(
    binding: AgentThreadRunBinding,
    encoded: &str,
) -> Result<ExecutedWorkspaceCommandOutcome, WorkspaceRunAccessError> {
    let command =
        serde_json::from_str(encoded).map_err(|_| WorkspaceRunAccessError::Unavailable)?;
    Ok(ExecutedWorkspaceCommandOutcome { binding, command })
}

fn map_effect_error(error: ToolEffectError) -> WorkspaceRunAccessError {
    match error {
        ToolEffectError::Conflict => WorkspaceRunAccessError::InvalidCommand(
            "the idempotency key was already used for another workspace mutation".into(),
        ),
        ToolEffectError::Unreplayable => WorkspaceRunAccessError::Indeterminate,
        ToolEffectError::InvalidInput(_)
        | ToolEffectError::Missing
        | ToolEffectError::Erased
        | ToolEffectError::Restricted
        | ToolEffectError::Storage(_) => WorkspaceRunAccessError::Unavailable,
    }
}

impl WorkspaceExecutionLease for WorkspaceRunAccess {
    fn is_live(&self) -> bool {
        self.live_workspace().is_ok()
    }
}

fn map_execution_error(error: WorkspaceExecutionError) -> WorkspaceRunAccessError {
    match error {
        WorkspaceExecutionError::InvalidRequest(reason) => {
            WorkspaceRunAccessError::InvalidCommand(reason)
        }
        WorkspaceExecutionError::WorkspaceUnavailable => WorkspaceRunAccessError::NotFound,
        WorkspaceExecutionError::ConfinementUnavailable | WorkspaceExecutionError::Io(_) => {
            WorkspaceRunAccessError::Unavailable
        }
    }
}

fn canonical_uuid(label: &str, value: &str) -> Result<Uuid, EdgeError> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| id.to_string() == value)
        .ok_or_else(|| EdgeError::Internal(format!("stored {label} id is invalid")))
}

fn map_file_error(error: WorkspaceAccessError) -> WorkspaceRunAccessError {
    match error {
        WorkspaceAccessError::InvalidPath(reason) => WorkspaceRunAccessError::InvalidPath(reason),
        WorkspaceAccessError::NotFound | WorkspaceAccessError::NotRegularFile => {
            WorkspaceRunAccessError::NotFound
        }
        WorkspaceAccessError::TooLarge => WorkspaceRunAccessError::TooLarge,
        WorkspaceAccessError::LocatorMismatch
        | WorkspaceAccessError::UnsafeStorage(_)
        | WorkspaceAccessError::Io(_) => WorkspaceRunAccessError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_workspace_identity_failures_are_never_projected_as_user_input_errors() {
        for error in [
            WorkspaceAccessError::LocatorMismatch,
            WorkspaceAccessError::UnsafeStorage("host path changed".into()),
            WorkspaceAccessError::Io("disk unavailable".into()),
        ] {
            assert_eq!(map_file_error(error), WorkspaceRunAccessError::Unavailable);
        }
    }

    #[test]
    fn confinement_failures_are_not_misreported_as_agent_input_errors() {
        for error in [
            WorkspaceExecutionError::ConfinementUnavailable,
            WorkspaceExecutionError::Io("runsc pipe closed".into()),
        ] {
            assert_eq!(
                map_execution_error(error),
                WorkspaceRunAccessError::Unavailable
            );
        }
    }

    #[test]
    fn file_write_journal_identity_is_retry_stable_bound_and_opaque() {
        let key = workspace_write_effect_key("retry-sensitive");
        assert_eq!(key, workspace_write_effect_key("retry-sensitive"));
        assert_ne!(key, workspace_write_effect_key("another-retry"));
        assert_ne!(key, workspace_exec_effect_key("retry-sensitive"));
        assert!(!key.contains("retry-sensitive"));

        let request = workspace_write_request_hash("notes/continuity.md", b"diagnosis");
        assert_eq!(
            request,
            workspace_write_request_hash("notes/continuity.md", b"diagnosis")
        );
        assert_ne!(
            request,
            workspace_write_request_hash("notes/other.md", b"diagnosis")
        );
        assert_ne!(
            request,
            workspace_write_request_hash("notes/continuity.md", b"new diagnosis")
        );
        for secret in ["notes/continuity.md", "diagnosis"] {
            assert!(!request.contains(secret));
        }
    }
}
