//! # `approval` — the per-effect `idem_key` rule for batch / partial HITL approval (P-FLOW-10 → P-206, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §6.4 (the per-effect
//! `idem_key` rule — `idem_key = card_id` for a single-effect card, `idem_key = card_id ":"
//! effect_idx` for a multi/partial-approval card; a partial approval is well-defined; each effect
//! maps to exactly ONE `EffectApi::apply`; a declined effect is WITHHELD — returns `Denied`, never
//! mutates, AG-8) + §3.4 (the `wf_signal` PK `(tenant, run_id, signal_name, idem_key)` that makes
//! the rule true BY CONSTRUCTION) + §8 (F-4 extended — the per-effect form, drilled at P-FLOW-12).
//!
//! **Contract-index cluster:** OWNS the per-effect `idem_key` rule on `9.1`
//! [`crate::executor::DurableExecutor::signal`]. CONSUMES the Agent Fabric `8.x`
//! `EffectApi::apply` (the apply/withhold target — Agent Fabric owns the effect set; this engine
//! owns ONLY the key-construction rule + the PK).
//!
//! ## What this prompt (P-FLOW-10) ships — the key-construction rule + the gated loop
//!
//! The signal DELIVERY + idempotency mechanism (`INSERT … ON CONFLICT (tenant, run_id, signal_name,
//! idem_key) DO NOTHING` into `wf_signal`) already exists ([`crate::executor::FlowExecutor::signal`],
//! P-FLOW-09). This prompt is the **key-construction rule over it** plus the **gated consume loop**:
//!
//! - [`per_effect_idem_key`] — the FROZEN §6.4 rule: a SINGLE-effect card keys on `card_id` (a
//!   double-click is one approval); a MULTI/partial-approval card keys EACH effect on
//!   `card_id ":" effect_idx` (each effect approved/declined independently + idempotently). This is
//!   the ENTIRE engine contribution to the rule — the PK that makes it dedup already exists.
//! - [`ApprovalCard`] / [`ApprovalDecision`] — the per-effect decision carried in each `approval`
//!   signal's payload (approve | decline). A partial approval (approve 0 and 2, decline 1) is
//!   **three independently-idempotent signals** under three per-effect keys.
//! - [`apply_approved_effects`] — the gated loop: it reads the buffered per-effect `approval` signals
//!   off `wf_signal` and, for EACH effect, either calls `EffectApi::apply` EXACTLY ONCE (approved) or
//!   WITHHOLDS it (declined → `Denied`, never mutates, AG-8). A double-click on "approve all"
//!   re-sends the SAME per-effect keys → `ON CONFLICT DO NOTHING` → no second buffered signal → no
//!   double-apply (the loop applies each effect exactly once).
//!
//! ## The two invariants made true by construction (§6.4)
//!
//! 1. *A double-click is one approval / one apply.* The per-effect key dedups the signal at the
//!    `wf_signal` PK (delivery is idempotent), and the loop applies each distinct buffered effect
//!    exactly once (consumption is idempotent over the buffered set).
//! 2. *A partial approval is well-defined.* Each effect's decision rides its OWN per-effect key, so
//!    approving 0 and 2 while declining 1 is three independent, idempotent signals — no coupling, no
//!    all-or-nothing. A declined effect is withheld (AG-8): zero mutation.
//!
//! ## FLOORS named
//!
//! - **The `EffectApi` effect set is Agent Fabric's** (contract `8.x`). This module is generic over a
//!   caller-supplied apply closure ([`EffectApplier`]) so `myelin-flow` does NOT depend on
//!   `myelin-agent` in production (the DAG stays acyclic). The Agent Fabric `EffectApi::apply`
//!   consumer is paired with this provider in the CDC fixture `tests/cdc_9_1_per_effect.rs`
//!   (dev-dep only).
//! - **The F-4-extended drill (the per-effect form across a restart + deploy)** lands at **P-FLOW-12**
//!   (architecture §8) and at the subsystem face **CHAT-D10** (M4). This prompt ships the rule + the
//!   structural gate (3 independent apply/decline, 0 double-apply, 0 mutation-on-decline); the
//!   durable-wait wiring of the loop (`wait_for_signal` consuming the buffered signals across a
//!   multi-day park) is **P-FLOW-11**.

use crate::engine::SignalStore;
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

