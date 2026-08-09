use std::collections::BTreeSet;

use myelin_agent::is_canonical_tool_name;
use serde_json::json;

use super::{is_canonical_uuid, CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_NAME_BYTES: usize = 80;
const MAX_TOOLS: usize = 128;

pub fn agent_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no agent command given (try: create <name> --tool <name> | list | show <id>)".into(),
        )
    })?;
    match *verb {
        "create" => create_call(rest),
        "list" => list_call(rest),
        "show" => show_call(rest),
        other => Err(CliError::Usage(format!(
            "unknown agent command token `{other}`"
        ))),
    }
}

fn create_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut name = None;
    let mut tools = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--tool" => tools.push(flag_value(args, &mut index, "--tool")?),
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
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(CliError::Usage(format!(
            "agent name must contain 1..={MAX_NAME_BYTES} bytes, without surrounding whitespace or control characters"
        )));
    }
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

    Ok(EdgeCall::post_json(
        "/v1/agents",
        json!({ "name": name, "tools": tools }),
    ))
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let (slot, flag): (&mut Option<&str>, &str) = match args[index] {
            "--limit" => (&mut limit, "--limit"),
            "--cursor" => (&mut cursor, "--cursor"),
            token => {
                return Err(CliError::Usage(format!(
                    "unknown agent list flag `{token}`"
                )))
            }
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!(
                "duplicate agent list flag `{flag}`"
            )));
        }
        *slot = Some(flag_value(args, &mut index, flag)?);
        index += 1;
    }

    let limit = canonical_limit(limit)?;
    if let Some(cursor) = cursor {
        require_agent_id("agent cursor", cursor)?;
    }
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/agents".into(),
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

fn canonical_limit(value: Option<&str>) -> Result<u32, CliError> {
    let Some(value) = value else {
        return Ok(DEFAULT_PAGE_LIMIT);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .filter(|parsed| (1..=MAX_PAGE_LIMIT).contains(parsed))
        .ok_or_else(|| {
            CliError::Usage("agent list limit must be an integer between 1 and 100".into())
        })
}

fn flag_value<'a>(args: &'a [&str], index: &mut usize, flag: &str) -> Result<&'a str, CliError> {
    *index += 1;
    args.get(*index)
        .copied()
        .ok_or_else(|| CliError::Usage(format!("`{flag}` needs a value")))
}

fn require_agent_id(label: &str, value: &str) -> Result<(), CliError> {
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

        let list = agent_dispatch(&["list", "--limit", "7", "--cursor", AGENT]).unwrap();
        assert_eq!(
            list.query.as_deref(),
            Some("limit=7&cursor=11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(
            agent_dispatch(&["show", AGENT]).unwrap().path,
            format!("/v1/agents/{AGENT}")
        );
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
                "--tool",
                "ci.read_run",
                "--tool",
                "ci.read_run",
            ],
            vec!["list", "--limit", "01"],
            vec!["list", "--cursor", "not-an-id"],
            vec!["show"],
            vec!["show", "not-an-id"],
        ] {
            assert!(agent_dispatch(&args).is_err(), "accepted {args:?}");
        }
    }
}
