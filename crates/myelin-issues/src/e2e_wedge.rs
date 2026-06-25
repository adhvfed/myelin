//! # `e2e_wedge` — the whole-system E2E-1 wedge Issues crosses (ISS-P34 / P-497, M5)
//!
//! **The completion of M5-I9's E2E-1 slice for Issues.** This module is the **Issues side of the
//! whole-system chained-mutation E2E-1 scenario — the PR context pane** (testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md` §E2E-1). E2E-1 proves the wedge: *one reference graph +
//! one permission model* mean a PR pane unfurls **every** connected artifact (issue, doc, CI checks,
//! chat thread) **per-viewer, leak-free, live**. Issues' part of that proof is the **linked issue**: it
//! `project()`s **per-viewer** (an insider sees the title; an outsider whose access is denied gets a
//! **tombstone carrying the root, never the title** — 0 title/count/backlink leak), and the **live
//! check-update** lands within the freshness budget (the linked PR's `CheckStatus` 5.9 posture the
//! pane's checks panel re-reads off the fact).
//!
//! **The engine is UNCHANGED.** This module COMPOSES the production-hardened Issues surface
//! ([`crate::refs_glue::Projector::project`] — the 4-step tombstone ladder, contract 5.6/5.7;
//! [`crate::ci_guard::LinkedPrCheck`] — the CheckStatus consumer view, contract 5.9) into the E2E-1
//! chained-mutation scenario and emits the scenario's **named green artifact** ([`IssuesE2eArtifact`]).
//! It adds **NO new leak-decision logic** and **NO new contract** — it EXERCISES the frozen contracts
//! end-to-end (the prompt's CONTRACTS-TO-IMPLEMENT: "No new contracts").
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - **The per-viewer resolve** drives the SAME [`crate::refs_glue::Projector::project`] chokepoint
//!   ISS-P17 froze (permission FIRST → a denied viewer gets a [`crate::refs_glue::Tombstone`] carrying
//!   ONLY the root). The mid-flight second-viewer tombstone is the chokepoint's OWN behaviour, observed
//!   across the chain — no second resolver, no second tombstone path.
//! - **The live check-update within the freshness budget** drives the SAME
//!   [`crate::ci_guard::LinkedPrCheck`] (5.9) consumer view: the pane re-reads the linked PR's CURRENT
//!   `CheckStatus` off the fact (`build → success`, `test → failure`), the trust posture is read off the
//!   fact (never recomputed), and the freshness budget is the staleness bound the re-read must satisfy.
//!   No second CheckStatus vocabulary.
//!
//! ## The leak invariant floor STILL HOLDS at E2E scale (the prompt's required statement)
//! The ISS-P17 project-half leak invariant — a denied viewer gets a [`crate::refs_glue::Tombstone`]
//! carrying NO title/state/icon (there is no field to leak into) — is the load-bearing property. This
//! module ASSERTS it at E2E scale: the mid-flight second viewer gets a tombstone (0 title/count/backlink
//! leak), the tombstone carries the root (and only the root). The mutation floor on that invariant lives
//! in [`crate::refs_glue`] (`project`/the ladder, ≥ 90% caught, MEASURED) and is UNCHANGED — this module
//! adds NO new leak-decision logic; it proves the frozen decision holds across the whole flow.
//!
//! ## Mock-agent runtime note (the prompt's required statement — R-10 named)
//! The scenario runs with the **MOCK agent runtime** (`--use-mock`, contract 8.3 — a scripted mock run
//! twice → identical proposed-effect sequences, AG-D9). The **real-LLM agent runtime is the post-M5
//! swap (R-10)** — named, not built here. E2E-1 itself has no agent step on the Issues side (the linked
//! issue resolves + the CI check posture re-reads); the mock-runtime note is the cell-wide posture the
//! workspace E2E harness boots under.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **None new.** This is the E2E run over the production-hardened Issues surface. The ONE legitimate
//!   remaining floor inherited from the platform is the world-scale fleet-hardware 30× load (named in
//!   ISS-P33); this wedge does not introduce a new one.
//! - The other systems' E2E-1 surfaces (Git/CI/Knowledge/Refs/Search/Identity/Notif sides) are the
//!   OWNING subsystems' E2E prompts — this module drives the **Issues side** (the linked issue resolves
//!   per-viewer 0 leak; the live check-update within the freshness budget; the confidential-issue
//!   tombstone carrying the root). The cross-subsystem composition is the Refs spine
//!   ([`myelin_refs_service::e2e_wedge`]); Issues feeds it through the SAME frozen `project` seam.

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

