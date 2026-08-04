use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::HitlGate;
use myelin_agent_service::HitlWait;
use myelin_agent_service::{
    gate_id_of, git_merge_required_caps, git_merge_tool_def, git_tool_defs, open_pr_required_caps,
    open_pr_tool_def, register_git_tools, run_hitl_loop, ApplyError, ApprovedTools,
    CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, HitlOutcome, PipelineSignals,
    PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply, TenantGuard, WaitDecision,
    GIT_MERGE_TOOL, GIT_SUBSYSTEM, OPEN_PR_TOOL,
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

fn open_pr_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(OPEN_PR_TOOL.into()),
        object: ArtifactRef("myelin://acme/git/repo/repo7".into()),
        input_json: r#"{"repo":"repo7","source_ref":"feat/x","target_ref":"main","title":"x"}"#
            .into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "git.open_pr",
            wholesale: 5,
            markup: 5,
        },
    }
}

fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    plan: &PlannedEffect,
    allowed_caps: &[&str],
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowCaps {
        allow: allowed_caps.iter().map(|c| c.to_string()).collect(),
    };
    let del = Delegate {
        caps: allowed_caps.iter().map(|c| c.to_string()).collect(),
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
    let out = p.apply_planned(plan);
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

#[test]
fn git_tools_register_with_the_frozen_6_3_defaults() {
    let cat = git_catalogue();

    let merge = cat
        .resolve(&ToolName(GIT_MERGE_TOOL.into()))
        .expect("git.merge registered");
    assert_eq!(merge.subsystem, GIT_SUBSYSTEM);
    assert!(
        merge.requires_approval,
        "git.merge carries requires_approval = yes (§6.3 / AG-8)"
    );
    assert_eq!(
        merge.effect_kind,
        EffectKind::Mutate,
        "git.merge routes through EffectApi"
    );
    assert_eq!(merge.required_caps, git_merge_required_caps());
    assert_eq!(
        merge.required_caps,
        vec!["pull_request.merge".to_string()],
        "4.9 cap"
    );

    let pr = cat
        .resolve(&ToolName(OPEN_PR_TOOL.into()))
        .expect("open_pr registered");
    assert!(
        !pr.requires_approval,
        "open_pr carries requires_approval = no (§6.3 - reversible)"
    );
    assert_eq!(pr.effect_kind, EffectKind::Mutate);
    assert_eq!(pr.required_caps, open_pr_required_caps());
    assert_eq!(pr.required_caps, vec!["repo.push".to_string()], "4.9 cap");
}

#[test]
fn knd11_git_merge_is_governed_zero_ungoverned_zero_double_apply() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["pull_request.merge"];

    let (result, muts_before) = apply_once(&cat, &endpoint, &merge_plan(), &caps, BTreeSet::new());
    let gate_id =
        gate_id_of(&result).expect("git.merge is requires_approval → it GATES (0 ungoverned)");
    assert!(
        matches!(result, EffectResult::Gated(_)),
        "the registered git.merge WITHHOLDS: {result:?}"
    );
    assert_eq!(
        muts_before, 0,
        "0 MUTATIONS before approval (KN-D11 - the merge did NOT apply)"
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
        "the run PARKED on the durable wait (state=waiting, no runtime)"
    );
    assert!(
        matches!(outcome, HitlOutcome::Approved(_)),
        "approval resumes: {outcome:?}"
    );

    if let HitlOutcome::Approved(ref gate) = outcome {
        approved.admit(gate);
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double-click is ONE approval (the set holds one tool)"
        );
    }

    let (result2, muts_after) =
        apply_once(&cat, &endpoint, &merge_plan(), &caps, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Applied(_)),
        "the approved merge APPLIES on resume: {result2:?}"
    );
    assert_eq!(
        muts_after, 1,
        "the merge applied EXACTLY ONCE (0 double-apply, after approval, never before)"
    );
}

#[test]
fn knd11_git_merge_rejected_never_applies_zero_mutation() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["pull_request.merge"];

    let (result, _) = apply_once(&cat, &endpoint, &merge_plan(), &caps, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("gated");

    let wait = ScriptedWait {
        decision: WaitDecision::Reject("failing required checks".into()),
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
    assert!(
        matches!(outcome, HitlOutcome::Halted(_)),
        "rejection halts: {outcome:?}"
    );
    assert!(
        !approved.contains(GIT_MERGE_TOOL),
        "a rejected merge never approves the tool (AG-8)"
    );

    let (result2, muts) = apply_once(&cat, &endpoint, &merge_plan(), &caps, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Gated(_)),
        "a rejected merge still GATES - never applies"
    );
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow (AG-8)");
}

#[test]
fn open_pr_applies_directly_no_hitl_gate() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (result, muts) = apply_once(
        &cat,
        &endpoint,
        &open_pr_plan(),
        &["repo.push"],
        BTreeSet::new(),
    );
    assert!(
        matches!(result, EffectResult::Applied(_)),
        "open_pr applies directly: {result:?}"
    );
    assert!(
        gate_id_of(&result).is_none(),
        "open_pr NEVER opens an HITL gate (it is reversible)"
    );
    assert_eq!(
        muts, 1,
        "open_pr mutated once, directly, with NO prior approval"
    );
}

#[test]
fn cdc_4_9_required_caps_are_the_git_rebac_fragment_permissions() {
    use myelin_git::rebac_fragment::{pull_request_fragment, repo_fragment};

    let pr_frag = pull_request_fragment();
    assert!(
        pr_frag.permissions.iter().any(|p| p.0 == "merge"),
        "the Git ReBAC `pull_request` fragment declares the `merge` permission (4.9)"
    );
    assert_eq!(
        git_merge_tool_def().required_caps,
        vec!["pull_request.merge".to_string()]
    );

    let repo_frag = repo_fragment();
    assert!(
        repo_frag.permissions.iter().any(|p| p.0 == "push"),
        "the Git ReBAC `repo` fragment declares the `push` permission (4.9)"
    );
    assert_eq!(
        open_pr_tool_def().required_caps,
        vec!["repo.push".to_string()]
    );
}

#[test]
fn git_producer_surface_is_a_projection() {
    let defs = git_tool_defs();
    let merge = defs
        .iter()
        .find(|d| d.name.0 == "merge")
        .expect("git.merge registered");
    let open_pr = defs
        .iter()
        .find(|d| d.name.0 == "open_pr")
        .expect("open_pr registered");
    assert_eq!(
        merge.effect_kind,
        EffectKind::Mutate,
        "git.merge routes through EffectApi"
    );
    assert_eq!(
        open_pr.effect_kind,
        EffectKind::Mutate,
        "open_pr routes through EffectApi"
    );
    assert!(merge.requires_approval, "git.merge gated");
    assert!(!open_pr.requires_approval, "open_pr not gated");
    assert!(
        defs.iter().any(|d| d.name.0 == "history_rewrite"),
        "git.history_rewrite registered"
    );
    assert!(
        defs.iter().any(|d| d.name.0 == "scip_index"),
        "git.scip_index registered"
    );
}
