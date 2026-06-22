//! # `loop_guards` — the FIVE structural loop guards, re-enforced at the Fabric tier (AG-P12 → P-224, AG-D7)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §5.5 (the structural loop guards — *loop prevention is structural, not a convention*; a human or
//! agent **can never typo their way into a loop**, EI-02 §6). Carried forward from Phase-3 §5.5,
//! unchanged. The guards live PRIMARILY in the Bus reactive/dispatch tier (contract 3.6) and the
//! Fabric **re-enforces at apply time — defence in depth**. This module is the Fabric tier of that
//! defence-in-depth: it owns the two guards the Fabric is the natural home of (the **self-guard** and
//! the **reference gate**) and the apply-time **idempotent-tools** re-enforcement, and it RE-USES the
//! engine's already-built three (the causal-depth ceiling, the shared-root tripwire, the bounded
//! activity pool — `myelin_flow::CausalGuard`, P-FLOW-18/P-214) rather than re-implementing them.
//!
//! ## The five structural guards (§5.5) — and where each is owned
//!
//! 1. **Self-guard** — drop an inbound event whose `actor.principal == this agent`. *An agent's own
//!    emission can never re-trigger the agent.* OWNED HERE ([`SelfGuard`]).
//! 2. **Reference gate** — ONLY a structured `artifact_ref` node ([`myelin_content::InlineNode::
//!    ArtifactRefNode`], the frozen 13.1 inline ref node) may re-trigger a run; **raw typed text NEVER
//!    does** (0 raw-text re-triggers). OWNED HERE ([`ReferenceGate`]).
//! 3. **Causal-depth ceiling** — drop/park when `depth > ceiling` (default [`AGENT_CEILING`] = 12).
//!    RE-USED from [`myelin_flow::CausalGuard::admit_child`] — the Fabric re-enforces it at the
//!    DEFAULT-12 agent ceiling (the engine's own in-process ceiling is wider; the agent lane's is the
//!    tighter 12 named in AG-D7). Defence in depth: the Bus already gates, the Fabric gates again.
//! 4. **Shared-root tripwire** — a per-tenant circuit breaker on `> K` events for one `correlation_id`
//!    in a window. RE-USED from [`myelin_flow::CausalGuard::admit_child`] (the tripwire leg).
//! 5. **Idempotent tools** — keyed on `(run, effect_id)`; a re-applied effect under the same key is a
//!    NO-OP (never a second mutation). OWNED HERE — the apply-time re-enforcement
//!    ([`IdempotentToolLedger`]).
//!
//! The **bounded dispatch pool** (drops over-cap, never forks unboundedly) is owned by the Bus (3.6);
//! the Fabric ASSERTS it does not fork unboundedly — see [`AgentLoopGuards::admit_dispatch`], which
//! defers to [`myelin_flow::CausalGuard::admit_activity`] (the same `LoopVerdict::Park`-over-cap shape,
//! never a `Fork`).
//!
//! ## The ONE invariant: drop/park, NEVER fork (§5.5, AG-D7)
//!
//! Every guard's refusal is a [`GuardVerdict::Drop`] or [`GuardVerdict::Park`] — there is **no `Fork`
//! variant** (the 0-unbounded-fork invariant is enforced by the TYPE, exactly as
//! [`myelin_flow::LoopVerdict`] is). The AG-D7 green artifact is: a deliberate agent→agent self-trigger
//! loop **halts `<=` the ceiling (12)**, the shared-root tripwire trips the per-tenant breaker, the
//! bounded pool drops over-cap, and there are **0 raw-text re-triggers** + **0 unbounded forks**.
//!
//! ## Floors named (cross-references; VISION §3)
//! - **NO floor for loop prevention** — it is structural and complete here (the ceiling DEFAULT of 12
//!   is tunable, but the mechanism is not a floor).
//! - **The agent-lane shed budget (the in-flight cap)** is a SEPARATE floor tuned in **AG-P22**; the
//!   bounded-pool cap here is the structural mechanism, the *number* is what AG-P22 measures.
//! - **Per-run identity (mint/scrub/revoke/re-mint)** is **AG-P13 (→ P-225)** — the self-guard reads
//!   `actor.principal`; the full per-run token lifecycle that BINDS that principal to one run is P-225.

use myelin_content::InlineNode;
use myelin_events::{Actor, EventEnvelope};
use myelin_flow::{CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason};
use myelin_identity::PrincipalId;
use std::collections::BTreeSet;

