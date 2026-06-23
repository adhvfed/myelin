//! # AG-P20 (→ P-347, M4) — the per-CONSUMER ToolDefs + explicit-first dispatch drills + CDC
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.1 (register
//! the Issues + Chat + CI consumer ToolDefs — OWNED) + 8.6 (explicit-first dispatch — OWNED wiring) +
//! CONSUMES 4.2 (the `CaveatContext` for the transition ABAC), 4.9 (the consumer ReBAC fragments),
//! 11.7 (reserve on the explicit run). Owning architecture: `agent-fabric.md` §6.1/§6.3 (the ONE
//! catalogue + the frozen requires_approval defaults), §5.2 step 2 (the field/transition ABAC caveat),
//! §3.4 (explicit-first dispatch — a mention notifies, does not auto-spawn).
//!
//! **Drills (the quantified gates):**
//! - **CHAT-D17** — a casual `@agent` mention → 0 auto-spawn (the dispatch-counter stays 0); the
//!   explicit run passes the reserve gate.
//! - **CHAT-D9 / CHAT-D10** — across a Chat+Workflow kill, a gated tool runs EXACTLY ONCE, a
//!   double-click is one approval; a batch 2-of-3 approval applies per-effect (the withheld never
//!   mutates).
//! - **ISS-D12** — an agent hitting a governed (SLA-bound, approver-edged) transition is HITL-gated,
//!   withheld, no mutation until approval.
//! - The consumer ToolDefs carry their frozen §6.3 requires_approval defaults (CI deploy/secret = yes,
//!   run_pipeline non-prod = no; Issues advisory = no, SLA transition gated; Chat post/react = no).
//!
//! These pair the NEW consumer ToolDefs (the AG-P20 registration data) + the NEW explicit-first
//! dispatch classifier with the REAL `PlanThenApply` pipeline (AG-P6), the REAL batch HITL loop
//! (AG-P10), and the frozen §6.3 defaults (AG-P8) — NO new engine, the registration lights up the
//! existing path.

use myelin_agent::GateId;
use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    // the NEW AG-P20 consumer surfaces:
    chat_tool_defs,
    ci_tool_defs,
    classify,
    deploy_tool_def,
    forecast_tool_def,
    issues_tool_defs,
    landing_requires_approval,
    post_message_tool_def,
    react_tool_def,
    register_chat_tools,
    register_ci_tools,
    register_issues_tools,
    run_pipeline_tool_def,
    transition_caveat,
    transition_tool_def,
    // the REUSED engine surfaces:
    ApplyError,
    ApplyLedger,
    ApprovedTools,
    BatchApprovalCard,
    BatchGatedEffect,
    CapabilityCheck,
    DecisionScript,
    DelegationLookup,
    DispatchCounter,
    DispatchDecision,
    DispatchTrigger,
    EffectBudget,
    EffectCost,
    PipelineSignals,
    PlanThenApply,
    PlannedEffect,
    RiskSummary,
    SubsystemApply,
    TenantGuard,
    WaitDecision,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

// ───────────────────────── the REAL consumed pipeline seams ─────────────────────────────

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

/// A `check` provider keyed on the cap STRING; an SLA-bound transition (a caveat carrying a
/// `transition`) with NO approver attr resolves `Conditional` (treated as a DENY — fail-closed) so an
/// un-approved governed transition can never silently apply. This mirrors the EffectApi CDC's OQ-E leg.
struct CheckProvider {
    allow: BTreeSet<String>,
    /// when true, an SLA-bound transition caveat resolves Conditional (no approver context → deny).
    transition_needs_approver: bool,
}
impl CapabilityCheck for CheckProvider {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision {
        let sla_bound = self.transition_needs_approver
            && caveat.map(|c| c.transition.is_some()).unwrap_or(false);
        if sla_bound {
            // the OQ-E leg: a governed transition with no approver context → Conditional (≡ deny).
            Decision::Conditional
        } else if self.allow.contains(&permission.0) {
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
        EffectivePolicy {
            caveats: self.caps.clone(),
        }
    }
}

struct PermitAll;
impl TenantGuard for PermitAll {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// The ONLY mutation path — records every apply so a test can assert 0 mutation before approval.
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
        self.applied.borrow_mut().push(tool.0.clone());
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

fn agent() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("mock".into()),
            on_behalf_of: None,
        },
        TenantId("acme".into()),
    )
}
fn human() -> Principal {
    Principal::stub(
        PrincipalId("psn:human-x".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// Run the apply pipeline once for `plan` under the given catalogue, `approved` set, and check
/// provider; returns the result + the total mutations recorded by the endpoint after the call.
fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    check: &CheckProvider,
    caps: Vec<String>,
    approved: BTreeSet<String>,
    plan: &PlannedEffect,
) -> (EffectResult, usize) {
    let del = Delegate { caps };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 10_000 };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: cat,
        check,
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
    let out = p.apply_planned(plan);
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

// ═════════════════════════ the §6.3 frozen-default gate (8.1) ═════════════════════════════════════

/// **GATE — the consumer ToolDefs carry their frozen §6.3 requires_approval defaults.** Issues
/// advisory = no, SLA transition = gated; Chat post/react = no; CI deploy/approve/secret = yes,
/// run_pipeline (non-prod) = no. This is the registered-catalogue assertion (8.1 / §6.3).
#[test]
fn consumer_tooldefs_carry_their_frozen_6_3_defaults() {
    // Issues: forecast/triage/sla_draft advisory (NOT gated); transition gated.
    let issues = issues_tool_defs();
    let gated_issues: Vec<&str> = issues
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(
        gated_issues,
        vec!["transition"],
        "only the SLA-bound transition is gated; forecast/triage/sla_draft are advisory"
    );

    // Chat: post_message + react both reversible (NOT gated).
    assert!(chat_tool_defs().iter().all(|d| !d.requires_approval));

    // CI: deploy/approve_deploy/write_secret gated; run_pipeline (non-prod) NOT gated.
    let ci = ci_tool_defs();
    let gated_ci: Vec<&str> = ci
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(
        gated_ci,
        vec!["deploy", "approve_deploy", "write_secret"],
        "the three privileged CI gates; run_pipeline (non-prod) is not gated"
    );
    assert!(!run_pipeline_tool_def().requires_approval);
}

/// **CDC 8.1 / 4.9 — all three consumer surfaces register into the ONE ToolSurface and resolve by
/// name with their FROZEN ReBAC-fragment caps.** The registration is the whole deliverable (a ToolDef
/// is a row in the ONE registry, no second governance model).
#[test]
fn cdc_8_1_4_9_all_consumer_tools_register_into_the_one_surface() {
    let mut cat = Catalogue { defs: vec![] };
    let issues = register_issues_tools(&mut cat).expect("issues seeded defs admit");
    let chat = register_chat_tools(&mut cat).expect("chat seeded defs admit");
    let ci = register_ci_tools(&mut cat).expect("ci seeded defs admit");
    assert_eq!(issues.len() + chat.len() + ci.len(), 4 + 2 + 4);

    // the caps are the frozen 4.9 fragment permissions (a rename in any fragment breaks this).
    assert_eq!(
        cat.resolve(&ToolName("transition".into()))
            .unwrap()
            .required_caps,
        vec!["issue_transition.perform_transition".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("post_message".into()))
            .unwrap()
            .required_caps,
        vec!["channel.post".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("deploy".into()))
            .unwrap()
            .required_caps,
        vec!["environment.deploy".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("write_secret".into()))
            .unwrap()
            .required_caps,
        vec!["ci_project.administer".to_string()]
    );
}

/// **The cross-subsystem rule (§6.3 last row) — a Chat-invoked effect is governed where it LANDS.** A
/// chat-invoked `ci.deploy` inherits CI's GATED default; a chat-invoked `issues.forecast` inherits
/// Issues' advisory (NOT gated) default.
#[test]
fn chat_invoked_effect_is_governed_where_it_lands() {
    assert!(
        landing_requires_approval("ci", "deploy"),
        "a chat-invoked ci.deploy lands in ci → gated"
    );
    assert!(
        landing_requires_approval("issues", "transition"),
        "a chat-invoked issues.transition lands in issues → gated (the SLA floor)"
    );
    assert!(
        !landing_requires_approval("issues", "forecast"),
        "a chat-invoked issues.forecast lands in issues → advisory (NOT gated)"
    );
}

// ═════════════════════════ CHAT-D17 — explicit-first dispatch (8.6 / §3.4) ════════════════════════

/// **CHAT-D17 (GATE) — a casual `@agent` mention → 0 auto-spawn; the explicit run dispatches (and
/// passes the reserve gate downstream).** The dispatch-counter STAYS 0 across casual chatter; only the
/// explicit "run an agent here" trigger spawns a costed run.
#[test]
fn chat_d17_casual_mention_zero_spawn_explicit_run_dispatches() {
    let mut counter = DispatchCounter::new();
    // a stream of casual mentions (the typical "@agent can you look at this?").
    for i in 0..12 {
        let d = counter.dispatch(&DispatchTrigger::Mention(format!("msg/{i}")));
        assert!(d.notifies(), "each casual mention NOTIFIES (0 spawn)");
    }
    assert_eq!(
        counter.auto_spawns(),
        0,
        "CHAT-D17: 0 auto-spawn on casual mentions (the dispatch-counter stays 0)"
    );

    // an EXPLICIT trigger on the SAME counter DISPATCHES exactly one costed run.
    let explicit = counter.dispatch(&DispatchTrigger::ExplicitRun("run-req/1".into()));
    assert_eq!(
        explicit,
        DispatchDecision::Dispatch("run-req/1".into()),
        "the explicit run dispatches (and the costed run passes reserve downstream, 11.7)"
    );
    assert_eq!(
        counter.auto_spawns(),
        1,
        "exactly ONE costed run was dispatched — the explicit one (the twelve mentions spawned 0)"
    );
}

/// **The L-3 floor is structural — no `DispatchTrigger::Mention` maps to `Dispatch`.** Implicit
/// auto-dispatch on a casual mention is NOT wired (counsel-gated, GDPR Art. 22 / EU AI-Act). A
/// hand-edit that wired a mention to dispatch would flip this assertion.
#[test]
fn the_l3_auto_dispatch_floor_is_structural() {
    for r in ["a", "@agent ship it", "please run the agent"] {
        assert!(
            !classify(&DispatchTrigger::Mention(r.into())).dispatches(),
            "a casual mention can NEVER dispatch (the L-3 floor): {r}"
        );
    }
}

// ═════════════════════════ ISS-D12 — the SLA-bound governed transition ════════════════════════════

/// **ISS-D12 (GATE) — an agent hitting a governed (SLA-bound, approver-edged) transition is
/// HITL-gated, WITHHELD, no mutation until approval.** The frozen §6.3 `transition` default is gated;
/// the apply pipeline WITHHOLDS at step 6 (returns `Gated`, 0 mutation); only after an approval threads
/// the tool into `approved` does a re-run APPLY — exactly once.
#[test]
fn iss_d12_governed_transition_withheld_then_approved_applies_once() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    // the agent holds the perform_transition cap (so the gate, not a deny, is what withholds).
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        // for THIS leg the approver context is present (the gate is the §6.3 default, not the caveat).
        transition_needs_approver: false,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];

    let object = ArtifactRef("myelin://acme/issues/issue/PROJ-1".into());
    let caveat = transition_caveat(object.clone(), "issue:PROJ-1:open->done");
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"PROJ-1","to_state":"done"}"#.into(),
        field: None,
        transition: caveat.transition.clone(),
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };

    // (1) WITHHOLD — the run is NOT approved → step 6 gates → `Gated`, 0 mutation (AG-8).
    let (withheld, muts0) = apply_once(
        &cat,
        &endpoint,
        &check,
        caps.clone(),
        BTreeSet::new(), // empty approved set → the gated tool withholds.
        &plan,
    );
    assert!(
        matches!(withheld, EffectResult::Gated(_)),
        "the governed transition is WITHHELD (Gated), never applied: {withheld:?}"
    );
    assert_eq!(muts0, 0, "ISS-D12: 0 mutation before approval (AG-8)");

    // (2) APPROVE → re-run with the tool threaded into `approved` → APPLIES exactly once.
    let approved: BTreeSet<String> = ["transition".to_string()].into_iter().collect();
    let (applied, muts1) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(applied, EffectResult::Applied(_)),
        "after approval the transition APPLIES: {applied:?}"
    );
    assert_eq!(muts1, 1, "ISS-D12: exactly one apply after approval");
}

