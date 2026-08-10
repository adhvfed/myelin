use myelin_agent::{
    EffectApi, EffectKind, EffectResult, ProposedEffect, RunCtx, ToolDef, ToolName, ToolSurface,
};
use myelin_storage::reserve_settle::{
    CostLedger, MicroUsd, Reservation, ReserveError, RunId, SettleError, SettleOutcome,
};
use myelin_tenancy::TenantId;

use crate::rebac_fragment::object_types as chat_objects;

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const CHAT_TOOL_VERSION: u32 = 1;

pub const POST_TOOL: &str = "post";
pub const REPLY_IN_THREAD_TOOL: &str = "reply_in_thread";
pub const REACT_TOOL: &str = "react";
pub const START_DM_TOOL: &str = "start_dm";
pub const CREATE_CHANNEL_TOOL: &str = "create_channel";
pub const INVITE_TOOL: &str = "invite";
pub const ARCHIVE_CHANNEL_TOOL: &str = "archive_channel";

pub const CHAT_TOOL_NAMES: &[&str] = &[
    POST_TOOL,
    REPLY_IN_THREAD_TOOL,
    REACT_TOOL,
    START_DM_TOOL,
    CREATE_CHANNEL_TOOL,
    INVITE_TOOL,
    ARCHIVE_CHANNEL_TOOL,
];

pub fn requires_approval_default(tool: &str) -> bool {
    match tool {
        POST_TOOL | REPLY_IN_THREAD_TOOL | REACT_TOOL | START_DM_TOOL => false,
        CREATE_CHANNEL_TOOL | INVITE_TOOL | ARCHIVE_CHANNEL_TOOL => true,
        _ => true,
    }
}

pub fn requires_approval_for_landing(landing_subsystem: &str, tool: &str) -> bool {
    if landing_subsystem == CHAT_SUBSYSTEM {
        requires_approval_default(tool)
    } else {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationSeamViolation {
    pub tool: String,
    pub effect_kind: EffectKind,
}

impl core::fmt::Display for MutationSeamViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "chat tool {} is side-effecting but effect_kind={:?} (would route through ToolHands::exec) \
             - a chat MUTATION MUST route through EffectApi (effect_kind=Mutate); the routing split is \
             the safety boundary (X-6)",
            self.tool, self.effect_kind
        )
    }
}

impl std::error::Error for MutationSeamViolation {}

pub fn assert_routes_through_effect_api(def: &ToolDef) -> Result<(), MutationSeamViolation> {
    if def.side_effecting && def.effect_kind != EffectKind::Mutate {
        return Err(MutationSeamViolation {
            tool: def.name.0.clone(),
            effect_kind: def.effect_kind,
        });
    }
    Ok(())
}

fn required_caps(tool: &str) -> Vec<String> {
    match tool {
        // `chat.post` is the public platform action granted to human callers and
        // delegated agents. The Edge adapter separately proves that the human
        // delegator can see the target conversation before it writes there.
        POST_TOOL => vec!["chat.post".to_string()],
        REPLY_IN_THREAD_TOOL | REACT_TOOL | START_DM_TOOL => {
            vec![format!("{}.post", chat_objects::CHANNEL)]
        }
        CREATE_CHANNEL_TOOL | INVITE_TOOL | ARCHIVE_CHANNEL_TOOL => {
            vec![format!("{}.manage", chat_objects::CHANNEL)]
        }
        _ => vec![format!("{}.manage", chat_objects::CHANNEL)],
    }
}

fn input_schema(tool: &str) -> &'static str {
    match tool {
        POST_TOOL => {
            r#"{"type":"object","required":["conversation_id","content"],"properties":{"conversation_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"},"content":{"type":"string","minLength":1,"maxLength":32768},"references":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}}},"additionalProperties":false}"#
        }
        REPLY_IN_THREAD_TOOL => {
            r#"{"type":"object","required":["channel","thread_root","body"],"properties":{"channel":{"type":"string"},"thread_root":{"type":"string"},"body":{"type":"string"}}}"#
        }
        REACT_TOOL => {
            r#"{"type":"object","required":["channel","message","emoji"],"properties":{"channel":{"type":"string"},"message":{"type":"string"},"emoji":{"type":"string"}}}"#
        }
        START_DM_TOOL => {
            r#"{"type":"object","required":["participants"],"properties":{"participants":{"type":"array","items":{"type":"string"}}}}"#
        }
        CREATE_CHANNEL_TOOL => {
            r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"},"parent_project":{"type":"string"}}}"#
        }
        INVITE_TOOL => {
            r#"{"type":"object","required":["channel","principal"],"properties":{"channel":{"type":"string"},"principal":{"type":"string"}}}"#
        }
        ARCHIVE_CHANNEL_TOOL => {
            r#"{"type":"object","required":["channel"],"properties":{"channel":{"type":"string"}}}"#
        }
        _ => r#"{"type":"object"}"#,
    }
}