/// **The AGENT-LANE causal-depth ceiling (§5.5, AG-D7) — default 12.** The agent fabric re-enforces a
/// TIGHTER ceiling than the engine's own in-process [`myelin_flow::CEILING`]: an agent→agent
/// self-trigger chain is halted at or below 12 hops. Chosen so a legitimate deep agent automation (a
/// run that schedules a CI pipeline that fans a few jobs) is well within bound, while a runaway
/// self-feeding loop hits it fast. **The ceiling is NEVER raised to make a loop "pass"** — a red AG-D7
/// is a dated scorecard row, not a tuning knob (per the prompt's DEFINITION OF DONE).
pub const AGENT_CEILING: u32 = 12;

/// **The agent-lane shared-root window cap (§5.5).** The maximum number of agent dispatches that may
/// share one `correlation_id` root within the tripwire's sliding window before the **per-tenant circuit
/// breaker** trips. A wide-but-shallow agent→event→agent loop re-enters the same root each hop; past
/// this many same-root dispatches the breaker fires. A legitimate fan (one trigger → a handful of agent
/// runs under one root) is well under cap.
pub const AGENT_SHARED_ROOT_CAP: u32 = 64;

/// **The agent-lane bounded dispatch-pool cap (§5.5 / X-3).** The maximum number of CONCURRENT agent
/// dispatches the pool admits; a would-be dispatch over the cap is SHED/PARKED, never forked (a mention
/// storm cannot fan out unboundedly). The Bus owns the authoritative pool (3.6); this is the Fabric's
/// defence-in-depth cap.
pub const AGENT_DISPATCH_POOL_CAP: u32 = 256;

/// **A structural loop-guard verdict — drop/park, NEVER fork (§5.5, AG-D7).** Mirrors
/// [`myelin_flow::LoopVerdict`] exactly: an [`Admit`](Self::Admit) lets the hop proceed; a
/// [`Drop`](Self::Drop) sheds it outright (self-guard / reference-gate / depth-ceiling / tripwire); a
/// [`Park`](Self::Park) holds it (bounded pool at cap). There is deliberately **no `Fork` variant** —
/// the whole posture is that a self-feeding loop is stopped, never multiplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardVerdict {
    /// the hop is ADMITTED — past all five guards (not self, a structured ref, within the ceiling,
    /// under the shared-root window, under the dispatch-pool cap).
    Admit,
    /// the hop is DROPPED — a guard shed it outright (self-guard, reference-gate raw-text rejection,
    /// depth-ceiling hit, or shared-root tripwire firing). The runaway chain is shed (never forked).
    Drop(GuardRefusal),
    /// the hop is PARKED — the bounded dispatch pool is at cap. Held (shed/parked, never forked); it is
    /// admitted later when an in-flight dispatch releases a slot.
    Park(GuardRefusal),
}

impl GuardVerdict {
    /// `true` iff the hop was admitted (not dropped/parked).
    pub fn is_admit(&self) -> bool {
        matches!(self, GuardVerdict::Admit)
    }
    /// `true` iff the hop was refused (dropped OR parked) — the loop was stopped.
    pub fn is_refused(&self) -> bool {
        !self.is_admit()
    }
    /// The machine reason a refused hop carries (for the audit), or `None` on admit.
    pub fn refusal(&self) -> Option<GuardRefusal> {
        match self {
            GuardVerdict::Admit => None,
            GuardVerdict::Drop(r) | GuardVerdict::Park(r) => Some(*r),
        }
    }
}

/// **Which structural guard refused a hop — the machine reason the audit records (no PII; EI-02 §4).**
/// Surfaced so a refusal is OBSERVABLE, never a silent drop. The three engine-owned reasons
/// ([`DepthCeiling`](Self::DepthCeiling)/[`SharedRootTripwire`](Self::SharedRootTripwire)/
/// [`DispatchPoolFull`](Self::DispatchPoolFull)) map 1:1 onto [`myelin_flow::RefusalReason`]; the two
/// Fabric-owned reasons ([`SelfTrigger`](Self::SelfTrigger)/[`RawTextNotAReference`](Self::
/// RawTextNotAReference)) are this module's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardRefusal {
    /// the self-guard fired — the inbound event's `actor.principal` IS this agent (an agent's own
    /// emission can never re-trigger the agent).
    SelfTrigger,
    /// the reference gate fired — the would-be re-trigger was RAW TYPED TEXT, not a structured
    /// `artifact_ref` node. Only a structured node may re-trigger a run (0 raw-text re-triggers).
    RawTextNotAReference,
    /// the causal-depth ceiling was hit (`depth + 1 > AGENT_CEILING`).
    DepthCeiling,
    /// the shared-root tripwire fired — the per-tenant breaker tripped (too many same-root dispatches
    /// in the window).
    SharedRootTripwire,
    /// the bounded dispatch pool is at cap (over-cap → shed/park).
    DispatchPoolFull,
}

