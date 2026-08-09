use crate::error::CliError;
use myelin_ci_controlplane::cli::{parse_cli as parse_ci_cli, CliCommand as CiCliCommand};
use myelin_git::api::{parse_cli, CliCommand, CliParseError};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::json;

mod chat;
mod issues;
mod knowledge;
mod projects;

pub use chat::chat_dispatch;
pub use issues::{issues_dispatch, issues_dispatch_with_context, issues_dispatch_with_project};
pub use knowledge::knowledge_dispatch;
pub use projects::{is_canonical_project_id, project_dispatch};

const FORM_QUERY_COMPONENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Default)]
struct FormQuery {
    encoded: String,
}

impl FormQuery {
    fn push(&mut self, name: &str, value: &str) {
        if !self.encoded.is_empty() {
            self.encoded.push('&');
        }
        self.encoded
            .push_str(&utf8_percent_encode(name, FORM_QUERY_COMPONENT_ENCODE_SET).to_string());
        self.encoded.push('=');
        self.encoded
            .push_str(&utf8_percent_encode(value, FORM_QUERY_COMPONENT_ENCODE_SET).to_string());
    }

    fn finish(self) -> String {
        self.encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPolicy {
    None,
    CallerKeyRequired,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeCall {
    pub method: HttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub idempotency_key: Option<String>,
    pub retry_policy: RetryPolicy,
}

impl EdgeCall {
    fn get(path: impl Into<String>) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Get,
            path: path.into(),
            query: None,
            payload: None,
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        }
    }

    fn post_json(path: impl Into<String>, payload: serde_json::Value) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Post,
            path: path.into(),
            query: None,
            payload: Some(payload.to_string().into_bytes()),
            idempotency_key: None,
            retry_policy: RetryPolicy::CallerKeyRequired,
        }
    }

    fn post_read_json(path: impl Into<String>, payload: serde_json::Value) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Post,
            path: path.into(),
            query: None,
            payload: Some(payload.to_string().into_bytes()),
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        }
    }

    pub fn with_idempotency_key(mut self, value: &str) -> Result<EdgeCall, CliError> {
        if self.retry_policy != RetryPolicy::CallerKeyRequired {
            return Err(CliError::Usage(
                "--idempotency-key applies only to mutating commands".into(),
            ));
        }
        let value = value.trim();
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(CliError::Usage(
                "--idempotency-key must be 1..128 ASCII-graphic bytes with no spaces".into(),
            ));
        }
        self.idempotency_key = Some(value.to_string());
        Ok(self)
    }
}

fn usage_from_git(e: CliParseError) -> CliError {
    let m = match e {
        CliParseError::Empty => {
            "no git command given (try: repo list | pr view <n> | search code <q>)".to_string()
        }
        CliParseError::Unknown { token } => format!("unknown git command token `{token}`"),
        CliParseError::MissingArg { what } => format!("missing argument: {what}"),
        CliParseError::DuplicateFlag { flag } => format!("duplicate flag: {flag}"),
        CliParseError::MissingValue { flag } => format!("missing value for flag: {flag}"),
        CliParseError::BadArg { value } => format!("malformed argument: `{value}`"),
    };
    CliError::Usage(m)
}

pub fn git_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let command = parse_cli(args).map_err(usage_from_git)?;
    git_command_to_call(&command)
}

/** Project the canonical `myelin repo ...` grammar onto Git's shared repo/PR/search parser. */
pub fn repo_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    if matches!(args.first().copied(), Some("pr" | "search")) {
        return git_dispatch(args);
    }
    let mut git_args = Vec::with_capacity(args.len() + 1);
    git_args.push("repo");
    git_args.extend_from_slice(args);
    git_dispatch(&git_args)
}

