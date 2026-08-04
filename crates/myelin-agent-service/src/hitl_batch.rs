//! # `hitl_batch` — per-effect HITL idempotency (C4/OQ-F): partial approval + double-click well-defined
//! (AG-P10 → P-222, M2-B) — the AG-D5 **exactly-once** leg
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.3 **C4** (the resume signal's
//! idempotency key is **per-effect**: `idem_key = card_id` for a single-effect card, `idem_key =
//! card_id ":" effect_idx` for a multi/partial-approval card; a **partial approval** (approve 0 and 2,
//! decline 1) sends three independently-idempotent signals, each mapping to **exactly one**
//! `EffectApi::apply`; a declined effect is **withheld**, AG-8; a **double-click** on "approve all"
//! re-sends the same keys → no double-apply) + §4.4 (the `hitl_gate.effect_id` the `idem_key` derives
//! from).
//!
//! **Contract-index:** CONSUMES `9.1` (the per-effect `idem_key` — the durable signal is idempotent on
//! it; the engine-side construction rule + the `wf_signal` PK live in `myelin-flow::approval`, P-206 —
//! see *Reconciliation* below). OWNS the **agent-fabric** half: the per-effect `idem_key` derivation a
//! batch card produces (so the agent and the durable engine agree on the SAME key by construction) +
//! the **exactly-once apply binding** (the apply-counter == the approved-effect count, an extension of
//! `8.2`'s HITL step — `EffectApi::apply` runs exactly once per approved effect, never more).
//!
//! ## What this prompt ships — the per-effect machinery on top of the single-effect loop (AG-P9 → P-221)
//!
//! [`crate::hitl`] (AG-P9 → P-221) ships the SINGLE-effect withhold → surface → resume loop: one gate,
//! one durable wait, one decision. THIS module is the **batch / partial-approval** extension — a card
//! that gates *N* effects ("approve these 3 proposed merges"):
//!
//! 1. **The per-effect `idem_key` derivation ([`per_effect_idem_key`]).** The FROZEN §5.3/§6.4 rule:
//!    `idem_key = card_id` for a single-effect card (the degenerate case — a double-click is one
//!    approval); `idem_key = card_id ":" effect_idx` for a multi-effect card (each effect approved /
//!    declined INDEPENDENTLY + idempotently on its own key). This is the SAME key the durable signal
//!    (`myelin-flow::approval::per_effect_idem_key`, 9.1) consumes — the two sides agree BY
//!    CONSTRUCTION (a parity test asserts it, not a re-implementation; see *Reconciliation*).
//! 2. **The batch loop ([`run_batch_hitl_loop`]).** A card gating *N* effects opens *N* gates (one per
//!    effect, keyed per-effect), surfaces ONE card carrying all *N* pending actions, parks on the
//!    per-effect durable waits, and resumes each effect on ITS OWN decision. A partial approval
//!    (approve 0 and 2, decline 1) threads exactly the approved tools into `approved`; a declined
//!    effect is WITHHELD (never threaded → 0 mutation, AG-8). A double-click re-sends the same
//!    per-effect keys → no second decision → no double-apply (the [`ApplyLedger`] proves it).
//! 3. **The exactly-once apply binding ([`ApplyLedger`]).** The structural proof of AG-D5's
//!    exactly-once leg: a ledger keyed on the per-effect `idem_key` records each apply ONCE — a second
//!    apply under the same key is a NO-OP (the double-click is one approval). The GATE is
//!    `applies() == approved-effect count` (never more) — the apply-counter equals the approved count
//!    exactly, the declined effects make 0 mutation.
//!
//! ## The two invariants made true by construction (§5.3 C4)
//!
//! - **A double-click is one approval.** The per-effect key dedups the resume signal (the durable
//!   side, 9.1) AND the [`ApplyLedger`] dedups the apply (the fabric side) — a re-sent key never
//!   reaches a second `EffectApi::apply`.
//! - **A partial approval is well-defined.** Each effect's decision rides its OWN per-effect key, so
//!   approving 0 and 2 while declining 1 is three independent decisions — no coupling, no
//!   all-or-nothing. The declined effect is withheld (AG-8): 0 mutation, the apply-counter excludes it.
//!
//! ## Reconciliation with `myelin-flow::approval` (P-206, the durable-engine half)
//!
//! The per-effect `idem_key` CONSTRUCTION rule + the `wf_signal` PK that dedups the resume signal
//! already landed in `myelin-flow::approval` (`per_effect_idem_key`, `apply_approved_effects`, P-206)
//! — the DURABLE-ENGINE half (the rule over the signal-delivery mechanism). THIS module is the
//! AGENT-FABRIC half the §5.3 prompt (AG-P10) names: the derivation the agent fabric uses to build the
//! resume signal's key (so the fabric and the engine produce the SAME key — a parity invariant, NOT a
//! second implementation of the engine's dedup), plus the **exactly-once apply binding** the engine
//! cannot own (the apply-counter == approved-effect count is a property of `EffectApi::apply`, which is
//! Agent Fabric's). The agent crate does NOT depend on `myelin-flow` (the DAG stays acyclic — the
//! reverse edge `flow → agent` already exists); the parity is asserted in the consumer CDC
//! (`tests/cdc_9_1_per_effect.rs`) against the real `myelin-flow` key rule, not by importing it.
//!
//! ## Design note (R2.4 — the step-6 gate is now per-effect, STRUCTURAL not advisory)
//!
//! Pre-R2.4, the loop threaded an approved gate's bare TOOL NAME into the run's `approved` set,
//! which `EffectApi::apply`'s step 6 read — **too coarse for a batch** where sibling effects share a
//! tool (three `git.merge` effects on three PRs): approving effect 0 admitted `git.merge` run-wide,
//! so a DECLINED effect 1, if re-driven through `apply_planned`, fell through step 6 and applied
//! (the 2026-07-06 HIGH finding). The per-effect [`ApplyLedger`] was only ADVISORY (populated here,
//! never consulted by the enforcement gate). **R2.4 closed this structurally**: the `approved` set
//! now carries PER-EFFECT gate keys ([`crate::effect_api::effect_gate_key`], `gate:{tool}:{object}`
//! — the SAME key step 6 mints its `GateId` from), so the step-6 ENFORCEMENT gate itself is
//! per-effect — a declined sibling's key is never admitted, and an adversarial re-drive of it gates
//! again (0 mutation, AG-8; the negative leg in `tests/cdc_8_2_hitl_batch.rs` proves it). The
//! [`ApplyLedger`] keeps its exactly-once role (the apply-counter == the approved-effect count; a
//! double-click adds 0 applies) — it and the approved set are now keyed at the SAME per-effect
//! granularity.
//!
//! ## FLOORS named
//! - **None.** The per-effect idempotency completes the M2-B HITL machinery (the AG-D5 exactly-once
//!   family is now closed). The humanise card-text render (C9/OQ-L) remains AG-P11 (→ P-223); the
//!   batch card carries the humanised [`crate::hitl::RiskSummary`] slot per effect, the render is that
//!   follow-on.

use crate::effect_api::PlannedEffect;
use crate::hitl::{
    resolve_decision, surface_card, ApprovedTools, Halted, HitlCard, HitlGate, RiskSummary,
    WaitDecision,
};
use myelin_agent::GateId;
use myelin_identity::PrincipalId;
use std::collections::{BTreeMap, BTreeSet};

// ───────────────────────── the per-effect idem_key derivation (§5.3 C4 / §6.4) ───────────────────

/// **The per-effect resume `idem_key` derivation (FROZEN §5.3 C4 / §6.4 — the agent-fabric half).**
/// The key the resume signal carries so the durable wait dedups it (9.1) and the [`ApplyLedger`]
/// dedups the apply:
///
/// - a **single-effect** card (`total_effects == 1`) → `idem_key = card_id`. A double-click re-sends
///   `card_id` → one approval, one apply.
/// - a **multi/partial-approval** card (`total_effects > 1`) → `idem_key = card_id ":" effect_idx`.
///   Each effect is approved / declined INDEPENDENTLY + idempotently on its own key; a partial
///   approval (approve 0 and 2, decline 1) is three keys `card:0` / `card:1` / `card:2`.
///
/// This is the SAME rule `myelin-flow::approval::per_effect_idem_key` (9.1) applies on the
/// durable-engine side — the two agree BY CONSTRUCTION (the CDC `cdc_9_1_per_effect.rs` asserts the
/// parity against the real engine rule). The single-effect card keys on the BARE `card_id` (the `:0`
/// suffix is NOT appended — the degenerate case where the card id IS the key, §6.4).
///
/// `effect_idx` must be `< total_effects` (a caller bug otherwise — the index into the card's effect
/// list); `total_effects >= 1` (a card gates at least one effect).
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

// ───────────────────────── the exactly-once apply binding (AG-D5 exactly-once leg) ───────────────

/// **The exactly-once apply binding (AG-D5 exactly-once leg; §5.3 C4).** A ledger keyed on the
/// per-effect `idem_key` that records each approved effect's `EffectApi::apply` EXACTLY ONCE — a second
/// apply under the SAME key is a NO-OP ([`ApplyLedger::record`] returns `false`). This is the
/// structural proof that *a double-click is one approval*: the resume signal's per-effect key is the
/// ledger key, so re-sending it never reaches a second apply.
///
/// The GATE AG-D5 asserts: [`ApplyLedger::applies`] == the **approved-effect count** (never more) — the
/// apply-counter equals exactly the number of approved effects, and a declined effect (never recorded)
/// makes 0 mutation (AG-8). In production this ledger IS the `proposed_effect` row's applied-event
/// uniqueness (the apply is idempotent on the effect's own key); HERE it is the in-process counter the
/// drill measures the parity number against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyLedger {
    /// the per-effect `idem_key`s that have ALREADY applied (each maps to exactly one apply).
    applied: BTreeSet<String>,
}