impl From<RefusalReason> for GuardRefusal {
    /// Map the engine's three loop-safety reasons onto the agent-fabric reasons (defence-in-depth: the
    /// Fabric re-enforces the engine's three guards and surfaces the SAME machine reason).
    fn from(r: RefusalReason) -> Self {
        match r {
            RefusalReason::DepthCeiling => GuardRefusal::DepthCeiling,
            RefusalReason::SharedRootTripwire => GuardRefusal::SharedRootTripwire,
            RefusalReason::ActivityPoolFull => GuardRefusal::DispatchPoolFull,
        }
    }
}

// ───────────────────────────── guard 1: the self-guard ─────────────────────────────

/// **The self-guard (§5.5) — drop an inbound event whose `actor.principal == this agent`.** *An agent's
/// own emission can never re-trigger the agent.* A free function (no state): the verdict is a pure read
/// of the envelope's [`Actor`] against the agent's own [`PrincipalId`]. This is the simplest of the five
/// guards and the first line of defence against a one-hop self-loop (an agent posts a comment → the
/// comment event would re-deliver to the same agent → dropped before it ever costs a run).
#[derive(Clone, Debug)]
pub struct SelfGuard {
    /// the agent's own principal id — an inbound event whose `actor.principal` equals this is dropped.
    agent: PrincipalId,
}

impl SelfGuard {
    /// A self-guard for the agent principal `agent` — every inbound event is checked against it.
    pub fn new(agent: PrincipalId) -> SelfGuard {
        SelfGuard { agent }
    }

    /// The agent principal this guard protects.
    pub fn agent(&self) -> &PrincipalId {
        &self.agent
    }

    /// **Admit (or drop) an inbound event by its actor (§5.5).** If `actor.principal == this agent` the
    /// event is the agent's OWN emission re-arriving — [`GuardVerdict::Drop`] ([`SelfTrigger`](
    /// GuardRefusal::SelfTrigger)), never forked. Otherwise admit (a HUMAN or ANOTHER agent's event may
    /// legitimately trigger this agent).
    pub fn admit(&self, actor: &Actor) -> GuardVerdict {
        if actor.0.principal_id == self.agent {
            GuardVerdict::Drop(GuardRefusal::SelfTrigger)
        } else {
            GuardVerdict::Admit
        }
    }

    /// Convenience: admit (or drop) a whole [`EventEnvelope`] by its `actor` field.
    pub fn admit_envelope(&self, ev: &EventEnvelope) -> GuardVerdict {
        self.admit(&ev.actor)
    }
}

// ───────────────────────────── guard 2: the reference gate ─────────────────────────────

/// **The reference gate (§5.5) — ONLY a structured `artifact_ref` node may re-trigger a run.** Raw
/// typed text NEVER does (0 raw-text re-triggers). Wired to the FROZEN 13.1 inline ref nodes
/// ([`myelin_content::InlineNode`]): a re-trigger is admitted IFF it carries a structured
/// [`InlineNode::ArtifactRefNode`] (the only node a re-trigger may key on). A [`InlineNode::Mention`]
/// (an `@agent` mention) is the EXPLICIT-dispatch surface (CHAT-1) — it notifies but does NOT itself
/// auto-re-trigger a costed run on the loop path; an [`InlineNode::Embed`] is a display node, not a
/// re-trigger. **Raw typed text — a plain string with no structured node — can NEVER re-trigger**,
/// which is exactly why a human (or agent) *cannot typo their way into a loop* (EI-02 §6).
#[derive(Clone, Copy, Debug, Default)]
pub struct ReferenceGate;

impl ReferenceGate {
    /// A fresh reference gate (stateless).
    pub fn new() -> ReferenceGate {
        ReferenceGate
    }

    /// **Admit (or drop) a would-be re-trigger by its inline node (§5.5).** Only a structured
    /// [`InlineNode::ArtifactRefNode`] re-triggers — [`GuardVerdict::Admit`]. A [`Mention`](
    /// InlineNode::Mention) or [`Embed`](InlineNode::Embed) is NOT a loop-path re-trigger here, and any
    /// raw typed text (which is NOT an `InlineNode` at all — see [`admit_raw_text`](Self::
    /// admit_raw_text)) is rejected. Returns [`GuardVerdict::Drop`] ([`RawTextNotAReference`](
    /// GuardRefusal::RawTextNotAReference)) for a non-`ArtifactRefNode` node.
    pub fn admit_node(&self, node: &InlineNode) -> GuardVerdict {
        match node {
            InlineNode::ArtifactRefNode(_) => GuardVerdict::Admit,
            // a Mention is explicit-dispatch (notify), not a loop re-trigger; an Embed is display.
            InlineNode::Mention(_) | InlineNode::Embed(_) => {
                GuardVerdict::Drop(GuardRefusal::RawTextNotAReference)
            }
        }
    }

