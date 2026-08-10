use crate::{
    DurableChatReadApi, DurableCiReadApi, DurableIssueReadApi, DurableKnowledgeReadApi, EdgeError,
};
use myelin_ci_controlplane::surfacing_store::CI_LOG_RANGE_DEFAULT;
use myelin_identity::Principal;
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_issues::{
    api::{is_canonical_uuid, IssueListState},
    IssuePageRequest,
};
use myelin_mcp::{DirectReadError, DirectReadExecutor, ReadAuthorization};
use myelin_storage::TenantScope;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::agent_delegation::is_active_delegation;
use crate::DurableGitBackend;

pub struct McpReadExecutor {
    ci: DurableCiReadApi,
    issues: Option<DurableIssueReadApi>,
    knowledge: Option<DurableKnowledgeReadApi>,
    chat: Option<DurableChatReadApi>,
    git: Option<Arc<DurableGitBackend>>,
    authority: Arc<RunTokenAuthorizer>,
    delegator: Principal,
}

impl McpReadExecutor {
    pub fn new(
        ci: DurableCiReadApi,
        authority: Arc<RunTokenAuthorizer>,
        delegator: Principal,
    ) -> Self {
        Self {
            ci,
            issues: None,
            knowledge: None,
            chat: None,
            git: None,
            authority,
            delegator,
        }
    }

    pub fn with_issues(mut self, issues: DurableIssueReadApi) -> Self {
        self.issues = Some(issues);
        self
    }

    pub fn with_knowledge(mut self, knowledge: DurableKnowledgeReadApi) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    pub fn with_chat(mut self, chat: DurableChatReadApi) -> Self {
        self.chat = Some(chat);
        self
    }

    pub fn with_git(mut self, git: Arc<DurableGitBackend>) -> Self {
        self.git = Some(git);
        self
    }
}

impl DirectReadExecutor for McpReadExecutor {
    fn execute(
        &self,
        principal: &Principal,
        authority: &ReadAuthorization,
        tool: &str,
        arguments: &Value,
    ) -> Result<Value, DirectReadError> {
        if authority.tool() != tool {
            return Err(DirectReadError::Denied);
        }
        if !is_active_delegation(principal, &self.delegator) {
            return Err(DirectReadError::Denied);
        }
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        self.authority
            .authorize(
                &scope,
                &principal.principal_id,
                authority.run_token(),
                authority.required_caps(),
            )
            .map_err(|_| DirectReadError::Denied)?;
        match tool {
            "ci.read_run" => {
                exact_fields(arguments, &["run_id"], &["run_id"])?;
                let run_id = required_string(arguments, "run_id")?;
                self.ci
                    .read_run(&self.delegator, run_id)
                    .map_err(map_edge_error)
            }
            "ci.read_log" => {
                exact_fields(
                    arguments,
                    &["run_id", "job_id"],
                    &["run_id", "job_id", "start", "limit"],
                )?;
                let run_id = required_string(arguments, "run_id")?;
                let job_id = required_string(arguments, "job_id")?;
                let start = optional_i64(arguments, "start")?.unwrap_or(0);
                let limit = optional_u32(arguments, "limit")?.unwrap_or(CI_LOG_RANGE_DEFAULT);
                self.ci
                    .read_log(&self.delegator, run_id, job_id, start, limit)
                    .map_err(map_edge_error)
            }
            "issues.list" => self
                .issues
                .as_ref()
                .ok_or(DirectReadError::Unavailable)?
                .list(&self.delegator, issue_page(arguments)?)
                .map_err(map_edge_error),
            "issues.view" => {
                exact_fields(arguments, &["issue_id"], &["issue_id"])?;
                let issue_id = required_string(arguments, "issue_id")?;
                if !is_canonical_uuid(issue_id) {
                    return Err(invalid("`issue_id` must be a canonical lowercase UUID"));
                }
                self.issues
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .view(&self.delegator, issue_id)
                    .map_err(map_edge_error)
            }
            "knowledge.list_pages" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.knowledge
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_pages(&self.delegator, limit, cursor)
                    .map_err(map_edge_error)
            }
            "knowledge.read_page" => {
                exact_fields(arguments, &["page_id"], &["page_id"])?;
                let page_id = required_string(arguments, "page_id")?;
                self.knowledge
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .read_page(&self.delegator, page_id)
                    .map_err(map_edge_error)
            }
            "chat.list_conversations" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.chat
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_conversations(&self.delegator, limit, cursor)
                    .map_err(map_edge_error)
            }
            "chat.read_messages" => {
                exact_fields(
                    arguments,
                    &["conversation_id"],
                    &["conversation_id", "limit", "before"],
                )?;
                let conversation_id = required_string(arguments, "conversation_id")?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let before = optional_string(arguments, "before")?.map(str::to_string);
                self.chat
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .read_messages(&self.delegator, conversation_id, limit, before)
                    .map_err(map_edge_error)
            }
            "git.list_repositories" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_repositories(&self.delegator, limit, cursor)
                    .map_err(map_edge_error)
            }
            "git.search_code" => {
                exact_fields(arguments, &["query"], &["query", "repo"])?;
                let query = required_string(arguments, "query")?;
                let repo = optional_string(arguments, "repo")?;
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .search_code(&self.delegator, query, repo)
                    .map_err(map_edge_error)
            }
            "git.read_file" => {
                exact_fields(
                    arguments,
                    &["repo", "ref", "path"],
                    &["repo", "ref", "path"],
                )?;
                let repo = required_string(arguments, "repo")?;
                let gitref = required_string(arguments, "ref")?;
                let path = required_string(arguments, "path")?;
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .read_file(&self.delegator, repo, gitref, path)
                    .map_err(map_edge_error)
            }
            _ => Err(DirectReadError::Unavailable),
        }
    }
}

