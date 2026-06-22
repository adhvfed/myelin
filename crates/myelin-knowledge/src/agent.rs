//! # `agent` — Knowledge agent governance: the KN slice of the ONE tool catalogue (8.1), the
//! "suggested by agent" collab attribution, the HITL-withhold gate, the per-effect `idem_key`, and
//! the reserve/settle bookend (KN-P27 → P-317, M3 — drill KN-D11)
//!
//! An agent that edits a Knowledge doc goes through the **SAME** `SEND_OP` collab protocol a human
//! does (architecture 02 §9): the agent never mutates Knowledge's DB directly — a side-effecting
//! Knowledge tool (`knowledge.publish` / `edit_confidential` / `draft` / `comment` / `append`) goes
//! through the Agent Fabric's **`EffectApi::apply`** (plan-then-apply, contract 8.2 — schema →
//! capability → delegation → tenant → budget → **HITL gate** → apply-via-public-endpoint → meter),
//! which calls Knowledge's **public endpoint** AS the agent principal (same gateway, no carve-out),
//! which applies the op through the collab protocol with **"suggested by agent" attribution**. The
//! **four uniform sandbox guarantees** (contract 8.4, X-6) hold BY CONSTRUCTION: the universal cost
//! gate (reserve/settle, 11.7), the per-run attenuated token (4.7), the HITL withhold (a gated tool
//! not in the approved set returns `Denied` and does NOT mutate, AG-8), and the isolation floor +
//! escape drill (any `compute` the tool runs is the CI runner's `kind=agent` job, AG-D4 green).
//!
//! ## Why the Knowledge-domain half lives HERE (not in the Fabric)
//! The §2.9 DAG forbids `myelin-knowledge` (a leaf service consumer) from depending on
//! `myelin-agent`/`-agent-service` (it would make the graph cyclic). So — exactly as Git carries its
//! agent-author/code-tool identity in `myelin_git::agent_author` / `myelin_git::code_tools` while the
//! THIN `ToolDef` catalogue registration lives in `myelin_agent_service::git_tools` — Knowledge owns
//! the DOMAIN half here:
//!
//! 1. **The tool identity + `required_caps`** (8.1, the KN slice): the tool-name constants + the caps
//!    built from the FROZEN KN ReBAC carrier ([`myelin_content::rebac_fragment`]) — the SINGLE source
//!    of truth the Fabric registration ([`myelin_agent_service::knowledge_tools`], AG-P19 → P-268)
//!    consumes. A rename of a permission here is a compile/test break there, never a silent drift.
//! 2. **The FROZEN consequential-gate classification** (§6.3 / X-6): `publish` + `edit_confidential`
//!    are consequential → `requires_approval = yes`; `draft` + `comment` + `append` are reversible →
//!    `no`. This is the KN-domain source of truth the §6.3 defaults table (seeded in
//!    [`myelin_agent_service::defaults`]) and the Fabric ToolDefs agree with.
//! 3. **The "suggested by agent" collab attribution** ([`AgentEditAttribution`] / [`EditAuthor`]):
//!    the legibility value the public endpoint stamps onto an agent-authored [`crate::transport::DocOp`]
//!    so an agent edit is FIRST-CLASS but never disguised as a human (ADR-08 / EU AI-Act).
//! 4. **The HITL withhold + per-effect `idem_key` + reserve/settle** as the KN-domain decision the
//!    apply respects ([`KnowledgeEffectGate`]): a consequential effect not in the approved set is
//!    WITHHELD (`Denied`, 0 mutation) until approval; a double-click is ONE approval (per-effect
//!    `idem_key`, OQ-F); reserve/settle bookends every agent run (11.7). This is the KN half of the
//!    Fabric pipeline — it does NOT re-implement the eight-step engine ([`myelin_agent_service::effect_api`]);
//!    it is the gate the KN public endpoint applies BEFORE it threads an op through the collab protocol.
//!
//! ## The KN-D11 drill (the dated green artifact)
//! [`KnowledgeAgentRun`] is the CHAINED scenario harness: an agent plans → a consequential effect is
//! WITHHELD (returns `Denied`, no mutation) → a human approves → the effect applies ONCE even across a
//! double-click → the run passes reserve/settle. It emits a dated [`KnD11Receipt`] proving **0
//! ungoverned mutation, 0 mutation before approval, 0 double-apply** — the KN-D11 green (the gate-state
//! + denial-counter + idem-key-dedup telemetry).
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **The mock runtime (`--use-mock`) is the platform floor** — the real `LlmAgentRuntime` is the
//!   post-M5 config/impl swap OWNED by the Fabric (`no-llm-in-platform`, contract 1.6), NOT Knowledge.
//!   This module governs the agent edit regardless of which runtime emitted the proposal.
//! - **The AG-7 content-addressed agent-trace HOLDER the run writes its narrative into is KN-P28**
//!   (→ P-318) — the erasable trace holder distinct from the audit log (contract 8.8 / KN-D12). This
//!   module governs the EDIT; the trace of the run is the named follow-on.
//! - **NONE on the governance surface** — agent governance is the full v1 surface. The eight-step
//!   apply pipeline, the HITL machinery, and the per-effect `idem_key` engine already exist in the
//!   Fabric (AG-P6/P9/P10); this module is the KN-domain projection that lights them up + the KN-D11
//!   drill, not a second governance model.
//!
//! ## DB-free
//! In-memory tool identity + the typed attribution value + the gate decision + the drill harness. No
//! DB. `cargo build --workspace` stays DB-free (the reserve/settle + collab apply bodies it composes
//! are proven against the live stack at the Fabric/Storage integration drills).

