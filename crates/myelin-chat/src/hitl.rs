//! # `hitl` — the chat HITL approval-card bridge (CHAT-P18 → P-413, M4-C6)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md` §5 (the
//! HITL approval-card bridge): the round-trip spans THREE owners; **Chat is the surface** (steps 2 +
//! 3). Step 2 — Chat renders the approval CARD in the thread/channel where the run is anchored AND
//! lands it as a Notif inbox item (`reason = approval_requested`, C-9). Step 3 — a human clicks
//! Approve / Reject; Chat gates the click with `Id.check(human, approve, run)` (contract 4.2), then
//! posts `DurableExecutor::signal(run, "approval:<card>", {decision}, idem_key = <PER-EFFECT KEY>)`
//! (contract 9.1 / 9.4) — THE BRIDGE. §5.2 (the batch / partial approval scheme — the FROZEN
//! per-effect `idem_key` rule, OQ-F): `idem_key = card_id` single / `card_id ":" effect_idx`
//! multi; a double-click is ONE approval; a partial approval is well-defined; each approved effect
//! maps to exactly one `EffectApi::apply`; a declined effect is WITHHELD (AG-8 — returns `Denied`,
//! never mutates). On resume the workflow runs under a freshly-minted attenuated token (contract 4.7,
//! re-mintable mid-workflow).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` §OQ-F (the per-effect `idem_key` rule —
//! `card_id` single, `card_id:<effect_idx>` multi/partial — frozen; a double-click is one approval,
//! a partial approval is well-defined). The KEY-CONSTRUCTION rule itself is owned BY THE WORKFLOW
//! ENGINE (`myelin_flow::per_effect_idem_key`, P-FLOW-10 → P-206); chat MUST agree with it byte-for-
//! byte. [`per_effect_idem_key`] here is the SAME frozen function — the CDC
//! `tests/cdc_9_1_9_4_chat_hitl.rs` asserts BYTE PARITY with `myelin_flow::per_effect_idem_key` so
//! there is ONE rule, not two divergent copies (the engine is the dev-dep; the runtime copy keeps the
//! production DAG acyclic — chat is a leaf consumer, it does NOT depend on `myelin-flow` in prod).
//!
//! **Contracts:** `contract-index.md` rows
//! - `9.1` / `9.4` `DurableExecutor::signal` (idempotent on `idem_key`, the per-effect rule + the
//!   durable HITL signal) — **CONSUMED** (the card posts the signal). Chat posts through the
//!   [`SignalPort`] seam so it depends on the TRAIT, never the concrete engine.
//! - `4.2` `check(human, approve, run)` (the approve gate) — **CONSUMED** ([`ClickGate`], fail-closed).
//! - `4.7` `mint_run_token` (the resume token) — **CONSUMED** ([`ResumeTokenMinter`]).
//! - `7.3` `humanise` (the card strings) — **CONSUMED** ([`render_card`] over the existing
//!   [`crate::glue`] HITL-card template keys — the ONE templating surface, OQ-L).
//! - `8.2` `EffectApi::apply` (one apply per APPROVED effect) — the routing the card posts INTO; OWNED
//!   in CHAT-P19. Here the card carries the per-effect decision; the apply is the engine's.
//!
//! ## What this prompt (CHAT-P18) ships — the card SURFACE (steps 2 + 3), nothing else
//!
//! - [`ChatApprovalCard`] / [`CardEffect`] — the chat-side card projection: the run it anchors, the
//!   `card_id` (the per-effect `idem_key` base), and the ordered gated effects (each a per-viewer
//!   SUBJECT ref + the PII-free action/risk/cost facets + the human's per-effect decision). All
//!   references-not-payloads (§3.4) — a restricted subject renders a TOMBSTONE, never a leak.
//! - [`render_card`] — render the card per-viewer: the SUBJECT line via Notif `humanise` (contract
//!   7.3, tombstone-on-deny, NOTIF-D4) + the action/risk/cost FACETS via [`crate::glue::
//!   chat_hitl_card_facets`] (PII-free literal agent strings through the ONE formatter). Chat renders
//!   NO string itself — it binds the ONE templating surface (OQ-L).
//! - [`ClickGate`] — gate the Approve/Reject click with `Id.check(human, approve, run)` (4.2),
//!   fail-closed (Deny / Conditional / Id-error all deny), Strong consistency (the new-enemy guard —
//!   a just-revoked approver cannot approve in the cache window).
//! - [`SignalPort`] — chat's port over `DurableExecutor::signal` (9.1/9.4). A re-click re-posts the
//!   SAME per-effect key → the engine's `ON CONFLICT DO NOTHING` dedups it → ONE buffered decision →
//!   one approval (a double-click is one approval, BY CONSTRUCTION).
//! - [`CardClick`] / [`post_decision`] — the bridge: a gated click on effect `idx` posts ONE
//!   `approval:<card>` signal under [`per_effect_idem_key`] carrying the decision (approve → the
//!   effect's refs; decline → the [`DECLINE_MARKER`], empty payload, AG-8 withhold). A
//!   [`CardOutcome::Withheld`] decline NEVER reaches the engine's apply.
//! - [`ResumeTokenMinter`] — mint a FRESH attenuated run token on resume (4.7) so a days-later
//!   approval runs under a fresh token, not a stale one. Chat mints the resume token; it does NOT own
//!   the wait/timer/budget/sandbox.
//! - [`auto_deny_on_timeout`] — the timeout AUTO-DENY: the durable timer (the engine's) fired first;
//!   chat surfaces the auto-deny as a [`CardDecision::Decline`] with the [`TIMEOUT_REASON`] marker (0
//!   mutation, AG-8). Chat does NOT own the timer — it renders the auto-deny outcome.
//!
//! ## The chat-owns-ONLY-the-card boundary (the FLOOR — stated, not re-implemented)
//!
//! Chat owns the **CARD**: the UI/render, the Approve/Reject affordance, the `Id.check(approve)`
//! gate, the `signal` post, the resume-token mint. Chat does **NOT** own — and MUST NOT re-implement —
//! the **durable wait**, the **timer**, the **budget / cost**, or the **withhold/resume logic**: those
//! are the M2 Workflow / Agent Fabric / Storage primitives (`wait_for_signal`, the durable timer,
//! `reserve`/`settle`, `EffectApi::apply` / the `HitlGate` state machine). The card carries the
//! per-effect DECISION; the engine consumes it. Re-implementing a wait or a budget here would be a
//! parallel second implementation (EI-01 §7) — forbidden. This module is the SURFACE, not the engine.
//!
//! ## FLOORS named
//! - **NONE NEW.** The card is chat's; the wait / timer / budget / sandbox are the M2 primitives
//!   (above). The exactly-once correctness across a multi-day kill is the ENGINE's durability
//!   (`myelin_flow`), proven there (FLOW-D4 / P-FLOW-11); chat's CDC drives the engine's REAL
//!   `FlowExecutor::signal` to prove the card's signal post lands exactly-once and the per-effect
//!   keys dedup (the chat face of CHAT-D9 / CHAT-D10). The `EffectApi::apply` ROUTING the approved
//!   effect lands in is OWNED in CHAT-P19 (the next prompt) — here the card carries the decision.

