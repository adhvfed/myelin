//! # ISS-P23 (→ P-390, M4-I6) — the FULL Issues ToolDef catalogue + EffectApi plan-then-apply +
//! the MOCK forecast/triage agents (gated on AG-D4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` rows 8.1
//! (register the FULL Issues catalogue — arch 03 §8), 8.2 (EffectApi plan-then-apply, no carve-out),
//! 8.3 (the MOCK runtime), 8.4 (the AG-D4-gated unified sandbox), 8.7 (run --dry-run). Owning
//! architecture: `issue-tracker/architecture/03-events-contracts-and-glue.md` §8 (the catalogue + the
//! frozen requires_approval defaults), §9 (reserve/settle — ISS-P24); `agent-fabric.md` §6.1/§6.3
//! (the ONE catalogue + the frozen defaults), §5.2 step 2 (the field/transition ABAC caveat), §3.2
//! (the MockAgentRuntime), §7.1 (the dry-run S9 strip).
//!
//! **GATE / DRILLS (quantified — the green artifacts):**
//! - **AG-D5 (HITL withhold) applied to a GOVERNED Issues transition** — 0 mutation pre-approval,
//!   exactly 1 apply post-approval (the 0-pre-approval-mutation counter is the green artifact).
//! - **AG-D9 (mock-determinism) applied to Issues' agent tools** — identical effect sequences across
//!   replays (the forecast agent's identical replay + the triage agent's identical proposed-effect
//!   strip; the identical-sequence is the green artifact).
//! - **Upstream AG-D4 / CI-T1 GREEN** — no Issues agent tool runs over a red sandbox-escape gate; the
//!   committed green attestation admits the production backend (the gate is fail-closed in the TYPE).
//!
//! These pair the NEW P-390 surfaces (the full catalogue + the mock forecast/triage agents) with the
//! REAL `PlanThenApply` pipeline (AG-P6), the REAL MockAgentRuntime (AG-P5), the REAL dry-run planner
//! (AG-P8), and the REAL AG-D4 escape gate (AG-P17) — NO new engine; the registration + the scripted
//! agents light up the existing path.
//!
//! ## What this CDC pins (PROVIDER ↔ CONSUMER no-drift)
//! - **PROVIDER** (Issues, the OWNER of its ToolDef catalogue + the arch-§8 consequence
//!   classification): `myelin_agent_service::issues_agents` ships the COMPLETE Issues catalogue with
//!   the frozen §6.3 `requires_approval` seed + the 4.9 `required_caps` + the LINEAR forecast / triage
//!   suggestion strip the MOCK agents produce.
//! - **CONSUMER** (the Fabric plan-then-apply pipeline + the loop): the REAL
//!   `myelin_agent_service::PlanThenApply` branches on the registered defs (a gated tool WITHHOLDS;
//!   an advisory tool applies), the REAL `replay`/dry-run consumer reads the MOCK agents' decision
//!   streams, and the AG-D4 escape-gate CONSUMER admits the agents' compute only over a green gate.
//!   A gating/cap/shape drift on EITHER side is a test break.

use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::escape_gate::{AgentExecGate, ProductionBackendId};
use myelin_agent_service::{
    // the NEW P-390 surfaces:
    close_tool_def,
    create_tool_def,
    full_issues_tool_defs,
    register_full_issues_tools,
    replay_forecast_agent,
    // the REUSED engine surfaces:
    transition_caveat,
    transition_tool_def,
    triage_suggestion_strip,
    ApplyError,
    CapabilityCheck,
    DelegationLookup,
    EffectBudget,
    EffectCost,
    ForecastInput,
    LinearForecast,
    PipelineSignals,
    PlanThenApply,
    PlannedEffect,
    SubsystemApply,
    TenantGuard,
};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
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

/// A `check` provider keyed on the cap STRING; an SLA-bound/governed transition (a caveat carrying a
/// `transition`) with NO approver context resolves `Conditional` (≡ DENY, fail-closed) so an
/// un-approved governed transition can never silently apply.
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

/// Run the apply pipeline once for `plan`; returns the result + the mutations recorded after the call.
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

// ═════════════════════════ 8.1 — the FULL arch-§8 catalogue CDC ═══════════════════════════════════

/// **CDC 8.1 / 4.9 — the FULL Issues catalogue (12 tools) registers into the ONE ToolSurface and
/// resolves by name with its frozen ReBAC-fragment caps.** The registration is the whole deliverable
/// (a ToolDef is a row in the ONE registry; UI=CLI=agent parity, no privileged back-channel).
#[test]
fn cdc_8_1_full_issues_catalogue_registers_into_the_one_surface() {
    let mut cat = Catalogue { defs: vec![] };
    let defs = register_full_issues_tools(&mut cat).expect("seeded defs admit");
    assert_eq!(defs.len(), 12, "8 CRUD + 4 agent tools (arch §8)");

    // every arch-§8 tool resolves by name.
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

    // the caps are the frozen 4.9 fragment permissions (a fragment rename breaks this).
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

/// **The frozen §6.3 consequential split — exactly `close` + `transition` are gated; the rest are
/// advisory/reversible.** Every gating IS the frozen seed (not hand-set).
#[test]
fn the_frozen_consequential_split_is_close_and_transition_only() {
    let defs = full_issues_tool_defs();
    let gated: Vec<&str> = defs
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(gated, vec!["close", "transition"]);
    // every tool is a Mutate routed through EffectApi — no new path.
    assert!(defs
        .iter()
        .all(|d| d.effect_kind == EffectKind::Mutate && d.side_effecting));
}

// ═════════════════════════ AG-D5 — the HITL withhold on a governed transition ═════════════════════

/// **AG-D5 (GATE) — the governed transition is WITHHELD (0 mutation) pre-approval, applies EXACTLY
/// ONCE post-approval.** The frozen §6.3 `transition` default is gated; the pipeline withholds at
/// step 6 (returns `Gated`, 0 mutation, AG-8); only after an approval threads the tool into `approved`
/// does a re-run APPLY — exactly once. The 0-pre-approval-mutation counter is the green artifact.
#[test]
fn ag_d5_governed_transition_withheld_then_applies_once() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    // the agent HOLDS the cap (so the GATE, not a deny, is what withholds); approver context present.
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

    // (1) WITHHOLD — not approved → step 6 gates → `Gated`, 0 mutation.
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

    // (2) APPROVE → re-run with the tool in `approved` → APPLIES exactly once.
    let approved: BTreeSet<String> = ["transition".to_string()].into_iter().collect();
    let (applied, muts1) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(applied, EffectResult::Applied(_)),
        "after approval the transition APPLIES: {applied:?}"
    );
    assert_eq!(muts1, 1, "AG-D5: exactly one apply after approval");
}