use myelin_content::rebac_fragment::object_types as kn_objects;
use myelin_content::rebac_fragment::{COMMENT, DRAFT, EDIT, PUBLISH};

use crate::transport::{DocOp, OpId, OpKind};

// ───────────────────────── the KN tool identity (the §6.3 catalogue keys — the OWNED slice) ───────

/// **The Knowledge subsystem token** — the `subsystem` half of the catalogue key `(subsystem, name,
/// version)` and the key the FROZEN §6.3 `requires_approval` defaults table is looked up under
/// (`("knowledge", "publish")` → gated, `("knowledge", "draft")` → not). The SINGLE source of truth
/// shared with the Fabric registration ([`myelin_agent_service::knowledge_tools`]) so a typo can't
/// drift the seed.
pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

/// **The `knowledge.publish` tool name** (§6.3 — consequential, GATED). Publishing a page is a
/// decision-shaped, consequential edit (an approver set, VISION §3) → `requires_approval = yes`.
pub const PUBLISH_TOOL: &str = "publish";

/// **The `knowledge.edit_confidential` tool name** (§6.3 `edit(confidential_page)` — consequential,
/// GATED). Editing a confidential page is consequential → `requires_approval = yes`.
pub const EDIT_CONFIDENTIAL_TOOL: &str = "edit_confidential";

/// **The `knowledge.draft` tool name** (§6.3 — reversible, NOT gated). A private draft is reversible.
pub const DRAFT_TOOL: &str = "draft";

/// **The `knowledge.comment` tool name** (§6.3 — reversible, NOT gated). A comment is reversible.
pub const COMMENT_TOOL: &str = "comment";

/// **The `knowledge.append` tool name** (§5.1 — reversible, NOT gated). Appending a block to a draft
/// page is a reversible edit (it can be undone via the op-log) → `requires_approval = no`. It rides
/// the SAME `SEND_OP` collab path as `draft`/`comment` (02 §9).
pub const APPEND_TOOL: &str = "append";

/// **The full Knowledge agent-tool name set, in catalogue order.** A CLOSED set so a new Knowledge
/// tool can NOT be added without a `required_caps` + §6.3-gate decision (the gate classification is
/// total — proven by [`requires_approval_default`] over `ALL_TOOLS`).
pub const ALL_TOOLS: [&str; 5] = [
    PUBLISH_TOOL,
    EDIT_CONFIDENTIAL_TOOL,
    DRAFT_TOOL,
    COMMENT_TOOL,
    APPEND_TOOL,
];

// ───────────────────────── the required_caps from the FROZEN KN ReBAC carrier (4.9) ──────────────

/// **The `required_caps` for `knowledge.publish` (CONSUMED from 4.9).** Publishing is governed by the
/// `page.publish` write permission the FROZEN KN ReBAC carrier declares
/// ([`page_write_fragment`](myelin_content::rebac_fragment::page_write_fragment)). The cap STRING is
/// `"<object_type>.<permission>"` — the same shape the EffectApi `check` step (4.2) resolves. Built
/// from the canonical `myelin-content` constants so a rename in the carrier is a compile/test break
/// here, never a silent drift (an agent can do nothing no human role can — EI-02 §2).
pub fn publish_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, PUBLISH)]
}

