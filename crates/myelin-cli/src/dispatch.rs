//! # The subsystem command framework — REUSE the grammars, map to the edge route.
//!
//! This is the headline of MR-020: the CLI is a THIN shell over (a) each subsystem's OWN command
//! grammar and (b) the MR-014 edge contract. A subsystem's command SET is NOT re-declared here — it
//! is parsed by the subsystem's own total parser, and the parsed command is mapped to an edge
//! `(method, path)`. So a new git command added to `myelin_git::api` flows to the CLI the moment its
//! `parse_cli` accepts it; the only thing this crate adds is the route mapping.
//!
//! ## How a subsystem adds CLI commands (the plug-in convention — mirrors the MR-014 edge plug-in)
//! The MR-014 edge convention is "a subsystem adds ONLY its routes + handlers; the gateway owns
//! auth/scope/error/pagination". The CLI's mirror is:
//!   1. **Parse** — the subsystem exposes a total `parse(args) -> Result<Command, ParseError>` over
//!      its frozen verb grammar (git: [`myelin_git::api::parse_cli`]; notif: [`myelin_notif::cli`]).
//!      The CLI calls it verbatim — it never re-derives the verbs.
//!   2. **Map** — a small `command -> EdgeCall` function names the `(method, path, query, payload)`
//!      the parsed command lowers to (reusing the subsystem's edge routes, registered MR-015+).
//!   3. **Dispatch** — the CLI shell (`main`) resolves the token, presents the Bearer, runs the call,
//!      and renders the `{items,page}` / view-model JSON (human or `--json`).
//!
//! Adding a subsystem to the CLI is steps 1+2 only — the auth, the Bearer presentation, the envelope
//! parsing, the rendering, and the exit codes are owned ONCE by the shell (this crate), for everyone.

use crate::error::CliError;
use myelin_ci_controlplane::cli::{parse_cli as parse_ci_cli, CliCommand as CiCliCommand};
use myelin_git::api::{parse_cli, CliCommand, CliParseError};
use myelin_issues::api::{parse_cli as parse_issues_cli, CliCommand as IssuesCliCommand};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::json;

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

/// The HTTP method an [`EdgeCall`] uses (the CLI only issues reads + simple writes today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// A read (`GET`).
    Get,
    /// A write (`POST`).
    Post,
}

impl HttpMethod {
    /// The uppercase HTTP token.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }
}

/// **A resolved edge call** — the `(method, path)` (+ optional query/payload) a parsed subsystem
/// command lowers to. The path is the versioned `/v1/...` route the MR-014 gateway dispatches; the
/// tenant is NEVER in the path (it is the verified token's — the IDOR floor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeCall {
    /// The HTTP method.
    pub method: HttpMethod,
    /// The versioned route path (`/v1/git/repos`).
    pub path: String,
    /// The raw query string (without `?`), if any.
    pub query: Option<String>,
    /// The request payload bytes (for a write), if any.
    pub payload: Option<Vec<u8>>,
}

impl EdgeCall {
    fn get(path: impl Into<String>) -> EdgeCall {
        EdgeCall { method: HttpMethod::Get, path: path.into(), query: None, payload: None }
    }

    /// A `POST` with a JSON body (the body bytes are the serialized payload). The tenant is NEVER in
    /// the body — it is the verified token's (the IDOR floor); the body carries only the proposal.
    fn post_json(path: impl Into<String>, payload: serde_json::Value) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Post,
            path: path.into(),
            query: None,
            payload: Some(payload.to_string().into_bytes()),
        }
    }
}

/// Map a parse error from a subsystem grammar to a clean [`CliError::Usage`] (exit 2) — a bad verb /
/// missing arg is never a panic.
fn usage_from_git(e: CliParseError) -> CliError {
    let m = match e {
        CliParseError::Empty => "no git command given (try: repo list | pr view <n> | search code <q>)".to_string(),
        CliParseError::Unknown { token } => format!("unknown git command token `{token}`"),
        CliParseError::MissingArg { what } => format!("missing argument: {what}"),
        CliParseError::DuplicateFlag { flag } => format!("duplicate flag: {flag}"),
        CliParseError::MissingValue { flag } => format!("missing value for flag: {flag}"),
        CliParseError::BadArg { value } => format!("malformed argument: `{value}`"),
    };
    CliError::Usage(m)
}