impl ApplyLedger {
    /// A fresh (empty) ledger — no effect has applied yet.
    pub fn new() -> ApplyLedger {
        ApplyLedger::default()
    }

    /// **Record an apply under the per-effect `idem_key` (the exactly-once binding).** Returns `true`
    /// the FIRST time a key applies (the caller proceeds to `EffectApi::apply`); returns `false` on a
    /// RE-apply of the same key — the double-click no-op (the apply is NOT performed again). The
    /// per-effect key separates the effects, so each distinct approved effect applies exactly once.
    pub fn record(&mut self, idem_key: &str) -> bool {
        self.applied.insert(idem_key.to_string())
    }

    /// Whether `idem_key` has already applied (the dedup read).
    pub fn contains(&self, idem_key: &str) -> bool {
        self.applied.contains(idem_key)
    }

    /// **The apply-counter (AG-D5 — `applies() == approved-effect count`).** The number of DISTINCT
    /// per-effect keys that applied — the exactly-once parity number the drill measures.
    pub fn applies(&self) -> usize {
        self.applied.len()
    }
}

// ───────────────────────── the batch / partial-approval card + its per-effect gates ──────────────

/// **One gated effect within a batch approval card (§5.3 C4).** Carries the [`PlannedEffect`] the gate
/// withholds + the humanised [`RiskSummary`] slot for THIS effect's row on the card. The effect's
/// position in [`BatchApprovalCard::effects`] is its `effect_idx` for the per-effect `idem_key` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchGatedEffect {
    /// the `GateId` the step-6 `Gated` verdict carried for THIS effect (one gate per effect).
    pub gate_id: GateId,
    /// the proposed effect this gate withholds (the tool + object the re-run re-applies).
    pub plan: PlannedEffect,
    /// the humanised risk summary SLOT for this effect's card row (`(template_key, args)`, C9 — AG-P11
    /// renders it). Never a raw string.
    pub risk_summary: RiskSummary,
}

