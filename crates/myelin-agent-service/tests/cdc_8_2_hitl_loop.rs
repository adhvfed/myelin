//! # The chained-e2e for the HITL withhold → surface → resume loop + the consumer CDCs for 9.4 / 4.4
//! (AG-P9 → P-221)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.2 (the
//! HITL GATE step completion — the `hitl_gate` state machine the `Gated` verdict resumes) +
//! CONSUMES 9.4 (the durable HITL signal — `state=waiting` holds no runtime; an approval/cancel
//! arrives hours/days later, re-leases + replays + consumes) + 4.4 (`list_subjects → SubjectTree`,
//! the approver set). Owning architecture: `agent-fabric.md` §5.3 (withhold → surface → resume) +
//! §4.4 (the `hitl_gate` table). Drill: AG-D5 (withhold/resume leg) — a gated tool is withheld
//! (returns an error, does NOT mutate); the card shows action + risk + cost; approval resumes and
//! applies; rejection halts; **0 mutations before approval**.
//!
//! This pairs the HITL machinery (this prompt) with the REAL `PlanThenApply` pipeline (AG-P6) so the
//! chained loop is proven end-to-end against the same eight-step body the dispatch tier runs: a mock
//! run proposes a gated effect → withheld (assert 0 mutation) → the workflow parks → an approval
//! signal arrives → resume (the tool is threaded into `approved`) → a re-run APPLIES; then a
//! rejection variant halts with the reason in the trace (0 mutation).

use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    derive_approver_set, gate_id_of, run_hitl_loop, surface_card, ApplyError, ApprovedTools,
    ApproverSet, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, Halted, HitlGate,
    HitlGateState, HitlOutcome, HitlWait, PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary,
    SubsystemApply, TenantGuard, WaitDecision,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, EffectivePolicy, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RuntimeRef, SubjectTree, Zookie,
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

/// The subsystem PUBLIC endpoint — the ONLY mutation path; records EVERY apply so the test can
/// assert 0 mutation before approval.
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
    settles: u64,
}
impl EffectBudget for Budget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        self.settles += 1;
        total
    }
}

// ───────────────────────── PROVIDER side: 9.4 the durable HITL wait ─────────────────────────

/// **A REAL provider on the contract-9.4 durable HITL wait surface (the durable-workflow side).** It
/// models the park-and-resume: it returns the scripted decision the human made days later (Approve /
/// Reject / Expired). The CONSUMER is the agent fabric's [`run_hitl_loop`] threading the decision into
/// the `hitl_gate` state machine. (The real `myelin-flow::approval::request_approval_and_wait` is the
/// production provider; this scripted one proves the consumer drives the three decisions correctly.)
struct ScriptedWait {
    decision: WaitDecision,
    parked: RefCell<Vec<String>>,
}
impl HitlWait for ScriptedWait {
    fn park_and_wait(&self, gate: &HitlGate) -> WaitDecision {
        // record that the run PARKED on this gate (state=waiting holds no runtime) before the human
        // decided — the wait is the durability seam; the worker is free while parked.
        self.parked.borrow_mut().push(gate.card_ref.clone());
        self.decision.clone()
    }
}

// ───────────────────────── PROVIDER side: 4.4 list_subjects (Identity) ─────────────────────────

/// **A REAL provider on the contract-4.4 `list_subjects` surface (the Identity side).** Returns the
/// approver userset for the gated object at the zookie snapshot. The CONSUMER ([`derive_approver_set`])
/// reads its `members` as the `hitl_gate.approver_filter`.
struct ProviderSubjects {
    members: Vec<PrincipalId>,
}
impl ApproverSet for ProviderSubjects {
    fn list_subjects(
        &self,
        object: &ArtifactRef,
        approve_perm: &Permission,
        at: &Consistency,
    ) -> SubjectTree {
        SubjectTree {
            object: ObjectId(object.0.clone()),
            relation: RelName(approve_perm.0.clone()),
            members: self.members.clone(),
            zookie: at.at_least.clone(),
        }
    }
}

// ───────────────────────── fixtures ─────────────────────────

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