/// **Parse + map a `myelin git …` invocation** (the args AFTER `git`). REUSES git's own
/// [`parse_cli`] (no re-derivation), then maps the parsed [`CliCommand`] to the edge route. A command
/// the grammar accepts but the edge does not yet expose is a HONEST [`CliError::Unsupported`] (exit
/// 4), never a faked success.
pub fn git_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let command = parse_cli(args).map_err(usage_from_git)?;
    git_command_to_call(&command)
}

/// Map a git [`CliCommand`] to its edge [`EdgeCall`]. As of GT-005 the operator surface is wired to
/// the DURABLE GT-003 edge: repo create/list/view, PR open/view/checks/review/merge/endorse, blob, and
/// code-search all hit a real `/v1/git/...` route under the Bearer capability token (the tenant is the
/// token's — no path-tenant). A server-side gate (the merge gate) surfaces as a clean edge error
/// through the client, never a CLI bypass. PR lists use the edge's leak-free list handlers: an
/// optional `--repo` selects the object-guarded repository list, while the unscoped command selects
/// the cross-repository "needs your review" front door.
pub fn git_command_to_call(command: &CliCommand) -> Result<EdgeCall, CliError> {
    match command {
        // ── Reads ──
        // GET /v1/git/repos?view=summary → the bounded lightweight catalogue rows. The CLI never
        // requests the legacy RepoHome list projection.
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
            })
        }
        // GET /v1/git/repos/<repo> → the per-repo home projection (durable on-disk state).
        CliCommand::RepoView { repo } => Ok(EdgeCall::get(format!("/v1/git/repos/{repo}"))),
        // GET /v1/git/repos/<repo>/prs/<n> → the durable PR overview.
        CliCommand::PrView { repo, number } => {
            Ok(EdgeCall::get(format!("/v1/git/repos/{repo}/prs/{number}")))
        }
        // GET /v1/git/repos/<repo>/prs/<n>/checks → the X-1 checks projection (the repo-owned ruleset).
        CliCommand::PrChecks { repo, number } => {
            Ok(EdgeCall::get(format!("/v1/git/repos/{repo}/prs/{number}/checks")))
        }
        // GET /v1/git/repos/<repo>/prs → every visible PR in one object-guarded repository.
        // GET /v1/git/prs → the cross-repository, leak-free attention list (needs-review by default).
        CliCommand::PrList { repo } => Ok(match repo {
            Some(repo) => EdgeCall::get(format!("/v1/git/repos/{repo}/prs")),
            None => EdgeCall::get("/v1/git/prs"),
        }),
        // GET /v1/git/search/code?q=… → the ACL-pre-filtered code-search hits.
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
            })
        }

        // ── Writes (durable GT-003; the tenant is the token's, never the body) ──
        // POST /v1/git/repos {slug} → create a durable bare repo.
        CliCommand::RepoCreate { slug } => {
            Ok(EdgeCall::post_json("/v1/git/repos", json!({ "slug": slug })))
        }
        // POST /v1/git/repos/<repo>/prs {base_ref, head_ref, head_oid, draft} → open a PR. The body
        // carries ONLY the proposal (never branch-protection policy or check facts — the GT-003 fix).
        CliCommand::PrOpen { repo, title, body: body_md, base_ref, head_ref, head_oid, draft } => {
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
            Ok(EdgeCall::post_json(format!("/v1/git/repos/{repo}/prs"), body))
        }
        // POST /v1/git/repos/<repo>/prs/<n>/reviews {verdict} → submit a review.
        CliCommand::PrReview { repo, number, verdict } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/reviews"),
            json!({ "verdict": verdict }),
        )),
        // POST /v1/git/repos/<repo>/prs/<n>/merge → the merge gate (server-enforced). A blocked gate is
        // a clean edge error the CLI surfaces (the reason), never a bypass. `--auto` (merge-when-green)
        // is a named durable follow-on; the immediate gate evaluation runs server-side either way.
        CliCommand::PrMerge { repo, number, .. } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/merge"),
            json!({}),
        )),
        // POST /v1/git/repos/<repo>/prs/<n>/endorse-fork-ci → the maintainer fork-CI endorsement.
        CliCommand::PrEndorseForkCi { repo, number } => Ok(EdgeCall::post_json(
            format!("/v1/git/repos/{repo}/prs/{number}/endorse-fork-ci"),
            json!({}),
        )),
    }
}

