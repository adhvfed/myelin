use std::sync::Arc;

use chrono::Utc;
use myelin_agent_service::workspace::{
    AgentWorkspaceStore, WorkspaceAccessError, WorkspaceFile, WrittenWorkspaceFile,
};
use myelin_storage::{AgentThreadRunBinding, DurableAgentThreadBacking};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::runtime::drive_edge_future;
use crate::EdgeError;

#[derive(Clone)]
pub struct DurableAgentWorkspaceAccess {
    threads: DurableAgentThreadBacking,
    files: Arc<dyn AgentWorkspaceStore>,
    runtime: Handle,
}

impl DurableAgentWorkspaceAccess {
    pub fn new(
        threads: DurableAgentThreadBacking,
        files: Arc<dyn AgentWorkspaceStore>,
        runtime: Handle,
    ) -> Self {
        Self {
            threads,
            files,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceRunAccessError {
    InvalidPath(String),
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
    ) -> Result<WrittenWorkspaceFileOutcome, WorkspaceRunAccessError> {
        let (binding, workspace_id) = self.live_workspace()?;
        let file = self
            .service
            .files
            .write_file(&self.tenant, workspace_id, path, bytes)
            .map_err(map_file_error)?;
        Ok(WrittenWorkspaceFileOutcome { binding, file })
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
        WorkspaceAccessError::UnsafeStorage(_) | WorkspaceAccessError::Io(_) => {
            WorkspaceRunAccessError::Unavailable
        }
    }
}
