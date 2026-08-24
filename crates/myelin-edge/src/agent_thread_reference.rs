use myelin_identity::{Principal, PrincipalStatus};
use myelin_storage::{DurableAgentThread, DurableAgentThreadBacking};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::runtime::drive_result_on_runtime;

#[derive(Clone)]
pub struct DurableAgentThreadReferenceApi {
    threads: DurableAgentThreadBacking,
    runtime: Handle,
}

impl DurableAgentThreadReferenceApi {
    pub fn new(threads: DurableAgentThreadBacking, runtime: Handle) -> Self {
        Self { threads, runtime }
    }

    pub(crate) fn project_threads(
        &self,
        principal: &Principal,
        thread_ids: &[Uuid],
    ) -> Result<Vec<DurableAgentThread>, AgentThreadReferenceError> {
        if principal.status != PrincipalStatus::Active {
            return Ok(Vec::new());
        }
        drive_result_on_runtime(
            &self.runtime,
            async {
                self.threads
                    .get_live_exact_for_owner(
                        principal.tenant.as_str(),
                        principal.principal_id.0.as_str(),
                        thread_ids,
                    )
                    .await
                    .map_err(|_| AgentThreadReferenceError::Storage)
            },
            AgentThreadReferenceError::Runtime,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentThreadReferenceError {
    Storage,
    Runtime,
}