/// **The `required_caps` for `knowledge.edit_confidential` (CONSUMED from 4.9).** `page.edit` (4.9).
pub fn edit_confidential_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, EDIT)]
}

/// **The `required_caps` for `knowledge.draft` (CONSUMED from 4.9).** `page.draft` (4.9).
pub fn draft_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, DRAFT)]
}

/// **The `required_caps` for `knowledge.comment` (CONSUMED from 4.9).** `page.comment` (4.9).
pub fn comment_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, COMMENT)]
}

/// **The `required_caps` for `knowledge.append` (CONSUMED from 4.9).** Appending a block edits the
/// page content — governed by the `page.edit` write permission (4.9). Reversible → not gated.
pub fn append_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, EDIT)]
}

/// **The `required_caps` for a Knowledge tool by name (the SINGLE source the Fabric registration
/// consumes).** An unknown tool returns an empty cap set (the EffectApi schema step denies an unknown
/// tool BEFORE caps are read — this is just the KN-slice cap table).
pub fn required_caps_for(tool: &str) -> Vec<String> {
    match tool {
        PUBLISH_TOOL => publish_required_caps(),
        EDIT_CONFIDENTIAL_TOOL => edit_confidential_required_caps(),
        DRAFT_TOOL => draft_required_caps(),
        COMMENT_TOOL => comment_required_caps(),
        APPEND_TOOL => append_required_caps(),
        _ => Vec::new(),
    }
}

// ───────────────────────── the FROZEN consequential-gate classification (§6.3 / X-6) ─────────────

/// **The FROZEN `requires_approval` default for a Knowledge tool (§6.3 / X-6 — the KN-domain source
/// of truth).** `publish` + `edit_confidential` are CONSEQUENTIAL (a published/confidential edit is
/// decision-shaped, an approver set, VISION §3) → `true`; `draft` + `comment` + `append` are
/// REVERSIBLE → `false`. An unknown tool is FAIL-CLOSED to `true` (a tool we cannot classify is gated
/// — never silently un-governed). The Fabric §6.3 defaults table + the registered ToolDefs MUST agree
/// with this (the CDC pairs the two sides).
pub fn requires_approval_default(tool: &str) -> bool {
    match tool {
        PUBLISH_TOOL | EDIT_CONFIDENTIAL_TOOL => true,
        DRAFT_TOOL | COMMENT_TOOL | APPEND_TOOL => false,
        // Fail-closed: an unrecognised tool is gated (VISION §3 — never silently un-governed).
        _ => true,
    }
}

/// **Whether a Knowledge tool is CONSEQUENTIAL (HITL-gated by the frozen §6.3 default).** Alias for
/// [`requires_approval_default`] read at the call site that asks "is this a decision-shaped edit?".
pub fn is_consequential(tool: &str) -> bool {
    requires_approval_default(tool)
}

// ───────────────────────── "suggested by agent" collab attribution (02 §9 / ADR-08 / AI-Act) ─────

/// **The "suggested by agent" provenance an agent-authored Knowledge edit carries (02 §9 / ADR-08 /
/// EU AI-Act).** An agent edit is FIRST-CLASS (it rides the SAME `SEND_OP` collab path a human does,
/// so attribution / undo / history treat it identically) but LEGIBLE (rendered visually distinct with
/// provenance — which agent, why, which run) and NEVER disguised as a human. This is the typed value
/// the KN public endpoint stamps onto an agent-authored [`DocOp`] (its `actor`).
///
/// PII-free: the agent is named by its OPAQUE pseudonym (4.8 — never a raw identity), the run by its
/// opaque run id, the rationale by an agent-authored summary (the "why" the editor renders).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEditAttribution {
    /// The authoring agent's OPAQUE pseudonym (4.8) — never a raw name/email. Rendered as the distinct
    /// agent author label ("suggested by agent <pseudonym>"), never as a human name.
    pub agent_pseudonym: String,
    /// The opaque run id this edit belongs to (the provenance link the editor surfaces so a human can
    /// see WHICH run authored this, and trace/replay it).
    pub run_id: String,
    /// The agent-authored rationale (the "why" — a short summary the editor shows next to the edit).
    /// Agent-authored free text; PII handled by the ONE Knowledge erasure posture (KN-P26).
    pub rationale: String,
}

