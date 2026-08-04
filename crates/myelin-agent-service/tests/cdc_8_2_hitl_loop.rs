use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    derive_approver_set, gate_id_of, run_hitl_loop, surface_card, ApplyError, ApprovedTools,
    ApproverSet, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, Halted, HitlGate,
    HitlGateState, HitlOutcome, HitlWait, PipelineSignals, PlanThenApply, PlannedEffect,
    RiskSummary, SubsystemApply, TenantGuard, WaitDecision,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, EffectivePolicy, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RuntimeRef, SubjectTree, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

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

struct ScriptedWait {
    decision: WaitDecision,
    parked: RefCell<Vec<String>>,
}
impl HitlWait for ScriptedWait {
    fn park_and_wait(&self, gate: &HitlGate) -> WaitDecision {
        self.parked.borrow_mut().push(gate.card_ref.clone());
        self.decision.clone()
    }
}

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
        input_schema:
            r#"{"type":"object","required":["pr"],"properties":{"pr":{"type":"integer"}}}"#.into(),
        required_caps: vec!["git.merge".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
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
        cost: EffectCost {
            unit: "git.merge",
            wholesale: 30,
            markup: 20,
        },
    }
}

fn approvers() -> Vec<PrincipalId> {
    vec![
        PrincipalId("psn:lead".into()),
        PrincipalId("psn:maintainer".into()),
    ]
}

fn strong(z: &str) -> Consistency {
    Consistency {
        at_least: Zookie(z.into()),
        mode: ConsistencyMode::Strong,
    }
}

fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowAll {
        allow: ["git.merge".to_string()].into_iter().collect(),
    };
    let del = Delegate {
        caps: vec!["git.merge".into()],
    };
    let tenant = PermitAll;
    let mut budget = Budget {
        remaining: 1_000,
        settles: 0,
    };
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

#[test]
fn chained_withhold_zero_mutation_then_approve_resumes_and_applies() {
    let cat = Catalogue {
        defs: vec![merge_tool()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (result, muts_before) = apply_once(&cat, &endpoint, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("a requires_approval tool GATES (AG-8)");
    assert!(
        matches!(result, EffectResult::Gated(_)),
        "the gated effect is WITHHELD: {result:?}"
    );
    assert_eq!(
        muts_before, 0,
        "0 MUTATIONS before approval (AG-D5 - the gated tool did NOT mutate)"
    );

    let subjects = ProviderSubjects {
        members: approvers(),
    };
    let approver_filter = derive_approver_set(
        &subjects,
        &merge_plan().object,
        &Permission("git.approve".into()),
        &strong("z-1"),
    );
    assert_eq!(
        approver_filter,
        approvers(),
        "the approver set = list_subjects(object, approve_perm) (4.4)"
    );

    let wait = ScriptedWait {
        decision: WaitDecision::Approve,
        parked: RefCell::new(vec![]),
    };
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

    assert_eq!(
        wait.parked.borrow().len(),
        1,
        "the run parked on the durable wait (state=waiting holds no runtime)"
    );
    match outcome {
        HitlOutcome::Approved(g) => assert_eq!(g.state, HitlGateState::Approved),
        other => panic!("expected Approved, got {other:?}"),
    }

    let (result2, muts_after) = apply_once(&cat, &endpoint, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Applied(_)),
        "the approved effect APPLIES on resume: {result2:?}"
    );
    assert_eq!(
        muts_after, 1,
        "the effect applied EXACTLY ONCE (after approval, never before)"
    );
}

#[test]
fn chained_withhold_then_reject_halts_with_reason_zero_mutation() {
    let cat = Catalogue {
        defs: vec![merge_tool()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (result, _) = apply_once(&cat, &endpoint, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("gated");

    let wait = ScriptedWait {
        decision: WaitDecision::Reject("merge not safe - failing checks".into()),
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

    assert_eq!(
        outcome,
        HitlOutcome::Halted(Halted::Rejected("merge not safe - failing checks".into())),
        "rejection settles Halted::Rejected with the reason"
    );
    assert!(
        !approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
        "a rejected gate never approves the effect (AG-8)"
    );
    let (result2, muts) = apply_once(&cat, &endpoint, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Gated(_)),
        "a rejected effect still GATES - never applies"
    );
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow (AG-8)");
}

#[test]
fn cdc_9_4_consumer_drives_all_three_wait_decisions() {
    let plan = merge_plan();
    let risk = RiskSummary::for_action("agent.hitl.merge_pr", &plan.object);

    for (decision, expect_approved, expect_halt) in [
        (WaitDecision::Approve, true, None),
        (
            WaitDecision::Reject("no".into()),
            false,
            Some(Halted::Rejected("no".into())),
        ),
        (WaitDecision::Expired, false, Some(Halted::Expired)),
    ] {
        let wait = ScriptedWait {
            decision,
            parked: RefCell::new(vec![]),
        };
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
        assert_eq!(
            wait.parked.borrow().len(),
            1,
            "every decision parked on the durable wait first"
        );
        assert_eq!(
            approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
            expect_approved,
            "only Approve threads the effect's per-(tool, object) key (R2.4)"
        );
        match (expect_halt, outcome) {
            (None, HitlOutcome::Approved(_)) => {}
            (Some(h), HitlOutcome::Halted(got)) => assert_eq!(got, h),
            (e, g) => panic!("decision drove the wrong outcome: expected halt {e:?}, got {g:?}"),
        }
    }
}

#[test]
fn cdc_4_4_consumer_approver_set_is_list_subjects_members() {
    let subjects = ProviderSubjects {
        members: approvers(),
    };
    let set = derive_approver_set(
        &subjects,
        &ArtifactRef("myelin://acme/git/pr/42".into()),
        &Permission("git.approve".into()),
        &strong("z-9"),
    );
    assert_eq!(set, approvers());

    let gate = HitlGate::open(
        myelin_agent::GateId("g".into()),
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        set.clone(),
        "card:R1:0",
    );
    let card = surface_card(&gate);
    assert_eq!(
        card.approvers,
        approvers(),
        "the card shows the approver set"
    );
    assert_eq!(
        card.cost_estimate, 50,
        "the card shows the LIVE cost estimate (wholesale 30 + markup 20)"
    );
    assert_eq!(
        card.action_tool, "git.merge",
        "the card shows the pending action"
    );
}