/// Parse an Issues invocation with the subsystem-owned grammar and map it to the existing durable
/// Edge routes. A create intentionally returns the Edge's `202` pending receipt; dispatch never
/// rewrites that asynchronous contract into a claim of immediate visibility.
pub fn issues_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let command = parse_issues_cli(args).map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(issues_command_to_call(&command))
}

/// Map a validated Issues command to its tenant-less Edge call.
pub fn issues_command_to_call(command: &IssuesCliCommand) -> EdgeCall {
    match command {
        IssuesCliCommand::List { state, key, limit, cursor } => {
            let mut query = format!("state={}&limit={limit}", state.as_str());
            if let Some(key) = key {
                query.push_str("&key=");
                query.push_str(key);
            }
            if let Some(cursor) = cursor {
                query.push_str("&cursor=");
                query.push_str(cursor);
            }
            EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/issues".into(),
                query: Some(query),
                payload: None,
            }
        }
        IssuesCliCommand::Create {
            project_id,
            type_id,
            prefix,
            title,
        } => EdgeCall::post_json(
            "/v1/issues",
            json!({
                "project_id": project_id,
                "type_id": type_id,
                "prefix": prefix,
                "title": title,
            }),
        ),
        IssuesCliCommand::View { issue_id } => EdgeCall::get(format!("/v1/issues/{issue_id}")),
        IssuesCliCommand::Close { issue_id } => {
            EdgeCall::post_json(format!("/v1/issues/{issue_id}/close"), json!({}))
        }
    }
}

/// Parse CI's subsystem-owned durable-read grammar and map it to the authenticated Edge routes.
/// `ci watch` is refused by that grammar until the separately deployed runner and Edge share a real
/// bounded resume authority.
pub fn ci_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let command = parse_ci_cli(args).map_err(|error| CliError::Usage(error.to_string()))?;
    Ok(ci_command_to_call(&command))
}

/// Map one validated CI read to the tenant-less Edge route.
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
            }
        }
    }
}

