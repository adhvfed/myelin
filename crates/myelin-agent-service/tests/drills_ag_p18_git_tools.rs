//! # AG-P18 (→ P-267, M3) — the Git producer ToolDefs drill + the KN-D11 git-merge leg + the 4.9 CDC
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **8.1**
//! (OWNED — the Git producer ToolDefs registered into the ONE ToolSurface) + CONSUMES **4.9** (the
//! Git ReBAC fragment supplies the `required_caps`: `pull_request.merge` / `repo.push`). Owning
//! architecture: `agent-fabric.md` §6.1 (the ONE catalogue) + §6.3 (the FROZEN `requires_approval`
//! defaults — `git.merge` = yes, `open_pr` = no) + §5.0/§5.2 (a `mutate` ToolDef routes through
//! EffectApi; a `requires_approval` tool withholds → `Gated`).
//!
//! **Drill — KN-D11 (the Git-merge leg, a Fabric-loop assertion):** an agent `git.merge` is
//! GOVERNED — **0 ungoverned merges / 0 mutations before approval / 0 double-apply**; a double-click
//! is ONE approval; `open_pr` applies DIRECTLY (no gate). This pairs the REGISTERED Git ToolDefs
//! (AG-P18, the SUT) with the REAL eight-step `PlanThenApply` pipeline (AG-P6) + the REAL HITL
//! withhold → surface → resume loop (AG-P9), so the chained loop is proven end-to-end on the exact
//! tools the catalogue registers (NOT a bespoke test fixture — `git_tool_defs()` IS the SUT).

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

/// A `check` provider that allows a fixed cap set (the 4.9 perms the Git tools require), else Deny.
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

/// The subsystem PUBLIC endpoint — the ONLY mutation path; records EVERY apply so the drill can
/// assert 0 mutation before approval + exactly-once.
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

/// A REAL provider on the 9.4 durable HITL wait — returns the scripted decision the human made days
/// later. A `parks` counter records that the run PARKED (state=waiting holds no runtime).
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

// ───────────────────────── fixtures (the SUT is git_tool_defs(), not a local def) ────────────────

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

/// A catalogue holding the REGISTERED Git producer ToolDefs (the SUT — register_git_tools is the
/// deliverable, so the drill registers through it, never hand-building the defs).
fn git_catalogue() -> Catalogue {
    let mut cat = Catalogue { defs: vec![] };
    register_git_tools(&mut cat).expect("the seeded Git defs always admit (no silent loosening)");
    cat
}

fn merge_plan() -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(GIT_MERGE_TOOL.into()), // "merge" under subsystem "git" (the §6.3 key)
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

/// Run the apply pipeline once over the SUT catalogue for `plan` under the given `approved` set;
/// returns the result + the TOTAL mutations the endpoint recorded after the call.
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

// ───────────────────────── the registration GATE (8.1 / §6.3 — the frozen defaults on the wire) ───

/// **GATE: the REGISTERED `git.merge` carries `requires_approval = yes` and routes through EffectApi;
/// `open_pr` carries `no` and applies directly (the frozen §6.3 defaults, ON THE CATALOGUE).** This is
/// the deliverable's core CI assertion: the defaults are not just in the seed table — they ride the
/// registered ToolDef the pipeline reads.
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
        "open_pr carries requires_approval = no (§6.3 — reversible)"
    );
    assert_eq!(pr.effect_kind, EffectKind::Mutate);
    assert_eq!(pr.required_caps, open_pr_required_caps());
    assert_eq!(pr.required_caps, vec!["repo.push".to_string()], "4.9 cap");
}

// ───────────────────────── KN-D11 git-merge leg — withhold (0 mutation) → approve → ONE merge ─────

/// **KN-D11 (the Git-merge leg): a registered `git.merge` is GOVERNED end-to-end — WITHHELD (0
/// mutation before approval), the run PARKS, an APPROVAL arrives, the resume threads the tool into
/// `approved`, a re-run APPLIES EXACTLY ONCE; a DOUBLE-CLICK on approve is ONE approval (0
/// double-apply).** The SUT is the catalogue `register_git_tools` built — the frozen default rides
/// the registered def into the REAL pipeline.
#[test]
fn knd11_git_merge_is_governed_zero_ungoverned_zero_double_apply() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let caps = ["pull_request.merge"];

    // 1. WITHHOLD — a fresh run (empty `approved`) proposes the registered git.merge → GATED.
    let (result, muts_before) = apply_once(&cat, &endpoint, &merge_plan(), &caps, BTreeSet::new());
    let gate_id =
        gate_id_of(&result).expect("git.merge is requires_approval → it GATES (0 ungoverned)");
    assert!(
        matches!(result, EffectResult::Gated(_)),
        "the registered git.merge WITHHOLDS: {result:?}"
    );
    assert_eq!(
        muts_before, 0,
        "0 MUTATIONS before approval (KN-D11 — the merge did NOT apply)"
    );

    // 2 + 3 + 4. PARK on the durable wait (9.4) → APPROVE (days later) → thread into `approved`.
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

    // a DOUBLE-CLICK on approve is ONE approval — admitting the same approved gate twice is idempotent
    // (the set is the truth, not the click count).
    if let HitlOutcome::Approved(ref gate) = outcome {
        approved.admit(gate); // re-admit (the double-click) — the gate is already in the set.
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double-click is ONE approval (the set holds one tool)"
        );
    }

    // 5. RESUME — the re-run with the populated `approved` set passes step 6 → APPLIES EXACTLY ONCE.
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