    /// **Raw typed text NEVER re-triggers a run (§5.5) — the 0-raw-text-re-trigger invariant.** A
    /// would-be re-trigger that is a PLAIN STRING (no structured node) is ALWAYS dropped
    /// ([`RawTextNotAReference`](GuardRefusal::RawTextNotAReference)). This is the structural reason a
    /// human or agent cannot typo into a loop: typing `"@agent please loop"` (or pasting an artifact's
    /// raw URL as text) produces no `ArtifactRefNode`, so it cannot re-trigger. The `_text` argument is
    /// taken to make the call site read intentionally ("admit this raw text") even though the verdict is
    /// invariant — a raw string is, by construction, never a structured reference.
    pub fn admit_raw_text(&self, _text: &str) -> GuardVerdict {
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference)
    }
}

// ───────────────────────────── guard 5: idempotent tools ─────────────────────────────

/// **The idempotent-tool ledger (§5.5) — apply-time dedup keyed on `(run, effect_id)`.** The
/// apply-time re-enforcement the Fabric OWNS (the prompt's owned contract): a tool effect that has
/// already applied under one `(run, effect_id)` key is a NO-OP on a re-apply — [`record`](Self::record)
/// returns `false` and the second apply mutates NOTHING. This is the structural reason a loop that
/// re-delivers the SAME effect (a retried dispatch, a double-clicked resume) never double-mutates: the
/// `(run, effect_id)` key dedups it. Reconciles with the per-effect [`crate::hitl_batch::ApplyLedger`]
/// (the HITL exactly-once binding, AG-D5) — that ledger is keyed on the per-effect HITL `idem_key`; THIS
/// one is keyed on `(run, effect_id)` for the general (non-HITL) tool-apply path. Two complementary
/// idempotency surfaces, NOT a second engine: both record-each-key-exactly-once with the same shape.
#[derive(Clone, Debug, Default)]
pub struct IdempotentToolLedger {
    /// the `(run, effect_id)` keys that have ALREADY applied (each maps to exactly one apply).
    applied: BTreeSet<(String, String)>,
}

impl IdempotentToolLedger {
    /// A fresh (empty) ledger — no effect has applied yet.
    pub fn new() -> IdempotentToolLedger {
        IdempotentToolLedger::default()
    }

    /// **Build the `(run, effect_id)` idempotency key (§5.5).** The apply-time key the ledger records:
    /// a tool effect is identified by the run it belongs to AND its per-run `effect_id`. Distinct runs
    /// keep distinct keys (run A's effect 1 is NOT run B's effect 1); within a run, distinct effects
    /// keep distinct keys; the SAME effect re-applied keeps the SAME key (→ deduped).
    pub fn key(run: &str, effect_id: &str) -> (String, String) {
        (run.to_string(), effect_id.to_string())
    }

    /// **Record an apply under `(run, effect_id)` — returns `true` on the FIRST apply, `false` on a
    /// re-apply (the dedup, §5.5).** The structural exactly-once guarantee: the first time this
    /// `(run, effect_id)` is recorded the tool may apply (`true`); every subsequent record under the
    /// same key is a NO-OP (`false`) — the second apply mutates nothing. *A loop that re-delivers the
    /// same effect double-mutates 0 times.*
    pub fn record(&mut self, run: &str, effect_id: &str) -> bool {
        self.applied.insert(Self::key(run, effect_id))
    }

    /// Whether `(run, effect_id)` has already applied (the dedup read — `true` means a re-apply is a
    /// no-op).
    pub fn contains(&self, run: &str, effect_id: &str) -> bool {
        self.applied.contains(&Self::key(run, effect_id))
    }

    /// The number of DISTINCT `(run, effect_id)` keys that applied — the exactly-once parity number a
    /// drill measures (it equals the number of unique effects applied, never the number of apply CALLS).
    pub fn applies(&self) -> usize {
        self.applied.len()
    }
}

// ───────────────────────────── the composed five-guard surface ─────────────────────────────

/// **The composed five structural loop guards, re-enforced at the Fabric tier (§5.5, AG-D7).** One
/// handle the Fabric's dispatch + apply path consults: the self-guard, the reference gate (both owned
/// here), the causal-depth ceiling, the shared-root tripwire, the bounded dispatch pool (all RE-USED
/// from [`myelin_flow::CausalGuard`], defence in depth), and the apply-time idempotent-tool ledger
/// (owned here). Every refusal is a [`GuardVerdict::Drop`]/[`GuardVerdict::Park`] — nothing ever forks.
///
/// The default caps are the AGENT-lane ones ([`AGENT_CEILING`] = 12 / [`AGENT_SHARED_ROOT_CAP`] /
/// [`AGENT_DISPATCH_POOL_CAP`]); [`with_caps`](Self::with_caps) drives small caps so the AG-D7
/// adversarial-loop drill hits them fast.
#[derive(Clone)]
pub struct AgentLoopGuards {
    self_guard: SelfGuard,
    reference_gate: ReferenceGate,
    /// the engine's three loop-safety mechanisms (depth ceiling + shared-root tripwire + bounded pool),
    /// re-used at the agent-lane caps — NOT re-implemented (EI-01 §7 coherence; P-FLOW-18/P-214).
    causal: CausalGuard,
}