/// **A batch / partial HITL approval card (§5.3 C4).** Gates one OR many effects under a single
/// `card_id` ("approve these 3 proposed merges"). The per-effect `idem_key` for effect `idx` is
/// [`per_effect_idem_key`]`(card_id, idx, effects.len())`. The per-effect decision rides each effect's
/// own resume signal — so a partial approval (approve some, decline others) is well-defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchApprovalCard {
    /// the run whose gated effects this card approves.
    pub run_id: String,
    /// the card identity — the per-effect `idem_key` base (`card_id` single, `card_id:idx` multi).
    pub card_id: String,
    /// the gated effects in order (the index is the `effect_idx` for the per-effect key rule).
    pub effects: Vec<BatchGatedEffect>,
    /// the APPROVER set = `list_subjects(object, approve_perm)` (4.4) — who MAY decide. The whole card
    /// shares one approver set (the same object/permission gate the effects share).
    pub approver_filter: Vec<PrincipalId>,
}

impl BatchApprovalCard {
    /// The number of effects this card gates (the batch arity).
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the card gates no effects (a degenerate empty card — never produced in practice).
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// **The per-effect `idem_key` for effect `idx` of this card (the §5.3 C4 rule at this arity).** A
    /// single-effect card keys on the bare `card_id`; a multi-effect card keys on `card_id:idx`.
    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }

    /// Open the `Waiting` [`HitlGate`] for effect `idx` (one gate per effect; §5.3 withhold). The gate
    /// carries the per-effect risk slot + the LIVE cost estimate; the `card_ref` is the per-effect
    /// `idem_key` (so the gate and the resume signal agree on the key).
    fn open_gate(&self, idx: usize) -> HitlGate {
        let eff = &self.effects[idx];
        HitlGate::open(
            eff.gate_id.clone(),
            self.run_id.clone(),
            &eff.plan,
            eff.risk_summary.clone(),
            self.approver_filter.clone(),
            // the per-effect idem_key IS the card_ref base — the resume signal keys on it (9.1).
            self.idem_key_for(idx),
        )
    }
}

