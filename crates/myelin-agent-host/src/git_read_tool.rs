use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use myelin_agent::{EffectKind, ToolCall, ToolDef, ToolName, ToolResult, ToolSchema};
use myelin_agent_service::{ToolExecError, ToolExecutionContext, ToolExecutor};
use myelin_git::check_status::GitOid;
use myelin_git::check_status_store::PgCheckStatusProjection;
use myelin_storage::{SubstrateProvider, TenantScope};

pub const GIT_READ_CHECK_STATUS_TOOL: &str = "git.read_check_status";
const GIT_SUBSYSTEM: &str = "git";
const GIT_READ_TOOL_VERSION: u32 = 1;

const GIT_READ_CHECK_STATUS_SCHEMA: &str = r#"{"type":"object","required":["repo","commit"],"properties":{"repo":{"type":"string"},"commit":{"type":"string"}}}"#;

pub fn git_check_status_read_tool_def() -> ToolDef {
    ToolDef {
        name: ToolName(GIT_READ_CHECK_STATUS_TOOL.to_string()),
        subsystem: GIT_SUBSYSTEM.to_string(),
        version: GIT_READ_TOOL_VERSION,
        input_schema: GIT_READ_CHECK_STATUS_SCHEMA.to_string(),
        required_caps: vec!["pull".to_string()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

pub fn git_check_status_read_tool_schema() -> ToolSchema {
    ToolSchema {
        name: ToolName(GIT_READ_CHECK_STATUS_TOOL.to_string()),
        description: "Read the current CI/external check-status rows Git recorded for a specific \
                      (repo, commit): each check context (e.g. ci/build, ci/test) and its state \
                      (success, failure, error, queued, in_progress, neutral, cancelled). Arguments: \
                      `repo` (the repo ref, e.g. myelin://<tenant>/git/repo/<id>) and `commit` (the \
                      commit OID). The read is scoped to your own tenant; you cannot read another \
                      tenant's data."
            .to_string(),
        input_schema: GIT_READ_CHECK_STATUS_SCHEMA.to_string(),
    }
}

pub struct GitCheckStatusReadExecutor {
    projection: PgCheckStatusProjection,
    scope: TenantScope,
    invocations: AtomicUsize,
    last_result: Mutex<Option<String>>,
}

impl GitCheckStatusReadExecutor {
    pub fn new(
        provider: SubstrateProvider,
        admission_provider: SubstrateProvider,
        runtime: tokio::runtime::Handle,
        scope: TenantScope,
    ) -> GitCheckStatusReadExecutor {
        GitCheckStatusReadExecutor {
            projection: PgCheckStatusProjection::production(provider, admission_provider, runtime),
            scope,
            invocations: AtomicUsize::new(0),
            last_result: Mutex::new(None),
        }
    }

    pub fn invocations(&self) -> usize {
        self.invocations.load(Ordering::SeqCst)
    }

    pub fn last_result(&self) -> Option<String> {
        self.last_result.lock().expect("last_result lock").clone()
    }
}

impl ToolExecutor for GitCheckStatusReadExecutor {
    fn execute(
        &self,
        _context: &ToolExecutionContext<'_>,
        def: &ToolDef,
        call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);

        if def.effect_kind != EffectKind::Read {
            return Err(ToolExecError::Failed(format!(
                "git.read_check_status executor received a non-Read tool `{}` ({:?})",
                def.name.0, def.effect_kind
            )));
        }

        let repo = call
            .arguments
            .get("repo")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolExecError::Failed(
                    "git.read_check_status requires a string `repo` argument".into(),
                )
            })?;
        let commit = call
            .arguments
            .get("commit")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolExecError::Failed(
                    "git.read_check_status requires a string `commit` argument".into(),
                )
            })?;

        let rows = self
            .projection
            .rows_for_commit(&self.scope, repo, &GitOid(commit.to_string()))
            .map_err(|e| ToolExecError::Failed(format!("git check-status read failed: {e}")))?;

        if rows.is_empty() {
            return Err(ToolExecError::Failed(format!(
                "no check status for commit `{commit}` in repo `{repo}` (absent or out of tenant scope)"
            )));
        }

        let parts: Vec<String> = rows
            .iter()
            .map(|row| {
                format!(
                    "{} = {:?} (run attempt {}, {:?}, cost_settled={})",
                    row.context.policy_token(),
                    row.state,
                    row.run_attempt,
                    row.trust_tier,
                    row.cost_settled,
                )
            })
            .collect();
        let text = format!(
            "check status for commit {commit} in repo {repo}: {}",
            parts.join("; ")
        );

        *self.last_result.lock().expect("last_result lock") = Some(text.clone());
        Ok(ToolResult(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_service::exec::{route_of, ToolRoute};

    #[test]
    fn read_tool_def_is_a_direct_route_read() {
        let def = git_check_status_read_tool_def();
        assert_eq!(def.name, ToolName(GIT_READ_CHECK_STATUS_TOOL.into()));
        assert_eq!(def.effect_kind, EffectKind::Read);
        assert!(!def.side_effecting);
        assert!(!def.requires_approval);
        assert_eq!(route_of(def.effect_kind), ToolRoute::Direct);
        assert_eq!(def.required_caps, vec!["pull".to_string()]);
    }

    #[test]
    fn tool_def_and_schema_share_name_and_input_schema() {
        let def = git_check_status_read_tool_def();
        let schema = git_check_status_read_tool_schema();
        assert_eq!(def.name, schema.name);
        assert_eq!(def.input_schema, schema.input_schema);
        assert!(schema.description.contains("check-status"));
        let parsed: serde_json::Value = serde_json::from_str(&def.input_schema).unwrap();
        assert_eq!(parsed["type"], "object");
        assert_eq!(parsed["required"][0], "repo");
        assert_eq!(parsed["required"][1], "commit");
    }
}
