use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    comment_required_caps, comment_tool_def, draft_required_caps, draft_tool_def,
    edit_confidential_required_caps, edit_confidential_tool_def, gate_id_of,
    is_content_addressed_kn_document, knowledge_tool_defs, publish_required_caps, publish_tool_def,
    register_knowledge_tools, run_hitl_loop, trace_ref_of, ApplyError, ApprovedTools,
    CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, HitlGate, HitlOutcome, HitlWait,
    PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply, TenantGuard,
    TraceDocument, WaitDecision, COMMENT_TOOL, DRAFT_TOOL, EDIT_CONFIDENTIAL_TOOL,
    KNOWLEDGE_SUBSYSTEM, PUBLISH_TOOL,
};
use myelin_content::{Block, Inline, Span};
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

fn kn_catalogue() -> Catalogue {
    let mut cat = Catalogue { defs: vec![] };
    register_knowledge_tools(&mut cat)
        .expect("the seeded KN defs always admit (no silent loosening)");
    cat
}

fn publish_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(PUBLISH_TOOL.into()),
        object: ArtifactRef("myelin://acme/knowledge/page/onboarding".into()),
        input_json: r#"{"page":"onboarding","space":"eng"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "knowledge.publish",
            wholesale: 10,
            markup: 10,
        },
    }
}

fn draft_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(DRAFT_TOOL.into()),
        object: ArtifactRef("myelin://acme/knowledge/space/eng".into()),
        input_json: r#"{"space":"eng","title":"WIP","blocks":[]}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "knowledge.draft",
            wholesale: 2,
            markup: 2,
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
fn knowledge_tools_register_with_the_frozen_6_3_defaults() {
    let cat = kn_catalogue();

    let publish = cat
        .resolve(&ToolName(PUBLISH_TOOL.into()))
        .expect("publish registered");
    assert_eq!(publish.subsystem, KNOWLEDGE_SUBSYSTEM);
    assert!(
        publish.requires_approval,
        "publish carries requires_approval = yes (§6.3 - consequential)"
    );
    assert_eq!(
        publish.effect_kind,
        EffectKind::Mutate,
        "publish routes through EffectApi"
    );
    assert_eq!(publish.required_caps, publish_required_caps());
    assert_eq!(
        publish.required_caps,
        vec!["page.publish".to_string()],
        "4.9 cap"
    );

    let edit = cat
        .resolve(&ToolName(EDIT_CONFIDENTIAL_TOOL.into()))
        .expect("edit_confidential registered");
    assert!(
        edit.requires_approval,
        "edit_confidential carries requires_approval = yes (§6.3)"
    );
    assert_eq!(edit.required_caps, edit_confidential_required_caps());
    assert_eq!(edit.required_caps, vec!["page.edit".to_string()]);

    let draft = cat
        .resolve(&ToolName(DRAFT_TOOL.into()))
        .expect("draft registered");
    assert!(
        !draft.requires_approval,
        "draft carries requires_approval = no (§6.3 - reversible)"
    );
    assert_eq!(draft.required_caps, draft_required_caps());
    assert_eq!(draft.required_caps, vec!["page.draft".to_string()]);

    let comment = cat
        .resolve(&ToolName(COMMENT_TOOL.into()))
        .expect("comment registered");
    assert!(
        !comment.requires_approval,
        "comment carries requires_approval = no (§6.3 - reversible)"
    );
    assert_eq!(comment.required_caps, comment_required_caps());
    assert_eq!(comment.required_caps, vec!["page.comment".to_string()]);
}

#[test]
fn knd11_kn_publish_is_governed_zero_ungoverned_zero_double_apply() {
    let cat = kn_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["page.publish"];

    let (result, muts_before) =
        apply_once(&cat, &endpoint, &publish_plan(), &caps, BTreeSet::new());
    let gate_id =
        gate_id_of(&result).expect("publish is requires_approval → it GATES (0 ungoverned)");
    assert!(
        matches!(result, EffectResult::Gated(_)),
        "the registered publish WITHHOLDS: {result:?}"
    );
    assert_eq!(
        muts_before, 0,
        "0 MUTATIONS before approval (KN-D11 - the publish did NOT apply)"
    );

    let wait = ScriptedWait {
        decision: WaitDecision::Approve,
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
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
        apply_once(&cat, &endpoint, &publish_plan(), &caps, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Applied(_)),
        "the approved publish APPLIES on resume: {result2:?}"
    );
    assert_eq!(
        muts_after, 1,
        "the publish applied EXACTLY ONCE (0 double-apply, after approval, never before)"
    );
}

#[test]
fn knd11_kn_publish_rejected_never_applies_zero_mutation() {
    let cat = kn_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["page.publish"];

    let (result, _) = apply_once(&cat, &endpoint, &publish_plan(), &caps, BTreeSet::new());
    let gate_id = gate_id_of(&result).expect("gated");

    let wait = ScriptedWait {
        decision: WaitDecision::Reject("not ready to publish".into()),
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
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
        !approved.contains(PUBLISH_TOOL),
        "a rejected publish never approves the tool"
    );

    let (result2, muts) = apply_once(&cat, &endpoint, &publish_plan(), &caps, approved.as_set());
    assert!(
        matches!(result2, EffectResult::Gated(_)),
        "a rejected publish still GATES - never applies"
    );
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow");
}

