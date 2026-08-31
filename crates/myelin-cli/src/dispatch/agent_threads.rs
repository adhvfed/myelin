use serde_json::json;

use super::agents::{
    canonical_limit, exactly_one_id, flag_value, paginated_call, require_agent_id,
    validate_clean_name,
};
use super::{CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_RETENTION_DAYS: u8 = 3;
const MAX_RETENTION_DAYS: u8 = 30;

pub(super) fn agent_thread_dispatch(
    args: &[&str],
    default_project: Option<&str>,
) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no agent thread command given (try: start <name> --agent <id> | ready <id> | retry <id> | list | show <id> | say <id> <message> | history <id>)"
                .into(),
        )
    })?;
    match *verb {
        "start" => start_call(rest, default_project),
        "ready" => ready_call(rest),
        "retry" => retry_call(rest),
        "list" => list_call(rest),
        "show" => show_call(rest),
        "say" => say_call(rest),
        "history" => history_call(rest),
        other => Err(CliError::Usage(format!(
            "unknown agent thread command token `{other}`"
        ))),
    }
}

fn start_call(args: &[&str], default_project: Option<&str>) -> Result<EdgeCall, CliError> {
    let mut name = None;
    let mut agent = None;
    let mut project = None;
    let mut retention_days = None;
    let mut repository = None;
    let mut base_ref = None;
    let mut branch = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--agent" if agent.is_none() => agent = Some(flag_value(args, &mut index, "--agent")?),
            "--agent" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --agent only once".into(),
                ));
            }
            "--project" if project.is_none() => {
                project = Some(flag_value(args, &mut index, "--project")?);
            }
            "--project" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --project only once".into(),
                ));
            }
            "--retention-days" if retention_days.is_none() => {
                retention_days = Some(flag_value(args, &mut index, "--retention-days")?);
            }
            "--retention-days" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --retention-days only once".into(),
                ));
            }
            "--repository" if repository.is_none() => {
                repository = Some(flag_value(args, &mut index, "--repository")?);
            }
            "--repository" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --repository only once".into(),
                ));
            }
            "--base-ref" if base_ref.is_none() => {
                base_ref = Some(flag_value(args, &mut index, "--base-ref")?);
            }
            "--base-ref" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --base-ref only once".into(),
                ));
            }
            "--branch" if branch.is_none() => {
                branch = Some(flag_value(args, &mut index, "--branch")?);
            }
            "--branch" => {
                return Err(CliError::Usage(
                    "agent thread start accepts --branch only once".into(),
                ));
            }
            token if !token.starts_with('-') && name.is_none() => name = Some(token),
            token if !token.starts_with('-') => {
                return Err(CliError::Usage(format!(
                    "unexpected agent thread start argument `{token}`"
                )));
            }
            flag => {
                return Err(CliError::Usage(format!(
                    "unknown agent thread start flag `{flag}`"
                )));
            }
        }
        index += 1;
    }

    let name = name.ok_or_else(|| CliError::Usage("agent thread start needs a name".into()))?;
    validate_clean_name("agent thread name", name)?;
    let agent = agent
        .ok_or_else(|| CliError::Usage("agent thread start needs --agent <agent_id>".into()))?;
    require_agent_id("agent id", agent)?;
    let project = project.or(default_project);
    if let Some(project) = project {
        if !super::is_canonical_project_id(project) {
            return Err(CliError::Usage(
                "agent thread project must be a canonical lowercase UUID".into(),
            ));
        }
    }
    let retention_days = retention_days
        .map(parse_retention_days)
        .transpose()?
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    let mut payload = json!({
        "name": name,
        "agent_id": agent,
        "retention_days": retention_days,
    });
    if let Some(project) = project {
        payload["project_id"] = json!(project);
    }
    if repository.is_none() && (base_ref.is_some() || branch.is_some()) {
        return Err(CliError::Usage(
            "agent thread --base-ref and --branch require --repository <slug>".into(),
        ));
    }
    if let Some(repository) = repository {
        validate_workspace_text("repository", repository, 255)?;
        let mut workspace = json!({ "repository": repository });
        if let Some(base_ref) = base_ref {
            validate_workspace_text("base ref", base_ref, 1024)?;
            workspace["base_ref"] = json!(base_ref);
        }
        if let Some(branch) = branch {
            validate_workspace_text("branch", branch, 1024)?;
            workspace["branch"] = json!(branch);
        }
        payload["workspace"] = workspace;
    }
    Ok(EdgeCall::post_json("/v1/agent-threads", payload))
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    paginated_call(
        args,
        "agent thread list",
        "agent thread cursor",
        "/v1/agent-threads",
    )
}