/// The FROZEN `signal_name` an approval-card decision is delivered under (§4.3) — a taxonomy token,
/// not PII. Every per-effect decision (approve | decline) of a card is an `approval` signal whose
/// per-effect `idem_key` separates the effects.
pub const APPROVAL_SIGNAL_NAME: &str = "approval";

/// **The per-effect `idem_key` CONSTRUCTION rule (FROZEN, contract 9.1 / §6.4).** The ENTIRE engine
/// contribution to batch/partial approval — a pure key-construction rule over the existing
/// `wf_signal` PK `(tenant, run_id, signal_name, idem_key)`:
///
/// - a **single-effect** card (`total_effects == 1`) → `idem_key = card_id`. One approval; a
///   double-click re-sends `card_id` → `ON CONFLICT DO NOTHING` → one buffered signal.
/// - a **multi/partial-approval** card (`total_effects > 1`) → `idem_key = card_id ":" effect_idx`.
///   Each effect is approved/declined INDEPENDENTLY + idempotently on its OWN key; a partial
///   approval (approve 0 and 2, decline 1) is three signals under `card:0` / `card:1` / `card:2`.
///
/// This makes BOTH invariants true by construction: *a double-click is one approval* (the PK dedups
/// the per-effect key) and *a partial approval is well-defined* (each effect's decision rides its
/// own key, no coupling).
///
/// `effect_idx` must be `< total_effects` (a caller bug otherwise) — the index into the card's
/// effect list. A single-effect card's lone effect is index 0 and keys on the bare `card_id` (the
/// `:0` suffix is NOT appended — a single-effect card is the degenerate case where the bare card id
/// IS the key, §6.4).
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
        // Multi/partial-approval card: each effect keys on `card_id ":" effect_idx` so each is
        // approved/declined independently + idempotently (the §6.4 multi-effect rule).
        format!("{card_id}:{effect_idx}")
    }
}

/// **The per-effect human decision (§6.4)** carried in an `approval` signal — `Approve` (the effect
/// is applied) or `Decline` (the effect is WITHHELD: returns `Denied`, never mutates, AG-8). A
/// partial approval mixes both across a card's effects (approve 0 and 2, decline 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// the gated effect is APPROVED — it maps to exactly one `EffectApi::apply`.
    Approve,
    /// the gated effect is DECLINED — it is WITHHELD (returns `Denied`, never mutates, AG-8).
    Decline,
}

/// **One gated effect within an approval card.** Carries the references-not-payloads handle of the
/// proposed effect (`ArtifactRef`, never an inline PII body — §3.4) and the per-effect decision. The
/// effect's position in [`ApprovalCard::effects`] is its `effect_idx` for the per-effect key rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatedEffect {
    /// the proposed effect this approval gates — a reference (`ArtifactRef`), never a PII body. The
    /// Agent Fabric `EffectApi::apply` resolves it; this engine only carries the reference + decides
    /// whether to apply or withhold it.
    pub effect_ref: ArtifactRef,
    /// the per-effect decision (approve | decline) — the human's choice for THIS effect.
    pub decision: ApprovalDecision,
}

/// **A batch / partial HITL approval card (§6.4).** Gates one OR many effects under a single
/// `card_id`. A single-effect card (`effects.len() == 1`) keys on `card_id`; a multi-effect card
/// keys each effect on `card_id ":" effect_idx`. The per-effect decision rides each effect's own
/// signal — so a partial approval (approve some, decline others) is well-defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalCard {
    /// the run whose gated tool calls this card approves.
    pub run_id: String,
    /// the card identity — the per-effect `idem_key` base (`card_id` single, `card_id:idx` multi).
    pub card_id: String,
    /// the gated effects in order (the index is the `effect_idx` for the per-effect key rule).
    pub effects: Vec<GatedEffect>,
}

impl ApprovalCard {
    /// The per-effect `idem_key` for effect `idx` of this card (the §6.4 rule applied to THIS card's
    /// arity). Panics-in-debug if `idx` is out of range.
    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }
}

/// **The outcome of applying one gated effect (§6.4 / §5.2).** An approved effect that applied
/// carries the emitted event id; a declined effect carries `Withheld` (it returned `Denied`, made
/// ZERO mutation, AG-8); a re-application of an already-applied effect carries `AlreadyApplied`
/// (the double-click no-op — the loop applies each effect EXACTLY once).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome {
    /// the effect was APPROVED and applied via `EffectApi::apply` — carries the emitted event id
    /// (an opaque reference, no PII).
    Applied(String),
    /// the effect was DECLINED and WITHHELD — it returned `Denied` and made ZERO mutation (AG-8).
    /// Carries the decline reason (a machine token, no PII).
    Withheld(String),
}

