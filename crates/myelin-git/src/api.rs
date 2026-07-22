//! # The Git CLI + HTTP/RPC + agent-tool API surface (GIT-P32 / P-293, M3-G8)
//!
//! **One API surface, three consumers** (UI, CLI, agents) — architecture
//! `04-views-cli-and-api.md` §3 (the two CLI surfaces) + §4 (the HTTP/RPC + agent-tool API). This
//! module is the **catalogue + parse/route logic** of that surface: the `myelin …` git CLI command
//! grammar ([`CliCommand`]) and the HTTP endpoint route table ([`Endpoint`]).
//!
//! **NO new handler / NO new contract** (the prompt states this): each CLI verb and HTTP route maps to
//! an **already-built** git handler —
//! - `pr merge` / `POST …/merge` → [`crate::merge_gate`] + [`crate::merge_queue`] (the merge gate +
//!   durable queue);
//! - `pr endorse-fork-ci` / `POST …/endorse-fork-ci` → [`crate::fork_gate`] (the `approve_untrusted_ci`
//!   endorsement);
//! - `pr checks` / `GET …/checks` → [`crate::check_status`] (the X-1 projection);
//! - `repo list [--limit 1..100] [--cursor <rl1_…>]` / `GET /api/git/repos?view=summary` →
//!   [`crate::list_filter`] (the leak-free `SetExpr` push-down);
//! - `search code` / `GET …/search/code` → [`crate::list_filter::code_search_pre_filter`] (the ACL
//!   pre-filter conjoined before scoring);
//! - `GET …/prs/{n}` / `GET …/blob/…` → [`crate::project`] (the per-viewer 0-leak projection).
//!
//! **The two structural invariants this catalogue PROVES (the X-1 / BUS-2 / ADR-08 floors):**
//! - **Every WRITE endpoint is one-transaction `Id.check` → state-change + outbox-emit** ([`Method`]
//!   `is_write` ⇒ [`Endpoint::id_checked`] is true): the catalogue refuses to register a write route
//!   that is not `Id.check`-gated (a mutant that drops the check fails the [`Endpoint::new`] guard).
//! - **Every CLI verb is identity-injected** ([`CliCommand`] carries the tenant FROM THE TOKEN, never
//!   the URL/arg — the GIT-D8 invariant): the CLI is a thin client over the SAME API the UI + agents
//!   use (no carve-out, ADR-08).
//! - **The agent-tool `requires_approval` defaults are FROZEN** ([`AgentToolDef`]): `git.merge = yes`
//!   (the ONLY consequential git gate), `open_pr = no`. Agents act through `EffectApi` (plan-then-
//!   apply), never direct writes (ADR-08).
//!
//! The production `russh` / `axum` transport wiring lives in the front-door host
//! ([`crate::front_door`], GIT-P13); this module is the route/command CATALOGUE that the host + the
//! `myelin` CLI binary dispatch over (the surfaces are thin; the handlers are already built).

/// An HTTP method on the Git API surface. The `is_write` classification drives the BUS-2 invariant
/// (every write endpoint is `Id.check`-gated + one-tx outbox-emit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A read endpoint — uses `project`/`resolve` (cell-local) for any cross-subsystem context.
    Get,
    /// A write endpoint — `Id.check` → state-change + `OutboxTx::emit` in ONE transaction (BUS-2).
    Post,
}

impl Method {
    /// Is this a write method (the BUS-2 / `Id.check` gate applies)?
    pub fn is_write(self) -> bool {
        matches!(self, Method::Post)
    }
}

/// The already-built git handler a route/command lowers to. NO new handler is introduced — this enum
/// NAMES the existing module the surface dispatches into (the prompt: "the CLI/API surfaces the
/// existing handlers"). It exists so the catalogue is a typed map from surface → handler, not a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    /// The per-viewer 0-leak projection ([`crate::project::Projector::project`]).
    Project,
    /// The leak-free `list_objects` `SetExpr` push-down ([`crate::list_filter`]).
    ListFilter,
    /// The X-1 `check_status` projection read ([`crate::check_status`]).
    CheckStatus,
    /// The merge gate + durable merge queue ([`crate::merge_gate`] / [`crate::merge_queue`]).
    MergeGate,
    /// The fork-endorsement gate ([`crate::fork_gate`] — `approve_untrusted_ci`).
    ForkEndorse,
    /// The PR/review/thread lifecycle ([`crate::lifecycle`]).
    Lifecycle,
    /// The receive-pack one-tx ref-CAS ([`crate::receive_pack`]) — the web-edit commit lowers here.
    ReceivePack,
    /// The code-search pre-filter ([`crate::list_filter::code_search_pre_filter`]).
    CodeSearch,
    /// A repo-settings / ruleset write ([`crate::lifecycle::BranchProtectionRuleset`]).
    Settings,
}