/// **ISS-D12 (the ABAC leg, §5.2 step 2 / 4.2) — an SLA-bound transition with NO approver context
/// resolves `Conditional` → DENY, never a silent apply.** The transition-ABAC caveat is fail-closed:
/// a `Conditional` (a caveat needing missing approver context) is a DENY, not a silent allow (the
/// caveat NEVER loosens the gated floor).
#[test]
fn iss_d12_sla_bound_transition_without_approver_context_is_denied() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    // the cap would be allowed in general, BUT an SLA-bound transition caveat → Conditional (deny).
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        transition_needs_approver: true,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];
    let object = ArtifactRef("myelin://acme/issues/issue/PROJ-9".into());
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"PROJ-9","to_state":"done"}"#.into(),
        field: None,
        // an SLA-bound transition — the caveat carries the transition the approver-edge ABAC gates.
        transition: transition_caveat(object, "issue:PROJ-9:open->done").transition,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };
    // even with the tool "approved", the ABAC caveat (Conditional) denies — fail-closed, never applies.
    let approved: BTreeSet<String> = ["transition".to_string()].into_iter().collect();
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(out, EffectResult::Denied(_)),
        "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
    );
    assert_eq!(muts, 0, "a denied governed transition makes 0 mutation");
}

// ═════════════════════════ CHAT-D9 / CHAT-D10 — exactly-once across a kill ════════════════════════