impl AgentLoopGuards {
    /// The five guards for `agent`, at the DEFAULT agent-lane caps ([`AGENT_CEILING`] = 12 /
    /// [`AGENT_SHARED_ROOT_CAP`] / [`AGENT_DISPATCH_POOL_CAP`]), no telemetry wired.
    pub fn new(agent: PrincipalId) -> AgentLoopGuards {
        AgentLoopGuards {
            self_guard: SelfGuard::new(agent),
            reference_gate: ReferenceGate::new(),
            causal: CausalGuard::with_caps(
                AGENT_CEILING,
                AGENT_SHARED_ROOT_CAP,
                AGENT_DISPATCH_POOL_CAP,
            ),
        }
    }

    /// The five guards for `agent` at EXPLICIT caps — the AG-D7 in-isolation drill drives small caps
    /// (a ceiling of, say, 12, a pool cap of 2) so the adversarial loop hits them fast without spawning
    /// thousands of hops.
    pub fn with_caps(
        agent: PrincipalId,
        ceiling: u32,
        shared_root_cap: u32,
        pool_cap: u32,
    ) -> AgentLoopGuards {
        AgentLoopGuards {
            self_guard: SelfGuard::new(agent),
            reference_gate: ReferenceGate::new(),
            causal: CausalGuard::with_caps(ceiling, shared_root_cap, pool_cap),
        }
    }

    /// Wire the [`FlowTelemetry`] the re-used causal guard feeds (the causal-depth histogram + the
    /// depth-ceiling hits / shared-root tripwire firings / pool sheds / the 0-fork counter). Builder.
    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> AgentLoopGuards {
        self.causal = self.causal.with_telemetry(telemetry);
        self
    }

    /// The configured causal-depth ceiling (the agent-lane ceiling the Fabric re-enforces).
    pub fn ceiling(&self) -> u32 {
        self.causal.ceiling()
    }

    /// The self-guard sub-surface (guard 1).
    pub fn self_guard(&self) -> &SelfGuard {
        &self.self_guard
    }

    /// The reference-gate sub-surface (guard 2).
    pub fn reference_gate(&self) -> &ReferenceGate {
        &self.reference_gate
    }

    /// **Admit (or refuse) a would-be agent DISPATCH (a child run re-triggered by an event) — the
    /// full self→reference→depth→tripwire gate (§5.5, AG-D7).** The order is the cheapest-first
    /// defence:
    ///
    /// 1. **self-guard** — `actor.principal == this agent`? → DROP ([`SelfTrigger`](GuardRefusal::
    ///    SelfTrigger)). An agent's own emission never re-triggers it.
    /// 2. **reference gate** — is the re-trigger a structured `artifact_ref` node? Raw text → DROP
    ///    ([`RawTextNotAReference`](GuardRefusal::RawTextNotAReference)). 0 raw-text re-triggers.
    /// 3. **causal-depth ceiling + shared-root tripwire** — re-used from [`CausalGuard::admit_child`]:
    ///    `depth + 1 > ceiling` → DROP ([`DepthCeiling`]); same-root over cap → DROP
    ///    ([`SharedRootTripwire`], the per-tenant breaker trips).
    ///
    /// The `re_trigger` is the inline node that would re-trigger the run (the reference-gate input).
    /// On admit the child depth is observed into the causal-depth histogram and the root tally advances.
    /// NEVER forks.
    pub fn admit_dispatch(
        &self,
        actor: &Actor,
        re_trigger: &InlineNode,
        correlation_id: &str,
        parent_depth: u32,
    ) -> GuardVerdict {
        // (1) self-guard — the cheapest read: drop the agent's own emission before anything else.
        let v = self.self_guard.admit(actor);
        if v.is_refused() {
            return v;
        }
        // (2) reference gate — only a structured artifact_ref node re-triggers; raw text never does.
        let v = self.reference_gate.admit_node(re_trigger);
        if v.is_refused() {
            return v;
        }
        // (3) the engine's depth-ceiling + shared-root tripwire (defence in depth; never forks).
        let (verdict, reason) = self.causal.admit_child(correlation_id, parent_depth);
        match verdict {
            LoopVerdict::Admit => GuardVerdict::Admit,
            LoopVerdict::Drop => {
                GuardVerdict::Drop(reason.expect("a drop carries a reason").into())
            }
            LoopVerdict::Park => {
                GuardVerdict::Park(reason.expect("a park carries a reason").into())
            }
        }
    }