pub fn git_command_to_call(command: &CliCommand) -> Result<EdgeCall, CliError> {
    match command {
        CliCommand::RepoList { limit, cursor } => {
            let mut query = FormQuery::default();
            query.push("view", "summary");
            if let Some(limit) = limit {
                query.push("limit", &limit.to_string());
            }
            if let Some(cursor) = cursor {
                query.push("cursor", cursor);
            }
            Ok(EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/git/repos".into(),
                query: Some(query.finish()),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            })
        }
        CliCommand::RepoView { repo } => Ok(EdgeCall::get(format!("/v1/git/repos/{repo}"))),
        CliCommand::PrView { repo, number } => {
            Ok(EdgeCall::get(format!("/v1/git/repos/{repo}/prs/{number}")))
        }
        CliCommand::PrChecks { repo, number } => Ok(EdgeCall::get(format!(
            "/v1/git/repos/{repo}/prs/{number}/checks"
        ))),
        CliCommand::PrList { repo } => Ok(match repo {
            Some(repo) => EdgeCall::get(format!("/v1/git/repos/{repo}/prs")),
            None => EdgeCall::get("/v1/git/prs"),
        }),
        CliCommand::SearchCode {
            query: search,
            repo,
        } => {
            let mut query = FormQuery::default();
            query.push("q", search);
            if let Some(repo) = repo {
                query.push("repo", repo);
            }
            Ok(EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/git/search/code".into(),
                query: Some(query.finish()),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            })
        }

        CliCommand::RepoCreate { slug } => Ok(EdgeCall::post_json(
            "/v1/git/repos",
            json!({ "slug": slug }),
        )),
        CliCommand::PrOpen {
            repo,
            title,
            body: body_md,
            base_ref,
            head_ref,
            head_oid,
            draft,
        } => {
            let mut body = json!({ "draft": draft, "title": title });
            if let Some(b) = body_md {
                body["body"] = json!(b);
            }
            if let Some(b) = base_ref {
                body["base_ref"] = json!(b);
            }
            if let Some(h) = head_ref {
                body["head_ref"] = json!(h);
            }
            if let Some(o) = head_oid {
                body["head_oid"] = json!(o);
            }
            Ok(EdgeCall::post_json(
                format!("/v1/git/repos/{repo}/prs"),
                body,
            ))
        }
        CliCommand::PrReview {
            repo,
            number,
            verdict,
        } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/reviews"),
            json!({ "verdict": verdict }),
        )),
        CliCommand::PrMerge { repo, number, .. } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/merge"),
            json!({}),
        )),
        CliCommand::PrEndorseForkCi { repo, number } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/endorse-fork-ci"),
            json!({}),
        )),
    }
}

pub fn ci_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let command = parse_ci_cli(args).map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(ci_command_to_call(&command))
}

pub fn ci_command_to_call(command: &CiCliCommand) -> EdgeCall {
    match command {
        CiCliCommand::List(request) => {
            let mut query = FormQuery::default();
            query.push("state", request.state.token());
            query.push("limit", &request.limit.to_string());
            if let Some(cursor) = &request.cursor {
                query.push("cursor", cursor);
            }
            EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/ci/runs".into(),
                query: Some(query.finish()),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            }
        }
        CiCliCommand::View { run_id } => EdgeCall::get(format!("/v1/ci/runs/{run_id}")),
        CiCliCommand::Logs {
            run_id,
            job_id,
            range,
        } => {
            let mut query = FormQuery::default();
            query.push("start", &range.start.to_string());
            query.push("limit", &range.limit.to_string());
            EdgeCall {
                method: HttpMethod::Get,
                path: format!("/v1/ci/runs/{run_id}/jobs/{job_id}/log"),
                query: Some(query.finish()),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            }
        }
        CiCliCommand::Watch { run_id, job_id } => {
            EdgeCall::get(format!("/v1/ci/runs/{run_id}/jobs/{job_id}/log/live"))
        }
    }
}