/// One **HTTP/RPC endpoint** on the Git API surface (arch §4). The catalogue maps `(method, path)` →
/// the already-built [`Handler`]. The BUS-2 invariant is STRUCTURAL: a write endpoint MUST be
/// `id_checked` (the [`Endpoint::new`] constructor enforces it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// The HTTP method.
    pub method: Method,
    /// The route path (`/api/git/repos/{repo}/prs/{n}/merge`).
    pub path: &'static str,
    /// The already-built handler this route dispatches into.
    pub handler: Handler,
    /// `true` iff this route runs `Id.check` before any state change. Enforced `true` for every write
    /// (the BUS-2 / authz gate); reads are `Id.check`-gated too (the projection denies per viewer), so
    /// this is `true` across the catalogue.
    pub id_checked: bool,
}

impl Endpoint {
    /// Construct an endpoint, ENFORCING the BUS-2 invariant: a write endpoint (`POST`) MUST be
    /// `id_checked`. Returns `None` for a write route that is not `Id.check`-gated (the structural
    /// guard — a mutant that registers an un-gated write fails here, not silently).
    pub fn new(
        method: Method,
        path: &'static str,
        handler: Handler,
        id_checked: bool,
    ) -> Option<Endpoint> {
        if method.is_write() && !id_checked {
            // BUS-2: no write endpoint may skip the Id.check → state-change + outbox-emit transaction.
            return None;
        }
        Some(Endpoint {
            method,
            path,
            handler,
            id_checked,
        })
    }
}

/// **The Git HTTP API route catalogue** (arch §4 — the representative endpoints). Every entry maps to
/// an already-built handler; every write is `Id.check`-gated (BUS-2). The catalogue is the typed
/// surface the front-door host (`axum`) + the agent-tool surface dispatch over.
pub fn http_catalogue() -> Vec<Endpoint> {
    // Every `unwrap()` here is safe by construction: writes pass `id_checked = true`. The `new` guard
    // makes a future un-gated write a COMPILE-exercised panic in the test, not a silent leak.
    vec![
        Endpoint::new(Method::Get, "/api/git/repos", Handler::ListFilter, true).unwrap(),
        Endpoint::new(Method::Post, "/api/git/repos", Handler::Lifecycle, true).unwrap(),
        Endpoint::new(
            Method::Get,
            "/api/git/repos/{repo}/prs/{n}",
            Handler::Project,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Get,
            "/api/git/repos/{repo}/prs/{n}/checks",
            Handler::CheckStatus,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/prs",
            Handler::Lifecycle,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/reviews",
            Handler::Lifecycle,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci",
            Handler::ForkEndorse,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/merge",
            Handler::MergeGate,
            true,
        )
        .unwrap(),
        // The repo-owned BRANCH-PROTECTION policy set (GT-003) — an authorized repo-ADMIN write
        // (`Id.check(repo_admin)`); the merge gate's required set + thresholds come from HERE, never
        // from author input. Id.check-gated.
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/branch-protection",
            Handler::Settings,
            true,
        )
        .unwrap(),
        // The CI check-report (GT-003) — the authorized producer that stamps green/fork facts on a PR
        // (the real CI producer is M4; the PR AUTHOR cannot set facts). A WRITE, Id.check-gated.
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/checks",
            Handler::CheckStatus,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Get,
            "/api/git/repos/{repo}/blob/{ref}/{path}",
            Handler::Project,
            true,
        )
        .unwrap(),
        // The single-file web-edit commit (GF-6) lowers to the receive-pack one-tx ref-CAS — a WRITE,
        // Id.check-gated.
        Endpoint::new(
            Method::Post,
            "/api/git/repos/{repo}/blob/{ref}/{path}",
            Handler::ReceivePack,
            true,
        )
        .unwrap(),
        Endpoint::new(
            Method::Get,
            "/api/git/search/code",
            Handler::CodeSearch,
            true,
        )
        .unwrap(),
    ]
}

