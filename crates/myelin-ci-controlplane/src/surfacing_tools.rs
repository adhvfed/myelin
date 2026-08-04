use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};

pub const CI_SUBSYSTEM: &str = "ci";

pub const CI_TOOL_VERSION: u32 = 1;

pub const CAP_RUN_TRIGGER: &str = "run.trigger";
pub const CAP_RUN_VIEW: &str = "run.view";
pub const CAP_ENVIRONMENT_DEPLOY: &str = "environment.deploy";
pub const CAP_CI_PROJECT_ADMINISTER: &str = "ci_project.administer";

pub const CI_TOOL_NAMES: &[&str] = &[
    "run",
    "run_pipeline",
    "cancel_run",
    "retry_run",
    "read_log",
    "read_run",
    "validate",
    "plan",
    "deploy",
    "approve_deploy",
    "rollback",
    "write_secret",
];

pub fn ci_requires_approval_default(tool: &str) -> bool {
    match tool {
        "deploy" | "approve_deploy" | "rollback" | "write_secret" => true,
        "run" | "run_pipeline" | "cancel_run" | "retry_run" | "read_log" | "read_run"
        | "validate" | "plan" => false,
        _ => true,
    }
}

pub fn ci_effect_kind(tool: &str) -> EffectKind {
    match tool {
        "read_log" | "read_run" | "validate" | "plan" => EffectKind::Read,
        _ => EffectKind::Mutate,
    }
}

pub fn ci_side_effecting(tool: &str) -> bool {
    !matches!(ci_effect_kind(tool), EffectKind::Read)
}

pub fn ci_required_caps(tool: &str) -> Vec<String> {
    let cap = match tool {
        "deploy" | "approve_deploy" | "rollback" => CAP_ENVIRONMENT_DEPLOY,
        "write_secret" => CAP_CI_PROJECT_ADMINISTER,
        "read_log" | "read_run" => CAP_RUN_VIEW,
        _ => CAP_RUN_TRIGGER,
    };
    vec![cap.to_string()]
}

pub fn ci_tool_def(tool: &str) -> ToolDef {
    let (input_schema, exposed_over_mcp) = match tool {
        "read_run" => (
            r#"{"type":"object","required":["run_id"],"properties":{"run_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},"additionalProperties":false}"#,
            true,
        ),
        "read_log" => (
            r#"{"type":"object","required":["run_id","job_id"],"properties":{"run_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"job_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"start":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":262144}},"additionalProperties":false}"#,
            true,
        ),
        _ => (r#"{"type":"object"}"#, false),
    };
    ToolDef {
        name: ToolName(tool.to_string()),
        subsystem: CI_SUBSYSTEM.to_string(),
        version: CI_TOOL_VERSION,
        input_schema: input_schema.to_string(),
        required_caps: ci_required_caps(tool),
        effect_kind: ci_effect_kind(tool),
        side_effecting: ci_side_effecting(tool),
        requires_approval: ci_requires_approval_default(tool),
        exposed_over_mcp,
    }
}

pub fn ci_tool_defs() -> Vec<ToolDef> {
    CI_TOOL_NAMES.iter().map(|t| ci_tool_def(t)).collect()
}

pub fn register_ci_tools<S: ToolSurface>(surface: &mut S) -> Vec<ToolDef> {
    let defs = ci_tool_defs();
    for def in &defs {
        surface.register_tool(def.clone());
    }
    defs
}

#[cfg(test)]
#[path = "surfacing_tools_tests.rs"]
mod tests;
