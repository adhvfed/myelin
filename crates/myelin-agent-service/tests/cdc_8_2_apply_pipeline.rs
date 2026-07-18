//! # The CDC pair for contract 8.2 (the apply-pipeline BODY) + the consumer CDCs for 4.2 / 4.5
//! (AG-P6 → P-218)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.2
//! (`EffectApi::apply(run, ProposedEffect) → Applied(event_id) | Gated(gate_id) | Denied(reason)` —
//! the eight-step plan-then-apply pipeline: SCHEMA → CAPABILITY → DELEGATION → TENANT → BUDGET →
//! HITL-gate → APPLY via the subsystem's PUBLIC endpoint → METER; Denied = ordinary tool error, no
//! privileged fallback; a withheld gated tool does NOT mutate, AG-8). Owning architecture:
//! `agent-fabric.md` §5.2. AG-P1 (→ P-130) shipped the SIGNATURE-half CDC
//! (`myelin-agent/tests/cdc_8_2_effect_api.rs`); THIS pair pins the PIPELINE-BODY half AG-P6 owns.
//!
//! It also carries the consumer CDCs the prompt's TESTS field names: the **consumer CDC for 4.2**
//! (`check` + `CaveatContext` — the engine consumes a REAL Identity `check` provider) and **4.5**
//! (`delegation` → `EffectivePolicy`, the ∩ algebra — the engine consumes a REAL `delegation`
//! provider). The provider impls below are the REAL Identity-side shapes; the consumer is the
//! `PlanThenApply` pipeline driving them through its [`CapabilityCheck`] / [`DelegationLookup`]
//! seams (the same trait-decoupling the 4.7 run-token CDC uses — no production dep on
//! `myelin-identity-service`).

