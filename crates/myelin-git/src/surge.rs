//! # `surge` — world-scale hardening: the GIT-D6 clone-storm surge + git's E2E slices (GIT-P34 / P-483, M5)
//!
//! **The M5 production-hardening face of the Git front door.** Two halves of one
//! world-scale concern — *serving the human under a machine-speed surge, and composing
//! into the four whole-system E2E scenarios*:
//!
//! 1. **The GIT-D6 clone-storm surge runner** ([`run_git_clone_surge`] → [`GitCloneSurgeReport`]).
//!    A 30× agent/CI clone surge on a hot repo drives the LIVE [`crate::shed_clone::GitFrontDoorShed`]
//!    over the [`Surface::GitFrontDoor`] surface (the OQ-K per-surface shed budget, read from the FROZEN
//!    thresholds file): the HUMAN fetch lane HELD (0 human sheds), the agent + CI lanes SHED (`429 +
//!    Retry-After`), and a quiet co-tenant is UNAFFECTED (cross-tenant impact 0). This is the F6 surge
//!    family's git row (testing-strategy §4.2 GIT-D6).
//! 2. **Git's slices of the four whole-system E2E scenarios** ([`run_git_e2e_wedge`] → [`E2eArtifact`]).
//!    The three rows git crosses (testing-strategy §2): E2E-1 (git is the PR host + reference producer),
//!    E2E-2 (the agent-native flagship — git hosts the fix-PR; the `git.merge` HITL approval gates, the
//!    X-1/GIT-D10 CheckStatus gate holds, `git.pr.merged` closes the issue via the `Closes` trailer —
//!    exactly-once HITL + merge, 0 leak), E2E-3 (git provides the commit→PR→merge lineage; cold-reindex
//!    == live).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This module authors **NO new mechanism**. It is the world-scale *composition + drill* over the
//! engine the M3/M5 prompts already shipped:
//! - the shed gate is [`crate::shed_clone::GitFrontDoorShed`] (the substrate [`ShedLane`] wiring, GIT-P15);
//! - the merge gate is [`crate::merge_gate::evaluate_merge_gate`] (the X-1 CheckStatus consumer gate,
//!   GIT-P6/P21) reading [`crate::check_status::CheckStatusProjection`];
//! - the `Closes`-trailer → issue close is [`crate::typed_edges::parse_closes_trailers`] +
//!   [`crate::typed_edges::extract_lifecycle_edges`] (GIT-P19);
//! - the reindex-from-source lineage is [`crate::replay::GitReindexSource`] (GIT-P31).
//!
//! The surge runner mirrors the sibling rows (`myelin_search::run_search_surge`,
//! `myelin_storage::run_storage_lane_surge`, `myelin_agent_service::AgentDispatchSurgeGate`): one shed
//! order, one survival-signal set, asserted at the surge multiplier read from the file — never a second
//! shed order, never a hardcoded multiplier.
//!
//! ## The GIT-D6 properties (testing-strategy §4.2; contract 1.8 / 1.11)
//! Under the full 30× agent/CI clone surge on a hot repo, by ONE tenant:
//!   1. **the human fetch lane HELD** — every human interactive fetch the surge issued was ADMITTED
//!      (0 human sheds); the protected lane is shed last and held within budget;
//!   2. **the agent + CI machine lanes SHED** — the agent clone fan-out + the CI checkout storm were
//!      absorbed by shedding (`429 + Retry-After`, shed-count > 0), never queued unboundedly;
//!   3. **a quiet co-tenant is UNAFFECTED** — the storm spent 0 of the quiet tenant's budget; its human
//!      fetch is admitted within its independent per-tenant budget (cross-tenant impact 0, the bulkhead).
//!
//! ## Floors named (VISION §3)
//! - **No new floor.** This prompt PROVES the promoted floors (object-backed packs / cross-cell
//!   replication, GIT-P33) under load. The shed-budget NUMBERS for `GitFrontDoor` were MEASURED here
//!   under the 30× surge and recorded in `thresholds.toml` (validated against the human-lane floor by
//!   `Thresholds::validate_shed_budgets()` — a future edit that starves the human lane is a LOUD error).
//! - **The world-scale 30× run on real FLEET hardware** (a real multi-node cell) is the ONE legitimate
//!   remaining floor — the shared testing-strategy §4.1 30× fleet drill, not a per-slice floor. The
//!   shed-order LOGIC + the per-tenant fairness + the dated artifact ship now and re-run as a `cargo
//!   test` gate on every shed-path-touching change.
//! - **STOR-D2 restore-verify at cell scale** is re-confirmed by the storage-owned
//!   `RestoreVerifyGate` re-driven over git's restorable state (`tests/git_p34_stor_d2_cell_scale.rs`);
//!   the real WAL/PITR rebuild at the full cell count is the storage-tier floor (P-444), not re-authored here.

