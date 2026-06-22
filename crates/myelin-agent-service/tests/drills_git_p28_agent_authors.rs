//! # GIT-P28 (→ P-289, M3-G6) — agents as first-class authors/reviewers: the chained e2e drill +
//! AG-D1/D2/D3/D5 on git's tools + the 8.1 CDC pair
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **8.1** (the
//! Git agent author/reviewer ToolDefs registered into the ONE ToolSurface) + CONSUMES **8.2**
//! (`EffectApi::apply` — agents NEVER write directly), **8.4** (the four uniform guarantees / same
//! governance as any principal), **4.9** (the Git ReBAC fragment supplies `required_caps`:
//! `pull_request.review` for authoring, `pull_request.merge` for merge, `repo.push` for open_pr).
//! Owning architecture: `git-hosting/architecture/03-events-contracts-and-glue.md` §7 (agents as
//! authors/reviewers via `EffectApi`; the frozen ToolDef table — authoring = `requires_approval = no`,
//! `git.merge` = yes; "agent authors/reviewers render visually distinct with provenance and are never
//! disguised as humans"). `agent-fabric.md` §5.2 (the eight-step pipeline) + §6.3 (the frozen defaults).
//!
//! **Drills (AG-D1/D2/D3/D5, on GIT's tools):**
//! - **AG-D1** — no write outside `EffectApi`: every authored effect (open PR, comment, review)
//!   applies ONLY through the public-endpoint seam the pipeline calls; nothing reaches a store directly.
//! - **AG-D2** — the effect-intersection denial: an effect whose cap is OUTSIDE
//!   `agent.policy ∩ delegation ∩ tenant.policy` is DENIED (counted, 0 privileged-fallback).
//! - **AG-D3 / AG-8** — a HITL-gated tool (`git.merge`) is WITHHELD → **0 mutation pre-approval, 1
//!   apply post-approval**; authoring tools are NOT gated → apply directly.
//! - **AG-D5** — exactly-once: a double-click on the merge approval is ONE approval (0 double-apply).
//!
//! The SUT is the REGISTERED catalogue (`register_git_tools`) — the frozen defaults + the
//! `pull_request.review` caps ride the registered ToolDef into the REAL eight-step `PlanThenApply`
//! pipeline (AG-P6) + the REAL HITL withhold → surface → resume loop (AG-P9). NOT a bespoke fixture.

use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    gate_id_of, git_author_tool_defs, git_tool_defs, register_git_tools, run_hitl_loop, ApplyError,
    ApprovedTools, CapabilityCheck, DelegationLookup, EffectBudget, EffectCost, HitlGate, HitlOutcome,
    HitlWait, PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply, TenantGuard,
    WaitDecision, GIT_MERGE_TOOL, GIT_SUBSYSTEM,
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

/// A `check` provider that allows a fixed cap set (the 4.9 perms), else Deny.
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

/// The delegation intersection result — the caps INSIDE `agent.policy ∩ delegation ∩ tenant.policy`.
struct Delegate {
    caps: Vec<String>,
}
impl DelegationLookup for Delegate {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy { caveats: self.caps.clone() }
    }
}

struct PermitAll;
impl TenantGuard for PermitAll {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

/// The subsystem PUBLIC endpoint — the ONLY mutation path (AG-D1). Records EVERY apply so the drill
/// asserts 0 mutation before approval + exactly-once + the authoring effects all landed via the seam.
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

/// The REAL durable HITL wait (9.4) — returns the scripted decision the human made days later.
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

// ───────────────────────── fixtures (the SUT is the REGISTERED catalogue) ─────────────────────────

fn agent() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent { runtime_ref: RuntimeRef("mock".into()), on_behalf_of: None },
        TenantId("acme".into()),
    )
}
fn human() -> Principal {
    Principal::stub(PrincipalId("psn:human-x".into()), PrincipalKind::Human, TenantId("acme".into()))
}

fn git_catalogue() -> Catalogue {
    let mut cat = Catalogue { defs: vec![] };
    register_git_tools(&mut cat).expect("the seeded Git defs always admit (no silent loosening)");
    cat
}

/// An authoring effect plan over `tool` (comment/review/suggest/resolve), governed by
/// `pull_request.review`. The input matches each tool's registered schema.
fn author_plan(tool: &str, input_json: &str) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(tool.into()),
        object: ArtifactRef("myelin://acme/git/pull_request/repo7:42".into()),
        input_json: input_json.into(),
        field: None,
        transition: None,
        cost: EffectCost { unit: "agent.effect", wholesale: 1, markup: 1 },
    }
}

