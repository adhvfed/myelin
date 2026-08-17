use std::collections::BTreeSet;

use serde_json::json;

use super::{is_canonical_uuid, CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_CAVEATS: usize = 128;
const MAX_CAVEAT_BYTES: usize = 255;
const MAX_BUDGET_MINOR_UNITS: u64 = 1_000_000_000_000;
const MAX_FIRINGS: u64 = 1_000_000;
const MAX_CAUSAL_DEPTH: u32 = 64;
const MAX_FILTER_BYTES: usize = 4 * 1024;

pub fn automation_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no automation command given (try: create | list | show | history | result | erase-result | approve | reject | pause | resume | disable)"
                .into(),
        )
    })?;
    match *verb {
        "create" => create_call(rest),
        "list" => list_call(rest),
        "show" => show_call(rest),
        "history" => history_call(rest),
        "result" => result_call(rest),
        "erase-result" => erase_result_call(rest),
        "approve" | "reject" => approval_call(rest, verb),
        "pause" | "resume" | "disable" => lifecycle_call(rest, verb),
        other => Err(CliError::Usage(format!(
            "unknown automation command token `{other}`"
        ))),
    }
}

#[derive(Default)]
struct CreateOptions<'a> {
    event_type: Option<&'a str>,
    subject_type: Option<&'a str>,
    repository: Option<&'a str>,
    source_branch: Option<&'a str>,
    filter: Option<&'a str>,
    run_as_agent_id: Option<&'a str>,
    task: Option<&'a str>,
    budget_minor_units: Option<&'a str>,
    max_firings: Option<&'a str>,
    max_causal_depth: Option<&'a str>,
    caveats: Vec<&'a str>,
    allow_personal_data: bool,
    require_human_approval: bool,
}

fn create_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut options = CreateOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--event" => singleton_value(&mut options.event_type, args, &mut index, "--event")?,
            "--subject-type" => singleton_value(
                &mut options.subject_type,
                args,
                &mut index,
                "--subject-type",
            )?,
            "--repo" => singleton_value(&mut options.repository, args, &mut index, "--repo")?,
            "--branch" => {
                singleton_value(&mut options.source_branch, args, &mut index, "--branch")?
            }
            "--where" => singleton_value(&mut options.filter, args, &mut index, "--where")?,
            "--run-as" => {
                singleton_value(&mut options.run_as_agent_id, args, &mut index, "--run-as")?
            }
            "--task" => singleton_value(&mut options.task, args, &mut index, "--task")?,
            "--budget-minor-units" => singleton_value(
                &mut options.budget_minor_units,
                args,
                &mut index,
                "--budget-minor-units",
            )?,
            "--max-firings" => {
                singleton_value(&mut options.max_firings, args, &mut index, "--max-firings")?
            }
            "--max-causal-depth" => singleton_value(
                &mut options.max_causal_depth,
                args,
                &mut index,
                "--max-causal-depth",
            )?,
            "--caveat" => options
                .caveats
                .push(flag_value(args, &mut index, "--caveat")?),
            "--allow-personal-data" if !options.allow_personal_data => {
                options.allow_personal_data = true
            }
            "--require-human-approval" if !options.require_human_approval => {
                options.require_human_approval = true
            }
            "--allow-personal-data" | "--require-human-approval" => {
                return Err(CliError::Usage(format!(
                    "duplicate automation create flag `{}`",
                    args[index]
                )))
            }
            token => {
                return Err(CliError::Usage(format!(
                    "unknown automation create argument `{token}`"
                )))
            }
        }
        index += 1;
    }

    let event_type = required(options.event_type, "--event")?;
    myelin_events::validate_event_type(event_type).map_err(|_| {
        CliError::Usage("--event must be a canonical registered Myelin event name".into())
    })?;
    resolve_subject_type(event_type, options.subject_type)?;
    if let Some(repository) = options.repository {
        validate_repository_scope(event_type, repository)?;
    }
    let run_as_agent_id = required(options.run_as_agent_id, "--run-as")?;
    require_automation_id("--run-as", run_as_agent_id)?;
    let task = required(options.task, "--task")?;
    myelin_agent::validate_automation_task(task)
        .map_err(|error| CliError::Usage(format!("invalid --task: {error}")))?;
    let budget_minor_units = bounded_u64(
        required(options.budget_minor_units, "--budget-minor-units")?,
        "--budget-minor-units",
        1,
        MAX_BUDGET_MINOR_UNITS,
    )?;
    let max_firings = bounded_u64(
        required(options.max_firings, "--max-firings")?,
        "--max-firings",
        1,
        MAX_FIRINGS,
    )?;
    let max_causal_depth = options
        .max_causal_depth
        .map(|value| bounded_u32(value, "--max-causal-depth", 0, MAX_CAUSAL_DEPTH))
        .transpose()?;
    validate_caveats(&options.caveats)?;
    if let Some(branch) = options.source_branch {
        validate_branch(branch)?;
    }
    if let Some(filter) = options.filter {
        validate_filter(filter)?;
    }

    let mut body = json!({
        "event_type": event_type,
        "run_as_agent_id": run_as_agent_id,
        "task": task,
        "budget_minor_units": budget_minor_units,
        "max_firings": max_firings,
        "delegation_caveats": options.caveats,
        "require_no_personal_data": !options.allow_personal_data,
        "require_human_approval": options.require_human_approval,
    });
    if let Some(branch) = options.source_branch {
        body["source_branch"] = json!(branch);
    }
    if let Some(repository) = options.repository {
        body["repository"] = json!(repository);
    }
    if let Some(filter) = options.filter {
        body["filter"] = json!(filter);
    }
    if let Some(subject_type) = options.subject_type {
        body["subject_type"] = json!(subject_type);
    }
    if let Some(depth) = max_causal_depth {
        body["max_causal_depth"] = json!(depth);
    }
    Ok(EdgeCall::post_json("/v1/triggers", body))
}