// ---------------------------------------------------------------------------
// The `myelin …` git CLI command surface (arch §3.2)
// ---------------------------------------------------------------------------

/// A parsed **`myelin …` git CLI command** (arch §3.2 — the thin client over the SAME API the UI +
/// agents use). The CLI noun alias `repo` maps to the canonical `git` ArtifactRef token (arch §3.2
/// note — the alias is render-time only). Each variant lowers to the [`Handler`] its `handler()` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// `myelin git repo list [--limit 1..100] [--cursor <rl1_…>]` — the leak-free lightweight repo
    /// catalogue.
    RepoList {
        /// Optional requested page size; the Edge default applies when absent.
        limit: Option<usize>,
        /// Optional canonical repository-list continuation token.
        cursor: Option<String>,
    },
    /// `myelin repo create <slug>` — create a durable bare repo (the GT-003 `create_repo` write). The
    /// tenant is the token's; the body carries only the `slug`.
    RepoCreate {
        /// The repo slug to create (under the verified tenant).
        slug: String,
    },
    /// `myelin repo view <repo>` — the per-viewer repo projection.
    RepoView {
        /// The repo slug/ref.
        repo: String,
    },
    /// `myelin pr list [--repo <repo>]` — the leak-free PR list.
    PrList {
        /// Optional repo filter.
        repo: Option<String>,
    },
    /// `myelin pr open <repo> [--base <ref>] [--head <ref>] [--head-oid <oid>] [--draft]` — open a PR
    /// (the GT-003 durable `open_pr`). The body carries ONLY the proposal (never branch-protection
    /// policy or check facts — those are repo-owned / producer-set, the GT-003 bypass fix).
    PrOpen {
        /// The repo slug the PR is opened against.
        repo: String,
        /// **The human title (R3.1 — required at create).** A PR without a title is a hollow list row
        /// (ux-git #3); the CLI requires `--title`.
        title: String,
        /// The optional Markdown body (`--body`).
        body: Option<String>,
        /// The base ref (default `refs/heads/main`).
        base_ref: Option<String>,
        /// The head ref (default `refs/heads/feature`).
        head_ref: Option<String>,
        /// The head commit oid the PR proposes (validated against the repo at merge).
        head_oid: Option<String>,
        /// Whether the PR opens as a draft.
        draft: bool,
    },
    /// `myelin pr view <repo> <pr>` — the PR overview projection (the context pane).
    PrView {
        /// The repo slug the PR lives in.
        repo: String,
        /// The PR number.
        number: u64,
    },
    /// `myelin pr checks <repo> <pr>` — the `check_status` projection (per-context state/required/summary).
    PrChecks {
        /// The repo slug the PR lives in.
        repo: String,
        /// The PR number.
        number: u64,
    },
    /// `myelin pr review <repo> <pr> --approve|--request-changes|--comment` — submit a review.
    PrReview {
        /// The repo slug the PR lives in.
        repo: String,
        /// The PR number.
        number: u64,
        /// The verdict (`approve` / `request-changes` / `comment`).
        verdict: String,
    },
    /// `myelin pr merge <repo> <pr> [--squash|--rebase|--merge] [--auto]` — the merge gate + queue.
    PrMerge {
        /// The repo slug the PR lives in.
        repo: String,
        /// The PR number.
        number: u64,
        /// `--auto` = merge-when-green (the durable `ci.result` wait).
        auto: bool,
    },
    /// `myelin pr endorse-fork-ci <repo> <pr>` — the maintainer `approve_untrusted_ci` endorsement (X-1).
    PrEndorseForkCi {
        /// The repo slug the PR lives in.
        repo: String,
        /// The PR number.
        number: u64,
    },
    /// `myelin search code "<query>" [--repo …]` — the ACL-pre-filtered code search.
    SearchCode {
        /// The query string.
        query: String,
        /// Optional repo scope.
        repo: Option<String>,
    },
}

