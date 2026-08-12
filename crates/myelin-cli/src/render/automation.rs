use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::{is_canonical_automation_id, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value) -> Option<String> {
    if let Some(erasure) = value.get("erasure") {
        return render_erasure(erasure);
    }
    if let Some(result) = value.get("result") {
        return render_result(result);
    }
    if value.get("firing").is_some() {
        return render_approval(value);
    }
    let automation = value.get("trigger")?;
    let summary = render_automation(automation)?;
    if let Some(action) = value.get("action").and_then(Value::as_str) {
        return render_lifecycle(value, action, summary);
    }
    let disposition = match value.get("created").and_then(Value::as_bool) {
        Some(true) => "Created automation",
        Some(false) => "Automation already exists",
        None => "Automation",
    };
    let task = terminal_safe_single_line(automation.get("task")?.as_str()?);
    let used = automation.get("firings_used")?.as_u64()?;
    let maximum = automation.get("max_firings")?.as_u64()?;
    let budget = automation.get("budget_minor_units")?.as_u64()?;
    let mut output = format!("{disposition}: {summary}\n");
    if let Some(condition) = automation
        .get("condition")
        .and_then(Value::as_str)
        .filter(|condition| !condition.is_empty())
    {
        let _ = writeln!(output, "  when: {}", terminal_safe_single_line(condition));
    }
    let _ = writeln!(output, "  task: {task}");
    let _ = writeln!(
        output,
        "  firings: {used}/{maximum}; per-run budget: {budget} minor-units"
    );
    if let Some((code, detail, event_id, event_recorded_at)) = evaluation_diagnostic(automation) {
        let _ = writeln!(
            output,
            "  last evaluation error [{code}]: {} (event {} at {})",
            terminal_safe_single_line(detail),
            terminal_safe_single_line(event_id),
            terminal_safe_single_line(event_recorded_at),
        );
    }
    output.push_str(
        "  Myelin owns the integration credentials and gives each run only its governed tools.\n",
    );
    Some(output)
}

fn render_erasure(erasure: &Value) -> Option<String> {
    let run_ref = erasure.get("run_ref")?.as_str()?;
    let trace_ref = erasure.get("trace_ref")?.as_str()?;
    let already_erased = erasure.get("already_erased")?.as_bool()?;
    if !erasure.get("erased")?.as_bool()?
        || erasure.get("available_results")?.as_u64()? != 0
        || !erasure.get("recreation_blocked")?.as_bool()?
        || !run_ref.starts_with("myelin://")
        || !run_ref.contains("/agent/run/")
        || !trace_ref.starts_with("myelin://")
        || !trace_ref.contains("/knowledge/doc/")
    {
        return None;
    }
    let disposition = if already_erased {
        "Agent result was already erased"
    } else {
        "Erased agent result"
    };
    Some(format!(
        "{disposition}: {}\n  trace: {}\n  available results: 0; recreation blocked\n",
        terminal_safe_single_line(run_ref),
        terminal_safe_single_line(trace_ref),
    ))
}

fn render_result(result: &Value) -> Option<String> {
    let run_ref = result.get("run_ref")?.as_str()?;
    let trace_ref = result.get("trace_ref")?.as_str()?;
    let answer = result.get("answer")?.as_str()?;
    let agent = result.get("agent_principal")?.as_str()?;
    let charged = result.get("charged_micro")?.as_u64()?;
    if !run_ref.starts_with("myelin://")
        || !run_ref.contains("/agent/run/")
        || !trace_ref.starts_with("myelin://")
        || !trace_ref.contains("/knowledge/doc/")
    {
        return None;
    }
    let answer = answer
        .split('\n')
        .map(terminal_safe_single_line)
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "Agent result from {}:\n{}\n\n  run: {}\n  trace: {}\n  charged: {} micro-units\n",
        terminal_safe_single_line(agent),
        answer,
        terminal_safe_single_line(run_ref),
        terminal_safe_single_line(trace_ref),
        charged,
    ))
}

fn render_approval(value: &Value) -> Option<String> {
    if !value.get("durable")?.as_bool()? {
        return None;
    }
    let action = value.get("action")?.as_str()?;
    let changed = value.get("changed")?.as_bool()?;
    let firing = value.get("firing")?;
    let summary = render_firing(firing)?;
    let disposition = match (action, changed) {
        ("approve", true) => "Approved automation firing",
        ("approve", false) => "Automation firing already approved",
        ("reject", true) => "Rejected automation firing",
        ("reject", false) => "Automation firing already rejected",
        _ => return None,
    };
    Some(format!("{disposition}: {summary}\n"))
}