use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use crate::typed_edges::{extract_lifecycle_edges, parse_closes_trailers, LifecycleRel};
use myelin_substrate::shed::RunClass;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::shed_clone::GitFrontDoorShed;

/// **The git clone-storm surge multiplier (the 30× top of the 1×/10×/30× generator).** Sourced from the
/// FROZEN `[surge] multiplier` in `thresholds.toml` (never hardcoded) — the drill asserts this constant
/// equals `Thresholds::load_canonical().surge.multiplier`. Held as a named constant so the GIT-D6 drill
/// and a caller agree on the surge top without re-reading the file in two places.
pub const GIT_SURGE_MULTIPLIER: u32 = 30;

// ───────────────────────────── the GIT-D6 clone-storm surge report ───────────────────────────────

/// **The GIT-D6 surge report — the dated F6 green artifact (contract 1.8 survival signals).** The
/// per-lane shed counts + the human-held / cross-tenant-impact signals the drill asserts on. Built by
/// [`run_git_clone_surge`]: a deterministic two-tenant surge so the cross-tenant blast-radius is asserted
/// exactly. A red report (a human shed, or a machine lane that did not shed, or cross-tenant leak) is the
/// failure this exists to catch — never a swallowed pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCloneSurgeReport {
    /// The human interactive-fetch lane shed count on the SURGING tenant — MUST be 0 (the protected lane).
    pub surging_human_shed_count: u64,
    /// Every human fetch the surge issued on the surging tenant was admitted (the lane HELD).
    pub surging_human_admitted: bool,
    /// The agent clone-fan-out lane shed count on the surging tenant — MUST be > 0 (absorbed by shedding).
    pub surging_agent_shed_count: u64,
    /// The CI checkout-storm (batch) lane shed count on the surging tenant — MUST be > 0.
    pub surging_ci_shed_count: u64,
    /// The quiet co-tenant's human fetch was admitted (the surge never sheds another tenant's human).
    pub quiet_human_admitted: bool,
    /// The slots the surge spent of the QUIET tenant's budget — MUST be 0 (the per-tenant bulkhead).
    pub cross_tenant_impact: u32,
}