impl CliCommand {
    /// The already-built handler this CLI command lowers to (no new handler — the surface is thin).
    pub fn handler(&self) -> Handler {
        match self {
            CliCommand::RepoList { .. } | CliCommand::PrList { .. } => Handler::ListFilter,
            CliCommand::RepoCreate { .. }
            | CliCommand::PrOpen { .. }
            | CliCommand::PrReview { .. } => Handler::Lifecycle,
            CliCommand::RepoView { .. } | CliCommand::PrView { .. } => Handler::Project,
            CliCommand::PrChecks { .. } => Handler::CheckStatus,
            CliCommand::PrMerge { .. } => Handler::MergeGate,
            CliCommand::PrEndorseForkCi { .. } => Handler::ForkEndorse,
            CliCommand::SearchCode { .. } => Handler::CodeSearch,
        }
    }

    /// Is this a write command (the `Id.check` → state-change + outbox-emit gate applies)?
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            CliCommand::RepoCreate { .. }
                | CliCommand::PrOpen { .. }
                | CliCommand::PrReview { .. }
                | CliCommand::PrMerge { .. }
                | CliCommand::PrEndorseForkCi { .. }
        )
    }
}

/// A loud, typed CLI parse error — an unknown verb / missing arg is NEVER silently coerced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliParseError {
    /// No subcommand given.
    Empty,
    /// An unknown noun/verb at position `idx`.
    Unknown {
        /// The offending token.
        token: String,
    },
    /// A required argument is missing.
    MissingArg {
        /// What argument is missing.
        what: &'static str,
    },
    /// A flag occurred more than once.
    DuplicateFlag {
        /// The duplicated flag.
        flag: &'static str,
    },
    /// A flag was not followed by its required value.
    MissingValue {
        /// The flag requiring a value.
        flag: &'static str,
    },
    /// An argument was malformed (e.g. a non-numeric PR number).
    BadArg {
        /// The argument value that failed to parse.
        value: String,
    },
}

/// Maximum page size accepted by `myelin git repo list`.
pub const REPO_LIST_CLI_MAX_LIMIT: usize = 100;
/// Maximum UTF-8 bytes accepted for one code-search query at every public boundary.
pub const CODE_SEARCH_QUERY_MAX_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 bytes accepted for an optional code-search repository filter.
pub const CODE_SEARCH_REPO_MAX_BYTES: usize = crate::web::REPO_LIST_ROW_MAX_SLUG_BYTES;

/// Validate the shared CLI/Edge code-search query contract. Spaces and other non-control Unicode
/// whitespace are valid inside a meaningful query, while empty/whitespace-only, control-bearing,
/// and oversized inputs fail closed.
pub fn valid_code_search_query(query: &str) -> bool {
    !query.trim().is_empty()
        && query.len() <= CODE_SEARCH_QUERY_MAX_BYTES
        && !query.chars().any(char::is_control)
}

/// Validate the shared CLI/Edge optional repository filter. This reuses the durable resolver's
/// namespaced slug grammar and adds the public 255-byte projection bound.
pub fn valid_code_search_repo(repo: &str) -> bool {
    repo.len() <= CODE_SEARCH_REPO_MAX_BYTES && crate::gix_backend::validate_repo_slug(repo).is_ok()
}

/// Parse a `myelin …` git CLI invocation (the args AFTER `myelin`, arch §3.2). A thin, total parser
/// over the frozen verb grammar — the noun alias `repo` is accepted (render-time alias for `git`). The
/// tenant is injected FROM THE TOKEN by the CLI shell (GIT-D8 — never an arg here), so this parser
/// never reads a tenant from the args.
pub fn parse_cli(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (head, rest) = args.split_first().ok_or(CliParseError::Empty)?;
    match *head {
        "repo" => parse_repo(rest),
        "pr" => parse_pr(rest),
        "search" => parse_search(rest),
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_repo(rest: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, args) = rest
        .split_first()
        .ok_or(CliParseError::MissingArg { what: "repo verb" })?;
    match *verb {
        "list" => parse_repo_list(args),
        "create" => {
            let slug = positional(args, 0).ok_or(CliParseError::MissingArg { what: "slug" })?;
            Ok(CliCommand::RepoCreate {
                slug: slug.to_string(),
            })
        }
        "view" => {
            let repo = positional(args, 0).ok_or(CliParseError::MissingArg { what: "repo" })?;
            Ok(CliCommand::RepoView {
                repo: repo.to_string(),
            })
        }
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_repo_list(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index];
        match flag {
            "--limit" => {
                if limit.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--limit" });
                }
                let value = required_flag_value(args, index, "--limit")?;
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string()
                        && (1..=REPO_LIST_CLI_MAX_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| CliParseError::BadArg {
                    value: value.to_string(),
                })?);
                index += 2;
            }
            "--cursor" => {
                if cursor.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--cursor" });
                }
                let value = required_flag_value(args, index, "--cursor")?;
                crate::web::RepoListCursor::parse(value).map_err(|_| CliParseError::BadArg {
                    value: value.to_string(),
                })?;
                cursor = Some(value.to_string());
                index += 2;
            }
            other => {
                return Err(CliParseError::Unknown {
                    token: other.to_string(),
                })
            }
        }
    }
    Ok(CliCommand::RepoList { limit, cursor })
}