    /// **Admit (or park) a would-be dispatch into the bounded pool (§5.5 / X-3) — defers to the Bus's
    /// pool shape ([`CausalGuard::admit_activity`]).** Over-cap → [`GuardVerdict::Park`]
    /// ([`DispatchPoolFull`](GuardRefusal::DispatchPoolFull)) — the fan-out is bounded, never forked.
    /// The caller MUST [`release_dispatch`](Self::release_dispatch) when the dispatch terminates.
    pub fn admit_dispatch_pool(&self) -> GuardVerdict {
        let (verdict, reason) = self.causal.admit_activity();
        match verdict {
            LoopVerdict::Admit => GuardVerdict::Admit,
            LoopVerdict::Drop => {
                GuardVerdict::Drop(reason.expect("a drop carries a reason").into())
            }
            LoopVerdict::Park => {
                GuardVerdict::Park(reason.expect("a park carries a reason").into())
            }
        }
    }

    /// Release one admitted dispatch (it terminated) — frees a pool slot (saturating). Defers to
    /// [`CausalGuard::release_activity`].
    pub fn release_dispatch(&self) {
        self.causal.release_activity();
    }

    /// The current concurrent-dispatch count (the bounded-pool gauge) — for the drill's pool assertion.
    pub fn dispatches_in_flight(&self) -> u32 {
        self.causal.activities_in_flight()
    }