use myelin_agent::{
    EffectApi, EffectKind, EffectResult, EventId, ProposedEffect, RunCtx, ToolDef, ToolName,
    ToolSurface,
};
use myelin_agent_service::{
    encode_proposed, ApplyError, CapabilityCheck, DelegationLookup, EffectApiBridge, EffectBudget,
    EffectCost, PipelineSignals, PlanThenApply, PlannedEffect, SubsystemApply, TenantGuard,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

// ───────────────────────── PROVIDER side: 4.2 check (Identity) ─────────────────────────

/// **A REAL provider on the contract-4.2 `check` surface (the Identity side).** It allows a fixed
/// cap set, denies otherwise, and returns `Conditional` (never a silent allow) when a transition
/// caveat is present but unmet (the OQ-E field/transition ABAC leg). The consumer (the pipeline)
/// reads the [`Decision`] fail-closed.
struct ProviderCheck {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for ProviderCheck {
    fn check(
        &self,
        _subject: &Principal,
        permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision {
        // a transition caveat with no approver attr → Conditional (the SLA-bound transition leg).
        if caveat.map(|c| c.transition.is_some()).unwrap_or(false)
            && caveat.map(|c| c.attrs.is_empty()).unwrap_or(true)
        {
            return Decision::Conditional;
        }
        if self.allow.contains(&permission.0) {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }
}

// ───────────────────────── PROVIDER side: 4.5 delegation (Identity) ─────────────────────────

/// **A REAL provider on the contract-4.5 `delegation` surface (the Identity side).** Returns the
/// effective policy after the monotone intersection `agent.policy ∩ delegation ∩ tenant.policy`
/// (intersection, never union). The consumer confines the run to exactly these caps.
struct ProviderDelegation {
    intersection: Vec<String>,
}
impl DelegationLookup for ProviderDelegation {
    fn delegation(&self, _agent: &Principal, _trigger: &Principal) -> EffectivePolicy {
        EffectivePolicy {
            caveats: self.intersection.clone(),
        }
    }
}

// ───────────────────────── the other consumed seams (tenant / apply / budget) ─────────────────

struct ProviderTenant;
impl TenantGuard for ProviderTenant {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// A REAL subsystem PUBLIC endpoint provider — the ONLY mutation path; records the apply.
struct ProviderEndpoint {
    applied: RefCell<Vec<String>>,
}
impl SubsystemApply for ProviderEndpoint {
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

/// A REAL reserve/settle budget provider (integer minor-units).
struct ProviderBudget {
    remaining: u64,
    settles: u64,
}
impl EffectBudget for ProviderBudget {
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

fn tool(name: &str, caps: &[&str], requires_approval: bool) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: "issues".into(),
        version: 1,
        input_schema:
            r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#
                .into(),
        required_caps: caps.iter().map(|c| c.to_string()).collect(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval,
        exposed_over_mcp: false,
    }
}

fn plan(tool: &str) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(tool.into()),
        object: ArtifactRef("myelin://acme/issues/i-1".into()),
        input_json: r#"{"title":"x"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 3,
            markup: 1,
        },
    }
}

/// **PROVIDER+CONSUMER CDC for 8.2 — the pipeline applies an allowed effect (Applied) and denies a
/// disallowed one (Denied), through the REAL `check`/`delegation` providers + the subsystem PUBLIC
/// endpoint.** The CONSUMER is the dispatch/loop tier handing a `ProposedEffect` and branching on
/// the three frozen outcomes; the PROVIDER is the eight-step pipeline.
#[test]
fn cdc_8_2_pipeline_applies_allowed_denies_disallowed() {
    let cat = Catalogue {
        defs: vec![
            tool("issue.create", &["issue.write"], false),
            tool("issue.delete", &["issue.delete"], false),
        ],
    };
    let check = ProviderCheck {
        allow: ["issue.write".to_string(), "issue.delete".to_string()]
            .into_iter()
            .collect(),
    };
    // the delegation ∩ grants write but NOT delete → the delete is confined out (4.5).
    let del = ProviderDelegation {
        intersection: vec!["issue.write".into()],
    };
    let tenant = ProviderTenant;
    let endpoint = ProviderEndpoint {
        applied: RefCell::new(vec![]),
    };
    let mut budget = ProviderBudget {
        remaining: 100,
        settles: 0,
    };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: &cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: &endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved: BTreeSet::new(),
        signals: &mut signals,
    };

    // CONSUMER: an allowed effect → Applied(event_id), metered once.
    match p.apply_planned(&plan("issue.create")) {
        EffectResult::Applied(EventId(id)) => {
            assert!(
                id.starts_with("evt:issue.create"),
                "applied carries the emitted event id: {id}"
            )
        }
        other => panic!("expected Applied, got {other:?}"),
    }
    // CONSUMER: a disallowed effect (outside the ∩) → Denied (no privileged fallback).
    match p.apply_planned(&plan("issue.delete")) {
        EffectResult::Denied(reason) => assert!(reason.contains("intersection"), "{reason}"),
        other => panic!("expected Denied, got {other:?}"),
    }

    assert_eq!(
        endpoint.applied.borrow().len(),
        1,
        "exactly one mutation reached a subsystem endpoint"
    );
    assert_eq!(
        budget.settles, 1,
        "exactly one cost event metered (the applied effect)"
    );
    assert_eq!(signals.applied(), 1);
    assert_eq!(signals.denied(), 1);
    assert_eq!(
        signals.privileged_fallback(),
        0,
        "AG-D2: 0 privileged fallback"
    );
}

/// **CONSUMER CDC for 4.2 (`check` + `CaveatContext`) — the pipeline consumes a REAL `check`
/// provider; a denied cap fail-closes; a transition caveat that is unmet returns Conditional and
/// the pipeline treats it as a DENY (never a silent allow, OQ-E/§8.6).**
#[test]
fn cdc_4_2_consumer_check_with_caveat_fail_closes() {
    let cat = Catalogue {
        defs: vec![tool("issue.transition", &["issue.transition"], false)],
    };
    // the provider allows the cap WITHOUT a caveat, but a transition caveat → Conditional.
    let check = ProviderCheck {
        allow: ["issue.transition".to_string()].into_iter().collect(),
    };
    let del = ProviderDelegation {
        intersection: vec!["issue.transition".into()],
    };
    let tenant = ProviderTenant;
    let endpoint = ProviderEndpoint {
        applied: RefCell::new(vec![]),
    };
    let mut budget = ProviderBudget {
        remaining: 100,
        settles: 0,
    };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: &cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: &endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved: BTreeSet::new(),
        signals: &mut signals,
    };

    // an SLA-bound transition with no approver context → Conditional → DENY (the caveat is consumed).
    let mut p_transition = plan("issue.transition");
    p_transition.transition = Some(myelin_identity::TransitionId("to_done".into()));
    match p.apply_planned(&p_transition) {
        EffectResult::Denied(_) => {}
        other => panic!("a Conditional caveat must DENY, never silently allow; got {other:?}"),
    }
    assert_eq!(
        endpoint.applied.borrow().len(),
        0,
        "the unmet caveat did NOT mutate"
    );
}

/// **CONSUMER CDC for 4.5 (`delegation` → `EffectivePolicy`) — the pipeline consumes the REAL ∩
/// algebra: a cap the agent holds but the delegation ∩ forbids is confined out (over-privilege
/// blocked, AG-D3).**
#[test]
fn cdc_4_5_consumer_delegation_intersection_confines() {
    let cat = Catalogue {
        defs: vec![tool("issue.delete", &["issue.delete"], false)],
    };
    // check ALLOWS delete (agent.policy), but delegation ∩ does NOT include it.
    let check = ProviderCheck {
        allow: ["issue.delete".to_string()].into_iter().collect(),
    };
    let del = ProviderDelegation {
        intersection: vec!["issue.write".into()],
    };
    let tenant = ProviderTenant;
    let endpoint = ProviderEndpoint {
        applied: RefCell::new(vec![]),
    };
    let mut budget = ProviderBudget {
        remaining: 100,
        settles: 0,
    };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: &cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: &endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved: BTreeSet::new(),
        signals: &mut signals,
    };

    match p.apply_planned(&plan("issue.delete")) {
        EffectResult::Denied(r) => assert!(r.contains("intersection"), "{r}"),
        other => panic!("over-privilege must be confined to the ∩; got {other:?}"),
    }
    assert_eq!(
        endpoint.applied.borrow().len(),
        0,
        "the over-privileged effect did NOT mutate"
    );
}