fn required_flag_value<'a>(
    args: &'a [&str],
    index: usize,
    flag: &'static str,
) -> Result<&'a str, CliParseError> {
    args.get(index + 1)
        .copied()
        .filter(|value| !value.starts_with("--"))
        .ok_or(CliParseError::MissingValue { flag })
}

fn parse_pr(rest: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, args) = rest
        .split_first()
        .ok_or(CliParseError::MissingArg { what: "pr verb" })?;
    match *verb {
        // `pr list` keeps its repo as an OPTIONAL `--repo` filter (no per-PR target).
        "list" => {
            let repo = flag_value(args, "--repo");
            Ok(CliCommand::PrList { repo })
        }
        // `pr open <repo> [--base …] [--head …] [--head-oid …] [--draft]` — the repo is the first
        // positional; the rest is the proposal (never policy/facts — the GT-003 bypass fix).
        "open" => {
            let repo = positional(args, 0).ok_or(CliParseError::MissingArg { what: "repo" })?;
            Ok(CliCommand::PrOpen {
                repo: repo.to_string(),
                title: flag_value(args, "--title")
                    .filter(|t| !t.trim().is_empty())
                    .ok_or(CliParseError::MissingArg { what: "title" })?,
                body: flag_value(args, "--body"),
                base_ref: flag_value(args, "--base"),
                head_ref: flag_value(args, "--head"),
                head_oid: flag_value(args, "--head-oid"),
                draft: args.contains(&"--draft"),
            })
        }
        // The per-PR verbs all take `<repo> <number>` (the edge path is /repos/<repo>/prs/<n> — the
        // repo is threaded so the CLI can build the durable route; GT-005).
        "view" => {
            let (repo, number) = repo_and_number(args)?;
            Ok(CliCommand::PrView { repo, number })
        }
        "checks" => {
            let (repo, number) = repo_and_number(args)?;
            Ok(CliCommand::PrChecks { repo, number })
        }
        "review" => {
            let (repo, number) = repo_and_number(args)?;
            let verdict = if args.contains(&"--approve") {
                "approve"
            } else if args.contains(&"--request-changes") {
                "request-changes"
            } else if args.contains(&"--comment") {
                "comment"
            } else {
                return Err(CliParseError::MissingArg {
                    what: "review verdict",
                });
            };
            Ok(CliCommand::PrReview {
                repo,
                number,
                verdict: verdict.to_string(),
            })
        }
        "merge" => {
            let (repo, number) = repo_and_number(args)?;
            let auto = args.contains(&"--auto");
            Ok(CliCommand::PrMerge { repo, number, auto })
        }
        "endorse-fork-ci" => {
            let (repo, number) = repo_and_number(args)?;
            Ok(CliCommand::PrEndorseForkCi { repo, number })
        }
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

/// The `n`th non-flag positional argument (0-indexed), if present.
fn positional<'a>(args: &'a [&str], n: usize) -> Option<&'a str> {
    args.iter().filter(|a| !a.starts_with("--")).nth(n).copied()
}