#[test]
fn draft_applies_directly_no_hitl_gate() {
    let cat = kn_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (result, muts) = apply_once(
        &cat,
        &endpoint,
        &draft_plan(),
        &["page.draft"],
        BTreeSet::new(),
    );
    assert!(
        matches!(result, EffectResult::Applied(_)),
        "draft applies directly: {result:?}"
    );
    assert!(
        gate_id_of(&result).is_none(),
        "draft NEVER opens an HITL gate (it is reversible)"
    );
    assert_eq!(
        muts, 1,
        "draft mutated once, directly, with NO prior approval"
    );
}

#[test]
fn chained_e2e_publish_withheld_then_approved_one_apply_draft_direct() {
    let cat = kn_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let (gated, m0) = apply_once(
        &cat,
        &endpoint,
        &publish_plan(),
        &["page.publish"],
        BTreeSet::new(),
    );
    let gate_id = gate_id_of(&gated).expect("publish withholds");
    assert_eq!(m0, 0, "0 mutation before approval");
    let wait = ScriptedWait {
        decision: WaitDecision::Approve,
        parked: RefCell::new(0),
    };
    let mut approved = ApprovedTools::new();
    let outcome = run_hitl_loop(
        gate_id,
        "R1",
        &publish_plan(),
        RiskSummary::for_action("agent.hitl.publish_page", &publish_plan().object),
        vec![PrincipalId("psn:lead".into())],
        "card:R1:0",
        &wait,
        &mut approved,
    );
    assert!(matches!(outcome, HitlOutcome::Approved(_)));
    let (applied, m1) = apply_once(
        &cat,
        &endpoint,
        &publish_plan(),
        &["page.publish"],
        approved.as_set(),
    );
    assert!(matches!(applied, EffectResult::Applied(_)));
    assert_eq!(m1, 1, "exactly ONE publish after approval");

    let (draft_res, m2) = apply_once(
        &cat,
        &endpoint,
        &draft_plan(),
        &["page.draft"],
        BTreeSet::new(),
    );
    assert!(
        matches!(draft_res, EffectResult::Applied(_)),
        "draft applied directly"
    );
    assert_eq!(m2, 2, "the draft applied (publish=1 + draft=1)");
    assert_eq!(
        *endpoint.applied.borrow(),
        vec![PUBLISH_TOOL.to_string(), DRAFT_TOOL.to_string()]
    );
}

fn text(s: &str) -> Inline {
    Inline {
        spans: vec![Span::Text {
            text: s.to_string(),
            marks: vec![],
            link: None,
        }],
        nodes: vec![],
    }
}

fn pii_trace(run_id: u128) -> TraceDocument {
    TraceDocument::new(
        run_id,
        vec![Block::Paragraph {
            inline: text(
                "the agent processed the support ticket for alice@example.com and drafted a reply",
            ),
        }],
    )
}

#[test]
fn cdc_8_8_trace_ref_resolves_to_a_content_addressed_13_1_document() {
    let doc = pii_trace(7);
    assert!(
        matches!(doc.blocks[0], Block::Paragraph { .. }),
        "the trace reuses the 13.1 Block taxonomy"
    );
    let trace_ref = trace_ref_of(&doc);
    assert!(
        trace_ref.starts_with("blake3:"),
        "run.trace_ref is a content address: {trace_ref}"
    );
    assert!(
        is_content_addressed_kn_document(&doc),
        "the trace is the content-addressed KN document (8.8)"
    );
}

#[test]
fn cdc_4_9_required_caps_are_the_kn_rebac_carrier_permissions() {
    use myelin_content::rebac_fragment::{
        object_types, page_write_fragment, COMMENT, DRAFT, EDIT, PUBLISH,
    };

    let page = page_write_fragment();
    assert_eq!(page.object_type.0, object_types::PAGE);
    for p in [PUBLISH, EDIT, DRAFT, COMMENT] {
        assert!(
            page.permissions.iter().any(|perm| perm.0 == p),
            "the carrier declares `page.{p}` (4.9)"
        );
    }
    assert_eq!(
        publish_tool_def().required_caps,
        vec!["page.publish".to_string()]
    );
    assert_eq!(
        edit_confidential_tool_def().required_caps,
        vec!["page.edit".to_string()]
    );
    assert_eq!(
        draft_tool_def().required_caps,
        vec!["page.draft".to_string()]
    );
    assert_eq!(
        comment_tool_def().required_caps,
        vec!["page.comment".to_string()]
    );
}

#[test]
fn cdc_13_1_trace_reuses_the_frozen_block_taxonomy() {
    let doc = TraceDocument::new(
        1,
        vec![
            Block::Paragraph {
                inline: text("reasoning"),
            },
            Block::CodeBlock {
                lang: Some("json".into()),
                text: "{}".into(),
            },
            Block::Divider,
        ],
    );
    assert_eq!(
        doc.blocks.len(),
        3,
        "the trace is a Vec<Block> of frozen 13.1 nodes"
    );
    assert_eq!(
        doc.content_address(),
        doc.content_address(),
        "deterministic over the frozen 13.1 model"
    );
}

#[test]
fn kn_producer_surface_is_a_projection_four_defs_only() {
    let defs = knowledge_tool_defs();
    assert_eq!(
        defs.len(),
        4,
        "exactly publish + edit_confidential + draft + comment - no other KN producer tool at M3"
    );
    for d in &defs {
        assert_eq!(
            d.effect_kind,
            EffectKind::Mutate,
            "every KN tool routes through EffectApi"
        );
        assert!(d.side_effecting);
    }
    assert!(
        defs[0].requires_approval && defs[1].requires_approval,
        "publish + edit_confidential gated"
    );
    assert!(
        !defs[2].requires_approval && !defs[3].requires_approval,
        "draft + comment not gated"
    );
}