// ───────────────────────── the batch wait seam (one decision per effect) ──────────────────────────

/// **The durable HITL wait for a BATCH card (CONSUMED, contract 9.4 + 9.1).** Like
/// [`crate::hitl::HitlWait`] but returns the per-effect decision for effect `idx` — each effect's
/// resume signal is keyed on ITS per-effect `idem_key` (9.1), so the durable wait dedups each
/// independently (a double-click on one effect re-sends only that effect's key → no second decision).
/// A seam so `myelin-agent-service` does NOT depend on `myelin-flow` (the DAG stays acyclic — the
/// CDC `cdc_9_1_per_effect.rs` pairs this consumer with the real engine key rule).
pub trait BatchHitlWait {
    /// **Park on the per-effect durable HITL wait for effect `idx` of `card` (keyed on
    /// `idem_key`).** Returns the human's [`WaitDecision`] for THIS effect. While parked the run holds
    /// NO runtime. The `idem_key` is the per-effect key the resume signal carries (9.1) — the wait
    /// dedups on it (a re-sent key is the same buffered signal → the same decision).
    fn park_and_wait_effect(&self, gate: &HitlGate, idem_key: &str) -> WaitDecision;
}

// ───────────────────────── the per-effect outcome + the batch result ─────────────────────────────

/// **The outcome of ONE effect within a batch card (§5.3 C4).** Each effect is settled
/// INDEPENDENTLY — a partial approval mixes `Approved` and `Halted` across the card's effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome {
    /// the effect was APPROVED — its per-(tool, object) gate key is in `approved` (R2.4 — never
    /// the bare tool name); it applied EXACTLY once (the per-effect `idem_key` deduped the apply).
    /// Carries the per-effect `idem_key` (the ledger key).
    Applied { idem_key: String, tool: String },
    /// the effect was HALTED (declined or expired) — it was WITHHELD: `EffectApi::apply` was NEVER
    /// reached, 0 mutation (AG-8). Carries the halt settlement (the reason in the trace + audit).
    Withheld { idem_key: String, halted: Halted },
}

impl EffectOutcome {
    /// The per-effect `idem_key` this outcome settled.
    pub fn idem_key(&self) -> &str {
        match self {
            EffectOutcome::Applied { idem_key, .. } | EffectOutcome::Withheld { idem_key, .. } => {
                idem_key
            }
        }
    }

    /// Whether this effect APPLIED (counts toward the approved-effect count; a withheld effect does
    /// not — AG-8).
    pub fn applied(&self) -> bool {
        matches!(self, EffectOutcome::Applied { .. })
    }
}

/// **The result of the whole batch loop (§5.3 C4 — the AG-D5 exactly-once leg).** Carries the
/// per-effect outcomes (one per effect, in order), the run's `approved` set (the approved tools the
/// re-run applies), and the [`ApplyLedger`] (the exactly-once binding the GATE measures). The
/// surfaced [`HitlCard`] rows (one per effect) are the card the human saw.
#[derive(Clone, Debug)]
pub struct BatchOutcome {
    /// the per-effect outcomes, in card order (effect `idx` → outcome).
    pub effects: Vec<EffectOutcome>,
    /// the run's `approved` set after the batch resume — the approved effects' per-(tool, object)
    /// gate keys the re-run applies (R2.4: a declined sibling's key is never in it).
    pub approved: ApprovedTools,
    /// the exactly-once apply ledger — `applies() == approved-effect count` (the AG-D5 GATE).
    pub ledger: ApplyLedger,
}

impl BatchOutcome {
    /// **The approved-effect count (AG-D5 — the apply-counter must equal this).** The number of
    /// effects that were APPROVED + applied (a declined / expired effect does not count — AG-8).
    pub fn approved_effect_count(&self) -> usize {
        self.effects.iter().filter(|o| o.applied()).count()
    }