fn resolve_subject_type(event_type: &str, explicit: Option<&str>) -> Result<String, CliError> {
    myelin_events::resolve_automation_subject_type(event_type, explicit)
        .map_err(|error| CliError::Usage(error.to_string()))
}

fn validate_filter(filter: &str) -> Result<(), CliError> {
    if filter.is_empty() || filter.len() > MAX_FILTER_BYTES || filter.trim() != filter {
        return Err(CliError::Usage(format!(
            "--where must contain 1..={MAX_FILTER_BYTES} bytes without surrounding whitespace"
        )));
    }
    myelin_query::parse_query(filter)
        .map(|_| ())
        .map_err(|error| CliError::Usage(format!("--where is not a valid Myelin query: {error}")))
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (limit, cursor) = page_options(args, true)?;
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/triggers".into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn show_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let id = exact_id(args, "show")?;
    Ok(EdgeCall::get(format!("/v1/triggers/{id}")))
}

fn history_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (id, rest) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("automation history needs an id".into()))?;
    require_automation_id("automation id", id)?;
    let (limit, cursor) = page_options(rest, false)?;
    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: format!("/v1/triggers/{id}/firings"),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn result_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [automation_id, run_id] = args else {
        return Err(CliError::Usage(
            "automation result needs an automation id and run id".into(),
        ));
    };
    require_automation_id("automation id", automation_id)?;
    require_automation_id("run id", run_id)?;
    Ok(EdgeCall::get(format!(
        "/v1/triggers/{automation_id}/runs/{run_id}/result"
    )))
}

fn erase_result_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [automation_id, run_id] = args else {
        return Err(CliError::Usage(
            "automation erase-result needs an automation id and run id".into(),
        ));
    };
    require_automation_id("automation id", automation_id)?;
    require_automation_id("run id", run_id)?;
    Ok(EdgeCall::post_json(
        format!("/v1/triggers/{automation_id}/runs/{run_id}/result/erase"),
        json!({}),
    ))
}

fn lifecycle_call(args: &[&str], action: &str) -> Result<EdgeCall, CliError> {
    let id = exact_id(args, action)?;
    Ok(EdgeCall::post_json(
        format!("/v1/triggers/{id}/{action}"),
        json!({}),
    ))
}

fn approval_call(args: &[&str], action: &str) -> Result<EdgeCall, CliError> {
    let (id, event_id) = match args {
        [id, event_id] => (*id, *event_id),
        [] | [_] => {
            return Err(CliError::Usage(format!(
                "automation {action} needs an automation id and event id"
            )))
        }
        [_, _, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected automation {action} argument `{extra}`"
            )))
        }
    };
    require_automation_id("automation id", id)?;
    if event_id.is_empty()
        || event_id.len() > 255
        || event_id.trim() != event_id
        || event_id.chars().any(char::is_control)
    {
        return Err(CliError::Usage(
            "event id must contain 1..=255 bytes without surrounding whitespace or control characters"
                .into(),
        ));
    }
    Ok(EdgeCall::post_json(
        format!("/v1/triggers/{id}/firings/{action}"),
        json!({ "event_id": event_id }),
    ))
}

