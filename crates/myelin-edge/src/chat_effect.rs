use std::sync::Arc;

use myelin_agent::{
    EffectApi, EffectAuthority, EffectResource, EffectResult, EventId, ProposedEffect, RunCtx,
};
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;
use serde_json::Value;

use crate::agent_delegation::is_active_delegation;
use crate::effect_carrier::parse_proposed;
use crate::DurableChatMutationApi;

pub struct ChatEffectApi {
    chat: DurableChatMutationApi,
    principal: Principal,
    delegator: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

impl ChatEffectApi {
    pub fn new(
        chat: DurableChatMutationApi,
        principal: Principal,
        delegator: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> Self {
        Self {
            chat,
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
                "run-token authority principal does not match the Chat adapter principal".into(),
            );
        }
        if !is_active_delegation(&self.principal, &self.delegator) {
            return Err("Chat effect delegation is not an active human relationship".into());
        }
        let definition = myelin_chat::tools::chat_tool_defs()
            .into_iter()
            .find(|definition| definition.canonical_name() == proposed_tool)
            .ok_or_else(|| format!("unknown Chat tool `{proposed_tool}`"))?;
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
        arguments: &Value,
        idempotency_key: &str,
    ) -> EffectResult {
        match tool {
            "chat.post" => {
                let Some(conversation_id) = string_argument(arguments, "conversation_id") else {
                    return missing("conversation_id");
                };
                let Some(content) = string_argument(arguments, "content") else {
                    return missing("content");
                };
                let references = match string_array_argument(arguments, "references") {
                    Ok(references) => references,
                    Err(reason) => return EffectResult::Denied(reason),
                };
                let nonce = mutation_nonce(
                    &self.principal.tenant.0,
                    &self.principal.principal_id.0,
                    idempotency_key,
                );
                match self.chat.post_message(
                    &self.principal,
                    &self.delegator,
                    conversation_id,
                    content,
                    &references,
                    nonce,
                ) {
                    Ok(message_id) => EffectResult::AppliedResource {
                        event_id: EventId(format!(
                            "chat.message.post:{}|{}",
                            message_id.as_str(),
                            run.0
                        )),
                        resource: EffectResource::new(
                            format!(
                                "myelin://{}/chat/message/{}",
                                self.principal.tenant.0,
                                message_id.as_str()
                            ),
                            serde_json::json!({
                                "id": message_id.as_str(),
                                "conversation_id": conversation_id,
                            }),
                        ),
                    },
                    Err(error) => EffectResult::Denied(error.client_message()),
                }
            }
            other => EffectResult::Denied(format!(
                "Chat tool `{other}` is registered but not yet wired through ChatEffectApi"
            )),
        }
    }
}

impl EffectApi for ChatEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "Chat mutation requires signed run-token authority; direct apply is denied".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((tool, arguments)) = parse_proposed(&effect.0) else {
            return EffectResult::Denied("malformed proposed Chat effect".into());
        };
        match self.authorize(authority, &tool) {
            Ok(()) => self.apply_tool(run, &tool, &arguments, &authority.idempotency_key),
            Err(reason) => EffectResult::Denied(reason),
        }
    }
}

fn string_argument<'a>(arguments: &'a Value, field: &str) -> Option<&'a str> {
    arguments.get(field).and_then(Value::as_str)
}

fn string_array_argument(arguments: &Value, field: &str) -> Result<Vec<String>, String> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| format!("Chat tool argument `{field}` must be an array"))?
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_string).ok_or_else(|| {
                        format!("Chat tool argument `{field}` must contain only strings")
                    })
                })
                .collect()
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn missing(field: &str) -> EffectResult {
    EffectResult::Denied(format!("Chat tool argument `{field}` is required"))
}

fn mutation_nonce(tenant: &str, principal: &str, idempotency_key: &str) -> String {
    let mut digest = blake3::Hasher::new();
    for part in [
        b"myelin.chat.mcp-effect.v1".as_slice(),
        tenant.as_bytes(),
        principal.as_bytes(),
        idempotency_key.as_bytes(),
    ] {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    format!("mcp-v1-{}", digest.finalize().to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_nonces_are_stable_scoped_and_reveal_no_caller_material() {
        let nonce = mutation_nonce("acme", "agent:a", "private-retry-key");
        assert_eq!(
            nonce,
            mutation_nonce("acme", "agent:a", "private-retry-key")
        );
        assert_ne!(nonce, mutation_nonce("acme", "agent:a", "other-key"));
        assert_ne!(
            nonce,
            mutation_nonce("other", "agent:a", "private-retry-key")
        );
        assert!(!nonce.contains("private"));
        assert!(nonce.len() <= 128);
    }

    #[test]
    fn proposed_chat_effects_are_strict_json_carriers() {
        let (tool, arguments) =
            parse_proposed(r#"tool:chat.post|args:{"conversation_id":"01A","content":"ready"}"#)
                .unwrap();
        assert_eq!(tool, "chat.post");
        assert_eq!(arguments["content"], "ready");
        assert!(parse_proposed("garbage").is_none());
        assert!(parse_proposed("tool:chat.post|args:not-json").is_none());
    }
}