pub fn chat_tool_def(tool: &str) -> ToolDef {
    ToolDef {
        name: ToolName(tool.to_string()),
        subsystem: CHAT_SUBSYSTEM.to_string(),
        version: CHAT_TOOL_VERSION,
        input_schema: input_schema(tool).to_string(),
        required_caps: required_caps(tool),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: requires_approval_default(tool),
        exposed_over_mcp: tool == POST_TOOL,
    }
}

pub fn chat_tool_defs() -> Vec<ToolDef> {
    CHAT_TOOL_NAMES.iter().map(|t| chat_tool_def(t)).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseningViolation {
    pub tool: String,
}

impl core::fmt::Display for LooseningViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "registration loosens the frozen requires_approval=yes default for chat.{} to no WITHOUT \
             authorisation (VISION §3: a consequential chat action may not be silently un-gated)",
            self.tool
        )
    }
}

impl std::error::Error for LooseningViolation {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    RoutingSplit(MutationSeamViolation),
    Loosening(LooseningViolation),
}

impl core::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RegisterError::RoutingSplit(v) => write!(f, "{v}"),
            RegisterError::Loosening(v) => write!(f, "{v}"),
        }
    }
}

impl std::error::Error for RegisterError {}

fn assert_no_silent_loosening(def: &ToolDef) -> Result<(), LooseningViolation> {
    let frozen = requires_approval_default(&def.name.0);
    if frozen && !def.requires_approval {
        return Err(LooseningViolation {
            tool: def.name.0.clone(),
        });
    }
    Ok(())
}

pub fn register_chat_tools<S: ToolSurface>(surface: &mut S) -> Result<Vec<ToolDef>, RegisterError> {
    let defs = chat_tool_defs();
    for def in &defs {
        assert_routes_through_effect_api(def).map_err(RegisterError::RoutingSplit)?;
        assert_no_silent_loosening(def).map_err(RegisterError::Loosening)?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostCostEstimate(pub MicroUsd);

pub fn reserve_spend_bearing_post(
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: RunId,
    estimate: PostCostEstimate,
    available: MicroUsd,
) -> Result<Reservation, ReserveError> {
    ledger.reserve(tenant, run, estimate.0, available)
}

pub fn settle_spend_bearing_post(
    ledger: &mut CostLedger,
    tenant: &TenantId,
    run: &RunId,
    units: &[myelin_storage::reserve_settle::MeteredUnit],
) -> Result<SettleOutcome, SettleError> {
    ledger.settle(tenant, run, units)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunEntry {
    pub tool: String,
    pub effect: ProposedEffect,
    pub would_gate: bool,
}

fn proposed_effect_for(tool: &str) -> ProposedEffect {
    ProposedEffect(format!("{CHAT_SUBSYSTEM}.{tool}"))
}

pub fn dry_run_chat_tools(invoked_tools: &[&str]) -> Vec<DryRunEntry> {
    invoked_tools
        .iter()
        .filter(|t| CHAT_TOOL_NAMES.contains(t))
        .map(|t| DryRunEntry {
            tool: t.to_string(),
            effect: proposed_effect_for(t),
            would_gate: requires_approval_default(t),
        })
        .collect()
}

pub struct ChatDryRun {
    invoked: Vec<String>,
}

impl ChatDryRun {
    pub fn new(invoked: Vec<String>) -> ChatDryRun {
        ChatDryRun { invoked }
    }
}

impl myelin_agent::DryRun for ChatDryRun {
    fn dry_run(&self, _inbox: myelin_agent::InboxEvent) -> Vec<ProposedEffect> {
        let names: Vec<&str> = self.invoked.iter().map(|s| s.as_str()).collect();
        dry_run_chat_tools(&names)
            .into_iter()
            .map(|e| e.effect)
            .collect()
    }
}

pub fn route_chat_effect_through_effect_api<E: EffectApi>(
    effect_api: &E,
    run: &RunCtx,
    tool: &str,
) -> EffectResult {
    effect_api.apply(run, proposed_effect_for(tool))
}

#[cfg(test)]
mod tests;