fn render_lifecycle(value: &Value, action: &str, summary: String) -> Option<String> {
    let changed = value.get("changed")?.as_bool()?;
    let canceled = value.get("canceled_firings")?.as_u64()?;
    if !value.get("durable")?.as_bool()? {
        return None;
    }
    let verb = match (action, changed) {
        ("pause", true) => "Paused automation",
        ("pause", false) => "Automation already paused",
        ("resume", true) => "Resumed automation",
        ("resume", false) => "Automation already active",
        ("disable", true) => "Disabled automation",
        ("disable", false) => "Automation already disabled",
        _ => return None,
    };
    let mut output = format!("{verb}: {summary}\n");
    match action {
        "pause" => output.push_str("  New matching events will not reserve work until resumed.\n"),
        "resume" => output.push_str("  New matching events may reserve governed work again.\n"),
        "disable" => output.push_str("  This automation cannot be resumed.\n"),
        _ => return None,
    }
    if canceled > 0 {
        let noun = if canceled == 1 { "firing" } else { "firings" };
        let _ = writeln!(output, "  canceled {canceled} unstarted {noun}.");
    }
    Some(output)
}

pub(super) fn render_item(value: &Value) -> Option<String> {
    render_automation(value).or_else(|| render_firing(value))
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    let limit = call
        .query
        .as_deref()
        .and_then(|query| query_field(query, "limit"))?;
    let parsed = limit.parse::<u32>().ok()?;
    if parsed.to_string() != limit || !(1..=100).contains(&parsed) {
        return None;
    }
    if call.path == "/v1/triggers" && is_canonical_automation_id(cursor) {
        return Some(format!(
            "myelin automation list --limit {limit} --cursor {cursor}"
        ));
    }
    let automation_id = call
        .path
        .strip_prefix("/v1/triggers/")?
        .strip_suffix("/firings")?;
    if !is_canonical_automation_id(automation_id) || !safe_event_cursor(cursor) {
        return None;
    }
    Some(format!(
        "myelin automation history {automation_id} --limit {limit} --cursor {cursor}"
    ))
}

fn render_automation(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let reference = value.get("ref")?.as_str()?;
    let event_type = value.get("event_type")?.as_str()?;
    let agent_id = value.get("run_as_agent_id")?.as_str()?;
    let state = value.get("state")?.as_str()?;
    if !is_canonical_automation_id(id)
        || !reference.starts_with("myelin://")
        || !reference.contains("/identity/trigger/")
        || !crate::dispatch::is_canonical_agent_id(agent_id)
    {
        return None;
    }
    let diagnostic = evaluation_diagnostic(value)
        .map(|(code, _, _, _)| format!("  [evaluation:{code}]"))
        .unwrap_or_default();
    Some(format!(
        "{} -> agent:{}  [{}]  {}{}",
        terminal_safe_single_line(event_type),
        terminal_safe_single_line(agent_id),
        terminal_safe_single_line(state),
        terminal_safe_single_line(reference),
        diagnostic,
    ))
}

fn evaluation_diagnostic(value: &Value) -> Option<(&str, &str, &str, &str)> {
    let diagnostic = value.get("last_evaluation_error")?.as_object()?;
    Some((
        diagnostic.get("code")?.as_str()?,
        diagnostic.get("detail")?.as_str()?,
        diagnostic.get("event_id")?.as_str()?,
        diagnostic.get("event_recorded_at")?.as_str()?,
    ))
}

