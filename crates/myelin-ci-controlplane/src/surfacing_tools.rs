use myelin_agent::{EffectKind, ToolDef, ToolName};

pub const CI_SUBSYSTEM: &str = "ci";

pub const CI_TOOL_VERSION: u32 = 1;

pub const CAP_RUN_VIEW: &str = "run.view";

pub const CI_TOOL_NAMES: &[&str] = &["read_log", "read_run"];

pub fn ci_tool_def(tool: &str) -> Option<ToolDef> {
    let input_schema = match tool {
        "read_run" => {
            r#"{"type":"object","required":["run_id"],"properties":{"run_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},"additionalProperties":false}"#
        }
        "read_log" => {
            r#"{"type":"object","required":["run_id","job_id"],"properties":{"run_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"job_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"start":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":262144}},"additionalProperties":false}"#
        }
        _ => return None,
    };
    Some(ToolDef {
        name: ToolName(tool.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: input_schema.to_string(),
        required_caps: vec![CAP_RUN_VIEW.to_string()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    })
}

pub fn ci_tool_defs() -> Vec<ToolDef> {
    CI_TOOL_NAMES
        .iter()
        .map(|tool| ci_tool_def(tool).expect("CI_TOOL_NAMES contains only executable reads"))
        .collect()
}

#[cfg(test)]
#[path = "surfacing_tools_tests.rs"]
mod tests;
