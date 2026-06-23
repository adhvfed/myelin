//! # The CDC pair for CI's OWNED cross-fabric surfacing rows 5.1 / 5.6 / 5.7 (CI-P25 → P-368, M4)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md`
//! - **5.1** `ArtifactRef` — CI's canonical mints (`myelin://<t>/ci/<type>/<id>`);
//! - **5.6** `project(ref, viewer)` — the per-viewer permission-checked projection (the ONLY cross-DB
//!   read of a CI artifact);
//! - **5.7** the unified `#sub` grammar — CI owns `step-<n>` / `check-<context>` / `L<a>-L<b>`.
//!
//! Owning architecture:
//! `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §7.1 (the mints) + §7.2 (`project`). Reconciliation: `00-reconciliation-decisions.md` §X-4/OQ-D.
//!
//! ## What this CDC pins (the PROVIDER ↔ CONSUMER no-drift property)
//! - **PROVIDER** (CI): CI MINTS its canonical `ArtifactRef`s + the `#step-<n>` sub through the ONE
//!   [`myelin_refs`] codec (0 ungrammatical refs), and BUILDS the per-viewer
//!   `{title, state, icon, render_hint, sub_anchor?}` projection only after a permission check.
//! - **CONSUMER** (a downstream surface — chat unfurl / PR context pane / knowledge embed / inbox /
//!   search): it reads the projection's `{title, state, icon}` IFF authorized, else the content-free
//!   tombstone text `"(not available)"` — it NEVER sees the title of a denied/erased run.
//!
//! The CONSUMER side is modelled locally (a downstream reader's field reads) because the cross-fabric
//! consumers (Refs/Search/Notif/Chat) do not depend back on `myelin-ci-controlplane` — `project` is
//! the seam; this CDC proves CI's producer shape is exactly what a consumer reads.

use myelin_ci_controlplane::{
    ci_run_ref, run_step_ref, ArtifactStore, Projected, Projector, RenderHint, RunMeta,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;
use std::collections::HashSet;

// ── a deterministic Id allow-list double (the FROZEN consumed `IdentityService::check` surface). ──
struct AllowList(HashSet<String>);
impl AllowList {
    fn allowing(refs: &[&ArtifactRef]) -> Self {
        AllowList(refs.iter().map(|r| format!("view@{}", r.0)).collect())
    }
}
impl IdentityService for AllowList {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(if self.0.contains(&format!("{}@{}", p.0, o.0)) {
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
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

fn a_run() -> RunMeta {
    RunMeta {
        number: 7,
        pipeline: "ci".into(),
        state: "passed".into(),
        dag_summary: "all green".into(),
        failed_step: None,
        duration_secs: Some(60),
    }
}

/// **PROVIDER 5.1/5.7:** CI mints canonical `ArtifactRef`s + the `#step-<n>` sub through the ONE refs
/// codec — round-trip byte-identical, 0 ungrammatical by construction. **CONSUMER:** Refs accepts and
/// re-parses the minted URN (the grammar it validates), so a downstream link never dangles on a
/// malformed ref.
#[test]
fn provider_mints_grammatical_refs_the_consumer_refs_codec_round_trips() {
    let run = ci_run_ref("acme", "01J7RUN");
    assert_eq!(myelin_refs::format(&run), "myelin://acme/ci/run/01J7RUN");
    // CONSUMER (Refs): the minted URN re-parses to the same value (a fixed point).
    assert_eq!(myelin_refs::parse(&myelin_refs::format(&run)).unwrap(), run);

    let step = run_step_ref(&run, 3).unwrap();
    assert_eq!(
        myelin_refs::format(&step),
        "myelin://acme/ci/run/01J7RUN#step-3"
    );
    // CONSUMER (Refs): strips the `#sub` to the canonical root (a broken sub still resolves to the run).
    assert_eq!(myelin_refs::strip_sub(&step), run);
}

/// **PROVIDER 5.6:** CI's `project` builds the `{title, state, icon, render_hint, sub_anchor?}`
/// projection for an authorized viewer. **CONSUMER:** a downstream surface reads exactly those fields
/// to render the run unfurl / context pane.
#[test]
fn provider_project_builds_the_projection_the_consumer_renders() {
    let run = ci_run_ref("acme", "01J7RUN");
    let mut store = ArtifactStore::new();
    store.put_run(&run, a_run());
    let projector = Projector::new(AllowList::allowing(&[&run]), store);

    // PROVIDER: the per-viewer projection.
    let got = projector
        .project(&run, &viewer("alice"), Zookie("z0".into()))
        .unwrap();

    // CONSUMER: reads the frozen projection fields.
    match got {
        Projected::Visible(p) => {
            assert_eq!(p.title, "Run #7 · ci");
            assert_eq!(p.state, "passed");
            assert_eq!(p.icon, "run");
            assert!(matches!(p.render_hint, Some(RenderHint::Run { .. })));
        }
        Projected::Tombstoned(_) => panic!("an authorized viewer must get the projection"),
    }
}

/// **PROVIDER 5.6 (the 0-leak invariant):** a denied viewer gets a content-free tombstone, never the
/// title. **CONSUMER:** the downstream surface renders only `"(not available)"` — it cannot read the
/// title of a confidential run (the cross-fabric leak counter = 0).
#[test]
fn provider_tombstones_on_deny_the_consumer_never_sees_the_title() {
    let run = ci_run_ref("acme", "01J7RUN");
    let mut store = ArtifactStore::new();
    store.put_run(&run, a_run());
    // the allow-list grants NOBODY.
    let projector = Projector::new(AllowList::allowing(&[]), store);

    let got = projector
        .project(&run, &viewer("mallory"), Zookie("z0".into()))
        .unwrap();

    // CONSUMER: 0 title leak — only the generic tombstone text.
    assert_eq!(got.title(), None);
    match got {
        Projected::Tombstoned(t) => assert_eq!(t.display_text(), "(not available)"),
        Projected::Visible(_) => panic!("a denied viewer must get a tombstone, never the title"),
    }
}