use crate::glue::chat_hitl_card_facets;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, DelegationCaveats, FailStaticBound, IdentityService,
    Permission, Principal, PrincipalId, RunId, RunToken, Zookie,
};
use myelin_notif::{humanise, Channel, HumanisedString, RefResolvePort, TemplateStore};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// **The FROZEN `permission` the click gate checks (contract 4.2 — `check(human, approve, run)`).**
/// A human may click Approve/Reject IFF they hold `approve` on the RUN the card gates — the approval
/// AUTHORITY gate (§5.1). A taxonomy token, not PII.
pub const APPROVE_PERMISSION: &str = "approve";

/// **The FROZEN `signal_name` prefix the card's decision is delivered under (§5.1 / arch §6.3).**
/// Each gated call's approval is a distinct wait `approval:<card>`; the per-effect `idem_key`
/// (single `card_id` / multi `card_id:<idx>`) separates the effects WITHIN the card. This is the
/// SAME `approval:<call>` name `myelin_flow::approval_wait_name` builds — chat agrees with the engine.
pub const APPROVAL_SIGNAL_PREFIX: &str = "approval";

/// **The machine marker a DECLINE / WITHHELD decision carries (§5.2, references-not-payloads §3.4).**
/// A declined effect's `approval` signal carries an EMPTY payload + this marker so the engine's gated
/// loop withholds it (AG-8) — `EffectApi::apply` is NEVER reached, ZERO mutation. The SAME
/// `myelin_flow::DECLINE_MARKER` value — chat agrees with the engine byte-for-byte (CDC-asserted).
pub const DECLINE_MARKER: &str = "decline";

