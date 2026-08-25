use std::sync::Arc;

use myelin_storage::{
    AgentTraceSubjectEraseReceipt, AgentTraceSubjectState, AgentTraceSubjectSummary,
    DurableAgentTraceStore, DurablePrivacyRequestStore,
};
use serde_json::json;
use tokio::runtime::Handle;

use crate::catalogue::{Handler, HandlerCtx};
use crate::gateway::GatewayBuilder;
use crate::request::{require_empty_json_object, EdgeResponse};
use crate::runtime::drive_edge_future;
use crate::{EdgeError, Method};

const MAX_PRIVACY_JSON_BYTES: usize = 1024;

mod request;

#[derive(Clone)]
pub(super) struct PrivacyHttpApi {
    traces: DurableAgentTraceStore,
    requests: DurablePrivacyRequestStore,
    runtime: Handle,
}

impl PrivacyHttpApi {
    fn summarize(
        &self,
        tenant: &str,
        subject: &str,
    ) -> Result<AgentTraceSubjectSummary, EdgeError> {
        drive_edge_future(
            &self.runtime,
            self.traces.summarize_subject(tenant, subject),
            "privacy HTTP",
        )?
        .map_err(|error| EdgeError::Internal(error.to_string()))
    }

    fn erase(
        &self,
        tenant: &str,
        subject: &str,
    ) -> Result<AgentTraceSubjectEraseReceipt, EdgeError> {
        drive_edge_future(
            &self.runtime,
            self.traces.erase_for_subject(tenant, subject),
            "privacy HTTP",
        )?
        .map_err(|error| EdgeError::Internal(error.to_string()))
    }

    pub(super) fn drive<F, T, E>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        drive_edge_future(&self.runtime, future, "privacy HTTP")?
            .map_err(|error| EdgeError::Internal(error.to_string()))
    }
}

struct AgentDataStatusHandler {
    api: PrivacyHttpApi,
}

impl Handler for AgentDataStatusHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_no_parameters_or_body(ctx, "agent-data privacy status")?;
        let summary = self
            .api
            .summarize(&ctx.principal.tenant.0, &ctx.principal.principal_id.0)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &agent_data_summary_json(summary),
        )))
    }
}

struct AgentDataEraseHandler {
    api: PrivacyHttpApi,
}

impl Handler for AgentDataEraseHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "agent-data erasure accepts no query parameters".into(),
            ));
        }
        require_empty_json_object(
            &ctx.request.body,
            "agent-data erasure",
            MAX_PRIVACY_JSON_BYTES,
        )?;
        let receipt = self
            .api
            .erase(&ctx.principal.tenant.0, &ctx.principal.principal_id.0)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &agent_data_erasure_json(receipt)?,
        )))
    }
}

pub fn register_privacy(
    builder: GatewayBuilder,
    traces: DurableAgentTraceStore,
    requests: DurablePrivacyRequestStore,
    runtime: Handle,
) -> GatewayBuilder {
    let api = PrivacyHttpApi {
        traces,
        requests,
        runtime,
    };
    let builder = builder
        .route(
            Method::Get,
            "/v1/privacy/me/agent-data",
            "privacy.agent_data.read",
            Arc::new(AgentDataStatusHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/privacy/me/agent-data/erase",
            "privacy.agent_data.erase",
            Arc::new(AgentDataEraseHandler { api: api.clone() }),
        );
    request::register(builder, api)
}

fn require_no_parameters_or_body(ctx: &HandlerCtx<'_>, operation: &str) -> Result<(), EdgeError> {
    if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
        return Err(EdgeError::BadRequest(format!(
            "{operation} accepts no query parameters or request body"
        )));
    }
    Ok(())
}

fn agent_data_summary_json(summary: AgentTraceSubjectSummary) -> serde_json::Value {
    let processing_allowed = summary.state == AgentTraceSubjectState::Active;
    json!({
        "agent_data": {
            "subject": "self",
            "scope": "agent_data",
            "state": summary.state.token(),
            "recoverable_records": summary.recoverable_records,
            "holders": ["agent_traces", "model_replay", "tool_effects"],
            "new_processing_allowed": processing_allowed,
            "erasure_is_irreversible": true,
        }
    })
}

fn agent_data_erasure_json(
    receipt: AgentTraceSubjectEraseReceipt,
) -> Result<serde_json::Value, EdgeError> {
    let records_erased = receipt
        .traces_erased
        .checked_add(receipt.model_steps_erased)
        .and_then(|total| total.checked_add(receipt.tool_effects_erased))
        .ok_or_else(|| EdgeError::Internal("agent-data erasure count overflowed".into()))?;
    Ok(json!({
        "erasure": {
            "subject": "self",
            "scope": "agent_data",
            "erased": true,
            "already_erased": receipt.already_erased,
            "records_erased": records_erased,
            "traces_erased": receipt.traces_erased,
            "model_steps_erased": receipt.model_steps_erased,
            "tool_effects_erased": receipt.tool_effects_erased,
            "key_destroyed_this_request": receipt.key_destroyed,
            "key_unrecoverable": receipt.key_unrecoverable,
            "new_processing_blocked": true,
            "irreversible": true,
        }
    }))
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_discloses_scope_and_permanent_erasure_semantics() {
        let body = agent_data_summary_json(AgentTraceSubjectSummary {
            state: AgentTraceSubjectState::Active,
            recoverable_records: 3,
        });
        assert_eq!(body["agent_data"]["subject"], "self");
        assert_eq!(body["agent_data"]["scope"], "agent_data");
        assert_eq!(body["agent_data"]["recoverable_records"], 3);
        assert_eq!(body["agent_data"]["new_processing_allowed"], true);
        assert_eq!(body["agent_data"]["erasure_is_irreversible"], true);
    }

    #[test]
    fn status_makes_an_interrupted_erasure_resumable() {
        let body = agent_data_summary_json(AgentTraceSubjectSummary {
            state: AgentTraceSubjectState::Erasing,
            recoverable_records: 2,
        });
        assert_eq!(body["agent_data"]["state"], "erasing");
        assert_eq!(body["agent_data"]["recoverable_records"], 2);
        assert_eq!(body["agent_data"]["new_processing_allowed"], false);
    }

    #[test]
    fn erasure_receipt_totals_every_agent_data_holder() {
        let body = agent_data_erasure_json(AgentTraceSubjectEraseReceipt {
            traces_erased: 2,
            model_steps_erased: 3,
            tool_effects_erased: 5,
            already_erased: false,
            key_destroyed: true,
            key_unrecoverable: true,
        })
        .unwrap();
        assert_eq!(body["erasure"]["records_erased"], 10);
        assert_eq!(body["erasure"]["new_processing_blocked"], true);
        assert_eq!(body["erasure"]["irreversible"], true);
    }
}