pub fn notif_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    use myelin_notif::cli::CliView;
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage("no notif command (try: list [--view <v>] | show <id> | read <id>)".into())
    })?;
    match *verb {
        "list" => {
            let mut view = None;
            let mut limit = None;
            let mut cursor = None;
            let mut index = 0;
            while index < rest.len() {
                let flag = rest[index];
                let value = rest
                    .get(index + 1)
                    .ok_or_else(|| CliError::Usage(format!("`notif list {flag}` needs a value")))?;
                match flag {
                    "--view" if view.is_none() => view = Some(*value),
                    "--limit" if limit.is_none() => limit = Some(*value),
                    "--cursor" if cursor.is_none() => cursor = Some(*value),
                    "--view" | "--limit" | "--cursor" => {
                        return Err(CliError::Usage(format!(
                            "duplicate notif list flag `{flag}`"
                        )))
                    }
                    other => {
                        return Err(CliError::Usage(format!(
                            "unknown notif list flag `{other}`"
                        )))
                    }
                }
                index += 2;
            }
            let view = CliView::parse(view).map_err(CliError::Usage)?;
            let view_token = match view {
                CliView::All => "all",
                CliView::MyWork => "my-work",
                CliView::Activity => "activity",
                CliView::ReviewRequests => "review-requests",
            };
            let mut query = FormQuery::default();
            query.push("view", view_token);
            if let Some(value) = limit {
                let parsed = value.parse::<u16>().map_err(|_| {
                    CliError::Usage("--limit must be an integer between 1 and 100".into())
                })?;
                if !(1..=100).contains(&parsed) {
                    return Err(CliError::Usage(
                        "--limit must be an integer between 1 and 100".into(),
                    ));
                }
                query.push("limit", value);
            }
            if let Some(value) = cursor {
                if value.is_empty() || value.len() > 1_024 {
                    return Err(CliError::Usage(
                        "--cursor must be a non-empty bounded inbox cursor".into(),
                    ));
                }
                query.push("cursor", value);
            }
            Ok(EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/notif/inbox".into(),
                query: Some(query.finish()),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            })
        }
        "show" | "read" => {
            let id = notif_item_id(verb, rest)?;
            let path = format!(
                "/v1/notif/inbox/{}",
                utf8_percent_encode(id, FORM_QUERY_COMPONENT_ENCODE_SET)
            );
            if *verb == "show" {
                Ok(EdgeCall::get(path))
            } else {
                Ok(EdgeCall::post_json(format!("{path}/read"), json!({})))
            }
        }
        other => Err(CliError::Usage(format!(
            "unknown notif command `{other}` (try: list | show <id> | read <id>)"
        ))),
    }
}