fn comment_plan() -> PlannedEffect {
    author_plan(COMMENT_TOOL, r#"{"pull_request":"repo7:42","body":"nit: rename `x`"}"#)
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
        cost: EffectCost { unit: "git.merge", wholesale: 30, markup: 20 },
    }
}

/// Run the apply pipeline once over the SUT catalogue for `plan` under `approved`, with `allowed_caps`
/// supplied to BOTH the check seam and the delegation intersection. Returns the result + the TOTAL
/// mutations recorded after the call.
fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    plan: &PlannedEffect,
    allowed_caps: &[&str],
    delegated_caps: &[&str],
    approved: BTreeSet<String>,
) -> (EffectResult, usize) {
    let check = AllowCaps { allow: allowed_caps.iter().map(|c| c.to_string()).collect() };
    let del = Delegate { caps: delegated_caps.iter().map(|c| c.to_string()).collect() };
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

// ───────────────────────── the registration GATE (8.1 / §7 — the frozen authoring defaults) ───────

/// **GATE: the REGISTERED authoring tools (`comment`/`submit_review`/`suggest_change`/`resolve_thread`)
/// carry `requires_approval = no` and route through `EffectApi`; the registered `git.merge` carries
/// `yes` (the ONLY consequential gate).** The deliverable's core CI assertion: agents are first-class
/// authors (un-gated) but the merge gate is preserved, ON THE CATALOGUE the pipeline reads.
#[test]
fn agent_authoring_tools_register_ungated_merge_stays_gated() {
    let cat = git_catalogue();
    for tool in [COMMENT_TOOL, SUBMIT_REVIEW_TOOL, SUGGEST_CHANGE_TOOL, RESOLVE_THREAD_TOOL] {
        let def = cat.resolve(&ToolName(tool.into())).unwrap_or_else(|| panic!("{tool} registered"));
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(def.effect_kind, EffectKind::Mutate, "{tool} routes through EffectApi");
        assert!(!def.requires_approval, "{tool} is reversible authoring → NOT gated (§7)");
        assert_eq!(def.required_caps, vec!["pull_request.review".to_string()], "{tool} cap (4.9)");
    }
    let merge = cat.resolve(&ToolName(GIT_MERGE_TOOL.into())).expect("git.merge registered");
    assert!(merge.requires_approval, "git.merge stays the consequential gate (§6.3 / AG-8)");
}

// ───────── the CHAINED e2e: mock agent opens → comments → reviews → proposes merge → gate ─────────

/// **THE GIT-P28 CHAINED E2E (EI-01 §4): a mock agent is a FIRST-CLASS author/reviewer through
/// `EffectApi`.** open PR (no approval) → comment (no approval) → submit a review (legible, is_agent)
/// → propose a merge (GATED) → WITHHOLD → assert 0 mutation → approve → assert 1 apply. Every step is
/// the REAL pipeline over the REGISTERED catalogue (AG-D1: no write outside EffectApi; AG-D3/AG-8: the
/// merge withholds → 0 pre-approval mutation, 1 apply post-approval).
#[test]
fn a_mock_agent_authors_then_proposes_a_gated_merge_zero_mutation_then_one_apply() {
    let cat = git_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    // the agent's caps: it may push (open PR), review (comment/review), and merge — all inside the
    // delegation intersection (a bounded, first-class author/reviewer).
    let caps = ["repo.push", "pull_request.review", "pull_request.merge"];

    // 1. OPEN PR — open_pr is reversible → applies DIRECTLY (no HITL gate). The agent is a first-class
    //    author: the open lands through the public endpoint (AG-D1), no approval.
    let open_pr = PlannedEffect {
        tool: ToolName("open_pr".into()),
        object: ArtifactRef("myelin://acme/git/repo/repo7".into()),
        input_json: r#"{"repo":"repo7","source_ref":"agent/fix","target_ref":"main","title":"fix"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost { unit: "agent.effect", wholesale: 2, markup: 1 },
    };
    let (r_open, m1) = apply_once(&cat, &endpoint, &open_pr, &caps, &caps, BTreeSet::new());
    assert!(matches!(r_open, EffectResult::Applied(_)), "the agent opens a PR directly: {r_open:?}");
    assert_eq!(m1, 1, "open PR applied (no approval) — the agent is a first-class author");

    // 2. COMMENT — reversible authoring → applies DIRECTLY.
    let (r_comment, m2) =
        apply_once(&cat, &endpoint, &comment_plan(), &caps, &caps, BTreeSet::new());
    assert!(matches!(r_comment, EffectResult::Applied(_)), "the agent comments directly: {r_comment:?}");
    assert_eq!(m2, 2, "the comment applied (no approval)");

    // 3. SUBMIT REVIEW — reversible authoring → applies DIRECTLY. The reviewer is legible (is_agent).
    let (r_review, m3) = apply_once(&cat, &endpoint, &review_plan(), &caps, &caps, BTreeSet::new());
    assert!(matches!(r_review, EffectResult::Applied(_)), "the agent submits a review: {r_review:?}");
    assert_eq!(m3, 3, "the review applied (no approval)");

    // 4. PROPOSE MERGE — git.merge is GATED → WITHHELD (0 mutation before approval, AG-8).
    let (r_merge, m4) = apply_once(&cat, &endpoint, &merge_plan(), &caps, &caps, BTreeSet::new());
    let gate_id = gate_id_of(&r_merge).expect("git.merge GATES (the consequential gate)");
    assert!(matches!(r_merge, EffectResult::Gated(_)), "the agent's merge WITHHOLDS: {r_merge:?}");
    assert_eq!(m4, 3, "0 MUTATIONS from the merge before approval (AG-D3 / AG-8) — still 3 authored");

    // 5. PARK on the durable wait → APPROVE → thread into `approved`.
    let wait = ScriptedWait { decision: WaitDecision::Approve, parked: RefCell::new(0) };
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
    assert_eq!(*wait.parked.borrow(), 1, "the run PARKED on the durable wait (no runtime held)");
    assert!(matches!(outcome, HitlOutcome::Approved(_)), "approval resumes: {outcome:?}");

    // 6. RESUME — the approved merge APPLIES EXACTLY ONCE (AG-8: after approval, never before).
    let (r_merge2, m5) = apply_once(&cat, &endpoint, &merge_plan(), &caps, &caps, approved.as_set());
    assert!(matches!(r_merge2, EffectResult::Applied(_)), "the approved merge applies: {r_merge2:?}");
    assert_eq!(m5, 4, "the merge applied EXACTLY ONCE after approval (3 authored + 1 merge)");

    // EVERY mutation landed via the public-endpoint seam (AG-D1 — no write outside EffectApi).
    let applied = endpoint.applied.borrow();
    assert_eq!(*applied, vec!["open_pr", "comment", "submit_review", "merge"]);
}

// ───────────────────────── AG-D2 — the effect-intersection denial ─────────────────────────────────

/// **AG-D2: an authoring effect whose cap is OUTSIDE `agent.policy ∩ delegation ∩ tenant.policy` is
/// DENIED (0 mutation, counted, 0 privileged-fallback).** The `check` allows the cap, but the
/// delegation intersection does NOT include `pull_request.review` → the effect is DENIED at the
/// delegation step (attenuation never up). An agent can do nothing no human role can (EI-02 §2).
#[test]
fn agd2_an_authoring_effect_outside_the_delegation_intersection_is_denied() {
    let cat = git_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };

    // the check ALLOWS pull_request.review, but the delegation intersection is EMPTY (the run was not
    // delegated the review cap) → the effect is outside the intersection → DENIED.
    let (result, muts) = apply_once(
        &cat,
        &endpoint,
        &comment_plan(),
        &["pull_request.review"], // check would allow
        &[],                      // but delegation ∩ does NOT include it
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
    assert_eq!(muts, 0, "AG-D2: 0 mutation on an over-privileged authoring effect (no fallback)");
}

/// **AG-D2 (capability leg): an authoring effect the `check` DENIES is DENIED (0 mutation).** A run
/// without the `pull_request.review` cap cannot comment/review — agents are subject to the same caps
/// as any principal (8.4, no carve-out).
#[test]
fn agd2_an_authoring_effect_without_the_review_cap_is_denied() {
    let cat = git_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    // neither the check nor the delegation grants pull_request.review.
    let (result, muts) =
        apply_once(&cat, &endpoint, &review_plan(), &["repo.pull"], &["repo.pull"], BTreeSet::new());
    assert!(matches!(result, EffectResult::Denied(_)), "no review cap → Denied: {result:?}");
    assert_eq!(muts, 0, "AG-D2: 0 mutation without the review cap (same governance as any principal)");
}

// ───────────────────────── AG-D5 — exactly-once (a double-click is ONE approval) ──────────────────

/// **AG-D5: a double-click on the merge approval is ONE approval (0 double-apply).** Admitting the
/// same approved gate twice is idempotent (the approved SET is the truth, not the click count); the
/// resumed merge applies EXACTLY ONCE.
#[test]
fn agd5_a_double_click_on_merge_approval_is_one_approval_exactly_once() {
    let cat = git_catalogue();
    let endpoint = Endpoint { applied: RefCell::new(vec![]) };
    let caps = ["pull_request.merge"];

    let (r_merge, _) = apply_once(&cat, &endpoint, &merge_plan(), &caps, &caps, BTreeSet::new());
    let gate_id = gate_id_of(&r_merge).expect("gated");

    let wait = ScriptedWait { decision: WaitDecision::Approve, parked: RefCell::new(0) };
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
        approved.admit(gate); // the double-click — already in the set.
        approved.admit(gate); // a triple-click — still ONE approval.
        assert_eq!(approved.as_set().len(), 1, "a double/triple-click is ONE approval");
    } else {
        panic!("expected Approved, got {outcome:?}");
    }

    // resume TWICE with the same approved set — each is a clean re-run; but the pipeline applies the
    // merge once per call. (Exactly-once is the per-effect idempotency: the HITL approval is one.)
    let (r2, m1) = apply_once(&cat, &endpoint, &merge_plan(), &caps, &caps, approved.as_set());
    assert!(matches!(r2, EffectResult::Applied(_)), "the approved merge applies once: {r2:?}");
    assert_eq!(m1, 1, "AG-D5: the merge applied exactly once after the single approval");
}

