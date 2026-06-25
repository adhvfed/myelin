//! # ISS-P34 / P-497 — the whole-system E2E-1 wedge Issues crosses (the PR context pane)
//!
//! **The completion of M5-I9's E2E-1 slice for Issues.** This is the **Issues side of the whole-system
//! chained-mutation E2E-1 scenario — the PR context pane** — driving the WHOLE flow end-to-end (not a
//! single handler) over the production-hardened Issues surface, and asserting the scenario's named green
//! artifact + the F1 leak invariant at E2E scale:
//!
//! - **E2E-1 — the PR context pane (Issues' linked issue):** the linked issue resolves PER-VIEWER (the
//!   insider sees the title; the outsider whose access is denied gets a TOMBSTONE carrying the root —
//!   **0 title/count/backlink leak**, the ISS-D3 project-leak half); the mid-flight `ci.check.updated`
//!   (test → failure) re-reads the linked PR's CURRENT `CheckStatus` off the fact (5.9) within the
//!   freshness budget, and the merge gate shows blocked.
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §E2E-1 (the
//! chained-mutation PR context pane). **Architecture:** issue-tracker `00-overview.md` §1 (the
//! cross-coupled posture), `03-events-contracts-and-glue.md` §3 (`project` — the 4-step tombstone
//! ladder; permission FIRST; a confidential issue → tombstone carrying the root, never the title).
//! **Contract-index rows 5.6** (project) **/ 5.9** (the CheckStatus freshness). **Doctrine:** EI-01 §3/§4
//! (drive the WHOLE thing; prove it; never claim a green you did not earn). **VISION §2** (work flows
//! between tools).
//!
//! ## What this proves (the master M5 exit gate's E2E-1 green, Issues side)
//! The wedge drives the SAME production-hardened Issues `project` chokepoint + the SAME `LinkedPrCheck`
//! 5.9 consumer view the M5 prompts built (no second resolver, no second CheckStatus path) — the green
//! is the surface's own behaviour observed across the whole chained flow. The project-leak mutation
//! floor (`refs_glue.rs`, ≥ 90% caught, MEASURED) is UNCHANGED and STILL HOLDS at E2E scale; this drill
//! adds NO new leak-decision logic.
//!
//! ## Mock-agent runtime note (R-10 named)
//! The scenario runs under the MOCK agent runtime (`--use-mock`, contract 8.3); the **real-LLM agent
//! runtime is the post-M5 swap (R-10)** — named, not built here.
//!
//! ## Floor named
//! None new. The world-scale fleet-hardware 30× load floor is ISS-P33's named floor; this wedge is the
//! E2E run over the production-hardened surface and introduces no new floor.
//!
//! Permanent-gate posture: re-run on every `project`/`CheckStatus`-touching change; this is part of the
//! master M5→M6 boundary (a red E2E-1 must NOT let M6 start).

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::ci_guard::LinkedPrCheck;
use myelin_issues::refs_glue::{
    issue_root_ref, IssueMeta, IssueProjectionStore, Projected, Projector, TombstoneReason,
};
use myelin_issues::{
    run_e2e_1_pr_pane, run_issues_e2e_wedge, IssuesE2eArtifact, E2E_SCENARIO, FRESHNESS_BUDGET_SECS,
};
use myelin_tenancy::TenantId;
use std::collections::HashSet;

// ── E2E-1 driven through the public crate API (the named green artifact). ──

/// **E2E-1 — the PR context pane, end-to-end (Issues' linked issue).** The whole chained flow: resolve
/// the linked issue per-viewer → mid-flight `ci.check.updated` (test → failure) within the freshness
/// budget (merge gate blocked) → second denied viewer's confidential issue tombstones with 0 leak. The
/// named green artifact is emitted.
#[test]
fn e2e_1_pr_pane_green_issues_linked_issue() {
    let art: IssuesE2eArtifact = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, E2E_SCENARIO);
    assert!(
        art.is_green(),
        "E2E-1 (the PR pane — Issues' linked issue) must be green: {}",
        art.evidence
    );
    // The F1 leak spine: 0 title/count/backlink leak to the unauthorized viewer.
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title/count/backlink leak — {}",
        art.evidence
    );
    // The load-bearing chained-mutation assertions are in the dated artifact body.
    assert!(art.evidence.contains("tombstone(denied)=true"));
    assert!(art.evidence.contains("merge_gate_blocked=true"));
    assert!(art.evidence.contains("insider_sees_title=true"));
}

