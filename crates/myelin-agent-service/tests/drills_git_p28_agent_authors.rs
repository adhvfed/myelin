use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    gate_id_of, git_author_tool_defs, git_tool_defs, register_git_tools, run_hitl_loop, ApplyError,
    ApprovedTools, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, HitlGate,
    HitlOutcome, HitlWait, PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary,
    SubsystemApply, TenantGuard, WaitDecision, GIT_MERGE_TOOL, GIT_SUBSYSTEM,
};
use myelin_git::agent_author::{
    AgentAuthorship, Authorship, COMMENT_TOOL, RESOLVE_THREAD_TOOL, SUBMIT_REVIEW_TOOL,
    SUGGEST_CHANGE_TOOL,
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

struct AllowCaps {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for AllowCaps {
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

struct ScriptedWait {
    decision: WaitDecision,
    parked: RefCell<u32>,
}
impl HitlWait for ScriptedWait {
    fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
        *self.parked.borrow_mut() += 1;
        self.decision.clone()
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

fn git_catalogue() -> Catalogue {
    let mut cat = Catalogue { defs: vec![] };
    register_git_tools(&mut cat).expect("the seeded Git defs always admit (no silent loosening)");
    cat
}

fn author_plan(tool: &str, input_json: &str) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(tool.into()),
        object: ArtifactRef("myelin://acme/git/pull_request/repo7:42".into()),
        input_json: input_json.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "agent.effect",
            wholesale: 1,
            markup: 1,
        },
    }
}

fn comment_plan() -> PlannedEffect {
    author_plan(
        COMMENT_TOOL,
        r#"{"pull_request":"repo7:42","body":"nit: rename `x`"}"#,
    )
}
fn review_plan() -> PlannedEffect {
    author_plan(
        SUBMIT_REVIEW_TOOL,
        r#"{"pull_request":"repo7:42","verdict":"request_changes","body":"please add a test"}"#,
    )
}
fn merge_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(GIT_MERGE_TOOL.into()),
        object: ArtifactRef("myelin://acme/git/pull_request/repo7:42".into()),
        input_json: r#"{"pull_request":"repo7:42","strategy":"squash"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "git.merge",
            wholesale: 30,
            markup: 20,
        },
    }
}

fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    plan: &PlannedEffect,
    allowed_caps: &[&str],
    delegated_caps: &[&str],
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowCaps {
        allow: allowed_caps.iter().map(|c| c.to_string()).collect(),
    };
    let del = Delegate {
        caps: delegated_caps.iter().map(|c| c.to_string()).collect(),
    };
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
    let out = p.apply_planned(plan);
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

#[test]
fn agent_authoring_tools_register_ungated_merge_stays_gated() {
    let cat = git_catalogue();
    for tool in [
        COMMENT_TOOL,
        SUBMIT_REVIEW_TOOL,
        SUGGEST_CHANGE_TOOL,
        RESOLVE_THREAD_TOOL,
    ] {
        let def = cat
            .resolve(&ToolName(tool.into()))
            .unwrap_or_else(|| panic!("{tool} registered"));
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(
            def.effect_kind,
            EffectKind::Mutate,
            "{tool} routes through EffectApi"
        );
        assert!(
            !def.requires_approval,
            "{tool} is reversible authoring → NOT gated (§7)"
        );
        assert_eq!(
            def.required_caps,
            vec!["pull_request.review".to_string()],
            "{tool} cap (4.9)"
        );
    }
    let merge = cat
        .resolve(&ToolName(GIT_MERGE_TOOL.into()))
        .expect("git.merge registered");
    assert!(
        merge.requires_approval,
        "git.merge stays the consequential gate (§6.3 / AG-8)"
    );
}

#[test]
fn a_mock_agent_authors_then_proposes_a_gated_merge_zero_mutation_then_one_apply() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["repo.push", "pull_request.review", "pull_request.merge"];

    let open_pr = PlannedEffect {
        tool: ToolName("open_pr".into()),
        object: ArtifactRef("myelin://acme/git/repo/repo7".into()),
        input_json:
            r#"{"repo":"repo7","source_ref":"agent/fix","target_ref":"main","title":"fix"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "agent.effect",
            wholesale: 2,
            markup: 1,
        },
    };
    let (r_open, m1) = apply_once(&cat, &endpoint, &open_pr, &caps, &caps, BTreeSet::new());
    assert!(
        matches!(r_open, EffectResult::Applied(_)),
        "the agent opens a PR directly: {r_open:?}"
    );
    assert_eq!(
        m1, 1,
        "open PR applied (no approval) - the agent is a first-class author"
    );

    let (r_comment, m2) = apply_once(
        &cat,
        &endpoint,
        &comment_plan(),
        &caps,
        &caps,
        BTreeSet::new(),
    );
    assert!(
        matches!(r_comment, EffectResult::Applied(_)),
        "the agent comments directly: {r_comment:?}"
    );
    assert_eq!(m2, 2, "the comment applied (no approval)");

    let (r_review, m3) = apply_once(
        &cat,
        &endpoint,
        &review_plan(),
        &caps,
        &caps,
        BTreeSet::new(),
    );
    assert!(
        matches!(r_review, EffectResult::Applied(_)),
        "the agent submits a review: {r_review:?}"
    );
    assert_eq!(m3, 3, "the review applied (no approval)");

    let (r_merge, m4) = apply_once(
        &cat,
        &endpoint,
        &merge_plan(),
        &caps,
        &caps,
        BTreeSet::new(),
    );
    let gate_id = gate_id_of(&r_merge).expect("git.merge GATES (the consequential gate)");
    assert!(
        matches!(r_merge, EffectResult::Gated(_)),
        "the agent's merge WITHHOLDS: {r_merge:?}"
    );
    assert_eq!(
        m4, 3,
        "0 MUTATIONS from the merge before approval (AG-D3 / AG-8) - still 3 authored"
    );

    let wait = ScriptedWait {
        decision: WaitDecision::Approve,
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    assert_eq!(
        *wait.parked.borrow(),
        1,
        "the run PARKED on the durable wait (no runtime held)"
    );
    assert!(
        matches!(outcome, HitlOutcome::Approved(_)),
        "approval resumes: {outcome:?}"
    );

    let (r_merge2, m5) = apply_once(
        &cat,
        &endpoint,
        &merge_plan(),
        &caps,
        &caps,
        approved.as_set(),
    );
    assert!(
        matches!(r_merge2, EffectResult::Applied(_)),
        "the approved merge applies: {r_merge2:?}"
    );
    assert_eq!(
        m5, 4,
        "the merge applied EXACTLY ONCE after approval (3 authored + 1 merge)"
    );

    let applied = endpoint.applied.borrow();
    assert_eq!(
        *applied,
        vec!["open_pr", "comment", "submit_review", "merge"]
    );
}