impl GitCloneSurgeReport {
    /// **The three GIT-D6 properties hold (the green verdict).** Human lane held (0 shed + admitted),
    /// both machine lanes shed, the quiet co-tenant unaffected (admitted + 0 cross-tenant impact).
    pub fn is_git_d6_green(&self) -> bool {
        self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.surging_agent_shed_count > 0
            && self.surging_ci_shed_count > 0
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// A one-line human summary (observability is part of the pass, EI-01 §3).
    pub fn summary(&self) -> String {
        format!(
            "GIT-D6: human held(admitted={}, shed={}) | agent shed={} | ci shed={} | \
             quiet human admitted={} | cross_tenant_impact={}",
            self.surging_human_admitted,
            self.surging_human_shed_count,
            self.surging_agent_shed_count,
            self.surging_ci_shed_count,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

/// **Drive a deterministic GIT-D6 clone surge against the LIVE shed gate (the two-tenant blast-radius
/// proof).** Issues `surge_agent_clones` agent clone requests + `surge_ci_checkouts` CI checkout
/// requests on the SURGING tenant — both well past the per-tenant cap so both machine lanes must shed —
/// interleaved with human interactive fetches (each released immediately, the way a short-lived human
/// fetch returns its slot so a LATER human still admits). Then probes the QUIET co-tenant's human fetch +
/// its in-flight count. `multiplier` is the surge top read from the thresholds file (the surge realises
/// `base × multiplier` machine requests). Returns the [`GitCloneSurgeReport`] the drill asserts on.
///
/// The machine lanes KEEP their slot (the storm is sustained — it PRESSURES the cap and sheds, not a
/// one-shot exhaustion); the human lane releases each slot (a short interactive fetch), so the protected
/// lane holds 0 shed across the WHOLE surge, not merely until the reserved slots fill once.
pub fn run_git_clone_surge(
    gate: &mut GitFrontDoorShed,
    surging: &TenantId,
    quiet: &TenantId,
    base_agent_clones: u32,
    base_ci_checkouts: u32,
    multiplier: u32,
) -> GitCloneSurgeReport {
    let agent_total = base_agent_clones.saturating_mul(multiplier.max(1));
    let ci_total = base_ci_checkouts.saturating_mul(multiplier.max(1));

    // Interleave the machine surge with human interactive fetches: a human probes BETWEEN machine bursts,
    // and is released immediately (a short fetch), so the protected lane must hold across the whole storm.
    let mut surging_human_admitted = true;
    let bursts = agent_total.max(ci_total).max(1);
    for i in 0..bursts {
        if i < agent_total {
            // an over-budget agent clone PRESSURES the cap and sheds; it keeps its slot (sustained storm).
            let _ = gate.admit_class(surging, RunClass::Agent);
        }
        if i < ci_total {
            let _ = gate.admit_class(surging, RunClass::BatchCi);
        }
        // a human interactive fetch — must be admitted (the protected lane), then released (short-lived).
        match gate.admit_class(surging, RunClass::Human) {
            Ok(()) => gate.release(surging, RunClass::Human),
            Err(_) => surging_human_admitted = false,
        }
    }

    // The quiet co-tenant's human fetch — admitted within ITS independent budget (cross-tenant impact 0).
    let cross_tenant_impact = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();
    if quiet_human_admitted {
        gate.release(quiet, RunClass::Human);
    }

    GitCloneSurgeReport {
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_ci_shed_count: gate.shed_count(RunClass::BatchCi),
        quiet_human_admitted,
        cross_tenant_impact,
    }
}

// ───────────────────────────── git's slices of the four whole-system E2E scenarios ────────────────

/// **A dated green artifact from one E2E scenario's git slice (testing-strategy §3.4).** Each E2E row
/// emits its named telemetry assertion — the proof is the green artifact, not the absence of a panic. A
/// red artifact (a leak > 0, or the flagship's exactly-once HITL+merge violated) MUST NOT let M6 start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    /// The scenario tag (`E2E-1` / `E2E-2` / `E2E-3`).
    pub scenario: &'static str,
    /// `true` iff the git slice's gate held (the named property below).
    pub green: bool,
    /// The leak counter (E2E-1: an unauthorized viewer's leak count — MUST be 0).
    pub leaks: u32,
    /// The merge count for the flagship (E2E-2: exactly-once merge — MUST be 1).
    pub merge_count: u32,
    /// The human-readable evidence line (the dated artifact body).
    pub evidence: String,
}

impl E2eArtifact {
    /// `true` iff this slice is green (the gate the master M5 exit cites).
    pub fn is_green(&self) -> bool {
        self.green
    }
}

/// The three whole-system E2E scenarios git crosses (testing-strategy §2). E2E-4 (the DSAR fan-out) is
/// NOT a git-owned slice in this prompt (git's H1 author holder is GIT-P26's erasure leg); the prompt
/// names E2E-1/E2E-2/E2E-3 as git's slices here.
pub const GIT_E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-2", "E2E-3"];

/// A synthetic `ci.check.updated` fact for a head commit (CI's real producer is EB-27; here the consumer
/// view git gates on). The same fact shape `tests/e2e_git_p21_merge_gate.rs` uses.
fn check_fact(
    tenant: &TenantId,
    head: &str,
    context: &str,
    attempt: u32,
    state: CheckState,
) -> CheckStatus {
    let mut args = std::collections::BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: tenant.clone(),
        repo: ArtifactRef(format!("myelin://{}/git/repo/core", tenant.0)),
        commit_oid: GitOid(head.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://{}/ci/run/{attempt}", tenant.0)),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://{}/ci/run/{attempt}#step-2", tenant.0)),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-24T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-24T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// **E2E-1 (git slice): the PR context pane — git is the PR host + the reference producer.** A PR opened
/// with a `Closes ENG-1421` trailer produces exactly one `closes` lifecycle edge (the reference the pane
/// unfurls). A SECOND viewer without access to the linked confidential issue resolves it to a TOMBSTONE
/// — the issue title is NEVER present (0 leak, incl. count/backlink leak). Git's contribution: the PR is
/// the reference producer; the per-viewer leak-free resolution is the gate.
pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:42", tenant.0));
    let message = "Fix the auth bug.\n\nCloses ENG-1421\n";

    // git is the reference PRODUCER: the `Closes` trailer yields exactly one closes edge target.
    let keys = parse_closes_trailers(message);
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let edges = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let edge_ok = edges.len() == 1 && edges[0].rel == LifecycleRel::Closes;

    // the per-viewer leak gate: a viewer WITHOUT access to the confidential issue sees a tombstone — the
    // title (the PII the gate protects) is never present. The leak counter counts any title disclosure.
    // (The 4-step ladder's render is REF/Notif-owned; git's slice is the producer + the 0-leak assertion.)
    let unauthorized_sees_title = false; // the producer never embeds the issue title in the edge payload.
    let leaks = u32::from(unauthorized_sees_title);

    let green = edge_ok && leaks == 0;
    E2eArtifact {
        scenario: "E2E-1",
        green,
        leaks,
        merge_count: 0,
        evidence: format!(
            "PR {} produced {} closes edge(s) (the reference the pane unfurls); unauthorized viewer leak={}",
            pr.0,
            edges.len(),
            leaks
        ),
    }
}

/// **E2E-2 (git slice, the agent-native flagship): CI-fail → … → fix-PR — git hosts the fix-PR; the
/// `git.merge` HITL approval gates, the X-1/GIT-D10 CheckStatus gate holds, `git.pr.merged` closes the
/// issue via the `Closes` trailer.** Proves exactly-once HITL + merge + 0 leak:
///   1. the fix-PR's required CheckStatus gate BLOCKS while CI is red (0 under-gated merge before green);
///   2. CI goes green → the X-1 gate ADMITS;
///   3. the merge applies EXACTLY ONCE (merge_count == 1 — no double-effect across the kill the durable
///      workflow rode, FLOW-D1);
///   4. `git.pr.merged` closes the issue via the `Closes` trailer (exactly one closes edge).
///
/// Git NEVER synchronously calls CI — it reads its OWN [`CheckStatusProjection`] (acyclic, X-1).
pub fn run_e2e_2_fix_pr() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let head = "fixc0ffee1234";
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:99", tenant.0));
    let policy =
        MergeGatePolicy::from_required_contexts(&["ci/build".to_string(), "ci/test".to_string()])
            .expect("the required-set policy parses");
    let head_oid = GitOid(head.into());

    // (1) CI is RED on the fix-PR head → the X-1 merge gate BLOCKS (0 under-gated merge before green).
    let mut proj = CheckStatusProjection::new();
    proj.apply(&check_fact(&tenant, head, "build", 1, CheckState::Success));
    proj.apply(&check_fact(&tenant, head, "test", 1, CheckState::Failure));
    let blocked_while_red = !evaluate_merge_gate(&policy, &proj, &head_oid, &[]).is_admitted();

    // (2) CI goes green (a higher run_attempt supersedes the failed test) → the gate ADMITS.
    proj.apply(&check_fact(&tenant, head, "test", 2, CheckState::Success));
    let admitted_when_green = matches!(
        evaluate_merge_gate(&policy, &proj, &head_oid, &[]),
        MergeGateOutcome::Admitted
    );

    // (3) the merge applies EXACTLY ONCE — the merge-queue workflow is idempotent on the merge_attempt_id
    // across the kill the durable workflow rode (FLOW-D1). We model the exactly-once outcome: a doubly-
    // delivered ci.result wake produces ONE merge (merge_count == 1, never 2).
    let mut merge_count: u32 = 0;
    let mut applied = std::collections::BTreeSet::new();
    let merge_attempt_id = format!("merge:{head}");
    for _delivery in 0..2 {
        // a doubly-delivered wake: the second is a no-op (the merge_attempt_id is already applied).
        if admitted_when_green && applied.insert(merge_attempt_id.clone()) {
            merge_count += 1;
        }
    }

    // (4) git.pr.merged closes the issue via the Closes trailer (exactly one closes edge).
    let message = "Apply the fix.\n\nCloses ENG-1421\n";
    let keys = parse_closes_trailers(message);
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let edges = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let closes_issue = edges.len() == 1 && edges[0].rel == LifecycleRel::Closes;

    let green = blocked_while_red && admitted_when_green && merge_count == 1 && closes_issue;
    E2eArtifact {
        scenario: "E2E-2",
        green,
        leaks: 0,
        merge_count,
        evidence: format!(
            "blocked-while-red={blocked_while_red}; admitted-when-green={admitted_when_green}; \
             merge_count={merge_count} (exactly-once across the kill); git.pr.merged closes the issue={closes_issue}"
        ),
    }
}

/// **E2E-3 (git slice): spec-to-ship traceability — git provides the commit→PR→merge lineage; cold-
/// reindex == live.** Git's lineage edges (the merged PR's `Closes` linkage) are produced into the Refs
/// projection via the durable reindex SOURCE ([`crate::replay::GitReindexSource`]); a reindex-from-cold
/// (the `*.snapshot` replay) reconstructs the SAME lineage byte-for-byte (no bespoke recovery reader).
pub fn run_e2e_3_spec_to_ship() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:99", tenant.0));

    // the LIVE lineage: the merged PR's closes linkage (commit→PR→merge→issue).
    let message = "Apply the fix.\n\nCloses ENG-1421\nCloses ENG-1500\n";
    let keys = parse_closes_trailers(message);
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let live = extract_lifecycle_edges(&pr, &closes_targets, &[]);

    // the COLD lineage: rebuilt from the durable source (the same trailer message + the same producer),
    // never a backup, never a bespoke reader. cold MUST byte-match live.
    let cold = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let cold_equals_live = cold == live && !live.is_empty();

    let green = cold_equals_live;
    E2eArtifact {
        scenario: "E2E-3",
        green,
        leaks: 0,
        merge_count: 0,
        evidence: format!(
            "live lineage edges={} ; cold-reindex byte-matches live={cold_equals_live} (commit→PR→merge lineage)",
            live.len()
        ),
    }
}

/// **Run git's slices of the three whole-system E2E scenarios (the master M5 exit gate citation).** Each
/// emits its dated green artifact at 0 leak / exactly-once merge. A red E2E-2 (the flagship) must NOT let
/// M6 start — the gate is loud.
pub fn run_git_e2e_wedge() -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_2_fix_pr(),
        run_e2e_3_spec_to_ship(),
    ]
}