/// **Parse + map a `myelin notif …` invocation** (the args AFTER `notif`/`inbox`). REUSES notif's own
/// grammar ([`myelin_notif::cli::CliView`] for the `--view` flag + its verb set). The durable list
/// route is live; item detail/read mutation remain honest unsupported floors.
pub fn notif_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    use myelin_notif::cli::CliView;
    let (verb, rest) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("no notif command (try: list [--view <v>] | show <id> | read <id>)".into()))?;
    match *verb {
        "list" => {
            let mut view = None;
            let mut limit = None;
            let mut cursor = None;
            let mut index = 0;
            while index < rest.len() {
                let flag = rest[index];
                let value = rest.get(index + 1).ok_or_else(|| {
                    CliError::Usage(format!("`notif list {flag}` needs a value"))
                })?;
                match flag {
                    "--view" if view.is_none() => view = Some(*value),
                    "--limit" if limit.is_none() => limit = Some(*value),
                    "--cursor" if cursor.is_none() => cursor = Some(*value),
                    "--view" | "--limit" | "--cursor" => {
                        return Err(CliError::Usage(format!("duplicate notif list flag `{flag}`")))
                    }
                    other => {
                        return Err(CliError::Usage(format!("unknown notif list flag `{other}`")))
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
            })
        }
        "show" | "read" => {
            let _id = rest
                .iter()
                .find(|a| !a.starts_with("--"))
                .ok_or_else(|| CliError::Usage(format!("`notif {verb}` needs an <item_id>")))?;
            Err(CliError::Unsupported(format!(
                "notif `{verb}` is not yet wired through the edge (/v1/notif routes are a follow-on) \
                 — deferred"
            )))
        }
        other => Err(CliError::Usage(format!(
            "unknown notif command `{other}` (try: list | show <id> | read <id>)"
        ))),
    }
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

    /// **The REAL command — `myelin git repo list` maps to the opt-in lightweight summary through
    /// git's OWN grammar** (the reuse proof: `parse_cli(["repo","list"])` is git's).
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

        let both = git_dispatch(&[
            "repo", "list", "--cursor", &cursor, "--limit", "2",
        ])
        .unwrap();
        assert_eq!(
            both.query.as_deref(),
            Some(format!("view=summary&limit=2&cursor={cursor}").as_str())
        );

        let mut encoded = FormQuery::default();
        encoded.push("cursor", "rl1_a&b%= ?");
        assert_eq!(encoded.finish(), "cursor=rl1_a%26b%25%3D%20%3F");
    }

    /// A git command defined in `api.rs` is reachable WITHOUT re-declaring it here: `search code`
    /// parses via git's grammar and maps to the edge search route.
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

    /// A bad git verb is a clean Usage error (exit 2), never a panic.
    #[test]
    fn git_bad_verb_is_usage_not_panic() {
        let err = git_dispatch(&["frobnicate"]).unwrap_err();
        assert_eq!(err.code(), 2);
        // a non-numeric PR number is also a clean usage error (git's BadArg).
        assert_eq!(git_dispatch(&["pr", "view", "abc"]).unwrap_err().code(), 2);
    }

    /// The PR-list grammar selects the matching leak-free edge endpoint. An unscoped list is the
    /// cross-repository attention inbox; `--repo` selects the object-guarded repository list.
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

    /// GT-005: the durable write commands now map to a real `/v1/git/...` route (POST with a JSON
    /// body) — the tenant is the token's, never the body (the IDOR floor).
    #[test]
    fn git_durable_writes_map_to_real_routes() {
        let create = git_dispatch(&["repo", "create", "alpha"]).unwrap();
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.path, "/v1/git/repos");
        let body = String::from_utf8(create.payload.clone().unwrap()).unwrap();
        assert!(body.contains("\"slug\":\"alpha\""));
        assert!(!body.contains("tenant"), "the tenant is never in the body (IDOR floor)");

        let view = git_dispatch(&["pr", "view", "alpha", "7"]).unwrap();
        assert_eq!(view.method, HttpMethod::Get);
        assert_eq!(view.path, "/v1/git/repos/alpha/prs/7");

        let merge = git_dispatch(&["pr", "merge", "alpha", "7"]).unwrap();
        assert_eq!(merge.method, HttpMethod::Post);
        assert_eq!(merge.path, "/v1/git/repos/alpha/prs/7/merge");

        let review = git_dispatch(&["pr", "review", "alpha", "7", "--approve"]).unwrap();
        assert_eq!(review.path, "/v1/git/repos/alpha/prs/7/reviews");
        assert!(String::from_utf8(review.payload.unwrap()).unwrap().contains("approve"));
    }

    /// notif REUSES its own view grammar and maps only recipient-neutral page coordinates.
    #[test]
    fn notif_list_maps_to_the_authenticated_inbox_route() {
        let call = notif_dispatch(&[
            "list", "--view", "my-work", "--limit", "25", "--cursor", "ni1_abc",
        ])
        .unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/v1/notif/inbox");
        assert_eq!(call.query.as_deref(), Some("view=my-work&limit=25&cursor=ni1_abc"));
        assert!(call.payload.is_none());
        assert_eq!(notif_dispatch(&["list"]).unwrap().query.as_deref(), Some("view=all"));
        assert_eq!(notif_dispatch(&["list", "--view", "everything"]).unwrap_err().code(), 2);
        assert_eq!(notif_dispatch(&["list", "--limit", "0"]).unwrap_err().code(), 2);
        assert_eq!(notif_dispatch(&["list", "--view"]).unwrap_err().code(), 2);
        assert_eq!(notif_dispatch(&["nope"]).unwrap_err().code(), 2);
        assert_eq!(notif_dispatch(&["show", "item-1"]).unwrap_err().code(), 4);
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
    }

    #[test]
    fn ci_reads_reuse_the_owned_grammar_and_map_exact_routes() {
        let run = "91000000-0000-4000-8000-000000000001";
        let job = "92000000-0000-4000-8000-000000000001";
        let cursor = canonical_ci_cursor();

        let list = ci_dispatch(&[
            "list",
            "--status",
            "failed",
            "--limit",
            "1",
            "--cursor",
            &cursor,
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
    }

    #[test]
    fn ci_live_and_malformed_requests_fail_locally() {
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
