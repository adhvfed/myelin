//! # `tools` — the Chat agent **ToolDef set** (the frozen X-6 defaults) routed through
//! [`EffectApi`](myelin_agent::EffectApi) + reserve/settle + `run --dry-run` (the routing-split
//! safety boundary) (CHAT-P19 → P-414, M4-C6)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §8 (the
//! Chat `ToolDef` set + the **frozen `requires_approval` defaults** — `post`/`reply_in_thread`/
//! `react`/`start_dm` = NOT gated (reversible, cheap); `create_channel`/`invite`/`archive_channel` =
//! gated (they change visibility / are destructive lifecycle); **a cross-subsystem effect inherits the
//! TARGET subsystem's default** — "governed where it LANDS, not where it's invoked"; all side-effecting
//! tools route through [`EffectApi`](myelin_agent::EffectApi), NEVER `ToolHands::exec` — **the routing
//! split is the safety boundary**; the four uniform guarantees), §9 (reserve/settle — Chat surfaces
//! cost but **never holds the wallet**).
//!
//! **Reconciliation §X-6** (`00-reconciliation-decisions.md`): the frozen `requires_approval` defaults
//! (Chat post/react = no; cross-subsystem inherits the target's default); the four uniform guarantees;
//! **the `EffectApi`-vs-`ToolHands` routing split**. **VISION §3** (agent-native; suggest-by-default —
//! reversible/cheap actions (post/react/reply) are NOT gated; consequential actions (membership /
//! lifecycle) are). **EI-01 §3** (prove-it — a chat mutation that bypasses `EffectApi` via `ToolHands`
//! is the failure a structural check forbids), **§8** (cost is decision-shaped — reserve fronts every
//! spend-bearing post).
//!
//! **Contract-index rows:**
//! - **8.1** `ToolSurface::register_tool(ToolDef)` — OWNED (the Chat slice): the frozen Chat ToolDef
//!   set + the frozen §8 `requires_approval` defaults.
//! - **8.2** `EffectApi::apply` (plan-then-apply) — CONSUMED: every chat MUTATION routes through it
//!   (the routing split). Chat does NOT re-implement the eight-step pipeline (the fabric owns it).
//! - **8.7** `run --dry-run(InboxEvent) → Vec<ProposedEffect>` — OWNED (on chat tools): returns the
//!   proposed effects WITHOUT applying any.
//! - **11.7** reserve/settle — CONSUMED: reserve fronts every spend-bearing post (no balance → no
//!   post). Chat surfaces the cost; Commercial owns the wallet ([`myelin_storage::reserve_settle`]).
//!
//! ## What this prompt (CHAT-P19) ships — the tool DEFINITIONS + the routing, NOT a new engine
//! - [`chat_tool_defs`] — the FULL frozen X-6 Chat ToolDef set (the seven owned chat actions), each
//!   built over the frozen [`ToolDef`](myelin_agent::ToolDef) shape with the frozen §8
//!   `requires_approval` default ([`requires_approval_default`]) and `required_caps` from the frozen
//!   Chat ReBAC fragment ([`crate::rebac_fragment`], 4.9). EVERY def is `effect_kind = Mutate` +
//!   `side_effecting` — so it routes through [`EffectApi`](myelin_agent::EffectApi), NEVER
//!   `ToolHands::exec` ([`assert_routes_through_effect_api`], the routing-split structural check).
//! - [`register_chat_tools`] — registers the set into the ONE [`ToolSurface`](myelin_agent::ToolSurface),
//!   passing every def through the routing-split check + the no-silent-loosening guard FIRST.
//! - [`requires_approval_for_landing`] — the cross-subsystem "governed where it LANDS" rule (§8 last
//!   row): a chat-invoked `EffectApi` tool that mutates ANOTHER subsystem inherits THAT subsystem's
//!   frozen default (a chat-invoked `git.merge` is GATED — Git's default — NOT un-gated by Chat's
//!   `post`).
//! - [`reserve_spend_bearing_post`] / [`settle_spend_bearing_post`] — the reserve/settle bookend
//!   (11.7) that fronts a spend-bearing agent post: reserve at dispatch (no balance → no post), settle
//!   on completion. Chat surfaces the cost; it does NOT own the wallet (the balance is the Commercial
//!   wallet's, passed in).
//! - [`dry_run_chat_tools`] — the `run --dry-run` lever (8.7) over the chat tool set: returns the
//!   [`ProposedEffect`](myelin_agent::ProposedEffect)s the tools WOULD propose, applying NOTHING
//!   (0 mutations, 0 reserve consumed).
//!
//! ## The routing split is the safety boundary (X-6 / EI-01 §3)
//! There are exactly two execution seams in the fabric: [`EffectApi::apply`](myelin_agent::EffectApi)
//! (governed MUTATION — plan-then-apply, reserves, HITL) and `ToolHands::exec` (sandboxed COMPUTE —
//! untrusted code). A chat mutation (post/react/invite/…) is a MUTATION → it MUST route through
//! `EffectApi`, NEVER `ToolHands::exec`. This is structural here: every Chat ToolDef is
//! `effect_kind = Mutate` (the loop routes `Mutate` to `EffectApi`, §5.0) and NONE is `Compute`/`External`
//! (which would route to the sandbox). [`assert_routes_through_effect_api`] is the structural check the
//! GATE asserts (0 chat tools route through `ToolHands`); the `no-host-exec` lint forbids any host-exec
//! bypass platform-wide.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1) — none new
//! Chat owns the tool **DEFINITIONS** + the **routing**; it does NOT re-implement a sandbox or a
//! budget. The four uniform sandbox guarantees + the plan-then-apply ENGINE are the M2 Agent
//! primitives ([`myelin_agent::EffectApi`] / the fabric `PlanThenApply`); the reserve/settle LEDGER is
//! the M1 Storage primitive ([`myelin_storage::reserve_settle::CostLedger`]). Chat MUST NOT
//! re-implement a sandbox or a budget — it CONSUMES both. The external MCP endpoint stays a post-M5
//! floor (`exposed_over_mcp = false`).

