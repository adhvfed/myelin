use crate::{
    DurableChatReadApi, DurableCiReadApi, DurableIssueReadApi, DurableKnowledgeReadApi,
    DurableProjectReadApi, EdgeError,
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

use crate::agent_delegation::ActiveDelegation;
use crate::workspace_access::{WorkspaceRunAccess, WorkspaceRunAccessError};
use crate::DurableGitBackend;

pub struct McpReadExecutor {
    ci: DurableCiReadApi,
    issues: Option<DurableIssueReadApi>,
    knowledge: Option<DurableKnowledgeReadApi>,
    chat: Option<DurableChatReadApi>,
    git: Option<Arc<DurableGitBackend>>,
    projects: Option<DurableProjectReadApi>,
    workspace: Option<WorkspaceRunAccess>,
    authority: Arc<RunTokenAuthorizer>,
    access_subject: Principal,
}

impl McpReadExecutor {
    pub fn new(
        ci: DurableCiReadApi,
        authority: Arc<RunTokenAuthorizer>,
        access_subject: Principal,
    ) -> Self {
        Self {
            ci,
            issues: None,
            knowledge: None,
            chat: None,
            git: None,
            projects: None,
            workspace: None,
            authority,
            access_subject,
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

    pub fn with_projects(mut self, projects: DurableProjectReadApi) -> Self {
        self.projects = Some(projects);
        self
    }

    pub fn with_workspace(mut self, workspace: Option<WorkspaceRunAccess>) -> Self {
        self.workspace = workspace;
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
        let delegation = ActiveDelegation::establish(principal, &self.access_subject)
            .ok_or(DirectReadError::Denied)?;
        let scope =
            TenantScope::from_verified_token(delegation.actor(), delegation.actor().region.clone());
        let access_subject = delegation.access_subject();
        let run_token = self
            .authority
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
                    .read_run(access_subject, run_id)
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
                    .read_log(access_subject, run_id, job_id, start, limit)
                    .map_err(map_edge_error)
            }
            "issues.list" => self
                .issues
                .as_ref()
                .ok_or(DirectReadError::Unavailable)?
                .list(access_subject, issue_page(arguments)?)
                .map_err(map_edge_error),
            "issues.view" => {
                let issues = self.issues.as_ref().ok_or(DirectReadError::Unavailable)?;
                match issue_view_target(arguments)? {
                    IssueViewTarget::Reference(issue_ref) => issues
                        .view_ref(access_subject, issue_ref)
                        .map_err(map_edge_error),
                    IssueViewTarget::LegacyId(issue_id) => {
                        if !is_canonical_uuid(issue_id) {
                            return Err(invalid("`issue_id` must be a canonical lowercase UUID"));
                        }
                        issues
                            .view(access_subject, issue_id)
                            .map_err(map_edge_error)
                    }
                }
            }
            "knowledge.list_pages" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.knowledge
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_pages(access_subject, limit, cursor)
                    .map_err(map_edge_error)
            }
            "knowledge.read_page" => {
                let knowledge = self
                    .knowledge
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?;
                match knowledge_page_target(arguments)? {
                    KnowledgePageTarget::Reference(page_reference) => knowledge
                        .read_page_ref(access_subject, page_reference)
                        .map_err(map_edge_error),
                    KnowledgePageTarget::LegacyId(page_id) => knowledge
                        .read_page(access_subject, page_id)
                        .map_err(map_edge_error),
                }
            }
            "chat.list_conversations" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.chat
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_conversations(access_subject, limit, cursor)
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
                    .read_messages(access_subject, conversation_id, limit, before)
                    .map_err(map_edge_error)
            }
            "git.list_repositories" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                if RunTokenAuthorizer::is_repository_scoped(&run_token) {
                    return Err(DirectReadError::Denied);
                }
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list_repositories(access_subject, limit, cursor)
                    .map_err(map_edge_error)
            }
            "git.search_code" => {
                exact_fields(arguments, &["query"], &["query", "repo"])?;
                let query = required_string(arguments, "query")?;
                let repo = optional_string(arguments, "repo")?;
                match repo {
                    Some(repo) if RunTokenAuthorizer::allows_repository(&run_token, repo) => {}
                    None if !RunTokenAuthorizer::is_repository_scoped(&run_token) => {}
                    _ => return Err(DirectReadError::Denied),
                }
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .search_code(access_subject, query, repo)
                    .map_err(map_edge_error)
            }
            "git.read_file" => {
                exact_fields(
                    arguments,
                    &["repo", "ref", "path"],
                    &["repo", "ref", "path"],
                )?;
                let repo = required_string(arguments, "repo")?;
                if !RunTokenAuthorizer::allows_repository(&run_token, repo) {
                    return Err(DirectReadError::Denied);
                }
                let gitref = required_string(arguments, "ref")?;
                let path = required_string(arguments, "path")?;
                self.git
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .read_file(access_subject, repo, gitref, path)
                    .map_err(map_edge_error)
            }
            "projects.list" => {
                exact_fields(arguments, &[], &["limit", "cursor"])?;
                let limit = optional_u32(arguments, "limit")?.unwrap_or(50);
                let cursor = optional_string(arguments, "cursor")?.map(str::to_string);
                self.projects
                    .as_ref()
                    .ok_or(DirectReadError::Unavailable)?
                    .list(access_subject, limit, cursor)
                    .map_err(map_edge_error)
            }
            "workspace.read_file" => {
                exact_fields(arguments, &["path"], &["path"])?;
                let path = required_string(arguments, "path")?;
                let outcome = self
                    .workspace
                    .as_ref()
                    .ok_or(DirectReadError::Denied)?
                    .read_file(path)
                    .map_err(map_workspace_error)?;
                let content = String::from_utf8(outcome.file.bytes)
                    .map_err(|_| invalid("workspace file is not valid UTF-8"))?;
                Ok(serde_json::json!({
                    "path": outcome.file.path,
                    "content": content,
                    "byte_len": content.len(),
                    "content_digest": blake3::hash(content.as_bytes()).to_hex().to_string(),
                    "workspace_generation": outcome.binding.workspace_generation,
                }))
            }
            _ => Err(DirectReadError::Unavailable),
        }
    }
}