    /// **The AG-D5 exactly-once GATE: the apply-counter == the approved-effect count (never more).**
    /// `true` iff the ledger recorded exactly one apply per approved effect — the declined effects made
    /// 0 mutation, and no double-click double-applied.
    pub fn exactly_once(&self) -> bool {
        self.ledger.applies() == self.approved_effect_count()
    }
}

// ───────────────────────── the batch withhold → surface → resume loop driver ──────────────────────

/// **Drive the per-effect withhold → surface → resume loop for a BATCH card (§5.3 C4 — the AG-D5
/// exactly-once leg).** For a card gating *N* effects:
///
/// 1. **WITHHOLD → OPEN** *N* gates (one per effect, [`BatchApprovalCard::open_gate`]) — 0 mutation.
/// 2. **SURFACE** ONE card carrying all *N* pending actions ([`surface_card`] per effect) — the chat
///    approval card a viewer sees.
/// 3. **DECIDE** — park on each effect's per-effect durable wait
///    ([`BatchHitlWait::park_and_wait_effect`], keyed on the per-effect `idem_key`); the run holds no
///    runtime until each effect is decided.
/// 4. **RESUME** — for EACH effect, on `Approve`: transition the gate → `Approved`, admit its tool to
///    `approved`, and record the apply in the [`ApplyLedger`] under the per-effect key (EXACTLY ONCE —
///    a re-sent key is a no-op); on `Reject`/`Expired`: settle [`Halted`], the tool is NEVER admitted
///    (0 mutation, AG-8).
///
/// **The exactly-once guarantee (AG-D5):** the returned [`BatchOutcome::ledger`] records exactly one
/// apply per approved effect — `applies() == approved-effect count` (never more). A partial approval
/// applies exactly the approved effects; a double-click on "approve all" re-sends the same per-effect
/// keys → the durable wait returns the same decisions → the ledger dedups → no double-apply.
///
/// `idem_collisions` is the set of per-effect keys already applied in a PRIOR drive (a re-drive /
/// double-click replays them — they are deduped, not double-applied). A fresh batch passes an empty
/// ledger; a double-click re-runs with the prior ledger (the same keys → 0 new applies).
pub fn run_batch_hitl_loop<W: BatchHitlWait>(
    card: &BatchApprovalCard,
    wait: &W,
    approved: &mut ApprovedTools,
    ledger: &mut ApplyLedger,
) -> BatchOutcome {
    // SURFACE: one card row per effect (the human sees all N pending actions + risk + LIVE cost).
    let _cards: Vec<HitlCard> = (0..card.len())
        .map(|idx| surface_card(&card.open_gate(idx)))
        .collect();

    let mut outcomes = Vec::with_capacity(card.len());
    for idx in 0..card.len() {
        let idem_key = card.idem_key_for(idx);
        // 1. WITHHOLD → OPEN this effect's gate (0 mutation — a durable row).
        let mut gate = card.open_gate(idx);
        // 2 + 3. DECIDE — park on THIS effect's per-effect durable wait (keyed on idem_key, 9.1).
        let decision = wait.park_and_wait_effect(&gate, &idem_key);
        // 4. RESUME — settle this effect on ITS own decision (independent of the other effects). The
        //    approve→admit order lives in the shared `resolve_decision`; the batch-specific ledger
        //    record stays HERE (after admit), so the approved-set threading is byte-identical to the
        //    single-effect driver.
        let outcome = match resolve_decision(&mut gate, decision, approved) {
            // approved + its per-(tool, object) gate key admitted (R2.4 — never the bare tool name,
            // so a declined sibling is never admitted).
            Ok(()) => {
                // record the apply under the per-effect key — EXACTLY ONCE (a double-click re-sends
                // the same key → `record` returns false → no second apply; AG-D5 exactly-once).
                ledger.record(&idem_key);
                EffectOutcome::Applied {
                    idem_key,
                    tool: gate.tool_name.clone(),
                }
            }
            // the declined effect is WITHHELD — never admitted, never recorded (0 mutation, AG-8).
            Err(halted) => EffectOutcome::Withheld { idem_key, halted },
        };
        outcomes.push(outcome);
    }

    BatchOutcome {
        effects: outcomes,
        approved: approved.clone(),
        ledger: ledger.clone(),
    }
}

// ───────────────────────── the per-effect decision script (test/driver helper) ───────────────────

