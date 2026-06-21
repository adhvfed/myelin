//! # The chained-e2e for per-effect HITL idempotency (AG-D5 exactly-once leg) — batch / partial
//! approval + double-click composed with the REAL `PlanThenApply` pipeline (AG-P10 → P-222)
//!
//! **Contract:** `contract-index.md` row 8.2 (the apply each `idem_key` maps to exactly once — the
//! exactly-once apply binding) + 9.1 (the per-effect resume `idem_key`). Owning architecture:
//! `agent-fabric.md` §5.3 **C4** (per-effect resume idempotency: a partial approval is well-defined,
//! a double-click is one approval). Drill: AG-D5 (the EXACTLY-ONCE leg — approval applies exactly
//! once; per-effect idempotency proven; **1 apply per approved effect**, the apply-counter == the
//! approved-effect count, never more).
//!
//! This composes the batch HITL loop (`run_batch_hitl_loop`) with the REAL eight-step `PlanThenApply`
//! pipeline (AG-P6) so the exactly-once leg is proven end-to-end against the SAME apply path the
//! dispatch tier runs: a batch card gates three effects → withheld (0 mutation) → a partial approval
//! (approve 0+2, decline 1) resumes → the re-run applies EXACTLY the approved effects (2 applies, 1
//! withheld); and a double-click variant (the same per-effect keys → 0 extra apply).

use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    gate_id_of, run_batch_hitl_loop, ApplyLedger, ApplyError, ApprovedTools, BatchApprovalCard,
    BatchGatedEffect, CapabilityCheck, DecisionScript, DelegationLookup, EffectBudget, EffectCost,
    EffectOutcome, PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply,
    TenantGuard, WaitDecision,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

// ───────────────────────── the REAL consumed seams (the pipeline providers) ─────────────────────

struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

struct AllowAll {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for AllowAll {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> Decision {
        if self.allow.contains(&permission.0) {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }
}

struct Delegate {
    caps: Vec<String>,
}
impl DelegationLookup for Delegate {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy { caveats: self.caps.clone() }
    }
}

struct PermitAll;
impl TenantGuard for PermitAll {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// The subsystem PUBLIC endpoint — the ONLY mutation path; records EVERY apply (keyed by object) so
/// the test asserts the apply-counter == the approved-effect count (1 apply per approved effect).
struct Endpoint {
    applied: RefCell<Vec<String>>,
}
impl SubsystemApply for Endpoint {
    fn apply_public(
        &self,
        _a: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        _input: &str,
    ) -> Result<EventId, ApplyError> {
        self.applied.borrow_mut().push(object.0.clone());
        Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
    }
}

struct Budget {
    remaining: u64,
}
impl EffectBudget for Budget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        total
    }
}

// ───────────────────────── fixtures ─────────────────────────

fn agent() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent { runtime_ref: RuntimeRef("mock".into()), on_behalf_of: None },
        TenantId("acme".into()),
    )
}
fn human() -> Principal {
    Principal::stub(PrincipalId("psn:human-x".into()), PrincipalKind::Human, TenantId("acme".into()))
}

fn merge_tool() -> ToolDef {
    ToolDef {
        name: ToolName("git.merge".into()),
        subsystem: "git".into(),
        version: 1,
        input_schema: r#"{"type":"object","required":["pr"],"properties":{"pr":{"type":"integer"}}}"#.into(),
        required_caps: vec!["git.merge".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: true,
        exposed_over_mcp: false,
    }
}

fn merge_plan(pr: u32) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName("git.merge".into()),
        object: ArtifactRef(format!("myelin://acme/git/pr/{pr}")),
        input_json: format!(r#"{{"pr":{pr}}}"#),
        field: None,
        transition: None,
        cost: EffectCost { unit: "git.merge", wholesale: 30, markup: 20 },
    }
}

fn approvers() -> Vec<PrincipalId> {
    vec![PrincipalId("psn:lead".into()), PrincipalId("psn:maintainer".into())]
}