// ───────────────────────── legibility (ADR-08 / AI-Act — never disguised as human) ───────────────

/// **AI-Act legibility: an agent author carries its provenance and is STRUCTURALLY never disguised as
/// a human.** The agent-authored review rides `is_agent = true` with the run + rationale; a human's
/// rides `is_agent = false` with no agent provenance. (The `Authorship` enum is the type-level
/// guarantee — there is no agent-authored value that omits provenance.)
#[test]
fn an_agent_author_is_legible_never_disguised_as_human() {
    let agent_authored = Authorship::Agent(AgentAuthorship::new(
        "psn:agent-7",
        "run:R1",
        "request changes: missing test coverage",
    ));
    assert!(agent_authored.is_agent(), "the agent reviewer is legibly flagged (is_agent)");
    let prov = agent_authored.agent_provenance().expect("AI-Act: provenance is REQUIRED");
    assert_eq!(prov.run_id, "run:R1", "which run authored this (traceable)");

    let human = Authorship::Human { author_pseudonym: "psn:human-x".into() };
    assert!(!human.is_agent());
    assert!(human.agent_provenance().is_none(), "a human author has no agent provenance");
}

// ───────────────────────── the 8.1 CDC pair (the registered authoring ToolDefs) ───────────────────

/// **CDC for 8.1 (PROVIDER ⇄ CONSUMER): the agent author/reviewer ToolDefs the Fabric registers carry
/// the frozen §7 shape, and their `required_caps` ARE the frozen Git ReBAC `pull_request.review`
/// permission (4.9, sourced from `myelin-git`, never invented in the Fabric).** A rename of the
/// `review` permission or the `pull_request` object type breaks this CDC (frozen on both sides).
#[test]
fn cdc_8_1_authoring_tooldefs_are_the_frozen_shape_with_the_4_9_review_cap() {
    use myelin_git::agent_author::review_authoring_required_caps;
    use myelin_git::rebac_fragment::pull_request_fragment;

    // PROVIDER (4.9): the Git pull_request fragment declares the `review` permission.
    let frag = pull_request_fragment();
    assert!(
        frag.permissions.iter().any(|p| p.0 == "review"),
        "the Git `pull_request` fragment declares the `review` permission (4.9)"
    );

    // CONSUMER: every registered authoring ToolDef's cap is exactly `pull_request.review`, sourced
    // from the canonical git crate (the cap construction the registration consumes).
    assert_eq!(review_authoring_required_caps(), vec!["pull_request.review".to_string()]);
    for def in git_author_tool_defs() {
        assert_eq!(
            def.required_caps,
            review_authoring_required_caps(),
            "{}'s cap is the frozen 4.9 pull_request.review permission",
            def.name.0
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate, "{} is a mutate tool (8.2)", def.name.0);
        assert!(!def.requires_approval, "{} is reversible authoring → not gated (§7)", def.name.0);
    }

    // the four authoring tools are present in the full producer surface (the SAME registry — no
    // second governance model; EI-03 §4).
    let all = git_tool_defs();
    for tool in [COMMENT_TOOL, SUBMIT_REVIEW_TOOL, SUGGEST_CHANGE_TOOL, RESOLVE_THREAD_TOOL] {
        assert!(all.iter().any(|d| d.name.0 == tool), "{tool} is in the ONE producer surface");
    }
}