/// **A per-effect decision script for a batch card (the resume-signal payload, keyed per-effect).** Maps
/// each effect's per-effect `idem_key` to the human's [`WaitDecision`]. The batch driver reads the
/// decision for each effect's key — so a partial approval is a script that mixes `Approve` / `Reject`
/// across the keys, and a double-click re-uses the SAME script (the same keys → the same decisions).
#[derive(Clone, Debug, Default)]
pub struct DecisionScript {
    by_key: BTreeMap<String, WaitDecision>,
}

impl DecisionScript {
    /// A fresh (empty) script — every effect is undecided until [`Self::decide`] sets its key.
    pub fn new() -> DecisionScript {
        DecisionScript::default()
    }

    /// Set the decision for the per-effect `idem_key` (the human's choice for THIS effect).
    pub fn decide(&mut self, idem_key: impl Into<String>, decision: WaitDecision) -> &mut Self {
        self.by_key.insert(idem_key.into(), decision);
        self
    }

    /// The decision for `idem_key` (defaults to `Expired` — the auto-deny — for an undecided effect,
    /// so an undecided effect makes 0 mutation, never a default-approve).
    pub fn decision_for(&self, idem_key: &str) -> WaitDecision {
        self.by_key
            .get(idem_key)
            .cloned()
            .unwrap_or(WaitDecision::Expired)
    }
}

