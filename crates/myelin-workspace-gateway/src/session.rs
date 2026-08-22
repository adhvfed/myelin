use myelin_agent_service::workspace::LocalDevelopmentWorkspaceProvisioner;
use myelin_ci_sandbox::gvisor::{
    ConfinedWorkspaceSession, WorkspaceSessionCommand, WorkspaceSessionLimits, WorkspaceTerminal,
};
use uuid::Uuid;

use crate::AuthenticatedWorkspace;

#[derive(Clone)]
pub struct LocalConfinedWorkspaceLauncher {
    workspaces: LocalDevelopmentWorkspaceProvisioner,
    limits: WorkspaceSessionLimits,
}

impl LocalConfinedWorkspaceLauncher {
    pub fn new(workspaces: LocalDevelopmentWorkspaceProvisioner) -> Self {
        Self {
            workspaces,
            limits: WorkspaceSessionLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: WorkspaceSessionLimits) -> Self {
        self.limits = limits;
        self
    }

    pub async fn launch(
        &self,
        authenticated: &AuthenticatedWorkspace,
        command: WorkspaceSessionCommand,
        terminal: Option<WorkspaceTerminal>,
    ) -> Result<ConfinedWorkspaceSession, WorkspaceLaunchError> {
        let tenant = authenticated.tenant.clone();
        let workspace_id = Uuid::parse_str(&authenticated.admission.workspace_id)
            .map_err(|_| WorkspaceLaunchError::InvalidAdmission)?;
        let locator = authenticated.admission.storage_locator.clone();
        let workspaces = self.workspaces.clone();
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            let workspace = workspaces
                .open_verified_directory(&tenant, workspace_id, &locator)
                .map_err(|_| WorkspaceLaunchError::WorkspaceUnavailable)?;
            ConfinedWorkspaceSession::launch(&workspace, command, limits, terminal)
                .map_err(|_| WorkspaceLaunchError::ConfinementUnavailable)
        })
        .await
        .map_err(|_| WorkspaceLaunchError::LaunchWorkerTerminated)?
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceLaunchError {
    InvalidAdmission,
    WorkspaceUnavailable,
    ConfinementUnavailable,
    LaunchWorkerTerminated,
}

impl core::fmt::Display for WorkspaceLaunchError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidAdmission => "workspace admission is malformed",
            Self::WorkspaceUnavailable => "the admitted workspace is unavailable",
            Self::ConfinementUnavailable => "workspace confinement is unavailable",
            Self::LaunchWorkerTerminated => "workspace launch worker terminated",
        })
    }
}

impl std::error::Error for WorkspaceLaunchError {}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use myelin_storage::agent_thread_durable::LiveWorkspaceSshAdmission;

    use super::*;

    fn durable_test_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("myelin-workspace-gateway-")
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    #[tokio::test]
    async fn a_database_locator_is_not_interpreted_as_a_host_path() {
        let temporary = durable_test_root();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let launcher = LocalConfinedWorkspaceLauncher::new(
            LocalDevelopmentWorkspaceProvisioner::open(temporary.path()).unwrap(),
        );
        let authenticated = AuthenticatedWorkspace::from_admission(
            "acme".into(),
            LiveWorkspaceSshAdmission {
                grant_id: Uuid::from_u128(1).to_string(),
                thread_id: Uuid::from_u128(2).to_string(),
                owner_principal_id: "user:alice".into(),
                workspace_id: Uuid::from_u128(3).to_string(),
                workspace_generation: 1,
                storage_locator: "/etc".into(),
                expires_at: "2026-08-22T12:05:00Z".into(),
            },
        );

        assert!(matches!(
            launcher
                .launch(&authenticated, WorkspaceSessionCommand::Shell, None)
                .await,
            Err(WorkspaceLaunchError::WorkspaceUnavailable)
        ));
    }
}
