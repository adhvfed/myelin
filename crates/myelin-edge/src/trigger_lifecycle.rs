use std::sync::Arc;

use myelin_events::UlidMinter;
use myelin_flow::{PgFlowExecutor, RunId as WorkflowRunId};
use myelin_notif::{agent_effect_approval_targets, pg_inbox::PgInboxStore};
use myelin_storage::hitl_gate_durable::DurableHitlGateBacking;
use myelin_storage::reserve_settle::RunId as CostRunId;
use myelin_storage::{
    AgentTriggerLifecycleAction, ChangeAgentTriggerLifecycleOutcome, DurableAgentTriggerBacking,
    DurableCostLedger, PgError, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

const DISABLED_RUN_REASON: &str = "automation disabled by owner";

#[derive(Clone)]
pub(crate) struct TriggerLifecycle {
    provider: SubstrateProvider,
    triggers: DurableAgentTriggerBacking,
    gates: DurableHitlGateBacking,
    costs: DurableCostLedger,
    inbox: Arc<PgInboxStore>,
    runtime: Handle,
}

impl TriggerLifecycle {
    pub(crate) fn new(
        provider: SubstrateProvider,
        triggers: DurableAgentTriggerBacking,
        inbox: Arc<PgInboxStore>,
        runtime: Handle,
    ) -> Self {
        Self {
            triggers,
            gates: DurableHitlGateBacking::new(provider.clone()),
            costs: DurableCostLedger::with_runtime(provider.clone(), runtime.clone()),
            provider,
            inbox,
            runtime,
        }
    }

    pub(crate) async fn change(
        &self,
        tenant: &TenantId,
        owner_principal_id: &str,
        binding_id: Uuid,
        action: AgentTriggerLifecycleAction,
    ) -> Result<ChangeAgentTriggerLifecycleOutcome, myelin_storage::ProviderError> {
        let tenant_id = tenant.clone();
        let tenant = tenant.0.clone();
        let region = Region(self.provider.config().region.clone());
        let owner_principal_id = owner_principal_id.to_string();
        let triggers = self.triggers.clone();
        let gates = self.gates.clone();
        let costs = self.costs.clone();
        let inbox = self.inbox.clone();
        let executor = PgFlowExecutor::new(
            self.provider.db_pool().clone(),
            self.runtime.clone(),
            Arc::new(UlidMinter::new()),
            tenant_id.clone(),
            region.clone(),
        );

        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let outcome = triggers
                        .change_lifecycle_on_conn(
                            conn,
                            &tenant,
                            &owner_principal_id,
                            binding_id,
                            action,
                        )
                        .await?;
                    let ChangeAgentTriggerLifecycleOutcome::Complete(lifecycle) = &outcome else {
                        return Ok(outcome);
                    };
                    if action != AgentTriggerLifecycleAction::Disable {
                        return Ok(outcome);
                    }

                    for run_id in &lifecycle.canceled_run_ids {
                        executor
                            .cancel_on_conn(
                                conn,
                                &WorkflowRunId(run_id.clone()),
                                DISABLED_RUN_REASON,
                            )
                            .await
                            .map_err(|error| lifecycle_error("terminate workflow", error))?;
                    }
                    let rejected_gates = gates
                        .reject_waiting_for_runs_on_conn(
                            conn,
                            &tenant,
                            &region.0,
                            &lifecycle.canceled_run_ids,
                            &owner_principal_id,
                        )
                        .await?;
                    for run_id in &lifecycle.canceled_run_ids {
                        costs
                            .stop_if_present_in_tx(conn, &tenant_id, &CostRunId(run_id.clone()))
                            .await
                            .map_err(|error| lifecycle_error("release run reservation", error))?;
                    }
                    for gate in rejected_gates {
                        for target in agent_effect_approval_targets(&tenant_id, &region, &gate) {
                            inbox
                                .complete_if_present_on_conn(conn, &target.scope, &target.item_id)
                                .await
                                .map_err(|error| {
                                    lifecycle_error("complete approval inbox item", error)
                                })?;
                        }
                    }
                    Ok(outcome)
                })
            })
            .await
    }
}

fn lifecycle_error(operation: &str, error: impl std::fmt::Display) -> PgError {
    PgError::Query(format!("{operation} for trigger lifecycle: {error}"))
}