fn exact_id<'a>(args: &'a [&str], action: &str) -> Result<&'a str, CliError> {
    let id = match args {
        [id] => *id,
        [] => return Err(CliError::Usage(format!("automation {action} needs an id"))),
        [_, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected automation {action} argument `{extra}`"
            )))
        }
    };
    require_automation_id("automation id", id)?;
    Ok(id)
}

fn page_options<'a>(
    args: &'a [&str],
    uuid_cursor: bool,
) -> Result<(u32, Option<&'a str>), CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--limit" => singleton_value(&mut limit, args, &mut index, "--limit")?,
            "--cursor" => singleton_value(&mut cursor, args, &mut index, "--cursor")?,
            token => {
                return Err(CliError::Usage(format!(
                    "unknown automation page flag `{token}`"
                )))
            }
        }
        index += 1;
    }
    let limit = limit
        .map(|value| bounded_u32(value, "--limit", 1, MAX_PAGE_LIMIT))
        .transpose()?
        .unwrap_or(DEFAULT_PAGE_LIMIT);
    if let Some(cursor) = cursor {
        if uuid_cursor {
            require_automation_id("automation cursor", cursor)?;
        } else if cursor.is_empty()
            || cursor.len() > 255
            || cursor.trim() != cursor
            || cursor.chars().any(char::is_control)
        {
            return Err(CliError::Usage(
                "automation history cursor must be a bounded event id".into(),
            ));
        }
    }
    Ok((limit, cursor))
}

fn singleton_value<'a>(
    slot: &mut Option<&'a str>,
    args: &'a [&str],
    index: &mut usize,
    flag: &str,
) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::Usage(format!(
            "automation command accepts `{flag}` only once"
        )));
    }
    *slot = Some(flag_value(args, index, flag)?);
    Ok(())
}

fn flag_value<'a>(args: &'a [&str], index: &mut usize, flag: &str) -> Result<&'a str, CliError> {
    *index += 1;
    args.get(*index)
        .copied()
        .ok_or_else(|| CliError::Usage(format!("`{flag}` needs a value")))
}

fn required<'a>(value: Option<&'a str>, flag: &str) -> Result<&'a str, CliError> {
    value.ok_or_else(|| CliError::Usage(format!("automation create needs `{flag}`")))
}

fn bounded_u64(value: &str, flag: &str, min: u64, max: u64) -> Result<u64, CliError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .filter(|parsed| (min..=max).contains(parsed))
        .ok_or_else(|| CliError::Usage(format!("`{flag}` must be an integer from {min} to {max}")))
}

fn bounded_u32(value: &str, flag: &str, min: u32, max: u32) -> Result<u32, CliError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .filter(|parsed| (min..=max).contains(parsed))
        .ok_or_else(|| CliError::Usage(format!("`{flag}` must be an integer from {min} to {max}")))
}

fn validate_caveats(caveats: &[&str]) -> Result<(), CliError> {
    if caveats.len() > MAX_CAVEATS {
        return Err(CliError::Usage(format!(
            "automation create accepts at most {MAX_CAVEATS} caveats"
        )));
    }
    let mut distinct = BTreeSet::new();
    for caveat in caveats {
        if caveat.is_empty()
            || caveat.len() > MAX_CAVEAT_BYTES
            || caveat.chars().any(char::is_control)
        {
            return Err(CliError::Usage(format!(
                "automation caveats must contain 1..={MAX_CAVEAT_BYTES} bytes without control characters"
            )));
        }
        if !distinct.insert(*caveat) {
            return Err(CliError::Usage(format!(
                "automation caveat `{caveat}` was selected more than once"
            )));
        }
        let repository_scope = caveat
            .strip_prefix("repo:")
            .is_some_and(|repo| myelin_git::coordinate::RepositorySlug::parse(repo).is_ok());
        let capability = caveat.split('.').count() >= 2
            && caveat.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            });
        if !repository_scope && !capability {
            return Err(CliError::Usage(format!(
                "automation caveat `{caveat}` must be a canonical capability such as `issue.create` or a `repo:<slug>` scope"
            )));
        }
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), CliError> {
    let reference = if branch.starts_with("refs/heads/") {
        branch.to_string()
    } else if branch.starts_with("refs/") {
        return Err(CliError::Usage(
            "--branch must name a branch, not another kind of ref".into(),
        ));
    } else {
        format!("refs/heads/{branch}")
    };
    if myelin_git::receive_pack::RefName::new(&reference)
        .validate()
        .is_err()
    {
        return Err(CliError::Usage(
            "--branch must be a canonical Git branch name".into(),
        ));
    }
    Ok(())
}