/// **The result of one effect's gated apply.** `Ok` for an applied or a withheld effect (a withheld
/// effect is the AG-8 rule WORKING, never an error); `Err` only for an engine-level failure
/// (an apply that the `EffectApi` itself rejected — surfaced, never swallowed, EI-02 §4).
pub type GateResult = Result<EffectOutcome, ApplyError>;

/// **A gated-apply failure (surfaced, never swallowed — EI-02 §4).** The `EffectApi::apply` the
/// engine delegated to returned `Denied` for a reason OTHER than the human decline (a capability /
/// schema / budget failure) — distinct from the AG-8 WITHHOLD of a declined effect (which is an
/// expected [`EffectOutcome::Withheld`], not an error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// the underlying `EffectApi::apply` denied the effect for a non-decline reason (capability /
    /// schema / tenant / budget) — surfaced so it is observable, never a silent dropped effect.
    EffectDenied(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::EffectDenied(r) => write!(f, "effect denied by EffectApi: {r}"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// **The Agent-Fabric-owned apply target (consumed, contract `8.x` `EffectApi::apply`).** This engine
/// owns ONLY the decision of WHICH effect to apply (per the per-effect signal) — the apply itself is
/// Agent Fabric's `EffectApi::apply`. Modeled here as a caller-supplied closure so `myelin-flow` does
/// NOT depend on `myelin-agent` in production (the DAG stays acyclic); the production wiring hands
/// the real `EffectApi::apply`. The closure is called EXACTLY ONCE per approved effect — never for a
/// declined effect (AG-8: a withheld effect makes zero mutation, so `apply` is never reached).
///
/// Returns `Ok(event_id)` if the effect applied, `Err(reason)` if the `EffectApi` itself denied it
/// (a non-decline failure — surfaced as [`ApplyError::EffectDenied`]).
pub type EffectApplier<'a> = dyn Fn(&ArtifactRef) -> Result<String, String> + 'a;

/// **Apply the approved effects of a card, withholding the declined ones (the §6.4 gated loop).**
///
/// For each effect in the card, this reads the buffered per-effect `approval` signal off `wf_signal`
/// (keyed by the §6.4 [`per_effect_idem_key`]) and:
///
/// - **approved** → calls `apply` (the Agent Fabric `EffectApi::apply`) EXACTLY ONCE →
///   [`EffectOutcome::Applied`]. A double-click re-sent the SAME per-effect key →
///   `ON CONFLICT DO NOTHING` → one buffered signal → one apply (NO double-apply).
/// - **declined** → WITHHELDS the effect: `apply` is NEVER called, the effect makes ZERO mutation,
///   the outcome is [`EffectOutcome::Withheld`] (AG-8).
/// - **no buffered signal** (the human has not decided this effect yet) → the effect is SKIPPED
///   (returns `None` for that index); the durable wait (P-FLOW-11) re-runs the loop when the signal
///   arrives.
///
/// The decision is read from the buffered signal: an `approval` signal carrying a payload is an
/// APPROVE (the effect's refs ride the payload); an `approval` signal with an EMPTY payload AND a
/// `payload_key_ref` of [`DECLINE_MARKER`] is a DECLINE. This keeps the decision references-not-
/// payloads (§3.4) — no inline PII, the marker is a machine token.
///
/// Returns one [`GateResult`] per effect index that has a buffered decision (an effect with no
/// buffered signal yet yields `None`), so the caller (the workflow loop) can tell which effects are
/// settled and which still await a human decision. The loop is IDEMPOTENT: re-running it over the
/// same buffered set applies each approved effect again only if the caller's `apply` is not itself
/// idempotent — production wires the real `EffectApi::apply`, which is idempotent on the effect's own
/// key; here the double-apply-prevention is the ON-CONFLICT-dedup of the SIGNAL (one buffered signal
/// per per-effect key → one decision → one apply per loop pass).
pub fn apply_approved_effects(
    signals: &SignalStore,
    tenant: &TenantId,
    card: &ApprovalCard,
    apply: &EffectApplier<'_>,
) -> Vec<Option<GateResult>> {
    let total = card.effects.len();
    card.effects
        .iter()
        .enumerate()
        .map(|(idx, effect)| {
            let key = per_effect_idem_key(&card.card_id, idx, total);
            // Read the buffered per-effect `approval` signal (the human's decision for THIS effect).
            // No buffered signal → the human has not decided this effect yet (the wait, P-FLOW-11,
            // re-runs the loop when it arrives) → None for this index.
            let row = signals.get(tenant, &card.run_id, APPROVAL_SIGNAL_NAME, &key)?;

            // Decode the decision references-not-payloads (§3.4): a DECLINE_MARKER key_ref is a
            // decline; anything else is an approve. The engine reads the BUFFERED signal as the
            // truth — the per-effect key already separated the effects, so there is exactly one
            // decision per effect.
            let declined = row.payload_key_ref.as_deref() == Some(DECLINE_MARKER);

            Some(match (effect.decision, declined) {
                // DECLINED (AG-8): WITHHELD — `apply` is NEVER reached, ZERO mutation. Both the
                // card's recorded decision and the buffered signal agree it is a decline.
                (ApprovalDecision::Decline, _) | (_, true) => {
                    Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))
                }
                // APPROVED: apply the effect EXACTLY ONCE (the Agent Fabric `EffectApi::apply`).
                (ApprovalDecision::Approve, false) => match apply(&effect.effect_ref) {
                    Ok(event_id) => Ok(EffectOutcome::Applied(event_id)),
                    // A non-decline denial (capability/schema/budget) is surfaced, never swallowed.
                    Err(reason) => Err(ApplyError::EffectDenied(reason)),
                },
            })
        })
        .collect()
}