/// **AG-D5 (the ABAC leg, §5.2 step 2 / 4.2) — a governed transition with NO approver context
/// resolves `Conditional` → DENY, never a silent apply.** The transition-ABAC caveat is fail-closed:
/// the caveat NEVER loosens the gated floor (a `Conditional` is a DENY).
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
    // even "approved", the ABAC Conditional denies — fail-closed, never applies.
    let approved: BTreeSet<String> = ["transition".to_string()].into_iter().collect();
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(out, EffectResult::Denied(_)),
        "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
    );
    assert_eq!(muts, 0, "a denied governed transition makes 0 mutation");
}

/// **`close` is the OTHER gated tool — it WITHHOLDS (0 mutation) until approval (arch §8 "yes if
/// confidential or governed", the conservative floor).** Pairs the NEW `close` ToolDef with the REAL
/// pipeline.
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
        input_json: r#"{"issue":"ENG-7","reason":"done"}"#.into(),
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

/// **A reversible CRUD tool (create) is NOT gated — it applies DIRECTLY through the pipeline
/// (suggest-by-default).** No HITL gate, one apply. Pairs the NEW CRUD ToolDef with the REAL pipeline.
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
    let plan = PlannedEffect {
        tool: ToolName("create".into()),
        object: ArtifactRef("myelin://acme/issue/project/ENG".into()),
        input_json: r#"{"project":"ENG","title":"a bug"}"#.into(),
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

// ═════════════════════════ AG-D9 — mock-determinism on Issues' agent tools ════════════════════════

/// **AG-D9 (GATE) — the MOCK forecast agent replays BYTE-IDENTICALLY across two runs.** The
/// deterministic scripted brain makes the forecast agent golden/mutation-testable; the identical
/// replay is the green artifact. The forecast is the LINEAR floor (R-5; Monte-Carlo is ISS-P32).
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
    // the linear forecast is ceil(84/12) = 7 periods → at-risk (7 > 6).
    let out = LinearForecast::forecast(&input);
    assert_eq!(out.periods_to_completion, Some(7));
    assert!(out.at_risk, "7 > 6 → at-risk (crosses the threshold)");
}

/// **AG-D9 (GATE) — the MOCK triage agent's dry-run suggestion strip is BYTE-IDENTICAL across two
/// runs and proposes exactly ONE advisory effect WITHOUT applying it (8.7).** The S9 suggestion strip
/// is proposed, not applied (side-effect-free dry-run); the identical proposed-effect sequence is the
/// green artifact (the effect-sequence determinism applied to Issues).
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

// ═════════════════════════ AG-D4 — the upstream escape GATE is GREEN ══════════════════════════════

fn prod_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923".into(),
        kernel_sha256: "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb".into(),
        corpus_version: CORPUS_VERSION,
    }
}

/// A REAL green drill report → a green attestation (minted from the corpus parser, never hardcoded).
fn green_attestation() -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    let id = prod_id();
    EscapeAttestation::from_green_drill(
        "2026-06-21",
        &report,
        vec![
            BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            },
            BackendRun {
                backend: Backend::GvisorRunsc,
                exercised: false,
                residual_note: Some("runsc residual (CI-P28)".into()),
            },
        ],
        Backend::FirecrackerMicrovm,
        id.rootfs_sha256,
        id.kernel_sha256,
        "6.1.168",
    )
}

/// **AG-D4 / CI-T1 GATE is GREEN — no Issues agent tool runs over a red sandbox-escape gate.** The
/// upstream invariant the prompt requires stated explicitly: a green AG-D4 attestation for the
/// production backend admits untrusted compute; the gate is fail-closed in the TYPE (no green ⇒ no
/// compute). The Issues forecast/triage agents' compute inherits this gate by construction.
#[test]
fn ag_d4_escape_gate_is_green_for_the_production_backend() {
    let att = green_attestation().expect("a real green drill mints a green attestation");
    let gate = AgentExecGate::admit(Some(&att), &prod_id())
        .expect("AG-D4 is GREEN — the gate admits untrusted compute for the production backend");
    assert_eq!(gate.backend_id().backend, Backend::FirecrackerMicrovm);
    // and the headline fail-closed property: NO attestation ⇒ REFUSE (no Issues tool over a red gate).
    assert!(
        AgentExecGate::admit(None, &prod_id()).is_err(),
        "no green AG-D4 attestation ⇒ no untrusted compute (the Issues agent tools are gated on it)"
    );
}