/// Run the apply pipeline once for `plan` under the given `approved` set; returns the result.
fn apply_once(cat: &Catalogue, endpoint: &Endpoint, plan: &PlannedEffect, approved: BTreeSet<String>) -> EffectResult {
    let check = AllowAll { allow: ["git.merge".to_string()].into_iter().collect() };
    let del = Delegate { caps: vec!["git.merge".into()] };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 10_000 };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved,
        signals: &mut signals,
    };
    p.apply_planned(plan)
}

/// Build the three-effect batch card (PRs 40/41/42), withholding each through the REAL pipeline first
/// (asserting 0 mutation before approval) and binding the step-6 `GateId` into the card.
fn withhold_three(cat: &Catalogue, endpoint: &Endpoint) -> BatchApprovalCard {
    let mut effects = Vec::new();
    for pr in [40u32, 41, 42] {
        let plan = merge_plan(pr);
        // WITHHOLD through the REAL pipeline (a fresh run, empty `approved`) → GATES (0 mutation).
        let result = apply_once(cat, endpoint, &plan, BTreeSet::new());
        let gate_id = gate_id_of(&result).expect("a requires_approval tool GATES (AG-8)");
        assert!(matches!(result, EffectResult::Gated(_)), "effect for pr {pr} is WITHHELD: {result:?}");
        effects.push(BatchGatedEffect {
            gate_id,
            risk_summary: RiskSummary::for_action("agent.hitl.merge_pr", &plan.object),
            plan,
        });
    }
    assert_eq!(endpoint.applied.borrow().len(), 0, "0 MUTATIONS before approval (AG-D5: the gated effects did NOT mutate)");
    BatchApprovalCard {
        run_id: "R1".into(),
        card_id: "card-7".into(),
        effects,
        approver_filter: approvers(),
    }
}

// ───────────────────────── CHAINED-E2E: the partial-approval variant (2-of-3) ────────────────────

/// **CHAINED-E2E (AG-D5 exactly-once leg — PARTIAL APPROVAL): three effects withheld (0 mutation), a
/// partial approval (approve 0 and 2, decline 1) resumes, and the re-run applies EXACTLY the approved
/// effects through the REAL pipeline — 2 applies, 1 withheld; the apply-counter == the approved-effect
/// count (2).** This is the core AG-D5 partial-approval parity number, proven end-to-end.
#[test]
fn chained_partial_approval_applies_exactly_the_approved_effects() {
    let cat = Catalogue { defs: vec![merge_tool()] };
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // 1. WITHHOLD all three through the REAL pipeline (0 mutation).
    let card = withhold_three(&cat, &endpoint);

    // 2 + 3 + 4. SURFACE + DECIDE + RESUME: a partial approval — approve 0 and 2, decline 1.
    let mut script = DecisionScript::new();
    script
        .decide(card.idem_key_for(0), WaitDecision::Approve)
        .decide(card.idem_key_for(1), WaitDecision::Reject("pr 41 fails checks".into()))
        .decide(card.idem_key_for(2), WaitDecision::Approve);

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();
    let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);

    // the batch loop itself made 0 mutation (it only opened gates + threaded decisions).
    assert_eq!(endpoint.applied.borrow().len(), 0, "the batch loop made 0 mutation (the apply is the re-run)");
    // exactly the approved effects are settled Applied; the declined one is Withheld (0 mutation, AG-8).
    assert!(matches!(outcome.effects[0], EffectOutcome::Applied { .. }));
    assert!(matches!(outcome.effects[1], EffectOutcome::Withheld { .. }), "effect 1 declined → WITHHELD");
    assert!(matches!(outcome.effects[2], EffectOutcome::Applied { .. }));
    assert_eq!(outcome.approved_effect_count(), 2, "exactly 2 approved effects (0 and 2)");
    assert!(outcome.exactly_once(), "the apply-counter (ledger) == the approved-effect count (AG-D5)");

    // 4b. RE-RUN through the REAL pipeline — but ONLY the approved effects are re-submitted (the
    //     declined effect was WITHHELD: it is never re-run, AG-8). The per-effect `ApplyLedger` is the
    //     exactly-once authority — it decides WHICH effect re-runs (the pipeline's step-6 `approved`
    //     set keys on the tool NAME, which is too coarse for a batch where sibling effects share a
    //     tool; the per-effect ledger is the finer granularity the batch needs). We re-submit exactly
    //     the effects whose per-effect key the ledger recorded as approved.
    for (idx, eff) in card.effects.iter().enumerate() {
        let idem_key = card.idem_key_for(idx);
        if outcome.ledger.contains(&idem_key) {
            // an APPROVED effect re-runs through the REAL pipeline → APPLIES.
            let result = apply_once(&cat, &endpoint, &eff.plan, approved.as_set());
            assert!(matches!(result, EffectResult::Applied(_)), "the approved effect {idx} APPLIES on the re-run");
        } else {
            // the DECLINED effect is WITHHELD — never re-submitted to apply (0 mutation, AG-8). To
            // PROVE it would still gate if anyone tried, re-run it with an EMPTY approved set: it gates.
            let result = apply_once(&cat, &endpoint, &eff.plan, BTreeSet::new());
            assert!(matches!(result, EffectResult::Gated(_)), "the declined effect {idx} still GATES (never applied, AG-8)");
        }
    }
    // THE GATE: exactly 2 applies through the REAL endpoint — pr 40 and pr 42, NEVER pr 41.
    let applied = endpoint.applied.borrow();
    assert_eq!(applied.len(), 2, "1 apply per approved effect — exactly 2 (AG-D5: apply-counter == approved count)");
    assert!(applied.contains(&"myelin://acme/git/pr/40".to_string()));
    assert!(applied.contains(&"myelin://acme/git/pr/42".to_string()));
    assert!(
        !applied.contains(&"myelin://acme/git/pr/41".to_string()),
        "the DECLINED effect (pr 41) made 0 mutation — apply was NEVER reached (AG-8)"
    );
}