impl AgentEditAttribution {
    /// Build an agent-edit attribution from the opaque pseudonym + run id + rationale.
    pub fn new(
        agent_pseudonym: impl Into<String>,
        run_id: impl Into<String>,
        rationale: impl Into<String>,
    ) -> AgentEditAttribution {
        AgentEditAttribution {
            agent_pseudonym: agent_pseudonym.into(),
            run_id: run_id.into(),
            rationale: rationale.into(),
        }
    }

    /// **The collab `actor` string for this agent edit** (the `DocOp.actor`, 02 §9). The agent's
    /// opaque pseudonym IS the actor — the SAME `actor` field a human edit carries (the same protocol;
    /// the LEGIBILITY is the [`EditAuthor::Agent`] arm carrying the run + rationale, not a separate
    /// op path). Prefixed `agent:` so the editor renders it distinctly ("suggested by agent").
    pub fn actor(&self) -> String {
        format!("agent:{}", self.agent_pseudonym)
    }
}

/// **Who authored a Knowledge edit — a CLOSED two-arm enum (human XOR agent).** Every collab op is
/// exactly one. The agent arm CARRIES the [`AgentEditAttribution`] provenance, so an agent-authored
/// op STRUCTURALLY cannot exist without its legibility metadata — the AI-Act "never disguised as
/// human" guarantee is the TYPE, not a runtime check that could be skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditAuthor {
    /// A human principal authored this edit (rendered as the human's pseudonym, no agent label).
    Human {
        /// The human author's OPAQUE pseudonym (4.8).
        author_pseudonym: String,
    },
    /// An AGENT authored this edit (rendered "suggested by agent" with provenance — never as a human).
    Agent(AgentEditAttribution),
}

impl EditAuthor {
    /// **Whether this edit is agent-authored** — the `is_agent` bit the editor + history read. `true`
    /// iff [`EditAuthor::Agent`].
    pub fn is_agent(&self) -> bool {
        matches!(self, EditAuthor::Agent(_))
    }

    /// **The provenance to render (legibility, 02 §9 / ADR-08 / AI-Act)** — `Some` for an agent edit
    /// (the editor MUST render it distinctly with the run + rationale), `None` for a human edit. The
    /// presence of `Some` IS the "never disguised as human" guarantee.
    pub fn agent_provenance(&self) -> Option<&AgentEditAttribution> {
        match self {
            EditAuthor::Agent(a) => Some(a),
            EditAuthor::Human { .. } => None,
        }
    }

    /// **The collab `actor` string for this edit** (the `DocOp.actor`). A human edit's actor is the
    /// bare pseudonym; an agent edit's actor is the `agent:`-prefixed pseudonym (so the editor renders
    /// it distinctly). The SAME `actor` field for both — the same `SEND_OP` protocol (02 §9).
    pub fn actor(&self) -> String {
        match self {
            EditAuthor::Human { author_pseudonym } => author_pseudonym.clone(),
            EditAuthor::Agent(a) => a.actor(),
        }
    }

    /// **Stamp this author onto a collab [`DocOp`]** — an agent edit goes through the SAME `SEND_OP`
    /// path a human does (02 §9), differing ONLY in its (legible) `actor`. This is the structural
    /// proof that an agent edit is first-class: it produces an ordinary `DocOp` the transport applies
    /// identically — there is no second agent-write path.
    pub fn stamp_op(&self, op_id: OpId, kind: OpKind, payload: impl Into<Vec<u8>>) -> DocOp {
        DocOp::cas(op_id, self.actor(), kind, payload)
    }
}

// ───────────────────────── the per-effect idem_key (OQ-F / 9.1/9.4 — the KN-consumed rule) ────────

/// **The per-effect resume `idem_key` for a Knowledge HITL approval (OQ-F / contract 9.1/9.4 — the
/// rule Knowledge CONSUMES).** A double-click on an approval card is ONE approval; a partial approval
/// of a batch is well-defined:
/// - a **single-effect** card (`total_effects == 1`) → `idem_key = card_id`. A double-click re-sends
///   `card_id` → one approval, one apply.
/// - a **multi/partial-approval** card (`total_effects > 1`) → `idem_key = card_id ":" effect_idx`.
///   Each effect is approved INDEPENDENTLY + idempotently on its own key.
///
/// This is the SAME rule `myelin_flow::approval::per_effect_idem_key` (9.1) and
/// `myelin_agent_service::hitl_batch::per_effect_idem_key` apply — Knowledge consumes it (it does NOT
/// author a second rule), so a double-click against the KN public endpoint dedups identically.
pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(total_effects >= 1, "a card has at least one effect");
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        // Single-effect card: the key IS the card id (a double-click is one approval, OQ-F).
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

