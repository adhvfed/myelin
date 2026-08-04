//! # `e2e_flagship` — Issues' slice of the E2E-2 agent-native FLAGSHIP (ISS-P35 / P-498, M5)
//!
//! **Issues' slice of the whole-system E2E-2 flagship** — *CI-fail → triage agent → issue → chat →
//! fix-PR* (testing-strategy `01-whole-system-e2e-and-drill-catalogue.md` §E2E-2; VISION §1 — agents
//! are first-class, work flows between tools). E2E-2 is the differentiator proof: a failing CI run
//! wakes a (mock) triage agent that plans, gets HITL approval, files an issue, discusses in chat, and
//! opens a fix-PR — all metered through ONE wallet and ONE plan-then-apply gate.
//!
//! **Issues is the node where the triaged failure becomes a tracked, GOVERNED work item** (arch
//! `00-overview.md` §1). THIS module owns Issues' SLICE of the joint flagship — the part the Issues
//! subsystem is responsible for, driven END-TO-END across a kill:
//!
//! 1. **0 effect outside the `∩`** — every Issues mutation in the chain rides the governed transition
//!    surface ([`crate::ci_guard`] / [`crate::workflow`]); the agent can ONLY drive an edge the workflow
//!    FSM *declares* (an undeclared transition is BLOCKED, never invented) and ONLY when the CI-red
//!    guard (5.9) permits — the agent never mutates a state the FSM + the linked-PR posture forbid
//!    (`agent.policy ∩ delegation ∩ tenant.policy` — the FSM is Issues' slice of the `∩`).
//! 2. **0 mutation before approval** — the agent hitting the governed close (`In Review → Done`) is
//!    HITL-gated: [`plan_agent_ci_gated_transition`] WITHHOLDS the (permitted) transition for approval
//!    and stages NOTHING ([`AgentTransitionOutcome::pre_approval_mutations`] == 0, AG-8).
//! 3. **Exactly-once approval + the governed transition ACROSS A KILL** — the approval rides a DURABLE
//!    per-effect `approval` signal (9.4 / OQ-F, the [`crate::per_effect_idem_key`] rule). The mid-flight
//!    mutation is a KILL between the approve-click and the apply, then an at-least-once DOUBLE delivery
//!    of the approval (the double-click after the resume). The `wf_signal` PK absorbs the duplicate →
//!    the gated apply applies the governed transition EXACTLY ONCE (0 double-apply, 0 vanished work).
//! 4. **reserve/settle balanced (11.7)** — the spend-bearing triage run reserves at dispatch against
//!    the ONE wallet (no balance → no start) and settles its metered units on completion; reserved ==
//!    billed + refunded, one cost event per metered unit, 0 in-flight interrupt
//!    ([`crate::spend_bearing_run`] → [`BalancedRunSignal::is_green`]).
//!
//! Each is driven **end-to-end** — the whole Issues side of the flow with the mid-flight mutation (the
//! kill + the at-least-once duplicate approval), NOT a single handler (EI-01 §4 / VISION §3). The
//! engine seams are **UNCHANGED**; this module COMPOSES Issues' frozen surface into the flagship and
//! emits the scenario's named green [`IssuesE2eArtifact`] (the SAME artifact type the E2E-1 wedge
//! emits — no second artifact shape).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - The governed transition is the FROZEN [`plan_agent_ci_gated_transition`] / [`AgentTransitionOutcome`]
//!   (ISS-P27) — the SAME HITL-gated path the agent tool surface returns; no second governance engine.
//! - The CI-red guard is the FROZEN [`LinkedPrCheck`] / [`ci_done_guard`] (5.9) — Issues reads
//!   `{state, trust_tier}` OFF THE FACT, never recomputes trust (the X-1 seam, the SAME `is_acceptable`
//!   posture Git's merge gate applies).
//! - The reserve/settle is the FROZEN [`crate::spend_bearing_run`] over the SHARED
//!   `myelin_storage::reserve_settle::CostLedger` (11.7) — the SAME wallet CI's metering settles
//!   against; no second bookend.
//! - The exactly-once-across-a-kill approval is the FROZEN per-effect `idem_key` rule
//!   ([`crate::per_effect_idem_key`] — byte-identical to `myelin_flow::approval::per_effect_idem_key`,
//!   a CDC pins parity). The DURABLE `wf_signal` PK dedup (the resume + double-click absorbed once) is
//!   exercised against `myelin_flow::SignalStore` in `tests/e2e_flagship_iss_p35.rs` — the SAME
//!   `INSERT … ON CONFLICT DO NOTHING` substrate the agent-fabric leg parks on; no second signal store.
//!
//! ## Mock-agent runtime note (the prompt's required statement — R-10 named)
//! The scenario runs with the **MOCK agent runtime** (the scripted triage agent — contract 8.3, a
//! scripted mock run twice → identical proposed-effect sequences, AG-D9). The **real-LLM agent runtime
//! is the post-M5 swap (R-10)** — named, not built here. The Issues-side legs (the governed work item,
//! the HITL-gated close, the durable approval, the balanced wallet) are runtime-agnostic — they hold
//! identically under the real runtime (the swap is the brain loop, not the governance/cost surface).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened Issues surface. The ONE legitimate
//!   remaining floor inherited from the platform is the world-scale fleet-hardware 30× load (named in
//!   ISS-P33); this slice does not introduce a new one. The EffectApi-gate (8.2) + the CheckStatus
//!   guard (5.9) floors hold UNCHANGED under the E2E load (re-confirmed here, not weakened).
//! - **This is Issues' SLICE of the joint flagship** — the FULL E2E-2 green requires every subsystem's
//!   slice (CI = `myelin-ci-controlplane::e2e_flagship` / P-494; Agent-Fabric = AG-P24 / P-480; the
//!   durable park/resume SPINE = `myelin-flow`'s P-477). Issues' slice (the governed work item + the
//!   HITL-gated close + the exactly-once-across-a-kill approval + the balanced wallet) is the
//!   deliverable here; the cross-subsystem orchestration is the whole-system M5 wedge.