fn gated_effect(tool: &str, idx: u32, gate: &str) -> BatchGatedEffect {
    let object = ArtifactRef(format!("myelin://acme/ci/deploy/{idx}"));
    BatchGatedEffect {
        gate_id: GateId(gate.into()),
        plan: PlannedEffect {
            tool: ToolName(tool.into()),
            object: object.clone(),
            input_json: r#"{"environment":"prod"}"#.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "ci.deploy",
                wholesale: 40,
                markup: 10,
            },
        },
        risk_summary: RiskSummary::for_action("agent.hitl.ci_deploy", &object),
    }
}

/// A three-effect batch card gating three CI `deploy`s (each gated by the frozen §6.3 default).
fn three_deploy_card() -> BatchApprovalCard {
    BatchApprovalCard {
        run_id: "run-1".into(),
        card_id: "card-1".into(),
        effects: vec![
            gated_effect("deploy", 0, "g0"),
            gated_effect("deploy", 1, "g1"),
            gated_effect("deploy", 2, "g2"),
        ],
        approver_filter: vec![PrincipalId("psn:lead".into())],
    }
}

/// **CHAT-D9 / CHAT-D10 (GATE) — a batch 2-of-3 approval applies PER-EFFECT; the withheld never
/// mutates; a double-click (re-drive on the SAME ledger) is one approval (no double-apply).** Approve
/// effects 0 and 2, decline 1: exactly two applies, the declined effect makes 0 mutation, and a
/// re-drive (the double-click / the Chat+Workflow kill-and-replay) under the same per-effect keys adds
/// 0 new applies.
#[test]
fn chat_d9_d10_batch_partial_approval_exactly_once_across_a_redrive() {
    let card = three_deploy_card();
    // approve 0 and 2, DECLINE 1 (the partial approval) — keyed per-effect.
    let mut script = DecisionScript::new();
    script
        .decide(card.idem_key_for(0), WaitDecision::Approve)
        .decide(card.idem_key_for(1), WaitDecision::Reject("not now".into()))
        .decide(card.idem_key_for(2), WaitDecision::Approve);

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();

    // FIRST drive — the partial approval.
    let out = myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        out.approved_effect_count(),
        2,
        "exactly two of three effects were approved (0 and 2); 1 was withheld"
    );
    assert_eq!(
        out.ledger.applies(),
        2,
        "CHAT-D9/D10: exactly two applies (the approved effects)"
    );
    assert!(
        out.exactly_once(),
        "the apply-counter equals the approved-effect count exactly (AG-D5)"
    );
    // the declined effect (idx 1) is WITHHELD — never applied (AG-8).
    assert!(
        !out.effects[1].applied(),
        "the declined effect makes 0 mutation (AG-8)"
    );

    // SECOND drive — the double-click / the kill-and-replay: the SAME per-effect keys are re-sent on
    // the SAME ledger. The ledger dedups → 0 new applies (a double-click is one approval).
    let before = ledger.applies();
    let out2 =
        myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        ledger.applies(),
        before,
        "CHAT-D9/D10: the double-click / kill-replay re-sends the same keys → 0 new applies"
    );
    assert!(
        out2.exactly_once(),
        "still exactly-once after the re-drive (no double-apply)"
    );
}

