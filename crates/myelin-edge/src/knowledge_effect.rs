use std::sync::Arc;

use myelin_agent::{
    EffectApi, EffectAuthority, EffectResource, EffectResult, EventId, ProposedEffect, RunCtx,
};
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;
use serde::Deserialize;

use crate::agent_delegation::is_active_delegation;
use crate::effect_carrier::parse_proposed;
use crate::{DurableKnowledgeMutationApi, KnowledgeLinkRequest};

pub struct KnowledgeEffectApi {
    knowledge: DurableKnowledgeMutationApi,
    principal: Principal,
    delegator: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeLinkArguments {
    page_id: String,
    reference: String,
    #[serde(default)]
    note: Option<String>,
}

impl KnowledgeEffectApi {
    pub fn new(
        knowledge: DurableKnowledgeMutationApi,
        principal: Principal,
        delegator: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> Self {
        Self {
            knowledge,
            principal,
            delegator,
            authority,
        }
    }

    fn authorize(&self, authority: &EffectAuthority, proposed_tool: &str) -> Result<(), String> {
        if authority.tool != proposed_tool {
            return Err(format!(
                "run-token authority is bound to `{}`, not proposed tool `{proposed_tool}`",
                authority.tool
            ));
        }
        if authority.principal_id != self.principal.principal_id {
            return Err(
                "run-token authority principal does not match the Knowledge adapter principal"
                    .into(),
            );
        }
        if !is_active_delegation(&self.principal, &self.delegator) {
            return Err("Knowledge effect delegation is not an active human relationship".into());
        }
        let definition = myelin_agent_service::link_work_tool_def();
        if definition.canonical_name() != proposed_tool || !definition.exposed_over_mcp {
            return Err(format!("unknown Knowledge tool `{proposed_tool}`"));
        }
        let scope =
            TenantScope::from_verified_token(&self.principal, self.principal.region.clone());
        self.authority
            .authorize(
                &scope,
                &self.principal.principal_id,
                &authority.run_token,
                &definition.required_caps,
            )
            .map(|_| ())
    }

    fn apply_tool(
        &self,
        run: &RunCtx,
        tool: &str,
        arguments: serde_json::Value,
        idempotency_key: &str,
    ) -> EffectResult {
        if tool != "knowledge.link_work" {
            return EffectResult::Denied(format!(
                "Knowledge tool `{tool}` is registered but not wired through KnowledgeEffectApi"
            ));
        }
        let arguments: KnowledgeLinkArguments = match serde_json::from_value(arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                return EffectResult::Denied(format!(
                    "invalid knowledge.link_work arguments: {error}"
                ))
            }
        };
        let request = KnowledgeLinkRequest {
            reference: arguments.reference,
            note: arguments.note,
        };
        match self.knowledge.link_work(
            &self.principal,
            &self.delegator,
            &arguments.page_id,
            request,
            idempotency_key,
        ) {
            Ok(outcome) => EffectResult::AppliedResource {
                event_id: EventId(format!("knowledge.link:{}|{}", outcome.block_id, run.0)),
                resource: EffectResource::new(
                    outcome.block_ref,
                    serde_json::json!({
                        "page_id": outcome.page_id,
                        "page_ref": outcome.page_ref,
                        "block_id": outcome.block_id,
                        "version": outcome.version,
                        "linked": outcome.created,
                    }),
                ),
            },
            Err(error) => EffectResult::Denied(error.client_message()),
        }
    }
}

impl EffectApi for KnowledgeEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "Knowledge mutation requires signed run-token authority; direct apply is denied".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((tool, arguments)) = parse_proposed(&effect.0) else {
            return EffectResult::Denied("malformed proposed Knowledge effect".into());
        };
        match self.authorize(authority, &tool) {
            Ok(()) => self.apply_tool(run, &tool, arguments, &authority.idempotency_key),
            Err(reason) => EffectResult::Denied(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_knowledge_links_are_strict_json_carriers() {
        let (tool, arguments) = parse_proposed(
            r#"tool:knowledge.link_work|args:{"page_id":"01J00000000000000000000000","reference":"myelin://acme/issue/issue/ENG-41","note":"Delivery issue"}"#,
        )
        .unwrap();
        assert_eq!(tool, "knowledge.link_work");
        let arguments: KnowledgeLinkArguments = serde_json::from_value(arguments).unwrap();
        assert_eq!(arguments.note.as_deref(), Some("Delivery issue"));
        assert!(parse_proposed("garbage").is_none());
        assert!(parse_proposed("tool:knowledge.link_work|args:not-json").is_none());
    }
}