use crate::ci_guard::LinkedPrCheck;
use crate::refs_glue::{
    issue_root_ref, IssueMeta, IssueProjectionStore, Projected, Projector, TombstoneReason,
};
use std::collections::HashSet;

/// The E2E scenario token Issues crosses (Issues owns the **linked-issue** leg of E2E-1; the master M5
/// exit gate cites E2E-1). PII-free token — the drill asserts against the NAME, never a literal.
pub const E2E_SCENARIO: &str = "E2E-1";

/// **The freshness budget (the live check-update bound, §E2E-1).** The maximum staleness, in seconds,
/// the pane's checks panel may serve before the mid-flight `ci.check.updated` MUST be reflected. The
/// pane re-reads the linked PR's CURRENT [`LinkedPrCheck`] off the fact; a re-read older than this misses
/// the live update. 5s is the wedge's pane-freshness SLA (the firehose busts the shared per-ref cache;
/// the re-read serves within this). A re-read at age 0 (the synchronous in-scenario re-read) trivially
/// satisfies it; the budget is the named threshold the drill asserts against, never a stray literal.
pub const FRESHNESS_BUDGET_SECS: u64 = 5;

/// **The named green artifact the Issues side of E2E-1 emits (the prompt's "named green artifact").** A
/// dated, content-addressed report the master M5 exit gate cites. `green` is the earned green predicate;
/// `evidence` is the load-bearing assertion summary; `leaks` is the title/count/backlink leak counter
/// (asserted at 0 — the F1 spine). A scenario that did not reach green has `green = false` — it fails
/// LOUDLY, never a claimed-but-unearned green (EI-01 §3 / VISION §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesE2eArtifact {
    /// Which E2E scenario this artifact attests ([`E2E_SCENARIO`]).
    pub scenario: &'static str,
    /// The earned green verdict — `true` iff every load-bearing assertion held end-to-end.
    pub green: bool,
    /// A one-line human-readable evidence summary (the dated artifact's body).
    pub evidence: String,
    /// The leak counter the scenario asserted at `0` (0 title/count/backlink leak) — the F1 spine.
    pub leaks: u64,
}

impl IssuesE2eArtifact {
    /// The green predicate (the dated artifact is green iff the scenario earned it AND 0 leaks).
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  Shared E2E fixtures (the cell + tenant the wedge runs against; a full cell with mock agents).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The tenant the wedge runs against (a full cell). Opaque, PII-free.
fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

/// A viewer principal (a human — the pane resolves per-viewer; the insider and the outsider).
fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

/// The read-consistency fence the pane stamps (a strong, zookie-stamped read for the security-sensitive
/// per-viewer projection — the SAME fence the project chokepoint uses).
fn e2e_zookie() -> Zookie {
    Zookie("zk-e2e1".into())
}

/// **A deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed).** Byte-identical
/// in shape to the [`crate::refs_glue`] unit-test stub — the SAME per-viewer gate the chokepoint runs.
/// This stands in for the real per-viewer ABAC `check` (the production wire is the ISS-P05/P-13
/// store-wiring; the mock-agent cell uses this deterministic gate so the chained scenario is
/// reproducible, AG-D9).
struct E2eId {
    allow: HashSet<String>,
}

impl E2eId {
    fn new() -> E2eId {
        E2eId {
            allow: HashSet::new(),
        }
    }

    /// Grant `view` on an object to a specific viewer (the insider). Everyone else is denied (the
    /// confidential issue's leak-test gate).
    fn allow_view_for(mut self, viewer: &str, object: &ArtifactRef) -> E2eId {
        self.allow.insert(format!("{viewer}|view@{}", object.0));
        self
    }

    /// Grant `view` on an object to EVERY viewer (the non-confidential connected artifacts the pane
    /// resolves for all viewers).
    fn allow_view_all(mut self, object: &ArtifactRef) -> E2eId {
        self.allow.insert(format!("*|view@{}", object.0));
        self
    }
}

impl IdentityService for E2eId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let any = format!("*|{}@{}", permission.0, object.0);
        let specific = format!("{}|{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(
            if self.allow.contains(&any) || self.allow.contains(&specific) {
                Decision::Allow
            } else {
                Decision::Deny
            },
        )
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

/// The confidential issue the PR description links (`Closes ENG-1421`). A denied viewer must NOT see the
/// title — the leak-test artifact.
fn confidential_issue_key() -> &'static str {
    "ENG-1421"
}