fn notif_item_id<'a>(verb: &str, args: &'a [&str]) -> Result<&'a str, CliError> {
    let [id] = args else {
        return Err(CliError::Usage(format!(
            "`notif {verb}` needs exactly one <item_id>"
        )));
    };
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err(CliError::Usage(
            "notification item id must be a non-empty bounded value".into(),
        ));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn canonical_ci_cursor() -> String {
        let mut frame = [0_u8; 60];
        frame[0] = 1;
        format!(
            "cr1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    #[test]
    fn git_repo_list_maps_to_the_edge_repos_route() {
        let call = git_dispatch(&["repo", "list"]).unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/v1/git/repos");
        assert_eq!(call.query.as_deref(), Some("view=summary"));
        assert!(call.payload.is_none());
    }

    #[test]
    fn git_repo_list_pagination_builds_exact_safe_summary_queries() {
        let cursor = myelin_git::web::RepoListCursor::new([4; 32], "alpha")
            .unwrap()
            .encode();
        let limit = git_dispatch(&["repo", "list", "--limit", "25"]).unwrap();
        assert_eq!(limit.query.as_deref(), Some("view=summary&limit=25"));

        let cursor_only = git_dispatch(&["repo", "list", "--cursor", &cursor]).unwrap();
        assert_eq!(
            cursor_only.query.as_deref(),
            Some(format!("view=summary&cursor={cursor}").as_str())
        );

        let both = git_dispatch(&["repo", "list", "--cursor", &cursor, "--limit", "2"]).unwrap();
        assert_eq!(
            both.query.as_deref(),
            Some(format!("view=summary&limit=2&cursor={cursor}").as_str())
        );

        let mut encoded = FormQuery::default();
        encoded.push("cursor", "rl1_a&b%= ?");
        assert_eq!(encoded.finish(), "cursor=rl1_a%26b%25%3D%20%3F");
    }

    #[test]
    fn git_search_code_reuses_the_grammar_and_maps_to_search_route() {
        let call = git_dispatch(&["search", "code", "needle"]).unwrap();
        assert_eq!(call.path, "/v1/git/search/code");
        assert_eq!(call.query.as_deref(), Some("q=needle"));
    }

    #[test]
    fn git_search_code_form_encodes_every_query_and_the_optional_repo() {
        for (search, encoded) in [
            ("two words", "two%20words"),
            ("x&limit=100", "x%26limit%3D100"),
            ("100%", "100%25"),
            ("kind=value", "kind%3Dvalue"),
            ("naïve café", "na%C3%AFve%20caf%C3%A9"),
        ] {
            let call = git_dispatch(&["search", "code", search]).unwrap();
            assert_eq!(call.query, Some(format!("q={encoded}")));
        }

        let scoped = git_dispatch(&[
            "search",
            "code",
            "symbol & path=src/lib.rs",
            "--repo",
            "platform/core",
        ])
        .unwrap();
        assert_eq!(
            scoped.query.as_deref(),
            Some("q=symbol%20%26%20path%3Dsrc%2Flib.rs&repo=platform%2Fcore")
        );
    }

    #[test]
    fn git_bad_verb_is_usage_not_panic() {
        let err = git_dispatch(&["frobnicate"]).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(git_dispatch(&["pr", "view", "abc"]).unwrap_err().code(), 2);
    }

    #[test]
    fn git_pr_list_maps_to_cross_repo_or_repo_scoped_route() {
        let cross_repo = git_dispatch(&["pr", "list"]).unwrap();
        assert_eq!(cross_repo.method, HttpMethod::Get);
        assert_eq!(cross_repo.path, "/v1/git/prs");
        assert!(cross_repo.query.is_none() && cross_repo.payload.is_none());

        let repo = git_dispatch(&["pr", "list", "--repo", "platform/api"]).unwrap();
        assert_eq!(repo.method, HttpMethod::Get);
        assert_eq!(repo.path, "/v1/git/repos/platform/api/prs");
        assert!(repo.query.is_none() && repo.payload.is_none());
    }

    #[test]
    fn git_durable_writes_map_to_real_routes() {
        let create = git_dispatch(&["repo", "create", "alpha"]).unwrap();
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.path, "/v1/git/repos");
        assert!(
            create.idempotency_key.is_none(),
            "the subsystem mapper cannot invent a retry identity"
        );
        let body = String::from_utf8(create.payload.clone().unwrap()).unwrap();
        assert!(body.contains("\"slug\":\"alpha\""));
        assert!(
            !body.contains("tenant"),
            "the tenant is never in the body (IDOR floor)"
        );

        let view = git_dispatch(&["pr", "view", "alpha", "7"]).unwrap();
        assert_eq!(view.method, HttpMethod::Get);
        assert_eq!(view.path, "/v1/git/repos/alpha/prs/7");

        let merge = git_dispatch(&["pr", "merge", "alpha", "7"]).unwrap();
        assert_eq!(merge.method, HttpMethod::Post);
        assert_eq!(merge.path, "/v1/git/repos/alpha/prs/7/merge");

        let review = git_dispatch(&["pr", "review", "alpha", "7", "--approve"]).unwrap();
        assert_eq!(review.path, "/v1/git/repos/alpha/prs/7/reviews");
        assert!(String::from_utf8(review.payload.unwrap())
            .unwrap()
            .contains("approve"));
    }

    #[test]
    fn mutation_idempotency_keys_are_explicit_and_share_the_edge_grammar() {
        let call = git_dispatch(&["pr", "merge", "alpha", "7"])
            .unwrap()
            .with_idempotency_key(" retry-123 ")
            .unwrap();
        assert_eq!(call.idempotency_key.as_deref(), Some("retry-123"));

        for invalid in ["", " ", "contains space", "ø"] {
            let error = git_dispatch(&["pr", "merge", "alpha", "7"])
                .unwrap()
                .with_idempotency_key(invalid)
                .unwrap_err();
            assert_eq!(error.code(), 2);
        }
        assert!(git_dispatch(&["pr", "merge", "alpha", "7"])
            .unwrap()
            .with_idempotency_key(&"x".repeat(129))
            .is_err());
        assert_eq!(
            git_dispatch(&["pr", "view", "alpha", "7"])
                .unwrap()
                .with_idempotency_key("read-key")
                .unwrap_err()
                .code(),
            2,
        );
    }

    #[test]
    fn notif_list_maps_to_the_authenticated_inbox_route() {
        let call = notif_dispatch(&[
            "list", "--view", "my-work", "--limit", "25", "--cursor", "ni1_abc",
        ])
        .unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/v1/notif/inbox");
        assert_eq!(
            call.query.as_deref(),
            Some("view=my-work&limit=25&cursor=ni1_abc")
        );
        assert!(call.payload.is_none());
        assert_eq!(
            notif_dispatch(&["list"]).unwrap().query.as_deref(),
            Some("view=all")
        );
        assert_eq!(
            notif_dispatch(&["list", "--view", "everything"])
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(
            notif_dispatch(&["list", "--limit", "0"])
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(notif_dispatch(&["list", "--view"]).unwrap_err().code(), 2);
        assert_eq!(notif_dispatch(&["nope"]).unwrap_err().code(), 2);
        let show = notif_dispatch(&["show", "item/1"]).unwrap();
        assert_eq!(show.method, HttpMethod::Get);
        assert_eq!(show.path, "/v1/notif/inbox/item%2F1");

        let read = notif_dispatch(&["read", "item-1"]).unwrap();
        assert_eq!(read.method, HttpMethod::Post);
        assert_eq!(read.path, "/v1/notif/inbox/item-1/read");
        assert_eq!(read.payload, Some(b"{}".to_vec()));

        for args in [
            vec!["show"],
            vec!["read", "item-1", "extra"],
            vec!["show", "item\n1"],
        ] {
            assert_eq!(notif_dispatch(&args).unwrap_err().code(), 2);
        }
    }

    #[test]
    fn issues_commands_reuse_the_total_grammar_and_map_exact_route_bodies() {
        let project = "11111111-1111-1111-1111-111111111111";
        let type_id = "22222222-2222-2222-2222-222222222222";
        let issue = "33333333-3333-3333-3333-333333333333";
        let cursor = myelin_issues::api::encode_issue_page_cursor(
            myelin_issues::api::IssueListState::All,
            Some("eng-"),
            1_700_000_000_123_456,
            issue,
        )
        .unwrap();

        let list = issues_dispatch(&[
            "list", "--state", "all", "--key", "eng-", "--limit", "10", "--cursor", &cursor,
        ])
        .unwrap();
        assert_eq!(list.method, HttpMethod::Get);
        assert_eq!(list.path, "/v1/issues");
        assert_eq!(
            list.query,
            Some(format!("state=all&limit=10&key=ENG-&cursor={cursor}"))
        );

        let create = issues_dispatch(&[
            "create",
            "--project",
            project,
            "--type",
            type_id,
            "--prefix",
            "ENG",
            "--title",
            "Founder issue",
        ])
        .unwrap();
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.path, "/v1/issues");
        let body: serde_json::Value = serde_json::from_slice(&create.payload.unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "project_id": project,
                "type_id": type_id,
                "prefix": "ENG",
                "title": "Founder issue"
            })
        );
        assert!(body.get("tenant").is_none() && body.get("region").is_none());

        let contextual = issues_dispatch_with_project(
            &[
                "create",
                "--type",
                type_id,
                "--prefix",
                "ENG",
                "--title",
                "Uses the active project",
            ],
            Some(project),
        )
        .unwrap();
        let contextual_body: serde_json::Value =
            serde_json::from_slice(&contextual.payload.unwrap()).unwrap();
        assert_eq!(contextual_body["project_id"], project);

        let explicit_project = "44444444-4444-4444-4444-444444444444";
        let explicit = issues_dispatch_with_project(
            &[
                "create",
                "--project",
                explicit_project,
                "--type",
                type_id,
                "--prefix",
                "ENG",
                "--title",
                "Explicit project wins",
            ],
            Some(project),
        )
        .unwrap();
        let explicit_body: serde_json::Value =
            serde_json::from_slice(&explicit.payload.unwrap()).unwrap();
        assert_eq!(explicit_body["project_id"], explicit_project);

        let view = issues_dispatch(&["view", issue]).unwrap();
        assert_eq!(view.method, HttpMethod::Get);
        assert_eq!(view.path, format!("/v1/issues/{issue}"));
        let close = issues_dispatch(&["close", issue]).unwrap();
        assert_eq!(close.method, HttpMethod::Post);
        assert_eq!(close.path, format!("/v1/issues/{issue}/close"));
        assert_eq!(close.payload, Some(b"{}".to_vec()));
    }

    #[test]
    fn issues_bad_input_is_local_usage_failure() {
        assert_eq!(
            issues_dispatch(&["list", "--limit", "0"])
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(
            issues_dispatch(&["view", "not-a-uuid"]).unwrap_err().code(),
            2
        );
        assert_eq!(
            issues_dispatch(&["list", "--limit", "2", "--limit", "3"])
                .unwrap_err()
                .code(),
            2
        );
        assert_eq!(
            issues_dispatch(&["create", "--tenant", "acme"])
                .unwrap_err()
                .code(),
            2
        );
        assert!(issues_dispatch_with_project(
            &[
                "create",
                "--type",
                "22222222-2222-2222-2222-222222222222",
                "--prefix",
                "ENG",
                "--title",
                "No project"
            ],
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("context use --project"));
    }

    #[test]
    fn ci_reads_reuse_the_owned_grammar_and_map_exact_routes() {
        let run = "91000000-0000-4000-8000-000000000001";
        let job = "92000000-0000-4000-8000-000000000001";
        let cursor = canonical_ci_cursor();

        let list = ci_dispatch(&[
            "list", "--status", "failed", "--limit", "1", "--cursor", &cursor,
        ])
        .unwrap();
        assert_eq!(list.method, HttpMethod::Get);
        assert_eq!(list.path, "/v1/ci/runs");
        assert_eq!(
            list.query.as_deref(),
            Some(format!("state=failed&limit=1&cursor={cursor}").as_str())
        );
        assert!(list.payload.is_none());

        let view = ci_dispatch(&["view", run]).unwrap();
        assert_eq!(view.path, format!("/v1/ci/runs/{run}"));
        assert!(view.query.is_none());

        let log =
            ci_dispatch(&["logs", run, "--job", job, "--start", "9", "--limit", "7"]).unwrap();
        assert_eq!(log.path, format!("/v1/ci/runs/{run}/jobs/{job}/log"));
        assert_eq!(log.query.as_deref(), Some("start=9&limit=7"));
        assert!(log.payload.is_none());

        let watch = ci_dispatch(&["watch", run, "--job", job]).unwrap();
        assert_eq!(watch.path, format!("/v1/ci/runs/{run}/jobs/{job}/log/live"));
        assert!(watch.query.is_none());
        assert!(watch.payload.is_none());
    }

    #[test]
    fn malformed_ci_requests_fail_locally() {
        let run = "91000000-0000-4000-8000-000000000001";
        for invalid in [
            vec!["watch", run],
            vec!["list", "--limit", "0"],
            vec!["view", "not-a-uuid"],
            vec!["logs", run],
        ] {
            assert_eq!(ci_dispatch(&invalid).unwrap_err().code(), 2);
        }
    }
}