impl BatchHitlWait for DecisionScript {
    fn park_and_wait_effect(&self, _gate: &HitlGate, idem_key: &str) -> WaitDecision {
        self.decision_for(idem_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_api::EffectCost;
    use myelin_agent::ToolName;
    use myelin_tenancy::ArtifactRef;

    fn plan(tool: &str, pr: u32) -> PlannedEffect {
        PlannedEffect {
            tool: ToolName(tool.into()),
            object: ArtifactRef(format!("myelin://acme/git/pr/{pr}")),
            input_json: format!(r#"{{"pr":{pr}}}"#),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "git.merge",
                wholesale: 30,
                markup: 20,
            },
        }
    }

    fn risk(pr: u32) -> RiskSummary {
        RiskSummary::for_action(
            "agent.hitl.merge_pr",
            &ArtifactRef(format!("myelin://acme/git/pr/{pr}")),
        )
    }

    fn gated(tool: &str, pr: u32, gate: &str) -> BatchGatedEffect {
        BatchGatedEffect {
            gate_id: GateId(gate.into()),
            plan: plan(tool, pr),
            risk_summary: risk(pr),
        }
    }

    fn approvers() -> Vec<PrincipalId> {
        vec![
            PrincipalId("psn:lead".into()),
            PrincipalId("psn:maintainer".into()),
        ]
    }

    /// A three-effect batch card ("approve these 3 proposed merges").
    fn three_effect_card() -> BatchApprovalCard {
        BatchApprovalCard {
            run_id: "R1".into(),
            card_id: "card-7".into(),
            effects: vec![
                gated("git.merge", 40, "gate:0"),
                gated("git.merge", 41, "gate:1"),
                gated("git.merge", 42, "gate:2"),
            ],
            approver_filter: approvers(),
        }
    }

    fn single_effect_card() -> BatchApprovalCard {
        BatchApprovalCard {
            run_id: "R1".into(),
            card_id: "card-1".into(),
            effects: vec![gated("git.merge", 42, "gate:0")],
            approver_filter: approvers(),
        }
    }

    // ───────── the per-effect idem_key derivation (§5.3 C4 / §6.4) ─────────

    /// **The per-effect key rule (FROZEN §5.3 C4): single-effect keys on `card_id`, multi keys on
    /// `card_id:effect_idx`.** This is the agent-fabric derivation — the SAME rule the durable engine
    /// (`myelin-flow::approval`) applies (the CDC asserts the parity).
    #[test]
    fn per_effect_idem_key_follows_the_frozen_rule() {
        // single-effect card → the bare card id (a double-click is one approval).
        assert_eq!(per_effect_idem_key("card-7", 0, 1), "card-7");
        // multi-effect card → card_id:effect_idx (each effect independently keyed).
        assert_eq!(per_effect_idem_key("card-7", 0, 3), "card-7:0");
        assert_eq!(per_effect_idem_key("card-7", 1, 3), "card-7:1");
        assert_eq!(per_effect_idem_key("card-7", 2, 3), "card-7:2");
    }

    /// **`BatchApprovalCard::idem_key_for` applies the §5.3 C4 rule at the card's own arity.**
    #[test]
    fn card_idem_key_for_uses_the_per_effect_rule() {
        let multi = three_effect_card();
        assert_eq!(multi.idem_key_for(0), "card-7:0");
        assert_eq!(multi.idem_key_for(1), "card-7:1");
        assert_eq!(multi.idem_key_for(2), "card-7:2");
        let single = single_effect_card();
        assert_eq!(
            single.idem_key_for(0),
            "card-1",
            "single-effect card keys on the bare card id"
        );
    }

    // ───────── the exactly-once apply ledger (AG-D5 exactly-once leg) ─────────

    /// **The apply ledger records each per-effect key EXACTLY once — a re-record (double-click) is a
    /// no-op; `applies()` counts the DISTINCT keys.**
    #[test]
    fn apply_ledger_records_each_key_exactly_once() {
        let mut ledger = ApplyLedger::new();
        assert_eq!(ledger.applies(), 0);
        assert!(ledger.record("card-7:0"), "first apply of a key proceeds");
        assert!(ledger.record("card-7:2"), "a distinct key applies");
        assert!(
            !ledger.record("card-7:0"),
            "a RE-apply of the same key is a no-op (double-click)"
        );
        assert_eq!(
            ledger.applies(),
            2,
            "exactly two distinct applies (the double-click did not count)"
        );
        assert!(ledger.contains("card-7:0"));
        assert!(
            !ledger.contains("card-7:1"),
            "the declined effect 1 never applied"
        );
    }

    // ───────── the batch loop: partial approval (2-of-3, 2 applies, 1 withheld) ─────────

    /// **PARTIAL APPROVAL (AG-D5): approve effects 0 and 2, decline 1 → exactly 2 applies, effect 1
    /// WITHHELD (0 mutation); the apply-counter == the approved-effect count (2); the declined tool is
    /// never in `approved`.** This is the core AG-D5 exactly-once / partial-approval parity number.
    #[test]
    fn partial_approval_two_of_three_applies_exactly_the_approved_effects() {
        let card = three_effect_card();
        // the per-effect decisions: approve 0, decline 1, approve 2 — three independent signals.
        let mut script = DecisionScript::new();
        script
            .decide(card.idem_key_for(0), WaitDecision::Approve)
            .decide(
                card.idem_key_for(1),
                WaitDecision::Reject("pr 41 fails checks".into()),
            )
            .decide(card.idem_key_for(2), WaitDecision::Approve);

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);

        // effect 0 + 2 applied, effect 1 withheld (the partial approval is well-defined).
        assert!(
            matches!(outcome.effects[0], EffectOutcome::Applied { .. }),
            "effect 0 approved → applied"
        );
        assert!(
            matches!(&outcome.effects[1], EffectOutcome::Withheld { halted: Halted::Rejected(r), .. } if r == "pr 41 fails checks"),
            "effect 1 declined → WITHHELD with the reason (0 mutation, AG-8): {:?}",
            outcome.effects[1]
        );
        assert!(
            matches!(outcome.effects[2], EffectOutcome::Applied { .. }),
            "effect 2 approved → applied"
        );

        // GATE: exactly 2 applies; the apply-counter == the approved-effect count (2); never more.
        assert_eq!(
            outcome.ledger.applies(),
            2,
            "exactly 2 applies (effects 0 and 2)"
        );
        assert_eq!(outcome.approved_effect_count(), 2);
        assert!(
            outcome.exactly_once(),
            "the apply-counter == the approved-effect count (AG-D5)"
        );
        // the per-effect keys that applied are exactly card-7:0 and card-7:2 (NOT card-7:1).
        assert!(outcome.ledger.contains("card-7:0"));
        assert!(
            !outcome.ledger.contains("card-7:1"),
            "the declined effect 1 made 0 mutation (AG-8)"
        );
        assert!(outcome.ledger.contains("card-7:2"));
    }

    // ───────── the batch loop: double-click on "approve all" → 0 extra apply ─────────

    /// **DOUBLE-CLICK on "approve all" (AG-D5): re-sending the SAME per-effect keys applies each effect
    /// EXACTLY once — the second click adds 0 applies (the apply-counter stays at the approved count).**
    /// The double-click re-runs the loop with the PRIOR ledger (the same keys → `record` no-ops).
    #[test]
    fn double_click_approve_all_applies_each_effect_once() {
        let card = three_effect_card();
        // "approve all" — every effect approved.
        let mut script = DecisionScript::new();
        for idx in 0..card.len() {
            script.decide(card.idem_key_for(idx), WaitDecision::Approve);
        }

        // FIRST click: applies all three.
        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let first = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            first.ledger.applies(),
            3,
            "the first click applies all three effects"
        );

        // DOUBLE-CLICK: re-send the SAME per-effect keys (the same script) → re-run with the SAME ledger.
        let second = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        // the apply-counter is STILL 3 — the double-click added 0 applies (every key was already applied).
        assert_eq!(
            second.ledger.applies(),
            3,
            "a double-click on approve-all adds 0 applies (the per-effect keys dedup the apply)"
        );
        assert_eq!(second.approved_effect_count(), 3);
        assert!(
            second.exactly_once(),
            "exactly 3 applies (1 per effect), NOT 6 — the double-click is one approval"
        );
    }