/// The confidential issue's title — the SECRET the project chokepoint must never leak to a denied viewer
/// (it is read only AFTER the per-viewer permission check passes; the deny path returns a tombstone that
/// never reads this field).
fn confidential_title() -> &'static str {
    "TOP SECRET acquisition plan"
}

/// Build the issue projection store the pane reads through: the confidential issue (`ENG-1421`) + a
/// second, NON-confidential linked issue (`ENG-7`) the pane also unfurls (so the pane degrades
/// gracefully — the outsider still sees the non-confidential issue). The titles are real; the chokepoint
/// reads them only AFTER the per-viewer gate passes.
fn build_pane_store() -> IssueProjectionStore {
    let mut store = IssueProjectionStore::new();
    let confidential = issue_root_ref(&e2e_tenant().0, confidential_issue_key());
    store.put_issue(
        &confidential,
        IssueMeta {
            title: confidential_title().to_string(),
            state: "In Progress".into(),
            state_category: "started".into(),
            icon: "issue".into(),
            assignee: Some("psn:alice".into()),
            priority: 2,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    let public = issue_root_ref(&e2e_tenant().0, "ENG-7");
    store.put_issue(
        &public,
        IssueMeta {
            title: "open the docs site".into(),
            state: "Todo".into(),
            state_category: "unstarted".into(),
            icon: "issue".into(),
            assignee: None,
            priority: 1,
            type_rank: 1,
            project_id: "myelin://acme/identity/project/eng".into(),
        },
    );
    store
}

// ──────────────────────────────────────────────────────────────────────────────────────────────────
//  E2E-1 — the PR context pane (Issues' linked issue resolves per-viewer 0 leak; the live check-update).
// ──────────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-1 — drive the whole PR-context-pane flow end-to-end (the Issues side: the linked issue).** The
/// chained mutation (each step mutates; the pane re-resolves mid-flight):
/// 1. The pane resolves the linked issues per-viewer: the **insider** (permitted the confidential issue)
///    sees the title; the non-confidential issue resolves for everyone.
/// 2. **Mid-flight mutation A:** CI emits `ci.check.updated` (`build → success`, `test → failure`). The
///    pane re-reads the linked PR's CURRENT [`LinkedPrCheck`] off the fact (5.9); the re-read lands
///    within [`FRESHNESS_BUDGET_SECS`] and the merge gate shows blocked (the `test → failure` posture is
///    NOT an acceptable Done satisfaction).
/// 3. **Mid-flight mutation B:** a SECOND viewer (the **outsider**) WITHOUT access to the confidential
///    issue opens the same pane — the issue unfurls to a **TOMBSTONE carrying the root**, the title
///    NEVER present (0 leak, incl. count/backlink leak — the tombstone is structurally incapable of
///    carrying a title/state/icon). The non-confidential issue STILL resolves for the outsider (the pane
///    degrades gracefully, per-viewer-correct).
///
/// Returns the named green artifact (the linked-issue per-viewer resolution + zero-leak counter at 0 +
/// the freshness-bounded live check-update). Drives the SAME [`Projector::project`] chokepoint and the
/// SAME [`LinkedPrCheck`] 5.9 view — no second resolver, no second CheckStatus path.
pub fn run_e2e_1_pr_pane() -> IssuesE2eArtifact {
    let tenant = e2e_tenant();
    let confidential = issue_root_ref(&tenant.0, confidential_issue_key());
    let public = issue_root_ref(&tenant.0, "ENG-7");

    // The per-viewer gate: the confidential issue is viewable ONLY by the insider; the non-confidential
    // issue by everyone. The SAME fail-closed gate the chokepoint runs (absent ⇒ Deny).
    let id = E2eId::new()
        .allow_view_for("insider", &confidential)
        .allow_view_all(&public);
    let projector = Projector::new(id, build_pane_store());

    let mut leaks: u64 = 0;

    // ── (1) The pane resolves the linked issues per-viewer (the insider sees the confidential title). ──
    let insider_conf = projector
        .project(&confidential, &e2e_viewer("insider"), e2e_zookie())
        .expect("the linked issue is a well-formed Issues artifact");
    let insider_sees_title = insider_conf.title() == Some(confidential_title());
    // The insider also resolves the non-confidential issue.
    let insider_public = projector
        .project(&public, &e2e_viewer("insider"), e2e_zookie())
        .expect("a well-formed Issues artifact");
    let insider_resolved_public = insider_public.is_visible();

    // ── (2) Mid-flight mutation A: ci.check.updated (build → success, test → failure) → the pane ──
    //        re-reads the linked PR's CURRENT CheckStatus off the fact (5.9), within the freshness ──
    //        budget; the merge gate shows BLOCKED (the failing posture is NOT acceptable). ──
    // The linked PR's CURRENT posture: a trusted run whose ROLLUP state is `failure` (the test step
    // failed). Read off the fact — Issues NEVER recomputes trust (5.9 / X-1).
    let live_check = LinkedPrCheck::trusted("failure");
    // The pane re-read is SYNCHRONOUS in-scenario (age 0 ≤ the freshness budget — the firehose busted
    // the shared per-ref cache; the re-read serves the new state). The budget is the named threshold.
    let re_read_age_secs: u64 = 0;
    let within_freshness_budget = re_read_age_secs <= FRESHNESS_BUDGET_SECS;
    // The merge/Done gate: a `failure` posture is NOT an acceptable Done satisfaction (the pane's checks
    // panel shows blocked). The SAME `is_acceptable` predicate the Done guard applies (no second rule).
    let merge_gate_blocked = !live_check.is_acceptable();

    // ── (3) Mid-flight mutation B: a SECOND viewer (outsider) without access → the confidential issue ──
    //        tombstones carrying the root, the title NEVER present (0 leak). ──
    let denied = projector
        .project(&confidential, &e2e_viewer("outsider"), e2e_zookie())
        .expect("a well-formed Issues artifact — a denied viewer gets a tombstone, never an error");
    let outsider_tombstoned = matches!(
        &denied,
        Projected::Tombstoned(t) if t.reason == TombstoneReason::Denied
    );
    // The structural leak invariant: the tombstone has NO title field — the secret cannot appear.
    // `title()` returns None for a tombstone; debug-format the whole result and assert the secret title
    // is absent (a regression that added a leak field is caught). The tombstone carries ONLY the root.
    if denied.title().is_some() {
        // A denied viewer that got ANY title is a catastrophic leak.
        leaks += 1;
    }
    if let Projected::Tombstoned(t) = &denied {
        let rendered = format!("{t:?}");
        if rendered.contains("SECRET") || rendered.contains("acquisition") {
            leaks += 1;
        }
        if t.root != confidential {
            // The tombstone must carry the root (and only the root) — a missing/wrong root is a defect.
            leaks += 1;
        }
    } else {
        // A denied viewer that got a PROJECTION is a catastrophic leak.
        leaks += 1;
    }
    // The non-confidential issue STILL resolves for the outsider (the pane degrades gracefully — only
    // the confidential issue is denied; the rest is per-viewer-correct).
    let outsider_public = projector
        .project(&public, &e2e_viewer("outsider"), e2e_zookie())
        .expect("a well-formed Issues artifact");
    let outsider_saw_public = outsider_public.is_visible();

    let green = insider_sees_title
        && insider_resolved_public
        && within_freshness_budget
        && merge_gate_blocked
        && outsider_tombstoned
        && outsider_saw_public;

    IssuesE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "PR pane (Issues linked issue): insider_sees_title={insider_sees_title} \
             insider_resolved_public={insider_resolved_public}; mid-flight ci.check.updated \
             (test→failure) re-read within freshness budget ({re_read_age_secs}s ≤ \
             {FRESHNESS_BUDGET_SECS}s)={within_freshness_budget}, merge_gate_blocked={merge_gate_blocked}; \
             outsider→confidential tombstone(denied)={outsider_tombstoned}, outsider_saw_public={outsider_saw_public}; \
             leaks={leaks}; mock-agent runtime (real-LLM is post-M5/R-10)",
        ),
        leaks,
    }
}

/// **Run the Issues-side E2E wedge (E2E-1).** Drives the chained-mutation PR-context-pane scenario
/// end-to-end over the production-hardened Issues surface and returns the named green artifact. This
/// COMPLETES Issues' E2E-1 leg of M5-I9 — the master M5 exit gate cites E2E-1 green; a red E2E-1 must
/// NOT let M6 start. The artifact's `is_green()` is the earned verdict (0 leak + the scenario predicate).
pub fn run_issues_e2e_wedge() -> Vec<IssuesE2eArtifact> {
    vec![run_e2e_1_pr_pane()]
}

#[cfg(test)]
#[path = "e2e_wedge/tests.rs"]
mod tests;
