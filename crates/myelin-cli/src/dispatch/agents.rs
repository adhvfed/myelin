use std::collections::BTreeSet;

use myelin_agent::is_canonical_tool_name;
use serde_json::json;

use super::{is_canonical_uuid, CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_NAME_BYTES: usize = 80;
const MAX_TOOLS: usize = 128;

pub fn agent_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    agent_dispatch_with_project(args, None)
}

pub fn agent_dispatch_with_project(
    args: &[&str],
    default_project: Option<&str>,
) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no agent command given (try: create <name> --tool <name> | list | show <id> | thread start|list|show | approve|reject <gate>)".into(),
        )
    })?;
    match *verb {
        "create" => create_call(rest),
        "list" => list_call(rest),
        "show" => show_call(rest),
        "suspend" => lifecycle_call(rest, "suspend"),
        "resume" => lifecycle_call(rest, "resume"),
        "retire" => lifecycle_call(rest, "retire"),
        "thread" => super::agent_threads::agent_thread_dispatch(rest, default_project),
        "approve" | "reject" => approval_call(rest, verb),
        other => Err(CliError::Usage(format!(
            "unknown agent command token `{other}`"
        ))),
    }
}

pub(super) fn validate_clean_name(label: &str, value: &str) -> Result<(), CliError> {
    if value.is_empty()
        || value.len() > MAX_NAME_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(CliError::Usage(format!(
            "{label} must contain 1..={MAX_NAME_BYTES} bytes, without surrounding whitespace or control characters"
        )))
    } else {
        Ok(())
    }
}

pub(super) fn exactly_one_id<'a>(
    args: &'a [&str],
    command: &str,
    label: &str,
) -> Result<&'a str, CliError> {
    match args {
        [id] => Ok(*id),
        [] => Err(CliError::Usage(format!("{command} needs a {label}"))),
        [_, extra, ..] => Err(CliError::Usage(format!(
            "unexpected {command} argument `{extra}`"
        ))),
    }
}

fn approval_call(args: &[&str], decision: &str) -> Result<EdgeCall, CliError> {
    let gate_id = match args {
        [gate_id] => *gate_id,
        [] => return Err(CliError::Usage(format!("agent {decision} needs a gate id"))),
        [_, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected agent {decision} argument `{extra}`"
            )))
        }
    };
    if !is_canonical_gate_id(gate_id) {
        return Err(CliError::Usage(
            "agent approval gate id must be `gate:` followed by 32 lowercase hex characters".into(),
        ));
    }
    Ok(EdgeCall::post_json(
        format!("/v1/agent-approvals/{gate_id}/decision"),
        json!({ "decision": decision }),
    ))
}

fn is_canonical_gate_id(value: &str) -> bool {
    value.strip_prefix("gate:").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn lifecycle_call(args: &[&str], action: &str) -> Result<EdgeCall, CliError> {
    let id = match args {
        [id] => *id,
        [] => return Err(CliError::Usage(format!("agent {action} needs an id"))),
        [_, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected agent {action} argument `{extra}`"
            )))
        }
    };
    require_agent_id("agent id", id)?;
    Ok(EdgeCall::post_json(
        format!("/v1/agents/{id}/{action}"),
        json!({}),
    ))
}

fn create_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut name = None;
    let mut tools = Vec::new();
    let mut runtime = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--tool" => tools.push(flag_value(args, &mut index, "--tool")?),
            "--runtime" if runtime.is_none() => {
                runtime = Some(flag_value(args, &mut index, "--runtime")?)
            }
            "--runtime" => {
                return Err(CliError::Usage(
                    "agent create accepts --runtime only once".into(),
                ))
            }
            token if !token.starts_with('-') && name.is_none() => name = Some(token),
            token if !token.starts_with('-') => {
                return Err(CliError::Usage(format!(
                    "unexpected agent create argument `{token}`"
                )))
            }
            flag => {
                return Err(CliError::Usage(format!(
                    "unknown agent create flag `{flag}`"
                )))
            }
        }
        index += 1;
    }

    let name = name.ok_or_else(|| CliError::Usage("agent create needs a name".into()))?;
    validate_clean_name("agent name", name)?;
    if tools.is_empty() {
        return Err(CliError::Usage(
            "agent create needs at least one --tool <subsystem.name>".into(),
        ));
    }
    if tools.len() > MAX_TOOLS {
        return Err(CliError::Usage(format!(
            "agent create accepts at most {MAX_TOOLS} tools"
        )));
    }
    let mut distinct = BTreeSet::new();
    for tool in &tools {
        if !is_canonical_tool_name(tool) {
            return Err(CliError::Usage(format!(
                "agent tool `{tool}` must be canonical `<subsystem>.<name>`"
            )));
        }
        if !distinct.insert(*tool) {
            return Err(CliError::Usage(format!(
                "agent tool `{tool}` was selected more than once"
            )));
        }
    }

    if let Some(runtime) = runtime {
        if !matches!(runtime, "external" | "hosted") {
            return Err(CliError::Usage(
                "agent runtime must be `external` or `hosted`".into(),
            ));
        }
    }

    let mut body = json!({ "name": name, "tools": tools });
    if let Some(runtime) = runtime {
        body["runtime"] = json!(runtime);
    }
    Ok(EdgeCall::post_json("/v1/agents", body))
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    paginated_call(args, "agent list", "agent cursor", "/v1/agents")
}