#[test]
fn agd2_an_authoring_effect_outside_the_delegation_intersection_is_denied() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (result, muts) = apply_once(
        &cat,
        &endpoint,
        &comment_plan(),
        &["pull_request.review"],
        &[],
        BTreeSet::new(),
    );
    match result {
        EffectResult::Denied(reason) => {
            assert!(
                reason.contains("delegation intersection"),
                "the denial names the intersection (attenuation never up): {reason}"
            );
        }
        other => panic!("expected Denied (outside the intersection), got {other:?}"),
    }
    assert_eq!(
        muts, 0,
        "AG-D2: 0 mutation on an over-privileged authoring effect (no fallback)"
    );
}

#[test]
fn agd2_an_authoring_effect_without_the_review_cap_is_denied() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let (result, muts) = apply_once(
        &cat,
        &endpoint,
        &review_plan(),
        &["repo.pull"],
        &["repo.pull"],
        BTreeSet::new(),
    );
    assert!(
        matches!(result, EffectResult::Denied(_)),
        "no review cap → Denied: {result:?}"
    );
    assert_eq!(
        muts, 0,
        "AG-D2: 0 mutation without the review cap (same governance as any principal)"
    );
}

#[test]
fn agd5_a_double_click_on_merge_approval_is_one_approval_exactly_once() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["pull_request.merge"];

    let (r_merge, _) = apply_once(
        &cat,
        &endpoint,
        &merge_plan(),
        &caps,
        &caps,
        BTreeSet::new(),
    );
    let gate_id = gate_id_of(&r_merge).expect("gated");

    let wait = ScriptedWait {
        decision: WaitDecision::Approve,
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &merge_plan(),
        RiskSummary::for_action("agent.hitl.merge_pr", &merge_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    if let HitlOutcome::Approved(ref gate) = outcome {
        approved.admit(gate);
        approved.admit(gate);
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double/triple-click is ONE approval"
        );
    } else {
        panic!("expected Approved, got {outcome:?}");
    }

    let (r2, m1) = apply_once(
        &cat,
        &endpoint,
        &merge_plan(),
        &caps,
        &caps,
        approved.as_set(),
    );
    assert!(
        matches!(r2, EffectResult::Applied(_)),
        "the approved merge applies once: {r2:?}"
    );
    assert_eq!(
        m1, 1,
        "AG-D5: the merge applied exactly once after the single approval"
    );
}

#[test]
fn an_agent_author_is_legible_never_disguised_as_human() {
    let agent_authored = Authorship::Agent(AgentAuthorship::new(
        "psn:agent-7",
        "run:R1",
        "request changes: missing test coverage",
    ));
    assert!(
        agent_authored.is_agent(),
        "the agent reviewer is legibly flagged (is_agent)"
    );
    let prov = agent_authored
        .agent_provenance()
        .expect("AI-Act: provenance is REQUIRED");
    assert_eq!(prov.run_id, "run:R1", "which run authored this (traceable)");

    let human = Authorship::Human {
        author_pseudonym: "psn:human-x".into(),
    };
    assert!(!human.is_agent());
    assert!(
        human.agent_provenance().is_none(),
        "a human author has no agent provenance"
    );
}

#[test]
fn cdc_8_1_authoring_tooldefs_are_the_frozen_shape_with_the_4_9_review_cap() {
    use myelin_git::agent_author::review_authoring_required_caps;
    use myelin_git::rebac_fragment::pull_request_fragment;

    let frag = pull_request_fragment();
    assert!(
        frag.permissions.iter().any(|p| p.0 == "review"),
        "the Git `pull_request` fragment declares the `review` permission (4.9)"
    );

    assert_eq!(
        review_authoring_required_caps(),
        vec!["pull_request.review".to_string()]
    );
    for def in git_author_tool_defs() {
        assert_eq!(
            def.required_caps,
            review_authoring_required_caps(),
            "{}'s cap is the frozen 4.9 pull_request.review permission",
            def.name.0
        );
        assert_eq!(
            def.effect_kind,
            EffectKind::Mutate,
            "{} is a mutate tool (8.2)",
            def.name.0
        );
        assert!(
            !def.requires_approval,
            "{} is reversible authoring → not gated (§7)",
            def.name.0
        );
    }

    let all = git_tool_defs();
    for tool in [
        COMMENT_TOOL,
        SUBMIT_REVIEW_TOOL,
        SUGGEST_CHANGE_TOOL,
        RESOLVE_THREAD_TOOL,
    ] {
        assert!(
            all.iter().any(|d| d.name.0 == tool),
            "{tool} is in the ONE producer surface"
        );
    }
}