/// **A single-effect gated CI deploy run survives a Chat+Workflow kill EXACTLY ONCE (CHAT-D9).** The
/// first drive approves + applies; a re-drive (the kill-and-replay) under the same key adds 0 applies
/// (the per-effect ledger is the exactly-once binding — the run resumes from the durable wait, never
/// re-applies).
#[test]
fn chat_d9_single_gated_deploy_survives_a_kill_exactly_once() {
    let card = BatchApprovalCard {
        run_id: "run-2".into(),
        card_id: "card-2".into(),
        effects: vec![gated_effect("deploy", 7, "g7")],
        approver_filter: vec![PrincipalId("psn:lead".into())],
    };
    let mut script = DecisionScript::new();
    // a single-effect card keys on the bare card_id (the degenerate per-effect rule).
    script.decide(card.idem_key_for(0), WaitDecision::Approve);

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();

    myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(ledger.applies(), 1, "the approved deploy applies once");

    // the kill-and-replay: the run re-drives on the same ledger → the same key → 0 new apply.
    myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        ledger.applies(),
        1,
        "CHAT-D9: across the Chat+Workflow kill the gated deploy runs EXACTLY ONCE"
    );
}

// ═════════════════════════ explicit-first applies advisory tools directly (8.6 / 8.2) ═════════════

