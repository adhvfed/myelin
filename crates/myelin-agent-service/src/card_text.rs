//! # `card_text` — humanise the HITL card text + agent-authored messages through the ONE templating surface (AG-P11 → P-223, M2-B / C9 / OQ-L)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.3 (C9: the HITL card text
//! goes through the ONE templating surface `humanise` — NEVER raw strings; the `risk_summary` + any
//! agent-authored message are a `(template_key, args)` pair + an `ArtifactRef`, humanised per-viewer;
//! there is NO second template engine and NO frontend string map; the card renders per-viewer —
//! permission/erasure-safe, ICU MessageFormat).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-L (humanise is the sole templating
//! surface). **Contracts:** contract-index.md row 7.3 (`humanise((template_key, args), viewer,
//! locale) -> HumanisedString` — per-viewer, permission/erasure-safe, ICU MessageFormat) — CONSUMED;
//! row 8.2 (the `hitl_gate.risk_summary` slot the [`crate::hitl::RiskSummary`] carries).
//!
//! **VISION §3** (GDPR-safe by construction: per-viewer, erasure-safe). **EI-01 §7** (abstract at
//! the third copy — ONE templating surface, not a second engine: this module is a thin ADAPTER over
//! Notif `humanise`, it ships no formatter of its own).
//!
//! ## What this prompt ships — the agent-fabric card-text path (C9/OQ-L)
//!
//! The [`crate::hitl`] module (AG-P9 → P-221) already models the HITL `risk_summary` as a
//! [`RiskSummary`] = `(template_key, args)` SLOT (NOT a raw string). THIS module is the render WIRING
//! the AG-P9 floor named: it lowers that slot — and any agent-authored message — onto Notif
//! `humanise` (the ONE templating surface, contract 7.3). Concretely:
//!
//! - [`humanise_risk_summary`] — render a [`RiskSummary`] per-viewer/locale into a `HumanisedString`
//!   by calling Notif [`humanise`](myelin_notif::humanise) with the slot's `(template_key, args)`.
//! - [`humanise_card`] — render a whole [`HitlCard`] per-viewer: the risk text humanised + the
//!   structured (non-text) fields (cost, approvers). The card's TEXT is NEVER a raw string.
//! - [`AgentMessage`] — the carrier for ANY agent-authored message: a `(template_key, args)` pair,
//!   exactly like [`RiskSummary`]. There is structurally NO raw-string constructor — an agent cannot
//!   hand the card a free string (the type forbids it). [`humanise_agent_message`] renders it.
//! - [`assert_no_raw_agent_surface`] — the 0-raw-string-surfaces assertion: a candidate
//!   agent-authored surface MUST be a registered `(template_key, args)` pair whose key is a stable
//!   taxonomy token (never free text). A raw agent string is REJECTED ([`RawAgentString`]).
//!
//! ## Permission-safe + erasure-safe BY CONSTRUCTION (the same property humanise gives Notif)
//! Because every arg is an [`ArtifactRef`] (references-not-payloads, §3.4), Notif `humanise`
//! resolves each slot PER-VIEWER through the Refs resolve chokepoint: a viewer WITH `view` sees the
//! title; a viewer WITHOUT it sees a TOMBSTONE (`a restricted issue`); an erased subject renders
//! `[erased user]`. The agent fabric inherits this for free — the card text leaks NO title to an
//! unauthorised viewer (the same NOTIF-D4 0-leak invariant), and the SAME card renders differently
//! for two viewers with different permissions (the per-viewer gate).
//!
//! ## FLOOR named: NONE — humanise is the sole templating surface (the prompt's explicit floor row).
//! There is no second template engine and no frontend string map in the agent fabric. The canonical
//! ICU/WASM render body is owned by Notif (the KN-P01 WASM target is Notif's named floor, not the
//! agent fabric's — this module only CALLS `humanise`). The agent platform-default templates seeded
//! here are the agent-fabric's `(template_key -> body)` rows registered into the SAME
//! [`TemplateStore`]; a tenant overrides them exactly like any other Notif template.