fn merge_tool() -> ToolDef {
    ToolDef {
        name: ToolName("git.merge".into()),
        subsystem: "git".into(),
        version: 1,
        input_schema: r#"{"type":"object","required":["pr"],"properties":{"pr":{"type":"integer"}}}"#.into(),
        required_caps: vec!["git.merge".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // the frozen §6.3 default: git.merge requires_approval = yes.
        requires_approval: true,
        exposed_over_mcp: false,
    }
}

fn merge_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName("git.merge".into()),
        object: ArtifactRef("myelin://acme/git/pr/42".into()),
        input_json: r#"{"pr":42}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost { unit: "git.merge", wholesale: 30, markup: 20 },
    }
}

fn approvers() -> Vec<PrincipalId> {
    vec![PrincipalId("psn:lead".into()), PrincipalId("psn:maintainer".into())]
}

fn strong(z: &str) -> Consistency {
    Consistency { at_least: Zookie(z.into()), mode: ConsistencyMode::Strong }
}

/// Run the apply pipeline once for `plan` under the given `approved` set; returns the result + the
/// total mutations recorded by the endpoint after the call.
fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowAll { allow: ["git.merge".to_string()].into_iter().collect() };
    let del = Delegate { caps: vec!["git.merge".into()] };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 1_000, settles: 0 };
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
    let out = p.apply_planned(&merge_plan());
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

// ───────────────────────── the chained-e2e: withhold (0 mutation) → resume → apply ───────────────

/// **CHAINED-E2E (AG-D5 withhold/resume leg): a gated effect is WITHHELD (0 mutation), the run PARKS
/// on the durable wait, an APPROVAL arrives days later, the resume threads the tool into `approved`,
/// and a re-run APPLIES — exactly once.** This is the full withhold → surface → resume loop composed
/// with the REAL eight-step pipeline.
#[test]
fn chained_withhold_zero_mutation_then_approve_resumes_and_applies() {
    let cat = Catalogue { defs: vec![merge_tool()] };
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // 1. WITHHOLD: a fresh run (empty `approved`) proposes the gated merge → the pipeline GATES it.
    let (result, muts_before) = apply_once(&cat, &endpoint, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("a requires_approval tool GATES (AG-8)");
    assert!(matches!(result, EffectResult::Gated(_)), "the gated effect is WITHHELD: {result:?}");
    assert_eq!(muts_before, 0, "0 MUTATIONS before approval (AG-D5 — the gated tool did NOT mutate)");

    // surface the card from the gate (action + risk + LIVE cost + approver set) — what the human sees.
    let subjects = ProviderSubjects { members: approvers() };
    let approver_filter = derive_approver_set(
        &subjects,
        &merge_plan().object,
        &Permission("git.approve".into()),
        &strong("z-1"),
    );
    assert_eq!(approver_filter, approvers(), "the approver set = list_subjects(object, approve_perm) (4.4)");

    // 2 + 3. SURFACE + DECIDE: the run PARKS on the durable wait (9.4); an APPROVAL arrives days later.
    let wait = ScriptedWait { decision: WaitDecision::Approve, parked: RefCell::new(vec![]) };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        approver_filter,
        "card:R1:0",
        &wait,
        &mut approved,
    );

    // the run PARKED on the durable wait before the human decided (the worker was free, holds no runtime).
    assert_eq!(wait.parked.borrow().len(), 1, "the run parked on the durable wait (state=waiting holds no runtime)");
    match outcome {
        HitlOutcome::Approved(g) => assert_eq!(g.state, HitlGateState::Approved),
        other => panic!("expected Approved, got {other:?}"),
    }

    // 4. RESUME: the re-run with the now-populated `approved` set passes step 6 and APPLIES — once.
    let (result2, muts_after) = apply_once(&cat, &endpoint, approved.as_set());
    assert!(matches!(result2, EffectResult::Applied(_)), "the approved effect APPLIES on resume: {result2:?}");
    assert_eq!(muts_after, 1, "the effect applied EXACTLY ONCE (after approval, never before)");
}