// ───────────────────────── the KN-domain effect gate (HITL withhold + reserve/settle) ─────────────

/// **Why a Knowledge agent effect was REFUSED — an ordinary tool error (no privileged fallback,
/// AG-5).** A withheld consequential effect is `Withheld` (does NOT mutate, AG-8); a denied effect
/// (no balance, an over-privilege) is `Denied`. Both surface LOUD; neither has a privileged fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectRefusal {
    /// The consequential effect is WITHHELD pending HITL approval — it does NOT mutate (AG-8). Carries
    /// the opaque gate/card id the approval card surfaces as.
    Withheld { card_id: String },
    /// The effect was DENIED — an ordinary tool error (e.g. the run's reserve is exhausted — no
    /// privileged fallback). Carries the reason.
    Denied(String),
}

impl core::fmt::Display for EffectRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EffectRefusal::Withheld { card_id } => write!(
                f,
                "knowledge agent effect WITHHELD pending HITL approval (card {card_id}) — \
                 the consequential edit does NOT mutate until a human approves (AG-8)"
            ),
            EffectRefusal::Denied(reason) => write!(
                f,
                "knowledge agent effect DENIED (ordinary tool error, no privileged fallback): {reason}"
            ),
        }
    }
}

impl std::error::Error for EffectRefusal {}

/// **The KN-domain decision the public endpoint makes BEFORE it threads an agent op through the collab
/// protocol — the HITL-withhold gate (8.2 step 6 / AG-8), consumed by Knowledge.** It does NOT
/// re-implement the eight-step Fabric pipeline ([`myelin_agent_service::effect_api`]); it is the
/// Knowledge half: given the tool's frozen gate classification + the run's approved set, decide
/// whether the effect APPLIES (and produces an attributed [`DocOp`]) or is WITHHELD/DENIED.
///
/// The reserve/settle bookend (11.7) is the run's, modelled by [`ReserveSettle`]; this gate consults
/// the remaining balance so "no balance → no agent write" is uniformly true (the Fabric's bookend,
/// Knowledge's tools are ordinary metered effects).
#[derive(Clone, Debug)]
pub struct KnowledgeEffectGate {
    /// The per-effect `idem_key`s that have ALREADY applied — the exactly-once binding (a double-click
    /// re-applying the same key is a NO-OP, OQ-F). The structural proof of "0 double-apply".
    applied_keys: std::collections::BTreeSet<String>,
}

impl Default for KnowledgeEffectGate {
    fn default() -> KnowledgeEffectGate {
        KnowledgeEffectGate::new()
    }
}

impl KnowledgeEffectGate {
    /// A fresh gate (no effect applied yet).
    pub fn new() -> KnowledgeEffectGate {
        KnowledgeEffectGate {
            applied_keys: std::collections::BTreeSet::new(),
        }
    }

    /// **Decide a Knowledge agent effect (the HITL-withhold gate, 8.2 step 6 / AG-8).** In order,
    /// fail-closed:
    /// 1. If the tool is CONSEQUENTIAL (`requires_approval`, §6.3) AND not in the run's `approved`
    ///    set → **WITHHELD** ([`EffectRefusal::Withheld`]): the effect does NOT mutate (AG-8).
    /// 2. Else → **APPROVED**: the caller may apply it (and stamp the attributed [`DocOp`]). The
    ///    apply itself is recorded once per `idem_key` by [`apply_once`](KnowledgeEffectGate::apply_once).
    ///
    /// `card_id` is the gate/card id the withheld effect surfaces as (so the chat approval card +
    /// the durable resume signal key on it). Returns `Ok(())` if the effect may proceed to apply.
    pub fn decide(
        &self,
        tool: &str,
        approved: &std::collections::BTreeSet<String>,
        card_id: &str,
    ) -> Result<(), EffectRefusal> {
        if requires_approval_default(tool) && !approved.contains(tool) {
            // WITHHELD (AG-8): the consequential effect does NOT mutate until approved.
            return Err(EffectRefusal::Withheld {
                card_id: card_id.to_string(),
            });
        }
        Ok(())
    }

