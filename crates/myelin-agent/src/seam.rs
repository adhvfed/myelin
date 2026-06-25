//! # `seam` — the post-M5 Fabric seam doc (AG-P25 → global P-481): the named floors, dated.
//!
//! **Status note (DATED 2026-06-25; re-date on any change — a claim that outlives its
//! verification misleads the next agent, VISION §3 / EI-01 §1).** This module is a *seam doc*:
//! it NAMES the three designed-not-built Fabric floors and the three `[OPEN -> LEGAL]` items as
//! the post-M5 follow-ons, each with its TRIGGER (what must be green to start the work) and its
//! FOLLOW-ON BAND (where the work lands). **No model / SDK / prompt / model-name string and no
//! engine code is written here** — this is the named-floor follow-on, designed-not-built. The
//! `no-llm-in-platform` ratchet (contract 1.6) stays green over this module: the live workspace
//! gate (`myelin-lints` `workspace_clean.rs`, scanning every `crates/*/src/*.rs`) and this
//! crate's [`crate::seam`]-targeted gap-report test both prove it.
//!
//! ## Why a seam doc, not a build (VISION §3; EI-03 §3; architecture §3.3)
//! During development we do **not** integrate real agents — we build the mock and use the
//! **strategy pattern** so that switching mock → real is a config/impl swap, not a rewrite. The
//! real runtime is the **only vendor seam**, swapped in **after** the safety drills are green.
//! The trigger for that swap is now PROVEN reachable: AG-P24's E2E-2 flagship (global P-480) and
//! the M5 surge/erasure drills (AG-D6 P-478, AG-D10 P-479) are green, so the safety drills the
//! swap waits on (AG-D4/D2/D3/D5) are demonstrated end-to-end. This doc records the swap; it does
//! NOT perform it. The `LlmAgentRuntime`, the external MCP endpoint, and long-term memory are the
//! deliberately-deferred follow-ons named below — naming them (not silently skipping them) is the
//! honest-floor discipline (VISION §3 "name your floors; a floor that masquerades as done is the
//! failure").
//!
//! ## The three named floors (architecture §3.3 / §6.2 / §12; roadmap §2 M5 / §3)
//!
//! 1. **`LlmAgentRuntime` — the real vendor brain (designed-not-built).**
//!    - *What is built:* the [`crate::AgentRuntime`] trait seam (contract 8.3) is FROZEN; the
//!      stateless brain boundary, the platform-owned [`crate::Conversation`] history, and the
//!      `MockAgentRuntime` behind the same seam all exist (AG-P1/AG-P5).
//!    - *What is the follow-on:* the real adapter — the **only** place a model / SDK / prompt /
//!      model-name string ever appears (enforced by the `no-llm-in-platform` lint, contract 1.6);
//!      EU-hostable, region-aware, swappable; it meters **one cost event per model call**
//!      (wholesale != markup). It is a config/impl swap behind the frozen seam, **NOT a rewrite**.
//!    - *Trigger:* the safety drills (AG-D4 / AG-D2 / AG-D3 / AG-D5) green — demonstrated by
//!      AG-P24's E2E-2 flagship (global P-480).
//!    - *Follow-on band:* **post-M5 / execution.**
//!    - *Coupled `[OPEN -> LEGAL]`:* the EU-sovereign sub-processor selection is AG-9, an open
//!      legal/commercial item (the EU-sovereign region-aware adapter ships behind a counsel-rated
//!      sub-processor; the structural seam ships regardless).
//!
//! 2. **The external MCP server endpoint (floor; architecture §6.2).**
//!    - *What is built:* the `exposed_over_mcp` column on `ToolDef` (the MCP seam, AG-P1/AG-P8)
//!      and the **internal** consumption path are built; the external surface is a projection of
//!      `ToolDef` (input_schema → MCP schema, required_caps → Id-enforced, side_effecting /
//!      requires_approval → the same plan-then-apply + HITL path) — no second governance model.
//!    - *What is the follow-on:* the **external** MCP server endpoint — its auth, its agent-lane
//!      rate-limit, its per-external-tenant budget, its threat model, and its Legal/DPO sign-off.
//!      An external MCP client is a `Principal` (no carve-out) and flows through `EffectApi`
//!      exactly like an internal agent.
//!    - *Trigger:* external-agent demand + counsel sign-off.
//!    - *Follow-on band:* **post-M5.**
//!    (MCP = Model Context Protocol, the agent-tool wire protocol; the string is named here in
//!    prose only — no SDK import, no model-name literal.)
//!
//! 3. **Agent long-term memory / RAG over prior runs (floor; architecture §12; roadmap §3).**
//!    - *What is built:* the agent-trace **holder seam** is the content-addressed trace document
//!      (AG-P19 the trace-holder, AG-P23 the DSR fan-out holder bodies); v1 agents are stateless
//!      across runs EXCEPT for this trace.
//!    - *What is the follow-on:* the embedding store — indexed via Search `semantic` (contract
//!      6.2), ACL-filtered during traversal, and **purged on `*.erased`** (the structural erasure
//!      path already exists, so cross-run recall does not open an un-erasable PII path).
//!    - *Trigger:* a measured need for cross-run recall; the holder seam already exists.
//!    - *Follow-on band:* **post-M5 (a Search / Knowledge follow-on).**
//!
//! ## The three `[OPEN -> LEGAL]` items (architecture §12; flagged to counsel/DPO)
//! The structural floor ships regardless; the residual is flagged to counsel — we are not counsel.
//!
//! - **L-3 — implicit auto-dispatch on a casual mention.** Explicit-first dispatch is v1
//!   (architecture §3.4): a mention NOTIFIES, it does not auto-spawn a costed run. Implicit
//!   auto-wake (with intent/cost detection) is a separately-decided product feature requiring DPO
//!   sign-off (GDPR Art. 22 / EU AI-Act human-oversight). *Defensible posture:* ship explicit-first
//!   only; wire NO auto-spawn path until counsel ratifies the human-oversight basis. *Follow-on
//!   owner:* Chat P6 + Commercial + Legal.
//!
//! - **L-4 — trace verbosity / reasoning-capture policy.** How much of the model's intermediate
//!   reasoning the trace captures (a privacy + AI-Act + cost trade-off) and its retention.
//!   *Defensible posture:* capture the tool-call / tool-result transcript by default (load-bearing
//!   for audit + replay); gate capture of free-form chain-of-thought behind a tenant setting tagged
//!   `#[personal_data]` under the one erasure posture (contract 10.9). *Follow-on owner:* Legal/DPO
//!   + the Knowledge trace owner.
//!
//! - **Build-data-as-training basis — FORECLOSED by default.** No platform code path feeds tenant
//!   content to model training; training-on-tenant-data is a separately-ratified opt-in, never a
//!   default. *Defensible posture:* foreclosed-by-default; the opt-in is a counsel-ratified product
//!   decision. *Follow-on owner:* Commercial + Legal.
//!
//! ## The gap-report invariant (this prompt's gate)
//! Each of the three floors and each of the three `[OPEN -> LEGAL]` items below is recorded as a
//! [`SeamFloor`] with a NON-EMPTY trigger + follow-on (and a band for the floors / an owner for
//! the legal items). The crate's `seam_floors_gap_report` test asserts **0 invisible gaps** over
//! [`NAMED_FLOORS`] and [`OPEN_LEGAL_ITEMS`], and re-runs the `no-llm-in-platform` lint over THIS
//! module's source (still green: no model/SDK/prompt fingerprint). The machine-checkable manifest
//! below is the single source of truth the gap-report cross-checks; keep it in sync with the prose.

