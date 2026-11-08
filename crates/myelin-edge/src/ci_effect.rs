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
use crate::DurableCiReadApi;

pub struct CiEffectApi {
    ci: DurableCiReadApi,
    principal: Principal,
    delegator: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunMutationRequest {
    run_id: String,
}

impl CiEffectApi {
    pub fn new(
        ci: DurableCiReadApi,
        principal: Principal,
        delegator: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> Self {
        Self {
            ci,
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
                "run-token authority principal does not match the CI adapter principal".into(),
            );
        }
        if !is_active_delegation(&self.principal, &self.delegator) {
            return Err("CI effect delegation is not an active human relationship".into());
        }
        let local_tool = proposed_tool
            .strip_prefix("ci.")
            .ok_or_else(|| format!("unknown CI tool `{proposed_tool}`"))?;
        let definition = myelin_ci_controlplane::ci_tool_def(local_tool)
            .filter(|definition| definition.canonical_name() == proposed_tool)
            .ok_or_else(|| format!("unknown CI tool `{proposed_tool}`"))?;
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
        _run: &RunCtx,
        proposed_tool: &str,
        arguments: serde_json::Value,
        idempotency_key: &str,
    ) -> EffectResult {
        let request: RunMutationRequest = match serde_json::from_value(arguments) {
            Ok(request) => request,
            Err(error) => {
                return EffectResult::Denied(format!("invalid CI run mutation arguments: {error}"))
            }
        };
        match proposed_tool {
            "ci.retry_run" => match self.ci.retry_run(
                &self.principal,
                &self.delegator,
                &request.run_id,
                idempotency_key,
            ) {
                Ok(outcome) => EffectResult::AppliedResource {
                    event_id: EventId(outcome.event_id),
                    resource: EffectResource::new(
                        format!(
                            "myelin://{}/ci/run/{}",
                            self.principal.tenant.0, outcome.run_id
                        ),
                        serde_json::json!({
                            "run_id": outcome.run_id,
                            "source_run_id": outcome.source_run_id,
                            "workflow_run_id": outcome.workflow_run_id,
                        }),
                    ),
                },
                Err(error) => EffectResult::Denied(error.client_message()),
            },
            "ci.cancel_run" => match self.ci.cancel_run(
                &self.principal,
                &self.delegator,
                &request.run_id,
                idempotency_key,
            ) {
                Ok(outcome) => EffectResult::AppliedResource {
                    event_id: EventId(outcome.event_id),
                    resource: EffectResource::new(
                        format!(
                            "myelin://{}/ci/run/{}",
                            self.principal.tenant.0, outcome.run_id
                        ),
                        serde_json::json!({
                            "run_id": outcome.run_id,
                            "workflow_run_id": outcome.workflow_run_id,
                            "state": "cancelled",
                        }),
                    ),
                },
                Err(error) => EffectResult::Denied(error.client_message()),
            },
            tool => EffectResult::Denied(format!("unknown CI mutation `{tool}`")),
        }
    }
}

impl EffectApi for CiEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "CI mutation requires signed run-token authority; direct apply is denied".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((tool, arguments)) = parse_proposed(&effect.0) else {
            return EffectResult::Denied("malformed proposed CI effect".into());
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
    fn retry_arguments_are_exact_and_canonical_at_the_carrier() {
        let (tool, arguments) = parse_proposed(
            r#"tool:ci.retry_run|args:{"run_id":"11111111-1111-1111-1111-111111111111"}"#,
        )
        .unwrap();
        assert_eq!(tool, "ci.retry_run");
        let request: RunMutationRequest = serde_json::from_value(arguments).unwrap();
        assert_eq!(request.run_id, "11111111-1111-1111-1111-111111111111");
        let (_, extra) = parse_proposed(
            r#"tool:ci.retry_run|args:{"run_id":"11111111-1111-1111-1111-111111111111","force":true}"#,
        )
        .unwrap();
        assert!(serde_json::from_value::<RunMutationRequest>(extra).is_err());
    }
}
