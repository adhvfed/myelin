use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_agent_service::workspace::AgentWorkspaceStore;
use myelin_storage::{
    AgentThreadExpiryCompletion, AgentThreadExpiryFailure, DurableAgentThreadBacking,
};

#[derive(Clone)]
pub struct AgentThreadReconciler {
    threads: DurableAgentThreadBacking,
    workspaces: Arc<dyn AgentWorkspaceStore>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentThreadReconciliationReport {
    pub made_inaccessible: usize,
    pub cleanup_candidates: usize,
    pub deleted: usize,
    pub cleanup_failures: usize,
    pub changed_before_completion: usize,
}

#[derive(Debug)]
pub struct AgentThreadReconciliationError(String);

impl core::fmt::Display for AgentThreadReconciliationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AgentThreadReconciliationError {}

impl AgentThreadReconciler {
    pub fn new(
        threads: DurableAgentThreadBacking,
        workspaces: Arc<dyn AgentWorkspaceStore>,
    ) -> Self {
        Self {
            threads,
            workspaces,
        }
    }

    pub async fn reconcile_tenant(
        &self,
        tenant: &str,
        observed_at: DateTime<Utc>,
        limit: u32,
    ) -> Result<AgentThreadReconciliationReport, AgentThreadReconciliationError> {
        let started = self
            .threads
            .start_due_expirations(tenant, observed_at, limit)
            .await
            .map_err(reconciliation_error(
                "make due private threads inaccessible",
            ))?;
        let candidates = self
            .threads
            .expirations_ready_for_cleanup(tenant, observed_at, limit)
            .await
            .map_err(reconciliation_error("load private thread cleanup work"))?;
        let mut report = AgentThreadReconciliationReport {
            made_inaccessible: started.len(),
            cleanup_candidates: candidates.len(),
            ..AgentThreadReconciliationReport::default()
        };

        for work in candidates {
            let workspaces = self.workspaces.clone();
            let deletion_tenant = tenant.to_string();
            let deletion_work = work.clone();
            let deletion = tokio::task::spawn_blocking(move || {
                workspaces.delete_workspace(
                    &deletion_tenant,
                    deletion_work.workspace_id,
                    deletion_work.storage_locator.as_deref(),
                )
            })
            .await;
            if !matches!(deletion, Ok(Ok(_))) {
                self.threads
                    .record_expiration_failure(
                        tenant,
                        &work,
                        AgentThreadExpiryFailure::WorkspaceCleanupFailed,
                        observed_at,
                    )
                    .await
                    .map_err(reconciliation_error(
                        "record private thread cleanup failure",
                    ))?;
                report.cleanup_failures += 1;
                continue;
            }

            match self
                .threads
                .complete_expiration(tenant, &work, observed_at)
                .await
                .map_err(reconciliation_error("complete private thread cleanup"))?
            {
                AgentThreadExpiryCompletion::Deleted
                | AgentThreadExpiryCompletion::AlreadyDeleted => report.deleted += 1,
                AgentThreadExpiryCompletion::Changed => report.changed_before_completion += 1,
            }
        }

        Ok(report)
    }
}

fn reconciliation_error(
    operation: &'static str,
) -> impl FnOnce(myelin_storage::ProviderError) -> AgentThreadReconciliationError {
    move |error| AgentThreadReconciliationError(format!("{operation}: {error}"))
}