/// The follow-on band a named floor lands in. A floor with no band is an invisible gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowOnBand {
    /// Scheduled for the post-M5 / execution slice (after the safety drills are green).
    PostM5Execution,
    /// A post-M5 follow-on (no execution-swap coupling), e.g. the external MCP endpoint.
    PostM5,
    /// A post-M5 follow-on owned by another system (Search / Knowledge), e.g. long-term memory.
    PostM5OtherSystem,
}

/// The kind of seam item: a named build-floor, or an `[OPEN -> LEGAL]` policy item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeamKind {
    /// A designed-not-built engineering floor (a trait seam exists; the body is the follow-on).
    NamedFloor,
    /// An `[OPEN -> LEGAL]` item flagged to counsel/DPO (the structural floor ships regardless).
    OpenLegal,
}

/// One seam-doc row: a named floor or a legal item with its trigger + follow-on, machine-checked.
///
/// The gap-report test asserts every field that must be non-empty IS non-empty, so no floor is
/// invisible (named-without-a-trigger or named-without-a-follow-on both count as a gap).
#[derive(Clone, Copy, Debug)]
pub struct SeamFloor {
    /// A short stable id for the floor / item (e.g. `"llm-agent-runtime"`).
    pub id: &'static str,
    /// What kind of seam item this is.
    pub kind: SeamKind,
    /// One line: what this floor IS (the designed-not-built thing).
    pub what: &'static str,
    /// What is already BUILT (the seam that makes the swap a config/impl change, not a rewrite).
    pub built: &'static str,
    /// What must be green / true to START the follow-on work. MUST be non-empty.
    pub trigger: &'static str,
    /// What the follow-on actually delivers. MUST be non-empty.
    pub follow_on: &'static str,
    /// The band / owner the follow-on lands in (band for floors, owner for legal items).
    pub band_or_owner: &'static str,
}

