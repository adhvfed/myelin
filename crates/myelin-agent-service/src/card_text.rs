use crate::hitl::{HitlCard, RiskSummary};
use myelin_identity::{Consistency, Principal};
use myelin_notif::{
    humanise, Channel, HumaniseTemplate, HumanisedString, RefResolvePort, TemplateStore,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    pub template_key: String,
    pub args: Vec<(String, ArtifactRef)>,
}

impl AgentMessage {
    pub fn new(template_key: impl Into<String>, args: Vec<(String, ArtifactRef)>) -> AgentMessage {
        AgentMessage {
            template_key: template_key.into(),
            args,
        }
    }

    pub fn about(template_key: impl Into<String>, object: &ArtifactRef) -> AgentMessage {
        AgentMessage {
            template_key: template_key.into(),
            args: vec![("object".to_string(), object.clone())],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAgentString {
    pub offered: String,
}

impl core::fmt::Display for RawAgentString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "a raw agent string is not a humanise surface: `{}` is not a stable (template_key, args) \
             taxonomy token - every agent-authored card/message routes through humanise (C9/OQ-L)",
            self.offered
        )
    }
}

impl std::error::Error for RawAgentString {}

pub fn assert_no_raw_agent_surface(template_key: &str) -> Result<(), RawAgentString> {
    let reject = || RawAgentString {
        offered: template_key.to_string(),
    };
    if template_key.is_empty() {
        return Err(reject());
    }
    if template_key.chars().any(|c| c.is_whitespace()) {
        return Err(reject());
    }
    if template_key.contains('!')
        || template_key.contains('?')
        || template_key.contains(',')
        || template_key.ends_with('.')
    {
        return Err(reject());
    }
    if !template_key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(reject());
    }
    Ok(())
}

pub const AGENT_PLATFORM_DEFAULT_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        "agent.hitl.merge_pr",
        "Merge {0} - this lands code on the default branch.",
        "merge",
    ),
    (
        "agent.hitl.deploy",
        "Deploy {0} - this changes a running environment.",
        "deploy",
    ),
    (
        "agent.hitl.delete",
        "Delete {0} - this is irreversible.",
        "delete",
    ),
    (
        "agent.hitl.transition",
        "Transition {0} - this changes its state.",
        "transition",
    ),
    (
        "agent.hitl.generic",
        "Approve the agent's action on {0}.",
        "approval",
    ),
    (
        "agent.msg.proposed_effect",
        "The agent proposed an effect on {0}.",
        "agent",
    ),
    (
        "agent.msg.halted_rejected",
        "The agent run was halted: the action on {0} was rejected.",
        "halt",
    ),
    (
        "agent.msg.completed",
        "The agent completed its work on {0}.",
        "done",
    ),
];

pub fn register_agent_templates(store: &mut TemplateStore) {
    for (key, body, icon) in AGENT_PLATFORM_DEFAULT_TEMPLATES {
        store.put(HumaniseTemplate {
            tenant: myelin_notif::PLATFORM_DEFAULT_TENANT.to_string(),
            template_key: (*key).to_string(),
            locale: myelin_notif::DEFAULT_LOCALE.to_string(),
            body: (*body).to_string(),
            icon: (*icon).to_string(),
        });
    }
}

#[derive(Clone, Copy)]
pub struct RenderCtx<'a> {
    pub resolver: &'a dyn RefResolvePort,
    pub tenant: &'a TenantId,
    pub region: &'a Region,
    pub templates: &'a TemplateStore,
    pub viewer: &'a Principal,
    pub locale: &'a str,
    pub at: &'a Consistency,
    pub channel: Channel,
}

fn humanise_with_args(
    ctx: &RenderCtx<'_>,
    template_key: &str,
    args: &[(String, ArtifactRef)],
) -> Result<HumanisedString, RawAgentString> {
    assert_no_raw_agent_surface(template_key)?;
    let ref_args: Vec<ArtifactRef> = args.iter().map(|(_, r)| r.clone()).collect();
    Ok(humanise(
        ctx.resolver,
        ctx.tenant,
        ctx.region,
        ctx.templates,
        template_key,
        &ref_args,
        ctx.viewer,
        ctx.locale,
        ctx.at,
        ctx.channel,
    ))
}

pub fn humanise_risk_summary(
    ctx: &RenderCtx<'_>,
    risk: &RiskSummary,
) -> Result<HumanisedString, RawAgentString> {
    humanise_with_args(ctx, &risk.template_key, &risk.args)
}

pub fn humanise_agent_message(
    ctx: &RenderCtx<'_>,
    msg: &AgentMessage,
) -> Result<HumanisedString, RawAgentString> {
    humanise_with_args(ctx, &msg.template_key, &msg.args)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedCard {
    pub risk_text: HumanisedString,
    pub action_tool: String,
    pub cost_estimate: u64,
    pub approver_count: usize,
}

pub fn humanise_card(
    ctx: &RenderCtx<'_>,
    card: &HitlCard,
) -> Result<RenderedCard, RawAgentString> {
    let risk_text = humanise_risk_summary(ctx, &card.risk_summary)?;
    Ok(RenderedCard {
        risk_text,
        action_tool: card.action_tool.clone(),
        cost_estimate: card.cost_estimate,
        approver_count: card.approvers.len(),
    })
}

#[cfg(test)]
mod tests;
