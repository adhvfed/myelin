use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::dispatch::{is_canonical_agent_id, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value, call: Option<&EdgeCall>) -> Option<String> {
    if let Some(thread) = value.get("thread") {
        return render_thread_response(value, thread, call);
    }
    let agent = value.get("agent")?;
    let summary = render_agent(agent)?;
    if let Some(action) = value.get("action") {
        return render_lifecycle(value, action.as_str()?, agent, summary);
    }
    let rename = call
        .is_some_and(|call| call.path.starts_with("/v1/agents/") && call.path.ends_with("/name"));
    let disposition = match (
        rename,
        value.get("created").and_then(Value::as_bool),
        value.get("changed").and_then(Value::as_bool),
        value.get("durable").and_then(Value::as_bool),
    ) {
        (true, None, Some(true), Some(true)) => "Renamed agent",
        (true, None, Some(false), Some(true)) => "Agent name already current",
        (true, _, _, _) => return None,
        (false, Some(true), _, _) => "Activated agent",
        (false, Some(false), _, _) => "Agent already active",
        (false, None, _, _) => "Agent",
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
    let workspace_profile = agent.get("workspace_profile")?;
    let workspace_revision = workspace_profile.get("revision")?.as_u64()?;
    if workspace_profile.get("personalization")?.is_null() {
        let _ = write!(
            output,
            "\n  workspace defaults: none (revision {workspace_revision})"
        );
    } else {
        let personalization = workspace_profile.get("personalization")?;
        if personalization.get("format")?.as_str()? != "workspace_augmentation_v1" {
            return None;
        }
        let _ = write!(
            output,
            "\n  workspace defaults: declarative personal overlay (revision {workspace_revision})"
        );
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
    render_thread(value).or_else(|| render_agent(value))
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if !is_canonical_agent_id(cursor) {
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
    match call.path.as_str() {
        "/v1/agents" => Some(format!(
            "myelin agent list --limit {limit} --cursor {cursor}"
        )),
        "/v1/agent-threads" => Some(format!(
            "myelin agent thread list --limit {limit} --cursor {cursor}"
        )),
        _ => None,
    }
}

fn render_thread_response(
    value: &Value,
    thread: &Value,
    call: Option<&EdgeCall>,
) -> Option<String> {
    let summary = render_thread(thread)?;
    let id = terminal_safe_single_line(thread.get("id")?.as_str()?);
    let conversation = terminal_safe_single_line(thread.get("conversation_ref")?.as_str()?);
    let agent = terminal_safe_single_line(thread.get("agent_ref")?.as_str()?);
    let agent_id = thread.get("agent_id")?.as_str()?;
    if !is_canonical_agent_id(agent_id) {
        return None;
    }
    let workspace = thread.get("workspace")?;
    let state = terminal_safe_single_line(workspace.get("state")?.as_str()?);
    let expires_at = terminal_safe_single_line(workspace.get("expires_at")?.as_str()?);
    let review_state = thread
        .get("review_state")
        .and_then(Value::as_str)
        .unwrap_or("draft");
    let renewal = call.is_some_and(|call| call.path.ends_with("/renew"));
    let prefix = match (
        renewal,
        review_state,
        value.get("changed").and_then(Value::as_bool),
        value.get("created").and_then(Value::as_bool),
    ) {
        (true, _, Some(true), _) => "Retained private agent workspace",
        (true, _, Some(false), _) => "Private agent workspace already retained",
        (false, "ready", Some(true), _) => "Published agent thread for review",
        (false, "ready", Some(false), _) => "Agent thread already ready for review",
        (false, _, _, Some(true)) => "Started private agent thread",
        (false, _, _, Some(false)) => "Private agent thread already exists",
        _ => "Private agent thread",
    };
    let mut output = format!(
        "{prefix}: {summary}\n  agent: {agent}\n  conversation: {conversation}\n  workspace: {state}; retained until {expires_at}\n"
    );
    if workspace_is_available(&state, &expires_at, Utc::now()) {
        let _ = writeln!(
            output,
            "  connect: myelin mcp serve --as {agent_id} --thread {id}"
        );
        let _ = writeln!(
            output,
            "  message: myelin agent thread say {id} \"<message>\""
        );
        let _ = writeln!(output, "  workspace: myelin agent thread ssh {id}");
        let _ = writeln!(output, "  files: myelin agent thread ssh {id} --sftp");
        let _ = writeln!(
            output,
            "  keep: myelin --idempotency-key <key> agent thread renew {id} --retention-days 30"
        );
        let _ = writeln!(
            output,
            "  editor: myelin agent thread ssh-config {id} --identity /path/to/ssh_identity"
        );
        if review_state == "draft" && thread.get("project_id").is_some_and(|id| !id.is_null()) {
            let _ = writeln!(output, "  share for review: myelin agent thread ready {id}");
        } else if review_state == "ready" {
            output.push_str("  review: visible to current project collaborators; workspace control remains yours.\n");
        }
    } else if matches!(state.as_str(), "failed" | "provisioning")
        && DateTime::parse_from_rfc3339(&expires_at).is_ok_and(|deadline| deadline > Utc::now())
    {
        let _ = writeln!(output, "  retry: myelin agent thread retry {id}");
    }
    Some(output)
}

fn workspace_is_available(state: &str, expires_at: &str, observed_at: DateTime<Utc>) -> bool {
    state == "ready"
        && DateTime::parse_from_rfc3339(expires_at).is_ok_and(|deadline| deadline > observed_at)
}

fn render_thread(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let reference = value.get("ref")?.as_str()?;
    let name = value.get("name")?.as_str()?;
    let workspace = value.get("workspace")?;
    let state = workspace.get("state")?.as_str()?;
    let expires_at = workspace.get("expires_at")?.as_str()?;
    if !is_canonical_agent_id(id)
        || !reference.starts_with("myelin://")
        || !reference.contains("/agent/thread/")
        || !matches!(
            state,
            "provisioning" | "ready" | "failed" | "expiring" | "deleted"
        )
    {
        return None;
    }
    Some(format!(
        "{}  [{}, until {}]  {}",
        terminal_safe_single_line(name),
        terminal_safe_single_line(state),
        terminal_safe_single_line(expires_at),
        terminal_safe_single_line(reference),
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
    const THREAD_ID: &str = "22222222-2222-2222-2222-222222222222";

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
                "ref": "myelin://acme/agent/tool/ci.read_run@1",
            }],
            "effective_tools": [],
            "grants": ["run.view"],
            "workspace_profile": {
                "revision": 0,
                "personalization": null,
                "updated_at": null,
            },
            "created_at": "2026-08-09T12:00:00Z",
        })
    }

    fn thread() -> Value {
        json!({
            "id": THREAD_ID,
            "ref": format!("myelin://acme/agent/thread/{THREAD_ID}"),
            "name": "Checkout\nrace",
            "agent_id": ID,
            "agent_ref": format!("myelin://acme/identity/agent/{ID}"),
            "project_id": null,
            "conversation_id": "01J00000000000000000000000",
            "conversation_ref": "myelin://acme/chat/channel/01J00000000000000000000000",
            "review_state": "draft",
            "review_ready_at": null,
            "workspace": {
                "id": "33333333-3333-3333-3333-333333333333",
                "generation": 1,
                "state": "ready",
                "retention_days": 3,
                "expires_at": "2099-08-25T12:00:00Z",
            },
            "created_at": "2026-08-22T12:00:00Z",
            "updated_at": "2026-08-22T12:00:00Z",
        })
    }

    #[test]
    fn activation_is_legible_addressable_and_honest_about_credentials() {
        let rendered = render_response(&json!({"agent": agent(), "created": true}), None).unwrap();
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
        let rendered = render_response(&json!({"agent": with_retired_tool}), None).unwrap();
        assert!(rendered.contains("selected tools: chat.retired.v1 [unavailable]"));
    }

    #[test]
    fn rename_receipts_name_the_identity_preserving_change() {
        let call = agent_dispatch(&[
            "rename",
            ID,
            "Release companion",
            "--expected-name",
            "Review companion",
        ])
        .unwrap();
        let receipt = json!({
            "agent": agent(),
            "changed": true,
            "durable": true,
        });
        assert!(render_response(&receipt, Some(&call))
            .unwrap()
            .starts_with("Renamed agent:"));
        let mut retry = receipt;
        retry["changed"] = json!(false);
        assert!(render_response(&retry, Some(&call))
            .unwrap()
            .starts_with("Agent name already current:"));
        retry.as_object_mut().unwrap().remove("durable");
        assert!(render_response(&retry, Some(&call)).is_none());
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
        let rendered = render_response(
            &json!({
                "agent": suspended,
                "action": "suspend",
                "changed": true,
                "terminated_runs": 1,
                "durable": true,
            }),
            None,
        )
        .unwrap();
        assert!(rendered.starts_with("Suspended agent: Review\\ncompanion"));
        assert!(rendered.contains("stopped 1 active run"));
        assert!(rendered.contains("blocked until this identity is resumed"));

        let mut resumed = agent();
        resumed["status"] = json!("active");
        let rendered = render_response(
            &json!({
                "agent": resumed,
                "action": "resume",
                "changed": true,
                "terminated_runs": 0,
                "durable": true,
            }),
            None,
        )
        .unwrap();
        assert!(rendered.starts_with("Resumed agent:"));
        assert!(rendered.contains("previously terminated runs remain closed"));
        assert!(!rendered.contains("stopped"));

        let mut retired = agent();
        retired["status"] = json!("disabled");
        let rendered = render_response(
            &json!({
                "agent": retired,
                "action": "retire",
                "changed": true,
                "terminated_runs": 2,
                "durable": true,
            }),
            None,
        )
        .unwrap();
        assert!(rendered.starts_with("Retired agent:"));
        assert!(rendered.contains("stopped 2 active runs"));
        assert!(rendered.contains("cannot be resumed"));
    }

    #[test]
    fn private_thread_receipts_explain_how_to_continue_the_work() {
        let rendered = render_response(
            &json!({
                "thread": thread(),
                "created": true,
                "durable": true,
            }),
            None,
        )
        .unwrap();
        assert!(rendered.starts_with("Started private agent thread: Checkout\\nrace"));
        assert!(rendered.contains("workspace: ready; retained until 2099-08-25T12:00:00Z"));
        assert!(rendered.contains(&format!("myelin agent thread say {THREAD_ID}")));
        assert!(rendered.contains(&format!("myelin agent thread ssh {THREAD_ID}")));
        assert!(rendered.contains(&format!("myelin agent thread ssh {THREAD_ID} --sftp")));
        assert!(rendered.contains(&format!("myelin agent thread ssh-config {THREAD_ID}")));
        assert!(rendered.contains(&format!("myelin mcp serve --as {ID} --thread {THREAD_ID}")));
        assert!(rendered.contains(&format!(
            "agent thread renew {THREAD_ID} --retention-days 30"
        )));
        assert!(!rendered.contains("storage_locator"));

        let renewal_call =
            agent_dispatch(&["thread", "renew", THREAD_ID, "--retention-days", "30"]).unwrap();
        let renewed = render_response(
            &json!({ "thread": thread(), "changed": true, "durable": true }),
            Some(&renewal_call),
        )
        .unwrap();
        assert!(renewed.starts_with("Retained private agent workspace:"));
        let replayed = render_response(
            &json!({ "thread": thread(), "changed": false, "durable": true }),
            Some(&renewal_call),
        )
        .unwrap();
        assert!(replayed.starts_with("Private agent workspace already retained:"));

        let mut project_draft = thread();
        project_draft["project_id"] = json!("33333333-3333-3333-3333-333333333333");
        let rendered = render_response(&json!({ "thread": project_draft }), None).unwrap();
        assert!(rendered.contains(&format!("myelin agent thread ready {THREAD_ID}")));

        let mut published = thread();
        published["project_id"] = json!("33333333-3333-3333-3333-333333333333");
        published["review_state"] = json!("ready");
        published["review_ready_at"] = json!("2026-08-22T13:00:00Z");
        let rendered = render_response(
            &json!({
                "thread": published,
                "changed": true,
                "durable": true,
            }),
            None,
        )
        .unwrap();
        assert!(rendered.starts_with("Published agent thread for review:"));
        assert!(rendered.contains("workspace control remains yours"));

        let mut expiring = thread();
        expiring["workspace"]["state"] = json!("expiring");
        let rendered = render_response(&json!({ "thread": expiring }), None).unwrap();
        assert!(rendered.starts_with("Private agent thread:"));
        assert!(!rendered.contains("myelin agent thread ssh"));
        assert!(!rendered.contains("myelin mcp serve"));

        let mut failed = thread();
        failed["workspace"]["state"] = json!("failed");
        let rendered = render_response(&json!({ "thread": failed }), None).unwrap();
        assert!(rendered.contains(&format!("myelin agent thread retry {THREAD_ID}")));
        assert!(!rendered.contains("myelin agent thread ssh"));

        let mut elapsed = thread();
        elapsed["workspace"]["expires_at"] = json!("2020-08-25T12:00:00Z");
        let rendered = render_response(&json!({ "thread": elapsed }), None).unwrap();
        assert!(!rendered.contains("myelin agent thread say"));
        assert!(!rendered.contains("myelin agent thread ssh"));
        assert!(!rendered.contains("myelin mcp serve"));
    }

    #[test]
    fn private_thread_pagination_hint_round_trips_through_dispatch() {
        let call = agent_dispatch(&["thread", "list", "--limit", "7"]).unwrap();
        let hint = page_command(&call, THREAD_ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            agent_dispatch(&words[2..]).unwrap().query.as_deref(),
            Some("limit=7&cursor=22222222-2222-2222-2222-222222222222")
        );
    }
}