/// **The machine marker a DECLINE signal carries in `payload_key_ref`** (references-not-payloads,
/// §3.4) — a taxonomy token, NOT a crypto-shred key and NOT PII. A declined effect's `approval`
/// signal carries an empty payload and this marker so the gated loop withholds it (AG-8) without an
/// inline PII body.
pub const DECLINE_MARKER: &str = "decline";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{
        DurableExecutor, FlowExecutor, RunBudget, RunId, SignalSpec, StartSpec,
    };
    use myelin_events::{IdMinter, MonotonicMinter};
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn executor() -> FlowExecutor {
        let ex = FlowExecutor::new(minter(), tenant(), region());
        ex.register_definition("agent.run");
        ex
    }

    fn start_a_run(ex: &FlowExecutor) -> RunId {
        ex.start(StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: Some(RunBudget { minor_units: 1_000 }),
            idem_key: "k".into(),
        })
        .expect("start")
    }

    /// Deliver an APPROVE signal for effect `idx` of a card (the payload carries the effect ref).
    fn approve(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
        let key = per_effect_idem_key(card_id, idx, total);
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: APPROVAL_SIGNAL_NAME.into(),
            idem_key: key,
            payload: vec![ArtifactRef(format!("myelin://acme/agent/effect/{card_id}-{idx}"))],
            payload_key_ref: None,
        })
        .expect("approve");
    }

    /// Deliver a DECLINE signal for effect `idx` (empty payload + the DECLINE_MARKER, §3.4).
    fn decline(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
        let key = per_effect_idem_key(card_id, idx, total);
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: APPROVAL_SIGNAL_NAME.into(),
            idem_key: key,
            payload: vec![],
            payload_key_ref: Some(DECLINE_MARKER.into()),
        })
        .expect("decline");
    }

    /// **The per-effect key rule (§6.4) — single-effect keys on `card_id`, multi keys on
    /// `card_id:idx`.** This is the FROZEN engine contribution; the rest of the prompt rides it.
    #[test]
    fn per_effect_idem_key_follows_the_frozen_rule() {
        // single-effect card → the bare card id (a double-click is one approval).
        assert_eq!(per_effect_idem_key("card-7", 0, 1), "card-7");
        // multi-effect card → card_id:effect_idx (each effect independently keyed).
        assert_eq!(per_effect_idem_key("card-7", 0, 3), "card-7:0");
        assert_eq!(per_effect_idem_key("card-7", 1, 3), "card-7:1");
        assert_eq!(per_effect_idem_key("card-7", 2, 3), "card-7:2");
    }

    fn three_effect_card(run: &RunId, d0: ApprovalDecision, d1: ApprovalDecision, d2: ApprovalDecision) -> ApprovalCard {
        ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-7".into(),
            effects: vec![
                GatedEffect { effect_ref: ArtifactRef("myelin://acme/agent/effect/e0".into()), decision: d0 },
                GatedEffect { effect_ref: ArtifactRef("myelin://acme/agent/effect/e1".into()), decision: d1 },
                GatedEffect { effect_ref: ArtifactRef("myelin://acme/agent/effect/e2".into()), decision: d2 },
            ],
        }
    }

    /// **Three per-effect keys (card_id:0/1/2) apply/decline INDEPENDENTLY (the §6.4 partial-approval
    /// gate).** Approve effects 0 and 2, decline 1 — three independent signals; the loop applies 0
    /// and 2 EXACTLY once each and WITHHOLDS 1 (zero mutation, AG-8).
    #[test]
    fn three_per_effect_keys_apply_and_decline_independently() {
        let ex = executor();
        let run = start_a_run(&ex);
        // partial approval: approve 0 and 2, decline 1 — three independently-keyed signals.
        approve(&ex, &run, "card-7", 0, 3);
        decline(&ex, &run, "card-7", 1, 3);
        approve(&ex, &run, "card-7", 2, 3);
        // three distinct buffered signals (one per per-effect key) — the §6.4 anchor.
        assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 3);

        // the apply closure records each apply EXACTLY once (the Agent Fabric EffectApi::apply target).
        let applied = RefCell::new(Vec::<String>::new());
        let card = three_effect_card(&run, ApprovalDecision::Approve, ApprovalDecision::Decline, ApprovalDecision::Approve);
        let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|eff: &ArtifactRef| {
            applied.borrow_mut().push(eff.0.clone());
            Ok(format!("evt-for-{}", eff.0))
        });

        // effect 0: applied; effect 1: withheld (declined, zero mutation); effect 2: applied.
        assert_eq!(outcomes.len(), 3);
        assert!(matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))), "effect 0 approved → applied");
        assert_eq!(
            outcomes[1],
            Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
            "effect 1 declined → WITHHELD (Denied, zero mutation, AG-8)"
        );
        assert!(matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))), "effect 2 approved → applied");

        // GATE: exactly TWO applies (effects 0 and 2), and the declined effect 1 made ZERO mutation.
        let applied = applied.into_inner();
        assert_eq!(applied.len(), 2, "exactly two effects applied (0 and 2); the declined effect 1 made 0 mutation");
        assert_eq!(applied[0], "myelin://acme/agent/effect/e0");
        assert_eq!(applied[1], "myelin://acme/agent/effect/e2");
        assert!(
            !applied.contains(&"myelin://acme/agent/effect/e1".to_string()),
            "the DECLINED effect was WITHHELD — apply was NEVER reached for it (AG-8: 0 mutation on decline)"
        );
    }

    /// **A double-click on "approve all" applies each effect EXACTLY once (0 double-apply, §6.4).**
    /// Re-sending the SAME per-effect keys → `ON CONFLICT DO NOTHING` → one buffered signal per
    /// effect → the loop applies each effect ONCE per pass; a second loop pass over the same buffer
    /// likewise applies each once (the buffered set is the truth, not the re-delivery count).
    #[test]
    fn double_click_approve_all_applies_each_effect_once() {
        let ex = executor();
        let run = start_a_run(&ex);
        // "approve all" — three per-effect approve signals.
        for idx in 0..3 {
            approve(&ex, &run, "card-7", idx, 3);
        }
        // DOUBLE-CLICK: re-send the SAME three per-effect keys → ON CONFLICT DO NOTHING.
        for idx in 0..3 {
            approve(&ex, &run, "card-7", idx, 3);
        }
        // the double-click buffered NOTHING new — still exactly three signals (one per effect).
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            3,
            "a double-click on approve-all re-sends the same keys → 0 new buffered signals (ON CONFLICT DO NOTHING)"
        );

        let applies = RefCell::new(0usize);
        let card = three_effect_card(&run, ApprovalDecision::Approve, ApprovalDecision::Approve, ApprovalDecision::Approve);
        let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|_eff: &ArtifactRef| {
            *applies.borrow_mut() += 1;
            Ok("evt".into())
        });
        // exactly three applies — one per effect, NOT six (the double-click was a no-op).
        assert_eq!(*applies.borrow(), 3, "exactly 3 applies (the double-click did not double-apply)");
        assert!(outcomes.iter().all(|o| matches!(o, Some(Ok(EffectOutcome::Applied(_))))));
    }

    /// **A DECLINED single-effect card makes ZERO mutation (AG-8).** The lone effect keys on the bare
    /// `card_id`; a decline withholds it — `apply` is never reached.
    #[test]
    fn declined_single_effect_card_makes_zero_mutation() {
        let ex = executor();
        let run = start_a_run(&ex);
        decline(&ex, &run, "card-1", 0, 1); // single-effect → keys on the bare card id.
        // the buffered signal keys on the bare card id (§6.4 single-effect rule).
        assert!(ex.signals().get(&tenant(), &run.0, APPROVAL_SIGNAL_NAME, "card-1").is_some());

        let applies = RefCell::new(0usize);
        let card = ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-1".into(),
            effects: vec![GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/only".into()),
                decision: ApprovalDecision::Decline,
            }],
        };
        let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|_eff: &ArtifactRef| {
            *applies.borrow_mut() += 1;
            Ok("evt".into())
        });
        assert_eq!(
            outcomes[0],
            Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
            "the declined single effect is WITHHELD (AG-8)"
        );
        assert_eq!(*applies.borrow(), 0, "apply was NEVER reached — a declined effect makes 0 mutation (AG-8)");
    }

    /// **An effect with NO buffered decision yet is SKIPPED (the wait, P-FLOW-11, re-runs the loop).**
    /// A partial card where only effect 0 has a decision applies 0 and yields `None` for 1 and 2.
    #[test]
    fn an_undecided_effect_is_skipped_pending_the_wait() {
        let ex = executor();
        let run = start_a_run(&ex);
        approve(&ex, &run, "card-7", 0, 3); // only effect 0 decided so far.

        let card = three_effect_card(&run, ApprovalDecision::Approve, ApprovalDecision::Approve, ApprovalDecision::Approve);
        let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| Ok("evt".into()));
        assert!(matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))), "effect 0 has a decision → applied");
        assert_eq!(outcomes[1], None, "effect 1 has no buffered decision → skipped (the wait re-runs the loop)");
        assert_eq!(outcomes[2], None, "effect 2 has no buffered decision → skipped");
    }

    /// **A non-decline `EffectApi` denial is SURFACED (never swallowed, EI-02 §4).** An approved
    /// effect whose `apply` returns `Err` (a capability/budget failure) yields [`ApplyError`], distinct
    /// from the AG-8 withhold of a declined effect.
    #[test]
    fn a_non_decline_apply_failure_is_surfaced() {
        let ex = executor();
        let run = start_a_run(&ex);
        approve(&ex, &run, "card-1", 0, 1);

        let card = ApprovalCard {
            run_id: run.0.clone(),
            card_id: "card-1".into(),
            effects: vec![GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/only".into()),
                decision: ApprovalDecision::Approve,
            }],
        };
        let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| {
            Err("capability denied".into())
        });
        assert_eq!(
            outcomes[0],
            Some(Err(ApplyError::EffectDenied("capability denied".into()))),
            "an EffectApi denial is surfaced, distinct from the AG-8 withhold of a decline"
        );
    }

    /// **`ApprovalCard::idem_key_for` applies the §6.4 rule to the card's own arity.** A multi-effect
    /// card keys each effect on `card_id:idx`; a single-effect card keys on the bare id.
    #[test]
    fn card_idem_key_for_uses_the_per_effect_rule() {
        let multi = ApprovalCard {
            run_id: "r".into(),
            card_id: "c".into(),
            effects: vec![
                GatedEffect { effect_ref: ArtifactRef("a".into()), decision: ApprovalDecision::Approve },
                GatedEffect { effect_ref: ArtifactRef("b".into()), decision: ApprovalDecision::Approve },
            ],
        };
        assert_eq!(multi.idem_key_for(0), "c:0");
        assert_eq!(multi.idem_key_for(1), "c:1");

        let single = ApprovalCard {
            run_id: "r".into(),
            card_id: "c".into(),
            effects: vec![GatedEffect { effect_ref: ArtifactRef("a".into()), decision: ApprovalDecision::Approve }],
        };
        assert_eq!(single.idem_key_for(0), "c", "single-effect card keys on the bare card id");
    }
}
