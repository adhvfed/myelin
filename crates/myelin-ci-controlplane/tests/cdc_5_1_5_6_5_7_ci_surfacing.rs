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

#[test]
fn provider_mints_grammatical_refs_the_consumer_refs_codec_round_trips() {
    let run = ci_run_ref("acme", "01J7RUN");
    assert_eq!(myelin_refs::format(&run), "myelin://acme/ci/run/01J7RUN");
    assert_eq!(myelin_refs::parse(&myelin_refs::format(&run)).unwrap(), run);

    let step = run_step_ref(&run, 3).unwrap();
    assert_eq!(
        myelin_refs::format(&step),
        "myelin://acme/ci/run/01J7RUN#step-3"
    );
    assert_eq!(myelin_refs::strip_sub(&step), run);
}

#[test]
fn provider_project_builds_the_projection_the_consumer_renders() {
    let run = ci_run_ref("acme", "01J7RUN");
    let mut store = ArtifactStore::new();
    store.put_run(&run, a_run());
    let projector = Projector::new(AllowList::allowing(&[&run]), store);

    let got = projector
        .project(&run, &viewer("alice"), Zookie("z0".into()))
        .unwrap();

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

#[test]
fn provider_tombstones_on_deny_the_consumer_never_sees_the_title() {
    let run = ci_run_ref("acme", "01J7RUN");
    let mut store = ArtifactStore::new();
    store.put_run(&run, a_run());
    let projector = Projector::new(AllowList::allowing(&[]), store);

    let got = projector
        .project(&run, &viewer("mallory"), Zookie("z0".into()))
        .unwrap();

    assert_eq!(got.title(), None);
    match got {
        Projected::Tombstoned(t) => assert_eq!(t.display_text(), "(not available)"),
        Projected::Visible(_) => panic!("a denied viewer must get a tombstone, never the title"),
    }
}