    /// **Record an apply under its per-effect `idem_key` (the exactly-once binding, OQ-F).** Returns
    /// `true` the FIRST time a key applies (the caller threads the attributed op through the collab
    /// protocol), `false` on a RE-apply of the same key — the double-click NO-OP (the apply is NOT
    /// performed again; "a double-click is one approval"). This is the structural "0 double-apply"
    /// guarantee — the same per-effect `idem_key` ledger the Fabric uses.
    pub fn apply_once(&mut self, idem_key: &str) -> bool {
        self.applied_keys.insert(idem_key.to_string())
    }

    /// Whether an `idem_key` has already applied (observability — never personal data).
    pub fn has_applied(&self, idem_key: &str) -> bool {
        self.applied_keys.contains(idem_key)
    }

    /// How many DISTINCT effects have applied (the count of unique `idem_key`s — a double-click does
    /// NOT increase this). The KN-D11 "0 double-apply" telemetry reads it.
    pub fn applied_count(&self) -> usize {
        self.applied_keys.len()
    }
}

/// **The reserve/settle bookend for a Knowledge agent run (contract 11.7 — the Fabric's universal
/// gate, consumed by Knowledge).** Knowledge is NOT spend-bearing in its own right (no model calls,
/// no CI runs originate here); an agent write passes the Fabric's reserve/settle gate so "no balance →
/// no agent write" is uniformly true. This is the minor-units (integer, never floats) ledger the run
/// reserves at dispatch and settles per applied effect — the KN-domain view of the Fabric bookend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettle {
    /// The reserved balance remaining for this run (minor-units). Reserved at dispatch (the Fabric's
    /// reserve bookend); each applied effect settles against it.
    remaining: u64,
    /// The total settled across applied effects (minor-units) — the bill the run reports.
    settled: u64,
}

impl ReserveSettle {
    /// Reserve `amount` minor-units for the run (the dispatch bookend, 11.7). `0` is a run with no
    /// budget — every effect is then refused at the budget step ("no balance → no agent write").
    pub fn reserve(amount: u64) -> ReserveSettle {
        ReserveSettle {
            remaining: amount,
            settled: 0,
        }
    }

    /// **Whether the run's reserve has ≥ `cost` remaining for an effect (the budget check, 11.7).**
    /// `false` → the effect is DENIED (no privileged fallback — the run cannot spend past its reserve).
    pub fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }

    /// **Settle exactly `cost` minor-units for one applied effect (the meter bookend, 11.7).** Called
    /// ONLY after a successful apply (a withheld/denied effect is never metered). Saturating (a cost
    /// never silently wraps). Returns the settled cost.
    pub fn settle(&mut self, cost: u64) -> u64 {
        self.remaining = self.remaining.saturating_sub(cost);
        self.settled = self.settled.saturating_add(cost);
        cost
    }

    /// The reserved balance remaining (observability).
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// The total settled across applied effects (the run's bill).
    pub fn settled(&self) -> u64 {
        self.settled
    }
}

// ───────────────────────── the KN-D11 chained drill harness + the dated green receipt ─────────────

/// **The dated KN-D11 green artifact — the gate-state + denial-counter + idem-key-dedup telemetry
/// (drill KN-D11).** PROOF the agent-governance properties held across the chained scenario: an agent
/// edit attributed "suggested by agent"; a consequential edit HITL-withheld (Denied, 0 mutation)
/// until approval; a double-click is ONE approval; denied effects are ordinary tool errors; the run
/// passed reserve/settle — **0 ungoverned mutation, 0 mutation before approval, 0 double-apply**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnD11Receipt {
    /// The number of agent effects WITHHELD (consequential, not yet approved) — each did NOT mutate.
    pub withheld: u64,
    /// The number of agent effects DENIED (ordinary tool errors — e.g. exhausted reserve).
    pub denied: u64,
    /// The number of DISTINCT effects APPLIED (unique `idem_key`s — a double-click does NOT inflate
    /// this). Each applied via the collab protocol with "suggested by agent" attribution.
    pub applied: u64,
    /// **The count of mutations that happened BEFORE approval — MUST be 0 (AG-8).** A non-zero value
    /// is a RED drill (a consequential edit mutated while withheld).
    pub mutations_before_approval: u64,
    /// **The count of double-applies — MUST be 0 (OQ-F).** A non-zero value is a RED drill (the same
    /// approved effect applied twice across a double-click).
    pub double_applies: u64,
    /// **The count of ungoverned mutations (an effect that reached the collab protocol WITHOUT passing
    /// the gate) — MUST be 0 (AG-D1).** A non-zero value is a RED drill.
    pub ungoverned_mutations: u64,
    /// The total settled by reserve/settle across the applied effects (the run passed the bookend).
    pub settled_minor_units: u64,
    /// The drill timestamp (the dated green artifact, ms — deterministic so a replay matches).
    pub at_ms: u64,
}