use myelin_agent::{
    EffectApi, EffectKind, EffectResult, ProposedEffect, RunCtx, ToolDef, ToolName, ToolSurface,
};
use myelin_storage::reserve_settle::{
    CostLedger, MicroUsd, Reservation, ReserveError, RunId, SettleError, SettleOutcome,
};
use myelin_tenancy::TenantId;

use crate::rebac_fragment::object_types as chat_objects;

// ───────────────────────── the Chat subsystem token + the frozen tool identity (§8) ──────────────

/// **The Chat subsystem token** — the `subsystem` half of the catalogue key + the key the frozen §8
/// defaults table is looked up under (`("chat", "post")` → not gated). Also the INVOKING subsystem
/// the cross-subsystem rule ([`requires_approval_for_landing`]) pins. The SINGLE source of truth.
pub const CHAT_SUBSYSTEM: &str = "chat";

/// **The ToolDef version** the Chat tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const CHAT_TOOL_VERSION: u32 = 1;

/// `chat.post` — post a message (the agent's chat output path). Reversible (edit/delete) → NOT gated.
pub const POST_TOOL: &str = "post";
/// `chat.reply_in_thread` — reply to a thread. Reversible → NOT gated.
pub const REPLY_IN_THREAD_TOOL: &str = "reply_in_thread";
/// `chat.react` — add a reaction. Reversible (un-react) → NOT gated.
pub const REACT_TOOL: &str = "react";
/// `chat.start_dm` — open a DM. Reversible → NOT gated.
pub const START_DM_TOOL: &str = "start_dm";
/// `chat.create_channel` — creating a channel is governance-shaped (changes visibility) → GATED.
pub const CREATE_CHANNEL_TOOL: &str = "create_channel";
/// `chat.invite` — adding a member changes who-can-see (sensitive) → GATED.
pub const INVITE_TOOL: &str = "invite";
/// `chat.archive_channel` — destructive lifecycle → GATED.
pub const ARCHIVE_CHANNEL_TOOL: &str = "archive_channel";