/// **An Issues advisory tool (forecast) is NOT gated — it applies DIRECTLY through the pipeline
/// (suggest-by-default).** No HITL gate, one apply (the advisory suggestion is recorded, cap-checked,
/// metered — but not withheld). Pairs the NEW advisory ToolDef with the REAL pipeline.
#[test]
fn issues_advisory_forecast_applies_directly_no_gate() {
    let cat = Catalogue {
        defs: vec![forecast_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue.transition".to_string()].into_iter().collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["issue.transition".to_string()];
    let plan = PlannedEffect {
        tool: ToolName("forecast".into()),
        object: ArtifactRef("myelin://acme/issues/issue/PROJ-2".into()),
        input_json: r#"{"issue":"PROJ-2","horizon_days":14}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "issue.forecast",
            wholesale: 2,
            markup: 1,
        },
    };
    // an advisory tool is not gated → it applies directly even with an EMPTY approved set.
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Applied(_)),
        "an advisory forecast applies directly (suggest-by-default, no gate): {out:?}"
    );
    assert_eq!(
        muts, 1,
        "exactly one apply (no withhold for the advisory tool)"
    );
}

/// **Chat post_message + react are NOT gated — they apply DIRECTLY (reversible/cheap).** The frozen
/// §6.3 defaults make both un-gated; the registration is data on the existing path.
#[test]
fn chat_post_and_react_apply_directly_no_gate() {
    for tool in [post_message_tool_def(), react_tool_def()] {
        assert!(
            !tool.requires_approval,
            "chat.{} is reversible → NOT gated",
            tool.name.0
        );
        assert_eq!(tool.effect_kind, EffectKind::Mutate);
    }
}

/// **A CI deploy is the gated consequential tool — it WITHHOLDS (0 mutation) until approval.** Pairs
/// the NEW CI deploy ToolDef with the REAL pipeline: an empty approved set → `Gated`, 0 mutation.
#[test]
fn ci_deploy_is_withheld_until_approval() {
    let cat = Catalogue {
        defs: vec![deploy_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["environment.deploy".to_string()].into_iter().collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["environment.deploy".to_string()];
    let plan = PlannedEffect {
        tool: ToolName("deploy".into()),
        object: ArtifactRef("myelin://acme/ci/environment/prod".into()),
        input_json: r#"{"environment":"prod","artifact":"build-9"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "ci.deploy",
            wholesale: 40,
            markup: 10,
        },
    };
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Gated(_)),
        "ci.deploy WITHHOLDS until approval (the frozen §6.3 gate): {out:?}"
    );
    assert_eq!(muts, 0, "0 mutation before the deploy approval (AG-8)");
}
