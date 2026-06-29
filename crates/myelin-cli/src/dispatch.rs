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
//! Adding a subsystem to the CLI is steps 1+2 only — the auth, the Bearer presentation, the envelope
//! parsing, the rendering, and the exit codes are owned ONCE by the shell (this crate), for everyone.

use crate::error::CliError;
use myelin_git::api::{parse_cli, CliCommand, CliParseError};

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
}

/// Map a parse error from a subsystem grammar to a clean [`CliError::Usage`] (exit 2) — a bad verb /
/// missing arg is never a panic.
fn usage_from_git(e: CliParseError) -> CliError {
    let m = match e {
        CliParseError::Empty => "no git command given (try: repo list | pr view <n> | search code <q>)".to_string(),
        CliParseError::Unknown { token } => format!("unknown git command token `{token}`"),
        CliParseError::MissingArg { what } => format!("missing argument: {what}"),
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

/// Map a git [`CliCommand`] to its edge [`EdgeCall`]. The implemented (REAL, end-to-end) reads are
/// the repo list + code search — the edge GET routes that line up with the grammar's args (the tenant
/// is from the token; neither carries a path-tenant). The remaining commands are honestly deferred:
/// the edge git surface (MR-015) is repos-list / single-PR / checks / blob / code-search, and several
/// grammar verbs (`repo view`, `pr list`, `pr view`) either have no edge endpoint yet or carry no
/// `<repo>` the edge PR/blob path requires — named, not faked.
pub fn git_command_to_call(command: &CliCommand) -> Result<EdgeCall, CliError> {
    match command {
        // REAL end-to-end: GET /v1/git/repos → the leak-free repo list ViewModel (the {items,page}
        // envelope). The tenant is the verified token's; the path carries none.
        CliCommand::RepoList => Ok(EdgeCall::get("/v1/git/repos")),

        // REAL end-to-end: GET /v1/git/search/code?q=… → the ACL-pre-filtered code-search hits.
        CliCommand::SearchCode { query, .. } => {
            if query.split_whitespace().count() != 1 {
                // The edge query parser does not percent-decode yet (a named follow-on); a
                // whitespace query would not round-trip. Honest, total — never a silent mismatch.
                return Err(CliError::Usage(
                    "code search currently supports a single whitespace-free term (the edge query \
                     codec is a named follow-on)".into(),
                ));
            }
            Ok(EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/git/search/code".into(),
                query: Some(format!("q={query}")),
                payload: None,
            })
        }

        // Honest deferrals — the grammar accepts these, but the edge does not expose a matching route
        // (or the grammar carries no `<repo>` the edge path needs). Named, never faked.
        CliCommand::RepoView { .. } => Err(CliError::Unsupported(
            "`git repo view` is not yet on the edge (the MR-015 git surface is repos-list / single-PR \
             / checks / blob / code-search) — deferred".into(),
        )),
        CliCommand::PrList { .. } => Err(CliError::Unsupported(
            "`git pr list` has no edge endpoint yet (the edge exposes a single PR view at \
             /v1/git/repos/<repo>/prs/<n>, not a list) — deferred".into(),
        )),
        CliCommand::PrView { .. } | CliCommand::PrChecks { .. } => Err(CliError::Unsupported(
            "the edge PR path is /v1/git/repos/<repo>/prs/<n>, but the `pr` grammar carries no \
             <repo> to build it — deferred until the grammar threads a repo".into(),
        )),
        CliCommand::PrReview { .. } | CliCommand::PrMerge { .. } | CliCommand::PrEndorseForkCi { .. } => {
            Err(CliError::Unsupported(
                "git write commands are wired at the edge but their durable effect is deferred to the \
                 Git track (E1.1), and the grammar carries no <repo> for the path — deferred".into(),
            ))
        }
    }
}

/// **Parse + map a `myelin notif …` invocation** (the args AFTER `notif`/`inbox`). REUSES notif's own
/// grammar ([`myelin_notif::cli::CliView`] for the `--view` flag + its verb set). Notif is NOT yet
/// wired through the edge (MR-015 plugged Git first; `/v1/notif/...` routes are a follow-on), so this
/// validates the command with notif's grammar and returns a HONEST [`CliError::Unsupported`] naming
/// the gap — it proves the framework dispatches a SECOND subsystem by its own grammar without faking
/// a call.
pub fn notif_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    use myelin_notif::cli::CliView;
    let (verb, rest) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("no notif command (try: list [--view <v>] | show <id> | read <id>)".into()))?;
    match *verb {
        "list" => {
            // REUSE notif's CliView grammar to validate the --view flag (a typo is rejected loudly,
            // never silently the ALL inbox — the same property notif's CLI enforces).
            let view_arg = flag_value(rest, "--view");
            let view = CliView::parse(view_arg.as_deref()).map_err(CliError::Usage)?;
            Err(CliError::Unsupported(format!(
                "notif is not yet wired through the edge (MR-015 plugged git first; /v1/notif routes \
                 are a follow-on) — parsed `inbox list --view {view:?}` via notif's grammar, deferred"
            )))
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

/// The value following a `--flag` in an arg slice (e.g. `--view my-work`), or `None`.
fn flag_value(args: &[&str], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| *a == flag)?;
    args.get(idx + 1).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The REAL command — `myelin git repo list` maps to GET /v1/git/repos through git's OWN
    /// grammar** (the reuse proof: `parse_cli(["repo","list"])` is git's, not re-declared here).
    #[test]
    fn git_repo_list_maps_to_the_edge_repos_route() {
        let call = git_dispatch(&["repo", "list"]).unwrap();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/v1/git/repos");
        assert!(call.query.is_none() && call.payload.is_none());
    }

    /// A git command defined in `api.rs` is reachable WITHOUT re-declaring it here: `search code`
    /// parses via git's grammar and maps to the edge search route.
    #[test]
    fn git_search_code_reuses_the_grammar_and_maps_to_search_route() {
        let call = git_dispatch(&["search", "code", "needle"]).unwrap();
        assert_eq!(call.path, "/v1/git/search/code");
        assert_eq!(call.query.as_deref(), Some("q=needle"));
    }

    /// A bad git verb is a clean Usage error (exit 2), never a panic.
    #[test]
    fn git_bad_verb_is_usage_not_panic() {
        let err = git_dispatch(&["frobnicate"]).unwrap_err();
        assert_eq!(err.code(), 2);
        // a non-numeric PR number is also a clean usage error (git's BadArg).
        assert_eq!(git_dispatch(&["pr", "view", "abc"]).unwrap_err().code(), 2);
    }

    /// A parsed-but-unmapped git command is an HONEST Unsupported (exit 4), not a faked success.
    #[test]
    fn git_unmapped_command_is_honest_unsupported() {
        let err = git_dispatch(&["repo", "view", "core"]).unwrap_err();
        assert_eq!(err.code(), 4);
    }

    /// notif REUSES its own grammar (CliView): a valid `--view` parses, an unknown view is rejected
    /// loudly (Usage), and the wired-call is honestly deferred (Unsupported).
    #[test]
    fn notif_reuses_cliview_grammar_and_defers_the_edge_call() {
        // a valid view parses (via notif's CliView) → the deferral, exit 4.
        assert_eq!(notif_dispatch(&["list", "--view", "my-work"]).unwrap_err().code(), 4);
        // an unknown view is rejected by notif's grammar → a clean Usage, exit 2 (never the ALL inbox).
        assert_eq!(notif_dispatch(&["list", "--view", "everything"]).unwrap_err().code(), 2);
        // a bad notif verb is Usage.
        assert_eq!(notif_dispatch(&["nope"]).unwrap_err().code(), 2);
    }
}