fn render_firing(value: &Value) -> Option<String> {
    let event_id = terminal_safe_single_line(value.get("event_id")?.as_str()?);
    let state = value.get("state")?.as_str()?;
    let outcome = value.get("outcome").and_then(Value::as_str);
    let terminal_reason = value
        .get("terminal_reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .map(terminal_safe_single_line);
    let result_state = match value.get("result_state")? {
        Value::Null => None,
        Value::String(state) if matches!(state.as_str(), "available" | "erased") => {
            Some(state.as_str())
        }
        _ => return None,
    };
    let marker = match outcome {
        Some("succeeded") => "✓",
        Some("failed" | "terminated" | "nondeterministic") => "✗",
        Some(_) => "?",
        None if terminal_reason.is_some() => "✗",
        None if state == "terminal" && value.get("run_id").is_some_and(Value::is_null) => "-",
        None => "…",
    };
    let canceled_before_start = state == "terminal"
        && value.get("run_id").is_some_and(Value::is_null)
        && terminal_reason.is_none();
    let result = outcome.unwrap_or(match (terminal_reason.as_ref(), canceled_before_start) {
        (Some(_), _) => "could-not-start",
        (None, true) => "canceled-before-start",
        (None, false) => state,
    });
    let destination = value
        .get("run_ref")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            value
                .get("trigger_ref")
                .and_then(Value::as_str)
                .unwrap_or("?")
        });
    let result_suffix = match result_state {
        Some("available") => "  result:available",
        Some("erased") => "  result:erased",
        _ => "",
    };
    let reason_suffix = terminal_reason
        .map(|reason| format!("  reason:{reason}"))
        .unwrap_or_default();
    Some(format!(
        "{marker} {}  {}  {}{result_suffix}{reason_suffix}",
        terminal_safe_single_line(result),
        event_id,
        terminal_safe_single_line(destination),
    ))
}

