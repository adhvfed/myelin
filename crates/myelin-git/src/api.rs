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
//! - `repo list` / `GET /api/git/repos` → [`crate::list_filter`] (the leak-free `SetExpr` push-down);
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
    /// `myelin repo list` — the leak-free repo list (the `SetExpr` push-down).
    RepoList,
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
    /// `myelin pr view <pr>` — the PR overview projection (the context pane).
    PrView {
        /// The PR number.
        number: u64,
    },
    /// `myelin pr checks <pr>` — the `check_status` projection (per-context state/required/summary).
    PrChecks {
        /// The PR number.
        number: u64,
    },
    /// `myelin pr review <pr> --approve|--request-changes|--comment` — submit a review.
    PrReview {
        /// The PR number.
        number: u64,
        /// The verdict (`approve` / `request-changes` / `comment`).
        verdict: String,
    },
    /// `myelin pr merge <pr> [--squash|--rebase|--merge] [--auto]` — the merge gate + queue.
    PrMerge {
        /// The PR number.
        number: u64,
        /// `--auto` = merge-when-green (the durable `ci.result` wait).
        auto: bool,
    },
    /// `myelin pr endorse-fork-ci <pr>` — the maintainer `approve_untrusted_ci` endorsement (X-1).
    PrEndorseForkCi {
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
            CliCommand::RepoList | CliCommand::PrList { .. } => Handler::ListFilter,
            CliCommand::RepoView { .. } | CliCommand::PrView { .. } => Handler::Project,
            CliCommand::PrChecks { .. } => Handler::CheckStatus,
            CliCommand::PrReview { .. } => Handler::Lifecycle,
            CliCommand::PrMerge { .. } => Handler::MergeGate,
            CliCommand::PrEndorseForkCi { .. } => Handler::ForkEndorse,
            CliCommand::SearchCode { .. } => Handler::CodeSearch,
        }
    }

    /// Is this a write command (the `Id.check` → state-change + outbox-emit gate applies)?
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            CliCommand::PrReview { .. }
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
    /// An argument was malformed (e.g. a non-numeric PR number).
    BadArg {
        /// The argument value that failed to parse.
        value: String,
    },
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
        "list" => Ok(CliCommand::RepoList),
        "view" => {
            let repo = args
                .first()
                .ok_or(CliParseError::MissingArg { what: "repo" })?;
            Ok(CliCommand::RepoView {
                repo: repo.to_string(),
            })
        }
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_pr(rest: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, args) = rest
        .split_first()
        .ok_or(CliParseError::MissingArg { what: "pr verb" })?;
    match *verb {
        "list" => {
            let repo = flag_value(args, "--repo");
            Ok(CliCommand::PrList { repo })
        }
        "view" => Ok(CliCommand::PrView {
            number: parse_number(args)?,
        }),
        "checks" => Ok(CliCommand::PrChecks {
            number: parse_number(args)?,
        }),
        "review" => {
            let number = parse_number(args)?;
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
                number,
                verdict: verdict.to_string(),
            })
        }
        "merge" => {
            let number = parse_number(args)?;
            let auto = args.contains(&"--auto");
            Ok(CliCommand::PrMerge { number, auto })
        }
        "endorse-fork-ci" => Ok(CliCommand::PrEndorseForkCi {
            number: parse_number(args)?,
        }),
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_search(rest: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, args) = rest.split_first().ok_or(CliParseError::MissingArg {
        what: "search verb",
    })?;
    match *verb {
        "code" => {
            let query = args
                .iter()
                .find(|a| !a.starts_with("--"))
                .ok_or(CliParseError::MissingArg { what: "query" })?;
            let repo = flag_value(args, "--repo");
            Ok(CliCommand::SearchCode {
                query: query.to_string(),
                repo,
            })
        }
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

/// The first non-flag positional argument parsed as a PR number (loud on a non-numeric value).
fn parse_number(args: &[&str]) -> Result<u64, CliParseError> {
    let raw = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or(CliParseError::MissingArg { what: "number" })?;
    raw.parse::<u64>().map_err(|_| CliParseError::BadArg {
        value: raw.to_string(),
    })
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
            handler: Handler::MergeGate,
        },
        AgentToolDef {
            name: "git.open_pr",
            requires_approval: false,
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            name: "git.submit_review",
            requires_approval: false,
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            name: "git.endorse_fork_ci",
            // Endorsing fork CI is itself permission-gated (approve_untrusted_ci) but reversible at the
            // tool layer; the SECURITY gate is the ABAC capability, not a HITL card (the human-with-cap
            // IS the gate). Not a HITL-default tool.
            requires_approval: false,
            handler: Handler::ForkEndorse,
        },
    ]
}

#[cfg(test)]
mod tests;