fn validate_repository_scope(event_type: &str, repository: &str) -> Result<(), CliError> {
    if myelin_git::coordinate::RepositorySlug::parse(repository).is_err() {
        return Err(CliError::Usage(
            "--repo must be a bounded canonical repository slug".into(),
        ));
    }
    if !event_type.starts_with("ci.run.") && event_type != "git.ref.updated" {
        return Err(CliError::Usage(
            "--repo is supported for CI run and Git ref automations".into(),
        ));
    }
    Ok(())
}

fn require_automation_id(label: &str, value: &str) -> Result<(), CliError> {
    if is_canonical_automation_id(value) {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{label} must be a canonical lowercase UUID"
        )))
    }
}

pub fn is_canonical_automation_id(value: &str) -> bool {
    is_canonical_uuid(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "11111111-1111-4111-8111-111111111111";
    const AUTOMATION: &str = "22222222-2222-4222-8222-222222222222";
    const RUN: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn one_command_captures_a_governed_automation_intent() {
        let call = automation_dispatch(&[
            "create",
            "--event",
            "ci.run.failed",
            "--repo",
            "platform/api",
            "--branch",
            "main",
            "--where",
            "payload.retryable == true",
            "--run-as",
            AGENT,
            "--task",
            "Triage the failure and open one issue.",
            "--budget-minor-units",
            "250000",
            "--max-firings",
            "10",
            "--max-causal-depth",
            "4",
            "--caveat",
            "repo:core",
            "--require-human-approval",
        ])
        .unwrap();

        assert_eq!(call.path, "/v1/triggers");
        assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(call.payload.as_ref().unwrap()).unwrap(),
            json!({
                "event_type": "ci.run.failed",
                "repository": "platform/api",
                "source_branch": "main",
                "filter": "payload.retryable == true",
                "run_as_agent_id": AGENT,
                "task": "Triage the failure and open one issue.",
                "budget_minor_units": 250000,
                "max_firings": 10,
                "max_causal_depth": 4,
                "delegation_caveats": ["repo:core"],
                "require_no_personal_data": true,
                "require_human_approval": true,
            })
        );
    }

    #[test]
    fn event_subjects_are_inferred_when_unambiguous_and_explicit_when_not() {
        assert_eq!(
            resolve_subject_type("issue.issue.updated", None).unwrap(),
            "issue"
        );
        assert_eq!(
            resolve_subject_type("ci.result", Some("run")).unwrap(),
            "run"
        );
        assert!(resolve_subject_type("ci.result", None).is_err());
        assert!(resolve_subject_type("issue.issue.updated", Some("run")).is_err());
        assert!(resolve_subject_type("ci.deployment.finished", None).is_err());

        let mut args = vec![
            "create",
            "--event",
            "ci.result",
            "--subject-type",
            "run",
            "--run-as",
            AGENT,
            "--task",
            "Summarize the completed rollup.",
            "--budget-minor-units",
            "1",
            "--max-firings",
            "1",
        ];
        let call = automation_dispatch(&args).unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(call.payload.as_ref().unwrap()).unwrap();
        assert_eq!(body["subject_type"], "run");

        args.drain(3..5);
        assert!(automation_dispatch(&args).is_err());
    }

    #[test]
    fn repository_scope_is_bounded_and_limited_to_repository_events() {
        assert!(validate_repository_scope("ci.run.failed", "platform/api").is_ok());
        assert!(validate_repository_scope("git.ref.updated", "platform/api").is_ok());
        assert!(validate_repository_scope("ci.run.failed", "../payroll").is_err());
        assert!(validate_repository_scope("issue.issue.updated", "platform/api").is_err());
    }

    #[test]
    fn automation_operations_are_addressable_pageable_and_retry_safe() {
        assert_eq!(
            automation_dispatch(&["show", AUTOMATION]).unwrap().path,
            format!("/v1/triggers/{AUTOMATION}")
        );
        assert_eq!(
            automation_dispatch(&["list", "--limit", "7", "--cursor", AUTOMATION])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=7&cursor=22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(
            automation_dispatch(&["history", AUTOMATION, "--cursor", "evt:failed/1"])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=50&cursor=evt%3Afailed%2F1")
        );
        assert_eq!(
            automation_dispatch(&["result", AUTOMATION, RUN])
                .unwrap()
                .path,
            format!("/v1/triggers/{AUTOMATION}/runs/{RUN}/result")
        );
        assert!(automation_dispatch(&["result", AUTOMATION]).is_err());
        let erase = automation_dispatch(&["erase-result", AUTOMATION, RUN]).unwrap();
        assert_eq!(
            erase.path,
            format!("/v1/triggers/{AUTOMATION}/runs/{RUN}/result/erase")
        );
        assert_eq!(erase.retry_policy, RetryPolicy::CallerKeyRequired);
        assert!(automation_dispatch(&["erase-result", AUTOMATION]).is_err());
        for action in ["pause", "resume", "disable"] {
            let call = automation_dispatch(&[action, AUTOMATION]).unwrap();
            assert_eq!(call.path, format!("/v1/triggers/{AUTOMATION}/{action}"));
            assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
        }
        for action in ["approve", "reject"] {
            let call = automation_dispatch(&[action, AUTOMATION, "evt:failed/1"]).unwrap();
            assert_eq!(
                call.path,
                format!("/v1/triggers/{AUTOMATION}/firings/{action}")
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(call.payload.as_ref().unwrap())
                    .unwrap(),
                json!({ "event_id": "evt:failed/1" })
            );
            assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
        }
    }

    #[test]
    fn malformed_automation_intent_never_reaches_edge() {
        for args in [
            vec![],
            vec!["create"],
            vec![
                "create",
                "--event",
                "CI.run.failed",
                "--run-as",
                AGENT,
                "--task",
                "Triage.",
                "--budget-minor-units",
                "1",
                "--max-firings",
                "1",
            ],
            vec![
                "create",
                "--event",
                "ci.run.failed",
                "--run-as",
                "not-an-agent",
                "--task",
                "Triage.",
                "--budget-minor-units",
                "1",
                "--max-firings",
                "1",
            ],
            vec![
                "create",
                "--event",
                "ci.run.failed",
                "--branch",
                "refs/tags/release",
                "--run-as",
                AGENT,
                "--task",
                "Triage.",
                "--budget-minor-units",
                "1",
                "--max-firings",
                "1",
            ],
            vec![
                "create",
                "--event",
                "ci.run.failed",
                "--where",
                "payload.retryable = true",
                "--run-as",
                AGENT,
                "--task",
                "Triage.",
                "--budget-minor-units",
                "1",
                "--max-firings",
                "1",
            ],
            vec![
                "create",
                "--event",
                "ci.run.failed",
                "--run-as",
                AGENT,
                "--task",
                "Triage.",
                "--budget-minor-units",
                "1",
                "--max-firings",
                "1",
                "--caveat",
                "issue:create",
            ],
            vec!["list", "--limit", "01"],
            vec!["show", "not-an-id"],
            vec!["history", AUTOMATION, "--cursor", " bad"],
            vec!["disable", AUTOMATION, "again"],
            vec!["approve", AUTOMATION],
            vec!["reject", AUTOMATION, " bad"],
        ] {
            assert!(automation_dispatch(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn automation_tasks_match_the_shared_agent_prompt_contract() {
        let base = [
            "create",
            "--event",
            "ci.run.failed",
            "--run-as",
            AGENT,
            "--task",
            "Inspect the failure.\nOpen one focused issue.",
            "--budget-minor-units",
            "1",
            "--max-firings",
            "1",
        ];
        assert!(automation_dispatch(&base).is_ok());

        for task in ["Inspect\0the failure.", "Inspect\u{1b}[31mthe failure."] {
            let mut args = base;
            args[6] = task;
            assert!(automation_dispatch(&args).is_err(), "accepted {task:?}");
        }
    }
}