/// **The frozen unbound `EffectApi::apply` shape cannot bypass external run-token authority.** MCP
/// uses `apply_authorized`; an unbound carrier is denied before the pipeline or endpoint.
#[test]
fn cdc_8_2_unbound_glue_trait_body_denies_without_mutating() {
    let cat = Catalogue {
        defs: vec![tool(
            "git.merge",
            &["git.merge"],
            /* requires_approval */ true,
        )],
    };
    let check = ProviderCheck {
        allow: ["git.merge".to_string()].into_iter().collect(),
    };
    let del = ProviderDelegation {
        intersection: vec!["git.merge".into()],
    };
    let tenant = ProviderTenant;
    let endpoint = ProviderEndpoint {
        applied: RefCell::new(vec![]),
    };
    let mut budget = ProviderBudget {
        remaining: 100,
        settles: 0,
    };
    let mut signals = PipelineSignals::new();
    let p = PlanThenApply {
        catalogue: &cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: &endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved: BTreeSet::new(),
        signals: &mut signals,
    };
    let bridge = EffectApiBridge::new(p);

    let mut merge_plan = plan("git.merge");
    merge_plan.cost.unit = "git.merge";
    let carrier: ProposedEffect = encode_proposed(&merge_plan);
    match bridge.apply(&RunCtx::default(), carrier) {
        EffectResult::Denied(reason) if reason.contains("signed run-token") => {}
        other => panic!("an unbound external EffectApi call must deny; got {other:?}"),
    }
    assert_eq!(
        endpoint.applied.borrow().len(),
        0,
        "an unbound effect does NOT mutate"
    );
}