fn map_workspace_error(error: WorkspaceRunAccessError) -> DirectReadError {
    match error {
        WorkspaceRunAccessError::InvalidPath(reason) => invalid(reason),
        WorkspaceRunAccessError::InvalidCommand(reason) => invalid(reason),
        WorkspaceRunAccessError::Indeterminate => DirectReadError::Unavailable,
        WorkspaceRunAccessError::NotFound => DirectReadError::NotFound,
        WorkspaceRunAccessError::TooLarge => {
            invalid("workspace file exceeds the interactive limit")
        }
        WorkspaceRunAccessError::Unavailable => DirectReadError::Unavailable,
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

enum IssueViewTarget<'a> {
    Reference(&'a str),
    LegacyId(&'a str),
}

fn issue_view_target(arguments: &Value) -> Result<IssueViewTarget<'_>, DirectReadError> {
    if arguments.get("issue_ref").is_some() {
        exact_fields(arguments, &["issue_ref"], &["issue_ref"])?;
        return required_string(arguments, "issue_ref").map(IssueViewTarget::Reference);
    }
    exact_fields(arguments, &["issue_id"], &["issue_id"])?;
    required_string(arguments, "issue_id").map(IssueViewTarget::LegacyId)
}

enum KnowledgePageTarget<'a> {
    Reference(&'a str),
    LegacyId(&'a str),
}

fn knowledge_page_target(arguments: &Value) -> Result<KnowledgePageTarget<'_>, DirectReadError> {
    if arguments.get("page_ref").is_some() {
        exact_fields(arguments, &["page_ref"], &["page_ref"])?;
        return required_string(arguments, "page_ref").map(KnowledgePageTarget::Reference);
    }
    exact_fields(arguments, &["page_id"], &["page_id"])?;
    required_string(arguments, "page_id").map(KnowledgePageTarget::LegacyId)
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
        | EdgeError::TooManyRequests(_)
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

    #[test]
    fn issue_view_accepts_one_versioned_identity_without_aliasing() {
        assert!(matches!(
            issue_view_target(&json!({"issue_ref":"myelin://acme/issue/issue/ENG-41"})),
            Ok(IssueViewTarget::Reference(
                "myelin://acme/issue/issue/ENG-41"
            ))
        ));
        assert!(matches!(
            issue_view_target(&json!({"issue_id":"11111111-1111-1111-1111-111111111111"})),
            Ok(IssueViewTarget::LegacyId(
                "11111111-1111-1111-1111-111111111111"
            ))
        ));
        for arguments in [
            json!({}),
            json!({"issue_ref":"myelin://acme/issue/issue/ENG-41","issue_id":"11111111-1111-1111-1111-111111111111"}),
            json!({"issue_ref":"myelin://acme/issue/issue/ENG-41","tenant":"other"}),
        ] {
            assert!(
                issue_view_target(&arguments).is_err(),
                "accepted {arguments}"
            );
        }
    }

    #[test]
    fn knowledge_read_accepts_one_versioned_identity_without_aliasing() {
        assert!(matches!(
            knowledge_page_target(
                &json!({"page_ref":"myelin://acme/knowledge/page/01J00000000000000000000000"})
            ),
            Ok(KnowledgePageTarget::Reference(
                "myelin://acme/knowledge/page/01J00000000000000000000000"
            ))
        ));
        assert!(matches!(
            knowledge_page_target(&json!({"page_id":"01J00000000000000000000000"})),
            Ok(KnowledgePageTarget::LegacyId("01J00000000000000000000000"))
        ));
        for arguments in [
            json!({}),
            json!({"page_ref":"myelin://acme/knowledge/page/01J00000000000000000000000","page_id":"01J00000000000000000000000"}),
            json!({"page_ref":"myelin://acme/knowledge/page/01J00000000000000000000000","tenant":"other"}),
        ] {
            assert!(
                knowledge_page_target(&arguments).is_err(),
                "accepted {arguments}"
            );
        }
    }
}