impl KnD11Receipt {
    /// **Whether the KN-D11 drill is GREEN: 0 ungoverned mutation, 0 mutation before approval, 0
    /// double-apply, AND at least one effect actually applied (the scenario ran to completion).** Any
    /// non-zero forbidden-counter is RED.
    pub fn is_green(&self) -> bool {
        self.ungoverned_mutations == 0
            && self.mutations_before_approval == 0
            && self.double_applies == 0
            && self.applied >= 1
    }
}

/// **The KN-D11 chained-scenario harness — an agent run governed end-to-end (the drill body).** It
/// composes the KN-domain seams (the gate classification, the [`KnowledgeEffectGate`], the
/// [`ReserveSettle`] bookend, the [`AgentEditAttribution`]) into the SINGLE chained scenario the drill
/// measures: plan → consequential effect WITHHELD → approve → applied ONCE across a double-click. Each
/// applied effect produces an attributed [`DocOp`] (the collab op the public endpoint would thread) —
/// the structural proof that an agent edit rides the SAME `SEND_OP` path a human does (02 §9).
pub struct KnowledgeAgentRun {
    /// The agent the run acts as (the attribution + the actor on every op).
    attribution: AgentEditAttribution,
    /// The HITL-withhold + idem-key gate (the run's decision + dedup state).
    gate: KnowledgeEffectGate,
    /// The reserve/settle bookend (11.7).
    budget: ReserveSettle,
    /// The set of tools APPROVED for this run (the HITL resume adds to it). Empty for a fresh run.
    approved: std::collections::BTreeSet<String>,
    /// The attributed collab ops the run actually APPLIED (each "suggested by agent"). The structural
    /// trail proving 0 ungoverned mutation + the attribution.
    applied_ops: Vec<DocOp>,
    /// The drill counters (the dated-receipt telemetry).
    withheld: u64,
    denied: u64,
    mutations_before_approval: u64,
    double_applies: u64,
    ungoverned_mutations: u64,
    /// The next op lamport (a deterministic per-run monotone counter for the op ids).
    next_lamport: u64,
}

impl KnowledgeAgentRun {
    /// Begin a governed agent run for `attribution`, with a reserved budget of `reserve` minor-units
    /// (the dispatch bookend, 11.7).
    pub fn begin(attribution: AgentEditAttribution, reserve: u64) -> KnowledgeAgentRun {
        KnowledgeAgentRun {
            attribution,
            gate: KnowledgeEffectGate::new(),
            budget: ReserveSettle::reserve(reserve),
            approved: std::collections::BTreeSet::new(),
            applied_ops: Vec::new(),
            withheld: 0,
            denied: 0,
            mutations_before_approval: 0,
            double_applies: 0,
            ungoverned_mutations: 0,
            next_lamport: 0,
        }
    }

    /// **Approve a tool for this run (the HITL resume, AG-P9).** Idempotent: approving an
    /// already-approved tool is a no-op. After approval a consequential effect for `tool` applies.
    pub fn approve(&mut self, tool: &str) {
        self.approved.insert(tool.to_string());
    }

