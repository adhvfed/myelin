use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::{is_canonical_agent_id, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value) -> Option<String> {
    let agent = value.get("agent")?;
    let summary = render_agent(agent)?;
    if let Some(action) = value.get("action") {
        return render_lifecycle(value, action.as_str()?, agent, summary);
    }
    let disposition = match value.get("created").and_then(Value::as_bool) {
        Some(true) => "Activated agent",
        Some(false) => "Agent already active",
        None => "Agent",
    };
    let principal = terminal_safe_single_line(agent.get("principal_id")?.as_str()?);
    let on_behalf_of = terminal_safe_single_line(agent.get("on_behalf_of")?.as_str()?);
    let selected = agent.get("selected_tools")?.as_array()?;
    let mut output = format!(
        "{disposition}: {summary}\n  principal: {principal}\n  on behalf of: {on_behalf_of}\n  selected tools:"
    );
    if selected.is_empty() {
        output.push_str(" (none)");
    } else {
        for tool in selected {
            let _ = write!(output, " {}", selected_tool_label(tool)?);
        }
    }
    output.push_str(
        "\n  Durable identity and policy are ready; no long-lived API key was created.\n",
    );
    Some(output)
}

fn render_lifecycle(value: &Value, action: &str, agent: &Value, summary: String) -> Option<String> {
    let status = agent.get("status")?.as_str()?;
    let expected_status = match action {
        "suspend" => "suspended",
        "resume" => "active",
        "retire" => "disabled",
        _ => return None,
    };
    if status != expected_status
        || value.get("durable").and_then(Value::as_bool) != Some(true)
        || value.get("changed").and_then(Value::as_bool).is_none()
    {
        return None;
    }
    let terminated_runs = value.get("terminated_runs")?.as_u64()?;
    let mut output = match action {
        "suspend" => format!("Suspended agent: {summary}\n"),
        "resume" => format!("Resumed agent: {summary}\n"),
        "retire" => format!("Retired agent: {summary}\n"),
        _ => unreachable!(),
    };
    if action != "resume" {
        let noun = if terminated_runs == 1 { "run" } else { "runs" };
        let _ = writeln!(output, "  stopped {terminated_runs} active {noun}.");
    }
    match action {
        "suspend" => output.push_str("  New runs are blocked until this identity is resumed.\n"),
        "resume" => {
            output.push_str("  New runs are allowed; previously terminated runs remain closed.\n")
        }
        "retire" => output.push_str("  This identity cannot be resumed.\n"),
        _ => unreachable!(),
    }
    Some(output)
}

fn selected_tool_label(value: &Value) -> Option<String> {
    if let (Some(name), Some(version)) = (
        value.get("name").and_then(Value::as_str),
        value.get("version").and_then(Value::as_u64),
    ) {
        return Some(format!("{}@v{version}", terminal_safe_single_line(name)));
    }
    let cursor = terminal_safe_single_line(value.get("cursor")?.as_str()?);
    let state = terminal_safe_single_line(value.get("state")?.as_str()?);
    Some(format!("{cursor} [{state}]"))
}

pub(super) fn render_item(value: &Value) -> Option<String> {
    render_agent(value)
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if call.path != "/v1/agents" || !is_canonical_agent_id(cursor) {
        return None;
    }
    let limit = call
        .query
        .as_deref()
        .and_then(|query| query_field(query, "limit"))?;
    let parsed = limit.parse::<u32>().ok()?;
    if parsed.to_string() != limit || !(1..=100).contains(&parsed) {
        return None;
    }
    Some(format!(
        "myelin agent list --limit {limit} --cursor {cursor}"
    ))
}

fn render_agent(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let reference = value.get("ref")?.as_str()?;
    let name = value.get("name")?.as_str()?;
    let status = value.get("status")?.as_str()?;
    let runtime = value.get("runtime_ref")?.as_str()?;
    if !is_canonical_agent_id(id)
        || !reference.starts_with("myelin://")
        || !reference.contains("/identity/agent/")
    {
        return None;
    }
    Some(format!(
        "{}  [{}, {}]  {}",
        terminal_safe_single_line(name),
        terminal_safe_single_line(status),
        terminal_safe_single_line(runtime),
        terminal_safe_single_line(reference),
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::dispatch::agent_dispatch;

    const ID: &str = "11111111-1111-1111-1111-111111111111";

    fn agent() -> Value {
        json!({
            "id": ID,
            "ref": format!("myelin://acme/identity/agent/{ID}"),
            "principal_id": format!("agent:{ID}"),
            "name": "Review\ncompanion",
            "runtime_ref": "external:mcp",
            "on_behalf_of": "human:ada",
            "status": "active",
            "selected_tools": [{
                "name": "ci.read_run",
                "version": 1,
                "ref": "myelin://acme/agent/tool/ci.read_run/v1",
            }],
            "effective_tools": [],
            "grants": ["run.view"],
            "created_at": "2026-08-09T12:00:00Z",
        })
    }

    #[test]
    fn activation_is_legible_addressable_and_honest_about_credentials() {
        let rendered = render_response(&json!({"agent": agent(), "created": true})).unwrap();
        assert!(rendered.starts_with(&format!(
            "Activated agent: Review\\ncompanion  [active, external:mcp]  myelin://acme/identity/agent/{ID}"
        )));
        assert!(rendered.contains("selected tools: ci.read_run@v1"));
        assert!(rendered.contains("no long-lived API key was created"));

        let mut with_retired_tool = agent();
        with_retired_tool["selected_tools"] = json!([{
            "cursor": "chat.retired.v1",
            "state": "unavailable",
        }]);
        let rendered = render_response(&json!({"agent": with_retired_tool})).unwrap();
        assert!(rendered.contains("selected tools: chat.retired.v1 [unavailable]"));
    }

    #[test]
    fn agent_pagination_hints_round_trip_through_dispatch() {
        let call = agent_dispatch(&["list", "--limit", "7"]).unwrap();
        let hint = page_command(&call, ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            agent_dispatch(&words[2..]).unwrap().query.as_deref(),
            Some("limit=7&cursor=11111111-1111-1111-1111-111111111111")
        );
    }

    #[test]
    fn lifecycle_results_explain_what_changed_without_exposing_credentials() {
        let mut suspended = agent();
        suspended["status"] = json!("suspended");
        let rendered = render_response(&json!({
            "agent": suspended,
            "action": "suspend",
            "changed": true,
            "terminated_runs": 1,
            "durable": true,
        }))
        .unwrap();
        assert!(rendered.starts_with("Suspended agent: Review\\ncompanion"));
        assert!(rendered.contains("stopped 1 active run"));
        assert!(rendered.contains("blocked until this identity is resumed"));

        let mut resumed = agent();
        resumed["status"] = json!("active");
        let rendered = render_response(&json!({
            "agent": resumed,
            "action": "resume",
            "changed": true,
            "terminated_runs": 0,
            "durable": true,
        }))
        .unwrap();
        assert!(rendered.starts_with("Resumed agent:"));
        assert!(rendered.contains("previously terminated runs remain closed"));
        assert!(!rendered.contains("stopped"));

        let mut retired = agent();
        retired["status"] = json!("disabled");
        let rendered = render_response(&json!({
            "agent": retired,
            "action": "retire",
            "changed": true,
            "terminated_runs": 2,
            "durable": true,
        }))
        .unwrap();
        assert!(rendered.starts_with("Retired agent:"));
        assert!(rendered.contains("stopped 2 active runs"));
        assert!(rendered.contains("cannot be resumed"));
    }
}