/// The THREE designed-not-built engineering floors (architecture §3.3 / §6.2 / §12).
pub const NAMED_FLOORS: &[SeamFloor] = &[
    SeamFloor {
        id: "llm-agent-runtime",
        kind: SeamKind::NamedFloor,
        what: "the real vendor brain behind the frozen AgentRuntime seam (8.3) — the only vendor \
               seam; EU-hostable, region-aware, swappable; one cost event per model call \
               (wholesale != markup)",
        built: "the AgentRuntime trait seam (8.3) is frozen; the stateless brain boundary + \
                platform-owned Conversation history + the MockAgentRuntime behind the same seam \
                exist (AG-P1/AG-P5)",
        trigger: "the safety drills (AG-D4/AG-D2/AG-D3/AG-D5) green — demonstrated by AG-P24's \
                  E2E-2 flagship (global P-480)",
        follow_on: "the real adapter — a config/impl swap behind the frozen seam, NOT a rewrite; \
                    the only place a model/SDK/prompt/model-name string ever appears (no-llm lint \
                    1.6); EU-sovereign sub-processor is [OPEN -> LEGAL] AG-9",
        band_or_owner: "post-M5 / execution",
    },
    SeamFloor {
        id: "external-mcp-endpoint",
        kind: SeamKind::NamedFloor,
        what: "the external MCP server endpoint exposing the exposed_over_mcp subset of ToolDef \
               as a projection (no second governance model)",
        built: "the exposed_over_mcp column on ToolDef + the internal consumption path \
                (AG-P1/AG-P8); an external MCP client is a Principal flowing through EffectApi \
                like an internal agent",
        trigger: "external-agent demand + counsel sign-off",
        follow_on: "the external endpoint: its auth, agent-lane rate-limit, per-external-tenant \
                    budget, threat model, and Legal/DPO sign-off",
        band_or_owner: "post-M5",
    },
    SeamFloor {
        id: "long-term-memory-rag",
        kind: SeamKind::NamedFloor,
        what: "agent long-term memory / RAG over prior runs (cross-run recall beyond the trace)",
        built: "the agent-trace holder seam — the content-addressed trace document (AG-P19 \
                holder, AG-P23 DSR fan-out bodies); v1 agents are stateless across runs except \
                this trace",
        trigger: "a measured need for cross-run recall; the holder seam already exists",
        follow_on: "the embedding store — indexed via Search semantic (6.2), ACL-filtered during \
                    traversal, purged on *.erased (the structural erasure path already exists)",
        band_or_owner: "post-M5 (a Search / Knowledge follow-on)",
    },
];

/// The THREE `[OPEN -> LEGAL]` items (architecture §12; the structural floor ships regardless).
pub const OPEN_LEGAL_ITEMS: &[SeamFloor] = &[
    SeamFloor {
        id: "l3-implicit-auto-dispatch",
        kind: SeamKind::OpenLegal,
        what: "implicit auto-dispatch on a casual mention (L-3) — auto-waking a costed run from a \
               mention instead of explicit-first",
        built: "explicit-first dispatch is v1 (architecture §3.4): a mention NOTIFIES, it does \
                not auto-spawn a costed run; no auto-spawn path is wired",
        trigger: "counsel ratifies the human-oversight basis (GDPR Art. 22 / EU AI-Act)",
        follow_on: "implicit auto-wake with intent/cost detection — a separately-decided product \
                    feature requiring DPO sign-off; ship explicit-first only until then",
        band_or_owner: "Chat P6 + Commercial + Legal",
    },
    SeamFloor {
        id: "l4-reasoning-capture",
        kind: SeamKind::OpenLegal,
        what: "trace verbosity / reasoning-capture policy (L-4) — how much intermediate model \
               reasoning the trace captures and its retention",
        built: "the tool-call / tool-result transcript is captured by default (load-bearing for \
                audit + replay); free-form chain-of-thought capture is gated",
        trigger:
            "counsel rates the privacy + AI-Act classification + retention of chain-of-thought",
        follow_on: "gate chain-of-thought capture behind a tenant setting tagged #[personal_data] \
                    under the one erasure posture (contract 10.9); flag retention to counsel",
        band_or_owner: "Legal/DPO + the Knowledge trace owner",
    },
    SeamFloor {
        id: "build-data-as-training",
        kind: SeamKind::OpenLegal,
        what: "build-data-as-training basis — feeding tenant content to model training",
        built: "FORECLOSED by default — no platform code path feeds tenant content to training",
        trigger: "a counsel-ratified opt-in product decision (never a default)",
        follow_on:
            "training-on-tenant-data as a separately-ratified opt-in; foreclosed-by-default \
                    until then",
        band_or_owner: "Commercial + Legal",
    },
];

impl SeamFloor {
    /// True iff this row is fully recorded — no invisible gap. A floor named without a trigger or
    /// without a follow-on (or without a band/owner) is an invisible gap and fails the report.
    pub fn is_fully_recorded(&self) -> bool {
        !self.id.is_empty()
            && !self.what.is_empty()
            && !self.built.is_empty()
            && !self.trigger.is_empty()
            && !self.follow_on.is_empty()
            && !self.band_or_owner.is_empty()
    }
}

/// Every seam item the gap-report must account for: the three floors + the three legal items.
/// The gap-report test asserts `0` of these is invisible (each fully recorded), per the prompt's
/// "none invisible" gate.
pub fn all_seam_items() -> Vec<SeamFloor> {
    NAMED_FLOORS
        .iter()
        .chain(OPEN_LEGAL_ITEMS.iter())
        .copied()
        .collect()
}
