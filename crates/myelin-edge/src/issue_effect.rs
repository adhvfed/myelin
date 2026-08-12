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
use crate::{DurableIssueMutationApi, IssueCreateRequest};

pub struct IssueEffectApi {
    issues: DurableIssueMutationApi,
    principal: Principal,
    delegator: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueCloseRequest {
    issue_ref: String,
}

impl IssueEffectApi {
    pub fn new(
        issues: DurableIssueMutationApi,
        principal: Principal,
        delegator: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> Self {
        Self {
            issues,
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
                "run-token authority principal does not match the Issues adapter principal".into(),
            );
        }
        if !is_active_delegation(&self.principal, &self.delegator) {
            return Err("Issues effect delegation is not an active human relationship".into());
        }
        let definition = match proposed_tool {
            "issues.create" => myelin_agent_service::create_tool_def(),
            "issues.close" => myelin_agent_service::close_tool_def(),
            _ => return Err(format!("unknown Issues tool `{proposed_tool}`")),
        };
        if definition.canonical_name() != proposed_tool || !definition.exposed_over_mcp {
            return Err(format!("unknown Issues tool `{proposed_tool}`"));
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
        match tool {
            "issues.create" => {
                let request: IssueCreateRequest = match serde_json::from_value(arguments) {
                    Ok(request) => request,
                    Err(error) => {
                        return EffectResult::Denied(format!(
                            "invalid issues.create arguments: {error}"
                        ))
                    }
                };
                match self.issues.create_issue(
                    &self.principal,
                    &self.delegator,
                    request,
                    idempotency_key,
                ) {
                    Ok(outcome) => {
                        let receipt = outcome.receipt;
                        EffectResult::AppliedResource {
                            event_id: EventId(format!("issue.create:{}|{}", receipt.id, run.0)),
                            resource: EffectResource::new(
                                format!(
                                    "myelin://{}/issue/issue/{}",
                                    self.principal.tenant.0, receipt.key
                                ),
                                serde_json::json!({
                                    "id": receipt.id,
                                    "key": receipt.key,
                                    "project_id": receipt.project_id,
                                }),
                            ),
                        }
                    }
                    Err(error) => EffectResult::Denied(error.client_message()),
                }
            }
            "issues.close" => {
                let request: IssueCloseRequest = match serde_json::from_value(arguments) {
                    Ok(request) => request,
                    Err(error) => {
                        return EffectResult::Denied(format!(
                            "invalid issues.close arguments: {error}"
                        ))
                    }
                };
                match self.issues.close_issue_ref(
                    &self.principal,
                    &self.delegator,
                    &request.issue_ref,
                ) {
                    Ok(issue) => EffectResult::AppliedResource {
                        event_id: EventId(format!("issue.close:{}|{}", issue.id, run.0)),
                        resource: EffectResource::new(
                            format!(
                                "myelin://{}/issue/issue/{}",
                                self.principal.tenant.0, issue.key
                            ),
                            serde_json::json!({
                                "id": issue.id,
                                "key": issue.key,
                                "state": issue.state,
                                "state_category": issue.state_category,
                                "version": issue.version,
                            }),
                        ),
                    },
                    Err(error) => EffectResult::Denied(error.client_message()),
                }
            }
            _ => EffectResult::Denied(format!("unknown Issues tool `{tool}`")),
        }
    }
}

impl EffectApi for IssueEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "Issues mutation requires signed run-token authority; direct apply is denied".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((tool, arguments)) = parse_proposed(&effect.0) else {
            return EffectResult::Denied("malformed proposed Issues effect".into());
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
    fn proposed_issue_effects_are_strict_project_native_json_carriers() {
        let (tool, arguments) = parse_proposed(
            r#"tool:issues.create|args:{"project_id":"11111111-1111-1111-1111-111111111111","title":"CI is red"}"#,
        )
        .unwrap();
        assert_eq!(tool, "issues.create");
        let request: IssueCreateRequest = serde_json::from_value(arguments).unwrap();
        assert_eq!(request.title, "CI is red");
        assert!(request.type_id.is_none());
        assert!(parse_proposed("garbage").is_none());
        assert!(parse_proposed("tool:issues.create|args:not-json").is_none());

        let (_, arguments) = parse_proposed(
            r#"tool:issues.create|args:{"project_id":"11111111-1111-1111-1111-111111111111","title":"CI is red","tenant":"other"}"#,
        )
        .unwrap();
        assert!(serde_json::from_value::<IssueCreateRequest>(arguments).is_err());

        let (_, close_arguments) = parse_proposed(
            r#"tool:issues.close|args:{"issue_ref":"myelin://acme/issue/issue/ENG-41"}"#,
        )
        .unwrap();
        let close: IssueCloseRequest = serde_json::from_value(close_arguments).unwrap();
        assert_eq!(close.issue_ref, "myelin://acme/issue/issue/ENG-41");
        let (_, ambiguous_close) = parse_proposed(
            r#"tool:issues.close|args:{"issue_ref":"myelin://acme/issue/issue/ENG-41","issue_id":"33333333-3333-3333-3333-333333333333"}"#,
        )
        .unwrap();
        assert!(serde_json::from_value::<IssueCloseRequest>(ambiguous_close).is_err());
    }
}