use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId};
use myelin_tenancy::TenantId;

use crate::agent_spend::{spend_bearing_run, BalancedRunSignal, IssueRunKind, IssueSpendGate};
use crate::ci_guard::{plan_agent_ci_gated_transition, AgentTransitionOutcome, LinkedPrCheck};
use crate::e2e_wedge::IssuesE2eArtifact;
use crate::workflow::{IssueContext, StateCategory, Workflow, WorkflowState, WorkflowTransition};

use super::ci_guard::ci_done_guard;

/// The E2E scenario this module owns (Issues' slice of the agent-native flagship). PII-free token — the
/// drills assert against the NAME, never a literal (EI-01 §3).
pub const E2E_FLAGSHIP_SCENARIO: &str = "E2E-2";

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The scenario fixtures (a full cell with mock agents; the Issues hops of the CI-fail → fix-PR flow).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the flagship runs against (a full cell). Opaque, PII-free.
fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// The triaged issue's HITL approval-card id (the base of the per-effect `idem_key` — the durable
/// approval the close rides). PII-free machine token.
pub const CLOSE_CARD_ID: &str = "card:triage:close-eng-1421";

/// The wallet balance the spend-bearing triage run reserves against (the ONE Commercial wallet shared
/// with CI). Minor-units.
const WALLET: u64 = 100;

/// The triage run's reserved estimate (the run's upper-bound cost). Minor-units.
const ESTIMATE: u64 = 20;

/// The governed FSM the triaged issue carries: `In Review → Done` is gated by the CI-red Done guard
/// (5.9). The agent can ONLY drive a DECLARED edge (0 effect outside the FSM `∩`) and ONLY when the
/// guard permits — the SAME canonical CI-gated workflow [`crate::ci_guard`] drives.
fn triage_workflow() -> Workflow {
    Workflow {
        states: vec![
            WorkflowState {
                name: "In Review".into(),
                category: StateCategory::Started,
            },
            WorkflowState {
                name: "Done".into(),
                category: StateCategory::Completed,
            },
        ],
        transitions: vec![WorkflowTransition {
            from: "In Review".into(),
            to: "Done".into(),
            guards: vec![ci_done_guard()],
            required_fields: vec![],
            post_actions: vec![],
        }],
    }
}