/// **The complete set of Chat tool names, in frozen catalogue order.** The single list every
/// registration + CDC + dry-run consumes (one source of truth). The first four are reversible (NOT
/// gated); the last three are consequential (GATED) — the frozen §8 split.
pub const CHAT_TOOL_NAMES: &[&str] = &[
    POST_TOOL,
    REPLY_IN_THREAD_TOOL,
    REACT_TOOL,
    START_DM_TOOL,
    CREATE_CHANNEL_TOOL,
    INVITE_TOOL,
    ARCHIVE_CHANNEL_TOOL,
];

// ───────────────────────── the frozen §8 requires_approval default (the seed source of truth) ────

/// **The FROZEN §8 / X-6 `requires_approval` default for a Chat `tool`.** The match arms ARE the §8
/// Chat ToolDef table, verbatim:
/// - NOT gated (`false`): `post` / `reply_in_thread` / `react` / `start_dm` — reversible, cheap
///   (suggest-by-default, VISION §3; a post/react is recovered by editing/deleting/un-reacting).
/// - GATED (`true`): `create_channel` / `invite` / `archive_channel` — they change who-can-see
///   (visibility) or are destructive lifecycle; consequential → human-confirm (VISION §3 / GDPR
///   Art. 22).
///
/// A Chat tool NOT named in the frozen table defaults to **gated (`true`)** — fail-closed: an
/// unrecognised chat action is gated until the table is extended HERE (a new gated/un-gated action is
/// a frozen-table edit, never a runtime invention). This is the conservative floor.
///
/// **The cross-subsystem rule is NOT here** — a chat-invoked effect that mutates another subsystem
/// inherits THAT subsystem's default ([`requires_approval_for_landing`]). This function answers "what
/// is the frozen default for a Chat tool that LANDS in chat?".
pub fn requires_approval_default(tool: &str) -> bool {
    match tool {
        // reversible, cheap → NOT gated (suggest-by-default).
        POST_TOOL | REPLY_IN_THREAD_TOOL | REACT_TOOL | START_DM_TOOL => false,
        // consequential (visibility change / destructive lifecycle) → GATED (human-confirm).
        CREATE_CHANNEL_TOOL | INVITE_TOOL | ARCHIVE_CHANNEL_TOOL => true,
        // fail-closed: an unrecognised Chat action is gated until the frozen table is extended.
        _ => true,
    }
}

/// **The cross-subsystem rule (§8 last row) — "governed where it LANDS, not where it's invoked".** A
/// Chat-invoked [`EffectApi`](myelin_agent::EffectApi) tool that mutates ANOTHER subsystem inherits
/// the TARGET subsystem's default. `landing_subsystem` is where the MUTATION lands (e.g. `git`); the
/// default is the LANDING subsystem's. A chat-invoked `git.merge` is gated (Git's default), NOT
/// un-gated (Chat's `post` default).
///
/// When the landing subsystem is `chat`, this collapses to [`requires_approval_default`] for that
/// tool. For any OTHER landing subsystem the fabric's frozen per-subsystem table owns the default
/// (the fabric `defaults::requires_approval_for_landing` is the cross-subsystem source of truth); a
/// non-chat landing is **fail-closed gated** here (`true`) — the chat crate does not vendor the whole
/// frozen table (that would be a second source of truth, EI-01 §7); it gates by default and the
/// fabric's `EffectApi` resolves the actual landing default at apply-time. Chat NEVER un-gates a
/// cross-subsystem effect.
pub fn requires_approval_for_landing(landing_subsystem: &str, tool: &str) -> bool {
    if landing_subsystem == CHAT_SUBSYSTEM {
        requires_approval_default(tool)
    } else {
        // A chat-invoked cross-subsystem effect is governed where it LANDS; chat does not vendor the
        // landing subsystem's table (the fabric owns it). Fail-closed gated: chat never UN-gates a
        // cross-subsystem effect (a real landing default ≤ this conservative floor, never above it).
        true
    }
}