/// **The reason marker a TIMEOUT auto-deny carries.** When the durable timer (the ENGINE's) fired
/// before the human decided, chat surfaces the auto-deny as a `Decline` carrying this marker — the
/// withhold is the timeout WORKING (0 mutation, AG-8), distinct from a human reject. A taxonomy token.
pub const TIMEOUT_REASON: &str = "timeout";

/// **The FROZEN `signal_name` an approval decision for `card_id` is delivered under (§5.1 / §6.3).**
/// `approval:<card_id>` — the SAME spelling `myelin_flow::approval_wait_name(card_id)` produces, so
/// the signal chat posts and the wait the engine parks on agree without coordination.
pub fn approval_signal_name(card_id: &str) -> String {
    format!("{APPROVAL_SIGNAL_PREFIX}:{card_id}")
}

/// **The FROZEN per-effect `idem_key` CONSTRUCTION rule (contract 9.1 / §6.4 / OQ-F).** The SAME
/// rule the workflow engine owns ([`myelin_flow::per_effect_idem_key`]) — chat agrees byte-for-byte
/// (the CDC asserts parity) so there is ONE rule, not two:
///
/// - a **single-effect** card (`total_effects == 1`) → `idem_key = card_id`. One approval; a
///   double-click re-posts `card_id` → the engine's `ON CONFLICT DO NOTHING` → one buffered decision.
/// - a **multi/partial-approval** card (`total_effects > 1`) → `idem_key = card_id ":" effect_idx`.
///   Each effect is approved/declined INDEPENDENTLY + idempotently on its OWN key; a partial approval
///   (approve 0 and 2, decline 1) is three signals under `card:0` / `card:1` / `card:2`.
///
/// Both invariants are true BY CONSTRUCTION: *a double-click is one approval* (the PK dedups the
/// per-effect key) and *a partial approval is well-defined* (each effect's decision rides its own
/// key). A single-effect card's lone effect is index 0 and keys on the BARE `card_id` (the `:0`
/// suffix is NOT appended — the degenerate per-effect case, §6.4).
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
        // Single-effect card: the key IS the card id (a double-click is one approval, §6.4).
        card_id.to_string()
    } else {
        // Multi/partial-approval card: each effect keys on `card_id ":" effect_idx`.
        format!("{card_id}:{effect_idx}")
    }
}

// ───────────────────────── the human's per-effect decision (§5.2) ─────────────────────────

/// **The human's per-effect decision (§5.2) carried in a card click.** `Approve` (the effect is
/// applied via `EffectApi::apply`) or `Decline` (the effect is WITHHELD — returns `Denied`, never
/// mutates, AG-8). A partial approval mixes both across a card's effects (approve 0 and 2, decline 1).
/// A TIMEOUT auto-deny is a `Decline` carrying the [`TIMEOUT_REASON`] marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardDecision {
    /// the gated effect is APPROVED — it maps to exactly one `EffectApi::apply`.
    Approve,
    /// the gated effect is DECLINED — WITHHELD (returns `Denied`, never mutates, AG-8).
    Decline,
}

