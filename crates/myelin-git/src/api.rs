#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn is_write(self) -> bool {
        matches!(self, Method::Post)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    Project,
    ListFilter,
    CheckStatus,
    MergeGate,
    ForkEndorse,
    Lifecycle,
    ReceivePack,
    CodeSearch,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub method: Method,
    pub path: &'static str,
    pub handler: Handler,
    id_checked: bool,
}

impl Endpoint {
    pub const fn checked(method: Method, path: &'static str, handler: Handler) -> Endpoint {
        Endpoint {
            method,
            path,
            handler,
            id_checked: true,
        }
    }

    pub fn new(
        method: Method,
        path: &'static str,
        handler: Handler,
        id_checked: bool,
    ) -> Option<Endpoint> {
        if method.is_write() && !id_checked {
            return None;
        }
        Some(Endpoint {
            method,
            path,
            handler,
            id_checked,
        })
    }

    pub const fn is_id_checked(&self) -> bool {
        self.id_checked
    }
}

pub fn http_catalogue() -> Vec<Endpoint> {
    vec![
        Endpoint::checked(Method::Get, "/api/git/repos", Handler::ListFilter),
        Endpoint::checked(Method::Post, "/api/git/repos", Handler::Lifecycle),
        Endpoint::checked(
            Method::Get,
            "/api/git/repos/{repo}/prs/{n}",
            Handler::Project,
        ),
        Endpoint::checked(
            Method::Get,
            "/api/git/repos/{repo}/prs/{n}/checks",
            Handler::CheckStatus,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/prs",
            Handler::Lifecycle,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/reviews",
            Handler::Lifecycle,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci",
            Handler::ForkEndorse,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/merge",
            Handler::MergeGate,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/branch-protection",
            Handler::Settings,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/prs/{n}/checks",
            Handler::CheckStatus,
        ),
        Endpoint::checked(
            Method::Get,
            "/api/git/repos/{repo}/blob/{ref}/{path}",
            Handler::Project,
        ),
        Endpoint::checked(
            Method::Get,
            "/api/git/repos/{repo}/blame/{ref}/{path}",
            Handler::Project,
        ),
        Endpoint::checked(
            Method::Post,
            "/api/git/repos/{repo}/blob/{ref}/{path}",
            Handler::ReceivePack,
        ),
        Endpoint::checked(Method::Get, "/api/git/search/code", Handler::CodeSearch),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    RepoList {
        limit: Option<usize>,
        cursor: Option<String>,
    },
    RepoCreate {
        slug: String,
    },
    RepoView {
        repo: String,
    },
    PrList {
        repo: Option<String>,
    },
    PrOpen {
        repo: String,
        title: String,
        body: Option<String>,
        base_ref: Option<String>,
        head_ref: Option<String>,
        head_oid: Option<String>,
        draft: bool,
    },
    PrView {
        repo: String,
        number: u64,
    },
    PrChecks {
        repo: String,
        number: u64,
    },
    PrReview {
        repo: String,
        number: u64,
        verdict: String,
    },
    PrMerge {
        repo: String,
        number: u64,
        auto: bool,
    },
    PrEndorseForkCi {
        repo: String,
        number: u64,
    },
    SearchCode {
        query: String,
        repo: Option<String>,
    },
}

impl CliCommand {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliParseError {
    Empty,
    Unknown { token: String },
    MissingArg { what: &'static str },
    DuplicateFlag { flag: &'static str },
    MissingValue { flag: &'static str },
    BadArg { value: String },
}

pub const REPO_LIST_CLI_MAX_LIMIT: usize = 100;
pub const CODE_SEARCH_QUERY_MAX_BYTES: usize = 4 * 1024;
pub const CODE_SEARCH_REPO_MAX_BYTES: usize = crate::web::REPO_LIST_ROW_MAX_SLUG_BYTES;

pub fn valid_code_search_query(query: &str) -> bool {
    !query.trim().is_empty()
        && query.len() <= CODE_SEARCH_QUERY_MAX_BYTES
        && !query.chars().any(char::is_control)
}

pub fn valid_code_search_repo(repo: &str) -> bool {
    repo.len() <= CODE_SEARCH_REPO_MAX_BYTES
        && crate::coordinate::RepositorySlug::parse(repo).is_ok()
}

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
                    value == parsed.to_string() && (1..=REPO_LIST_CLI_MAX_LIMIT).contains(parsed)
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
        "list" => {
            let repo = flag_value(args, "--repo");
            Ok(CliCommand::PrList { repo })
        }
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

fn positional<'a>(args: &'a [&str], n: usize) -> Option<&'a str> {
    args.iter().filter(|a| !a.starts_with("--")).nth(n).copied()
}

fn repo_and_number(args: &[&str]) -> Result<(String, u64), CliParseError> {
    let repo = positional(args, 0).ok_or(CliParseError::MissingArg { what: "repo" })?;
    let raw = positional(args, 1).ok_or(CliParseError::MissingArg { what: "number" })?;
    let number =
        crate::coordinate::parse_pull_request_number(raw).ok_or_else(|| CliParseError::BadArg {
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

fn flag_value(args: &[&str], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| *a == flag)?;
    args.get(idx + 1).map(|s| s.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentToolDef {
    local_name: &'static str,
    pub input_schema: &'static str,
    pub requires_approval: bool,
    pub required_caps: &'static [&'static str],
    pub handler: Handler,
}

impl AgentToolDef {
    pub fn canonical_name(&self) -> String {
        format!("git.{}", self.local_name)
    }

    pub fn to_tool_def(self) -> myelin_agent::ToolDef {
        myelin_agent::ToolDef {
            name: myelin_agent::ToolName(self.local_name.to_string()),
            subsystem: "git".to_string(),
            version: 1,
            input_schema: self.input_schema.into(),
            required_caps: self
                .required_caps
                .iter()
                .map(|cap| (*cap).to_string())
                .collect(),
            effect_kind: myelin_agent::EffectKind::Mutate,
            side_effecting: true,
            requires_approval: self.requires_approval,
            exposed_over_mcp: true,
        }
    }
}

pub fn agent_tools() -> Vec<AgentToolDef> {
    vec![
        AgentToolDef {
            local_name: "merge",
            input_schema: r#"{"type":"object","required":["repo","number"],"properties":{"repo":{"type":"string","description":"the repository slug; tenant and region come from the verified run token"},"number":{"type":"integer","minimum":1}},"additionalProperties":false}"#,
            requires_approval: true,
            required_caps: &["pull_request.merge"],
            handler: Handler::MergeGate,
        },
        AgentToolDef {
            local_name: "open_pr",
            input_schema: r#"{"type":"object","required":["repo","title"],"properties":{"repo":{"type":"string","description":"the target repository slug; tenant and region come from the verified run token"},"title":{"type":"string"},"head_ref":{"type":"string"},"base_ref":{"type":"string"},"head_repo":{"type":"string"},"head_oid":{"type":"string"},"body_md":{"type":"string"},"draft":{"type":"boolean"},"reviewers":{"type":"array","items":{"type":"string"},"maxItems":100}},"additionalProperties":false}"#,
            requires_approval: false,
            required_caps: &["repo.push"],
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            local_name: "write_file",
            input_schema: r#"{"type":"object","required":["repo","ref","path","contents","base_oid"],"properties":{"repo":{"type":"string","description":"the repository slug; tenant and region come from the verified run token"},"ref":{"type":"string","description":"the branch to update or create"},"path":{"type":"string","minLength":1,"maxLength":4096},"contents":{"type":"string","maxLength":1048576},"base_oid":{"type":"string","description":"the blob OID read before editing; use an empty string only when the file does not exist"},"start_ref":{"type":"string","description":"the existing branch used as the first parent when creating ref"}},"additionalProperties":false}"#,
            requires_approval: false,
            required_caps: &["repo.push"],
            handler: Handler::ReceivePack,
        },
        AgentToolDef {
            local_name: "submit_review",
            input_schema: r#"{"type":"object","required":["repo","number","verdict"],"properties":{"repo":{"type":"string","description":"the repository slug; tenant and region come from the verified run token"},"number":{"type":"integer","minimum":1},"verdict":{"type":"string","enum":["approve","request_changes","comment"]}},"additionalProperties":false}"#,
            requires_approval: false,
            required_caps: &["pull_request.review"],
            handler: Handler::Lifecycle,
        },
        AgentToolDef {
            local_name: "endorse_fork_ci",
            input_schema: r#"{"type":"object","required":["repo","number"],"properties":{"repo":{"type":"string","description":"the repository slug; tenant and region come from the verified run token"},"number":{"type":"integer","minimum":1},"contexts":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}"#,
            requires_approval: false,
            required_caps: &["repo.approve_untrusted_ci"],
            handler: Handler::ForkEndorse,
        },
    ]
}

pub fn agent_tool_defs() -> Vec<myelin_agent::ToolDef> {
    agent_tools()
        .into_iter()
        .map(AgentToolDef::to_tool_def)
        .collect()
}

#[cfg(test)]
mod tests;