    /// **Propose an agent effect through the governed path (the §5.2 apply, KN-domain half).** In
    /// order, fail-closed: BUDGET (11.7) → HITL-GATE (8.2 step 6 / AG-8) → idem-key dedup (OQ-F) →
    /// APPLY-via-collab with attribution. Returns:
    /// - `Ok(Some(op))` — the effect APPLIED: an attributed [`DocOp`] threaded through the collab
    ///   protocol (the public endpoint's `SEND_OP`), metered once against the reserve.
    /// - `Ok(None)` — the effect was a double-click NO-OP (the same `idem_key` already applied) — ONE
    ///   approval, no second mutation.
    /// - `Err(refusal)` — WITHHELD (consequential, not yet approved → 0 mutation, AG-8) or DENIED
    ///   (exhausted reserve → ordinary tool error, no privileged fallback).
    ///
    /// `card_id` keys the approval card / the per-effect `idem_key`; `effect_idx`/`total_effects`
    /// derive the per-effect key (a single-effect card keys on `card_id`; a batch on
    /// `card_id:effect_idx`, OQ-F). `cost` is the effect's metered minor-units.
    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        &mut self,
        tool: &str,
        kind: OpKind,
        payload: impl Into<Vec<u8>>,
        cost: u64,
        card_id: &str,
        effect_idx: usize,
        total_effects: usize,
    ) -> Result<Option<DocOp>, EffectRefusal> {
        // (BUDGET, 11.7) — the reserve must have remaining balance. No balance → DENIED (no
        // privileged fallback — the run cannot spend past its reserve). "no balance → no agent write".
        if !self.budget.has_remaining(cost) {
            self.denied = self.denied.saturating_add(1);
            return Err(EffectRefusal::Denied(format!(
                "reserve exhausted — no remaining balance for cost {cost} minor-units (11.7)"
            )));
        }

        // (HITL-GATE, 8.2 step 6 / AG-8) — a consequential effect not in the approved set is
        // WITHHELD: it does NOT mutate. This is the "0 mutation before approval" structural guarantee.
        if let Err(refusal) = self.gate.decide(tool, &self.approved, card_id) {
            self.withheld = self.withheld.saturating_add(1);
            return Err(refusal);
        }

        // (IDEM-KEY DEDUP, OQ-F) — a double-click re-sends the SAME per-effect key → the apply is a
        // NO-OP (one approval, one apply). "0 double-apply".
        let idem_key = per_effect_idem_key(card_id, effect_idx, total_effects);
        if self.gate.has_applied(&idem_key) {
            // The double-click: count it as the well-defined no-op (NOT a double-apply — we refuse to
            // mutate again). If this had instead threaded a second op, double_applies would increment.
            return Ok(None);
        }

        // (APPLY-via-collab with attribution, 02 §9) — the agent edit rides the SAME `SEND_OP` path a
        // human does, stamped with the "suggested by agent" actor. The op is produced HERE (the public
        // endpoint threads it through the collab protocol). Recorded once per idem_key (exactly-once).
        let applied_fresh = self.gate.apply_once(&idem_key);
        if !applied_fresh {
            // Belt-and-braces: if the key was somehow already present (it isn't, we checked above), a
            // second apply is a double-apply — count it RED. This branch never fires for a fresh key.
            self.double_applies = self.double_applies.saturating_add(1);
            return Ok(None);
        }
        let author = EditAuthor::Agent(self.attribution.clone());
        let op_id = OpId::new(self.attribution.run_id.clone(), self.next_lamport);
        self.next_lamport += 1;
        let op = author.stamp_op(op_id, kind, payload);
        self.applied_ops.push(op.clone());

        // (METER, 11.7) — settle exactly this effect's cost against the reserve (only an applied
        // effect is metered).
        self.budget.settle(cost);
        Ok(Some(op))
    }

    /// The attributed collab ops the run applied (each "suggested by agent"). The structural trail.
    pub fn applied_ops(&self) -> &[DocOp] {
        &self.applied_ops
    }

    /// The reserve/settle bookend (observability — the run's bill).
    pub fn budget(&self) -> &ReserveSettle {
        &self.budget
    }

    /// **Seal the dated KN-D11 green receipt (the gate-state + denial-counter + idem-key-dedup
    /// telemetry).** Every applied op MUST be agent-attributed (the "suggested by agent" guarantee)
    /// and MUST have passed the gate — an op in `applied_ops` whose actor is NOT `agent:`-prefixed is
    /// an ungoverned mutation (RED). The forbidden counters (`mutations_before_approval`,
    /// `double_applies`, `ungoverned_mutations`) are 0 by construction of [`propose`](Self::propose).
    pub fn seal(&self, at_ms: u64) -> KnD11Receipt {
        // Verify every applied op is agent-attributed (the structural "0 ungoverned mutation" check):
        // an applied op whose actor is not the run's agent actor reached the collab protocol without
        // the governed path (it never does — `propose` is the only apply site).
        let agent_actor = self.attribution.actor();
        let ungoverned = self
            .applied_ops
            .iter()
            .filter(|op| op.actor != agent_actor)
            .count() as u64;
        KnD11Receipt {
            withheld: self.withheld,
            denied: self.denied,
            applied: self.gate.applied_count() as u64,
            mutations_before_approval: self.mutations_before_approval,
            double_applies: self.double_applies,
            ungoverned_mutations: self.ungoverned_mutations.saturating_add(ungoverned),
            settled_minor_units: self.budget.settled(),
            at_ms,
        }
    }
}

#[cfg(test)]
mod tests;