/// **One gated effect within a chat approval card (§5.2).** Carries the references-not-payloads
/// SUBJECT (`ArtifactRef`, never an inline PII body — §3.4) the per-viewer `humanise` resolves
/// (tombstone-on-deny), the PII-free action/risk/cost FACETS the card surfaces (NOTIF-P9 — the human
/// approves a KNOWN action at a KNOWN risk for a KNOWN cost, never a blank cheque), and the effect's
/// references the engine's `EffectApi::apply` consumes on approve. The effect's position in
/// [`ChatApprovalCard::effects`] is its `effect_idx` for the per-effect key rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardEffect {
    /// the per-viewer SUBJECT ref the card renders (the effect's target artifact) — resolved
    /// per-viewer by `humanise` (a restricted subject → tombstone, never a leaked title, NOTIF-D4).
    pub subject: ArtifactRef,
    /// the proposed ACTION facet (the effect the agent wants applied: "merge", "archive-channel",
    /// "deploy") — a PII-free verb, never a per-viewer ref.
    pub action: String,
    /// the RISK band facet ("irreversible" / "reversible") — the L-ladder facet (recon §6). PII-free.
    pub risk: String,
    /// the metered COST estimate facet (the reserve/settle estimate, contract 11.7 — a KNOWN cost).
    /// A PII-free estimate string the AGENT FABRIC computed; chat surfaces it, never holds the wallet.
    pub cost: String,
    /// the effect references the engine's `EffectApi::apply` consumes on APPROVE (references-not-
    /// payloads, §3.4) — the apply target the per-effect approve signal carries. Empty on a decline
    /// (the withheld effect carries no payload, AG-8).
    pub effect_refs: Vec<ArtifactRef>,
}

/// **A chat HITL approval card (§5.1 / §5.2).** Gates one OR many effects under a single `card_id`,
/// anchored to the RUN that triggered it + the thread/channel where it renders (the `correlation_id`
/// anchoring rule, §5.1). A single-effect card (`effects.len() == 1`) keys on `card_id`; a
/// multi-effect card keys each effect on `card_id ":" effect_idx`. Chat owns ONLY this surface — the
/// durable wait/timer/budget/sandbox are the engine's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatApprovalCard {
    /// the run whose gated tool calls this card approves (the `Id.check(human, approve, run)` object).
    pub run_id: RunId,
    /// the card identity — the per-effect `idem_key` base (`card_id` single, `card_id:idx` multi) AND
    /// the `approval:<card_id>` signal-name base.
    pub card_id: String,
    /// the gated effects in order (the index is the `effect_idx` for the per-effect key rule).
    pub effects: Vec<CardEffect>,
}

impl ChatApprovalCard {
    /// The per-effect `idem_key` for effect `idx` of this card (the §6.4 rule applied to THIS card's
    /// arity). Panics-in-debug if `idx` is out of range.
    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }

    /// The `approval:<card_id>` signal name this card's decisions are delivered under.
    pub fn signal_name(&self) -> String {
        approval_signal_name(&self.card_id)
    }
}

// ───────────────────────── step 2: render the card per-viewer (contract 7.3) ─────────────────────

/// **The rendered chat approval card for ONE effect, per-viewer (§5.1 — step 2).** The SUBJECT line
/// is `humanise`d per-viewer (tombstone-on-deny, NOTIF-D4); the FACETS line is the PII-free
/// action/risk/cost bound through the ONE formatter. Two viewers with different permissions see the
/// SAME card render DIFFERENTLY (the per-viewer gate) — a viewer without `view` on the subject sees a
/// tombstone, never the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedCardEffect {
    /// the per-viewer SUBJECT line (`humanise` over [`crate::glue::TPL_CHAT_CARD`]) — a tombstone for
    /// a denied/erased subject, NEVER a leaked title. Carries the routable links the viewer MAY follow.
    pub subject_line: HumanisedString,
    /// the PII-free FACETS line (action / risk / cost) bound through the ONE Notif formatter over
    /// [`crate::glue::TPL_CHAT_CARD_FACETS`] — never ref-resolved (the facets are agent metadata).
    pub facets_line: String,
    /// the per-effect `idem_key` this effect's Approve/Reject click posts under (the §6.4 rule).
    pub idem_key: String,
}

