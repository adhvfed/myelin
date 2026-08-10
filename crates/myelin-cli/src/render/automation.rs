use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::{is_canonical_automation_id, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value) -> Option<String> {
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
    Some(format!(
        "{disposition}: {summary}\n  task: {task}\n  firings: {used}/{maximum}; per-run budget: {budget} minor-units\n  Myelin owns the integration credentials and gives each run only its governed tools.\n"
    ))
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
    Some(format!(
        "{} -> agent:{}  [{}]  {}",
        terminal_safe_single_line(event_type),
        terminal_safe_single_line(agent_id),
        terminal_safe_single_line(state),
        terminal_safe_single_line(reference),
    ))
}

fn render_firing(value: &Value) -> Option<String> {
    let event_id = terminal_safe_single_line(value.get("event_id")?.as_str()?);
    let state = value.get("state")?.as_str()?;
    let outcome = value.get("outcome").and_then(Value::as_str);
    let marker = match outcome {
        Some("succeeded") => "✓",
        Some("failed" | "terminated" | "nondeterministic") => "✗",
        Some(_) => "?",
        None if state == "terminal" && value.get("run_id").is_some_and(Value::is_null) => "-",
        None => "…",
    };
    let canceled_before_start =
        state == "terminal" && value.get("run_id").is_some_and(Value::is_null);
    let result = outcome.unwrap_or(if canceled_before_start {
        "canceled-before-start"
    } else {
        state
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
    Some(format!(
        "{marker} {}  {}  {}",
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
        assert!(output.contains("task: Triage CI.\\nOpen one issue."));
        assert!(output.contains("firings: 1/10; per-run budget: 250000 minor-units"));
        assert!(output.contains("Myelin owns the integration credentials"));
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
        }))
        .unwrap();
        assert_eq!(
            succeeded,
            "✓ succeeded  ci-failed-1  myelin://acme/agent/run/33333333-3333-4333-8333-333333333333"
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