fn show_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let id = exactly_one_id(args, "agent thread show", "thread id")?;
    require_agent_id("agent thread id", id)?;
    Ok(EdgeCall::get(format!("/v1/agent-threads/{id}")))
}

fn retry_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let id = exactly_one_id(args, "agent thread retry", "thread id")?;
    require_agent_id("agent thread id", id)?;
    Ok(EdgeCall::post_retry_safe_json(
        format!("/v1/agent-threads/{id}/retry"),
        json!({}),
    ))
}

fn ready_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let id = exactly_one_id(args, "agent thread ready", "thread id")?;
    require_agent_id("agent thread id", id)?;
    Ok(EdgeCall::post_retry_safe_json(
        format!("/v1/agent-threads/{id}/ready"),
        json!({}),
    ))
}

fn say_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [id, content] = args else {
        return Err(CliError::Usage(
            "agent thread say needs exactly one <thread_id> and one quoted <message>".into(),
        ));
    };
    require_agent_id("agent thread id", id)?;
    super::chat::validate_message(content)?;
    Ok(EdgeCall::post_json(
        format!("/v1/agent-threads/{id}/messages"),
        json!({ "content": content }),
    ))
}

fn history_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let Some((id, flags)) = args.split_first() else {
        return Err(CliError::Usage(
            "agent thread history needs a thread id".into(),
        ));
    };
    require_agent_id("agent thread id", id)?;
    let mut limit = None;
    let mut before = None;
    let mut index = 0;
    while index < flags.len() {
        let (slot, flag): (&mut Option<&str>, &str) = match flags[index] {
            "--limit" => (&mut limit, "--limit"),
            "--before" => (&mut before, "--before"),
            token => {
                return Err(CliError::Usage(format!(
                    "unknown agent thread history flag `{token}`"
                )));
            }
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!(
                "duplicate agent thread history flag `{flag}`"
            )));
        }
        *slot = Some(flag_value(flags, &mut index, flag)?);
        index += 1;
    }
    let limit = canonical_limit(limit, "agent thread history")?;
    if let Some(before) = before {
        super::chat::canonical_ulid("agent thread message cursor", before)?;
    }
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(before) = before {
        query.push("before", before);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: format!("/v1/agent-threads/{id}/messages"),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn parse_retention_days(value: &str) -> Result<u8, CliError> {
    value
        .parse::<u8>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .filter(|parsed| (1..=MAX_RETENTION_DAYS).contains(parsed))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "agent thread retention must be an integer between 1 and {MAX_RETENTION_DAYS} days"
            ))
        })
}