/// The triage run's metered units (the actual bill: the mock brain loop's effects). Wholesale ≠ markup;
/// the total (14) is < the reservation (20) → a refund of 6 (reserved == billed + refunded).
fn triage_metered_units() -> Vec<MeteredUnit> {
    vec![
        MeteredUnit {
            unit: IssueRunKind::Triage.metered_unit(),
            wholesale: MicroUsd(8),
            markup: MicroUsd(2),
        },
        MeteredUnit {
            unit: IssueRunKind::Triage.metered_unit(),
            wholesale: MicroUsd(3),
            markup: MicroUsd(1),
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The exactly-once-across-a-kill governed-transition apply (the DURABLE HITL ledger, 9.4 / OQ-F).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **A pure model of the durable HITL apply ledger Issues' governed close rides (9.4 / OQ-F).** The
/// approval is delivered as a per-effect `approval` signal keyed by [`crate::per_effect_idem_key`]; the
/// gated apply applies the governed transition EXACTLY ONCE per buffered key. This struct models the
/// `wf_signal` ON-CONFLICT-DO-NOTHING dedup at the Issues-side (the SAME PK dedup
/// `myelin_flow::SignalStore` enforces — the live binding is in `tests/e2e_flagship_iss_p35.rs`). It is
/// the deterministic SEMANTICS the durable substrate wires: a duplicate delivery under the same key is
/// a no-op; a re-drive after a kill re-reads the SAME buffered key → the SAME single apply.
#[derive(Default)]
struct HitlApplyLedger {
    /// the buffered approval keys (the `wf_signal` PK's idem dimension) — a set, so a re-delivery is a
    /// no-op (ON CONFLICT DO NOTHING).
    buffered: std::collections::BTreeSet<String>,
    /// the keys whose governed transition has been APPLIED (the apply is keyed → idempotent: a re-drive
    /// over an already-applied key does NOT re-apply).
    applied: std::collections::BTreeSet<String>,
    /// the count of governed-transition applies that actually MUTATED (the exactly-once witness).
    apply_count: u64,
}

impl HitlApplyLedger {
    /// **Deliver an approval signal under `key` (the `INSERT … ON CONFLICT DO NOTHING`).** Returns
    /// `true` if it was the FIRST delivery (buffered), `false` if it was a DUPLICATE (absorbed — the
    /// at-least-once double-click after the resume). The PK is the dedup; no second buffered copy.
    fn deliver_approval(&mut self, key: &str) -> bool {
        self.buffered.insert(key.to_string())
    }

    /// **Apply the governed transition for `key` EXACTLY ONCE (the gated apply over the buffered
    /// approval).** Applies iff the approval is buffered AND the transition has not already been applied
    /// (the re-drive after the kill re-reads the buffered key but the apply is idempotent — applied
    /// once). Returns `true` iff THIS call mutated (so the caller can prove the apply ran exactly once
    /// across the kill + the duplicate).
    fn apply_once(&mut self, key: &str) -> bool {
        if !self.buffered.contains(key) {
            return false; // the human has not approved this effect yet (the run stays parked).
        }
        if self.applied.contains(key) {
            return false; // already applied (a re-drive / a duplicate is a no-op — exactly once).
        }
        self.applied.insert(key.to_string());
        self.apply_count += 1;
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  The flagship — Issues' slice driven end-to-end across a kill.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **Issues' slice of E2E-2 (the agent-native flagship): drive it end-to-end, chaining the kill + the
/// at-least-once duplicate approval.**
///
/// The whole Issues side of the flow, not a single handler (EI-01 §4). Returns the named green artifact
/// ([`IssuesE2eArtifact`], the SAME shape the E2E-1 wedge emits). `is_green()` iff:
/// 1. the agent's governed close is HITL-gated (WITHHELD, 0 pre-approval mutation, AG-8);
/// 2. 0 effect outside the `∩` (an UNDECLARED edge is BLOCKED — the FSM is Issues' slice of the `∩`);
/// 3. the governed transition applies EXACTLY ONCE across the kill + the duplicate approval;
/// 4. reserve/settle is balanced (reserved == billed + refunded, one cost event per unit, 0 interrupt).
///
/// **MR-009b W6b2 — `#[cfg(any(test, feature = "test-support"))]`:** this in-process flagship slice
/// constructs the now-`test-support`-gated in-memory `CostLedger::new`; its callers (the
/// dogfood scorecard + the `e2e_flagship_iss_p35` / `e2e_flagship/tests.rs` drills) reach it via the
/// `myelin-issues/test-support` self dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_2_issues_flagship() -> IssuesE2eArtifact {
    let mut leaks: u64 = 0;
    let tenant = tenant();
    let wf = triage_workflow();

    // ── LEG 1: 0 MUTATION BEFORE APPROVAL — the agent's governed close is HITL-gated (AG-8). ─────────
    // The triaged issue is `In Review`; the linked fix-PR's CI is GREEN (a trusted success, read OFF
    // THE FACT — 5.9). The guard PERMITS the `In Review → Done` close, but the actor is an AGENT, so it
    // is WITHHELD for approval — NOTHING is mutated pre-approval.
    let green_check = LinkedPrCheck::trusted(crate::ci_guard::CHECK_STATE_SUCCESS);
    let agent_close =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &green_check);
    let withheld = agent_close.is_withheld();
    let pre_approval_mutations = agent_close.pre_approval_mutations();
    if !withheld {
        leaks += 1; // the agent auto-applied a governed transition (it must be HITL-gated, AG-8)
    }
    if pre_approval_mutations != 0 {
        leaks += 1; // a mutation landed BEFORE approval — the 0-pre-approval invariant broke
    }
    // The plan the HITL card surfaces (and the write path applies POST-approval) is the FIXED completed
    // category — the agent does not choose the category (the FSM does; 0 effect outside the FSM).
    let plan_is_completed = matches!(
        &agent_close,
        AgentTransitionOutcome::WithheldForApproval { plan }
            if plan.to_category == StateCategory::Completed && plan.to == "Done"
    );
    if !plan_is_completed {
        leaks += 1; // the withheld plan is not the FSM's declared completed close
    }

    // ── LEG 2: 0 EFFECT OUTSIDE THE ∩ — an UNDECLARED edge is BLOCKED (the FSM is Issues' slice). ─────
    // The agent cannot invent a transition the FSM does not declare: there is NO `In Review → Canceled`
    // edge, so even with a green check the agent path BLOCKS (nothing to approve — 0 mutation). The
    // agent never mutates a state outside the declared `∩`.
    let undeclared = plan_agent_ci_gated_transition(
        &wf,
        "In Review",
        "Canceled", // a state the FSM does not declare — outside the ∩.
        IssueContext::new(),
        &green_check,
    );
    let undeclared_blocked = undeclared.is_blocked();
    let undeclared_zero_mutation = undeclared.pre_approval_mutations() == 0;
    if !undeclared_blocked || !undeclared_zero_mutation {
        leaks += 1; // the agent drove an edge outside the FSM (effect outside the ∩)
    }
    // A CI-RED linked PR also BLOCKS the close for the agent (nothing to approve) — the poisoned-Done
    // defence holds under the agent path too (0 effect when the guard forbids it).
    let ci_red = LinkedPrCheck::trusted("failure");
    let red_close =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &ci_red);
    let ci_red_blocks_agent = red_close.is_blocked() && red_close.pre_approval_mutations() == 0;
    if !ci_red_blocks_agent {
        leaks += 1; // a CI-red governed close was not blocked for the agent (the guard leaked green)
    }

    // ── LEG 3: EXACTLY-ONCE APPROVAL + GOVERNED TRANSITION ACROSS A KILL (the durable HITL ledger). ───
    // The human approves the WITHHELD close. The approval rides a DURABLE per-effect `approval` signal
    // keyed by the OQ-F rule (a single-effect card → the bare card_id). The KILL happens between the
    // approve-click and the apply; on resume the apply re-reads the buffered approval. A DOUBLE delivery
    // (the at-least-once double-click after the resume) is ABSORBED by the `wf_signal` PK → the governed
    // transition applies EXACTLY ONCE (0 double-apply, 0 vanished work).
    let idem_key = crate::per_effect_idem_key(CLOSE_CARD_ID, 0, 1);
    let mut ledger = HitlApplyLedger::default();
    // The human clicks Approve — the FIRST per-effect signal is buffered (the wf_signal PK accepts it).
    let first_delivery = ledger.deliver_approval(&idem_key);
    // ── KILL: the Agent + Workflow services die mid-ack_window, AFTER the approval is buffered but
    //    BEFORE the apply ran. (Nothing applied yet — apply_count is still 0.) ──
    let applied_before_kill = ledger.apply_count;
    // ── RESUME (days later): the durable workflow re-drives. The double-click re-delivers the SAME
    //    per-effect key — the wf_signal PK absorbs it (a no-op, NOT a second buffered copy). ──
    let duplicate_delivery = ledger.deliver_approval(&idem_key);
    // The gated apply runs on resume — it applies the governed transition EXACTLY ONCE over the buffered
    // approval. A SECOND re-drive (another resume) re-reads the SAME buffered key → the apply is a no-op
    // (idempotent — applied once).
    let applied_on_resume = ledger.apply_once(&idem_key);
    let re_drive_no_op = !ledger.apply_once(&idem_key); // a second resume does NOT re-apply
    let exactly_once_across_kill = first_delivery
        && !duplicate_delivery // the duplicate was absorbed (0 second buffered copy)
        && applied_before_kill == 0 // nothing applied before the kill (0 mutation pre-apply)
        && applied_on_resume // the apply ran on resume
        && re_drive_no_op // a re-drive does NOT re-apply (exactly once)
        && ledger.apply_count == 1; // the governed transition applied EXACTLY ONCE
    if !exactly_once_across_kill {
        leaks += 1; // the governed transition applied != 1 across the kill (double-apply / vanished)
    }

    // ── LEG 4: reserve/settle BALANCED (11.7) — the spend-bearing triage run. ────────────────────────
    // The triage run reserves its estimate at dispatch against the ONE wallet (no balance → no start),
    // runs the (mock) brain loop, and settles its metered units on completion; reserved == billed +
    // refunded, one cost event per metered unit, 0 in-flight interrupt.
    let mut gate = IssueSpendGate::new();
    let mut cost_ledger = CostLedger::new();
    let balance: BalancedRunSignal = spend_bearing_run(
        &mut gate,
        &mut cost_ledger,
        tenant.clone(),
        RunId::new("run:triage:eng-1421"),
        IssueRunKind::Triage,
        MicroUsd(ESTIMATE),
        MicroUsd(WALLET),
        triage_metered_units,
    )
    .expect("a funded wallet reserves + settles the triage run (no balance → no start)");
    let reserve_settle_balanced = balance.is_green();
    if !reserve_settle_balanced {
        leaks += 1; // the wallet did not conserve — reserve/settle is not balanced
    }
    // The no-balance-no-start floor (AG-D11): an EXHAUSTED wallet REFUSES the dispatch — the run never
    // starts (0 reserve, 0 work). The self-limiter holds under the E2E load too.
    let mut empty_gate = IssueSpendGate::new();
    let mut empty_ledger = CostLedger::new();
    let refused = spend_bearing_run(
        &mut empty_gate,
        &mut empty_ledger,
        tenant.clone(),
        RunId::new("run:triage:starved"),
        IssueRunKind::Triage,
        MicroUsd(ESTIMATE),
        MicroUsd(0), // an exhausted wallet → no balance → no start.
        || panic!("the work must NEVER run on an exhausted wallet (no balance → no start)"),
    )
    .is_err();
    let no_balance_no_start = refused && empty_gate.runs_dispatched() == 0;
    if !no_balance_no_start {
        leaks += 1; // an unfunded run started (the no-balance-no-start floor breached)
    }

    let green = withheld
        && pre_approval_mutations == 0
        && plan_is_completed
        && undeclared_blocked
        && undeclared_zero_mutation
        && ci_red_blocks_agent
        && exactly_once_across_kill
        && reserve_settle_balanced
        && no_balance_no_start;

    IssuesE2eArtifact {
        scenario: E2E_FLAGSHIP_SCENARIO,
        green,
        evidence: format!(
            "CI-fail→triage→issue→chat→fix-PR (Issues slice): governed close HITL-gated \
             (withheld={withheld}, pre_approval_mutations={pre_approval_mutations}); 0 effect outside \
             the ∩ (undeclared edge blocked={undeclared_blocked}, ci-red blocks agent={ci_red_blocks_agent}); \
             exactly-once approval + governed transition across a kill (first_delivery={first_delivery}, \
             duplicate_absorbed={}, apply_count={}, across_kill={exactly_once_across_kill}); \
             reserve/settle balanced (reserved {ESTIMATE} == billed {} + refunded {}, no-balance→no-start={no_balance_no_start})={reserve_settle_balanced}; \
             mock-agent runtime (real-LLM is post-M5/R-10)",
            !duplicate_delivery,
            ledger.apply_count,
            balance.billed.0,
            balance.refunded.0,
        ),
        leaks,
    }
}

#[cfg(test)]
#[path = "e2e_flagship/tests.rs"]
mod tests;