pub(super) fn paginated_call(
    args: &[&str],
    command: &str,
    cursor_label: &str,
    path: &str,
) -> Result<EdgeCall, CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let (slot, flag): (&mut Option<&str>, &str) = match args[index] {
            "--limit" => (&mut limit, "--limit"),
            "--cursor" => (&mut cursor, "--cursor"),
            token => return Err(CliError::Usage(format!("unknown {command} flag `{token}`"))),
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!(
                "duplicate {command} flag `{flag}`"
            )));
        }
        *slot = Some(flag_value(args, &mut index, flag)?);
        index += 1;
    }

    let limit = canonical_limit(limit, command)?;
    if let Some(cursor) = cursor {
        require_agent_id(cursor_label, cursor)?;
    }
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: path.into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn show_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let id = match args {
        [id] => *id,
        [] => return Err(CliError::Usage("agent show needs an id".into())),
        [_, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected agent show argument `{extra}`"
            )))
        }
    };
    require_agent_id("agent id", id)?;
    Ok(EdgeCall::get(format!("/v1/agents/{id}")))
}

pub(super) fn canonical_limit(value: Option<&str>, command: &str) -> Result<u32, CliError> {
    let Some(value) = value else {
        return Ok(DEFAULT_PAGE_LIMIT);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .filter(|parsed| (1..=MAX_PAGE_LIMIT).contains(parsed))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "{command} limit must be an integer between 1 and 100"
            ))
        })
}

pub(super) fn flag_value<'a>(
    args: &'a [&str],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, CliError> {
    *index += 1;
    args.get(*index)
        .copied()
        .ok_or_else(|| CliError::Usage(format!("`{flag}` needs a value")))
}

pub(super) fn require_agent_id(label: &str, value: &str) -> Result<(), CliError> {
    if is_canonical_agent_id(value) {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{label} must be a canonical lowercase UUID"
        )))
    }
}

pub fn is_canonical_agent_id(value: &str) -> bool {
    is_canonical_uuid(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn agent_commands_name_the_identity_and_select_catalogue_tools() {
        let create = agent_dispatch(&[
            "create",
            "Review companion",
            "--tool",
            "ci.read_run",
            "--tool",
            "git.open_pr",
        ])
        .unwrap();
        assert_eq!(create.path, "/v1/agents");
        assert_eq!(create.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(create.payload.as_ref().unwrap()).unwrap(),
            json!({
                "name": "Review companion",
                "tools": ["ci.read_run", "git.open_pr"],
            })
        );

        let hosted = agent_dispatch(&[
            "create",
            "CI fixer",
            "--runtime",
            "hosted",
            "--tool",
            "git.open_pr",
        ])
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(hosted.payload.as_ref().unwrap()).unwrap(),
            json!({
                "name": "CI fixer",
                "runtime": "hosted",
                "tools": ["git.open_pr"],
            })
        );

        let list = agent_dispatch(&["list", "--limit", "7", "--cursor", AGENT]).unwrap();
        assert_eq!(
            list.query.as_deref(),
            Some("limit=7&cursor=11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(
            agent_dispatch(&["show", AGENT]).unwrap().path,
            format!("/v1/agents/{AGENT}")
        );
        for action in ["suspend", "resume", "retire"] {
            let call = agent_dispatch(&[action, AGENT]).unwrap();
            assert_eq!(call.path, format!("/v1/agents/{AGENT}/{action}"));
            assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
        }
    }

    #[test]
    fn malformed_agent_commands_fail_before_transport() {
        for args in [
            vec![],
            vec!["create", "Reviewer"],
            vec!["create", "Reviewer", "--tool", "Git.open_pr"],
            vec![
                "create",
                "Reviewer",
                "--runtime",
                "somewhere",
                "--tool",
                "git.open_pr",
            ],
            vec![
                "create",
                "Reviewer",
                "--runtime",
                "hosted",
                "--runtime",
                "external",
                "--tool",
                "git.open_pr",
            ],
            vec![
                "create",
                "Reviewer",
                "--tool",
                "ci.read_run",
                "--tool",
                "ci.read_run",
            ],
            vec!["list", "--limit", "01"],
            vec!["list", "--cursor", "not-an-id"],
            vec!["show"],
            vec!["show", "not-an-id"],
            vec!["suspend"],
            vec!["resume", "not-an-id"],
            vec!["retire", AGENT, "again"],
        ] {
            assert!(agent_dispatch(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn a_human_can_decide_the_exact_gate_copied_from_the_inbox() {
        let gate = "gate:0123456789abcdef0123456789abcdef";
        for decision in ["approve", "reject"] {
            let call = agent_dispatch(&[decision, gate]).unwrap();
            assert_eq!(call.path, format!("/v1/agent-approvals/{gate}/decision"));
            assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(call.payload.as_ref().unwrap())
                    .unwrap(),
                json!({ "decision": decision })
            );
        }
        assert!(agent_dispatch(&["approve", "gate:$(touch bad)"]).is_err());
    }
}