fn validate_workspace_text(label: &str, value: &str, maximum: usize) -> Result<(), CliError> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CliError::Usage(format!(
            "agent thread {label} must contain 1..={maximum} clean UTF-8 bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{agent_dispatch, agent_dispatch_with_project};

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const THREAD: &str = "22222222-2222-2222-2222-222222222222";
    const PROJECT: &str = "33333333-3333-3333-3333-333333333333";
    const MESSAGE: &str = "01J00000000000000000000000";

    #[test]
    fn commands_preserve_a_named_problem_and_bounded_workspace() {
        let start = agent_dispatch_with_project(
            &[
                "thread",
                "start",
                "Investigate checkout race",
                "--agent",
                AGENT,
                "--retention-days",
                "7",
            ],
            Some(PROJECT),
        )
        .unwrap();
        assert_eq!(start.path, "/v1/agent-threads");
        assert_eq!(start.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(start.payload.as_ref().unwrap()).unwrap(),
            json!({
                "name": "Investigate checkout race",
                "agent_id": AGENT,
                "project_id": PROJECT,
                "retention_days": 7,
            })
        );

        let without_project =
            agent_dispatch(&["thread", "start", "Private scratchpad", "--agent", AGENT]).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(without_project.payload.as_ref().unwrap())
                .unwrap(),
            json!({
                "name": "Private scratchpad",
                "agent_id": AGENT,
                "retention_days": 3,
            })
        );

        let repository_workspace = agent_dispatch(&[
            "thread",
            "start",
            "Private cloud branch",
            "--agent",
            AGENT,
            "--repository",
            "platform/api",
            "--base-ref",
            "release",
            "--branch",
            "alice/diagnosis",
        ])
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                repository_workspace.payload.as_ref().unwrap()
            )
            .unwrap(),
            json!({
                "name": "Private cloud branch",
                "agent_id": AGENT,
                "retention_days": 3,
                "workspace": {
                    "repository": "platform/api",
                    "base_ref": "release",
                    "branch": "alice/diagnosis",
                },
            })
        );
        assert!(agent_dispatch(&[
            "thread",
            "start",
            "Missing repository",
            "--agent",
            AGENT,
            "--base-ref",
            "main",
        ])
        .is_err());

        let list = agent_dispatch(&["thread", "list", "--limit", "7", "--cursor", THREAD]).unwrap();
        assert_eq!(list.path, "/v1/agent-threads");
        assert_eq!(
            list.query.as_deref(),
            Some("limit=7&cursor=22222222-2222-2222-2222-222222222222")
        );
        assert_eq!(
            agent_dispatch(&["thread", "show", THREAD]).unwrap().path,
            format!("/v1/agent-threads/{THREAD}")
        );
        let retry = agent_dispatch(&["thread", "retry", THREAD]).unwrap();
        assert_eq!(retry.path, format!("/v1/agent-threads/{THREAD}/retry"));
        assert_eq!(retry.retry_policy, RetryPolicy::None);
        assert_eq!(retry.payload.as_deref(), Some(b"{}".as_slice()));

        let ready = agent_dispatch(&["thread", "ready", THREAD]).unwrap();
        assert_eq!(ready.path, format!("/v1/agent-threads/{THREAD}/ready"));
        assert_eq!(ready.retry_policy, RetryPolicy::None);
        assert_eq!(ready.payload.as_deref(), Some(b"{}".as_slice()));

        let say = agent_dispatch(&[
            "thread",
            "say",
            THREAD,
            "Please inspect the final-reader lease.",
        ])
        .unwrap();
        assert_eq!(say.path, format!("/v1/agent-threads/{THREAD}/messages"));
        assert_eq!(say.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(say.payload.as_ref().unwrap()).unwrap(),
            json!({ "content": "Please inspect the final-reader lease." })
        );

        let history = agent_dispatch(&[
            "thread", "history", THREAD, "--limit", "7", "--before", MESSAGE,
        ])
        .unwrap();
        assert_eq!(history.path, format!("/v1/agent-threads/{THREAD}/messages"));
        assert_eq!(
            history.query.as_deref(),
            Some("limit=7&before=01J00000000000000000000000")
        );
    }

    #[test]
    fn malformed_intent_never_reaches_edge() {
        for args in [
            vec!["thread"],
            vec!["thread", "start"],
            vec!["thread", "start", "A", "--agent", "not-an-agent"],
            vec!["thread", "start", " A", "--agent", AGENT],
            vec![
                "thread",
                "start",
                "A",
                "--agent",
                AGENT,
                "--retention-days",
                "0",
            ],
            vec![
                "thread",
                "start",
                "A",
                "--agent",
                AGENT,
                "--retention-days",
                "31",
            ],
            vec!["thread", "list", "--limit", "01"],
            vec!["thread", "list", "--cursor", "not-a-thread"],
            vec!["thread", "show"],
            vec!["thread", "show", "not-a-thread"],
            vec!["thread", "retry"],
            vec!["thread", "retry", "not-a-thread"],
            vec!["thread", "ready"],
            vec!["thread", "ready", "not-a-thread"],
            vec!["thread", "say", THREAD],
            vec!["thread", "say", THREAD, "  "],
            vec!["thread", "history"],
            vec!["thread", "history", THREAD, "--before", "not-a-message"],
        ] {
            assert!(agent_dispatch(&args).is_err(), "accepted {args:?}");
        }
        assert!(agent_dispatch_with_project(
            &["thread", "start", "A", "--agent", AGENT],
            Some("not-a-project")
        )
        .is_err());
    }
}