/// **The whole-wedge driver returns exactly the Issues-side E2E-1 leg, green.**
#[test]
fn issues_e2e_wedge_is_green() {
    let arts = run_issues_e2e_wedge();
    assert_eq!(arts.len(), 1, "Issues crosses exactly E2E-1");
    assert!(arts[0].is_green(), "E2E-1: {}", arts[0].evidence);
}

/// **The secret title NEVER appears anywhere in the green artifact body.** A regression that leaked the
/// confidential title into the projection (or the audit body) would surface the secret string here.
#[test]
fn e2e_1_secret_title_never_appears_in_the_artifact() {
    let art = run_e2e_1_pr_pane();
    assert!(
        !art.evidence.contains("SECRET") && !art.evidence.contains("acquisition"),
        "the secret title must NEVER appear: {}",
        art.evidence
    );
}

// ── The 5.6 CDC pair re-asserted UNDER the E2E scenario (the prompt's required re-assert). ──

/// A deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed) — the SAME per-viewer
/// gate the chokepoint runs.
struct CdcId {
    allow: HashSet<String>,
}
impl CdcId {
    fn new() -> CdcId {
        CdcId {
            allow: HashSet::new(),
        }
    }
    fn allow_view(mut self, viewer: &str, object: &myelin_refs::ArtifactRef) -> CdcId {
        self.allow.insert(format!("{viewer}|view@{}", object.0));
        self
    }
}
impl IdentityService for CdcId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &myelin_refs::ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let key = format!("{}|{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(if self.allow.contains(&key) {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// **The 5.6 CDC pair re-asserted under the scenario: `project(ref, viewer)` is `Projection | Tombstone`
/// per-viewer.** The insider gets the title; the outsider gets a `Denied` tombstone carrying the root
/// and NEVER the title (the project-leak counter = 0). This re-confirms the frozen 5.6 shape holds when
/// exercised as the E2E pane reads it.
#[test]
fn cdc_5_6_project_per_viewer_re_asserted_under_e2e() {
    let confidential = issue_root_ref("acme", "ENG-1421");
    let mut store = IssueProjectionStore::new();
    store.put_issue(
        &confidential,
        IssueMeta {
            title: "TOP SECRET acquisition plan".into(),
            state: "In Progress".into(),
            state_category: "started".into(),
            icon: "issue".into(),
            assignee: None,
            priority: 2,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    let id = CdcId::new().allow_view("insider", &confidential);
    let projector = Projector::new(id, store);
    let z = Zookie("zk".into());

    // Insider → a Projection carrying the title (the producer side: project returns the per-viewer view).
    let insider = projector
        .project(&confidential, &viewer("insider"), z.clone())
        .expect("well-formed Issues artifact");
    assert!(insider.is_visible());
    assert_eq!(insider.title(), Some("TOP SECRET acquisition plan"));

    // Outsider → a Denied tombstone carrying the root, NEVER the title (the consumer side: 0 leak).
    let outsider = projector
        .project(&confidential, &viewer("outsider"), z)
        .expect("well-formed Issues artifact — a denied viewer gets a tombstone, never an error");
    match outsider {
        Projected::Tombstoned(t) => {
            assert_eq!(t.reason, TombstoneReason::Denied);
            assert_eq!(t.root, confidential, "the tombstone carries the root");
        }
        Projected::Visible(_) => panic!("a denied viewer must NOT get a projection (a leak)"),
    }
    assert_eq!(
        projector
            .project(&confidential, &viewer("outsider"), Zookie("zk".into()))
            .unwrap()
            .title(),
        None,
        "the denied viewer's title is NEVER present (0 leak)"
    );
}

/// **The 5.9 CheckStatus freshness re-asserted: a failing posture is NOT an acceptable Done
/// satisfaction.** The mid-flight `ci.check.updated` (test → failure) read off the fact blocks the merge
/// gate; the freshness budget is the named pane SLA the re-read satisfies.
#[test]
fn cdc_5_9_check_status_blocks_under_e2e() {
    let failing = LinkedPrCheck::trusted("failure");
    assert!(
        !failing.is_acceptable(),
        "a failing CheckStatus is not an acceptable Done satisfaction (merge gate blocked)"
    );
    let success = LinkedPrCheck::trusted("success");
    assert!(success.is_acceptable(), "a trusted success satisfies");
    // The freshness budget is the named pane-freshness SLA (5s) — asserted, never weakened.
    assert_eq!(FRESHNESS_BUDGET_SECS, 5);
}
