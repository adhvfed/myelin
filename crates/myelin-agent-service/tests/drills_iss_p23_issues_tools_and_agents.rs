use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    close_tool_def, create_tool_def, full_issues_tool_defs, register_full_issues_tools,
    replay_forecast_agent, transition_caveat, transition_tool_def, triage_suggestion_strip,
    ApplyError, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, ForecastInput,
    LinearForecast, PipelineSignals, PlanThenApply, PlannedEffect, SubsystemApply, TenantGuard,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
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

struct CheckProvider {
    allow: BTreeSet<String>,
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

#[test]
fn cdc_8_1_full_issues_catalogue_registers_into_the_one_surface() {
    let mut cat = Catalogue { defs: vec![] };
    let defs = register_full_issues_tools(&mut cat).expect("seeded defs admit");
    assert_eq!(
        defs.len(),
        13,
        "the compatibility create contract remains beside 8 current CRUD and 4 advisory tools"
    );

    for name in [
        "create",
        "update",
        "comment",
        "link",
        "estimate",
        "reorder",
        "assign",
        "close",
        "forecast",
        "triage",
        "sla_draft",
        "transition",
    ] {
        assert!(
            cat.resolve(&ToolName(name.into())).is_some(),
            "{name} registered"
        );
    }

    assert_eq!(
        cat.resolve(&ToolName("create".into()))
            .unwrap()
            .required_caps,
        vec!["issue.create".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("close".into()))
            .unwrap()
            .required_caps,
        vec!["issue.transition".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("transition".into()))
            .unwrap()
            .required_caps,
        vec!["issue_transition.perform_transition".to_string()]
    );
}

#[test]
fn the_frozen_consequential_split_is_close_and_transition_only() {
    let defs = full_issues_tool_defs();
    let gated: Vec<&str> = defs
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(gated, vec!["close", "transition"]);
    assert!(defs
        .iter()
        .all(|d| d.effect_kind == EffectKind::Mutate && d.side_effecting));
}

#[test]
fn ag_d5_governed_transition_withheld_then_applies_once() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];
    let object = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    let caveat = transition_caveat(object.clone(), "issue:ENG-1421:open->done");
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"ENG-1421","to_state":"done"}"#.into(),
        field: None,
        transition: caveat.transition.clone(),
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };

    let (withheld, muts0) = apply_once(
        &cat,
        &endpoint,
        &check,
        caps.clone(),
        BTreeSet::new(),
        &plan,
    );
    assert!(
        matches!(withheld, EffectResult::Gated(_)),
        "AG-D5: the governed transition is WITHHELD, never applied: {withheld:?}"
    );
    assert_eq!(
        muts0, 0,
        "AG-D5: 0 mutation before approval (the green counter)"
    );

    let approved: BTreeSet<String> = [myelin_agent_service::effect_gate_key(
        &plan.tool,
        &plan.object,
    )]
    .into_iter()
    .collect();
    let (applied, muts1) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(applied, EffectResult::Applied(_)),
        "after approval the transition APPLIES: {applied:?}"
    );
    assert_eq!(muts1, 1, "AG-D5: exactly one apply after approval");
}

#[test]
fn ag_d5_governed_transition_without_approver_context_is_denied() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        transition_needs_approver: true,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];
    let object = ArtifactRef("myelin://acme/issue/issue/ENG-9".into());
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"ENG-9","to_state":"done"}"#.into(),
        field: None,
        transition: transition_caveat(object, "issue:ENG-9:open->done").transition,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };
    let approved: BTreeSet<String> = [myelin_agent_service::effect_gate_key(
        &plan.tool,
        &plan.object,
    )]
    .into_iter()
    .collect();
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(out, EffectResult::Denied(_)),
        "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
    );
    assert_eq!(muts, 0, "a denied governed transition makes 0 mutation");
}

#[test]
fn close_is_withheld_until_approval() {
    let cat = Catalogue {
        defs: vec![close_tool_def()],
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
        tool: ToolName("close".into()),
        object: ArtifactRef("myelin://acme/issue/issue/ENG-7".into()),
        input_json: r#"{"issue_ref":"myelin://acme/issue/issue/ENG-7"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 4,
            markup: 1,
        },
    };
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Gated(_)),
        "close WITHHOLDS until approval (the frozen §6.3 floor): {out:?}"
    );
    assert_eq!(muts, 0, "0 mutation before the close approval (AG-8)");
}

#[test]
fn create_applies_directly_no_gate() {
    let cat = Catalogue {
        defs: vec![create_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue.create".to_string()].into_iter().collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["issue.create".to_string()];
    let project_ref = "myelin://acme/identity/project/01234567-89ab-cdef-0123-456789abcdef";
    let plan = PlannedEffect {
        tool: ToolName("create".into()),
        object: ArtifactRef(project_ref.into()),
        input_json: format!(r#"{{"project_ref":"{project_ref}","title":"a bug"}}"#),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 2,
            markup: 1,
        },
    };
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Applied(_)),
        "create applies directly (no gate): {out:?}"
    );
    assert_eq!(
        muts, 1,
        "exactly one apply (no withhold for the reversible tool)"
    );
}

#[test]
fn ag_d9_forecast_agent_replay_is_byte_identical() {
    let input = ForecastInput {
        remaining: 84,
        velocity_per_period: 12,
        at_risk_threshold_periods: 6,
    };
    let a = replay_forecast_agent(&input);
    let b = replay_forecast_agent(&input);
    assert_eq!(a, b, "AG-D9: two forecast-agent replays are byte-identical");
    assert!(a.terminated, "the compute-only forecast agent terminates");
    let out = LinearForecast::forecast(&input);
    assert_eq!(out.periods_to_completion, Some(7));
    assert!(out.at_risk, "7 > 6 → at-risk (crosses the threshold)");
}

#[test]
fn ag_d9_triage_strip_is_byte_identical_and_proposes_one_effect() {
    let a = triage_suggestion_strip("ENG-1421");
    let b = triage_suggestion_strip("ENG-1421");
    assert_eq!(a, b, "AG-D9: two triage dry-run strips are byte-identical");
    assert_eq!(
        a.len(),
        1,
        "the triage agent proposes one advisory effect (S9)"
    );
    assert!(
        a[0].0.contains("tool=triage") && a[0].0.contains("ENG-1421"),
        "the proposed effect is the triage suggestion for the named issue: {}",
        a[0].0
    );
}