/// **KN-D11 (rejection leg): a registered `git.merge` REJECTED never applies — 0 mutation across the
/// whole flow (AG-8). The merge is governed; a decline is honoured.**
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
        "a rejected merge still GATES — never applies"
    );
    assert_eq!(muts, 0, "0 MUTATIONS across the entire reject flow (AG-8)");
}

// ───────────────────────── open_pr applies DIRECTLY (no gate) ─────────────────────────────────────

/// **`open_pr` is reversible (§6.3 = no) → it applies DIRECTLY through the pipeline: schema ✓ → cap ✓
/// → … → NO gate → APPLY, no HITL withhold.** The pipeline never returns `Gated` for `open_pr`; the
/// effect mutates once on the first call (the contrast that proves the gate is a per-tool frozen
/// default, not a blanket policy).**
#[test]
fn open_pr_applies_directly_no_hitl_gate() {
    let cat = git_catalogue();
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    // a FRESH run (empty `approved`) — open_pr applies on the FIRST call (no withhold).
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

// ───────────────────────── the consumer CDC for 4.9 (the Git ReBAC fragment supplies caps) ────────

/// **CONSUMER CDC for 4.9 — the Git producer ToolDefs' `required_caps` ARE the frozen Git ReBAC
/// fragment permissions, sourced from `myelin-git` (the PROVIDER), never invented in the Fabric.**
/// A rename of the `merge`/`push` permission or the `pull_request`/`repo` object type in the Git
/// fragment breaks this CDC (the contract is the names, frozen on both sides).
#[test]
fn cdc_4_9_required_caps_are_the_git_rebac_fragment_permissions() {
    use myelin_git::rebac_fragment::{pull_request_fragment, repo_fragment};

    // PROVIDER (4.9): git.merge's cap is the `pull_request.merge` permission the fragment declares.
    let pr_frag = pull_request_fragment();
    assert!(
        pr_frag.permissions.iter().any(|p| p.0 == "merge"),
        "the Git ReBAC `pull_request` fragment declares the `merge` permission (4.9)"
    );
    // CONSUMER: the registered git.merge ToolDef's required_cap is exactly `<object_type>.merge`.
    assert_eq!(
        git_merge_tool_def().required_caps,
        vec!["pull_request.merge".to_string()]
    );

    // PROVIDER (4.9): open_pr's cap is the `repo.push` permission the fragment declares.
    let repo_frag = repo_fragment();
    assert!(
        repo_frag.permissions.iter().any(|p| p.0 == "push"),
        "the Git ReBAC `repo` fragment declares the `push` permission (4.9)"
    );
    // CONSUMER: the registered open_pr ToolDef's required_cap is exactly `<object_type>.push`.
    assert_eq!(
        open_pr_tool_def().required_caps,
        vec!["repo.push".to_string()]
    );
}

/// **NO-NEW-ENGINE check (EI-03 §4 / EI-01 §7): the whole Git producer surface is `git_tool_defs()`.**
/// At AG-P18 (P-267) this was exactly two `mutate` ToolDefs (`git.merge`, `open_pr`). GIT-P27 (P-283)
/// EXTENDED it with the two code-executing tools on the unified sandbox: `git.history_rewrite` (a
/// gated `mutate` → EffectApi — the audited erasure-admin op, 10.6) and `git.scip_index` (a `compute`
/// tool → the sandbox the AG-D4 escape drill gates). They are still PURE registration data (no second
/// apply/gate engine) — the routing/gating/sandbox machinery is the existing pipeline. This test pins
/// the two ORIGINAL producer mutations are present + correctly seeded; the full four-tool surface is
/// pinned in `git_tools.rs`'s `all_four_git_tools_are_seeded_from_the_frozen_defaults`.
#[test]
fn git_producer_surface_is_a_projection() {
    let defs = git_tool_defs();
    // the two producer MUTATIONS route through EffectApi (plan-then-apply).
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
    // the code-executing tools (P-283) are registered into the SAME surface (no second registry).
    assert!(
        defs.iter().any(|d| d.name.0 == "history_rewrite"),
        "git.history_rewrite registered"
    );
    assert!(
        defs.iter().any(|d| d.name.0 == "scip_index"),
        "git.scip_index registered"
    );
}