/// **Render ONE effect of a chat approval card per-viewer (§5.1 step 2, contract 7.3).** The SUBJECT
/// is `humanise`d per-viewer through the ONE templating surface (the [`crate::glue::TPL_CHAT_CARD`]
/// key, `"Approval requested on {0}"`) — a denied/erased subject binds the slot to a TOMBSTONE, never
/// a title (NOTIF-D4 — 0 leak, inherited for free). The action/risk/cost FACETS are bound through the
/// SAME ONE formatter ([`chat_hitl_card_facets`] over [`crate::glue::TPL_CHAT_CARD_FACETS`]) — PII-free
/// literal agent strings, never ref-resolved. Chat renders NO string itself (OQ-L).
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
    // SUBJECT line — humanise resolves the per-viewer subject ref through the ONE surface; a denied
    // subject binds a tombstone (NOTIF-D4). The card's ONLY ref slot.
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
    // FACETS line — PII-free action/risk/cost through the SAME ONE formatter (never ref-resolved).
    let facets_line = chat_hitl_card_facets(templates, &effect.action, &effect.risk, &effect.cost);
    RenderedCardEffect {
        subject_line,
        facets_line,
        idem_key: card.idem_key_for(effect_idx),
    }
}

// ───────────────────────── step 3a: gate the click (contract 4.2) ─────────────────────────

/// **A click-gate failure — the human is NOT authorised to approve this run (fail-closed, ADR-03).**
/// Only an explicit `Allow` permits the click; a `Deny`, a `Conditional` (a caveat needing context —
/// never a silent allow), OR an Id-error ALL deny. NO effect is posted on a denied click — the gate
/// is the chokepoint between the click and the signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClickDenied {
    /// the run the click tried to approve (audit; a machine id, no PII).
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

/// **The Approve/Reject click AUTHORITY gate (§5.1 step 3, contract 4.2 — `check(human, approve,
/// run)`).** Generic over the frozen [`IdentityService`] ABI (the SAME seam [`crate::membership::
/// MembershipGate`] takes over `check`). Fail-closed: only an explicit `Ok(Decision::Allow)` permits
/// the click; `Deny` / `Conditional` / Id-error ALL deny. The check is at the run's stamped zookie
/// with [`ConsistencyMode::Strong`] so a JUST-REVOKED approver cannot approve in the cache window (the
/// new-enemy guard applies to the approval gate too).
pub struct ClickGate<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> ClickGate<I> {
    /// Compose the gate over the Id dependency.
    pub fn new(id: I) -> ClickGate<I> {
        ClickGate { id }
    }

    /// **Gate an Approve/Reject click (fail-closed, contract 4.2).** `clicker` must hold
    /// [`APPROVE_PERMISSION`] on the `card.run_id` at-or-after `at_zookie` (the run's stamped acl
    /// watermark, or the empty zookie). Returns `Ok(())` iff `Allow`; otherwise [`ClickDenied`]. A
    /// denied click posts NO signal — the run's gated tool stays withheld.
    pub fn check_click(
        &self,
        clicker: &Principal,
        card: &ChatApprovalCard,
        at_zookie: Option<&str>,
    ) -> Result<(), ClickDenied> {
        // The `check` object is the run the card gates — `run:<id>` (the approval-authority object,
        // §5.1). references-not-payloads: a machine id, never PII.
        let object = ArtifactRef(run_object(&card.run_id.0));
        let at = Consistency {
            at_least: Zookie(at_zookie.unwrap_or("").to_string()),
            // Strong: bypass the fail-static cache, honour the new-enemy guard (a revoked approver is
            // denied immediately, not after the cache window — §8.7).
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(APPROVE_PERMISSION.to_string());
        match self.id.check(clicker, &permission, &object, &at, None) {
            Ok(Decision::Allow) => Ok(()),
            // Fail-closed: Deny / Conditional / Id-error ALL deny (no silent approval).
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => Err(ClickDenied {
                run_id: card.run_id.0.clone(),
            }),
        }
    }
}

/// The `run:<id>` object id the approval gate's `check` resolves against (the approval-authority
/// object, §5.1). A taxonomy-shaped id, never PII.
pub fn run_object(run_id: &str) -> String {
    format!("run:{run_id}")
}

// ───────────────────────── step 3b: post the signal (contract 9.1 / 9.4) ─────────────────────────

/// **A signal the card posts to the durable run (the chat-side mirror of `myelin_flow::SignalSpec`).**
/// Carries the target `run`, the `approval:<card>` signal name, the per-effect `idem_key` (the §6.4
/// dedup anchor — a double-click re-posts the SAME key → the engine dedups → one approval), the
/// references-not-payloads `payload` (the approved effect's refs, never PII), and the
/// `payload_key_ref` (the [`DECLINE_MARKER`] for a WITHHELD effect — empty payload, AG-8). This is the
/// shape the [`SignalPort`] lowers onto `DurableExecutor::signal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardSignal {
    /// the run to deliver the decision to (the `card.run_id`).
    pub run_id: RunId,
    /// the `approval:<card_id>` signal name (the §6.3 spelling, agrees with the engine's wait name).
    pub signal_name: String,
    /// the per-effect `idem_key` (the §6.4 rule — the double-click-is-one-approval dedup anchor).
    pub idem_key: String,
    /// the decision payload as `ArtifactRef`s (the approved effect's refs; EMPTY on a decline — §3.4).
    pub payload: Vec<ArtifactRef>,
    /// the `DECLINE_MARKER` IFF the decision is a WITHHELD decline (AG-8); `None` on an approve.
    pub payload_key_ref: Option<String>,
}

