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

#[test]
fn e2e_1_pr_pane_green_issues_linked_issue() {
    let art: IssuesE2eArtifact = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, E2E_SCENARIO);
    assert!(
        art.is_green(),
        "E2E-1 (the PR pane - Issues' linked issue) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title/count/backlink leak - {}",
        art.evidence
    );
    assert!(art.evidence.contains("tombstone(denied)=true"));
    assert!(art.evidence.contains("merge_gate_blocked=true"));
    assert!(art.evidence.contains("insider_sees_title=true"));
}

#[test]
fn issues_e2e_wedge_is_green() {
    let arts = run_issues_e2e_wedge();
    assert_eq!(arts.len(), 1, "Issues crosses exactly E2E-1");
    assert!(arts[0].is_green(), "E2E-1: {}", arts[0].evidence);
}

#[test]
fn e2e_1_secret_title_never_appears_in_the_artifact() {
    let art = run_e2e_1_pr_pane();
    assert!(
        !art.evidence.contains("SECRET") && !art.evidence.contains("acquisition"),
        "the secret title must NEVER appear: {}",
        art.evidence
    );
}

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

    let insider = projector
        .project(&confidential, &viewer("insider"), z.clone())
        .expect("well-formed Issues artifact");
    assert!(insider.is_visible());
    assert_eq!(insider.title(), Some("TOP SECRET acquisition plan"));

    let outsider = projector
        .project(&confidential, &viewer("outsider"), z)
        .expect("well-formed Issues artifact - a denied viewer gets a tombstone, never an error");
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

#[test]
fn cdc_5_9_check_status_blocks_under_e2e() {
    let failing = LinkedPrCheck::trusted("failure");
    assert!(
        !failing.is_acceptable(),
        "a failing CheckStatus is not an acceptable Done satisfaction (merge gate blocked)"
    );
    let success = LinkedPrCheck::trusted("success");
    assert!(success.is_acceptable(), "a trusted success satisfies");
    assert_eq!(FRESHNESS_BUDGET_SECS, 5);
}
