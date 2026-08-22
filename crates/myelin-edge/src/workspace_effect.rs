use std::sync::Arc;

use myelin_agent::{
    EffectApi, EffectAuthority, EffectResource, EffectResult, EventId, ProposedEffect, RunCtx,
};
use myelin_agent_service::workspace_tools::workspace_tool_defs;
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::TenantScope;

use crate::effect_carrier::parse_proposed;
use crate::workspace_access::{WorkspaceRunAccess, WorkspaceRunAccessError};

pub struct WorkspaceEffectApi {
    access: WorkspaceRunAccess,
    principal: Principal,
    authority: Arc<RunTokenAuthorizer>,
}

impl WorkspaceEffectApi {
    pub fn new(
        access: WorkspaceRunAccess,
        principal: Principal,
        authority: Arc<RunTokenAuthorizer>,
    ) -> Self {
        Self {
            access,
            principal,
            authority,
        }
    }

    fn authorize(&self, authority: &EffectAuthority, proposed_tool: &str) -> Result<(), String> {
        if authority.tool != proposed_tool || authority.principal_id != self.principal.principal_id
        {
            return Err("workspace effect does not match its signed run authority".into());
        }
        let definition = workspace_tool_defs()
            .into_iter()
            .find(|definition| definition.canonical_name() == proposed_tool)
            .ok_or_else(|| format!("unknown workspace tool `{proposed_tool}`"))?;
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
}

impl EffectApi for WorkspaceEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "workspace mutation requires signed run-token authority; direct apply is denied".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((tool, arguments)) = parse_proposed(&effect.0) else {
            return EffectResult::Denied("malformed proposed workspace effect".into());
        };
        if let Err(reason) = self.authorize(authority, &tool) {
            return EffectResult::Denied(reason);
        }
        if tool != "workspace.write_file" {
            return EffectResult::Denied(format!("workspace tool `{tool}` is not a mutation"));
        }
        let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
            return EffectResult::Denied("workspace write requires `path`".into());
        };
        let Some(content) = arguments.get("content").and_then(serde_json::Value::as_str) else {
            return EffectResult::Denied("workspace write requires `content`".into());
        };
        match self.access.write_file(path, content.as_bytes()) {
            Ok(outcome) => {
                let event_id = workspace_write_event_id(
                    &outcome.binding.thread_id,
                    &authority.idempotency_key,
                    &outcome.file.path,
                    &outcome.file.content_digest,
                    run,
                );
                EffectResult::AppliedResource {
                    event_id: EventId(event_id),
                    resource: EffectResource::new(
                        format!(
                            "myelin://{}/agent/workspace/{}",
                            self.principal.tenant.0, outcome.binding.workspace_id
                        ),
                        serde_json::json!({
                            "path": outcome.file.path,
                            "byte_len": outcome.file.byte_len,
                            "content_digest": outcome.file.content_digest,
                            "workspace_generation": outcome.binding.workspace_generation,
                        }),
                    ),
                }
            }
            Err(error) => EffectResult::Denied(public_error(error)),
        }
    }
}

fn workspace_write_event_id(
    thread_id: &str,
    idempotency_key: &str,
    path: &str,
    content_digest: &str,
    run: &RunCtx,
) -> String {
    let mut digest = blake3::Hasher::new();
    for part in [thread_id, idempotency_key, path, content_digest, &run.0] {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("workspace.file.write:{}", digest.finalize().to_hex())
}

fn public_error(error: WorkspaceRunAccessError) -> String {
    match error {
        WorkspaceRunAccessError::InvalidPath(reason) => reason,
        WorkspaceRunAccessError::NotFound => {
            "workspace or file is unavailable to this agent run".into()
        }
        WorkspaceRunAccessError::TooLarge => "workspace file exceeds the interactive limit".into(),
        WorkspaceRunAccessError::Unavailable => "workspace storage is unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_identity_is_retry_stable_bound_and_opaque() {
        let run = RunCtx("runtok:sensitive-jti|principal:agent-1|tool:workspace.write_file".into());
        let event = workspace_write_event_id(
            "thread-sensitive",
            "retry-sensitive",
            "notes/continuity.md",
            "blake3:content-sensitive",
            &run,
        );
        assert_eq!(
            event,
            workspace_write_event_id(
                "thread-sensitive",
                "retry-sensitive",
                "notes/continuity.md",
                "blake3:content-sensitive",
                &run,
            )
        );
        assert_ne!(
            event,
            workspace_write_event_id(
                "thread-sensitive",
                "another-retry",
                "notes/continuity.md",
                "blake3:content-sensitive",
                &run,
            )
        );
        for secret in ["sensitive-jti", "thread-sensitive", "retry-sensitive"] {
            assert!(!event.contains(secret));
        }
    }

    #[test]
    fn filesystem_failures_cross_the_mcp_boundary_as_public_categories() {
        assert_eq!(
            public_error(WorkspaceRunAccessError::Unavailable),
            "workspace storage is unavailable"
        );
        assert_eq!(
            public_error(WorkspaceRunAccessError::TooLarge),
            "workspace file exceeds the interactive limit"
        );
    }
}