/// **The outcome of delivering a card signal (the chat-side mirror of `myelin_flow::SignalOutcome`).**
/// Both variants are `Ok` — a re-delivery is the idempotency WORKING (a double-click), never an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalDelivery {
    /// the FIRST delivery under `(run, signal_name, idem_key)` — the decision was buffered (the
    /// workflow will wake once).
    Buffered,
    /// a RE-delivery under an already-buffered key — a no-op (the double-click → one approval).
    Duplicate,
}

/// **A signal-post failure (surfaced, never swallowed — EI-02 §4).** The durable executor rejected
/// the signal (an unknown run handle — a card posting to a phantom run). Surfaced so a
/// misconfiguration is observable, never a silently dropped approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalPostError {
    /// the executor-surfaced reason (a machine token, no PII).
    pub reason: String,
}

impl core::fmt::Display for SignalPostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "approval signal post failed: {}", self.reason)
    }
}

impl std::error::Error for SignalPostError {}

/// **The chat seam over `DurableExecutor::signal` (contract 9.1 / 9.4 — CONSUMED).** Chat depends on
/// THIS trait, never on the concrete `myelin_flow::FlowExecutor` — so chat stays a leaf consumer and
/// the §2.9 production DAG stays acyclic (the SAME seam discipline [`crate::composer::AutocompletePort`]
/// takes over Search). The production wire hands the real `FlowExecutor::signal` (proven in the CDC,
/// a dev-dep). The signal is idempotent on `idem_key` — a double-click re-posts the same per-effect
/// key → `Duplicate` (one approval). The post is the BRIDGE; the durable wait that consumes it is the
/// ENGINE's (P-FLOW-11), NOT chat's.
pub trait SignalPort {
    /// Deliver `signal` to the durable run, idempotent on `idem_key`. Returns
    /// [`SignalDelivery::Buffered`] for the first delivery, [`SignalDelivery::Duplicate`] for a
    /// re-delivery (both `Ok`); [`SignalPostError`] only for an engine-level failure (unknown run).
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError>;
}

/// **The terminal outcome of one effect's gated decision (§5.2).** `Approved` — the effect's approve
/// signal was posted (the engine's wait will run `EffectApi::apply` once); `Withheld` — the decline
/// signal was posted (the engine WITHHOLDS the effect, AG-8 — `apply` is NEVER reached, 0 mutation).
/// Carries the [`SignalDelivery`] so the caller can tell a first decision from a double-click no-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardOutcome {
    /// the effect was APPROVED — the approve signal was posted (the engine applies it once).
    Approved(SignalDelivery),
    /// the effect was WITHHELD (declined / timed-out) — the decline signal was posted; the engine
    /// withholds the effect (AG-8 — `EffectApi::apply` is NEVER reached, 0 mutation). Carries the
    /// reason marker ([`DECLINE_MARKER`] for a human reject, [`TIMEOUT_REASON`] for a timeout).
    Withheld(SignalDelivery, String),
}