// ───────────────────────── the routing-split structural check (the safety boundary, X-6) ─────────

/// **A Chat ToolDef that VIOLATES the routing split (a side-effecting chat tool NOT routed through
/// [`EffectApi`](myelin_agent::EffectApi)).** Surfaced LOUD — the registration is REJECTED. A
/// side-effecting chat tool MUST be `effect_kind = Mutate` (the loop routes `Mutate` through
/// `EffectApi`, §5.0); a `Compute`/`External` def would route to `ToolHands::exec` (the sandbox),
/// which carries untrusted compute, NOT privileged chat mutation — that is the failure this forbids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationSeamViolation {
    /// The tool whose registration would route a chat MUTATION through `ToolHands::exec`.
    pub tool: String,
    /// The (wrong) effect kind the def carried.
    pub effect_kind: EffectKind,
}

impl core::fmt::Display for MutationSeamViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "chat tool {} is side-effecting but effect_kind={:?} (would route through ToolHands::exec) \
             — a chat MUTATION MUST route through EffectApi (effect_kind=Mutate); the routing split is \
             the safety boundary (X-6)",
            self.tool, self.effect_kind
        )
    }
}

impl std::error::Error for MutationSeamViolation {}

/// **The routing-split structural check (X-6 / EI-01 §3) — the safety boundary.** Asserts a Chat
/// [`ToolDef`](myelin_agent::ToolDef) routes a MUTATION through [`EffectApi`](myelin_agent::EffectApi),
/// NEVER `ToolHands::exec`:
/// - a side-effecting chat tool MUST be `effect_kind = Mutate` (routes to `EffectApi`, §5.0);
/// - a side-effecting chat tool that is `Compute`/`External` (routes to `ToolHands::exec`, the
///   sandbox) is **REJECTED** (loud) — that seam carries untrusted compute, not privileged mutation.
///
/// Returns `Ok(())` when the def routes through `EffectApi`, `Err(MutationSeamViolation)` otherwise.
/// This is the structural check the GATE asserts (0 chat mutations via `ToolHands`).
pub fn assert_routes_through_effect_api(def: &ToolDef) -> Result<(), MutationSeamViolation> {
    // A read-only chat tool (none ship at v1) needs no governance; only a side-effecting tool is
    // constrained to the EffectApi seam.
    if def.side_effecting && def.effect_kind != EffectKind::Mutate {
        return Err(MutationSeamViolation {
            tool: def.name.0.clone(),
            effect_kind: def.effect_kind,
        });
    }
    Ok(())
}

// ───────────────────────── the required_caps from the Chat ReBAC fragment (4.9) ──────────────────

/// **The `required_caps` for a Chat tool (CONSUMED from the frozen Chat ReBAC fragment, 4.9).** Built
/// from the canonical [`crate::rebac_fragment`] object-type constants so a rename in the fragment is a
/// compile/test break here, never a silent drift. Posting/reacting/replying/DM is governed by the
/// `channel.post` permission (an agent may only post where it is a member, exactly as a human);
/// creating/inviting/archiving is governed by the `channel.manage` permission (the frozen §5 fragment
/// permission that gates "invite / archive / settings" — `manage = member & parent_project->admin`),
/// because a membership/lifecycle mutation changes who-can-see.
fn required_caps(tool: &str) -> Vec<String> {
    match tool {
        POST_TOOL | REPLY_IN_THREAD_TOOL | REACT_TOOL | START_DM_TOOL => {
            vec![format!("{}.post", chat_objects::CHANNEL)]
        }
        CREATE_CHANNEL_TOOL | INVITE_TOOL | ARCHIVE_CHANNEL_TOOL => {
            vec![format!("{}.manage", chat_objects::CHANNEL)]
        }
        // fail-closed: an unrecognised chat tool requires the most-privileged chat cap.
        _ => vec![format!("{}.manage", chat_objects::CHANNEL)],
    }
}