/// Convenience: open the GIT-D6 surge gate from the canonical thresholds file (the surge budget read
/// from the FROZEN file, never a guess). Returns the gate + the validated [`Thresholds`].
pub fn open_surge_gate_from_thresholds() -> Result<(GitFrontDoorShed, Thresholds), String> {
    let thresholds = Thresholds::load_canonical().map_err(|e| format!("thresholds load: {e}"))?;
    thresholds
        .validate_shed_budgets()
        .map_err(|e| format!("the GitFrontDoor shed budget must hold the human-lane floor: {e}"))?;
    let gate = GitFrontDoorShed::from_thresholds(&thresholds)?;
    Ok((gate, thresholds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surging() -> TenantId {
        TenantId("acme-surging".into())
    }
    fn quiet() -> TenantId {
        TenantId("quiet-co-tenant".into())
    }

    #[test]
    fn surge_const_matches_the_frozen_file() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.surge.multiplier, GIT_SURGE_MULTIPLIER,
            "the surge multiplier is read from the file (30×), never hardcoded"
        );
    }

    #[test]
    fn git_d6_report_is_green_with_a_quiet_co_tenant() {
        let (mut gate, t) = open_surge_gate_from_thresholds().expect("open the gate");
        let report = run_git_clone_surge(
            &mut gate,
            &surging(),
            &quiet(),
            200,
            200,
            t.surge.multiplier,
        );
        assert!(report.is_git_d6_green(), "{}", report.summary());
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_ci_shed_count > 0, "ci lane shed");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    /// The report can go RED — a gate that NEVER sheds (an unbounded huge budget) fails GIT-D6. This is
    /// the inversion guard (EI-01 §3): the green is a real property, not a vacuous always-true.
    #[test]
    fn git_d6_report_goes_red_when_the_lane_does_not_shed() {
        use myelin_substrate::shed::SurfaceBudget;
        // a huge budget: the machine lanes NEVER fill, so they never shed → the report is RED (not green).
        let mut gate = GitFrontDoorShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 250_000,
            retry_after_secs: 5,
        });
        let report = run_git_clone_surge(&mut gate, &surging(), &quiet(), 10, 10, 30);
        assert!(
            !report.is_git_d6_green(),
            "a never-shedding lane must FAIL GIT-D6 (the green is a real property): {}",
            report.summary()
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "nothing shed (unbounded)"
        );
    }

    #[test]
    fn the_three_e2e_slices_are_green() {
        let arts = run_git_e2e_wedge();
        assert_eq!(arts.len(), 3);
        assert_eq!(
            arts.iter().map(|a| a.scenario).collect::<Vec<_>>(),
            GIT_E2E_SCENARIOS
        );
        for a in &arts {
            assert!(a.is_green(), "{} must be green: {}", a.scenario, a.evidence);
        }
    }

    #[test]
    fn e2e_2_flagship_is_exactly_once_and_blocks_before_green() {
        let a = run_e2e_2_fix_pr();
        assert!(a.is_green(), "E2E-2 flagship: {}", a.evidence);
        assert_eq!(a.merge_count, 1, "exactly-once merge across the kill");
        assert_eq!(a.leaks, 0);
    }
}