/// **One human click on ONE effect of a card (§5.1 step 3).** The clicked `effect_idx`, the per-effect
/// `decision`, and (for a timeout auto-deny) the reason marker. The bridge ([`post_decision`])
/// validates the click is gated, builds the per-effect signal, and posts it through the [`SignalPort`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardClick {
    /// the effect this click decides (the index into [`ChatApprovalCard::effects`] — the per-effect
    /// `idem_key` is derived from it via the §6.4 rule).
    pub effect_idx: usize,
    /// the human's decision for THIS effect (approve | decline).
    pub decision: CardDecision,
    /// the decline reason marker — [`DECLINE_MARKER`] for a human reject, [`TIMEOUT_REASON`] for a
    /// timeout auto-deny. Ignored on an approve (the approve carries the effect's refs).
    pub decline_reason: String,
}

/// **Build the per-effect signal a card click posts (§5.1 step 3 / §5.2 — references-not-payloads).**
/// An APPROVE carries the effect's `effect_refs` (the apply target the engine's `EffectApi::apply`
/// consumes); a DECLINE carries an EMPTY payload + the decline-reason marker (the engine WITHHOLDS it,
/// AG-8 — `apply` is never reached, 0 mutation). The `idem_key` is the §6.4 per-effect key, so a
/// double-click re-posts the SAME key → the engine dedups → one approval.
pub fn build_card_signal(card: &ChatApprovalCard, click: &CardClick) -> CardSignal {
    let effect = &card.effects[click.effect_idx];
    let idem_key = card.idem_key_for(click.effect_idx);
    let signal_name = card.signal_name();
    match click.decision {
        CardDecision::Approve => CardSignal {
            run_id: card.run_id.clone(),
            signal_name,
            idem_key,
            // APPROVE: the effect's apply-target refs ride the payload (references-not-payloads §3.4).
            payload: effect.effect_refs.clone(),
            payload_key_ref: None,
        },
        CardDecision::Decline => CardSignal {
            run_id: card.run_id.clone(),
            signal_name,
            idem_key,
            // DECLINE: EMPTY payload + the decline-reason marker → the engine WITHHOLDS it (AG-8). The
            // effect makes ZERO mutation — `EffectApi::apply` is NEVER reached.
            payload: vec![],
            payload_key_ref: Some(click.decline_reason.clone()),
        },
    }
}

/// **The bridge: post ONE gated card decision (§5.1 steps 2→3 / contract 9.1 / 9.4).** This is the
/// chat-owned round-trip surface — it (1) GATES the click with the supplied [`ClickGate`]
/// (`Id.check(human, approve, run)`, 4.2 — fail-closed; a denied click posts NOTHING), (2) BUILDS the
/// per-effect signal ([`build_card_signal`] — approve carries the effect refs, decline withholds with
/// the marker, AG-8), and (3) POSTS it through the [`SignalPort`] (`DurableExecutor::signal`,
/// idempotent on the §6.4 per-effect `idem_key` — a double-click is ONE approval).
///
/// Returns the [`CardOutcome`] (`Approved` / `Withheld`) carrying the [`SignalDelivery`] (`Buffered`
/// first, `Duplicate` on a re-click). A [`ClickDenied`] short-circuits BEFORE any signal post (the
/// gate is the chokepoint). Chat owns ONLY this post — the durable wait that CONSUMES the buffered
/// decision and runs / withholds the effect is the ENGINE's (P-FLOW-11), NOT chat's.
pub fn post_decision<I: IdentityService, P: SignalPort>(
    gate: &ClickGate<I>,
    port: &P,
    card: &ChatApprovalCard,
    click: &CardClick,
    clicker: &Principal,
    at_zookie: Option<&str>,
) -> Result<CardOutcome, PostDecisionError> {
    // (1) GATE the click — fail-closed; a denied click posts NO signal (the run stays withheld).
    gate.check_click(clicker, card, at_zookie)
        .map_err(PostDecisionError::Denied)?;
    // (2) BUILD the per-effect signal (approve → effect refs; decline → withhold marker, AG-8).
    let signal = build_card_signal(card, click);
    // (3) POST through the SignalPort (idempotent on the per-effect idem_key — double-click is one).
    let delivery = port.post_signal(&signal).map_err(PostDecisionError::Post)?;
    Ok(match click.decision {
        CardDecision::Approve => CardOutcome::Approved(delivery),
        CardDecision::Decline => CardOutcome::Withheld(delivery, click.decline_reason.clone()),
    })
}