use crate::hitl::{HitlCard, RiskSummary};
use myelin_identity::{Consistency, Principal};
use myelin_notif::{
    humanise, Channel, HumaniseTemplate, HumanisedString, RefResolvePort, TemplateStore,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

// ───────────────────────── the agent-authored message carrier (C9 — NEVER a raw string) ─────────

/// **An agent-authored message — a `(template_key, args)` pair, NEVER a raw string (C9/OQ-L).** Any
/// message the agent loop authors for a human surface (a card subtitle, a proposal note, a halt
/// reason shown to a person) is THIS shape: a stable `template_key` (a taxonomy token) + ordered
/// `(arg_name, ArtifactRef)` args. There is **no** `AgentMessage::from_raw(String)` constructor — an
/// agent structurally cannot hand a human surface a free string. Notif `humanise` renders it
/// per-viewer/locale, permission- + erasure-safe.
///
/// This mirrors [`RiskSummary`] exactly (the HITL risk slot is one kind of agent-authored message);
/// they share the [`humanise_with_args`] render path so there is ONE lowering onto the ONE surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    /// the stable template key (e.g. `agent.msg.proposed_effect`) — a taxonomy token, never free
    /// text. [`assert_no_raw_agent_surface`] rejects a key that looks like free text.
    pub template_key: String,
    /// the ordered humanise args — `(arg_name, ArtifactRef)` (references-not-payloads). Each is a
    /// REFERENCE Notif resolves per-viewer, never an inline PII body.
    pub args: Vec<(String, ArtifactRef)>,
}

impl AgentMessage {
    /// Build an agent message from a template key + its `(arg_name, ArtifactRef)` args. The ONLY
    /// constructor — there is deliberately no raw-string path (an agent cannot author a free string).
    pub fn new(template_key: impl Into<String>, args: Vec<(String, ArtifactRef)>) -> AgentMessage {
        AgentMessage {
            template_key: template_key.into(),
            args,
        }
    }

    /// An agent message about a single object (the common one-arg shape).
    pub fn about(template_key: impl Into<String>, object: &ArtifactRef) -> AgentMessage {
        AgentMessage {
            template_key: template_key.into(),
            args: vec![("object".to_string(), object.clone())],
        }
    }
}

// ───────────────────────── the 0-raw-string-surfaces assertion (the GATE) ─────────────────────────

/// **A raw agent string was offered to a human surface — REJECTED (0 raw-string surfaces, the GATE).**
/// The agent loop has NO raw-string path to a card or a chat message: every agent-authored surface is
/// a `(template_key, args)` pair whose key is a stable taxonomy token. A candidate that is NOT such a
/// pair (a free-text key, or a key that is plainly a sentence) is refused HERE — the loud, structural
/// proof that there are 0 raw-string agent surfaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAgentString {
    /// the offending candidate text (so the rejection is observable in the audit — this is the
    /// template_key field that failed the taxonomy-token shape, never a leaked human string).
    pub offered: String,
}

impl core::fmt::Display for RawAgentString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "a raw agent string is not a humanise surface: `{}` is not a stable (template_key, args) \
             taxonomy token — every agent-authored card/message routes through humanise (C9/OQ-L)",
            self.offered
        )
    }
}

impl std::error::Error for RawAgentString {}

/// **The 0-raw-string-surfaces assertion (the AG-P11 GATE).** A template key is a STABLE taxonomy
/// token — lowercase, dot/underscore-segmented, no whitespace, no sentence punctuation. A raw agent
/// string (a sentence, a key with spaces, an empty key) is REJECTED ([`RawAgentString`]). This is the
/// structural proof that the agent loop has no raw-string path to a card or a chat message: a key
/// that does not pass this gate cannot reach Notif `humanise` as an agent-authored surface.
///
/// The shape rules (a key like `agent.hitl.merge_pr` passes; `Please review this PR!` fails):
/// - non-empty;
/// - no ASCII whitespace (a free-text sentence has spaces);
/// - no sentence punctuation (`! ? ,` or a trailing `.`) — a taxonomy token uses `.`/`_` as
///   SEPARATORS, never as sentence punctuation, so an interior `.` is fine but a trailing one is not;
/// - every character is a token char (`a-z`, `0-9`, `.`, `_`, `-`).
pub fn assert_no_raw_agent_surface(template_key: &str) -> Result<(), RawAgentString> {
    let reject = || RawAgentString {
        offered: template_key.to_string(),
    };
    if template_key.is_empty() {
        return Err(reject());
    }
    // A free-text sentence has spaces; a taxonomy token never does.
    if template_key.chars().any(|c| c.is_whitespace()) {
        return Err(reject());
    }
    // Sentence punctuation is a tell of a raw human string (a key never uses `!`/`?`/`,`, and never
    // ENDS in `.` — interior `.` is a segment separator).
    if template_key.contains('!')
        || template_key.contains('?')
        || template_key.contains(',')
        || template_key.ends_with('.')
    {
        return Err(reject());
    }
    // Every char is a token char (lowercase alnum + the segment separators `.`/`_`/`-`). An
    // uppercase letter (a Capitalised Sentence) or any other symbol is a raw-string tell.
    if !template_key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(reject());
    }
    Ok(())
}