/// Parse a per-PR target `<repo> <number>` from the positionals: the FIRST positional is the repo
/// slug, the SECOND is the PR number (loud on a missing repo / missing or non-numeric number).
fn repo_and_number(args: &[&str]) -> Result<(String, u64), CliParseError> {
    let repo = positional(args, 0).ok_or(CliParseError::MissingArg { what: "repo" })?;
    let raw = positional(args, 1).ok_or(CliParseError::MissingArg { what: "number" })?;
    let number = raw.parse::<u64>().map_err(|_| CliParseError::BadArg {
        value: raw.to_string(),
    })?;
    Ok((repo.to_string(), number))
}

fn parse_search(rest: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, args) = rest.split_first().ok_or(CliParseError::MissingArg {
        what: "search verb",
    })?;
    match *verb {
        "code" => {
            let mut query = None;
            let mut repo = None;
            let mut index = 0;
            while index < args.len() {
                match args[index] {
                    "--repo" => {
                        if repo.is_some() {
                            return Err(CliParseError::DuplicateFlag { flag: "--repo" });
                        }
                        let value = required_flag_value(args, index, "--repo")?;
                        if !valid_code_search_repo(value) {
                            return Err(CliParseError::BadArg {
                                value: value.to_string(),
                            });
                        }
                        repo = Some(value.to_string());
                        index += 2;
                    }
                    other if other.starts_with("--") => {
                        return Err(CliParseError::Unknown {
                            token: other.to_string(),
                        });
                    }
                    value => {
                        if query.is_some() || !valid_code_search_query(value) {
                            return Err(CliParseError::BadArg {
                                value: value.to_string(),
                            });
                        }
                        query = Some(value.to_string());
                        index += 1;
                    }
                }
            }
            Ok(CliCommand::SearchCode {
                query: query.ok_or(CliParseError::MissingArg { what: "query" })?,
                repo,
            })
        }
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

/// The value following a `--flag` (e.g. `--repo core`), or `None` if absent.
fn flag_value(args: &[&str], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| *a == flag)?;
    args.get(idx + 1).map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// The agent-tool surface (arch §4 — the ToolDef set with frozen requires_approval defaults)
// ---------------------------------------------------------------------------

/// An **agent-tool definition** on the Git surface (arch §4 — the `ToolDef` set). The `requires_approval`
/// default is FROZEN (`git.merge = yes` — the ONLY consequential git gate; `open_pr = no`). Agents act
/// through `EffectApi` (plan-then-apply), the SAME endpoints the UI calls (no carve-out, ADR-08). The
/// thin `ToolDef` REGISTRATION lives in `myelin_agent_service::git_tools` (the §2.9 DAG — git is a
/// leaf); this is the git-side NAME + default the registration keys on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentToolDef {
    /// The frozen tool name (`git.merge`, `git.open_pr`, …).
    pub name: &'static str,
    /// `true` iff the tool requires a HITL approval before `EffectApi::apply` runs (frozen default).
    pub requires_approval: bool,
    /// Capabilities the exact minted/delegated run-token authority must carry. These travel with
    /// the subsystem-owned MCP definition so routing never invents a second permission map.
    pub required_caps: &'static [&'static str],
    /// The handler this tool's effect lowers to (the same already-built handler the UI/CLI route into).
    pub handler: Handler,
}

/// The frozen agent-tool catalogue (arch §4). The `requires_approval` defaults are the recon X-1 /
/// ADR-08 frozen values: `git.merge = yes` (the ONLY consequential git gate), everything else `no`
/// (authoring is reversible → not HITL-gated, §6.3).
pub fn agent_tools() -> Vec<AgentToolDef> {
    vec![
        AgentToolDef {
            name: "git.merge",
            requires_approval: true,
            required_caps: &["pull_request.merge"],
            handler: Handler::MergeGate,
        },
        AgentToolDef {
            name: "git.open_pr",
            requires_approval: false,
            required_caps: &["repo.push"],
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            name: "git.submit_review",
            requires_approval: false,
            required_caps: &["pull_request.review"],
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            name: "git.endorse_fork_ci",
            // Endorsing fork CI is itself permission-gated (approve_untrusted_ci) but reversible at the
            // tool layer; the SECURITY gate is the ABAC capability, not a HITL card (the human-with-cap
            // IS the gate). Not a HITL-default tool.
            requires_approval: false,
            required_caps: &["repo.approve_untrusted_ci"],
            handler: Handler::ForkEndorse,
        },
    ]
}

#[cfg(test)]
mod tests;