fn issue_page(arguments: &Value) -> Result<IssuePageRequest, DirectReadError> {
    exact_fields(arguments, &[], &["state", "key", "limit", "cursor"])?;
    let state = optional_string(arguments, "state")?
        .map(|value| {
            IssueListState::parse(value)
                .ok_or_else(|| invalid("`state` must be open, closed, or all"))
        })
        .transpose()?
        .unwrap_or(IssueListState::Open);
    let key = optional_string(arguments, "key")?.map(str::to_string);
    let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
    let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
    IssuePageRequest::filtered(state, key, limit, cursor)
        .map_err(|error| invalid(error.to_string()))
}

fn exact_fields(
    arguments: &Value,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), DirectReadError> {
    let object = arguments
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object"))?;
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(field.as_str()))
    {
        return Err(invalid(format!("unknown field `{field}`")));
    }
    if let Some(field) = required.iter().find(|field| !object.contains_key(**field)) {
        return Err(invalid(format!("missing field `{field}`")));
    }
    Ok(())
}

fn required_string<'a>(arguments: &'a Value, field: &str) -> Result<&'a str, DirectReadError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("`{field}` must be a string")))
}

fn optional_string<'a>(
    arguments: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, DirectReadError> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid(format!("`{field}` must be a string")))
        })
        .transpose()
}

fn optional_i64(arguments: &Value, field: &str) -> Result<Option<i64>, DirectReadError> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| invalid(format!("`{field}` must be an integer")))
        })
        .transpose()
}

fn optional_u32(arguments: &Value, field: &str) -> Result<Option<u32>, DirectReadError> {
    arguments
        .get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| invalid(format!("`{field}` must be a non-negative 32-bit integer")))
        })
        .transpose()
}

fn invalid(reason: impl Into<String>) -> DirectReadError {
    DirectReadError::InvalidInput(reason.into())
}

fn map_edge_error(error: EdgeError) -> DirectReadError {
    match error {
        EdgeError::BadRequest(reason) | EdgeError::Unprocessable(reason) => {
            DirectReadError::InvalidInput(reason)
        }
        EdgeError::NotFound(_) | EdgeError::Forbidden(_) | EdgeError::Unauthorized(_) => {
            DirectReadError::NotFound
        }
        EdgeError::Conflict(_)
        | EdgeError::PayloadTooLarge(_)
        | EdgeError::RequestTimeout(_)
        | EdgeError::Unavailable(_)
        | EdgeError::Internal(_) => DirectReadError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn argument_grammar_is_exact_and_bounded_before_storage() {
        assert!(exact_fields(&json!({"run_id":"x"}), &["run_id"], &["run_id"]).is_ok());
        for invalid_args in [
            json!(null),
            json!({}),
            json!({"run_id":"x","tenant":"other"}),
        ] {
            assert!(exact_fields(&invalid_args, &["run_id"], &["run_id"]).is_err());
        }
        assert_eq!(
            optional_i64(&json!({"start": 0}), "start").unwrap(),
            Some(0)
        );
        assert!(optional_i64(&json!({"start": -1.5}), "start").is_err());
        assert_eq!(
            optional_u32(&json!({"limit": 262144}), "limit").unwrap(),
            Some(262_144)
        );
        assert!(optional_u32(&json!({"limit": -1}), "limit").is_err());
        assert!(optional_u32(&json!({"limit": 4294967296_u64}), "limit").is_err());
    }

    #[test]
    fn issue_list_arguments_are_a_small_normalized_query_language() {
        assert_eq!(
            issue_page(&json!({})).unwrap(),
            IssuePageRequest::filtered(IssueListState::Open, None, 50, None).unwrap()
        );
        assert_eq!(
            issue_page(&json!({"state":"closed","key":"eng-","limit":7})).unwrap(),
            IssuePageRequest::filtered(IssueListState::Closed, Some("ENG-".into()), 7, None)
                .unwrap()
        );

        for arguments in [
            json!({"state":"OPEN"}),
            json!({"key":"title search"}),
            json!({"limit":0}),
            json!({"limit":"7"}),
            json!({"cursor":7}),
            json!({"tenant":"other"}),
        ] {
            assert!(issue_page(&arguments).is_err(), "accepted {arguments}");
        }
    }
}
