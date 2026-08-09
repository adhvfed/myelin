use myelin_agent::is_canonical_tool_name;

use crate::error::CliError;

use super::{EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

pub fn tool_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    match args {
        ["list", rest @ ..] => list_call(rest),
        ["show", name] => show_call(name),
        ["show"] => Err(CliError::Usage(
            "tool show needs a canonical `<subsystem>.<name>`".into(),
        )),
        ["describe", "--mcp"] => Ok(EdgeCall {
            method: HttpMethod::Get,
            path: "/v1/tools".into(),
            query: Some("format=mcp".into()),
            payload: None,
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        }),
        ["describe", ..] => Err(CliError::Usage(
            "tool describe currently requires exactly `--mcp`".into(),
        )),
        [] => Err(CliError::Usage(
            "no tool command given (try: tool list | tool show <name> | tool describe --mcp)"
                .into(),
        )),
        [command, ..] => Err(CliError::Usage(format!(
            "unknown tool command `{command}` (expected list, show, or describe)"
        ))),
    }
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index];
        let slot = match flag {
            "--limit" => &mut limit,
            "--cursor" => &mut cursor,
            other => return Err(CliError::Usage(format!("unknown tool list flag `{other}`"))),
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!(
                "duplicate tool list flag `{flag}`"
            )));
        }
        index += 1;
        *slot = Some(
            args.get(index)
                .copied()
                .ok_or_else(|| CliError::Usage(format!("`{flag}` needs a value")))?,
        );
        index += 1;
    }
    let limit = match limit {
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|parsed| parsed.to_string() == value)
            .filter(|parsed| (1..=MAX_PAGE_LIMIT).contains(parsed))
            .ok_or_else(|| {
                CliError::Usage(
                    "tool list limit must be a canonical integer between 1 and 100".into(),
                )
            })?,
        None => DEFAULT_PAGE_LIMIT,
    };
    if let Some(cursor) = cursor {
        if !is_canonical_tool_cursor(cursor) {
            return Err(CliError::Usage(
                "tool cursor must be canonical `<subsystem>.<name>.v<version>`".into(),
            ));
        }
    }
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/tools".into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn show_call(name: &str) -> Result<EdgeCall, CliError> {
    if !is_canonical_tool_name(name) {
        return Err(CliError::Usage(
            "tool name must be canonical `<subsystem>.<name>`".into(),
        ));
    }
    Ok(EdgeCall::get(format!("/v1/tools/{name}")))
}

pub fn is_canonical_tool_cursor(value: &str) -> bool {
    let Some((name, version)) = value.rsplit_once(".v") else {
        return false;
    };
    is_canonical_tool_name(name)
        && version
            .parse::<u32>()
            .ok()
            .filter(|parsed| *parsed > 0)
            .is_some_and(|parsed| parsed.to_string() == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_commands_project_the_documented_catalogue_grammar() {
        assert_eq!(
            tool_dispatch(&["list"]).unwrap().query.as_deref(),
            Some("limit=50")
        );
        assert_eq!(
            tool_dispatch(&["list", "--cursor", "git.open_pr.v1", "--limit", "7"])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=7&cursor=git.open_pr.v1")
        );
        assert_eq!(
            tool_dispatch(&["show", "issue.create"]).unwrap().path,
            "/v1/tools/issue.create"
        );
        assert_eq!(
            tool_dispatch(&["describe", "--mcp"])
                .unwrap()
                .query
                .as_deref(),
            Some("format=mcp")
        );
    }

    #[test]
    fn malformed_tool_commands_fail_before_transport() {
        for args in [
            vec![],
            vec!["show"],
            vec!["show", "Git.merge"],
            vec!["show", "git.merge", "extra"],
            vec!["list", "--limit", "01"],
            vec!["list", "--limit", "1", "--limit", "2"],
            vec!["list", "--cursor", "git.merge.v0"],
            vec!["describe"],
            vec!["describe", "--json"],
        ] {
            assert!(tool_dispatch(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn tool_cursor_grammar_is_exact() {
        assert!(is_canonical_tool_cursor("git.open_pr.v1"));
        for invalid in [
            "git.open_pr",
            "git.open_pr.v0",
            "git.open_pr.v01",
            "git.open.pr.v1",
            "Git.open_pr.v1",
        ] {
            assert!(!is_canonical_tool_cursor(invalid), "accepted `{invalid}`");
        }
    }
}
