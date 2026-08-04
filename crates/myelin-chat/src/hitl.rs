use crate::glue::chat_hitl_card_facets;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, DelegationCaveats, FailStaticBound, IdentityService,
    Permission, Principal, PrincipalId, RunId, RunToken, Zookie,
};
use myelin_notif::{humanise, Channel, HumanisedString, RefResolvePort, TemplateStore};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

pub const APPROVE_PERMISSION: &str = "approve";

pub const APPROVAL_SIGNAL_PREFIX: &str = "approval";

pub const DECLINE_MARKER: &str = "decline";

pub const TIMEOUT_REASON: &str = "timeout";

pub fn approval_signal_name(card_id: &str) -> String {
    format!("{APPROVAL_SIGNAL_PREFIX}:{card_id}")
}

pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(
        total_effects >= 1,
        "a card gates at least one effect (total_effects >= 1)"
    );
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardDecision {
    Approve,
    Decline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardEffect {
    pub subject: ArtifactRef,
    pub action: String,
    pub risk: String,
    pub cost: String,
    pub effect_refs: Vec<ArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatApprovalCard {
    pub run_id: RunId,
    pub card_id: String,
    pub effects: Vec<CardEffect>,
}

impl ChatApprovalCard {
    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }

    pub fn signal_name(&self) -> String {
        approval_signal_name(&self.card_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedCardEffect {
    pub subject_line: HumanisedString,
    pub facets_line: String,
    pub idem_key: String,
}

#[allow(clippy::too_many_arguments)]
pub fn render_card(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    tenant: &TenantId,
    region: &Region,
    card: &ChatApprovalCard,
    effect_idx: usize,
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> RenderedCardEffect {
    let effect = &card.effects[effect_idx];
    let subject_line = humanise(
        resolver,
        tenant,
        region,
        templates,
        crate::glue::TPL_CHAT_CARD,
        std::slice::from_ref(&effect.subject),
        viewer,
        locale,
        at,
        channel,
    );
    let facets_line = chat_hitl_card_facets(templates, &effect.action, &effect.risk, &effect.cost);
    RenderedCardEffect {
        subject_line,
        facets_line,
        idem_key: card.idem_key_for(effect_idx),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickDenied {
    pub run_id: String,
}

impl core::fmt::Display for ClickDenied {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "approve click denied: clicker lacks `{APPROVE_PERMISSION}` on run `{}` (fail-closed)",
            self.run_id
        )
    }
}

impl std::error::Error for ClickDenied {}

pub struct ClickGate<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> ClickGate<I> {
    pub fn new(id: I) -> ClickGate<I> {
        ClickGate { id }
    }

    pub fn check_click(
        &self,
        clicker: &Principal,
        card: &ChatApprovalCard,
        at_zookie: Option<&str>,
    ) -> Result<(), ClickDenied> {
        let object = ArtifactRef(run_object(&card.run_id.0));
        let at = Consistency {
            at_least: Zookie(at_zookie.unwrap_or("").to_string()),
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(APPROVE_PERMISSION.to_string());
        match self.id.check(clicker, &permission, &object, &at, None) {
            Ok(Decision::Allow) => Ok(()),
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => Err(ClickDenied {
                run_id: card.run_id.0.clone(),
            }),
        }
    }
}

pub fn run_object(run_id: &str) -> String {
    format!("run:{run_id}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardSignal {
    pub run_id: RunId,
    pub signal_name: String,
    pub idem_key: String,
    pub payload: Vec<ArtifactRef>,
    pub payload_key_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalDelivery {
    Buffered,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalPostError {
    pub reason: String,
}

impl core::fmt::Display for SignalPostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "approval signal post failed: {}", self.reason)
    }
}

impl std::error::Error for SignalPostError {}

pub trait SignalPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardOutcome {
    Approved(SignalDelivery),
    Withheld(SignalDelivery, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardClick {
    pub effect_idx: usize,
    pub decision: CardDecision,
    pub decline_reason: String,
}

pub fn build_card_signal(card: &ChatApprovalCard, click: &CardClick) -> CardSignal {
    let effect = &card.effects[click.effect_idx];
    let idem_key = card.idem_key_for(click.effect_idx);
    let signal_name = card.signal_name();
    match click.decision {
        CardDecision::Approve => CardSignal {
            run_id: card.run_id.clone(),
            signal_name,
            idem_key,
            payload: effect.effect_refs.clone(),
            payload_key_ref: None,
        },
        CardDecision::Decline => CardSignal {
            run_id: card.run_id.clone(),
            signal_name,
            idem_key,
            payload: vec![],
            payload_key_ref: Some(click.decline_reason.clone()),
        },
    }
}

pub fn post_decision<I: IdentityService, P: SignalPort>(
    gate: &ClickGate<I>,
    port: &P,
    card: &ChatApprovalCard,
    click: &CardClick,
    clicker: &Principal,
    at_zookie: Option<&str>,
) -> Result<CardOutcome, PostDecisionError> {
    gate.check_click(clicker, card, at_zookie)
        .map_err(PostDecisionError::Denied)?;
    let signal = build_card_signal(card, click);
    let delivery = port.post_signal(&signal).map_err(PostDecisionError::Post)?;
    Ok(match click.decision {
        CardDecision::Approve => CardOutcome::Approved(delivery),
        CardDecision::Decline => CardOutcome::Withheld(delivery, click.decline_reason.clone()),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostDecisionError {
    Denied(ClickDenied),
    Post(SignalPostError),
}

impl core::fmt::Display for PostDecisionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PostDecisionError::Denied(d) => write!(f, "{d}"),
            PostDecisionError::Post(p) => write!(f, "{p}"),
        }
    }
}

impl std::error::Error for PostDecisionError {}

pub fn auto_deny_on_timeout(effect_idx: usize) -> CardClick {
    CardClick {
        effect_idx,
        decision: CardDecision::Decline,
        decline_reason: TIMEOUT_REASON.to_string(),
    }
}

pub struct ResumeTokenMinter<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> ResumeTokenMinter<I> {
    pub fn new(id: I) -> ResumeTokenMinter<I> {
        ResumeTokenMinter { id }
    }

    pub fn mint_resume_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &RunId,
        caveats: &DelegationCaveats,
    ) -> myelin_identity::Result<RunToken> {
        self.id
            .mint_run_token(agent_id, run_id, caveats, &FailStaticBound::DEFAULT_W)
    }
}

#[cfg(test)]
mod tests;