/// **CHAINED-E2E (AG-D5 rejection leg): a gated effect is WITHHELD, the run parks, a REJECTION
/// arrives → `Halted::Rejected(reason)` (the reason rides into the trace + audit); a re-run still
/// GATES (the tool was never approved) — 0 mutation across the whole flow (AG-8).**
#[test]
fn chained_withhold_then_reject_halts_with_reason_zero_mutation() {
    let cat = Catalogue { defs: vec![merge_tool()] };
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // WITHHOLD.
    let (result, _) = apply_once(&cat, &endpoint, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("gated");

    // DECIDE → REJECT (days later).
    let wait = ScriptedWait {
        decision: WaitDecision::Reject("merge not safe — failing checks".into()),
        parked: RefCell::new(vec![]),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        approvers(),
        "card:R1:0",
        &wait,
        &mut approved,
    );

    // the rejection settles Halted::Rejected with the reason (recorded in the trace + audit).
    assert_eq!(
        outcome,
        HitlOutcome::Halted(Halted::Rejected("merge not safe — failing checks".into())),
        "rejection settles Halted::Rejected with the reason"
    );
    // the tool was NEVER threaded into `approved` → a re-run GATES again (never applies).
    assert!(!approved.contains("git.merge"), "a rejected gate never approves the tool (AG-8)");
    let (result2, muts) = apply_once(&cat, &endpoint, approved.as_set());
    assert!(matches!(result2, EffectResult::Gated(_)), "a rejected effect still GATES — never applies");
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow (AG-8)");
}

/// **CONSUMER CDC for 9.4 (the durable HITL wait) — the agent fabric consumes the three decisions a
/// durable wait can resume with (Approve / Reject / Expired) and drives the `hitl_gate` state machine
/// correctly for each.** The PROVIDER is the durable-workflow wait; the CONSUMER is [`run_hitl_loop`].
#[test]
fn cdc_9_4_consumer_drives_all_three_wait_decisions() {
    let plan = merge_plan();
    let risk = RiskSummary::for_action("agent.hitl.merge_pr", &plan.object);

    for (decision, expect_approved, expect_halt) in [
        (WaitDecision::Approve, true, None),
        (WaitDecision::Reject("no".into()), false, Some(Halted::Rejected("no".into()))),
        (WaitDecision::Expired, false, Some(Halted::Expired)),
    ] {
        let wait = ScriptedWait { decision, parked: RefCell::new(vec![]) };
        let mut approved = ApprovedTools::new();
        let outcome = run_hitl_loop(
            gate_id_of(&EffectResult::Gated(myelin_agent::GateId("g".into()))).unwrap(),
            "R1",
            &plan,
            risk.clone(),
            approvers(),
            "card:R1:0",
            &wait,
            &mut approved,
        );
        assert_eq!(wait.parked.borrow().len(), 1, "every decision parked on the durable wait first");
        assert_eq!(approved.contains("git.merge"), expect_approved, "only Approve threads the tool");
        match (expect_halt, outcome) {
            (None, HitlOutcome::Approved(_)) => {}
            (Some(h), HitlOutcome::Halted(got)) => assert_eq!(got, h),
            (e, g) => panic!("decision drove the wrong outcome: expected halt {e:?}, got {g:?}"),
        }
    }
}

/// **CONSUMER CDC for 4.4 (`list_subjects`) — the approver set the HITL card carries is exactly the
/// `list_subjects(object, approve_perm)` members at the run's zookie (the REAL Identity provider).**
#[test]
fn cdc_4_4_consumer_approver_set_is_list_subjects_members() {
    let subjects = ProviderSubjects { members: approvers() };
    let set = derive_approver_set(
        &subjects,
        &ArtifactRef("myelin://acme/git/pr/42".into()),
        &Permission("git.approve".into()),
        &strong("z-9"),
    );
    assert_eq!(set, approvers());

    // the card the gate surfaces carries this approver set (who MAY decide) + the action + LIVE cost.
    let gate = HitlGate::open(
        myelin_agent::GateId("g".into()),
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        set.clone(),
        "card:R1:0",
    );
    let card = surface_card(&gate);
    assert_eq!(card.approvers, approvers(), "the card shows the approver set");
    assert_eq!(card.cost_estimate, 50, "the card shows the LIVE cost estimate (wholesale 30 + markup 20)");
    assert_eq!(card.action_tool, "git.merge", "the card shows the pending action");
}