/// The frozen input-schema (JSON Schema, opaque-string carrier at this seam) for a Chat tool.
fn input_schema(tool: &str) -> &'static str {
    match tool {
        POST_TOOL => {
            r#"{"type":"object","required":["channel","body"],"properties":{"channel":{"type":"string"},"body":{"type":"string"}}}"#
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

// ───────────────────────── the Chat ToolDef set (8.1 — the OWNED registration) ────────────────────

/// Build one frozen Chat [`ToolDef`](myelin_agent::ToolDef): a `Mutate`, side-effecting tool (routes
/// through [`EffectApi`](myelin_agent::EffectApi), the routing split) with the frozen §8
/// `requires_approval` default + the 4.9 `required_caps`. `effect_kind = Mutate` is the structural
/// guarantee the routing split rests on (NEVER `Compute`/`External`, which would route to the sandbox).
pub fn chat_tool_def(tool: &str) -> ToolDef {
    ToolDef {
        name: ToolName(tool.to_string()),
        subsystem: CHAT_SUBSYSTEM.to_string(),
        version: CHAT_TOOL_VERSION,
        input_schema: input_schema(tool).to_string(),
        required_caps: required_caps(tool),
        // the routing split: a chat mutation routes through EffectApi → Mutate (never Compute/External).
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED from the frozen §8 default (the column is not hand-set per-call site).
        requires_approval: requires_approval_default(tool),
        // the external MCP endpoint is a post-M5 floor.
        exposed_over_mcp: false,
    }
}

/// **The complete frozen Chat ToolDef set, in catalogue order (8.1 — the OWNED deliverable).** The
/// single list every registration + CDC + dry-run consumes. Each def is `effect_kind = Mutate`
/// (routes through [`EffectApi`](myelin_agent::EffectApi)) with the frozen §8 `requires_approval`
/// default (post/reply/react/start_dm = no; create_channel/invite/archive = yes).
pub fn chat_tool_defs() -> Vec<ToolDef> {
    CHAT_TOOL_NAMES.iter().map(|t| chat_tool_def(t)).collect()
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **A registration that LOOSENS a frozen §8 `yes → no` (un-gates a consequential chat action) WITHOUT
/// authorisation.** Surfaced LOUD — the registration is REJECTED. A subsystem may TIGHTEN a default
/// (mark more tools gated) but may NOT silently un-gate a consequential action (VISION §3 / GDPR
/// Art. 22). Carries the tool whose frozen `yes` the registration tried to silently flip to `no`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseningViolation {
    /// The tool whose frozen `yes` the registration silently flipped to `no`.
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

/// **An error registering the Chat tools — either a routing-split violation or a silent loosening.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A side-effecting chat tool would route through `ToolHands::exec` (not `EffectApi`).
    RoutingSplit(MutationSeamViolation),
    /// A registration silently un-gated a frozen-consequential chat action.
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

/// Assert a Chat def does not silently LOOSEN the frozen §8 default (a frozen `yes` registered as
/// `no`). Tightening (`no → yes`) is always allowed; matching the frozen default is allowed.
fn assert_no_silent_loosening(def: &ToolDef) -> Result<(), LooseningViolation> {
    let frozen = requires_approval_default(&def.name.0);
    if frozen && !def.requires_approval {
        return Err(LooseningViolation {
            tool: def.name.0.clone(),
        });
    }
    Ok(())
}

/// **Register the Chat ToolDef set into the ONE [`ToolSurface`](myelin_agent::ToolSurface) (8.1 / §8)
/// — the OWNED deliverable.** Every def is passed through BOTH structural ratchets FIRST:
/// 1. the **routing split** ([`assert_routes_through_effect_api`]) — a side-effecting chat tool MUST
///    route through [`EffectApi`](myelin_agent::EffectApi) (`Mutate`), never `ToolHands::exec`;
/// 2. the **no-silent-loosening guard** ([`assert_no_silent_loosening`]) — a frozen-consequential
///    chat action may not be silently un-gated.
///
/// Returns the registered defs, or the FIRST violation (loud). On success, every chat tool is in the
/// ONE catalogue with its frozen §8 shape; a side-effecting chat tool that bypasses `EffectApi` is
/// structurally impossible to register.
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

// ───────────────────────── reserve/settle on a spend-bearing post (11.7 — CONSUMED) ──────────────

/// **The cost ESTIMATE chat surfaces for a spend-bearing post (the HITL card's live estimate, §9).**
/// Chat surfaces the cost; it NEVER holds the wallet — this is the upper-bound estimate the reserve
/// fronts, expressed in integer minor-units (the frozen cost unit; a fractional cost is
/// unrepresentable). The actual metered cost is settled on completion (≤ this reserve).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostCostEstimate(pub MicroUsd);

/// **Reserve-at-dispatch for a spend-bearing agent post (11.7 — CONSUMED).** Fronts the post's cost
/// estimate against the Commercial wallet `available` balance via the M1 Storage ledger: **no balance
/// → no post** (an exhausted wallet REFUSES the dispatch, nothing is written). Chat does NOT own the
/// wallet (the `available` balance is passed in from Commercial); chat does NOT own the ledger (it is
/// [`myelin_storage::reserve_settle::CostLedger`]) — chat CONSUMES the reserve gate. Returns the
/// [`Reservation`](myelin_storage::reserve_settle::Reservation) on success, or
/// [`ReserveError`](myelin_storage::reserve_settle::ReserveError) (loud) on an exhausted/duplicate
/// reservation.
pub fn reserve_spend_bearing_post(
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: RunId,
    estimate: PostCostEstimate,
    available: MicroUsd,
) -> Result<Reservation, ReserveError> {
    // The reserve gate is the M1 Storage primitive — chat fronts the estimate, the ledger enforces
    // no-balance-no-run. Chat surfaces the cost; Commercial owns the wallet (the `available` balance).
    ledger.reserve(tenant, run, estimate.0, available)
}

/// **Settle-on-completion for a spend-bearing agent post (11.7 — CONSUMED).** Closes the reservation
/// with the actual metered units once the post completes (never interrupting in-flight). The billed
/// total is capped at the reserved estimate (the reserve is the upper bound); the over-reservation is
/// released. Idempotent on `(tenant, run)` — a double-settle never double-charges. Chat CONSUMES the
/// settle path; it does not own the ledger.
pub fn settle_spend_bearing_post(
    ledger: &mut CostLedger,
    tenant: &TenantId,
    run: &RunId,
    units: &[myelin_storage::reserve_settle::MeteredUnit],
) -> Result<SettleOutcome, SettleError> {
    ledger.settle(tenant, run, units)
}

// ───────────────────────── run --dry-run on the chat tools (8.7 — OWNED) ──────────────────────────

/// **One entry in a chat `run --dry-run` plan: the proposed effect + the gate verdict it WOULD get
/// (8.7).** The verdict is the frozen §8 default — `would_gate` (the tool's `requires_approval`)
/// vs would-apply — WITHOUT any mutation or reserve. A test asserts the plan + the per-effect
/// verdicts; an E2E asserts the wallet balance is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunEntry {
    /// The chat tool the effect proposes.
    pub tool: String,
    /// The opaque [`ProposedEffect`](myelin_agent::ProposedEffect) carrier (references-not-payload).
    pub effect: ProposedEffect,
    /// Whether the effect WOULD gate (the frozen §8 `requires_approval`) — no mutation, no reserve.
    pub would_gate: bool,
}

/// Encode a chat tool invocation as an opaque [`ProposedEffect`](myelin_agent::ProposedEffect) carrier
/// (references-not-payload — the seam carries the chat tool name; the fabric's `EffectApi` resolves
/// the structured plan at apply-time). The dry-run NEVER applies it.
fn proposed_effect_for(tool: &str) -> ProposedEffect {
    ProposedEffect(format!("{CHAT_SUBSYSTEM}.{tool}"))
}

/// **`run --dry-run` over the chat tools (8.7 — the OWNED lever).** Given the chat tools a run WOULD
/// invoke (by name), returns the [`ProposedEffect`](myelin_agent::ProposedEffect)s they propose —
/// **applying NOTHING** (0 mutations, 0 reserve consumed). This is the plan-then-apply testability
/// lever: a dry-run shows WHAT a run would do without doing it. Each effect carries its frozen §8
/// would-gate verdict (the card a real run would render). An unknown tool is dropped (it is not a
/// chat effect).
///
/// **Side-effect-free by construction:** this function holds no ledger, no `EffectApi`, no apply
/// endpoint — it CANNOT mutate or reserve. The plan it returns IS the plan a live run would route
/// through `EffectApi`; the dry-run simply does not route it.
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

/// **A frozen-shape [`DryRun`](myelin_agent::DryRun) bridge over the chat tool set (8.7).** Bridges
/// the frozen glue `dry_run(InboxEvent) -> Vec<ProposedEffect>` signature to [`dry_run_chat_tools`] by
/// holding the chat tools the delivered event resolves to. The CLI `run --dry-run` entry builds this
/// from the delivered event and calls the frozen `dry_run`. Returns the proposed-effect plan,
/// **0 applies + 0 reserve consumed** (the wallet is unchanged after a dry-run).
pub struct ChatDryRun {
    /// The chat tools the `InboxEvent` resolves to (the plan the dry-run replays).
    invoked: Vec<String>,
}

impl ChatDryRun {
    /// Build a chat dry-run bridge over the chat tools a delivered event resolves to.
    pub fn new(invoked: Vec<String>) -> ChatDryRun {
        ChatDryRun { invoked }
    }
}

impl myelin_agent::DryRun for ChatDryRun {
    /// **8.7 — plan the chat run for a delivered event WITHOUT applying any effect.** Returns the
    /// proposed-effect plan the held chat tools would propose; side-effect-free (0 apply, 0 reserve).
    fn dry_run(&self, _inbox: myelin_agent::InboxEvent) -> Vec<ProposedEffect> {
        let names: Vec<&str> = self.invoked.iter().map(|s| s.as_str()).collect();
        dry_run_chat_tools(&names)
            .into_iter()
            .map(|e| e.effect)
            .collect()
    }
}

// ───────────────────────── the EffectApi routing entry (8.2 — CONSUMED) ───────────────────────────

/// **Route a chat tool's proposed effect through the fabric's [`EffectApi`](myelin_agent::EffectApi)
/// (8.2 — CONSUMED; the ONLY chat-mutation path).** Chat NEVER mutates directly and NEVER routes a
/// mutation through `ToolHands::exec` — every chat side-effect goes through `EffectApi::apply`
/// (plan-then-apply: schema → capability → delegation → tenant → budget → HITL → apply → meter). A
/// gated tool is WITHHELD ([`EffectResult::Gated`](myelin_agent::EffectResult)); it does NOT mutate
/// (AG-8). This is the chat-side entry that proves the routing split: the only way a chat mutation
/// reaches the world is through this `EffectApi` call.
///
/// `effect_api` is the fabric's plan-then-apply engine (chat does NOT implement it). Returns the
/// [`EffectResult`](myelin_agent::EffectResult) (Applied / Gated / Denied) the fabric produced.
pub fn route_chat_effect_through_effect_api<E: EffectApi>(
    effect_api: &E,
    run: &RunCtx,
    tool: &str,
) -> EffectResult {
    effect_api.apply(run, proposed_effect_for(tool))
}

#[cfg(test)]
mod tests;
