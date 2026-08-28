use std::fmt::Write as _;

use myelin_agent::is_canonical_tool_name;
use serde_json::Value;

use crate::dispatch::{is_canonical_tool_cursor, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value, call: Option<&EdgeCall>) -> Option<String> {
    if call.is_some_and(is_mcp_manifest_call) {
        let tools = value.get("tools")?.as_array()?;
        if !tools.iter().all(is_mcp_tool) {
            return None;
        }
        let mut rendered = serde_json::to_string_pretty(value).ok()?;
        rendered.push('\n');
        return Some(rendered);
    }

    let tool = value.get("tool")?;
    let name = valid_name(tool)?;
    let reference = valid_ref(tool)?;
    let version = tool.get("version")?.as_u64()?;
    let effect = tool.get("effect_kind")?.as_str()?;
    let side_effecting = tool.get("side_effecting")?.as_bool()?;
    let approval = tool.get("requires_approval")?.as_bool()?;
    let capabilities = tool.get("required_capabilities")?.as_array()?;
    let schema = tool.get("input_schema")?;

    let mut output = format!(
        "{} v{}\n  ref: {}\n  effect: {}{}{}\n  capabilities:",
        terminal_safe_single_line(name),
        version,
        terminal_safe_single_line(reference),
        terminal_safe_single_line(effect),
        if side_effecting {
            ", side-effecting"
        } else {
            ""
        },
        if approval { ", approval required" } else { "" },
    );
    if capabilities.is_empty() {
        output.push_str(" (none)");
    } else {
        for capability in capabilities {
            let capability = capability.as_str()?;
            let _ = write!(output, " {}", terminal_safe_single_line(capability));
        }
    }
    output.push_str("\n  input schema:\n");
    for line in serde_json::to_string_pretty(schema).ok()?.lines() {
        let _ = writeln!(output, "    {line}");
    }
    Some(output)
}

pub(super) fn render_item(value: &Value) -> Option<String> {
    let name = valid_name(value)?;
    let reference = valid_ref(value)?;
    let version = value.get("version")?.as_u64()?;
    let effect = value.get("effect_kind")?.as_str()?;
    let approval = value.get("requires_approval")?.as_bool()?;
    Some(format!(
        "{} v{}  [{}{}]  {}",
        terminal_safe_single_line(name),
        version,
        terminal_safe_single_line(effect),
        if approval { ", approval" } else { "" },
        terminal_safe_single_line(reference),
    ))
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if call.path != "/v1/tools" || !is_canonical_tool_cursor(cursor) {
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
        "myelin tool list --limit {limit} --cursor {cursor}"
    ))
}

fn is_mcp_manifest_call(call: &EdgeCall) -> bool {
    call.path == "/v1/tools" && call.query.as_deref() == Some("format=mcp")
}

fn is_mcp_tool(value: &Value) -> bool {
    value
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(is_canonical_tool_name)
        && value.get("inputSchema").is_some_and(Value::is_object)
        && value.get("annotations").is_some_and(Value::is_object)
}

fn valid_name(value: &Value) -> Option<&str> {
    value
        .get("name")?
        .as_str()
        .filter(|name| is_canonical_tool_name(name))
}

fn valid_ref(value: &Value) -> Option<&str> {
    value.get("ref")?.as_str().filter(|reference| {
        reference.starts_with("myelin://") && reference.contains("/agent/tool/")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::dispatch::tool_dispatch;

    fn tool() -> Value {
        json!({
            "name": "git.merge",
            "ref": "myelin://acme/agent/tool/git.merge@1",
            "subsystem": "git",
            "version": 1,
            "input_schema": {"type": "object", "required": ["repo"]},
            "required_capabilities": ["pull_request.merge"],
            "effect_kind": "mutate",
            "side_effecting": true,
            "requires_approval": true,
            "exposed_over_mcp": true,
        })
    }

    #[test]
    fn tool_rows_and_details_are_addressable_legible_and_terminal_safe() {
        assert_eq!(
            render_item(&tool()).unwrap(),
            "git.merge v1  [mutate, approval]  myelin://acme/agent/tool/git.merge@1"
        );
        let mut unsafe_tool = tool();
        unsafe_tool["required_capabilities"] = json!(["pull_request.merge\nforged"]);
        let details = render_response(&json!({"tool": unsafe_tool}), None).unwrap();
        assert!(details.contains("effect: mutate, side-effecting, approval required"));
        assert!(details.contains("pull_request.merge\\nforged"));
        assert!(details.contains("input schema:"));
    }

    #[test]
    fn tool_pagination_hints_round_trip_through_dispatch() {
        let call = tool_dispatch(&["list", "--limit", "7"]).unwrap();
        let hint = page_command(&call, "git.open_pr.v1").unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            tool_dispatch(&words[2..]).unwrap().query.as_deref(),
            Some("limit=7&cursor=git.open_pr.v1")
        );
    }

    #[test]
    fn mcp_description_is_redirectable_json_even_in_human_mode() {
        let call = tool_dispatch(&["describe", "--mcp"]).unwrap();
        let manifest = json!({
            "tools": [{
                "name": "git.merge",
                "description": "merge",
                "inputSchema": {"type": "object"},
                "annotations": {"requiresApproval": true}
            }]
        });
        let rendered = render_response(&manifest, Some(&call)).unwrap();
        assert!(rendered.starts_with("{\n"));
        assert!(rendered.ends_with('\n'));
        assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), manifest);
    }
}