fn safe_event_cursor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::dispatch::automation_dispatch;

    const AUTOMATION: &str = "22222222-2222-4222-8222-222222222222";
    const AGENT: &str = "11111111-1111-4111-8111-111111111111";

    fn automation() -> Value {
        json!({
            "id": AUTOMATION,
            "ref": format!("myelin://acme/identity/trigger/{AUTOMATION}"),
            "owner_principal_id": "ada",
            "run_as_agent_id": AGENT,
            "event_type": "ci.run.failed",
            "condition": "event.type == 'ci.run.failed' AND payload.source_ref == 'refs/heads/main'",
            "task": "Triage CI.\nOpen one issue.",
            "budget_minor_units": 250000,
            "max_firings": 10,
            "firings_used": 1,
            "state": "active"
        })
    }

    #[test]
    fn automation_output_explains_intent_budget_and_credential_ownership() {
        let output = render_response(&json!({
            "created": true,
            "durable": true,
            "trigger": automation(),
        }))
        .unwrap();
        assert!(output.starts_with("Created automation: ci.run.failed -> agent:"));
        assert!(output.contains(
            "when: event.type == 'ci.run.failed' AND payload.source_ref == 'refs/heads/main'"
        ));
        assert!(output.contains("task: Triage CI.\\nOpen one issue."));
        assert!(output.contains("firings: 1/10; per-run budget: 250000 minor-units"));
        assert!(output.contains("Myelin owns the integration credentials"));
    }

    #[test]
    fn automation_output_explains_the_last_rule_evaluation_failure() {
        let mut automation = automation();
        automation["last_evaluation_error"] = json!({
            "code": "type_error",
            "detail": "comparison is not defined over the operand types",
            "event_id": "push-main-42",
            "event_recorded_at": "2026-08-10T12:00:00Z",
        });
        let output = render_response(&json!({
            "durable": true,
            "trigger": automation,
        }))
        .unwrap();

        assert!(output.contains("[evaluation:type_error]"));
        assert!(output.contains("last evaluation error [type_error]"));
        assert!(output.contains("comparison is not defined over the operand types"));
        assert!(output.contains("event push-main-42 at 2026-08-10T12:00:00Z"));
    }

    #[test]
    fn a_completed_agent_answer_reads_as_the_work_product_not_runtime_plumbing() {
        let output = render_response(&json!({
            "result": {
                "run_id": "33333333-3333-4333-8333-333333333333",
                "run_ref": "myelin://acme/agent/run/33333333-3333-4333-8333-333333333333",
                "trace_ref": "myelin://acme/knowledge/doc/blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "agent_principal": format!("agent:{AGENT}"),
                "answer": "I read the failed run.\nI opened one issue.",
                "charged_micro": 42,
                "recorded_at": "2026-08-10T12:00:00Z"
            }
        }))
        .unwrap();

        assert!(output.starts_with(&format!("Agent result from agent:{AGENT}:")));
        assert!(output.contains("I read the failed run.\nI opened one issue."));
        assert!(output.contains("trace: myelin://acme/knowledge/doc/blake3:"));
        assert!(output.contains("charged: 42 micro-units"));
    }

    #[test]
    fn an_erased_agent_answer_is_explicitly_irrecoverable_and_replay_safe() {
        let output = render_response(&json!({
            "erasure": {
                "run_id": "33333333-3333-4333-8333-333333333333",
                "run_ref": "myelin://acme/agent/run/33333333-3333-4333-8333-333333333333",
                "trace_ref": "myelin://acme/knowledge/doc/blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "erased": true,
                "already_erased": false,
                "available_results": 0,
                "recreation_blocked": true
            }
        }))
        .unwrap();

        assert!(output.starts_with("Erased agent result: myelin://acme/agent/run/"));
        assert!(output.contains("available results: 0; recreation blocked"));
    }

    #[test]
    fn lifecycle_and_history_are_legible() {
        let mut paused = automation();
        paused["state"] = json!("paused");
        let output = render_response(&json!({
            "action": "pause",
            "changed": true,
            "canceled_firings": 0,
            "durable": true,
            "trigger": paused,
        }))
        .unwrap();
        assert!(output.starts_with("Paused automation:"));
        assert!(output.contains("will not reserve work until resumed"));

        let succeeded = render_item(&json!({
            "event_id": "ci-failed-1",
            "event_type": "ci.run.failed",
            "trigger_ref": format!("myelin://acme/identity/trigger/{AUTOMATION}"),
            "state": "terminal",
            "run_id": "33333333-3333-4333-8333-333333333333",
            "run_ref": "myelin://acme/agent/run/33333333-3333-4333-8333-333333333333",
            "outcome": "succeeded",
            "result_state": "available",
            "terminal_reason": null,
        }))
        .unwrap();
        assert_eq!(
            succeeded,
            "✓ succeeded  ci-failed-1  myelin://acme/agent/run/33333333-3333-4333-8333-333333333333  result:available"
        );

        let approval = render_response(&json!({
            "action": "approve",
            "changed": true,
            "durable": true,
            "firing": {
                "event_id": "ci-failed-2",
                "event_type": "ci.run.failed",
                "trigger_ref": format!("myelin://acme/identity/trigger/{AUTOMATION}"),
                "state": "queued",
                "run_id": null,
                "run_ref": null,
                "outcome": null,
                "result_state": null,
                "terminal_reason": null,
                "approval": {
                    "decision": "approved",
                    "decided_by": "ada",
                    "decided_at": "2026-08-10T10:00:00Z"
                }
            }
        }))
        .unwrap();
        assert!(approval.starts_with("Approved automation firing: … queued  ci-failed-2"));

        let poison = render_item(&json!({
            "event_id": "ci-failed-poison",
            "event_type": "ci.run.failed",
            "trigger_ref": format!("myelin://acme/identity/trigger/{AUTOMATION}"),
            "state": "terminal",
            "run_id": null,
            "run_ref": null,
            "outcome": null,
            "result_state": null,
            "terminal_reason": "invalid trigger claim: envelope identity mismatch",
        }))
        .unwrap();
        assert_eq!(
            poison,
            concat!(
                "✗ could-not-start  ci-failed-poison  ",
                "myelin://acme/identity/trigger/22222222-2222-4222-8222-222222222222",
                "  reason:invalid trigger claim: envelope identity mismatch",
            )
        );

        let failed = render_item(&json!({
            "event_id": "ci-failed-provider",
            "event_type": "ci.run.failed",
            "trigger_ref": format!("myelin://acme/identity/trigger/{AUTOMATION}"),
            "state": "terminal",
            "run_id": "33333333-3333-4333-8333-333333333333",
            "run_ref": "myelin://acme/agent/run/33333333-3333-4333-8333-333333333333",
            "outcome": "failed",
            "result_state": null,
            "terminal_reason":
                "agent run failed; retry it or inspect the hosted-agent service diagnostics",
        }))
        .unwrap();
        assert_eq!(
            failed,
            concat!(
                "✗ failed  ci-failed-provider  ",
                "myelin://acme/agent/run/33333333-3333-4333-8333-333333333333",
                "  reason:agent run failed; retry it or inspect the hosted-agent service diagnostics",
            )
        );
    }

    #[test]
    fn automation_pagination_hints_round_trip() {
        let list = automation_dispatch(&["list", "--limit", "7"]).unwrap();
        assert_eq!(
            page_command(&list, AUTOMATION).as_deref(),
            Some(concat!(
                "myelin automation list --limit 7 --cursor ",
                "22222222-2222-4222-8222-222222222222"
            ))
        );
        let history = automation_dispatch(&["history", AUTOMATION, "--limit", "9"]).unwrap();
        assert_eq!(
            page_command(&history, "ci-failed-1").as_deref(),
            Some(concat!(
                "myelin automation history 22222222-2222-4222-8222-222222222222 ",
                "--limit 9 --cursor ci-failed-1"
            ))
        );
    }
}