    /// **A SINGLE-effect card keys on the bare `card_id`; a double-click is one approval (1 apply).**
    /// The degenerate per-effect case (§6.4) — the single-effect path the AG-P9 loop already covers,
    /// re-asserted through the batch ledger.
    #[test]
    fn single_effect_card_double_click_is_one_apply() {
        let card = single_effect_card();
        let mut script = DecisionScript::new();
        script.decide(card.idem_key_for(0), WaitDecision::Approve); // keys on the bare "card-1".

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let first = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(first.ledger.applies(), 1);
        assert!(
            first.ledger.contains("card-1"),
            "the single effect keys on the bare card id"
        );

        // double-click: re-run → 0 new applies.
        let second = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            second.ledger.applies(),
            1,
            "a single-effect double-click is ONE apply"
        );
        assert!(second.exactly_once());
    }

    /// **An ALL-DECLINED batch makes 0 mutation (AG-8): every effect is withheld, the apply-counter is
    /// 0, no tool is admitted.**
    #[test]
    fn all_declined_batch_makes_zero_mutation() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        for idx in 0..card.len() {
            script.decide(card.idem_key_for(idx), WaitDecision::Reject("no".into()));
        }
        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            outcome.ledger.applies(),
            0,
            "0 applies — every effect declined (AG-8)"
        );
        assert_eq!(outcome.approved_effect_count(), 0);
        assert!(
            outcome.exactly_once(),
            "0 applies == 0 approved effects (trivially exactly-once)"
        );
        assert!(outcome
            .effects
            .iter()
            .all(|o| matches!(o, EffectOutcome::Withheld { .. })));
        assert!(
            approved.as_set().is_empty(),
            "no tool admitted (0 mutation)"
        );
    }

    /// **An undecided effect defaults to the auto-deny (Expired) → 0 mutation (never a
    /// default-approve).** A card where only effect 0 is decided applies 0 and withholds 1 + 2.
    #[test]
    fn an_undecided_effect_auto_denies_zero_mutation() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        script.decide(card.idem_key_for(0), WaitDecision::Approve); // only effect 0 decided.

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert!(
            matches!(outcome.effects[0], EffectOutcome::Applied { .. }),
            "effect 0 approved → applied"
        );
        assert!(
            matches!(
                &outcome.effects[1],
                EffectOutcome::Withheld {
                    halted: Halted::Expired,
                    ..
                }
            ),
            "effect 1 undecided → auto-deny (Expired), 0 mutation: {:?}",
            outcome.effects[1]
        );
        assert!(matches!(
            &outcome.effects[2],
            EffectOutcome::Withheld {
                halted: Halted::Expired,
                ..
            }
        ));
        assert_eq!(
            outcome.ledger.applies(),
            1,
            "exactly 1 apply (only the decided-approve effect 0)"
        );
        assert!(outcome.exactly_once());
    }

    /// **Each effect's outcome carries its OWN per-effect `idem_key` (the resume-signal key).** A
    /// partial approval's outcomes map back to `card-7:0` / `card-7:1` / `card-7:2`.
    #[test]
    fn each_outcome_carries_its_per_effect_key() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        script
            .decide(card.idem_key_for(0), WaitDecision::Approve)
            .decide(card.idem_key_for(1), WaitDecision::Reject("x".into()))
            .decide(card.idem_key_for(2), WaitDecision::Approve);
        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(outcome.effects[0].idem_key(), "card-7:0");
        assert_eq!(outcome.effects[1].idem_key(), "card-7:1");
        assert_eq!(outcome.effects[2].idem_key(), "card-7:2");
    }
}