/// **A `post_decision` failure — a denied click OR a signal-post failure (surfaced, EI-02 §4).** A
/// `Denied` short-circuits before any post (the approval-authority gate); a `Post` surfaces an
/// engine-level signal failure (an unknown run). Neither is ever swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostDecisionError {
    /// the click failed the `Id.check(human, approve, run)` gate (4.2) — NO signal was posted.
    Denied(ClickDenied),
    /// the signal post failed at the durable executor (unknown run) — surfaced, never a dropped
    /// approval.
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

/// **The TIMEOUT auto-deny click (§5.1 — "timeout auto-denies").** When the durable timer (the
/// ENGINE's — chat does NOT own it) fires before the human decides, chat surfaces the auto-deny as a
/// `Decline` carrying the [`TIMEOUT_REASON`] marker. This builds the click; the resulting decline
/// posts an empty payload + the timeout marker → the engine WITHHOLDS the effect (0 mutation, AG-8).
/// Chat renders the auto-deny outcome; it does NOT implement the timer.
pub fn auto_deny_on_timeout(effect_idx: usize) -> CardClick {
    CardClick {
        effect_idx,
        decision: CardDecision::Decline,
        decline_reason: TIMEOUT_REASON.to_string(),
    }
}

// ───────────────────────── step 4: mint the resume token (contract 4.7) ─────────────────────────

/// **Mint the FRESH attenuated run token a days-later approval resumes under (§5.1 / contract 4.7).**
/// On resume the workflow re-mints its attenuated agent token (4.7, re-mintable mid-workflow) so an
/// approval that arrives DAYS later runs under a FRESH token, not a stale one (a long-parked run's
/// original token may have been revoked/expired; the resume token carries the SAME attenuated scope,
/// freshly minted). Chat MINTS the resume token through the [`IdentityService`] seam; it does NOT own
/// the run lifecycle — the engine re-leases the run, chat hands it a fresh token.
///
/// The token's TTL is bounded by the fail-static window W (4.11) so a revoked resume token expires
/// inside the revocation SLA. The `delegation_caveats` are the delegating human's attenuate-only
/// grant (the SAME scope the original run held — a resume cannot ESCALATE).
pub struct ResumeTokenMinter<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> ResumeTokenMinter<I> {
    /// Compose the minter over the Id dependency.
    pub fn new(id: I) -> ResumeTokenMinter<I> {
        ResumeTokenMinter { id }
    }

    /// **Mint a fresh resume token for `run_id` under `agent_id` (4.7).** Called on resume (a
    /// days-later approval) so the gated tool runs under a FRESH attenuated token, not a stale one.
    /// The `caveats` are the original attenuate-only scope (a resume cannot escalate); the TTL is the
    /// fail-static bound W (4.11) so a revoked token expires inside the SLA.
    pub fn mint_resume_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &RunId,
        caveats: &DelegationCaveats,
    ) -> myelin_identity::Result<RunToken> {
        // The resume token life == run life (4.7), TTL bounded by W (4.11) — a revoked resume token
        // expires inside the revocation SLA, never outlives the run.
        self.id
            .mint_run_token(agent_id, run_id, caveats, &FailStaticBound::DEFAULT_W)
    }
}

#[cfg(test)]
mod tests;