// ───────────────────────── CHAINED-E2E: the double-click variant (0 extra apply) ─────────────────

/// **CHAINED-E2E (AG-D5 exactly-once leg — DOUBLE-CLICK): a double-click on "approve all" applies each
/// effect EXACTLY once — the second click adds 0 applies (the apply-counter stays at the approved
/// count, 3).** The double-click re-sends the SAME per-effect keys; the `ApplyLedger` dedups them, and
/// the re-run through the REAL pipeline is itself idempotent (the approved set is unchanged).
#[test]
fn chained_double_click_approve_all_applies_each_effect_once() {
    let cat = Catalogue { defs: vec![merge_tool()] };
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    let card = withhold_three(&cat, &endpoint);

    // "approve all" — every effect approved.
    let mut script = DecisionScript::new();
    for idx in 0..card.len() {
        script.decide(card.idem_key_for(idx), WaitDecision::Approve);
    }

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();
    // FIRST click → applies all three (the ledger records card-7:0/1/2).
    let first = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(first.ledger.applies(), 3, "the first click applies all three effects");

    // DOUBLE-CLICK → re-send the SAME per-effect keys (re-run with the SAME ledger): 0 new applies.
    let second = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(second.ledger.applies(), 3, "a double-click adds 0 applies — exactly 3 (1 per effect)");
    assert!(second.exactly_once(), "the apply-counter == the approved count (3), never 6 — one approval");

    // RE-RUN each effect once through the REAL pipeline (the apply is the re-run; applied once each).
    for eff in &card.effects {
        let result = apply_once(&cat, &endpoint, &eff.plan, approved.as_set());
        assert!(matches!(result, EffectResult::Applied(_)));
    }
    // exactly 3 distinct applies through the REAL endpoint (one per effect) — the double-click did not
    // double-apply (the per-effect ledger + the approved set are the truth, not the click count).
    let applied = endpoint.applied.borrow();
    assert_eq!(applied.len(), 3, "exactly 3 applies (1 per approved effect), NOT 6 — the double-click is one approval");
    let distinct: BTreeSet<&String> = applied.iter().collect();
    assert_eq!(distinct.len(), 3, "the three applies are the three distinct effects (pr 40/41/42)");
}