    /// The number of dispatches seen for `correlation_id` in the window (the shared-root tripwire
    /// tally) — for the drill's tripwire assertion.
    pub fn root_dispatches(&self, correlation_id: &str) -> u32 {
        self.causal.root_starts(correlation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::InlineNode;
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalKind, RuntimeRef};
    use myelin_tenancy::TenantId;

    fn agent_principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            TenantId("acme".into()),
        )
    }

    fn human_principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn artifact_ref_node() -> InlineNode {
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()))
    }

    // ───────── guard 1: self-guard ─────────

    /// **The self-guard drops the agent's OWN emission (§5.5).** An inbound event whose
    /// `actor.principal == this agent` is the agent re-triggering itself — DROPPED ([`SelfTrigger`]),
    /// never forked. A human's (or another agent's) event is admitted.
    #[test]
    fn self_guard_drops_own_emission_admits_others() {
        let guard = SelfGuard::new(PrincipalId("agent-alice".into()));

        // the agent's OWN emission re-arriving → dropped.
        let own = Actor(agent_principal("agent-alice"));
        let v = guard.admit(&own);
        assert_eq!(v, GuardVerdict::Drop(GuardRefusal::SelfTrigger));
        assert!(v.is_refused(), "a self-trigger is refused");

        // a HUMAN's event → admitted (a human may legitimately trigger the agent).
        let human = Actor(human_principal("user-bob"));
        assert_eq!(guard.admit(&human), GuardVerdict::Admit);

        // ANOTHER agent's event → admitted (agent→agent IS allowed, bounded by depth/tripwire).
        let other = Actor(agent_principal("agent-carol"));
        assert_eq!(guard.admit(&other), GuardVerdict::Admit);
    }

    // ───────── guard 2: reference gate ─────────

    /// **The reference gate admits ONLY a structured `artifact_ref` node — raw text NEVER re-triggers
    /// (§5.5, the 0-raw-text invariant).** This is THE structural reason a human/agent cannot typo into
    /// a loop: a plain string produces no `ArtifactRefNode`, so it cannot re-trigger.
    #[test]
    fn reference_gate_admits_only_artifact_ref_node_never_raw_text() {
        let gate = ReferenceGate::new();

        // a structured artifact_ref node → admitted (the ONLY re-trigger).
        assert_eq!(gate.admit_node(&artifact_ref_node()), GuardVerdict::Admit);

        // RAW TYPED TEXT → ALWAYS dropped (0 raw-text re-triggers), no matter the content.
        for raw in [
            "@agent-alice please re-run this",
            "myelin://acme/issues/issue/PROJ-1", // even an artifact URL typed as TEXT is not a node.
            "",
        ] {
            let v = gate.admit_raw_text(raw);
            assert_eq!(
                v,
                GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
                "raw text {raw:?} must NEVER re-trigger",
            );
        }

        // a Mention is explicit-dispatch (notify), NOT a loop re-trigger here → dropped on the loop path.
        let mention = InlineNode::Mention(agent_principal("agent-alice"));
        assert_eq!(
            gate.admit_node(&mention),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "a mention is explicit-dispatch, not a loop re-trigger",
        );
        // an Embed is a display node, not a re-trigger → dropped.
        let embed = InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into()));
        assert_eq!(
            gate.admit_node(&embed),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        );
    }

    // ───────── guard 5: idempotent tools ─────────

    /// **The idempotent-tool ledger dedups on `(run, effect_id)` (§5.5).** The first apply records
    /// (`true`); a re-apply under the SAME `(run, effect_id)` is a NO-OP (`false`) — 0 double-mutation.
    /// Distinct runs and distinct effects keep distinct keys.
    #[test]
    fn idempotent_tool_ledger_dedups_on_run_effect_id() {
        let mut ledger = IdempotentToolLedger::new();

        // first apply of (run-1, eff-1) → records, may apply.
        assert!(ledger.record("run-1", "eff-1"), "first apply records");
        assert!(ledger.contains("run-1", "eff-1"));

        // a re-apply of the SAME (run, effect_id) → no-op (a loop re-delivering the effect 0-mutates).
        assert!(
            !ledger.record("run-1", "eff-1"),
            "a re-apply under the same key is a NO-OP",
        );

        // a DISTINCT effect in the same run → distinct key → applies.
        assert!(ledger.record("run-1", "eff-2"), "a distinct effect applies");
        // the SAME effect_id in a DISTINCT run → distinct key → applies (run A eff 1 != run B eff 1).
        assert!(ledger.record("run-2", "eff-1"), "a distinct run applies");

        // the exactly-once parity: 3 DISTINCT keys applied (run-1/eff-1, run-1/eff-2, run-2/eff-1),
        // even though `record` was CALLED 4 times (one was the deduped re-apply).
        assert_eq!(ledger.applies(), 3, "exactly 3 distinct effects applied");
    }

    // ───────── guards 3+4 (re-used): depth ceiling + shared-root tripwire ─────────

    /// **The composed dispatch gate halts a self-feeding loop at the ceiling (§5.5, AG-D7).** A loop
    /// that keeps dispatching a child at `depth + 1` (carrying a STRUCTURED artifact_ref, from a
    /// DIFFERENT actor each hop so the self-guard does not pre-empt) is admitted up to the ceiling, then
    /// the next hop is DROPPED ([`DepthCeiling`]) — the depth never exceeds the ceiling, never forked.
    #[test]
    fn composed_gate_halts_self_feeding_loop_at_ceiling() {
        let telemetry = FlowTelemetry::new();
        // small ceiling so the loop hits it fast; a generous tripwire/pool so ONLY depth fires.
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 10_000, 10_000)
                .with_telemetry(telemetry.clone());
        let other = Actor(human_principal("user-bob")); // not the agent → self-guard passes.
        let node = artifact_ref_node(); // a structured ref → reference gate passes.
        let root = "corr-loop";

        let mut depth = 0u32;
        let mut admitted = 0u32;
        let mut dropped = 0u32;
        for _ in 0..50 {
            let v = guards.admit_dispatch(&other, &node, root, depth);
            match v {
                GuardVerdict::Admit => {
                    admitted += 1;
                    depth += 1; // the loop self-feeds: the child becomes the next parent.
                }
                GuardVerdict::Drop(r) => {
                    dropped += 1;
                    assert_eq!(r, GuardRefusal::DepthCeiling);
                    break;
                }
                GuardVerdict::Park(_) => panic!("the depth ceiling drops, it does not park"),
            }
        }

        assert_eq!(
            admitted, 12,
            "admitted exactly up to the ceiling (children 1..=12)"
        );
        assert_eq!(dropped, 1, "the hop past the ceiling was dropped");
        assert!(
            telemetry.causal_depth_max() <= guards.ceiling(),
            "the causal-depth max never exceeds the ceiling — halted AT it",
        );
        assert_eq!(
            telemetry.causal_depth_max(),
            12,
            "deepest admitted child at the ceiling"
        );
        assert_eq!(telemetry.depth_ceiling_hits(), 1, "the ceiling fired once");
        assert_eq!(
            telemetry.fork_count(),
            0,
            "NEVER forked — the headline invariant"
        );
    }

    /// **The shared-root tripwire trips the per-tenant breaker on a wide same-root loop (§5.5, AG-D7).**
    /// A loop that stays SHALLOW but re-enters the SAME correlation root is caught by the ROOT, not the
    /// depth: past the window cap the tripwire fires (drop, never fork).
    #[test]
    fn composed_gate_trips_shared_root_breaker_on_wide_loop() {
        let telemetry = FlowTelemetry::new();
        // generous depth so ONLY the tripwire can stop this loop.
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 10_000, 3, 10_000)
                .with_telemetry(telemetry.clone());
        let other = Actor(human_principal("user-bob"));
        let node = artifact_ref_node();
        let root = "corr-shared";

        let mut admitted = 0u32;
        let mut tripped = 0u32;
        for _ in 0..10 {
            let v = guards.admit_dispatch(&other, &node, root, 1); // shallow depth — only root catches.
            match v {
                GuardVerdict::Admit => admitted += 1,
                GuardVerdict::Drop(r) => {
                    tripped += 1;
                    assert_eq!(r, GuardRefusal::SharedRootTripwire);
                }
                GuardVerdict::Park(_) => panic!("the tripwire drops, it does not park"),
            }
        }

        assert_eq!(
            admitted, 3,
            "the first 3 same-root dispatches admitted (the window cap)"
        );
        assert_eq!(
            tripped, 7,
            "every same-root dispatch past the cap tripped the breaker"
        );
        assert_eq!(
            telemetry.depth_ceiling_hits(),
            0,
            "depth NEVER fired (the loop stayed shallow)"
        );
        assert!(
            telemetry.shared_root_tripwire_firings() >= 1,
            "the breaker tripped"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    /// **The bounded dispatch pool caps concurrency (§5.5 / X-3).** Admitting up to the cap succeeds;
    /// the next is SHED/PARKED ([`DispatchPoolFull`]), never forked. Releasing one frees a slot.
    #[test]
    fn composed_gate_bounds_dispatch_pool_never_forks() {
        let telemetry = FlowTelemetry::new();
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 10_000, 10_000, 2)
                .with_telemetry(telemetry.clone());

        assert_eq!(guards.admit_dispatch_pool(), GuardVerdict::Admit);
        assert_eq!(guards.admit_dispatch_pool(), GuardVerdict::Admit);
        assert_eq!(guards.dispatches_in_flight(), 2, "the pool is at cap");

        // over-cap → PARK, never fork.
        let v = guards.admit_dispatch_pool();
        assert_eq!(v, GuardVerdict::Park(GuardRefusal::DispatchPoolFull));
        assert_eq!(telemetry.activity_pool_sheds(), 1, "one shed recorded");

        guards.release_dispatch();
        assert_eq!(guards.dispatches_in_flight(), 1);
        assert_eq!(
            guards.admit_dispatch_pool(),
            GuardVerdict::Admit,
            "a freed slot admits"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    /// **The self-guard pre-empts the other guards — a self-trigger is dropped BEFORE depth even
    /// counts (§5.5).** An agent's OWN emission carrying a structured ref at depth 0 is dropped on the
    /// self-guard, NOT admitted by depth — the cheapest-first order is correct.
    #[test]
    fn self_guard_preempts_in_composed_gate() {
        let telemetry = FlowTelemetry::new();
        let guards = AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 64, 256)
            .with_telemetry(telemetry.clone());
        let own = Actor(agent_principal("agent-alice")); // the agent's OWN emission.
        let node = artifact_ref_node(); // even a STRUCTURED ref does not save it.

        let v = guards.admit_dispatch(&own, &node, "corr", 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::SelfTrigger),
            "self-guard pre-empts"
        );
        // the causal guard was never consulted → no depth observed, no fork.
        assert_eq!(
            telemetry.causal_depth_max(),
            0,
            "depth never observed (self-guard pre-empted)"
        );
        assert_eq!(telemetry.fork_count(), 0);
    }

    /// **The reference gate pre-empts depth — raw text at a shallow depth is dropped, not admitted
    /// (§5.5).** A would-be re-trigger from another actor at depth 0 but with a NON-ref node (a mention)
    /// is dropped on the reference gate.
    #[test]
    fn reference_gate_preempts_depth_in_composed_gate() {
        let guards = AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 64, 256);
        let other = Actor(human_principal("user-bob"));
        let mention = InlineNode::Mention(human_principal("user-bob")); // NOT a structured ref.

        let v = guards.admit_dispatch(&other, &mention, "corr", 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "the reference gate pre-empts: a non-ref re-trigger is dropped",
        );
    }

    /// **The verdict predicates partition admit from refused** — only `Admit` is an admit; both `Drop`
    /// and `Park` are refusals; the refusal reason is surfaced on both.
    #[test]
    fn verdict_predicates_and_refusal_surface() {
        assert!(GuardVerdict::Admit.is_admit());
        assert!(GuardVerdict::Admit.refusal().is_none());
        let d = GuardVerdict::Drop(GuardRefusal::SelfTrigger);
        assert!(d.is_refused());
        assert_eq!(d.refusal(), Some(GuardRefusal::SelfTrigger));
        let p = GuardVerdict::Park(GuardRefusal::DispatchPoolFull);
        assert!(p.is_refused());
        assert_eq!(p.refusal(), Some(GuardRefusal::DispatchPoolFull));
    }

    /// **The default agent-lane ceiling is 12 (AG-D7).** The Fabric re-enforces the TIGHTER agent
    /// ceiling, not the engine's own wider in-process one — the AG-D7 halt bound.
    #[test]
    fn default_agent_ceiling_is_twelve() {
        let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
        assert_eq!(
            guards.ceiling(),
            12,
            "the agent-lane ceiling default is 12 (AG-D7)"
        );
        assert_eq!(AGENT_CEILING, 12);
    }
}