// ───────────────────────── the agent-fabric platform-default templates (registered into Notif) ────

/// The agent-fabric's platform-default humanise templates (the `(template_key, body, icon)` rows the
/// HITL cards + agent messages render with). Each is a NULL-tenant ([`PLATFORM_DEFAULT_TENANT`])
/// `en` row registered into the SAME Notif [`TemplateStore`] (the ONE store — NOT a second engine); a
/// tenant brands/localises by `put`ting an override. `{0}` is the per-viewer-resolved SUBJECT slot.
///
/// These are ICU-subset bodies (the Notif `render_message` subset). A denied/erased subject renders
/// as a tombstone in `{0}` — the card text NEVER leaks a title.
pub const AGENT_PLATFORM_DEFAULT_TEMPLATES: &[(&str, &str, &str)] = &[
    // The HITL risk-summary card bodies (the `agent.hitl.*` family — the risk shown on the card).
    (
        "agent.hitl.merge_pr",
        "Merge {0} — this lands code on the default branch.",
        "merge",
    ),
    (
        "agent.hitl.deploy",
        "Deploy {0} — this changes a running environment.",
        "deploy",
    ),
    (
        "agent.hitl.delete",
        "Delete {0} — this is irreversible.",
        "delete",
    ),
    (
        "agent.hitl.transition",
        "Transition {0} — this changes its state.",
        "transition",
    ),
    (
        "agent.hitl.generic",
        "Approve the agent's action on {0}.",
        "approval",
    ),
    // The agent-authored message bodies (the `agent.msg.*` family).
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

/// **Register the agent-fabric platform-default templates into a Notif [`TemplateStore`] (the ONE
/// store).** The agent fabric ships its `(template_key -> body)` rows into the SAME templating store
/// every other subsystem registers against — there is no second engine and no frontend string map.
/// The serve-boot path calls this once so the shared store carries the agent card/message bodies.
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

// ───────────────────────── the render wiring — lower (template_key, args) onto humanise ──────────

/// **The ONE lowering of an agent-authored `(template_key, ordered ArtifactRef args)` onto Notif
/// `humanise` (contract 7.3).** Both [`humanise_risk_summary`] and [`humanise_agent_message`] go
/// through HERE — there is one render path, not two. The `(arg_name, ArtifactRef)` args are flattened
/// to the ordered `&[ArtifactRef]` Notif binds to `{0}`, `{1}`, … per-viewer (each resolved through
/// the Refs chokepoint — a denied/erased subject becomes a tombstone, never a leaked title).
///
/// Refuses ([`RawAgentString`]) a key that is not a stable taxonomy token (the 0-raw-string gate runs
/// on EVERY render — an agent cannot smuggle a raw string in as a "key").
#[allow(clippy::too_many_arguments)]
fn humanise_with_args(
    resolver: &dyn RefResolvePort,
    tenant: &TenantId,
    region: &Region,
    templates: &TemplateStore,
    template_key: &str,
    args: &[(String, ArtifactRef)],
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> Result<HumanisedString, RawAgentString> {
    // The 0-raw-string-surfaces gate runs on every render (no raw key reaches humanise).
    assert_no_raw_agent_surface(template_key)?;
    // Flatten `(name, ref)` to the ordered ref slots humanise binds to `{0}`, `{1}`, … per-viewer.
    let ref_args: Vec<ArtifactRef> = args.iter().map(|(_, r)| r.clone()).collect();
    Ok(humanise(
        resolver,
        tenant,
        region,
        templates,
        template_key,
        &ref_args,
        viewer,
        locale,
        at,
        channel,
    ))
}

/// **Render a [`RiskSummary`] per-viewer through Notif `humanise` (C9 — the HITL card risk text).**
/// The `hitl_gate.risk_summary` slot (a `(template_key, args)` pair, NEVER a raw string) lowers onto
/// the ONE templating surface. The returned [`HumanisedString`] is permission-/erasure-safe: a viewer
/// without `view` on the subject sees a tombstone, NEVER the title (NOTIF-D4, inherited for free).
#[allow(clippy::too_many_arguments)]
pub fn humanise_risk_summary(
    resolver: &dyn RefResolvePort,
    tenant: &TenantId,
    region: &Region,
    templates: &TemplateStore,
    risk: &RiskSummary,
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> Result<HumanisedString, RawAgentString> {
    humanise_with_args(
        resolver,
        tenant,
        region,
        templates,
        &risk.template_key,
        &risk.args,
        viewer,
        locale,
        at,
        channel,
    )
}

/// **Render an [`AgentMessage`] per-viewer through Notif `humanise` (C9 — any agent-authored
/// message).** The SAME lowering as the risk summary (one render path). Permission-/erasure-safe.
#[allow(clippy::too_many_arguments)]
pub fn humanise_agent_message(
    resolver: &dyn RefResolvePort,
    tenant: &TenantId,
    region: &Region,
    templates: &TemplateStore,
    msg: &AgentMessage,
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> Result<HumanisedString, RawAgentString> {
    humanise_with_args(
        resolver,
        tenant,
        region,
        templates,
        &msg.template_key,
        &msg.args,
        viewer,
        locale,
        at,
        channel,
    )
}

// ───────────────────────── the rendered card (the per-viewer projection of a HitlGate card) ───────

/// **A HITL card rendered for ONE viewer (C9 — the per-viewer card a person sees).** The structured
/// [`HitlCard`] (action + risk SLOT + cost + approvers) is projected to the human-facing card by
/// humanising its risk text PER-VIEWER. The non-text fields (cost, approvers) ride through unchanged;
/// the TEXT is the `HumanisedString` from Notif `humanise` — never a raw string. Two viewers with
/// different permissions get DIFFERENT `risk_text` (one a title, one a tombstone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedCard {
    /// the humanised risk text (per-viewer: a title for an authorised viewer, a tombstone otherwise).
    pub risk_text: HumanisedString,
    /// the pending action tool (a stable taxonomy token, not PII — rides through unchanged).
    pub action_tool: String,
    /// the LIVE cost estimate, integer minor-units (the bill the human approves with in view).
    pub cost_estimate: u64,
    /// the number of approvers (the structured count; the principal pseudonyms are not rendered text).
    pub approver_count: usize,
}

/// **Render a [`HitlCard`] for ONE viewer (C9/OQ-L — the card text through the ONE templating
/// surface).** Humanises the card's [`RiskSummary`] per-viewer through Notif `humanise`, then carries
/// the structured fields through. The card TEXT is NEVER a raw string; it is permission-/erasure-safe
/// (an unauthorised viewer sees a tombstone in `risk_text`, never the subject's title). The SAME card
/// renders DIFFERENTLY for two viewers with different permissions (the per-viewer gate is proven in
/// the tests).
#[allow(clippy::too_many_arguments)]
pub fn humanise_card(
    resolver: &dyn RefResolvePort,
    tenant: &TenantId,
    region: &Region,
    templates: &TemplateStore,
    card: &HitlCard,
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> Result<RenderedCard, RawAgentString> {
    let risk_text = humanise_risk_summary(
        resolver,
        tenant,
        region,
        templates,
        &card.risk_summary,
        viewer,
        locale,
        at,
        channel,
    )?;
    Ok(RenderedCard {
        risk_text,
        action_tool: card.action_tool.clone(),
        cost_estimate: card.cost_estimate,
        approver_count: card.approvers.len(),
    })
}

#[cfg(test)]
mod tests;
