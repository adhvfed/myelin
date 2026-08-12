use super::*;
use crate::check_emitter::CheckState;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RelName, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

struct StubId {
    allow: HashSet<String>,
    hiccup: bool,
}

impl StubId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
            hiccup: false,
        }
    }
    fn allow_view(mut self, object: &ArtifactRef) -> Self {
        self.allow.insert(format!("view@{}", object.0));
        self
    }
    fn with_hiccup(mut self) -> Self {
        self.hiccup = true;
        self
    }
}

impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        if self.hiccup {
            return Err(AuthzError::Unavailable("forced Id break".into()));
        }
        let key = format!("{}@{}", permission.0, object.0);
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
        _ty: &ObjectType,
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
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
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

fn z() -> Zookie {
    Zookie("z0".into())
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn region() -> Region {
    Region("fr-par".into())
}

fn a_run() -> RunMeta {
    RunMeta {
        number: 42,
        pipeline: "build-and-test".into(),
        state: "failed".into(),
        dag_summary: "3/4 stages green".into(),
        failed_step: Some(3),
        duration_secs: Some(187),
    }
}

#[test]
fn ci_artifact_refs_round_trip_canonical_keys() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    assert_eq!(myelin_refs::format(&run), "myelin://acme/ci/run/01J7RUN");
    assert_eq!(myelin_refs::parse(&myelin_refs::format(&run)).unwrap(), run);

    assert_eq!(
        myelin_refs::format(&ci_deployment_ref("acme", "dep9").unwrap()),
        "myelin://acme/ci/deployment/dep9"
    );
    assert_eq!(
        myelin_refs::format(&ci_pipeline_ref("acme", "pl3").unwrap()),
        "myelin://acme/ci/pipeline/pl3"
    );
    assert_eq!(
        myelin_refs::format(&ci_runner_ref("acme", "rn1").unwrap()),
        "myelin://acme/ci/runner/rn1"
    );
    assert_eq!(
        myelin_refs::format(&ci_artifact_ref("acme", "art2").unwrap()),
        "myelin://acme/ci/artifact/art2"
    );
}

#[test]
fn ci_artifact_refs_reject_ambiguous_root_components() {
    assert_eq!(
        ci_run_ref("", "run-1"),
        Err(CiRefError::InvalidComponent {
            component: "tenant"
        })
    );
    assert_eq!(
        ci_run_ref("acme/eu", "run-1"),
        Err(CiRefError::InvalidComponent {
            component: "tenant"
        })
    );
    assert_eq!(
        ci_artifact_ref("acme", "manifest#latest"),
        Err(CiRefError::InvalidComponent { component: "id" })
    );
}

#[test]
fn step_mint_is_stable_across_retries_and_byte_identical_to_details_ref() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    let s1 = run_step_ref(&run, 3).unwrap();
    let s2 = run_step_ref(&run, 3).unwrap();
    assert_eq!(s1, s2, "the step mint is stable for a given (run, step)");
    assert_eq!(
        myelin_refs::format(&s1),
        "myelin://acme/ci/run/01J7RUN#step-3"
    );

    let from_details = crate::check_emitter::details_ref(&run.0, CheckState::Failure, Some(3));
    assert_eq!(
        myelin_refs::format(&s1),
        from_details,
        "the surfacing step mint == the check_emitter details_ref (byte-identical)"
    );
}

#[test]
fn line_range_and_check_mints_go_through_the_one_codec() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    let lr = run_step_line_ref(&run, 42, 88).unwrap();
    assert_eq!(
        myelin_refs::format(&lr),
        "myelin://acme/ci/run/01J7RUN#L42-L88"
    );
    assert!(run_step_line_ref(&run, 88, 42).is_err());

    let check = commit_check_ref(&run, "build").unwrap();
    assert_eq!(
        myelin_refs::format(&check),
        "myelin://acme/ci/run/01J7RUN#check-build"
    );
}

#[test]
fn classify_rejects_a_non_ci_ref() {
    let git = myelin_refs::parse("myelin://acme/git/repo/r1").unwrap();
    assert!(matches!(
        classify(&git),
        Err(ProjectError::NotACiArtifact { .. })
    ));
}

#[test]
fn none_set_lowers_to_false_never_a_permissive_default() {
    let l = lower_over_run_id(&SetExpr::None, &viewer("alice"));
    assert_eq!(l.sql_predicate, "FALSE");
    assert!(l.joins.is_empty());
}

#[test]
fn empty_ids_lowers_to_false_the_leak_free_identity_element() {
    let l = lower_over_run_id(&SetExpr::Ids(vec![]), &viewer("alice"));
    assert_eq!(l.sql_predicate, "FALSE");
    let l2 = lower_over_run_id(&SetExpr::NotIds(vec![]), &viewer("alice"));
    assert_eq!(l2.sql_predicate, "TRUE");
}

#[test]
fn ids_lower_to_a_bound_in_over_run_id_never_interpolated() {
    let l = lower_over_run_id(
        &SetExpr::Ids(vec![ObjectId("r1".into()), ObjectId("r2".into())]),
        &viewer("alice"),
    );
    assert_eq!(l.sql_predicate, "ci_run.run_id IN (:id_0, :id_1)");
    assert_eq!(l.params.len(), 2);
    assert_eq!(l.params[0].value, "r1");
    assert!(!l.sql_predicate.contains("r1"), "0 interpolated literals");
}

#[test]
fn in_relation_lowers_to_one_authz_visible_join_no_n_plus_1() {
    let read = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    };
    let expr = SetExpr::Union(vec![read.clone(), read]);
    let l = lower_over_run_id(&expr, &viewer("alice"));
    assert_eq!(
        l.joins.len(),
        1,
        "one JOIN per distinct (viewer, relation) - no N+1"
    );
    assert!(l.joins[0].clause.contains("authz_visible"));
    assert!(l.joins[0].clause.contains("av0.object_id = ci_run.run_id"));
    assert!(l.depends_on_reverse_index());
}

#[test]
fn composed_run_list_query_is_one_leak_free_statement_with_the_tenant_predicate() {
    let read = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    };
    let q = compose_run_list_query(&read, &viewer("alice"), &tenant(), &region());
    assert_eq!(
        q.statement_count(),
        1,
        "ONE query (no N+1, no second statement)"
    );
    assert!(q.sql.contains("ci_run.tenant_id = :tenant"));
    assert!(q.sql.contains("ci_run.region = :region"));
    let where_pos = q.sql.find("WHERE").unwrap();
    let order_pos = q.sql.find("ORDER BY").unwrap();
    let join_pos = q.sql.find("JOIN authz_visible").unwrap();
    assert!(
        join_pos < where_pos,
        "the JOIN is in the FROM, before WHERE"
    );
    assert!(where_pos < order_pos, "the ACL filter precedes pagination");
}

#[test]
fn the_push_down_returns_only_visible_rows_zero_leak_and_revoke_reflected() {
    let idx = AuthzVisibleIndex::new();
    let alice = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "read", "r1");
    idx.grant(&tenant(), &region(), "alice", "read", "r3");

    let read = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    };
    let lowered = lower_over_run_id(&read, &alice);
    let candidates = vec![
        ObjectId("r1".into()),
        ObjectId("r2".into()),
        ObjectId("r3".into()),
    ];
    let visible = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(
        visible,
        vec![ObjectId("r1".into()), ObjectId("r3".into())],
        "0 leaked rows - the confidential r2 never survives the JOIN"
    );

    idx.revoke(&tenant(), &region(), "alice", "read", "r3");
    let after = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(
        after,
        vec![ObjectId("r1".into())],
        "a revoked grant is reflected - r3 no longer surfaces"
    );
}

#[test]
fn the_search_pre_filter_carries_the_acl_filter_binder_for_the_lint() {
    let read = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    };
    let pf = run_search_pre_filter(&read, &viewer("alice"));
    assert!(pf.acl_filter.depends_on_reverse_index());
    let idx = AuthzVisibleIndex::new();
    let visible = idx.evaluate(
        &tenant(),
        &region(),
        &viewer("alice"),
        &pf.acl_filter,
        &[ObjectId("secret-run".into())],
    );
    assert!(
        visible.is_empty(),
        "0 leak: no grant ⇒ no row in the search input"
    );
}

#[test]
fn difference_lowers_to_and_not_for_the_fork_exclusion() {
    let view = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: ci_run_id_colref(),
    };
    let fork = SetExpr::Ids(vec![ObjectId("fork-run".into())]);
    let expr = SetExpr::Difference(Box::new(view), Box::new(fork));
    let l = lower_over_run_id(&expr, &viewer("alice"));
    assert!(l.sql_predicate.contains("AND NOT"));

    let idx = AuthzVisibleIndex::new();
    idx.grant(&tenant(), &region(), "alice", "view", "good-run");
    idx.grant(&tenant(), &region(), "alice", "view", "fork-run");
    let visible = idx.evaluate(
        &tenant(),
        &region(),
        &viewer("alice"),
        &l,
        &[ObjectId("good-run".into()), ObjectId("fork-run".into())],
    );
    assert_eq!(
        visible,
        vec![ObjectId("good-run".into())],
        "the fork run is excluded by the Difference (AND NOT)"
    );
}

#[test]
fn authorized_viewer_gets_the_run_projection() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);

    let got = p.project(&run_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_visible());
    assert_eq!(got.title(), Some("Run #42 · build-and-test"));
    if let Projected::Visible(proj) = got {
        assert_eq!(proj.state, "failed");
        assert_eq!(proj.icon, "run");
        match proj.render_hint.expect("a run carries a render hint") {
            RenderHint::Run {
                dag_summary,
                failed_step,
                duration_secs,
            } => {
                assert_eq!(dag_summary, "3/4 stages green");
                assert_eq!(failed_step, Some(3));
                assert_eq!(duration_secs, Some(187));
            }
            other => panic!("expected a Run render hint, got {other:?}"),
        }
    }
}

#[test]
fn unauthorized_viewer_gets_a_tombstone_never_the_title() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    let p = Projector::new(StubId::new(), store);

    let got = p.project(&run_ref, &viewer("mallory"), z()).unwrap();
    assert!(
        got.is_tombstone(),
        "an unauthorized viewer must get a tombstone"
    );
    assert_eq!(
        got.title(),
        None,
        "0 title leak - the denied viewer never gets the title"
    );
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Unauthorized);
        assert_eq!(t.display_text(), "(not available)");
    }
}

#[test]
fn an_id_hiccup_fails_closed_to_a_tombstone() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    let p = Projector::new(StubId::new().allow_view(&run_ref).with_hiccup(), store);

    let got = p.project(&run_ref, &viewer("alice"), z()).unwrap();
    assert!(
        got.is_tombstone(),
        "an Id hiccup fails closed to a tombstone"
    );
    assert_eq!(got.title(), None);
}

#[test]
fn an_erased_run_projects_to_a_tombstone() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    store.mark_erased(&run_ref);
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);

    let got = p.project(&run_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(
        got.title(),
        None,
        "an erased run never leaks its (gone) title"
    );
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Erased);
    }
}

#[test]
fn a_restricted_subject_projects_to_a_tombstone() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    store.mark_restricted(&run_ref);
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);
    let got = p.project(&run_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Restricted);
    }
}

#[test]
fn a_step_sub_anchor_projects_and_inherits_the_parent_run_permission() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let step_ref = run_step_ref(&run_ref, 3).unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);

    let got = p.project(&step_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_visible());
    if let Projected::Visible(proj) = got {
        let anchor = proj.sub_anchor.expect("a step sub carries a sub_anchor");
        assert_eq!(anchor.kind, "step");
        assert_eq!(anchor.step, 3);
    }
}

#[test]
fn a_step_sub_is_tombstoned_when_the_parent_run_is_denied() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let step_ref = run_step_ref(&run_ref, 3).unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    let p = Projector::new(StubId::new(), store);
    let got = p.project(&step_ref, &viewer("mallory"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(got.title(), None);
}

#[test]
fn project_deployment_and_pipeline_for_authorized_viewer() {
    let dep_ref = ci_deployment_ref("acme", "dep9").unwrap();
    let pl_ref = ci_pipeline_ref("acme", "pl3").unwrap();
    let mut store = ArtifactStore::new();
    store.put_deployment(
        &dep_ref,
        DeploymentMeta {
            env: "prod".into(),
            version: "v1.4.2".into(),
            state: DeployState::AwaitingApproval,
            risk: "high".into(),
            rollback_available: true,
        },
    );
    store.put_pipeline(
        &pl_ref,
        PipelineMeta {
            name: "release".into(),
            valid: true,
            last_run: Some("myelin://acme/ci/run/01J7RUN".into()),
        },
    );
    let p = Projector::new(
        StubId::new().allow_view(&dep_ref).allow_view(&pl_ref),
        store,
    );

    let d = p.project(&dep_ref, &viewer("alice"), z()).unwrap();
    assert_eq!(d.title(), Some("Deploy prod · v1.4.2"));
    if let Projected::Visible(proj) = &d {
        assert_eq!(proj.state, "awaiting_approval");
        assert_eq!(proj.icon, "deployment");
        assert!(matches!(
            proj.render_hint,
            Some(RenderHint::Deployment {
                rollback_available: true,
                ..
            })
        ));
    }

    let pl = p.project(&pl_ref, &viewer("alice"), z()).unwrap();
    assert_eq!(pl.title(), Some("release"));
    if let Projected::Visible(proj) = &pl {
        assert_eq!(proj.state, "valid");
        assert_eq!(proj.icon, "pipeline");
    }
}

#[test]
fn a_dangling_ref_is_not_found_not_a_tombstone() {
    let run_ref = ci_run_ref("acme", "missing").unwrap();
    let p = Projector::new(StubId::new().allow_view(&run_ref), ArtifactStore::new());
    assert!(matches!(
        p.project(&run_ref, &viewer("alice"), z()),
        Err(ProjectError::NotFound { .. })
    ));
}

#[test]
fn a_non_ci_ref_is_a_loud_error_not_a_tombstone() {
    let git = myelin_refs::parse("myelin://acme/git/repo/r1").unwrap();
    let p = Projector::new(StubId::new().allow_view(&git), ArtifactStore::new());
    assert!(matches!(
        p.project(&git, &viewer("alice"), z()),
        Err(ProjectError::NotACiArtifact { .. })
    ));
}

#[test]
fn an_erased_step_subref_tombstones_even_when_the_root_is_not_erased() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let step_ref = run_step_ref(&run_ref, 3).unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    store.mark_erased(&step_ref);
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);

    let got = p.project(&step_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone(), "an erased step sub-ref tombstones");
    assert_eq!(got.title(), None);
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Erased);
    }
}

#[test]
fn a_restricted_step_subref_tombstones_even_when_the_root_is_not_restricted() {
    let run_ref = ci_run_ref("acme", "01J7RUN").unwrap();
    let step_ref = run_step_ref(&run_ref, 3).unwrap();
    let mut store = ArtifactStore::new();
    store.put_run(&run_ref, a_run());
    store.mark_restricted(&step_ref);
    let p = Projector::new(StubId::new().allow_view(&run_ref), store);

    let got = p.project(&step_ref, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Restricted);
    }
}

#[test]
fn runner_and_artifact_project_to_a_minimal_id_based_projection() {
    let runner_ref = ci_runner_ref("acme", "rn1").unwrap();
    let art_ref = ci_artifact_ref("acme", "art2").unwrap();
    let p = Projector::new(
        StubId::new().allow_view(&runner_ref).allow_view(&art_ref),
        ArtifactStore::new(),
    );

    let r = p.project(&runner_ref, &viewer("alice"), z()).unwrap();
    assert_eq!(r.title(), Some("runner rn1"), "the id segment is rn1");
    if let Projected::Visible(proj) = &r {
        assert_eq!(proj.icon, "runner");
        assert_eq!(proj.state, "present");
    }

    let a = p.project(&art_ref, &viewer("alice"), z()).unwrap();
    assert_eq!(a.title(), Some("artifact art2"));
}

#[test]
fn projected_is_visible_and_is_tombstone_discriminate() {
    let vis = Projected::Visible(Projection {
        title: "t".into(),
        state: "s".into(),
        icon: "run".into(),
        render_hint: None,
        sub_anchor: None,
    });
    let tomb = Projected::Tombstoned(Tombstone {
        reason: TombstoneReason::Unauthorized,
    });
    assert!(vis.is_visible() && !vis.is_tombstone());
    assert!(tomb.is_tombstone() && !tomb.is_visible());
    assert_eq!(vis.title(), Some("t"));
    assert_eq!(tomb.title(), None);
}

#[test]
fn project_errors_display_loud_and_name_the_reference() {
    let nc = ProjectError::NotACiArtifact {
        reference: "myelin://acme/git/repo/r1".into(),
    };
    assert!(nc.to_string().contains("not a CI artifact"));
    assert!(nc.to_string().contains("r1"));
    let ut = ProjectError::UnknownCiType {
        ty: "widget".into(),
    };
    assert!(ut.to_string().contains("widget"));
    let nf = ProjectError::NotFound {
        reference: "myelin://acme/ci/run/x".into(),
    };
    assert!(nf.to_string().contains("dangling"));
}

#[test]
fn the_evaluator_resolves_or_and_difference_precedence_correctly() {
    let idx = AuthzVisibleIndex::new();
    let alice = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "view", "r1");
    idx.grant(&tenant(), &region(), "alice", "view", "r2");

    let expr = SetExpr::Difference(
        Box::new(SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: ci_run_id_colref(),
        }),
        Box::new(SetExpr::Ids(vec![ObjectId("r2".into())])),
    );
    let lowered = lower_over_run_id(&expr, &alice);
    let got = idx.evaluate(
        &tenant(),
        &region(),
        &alice,
        &lowered,
        &[
            ObjectId("r1".into()),
            ObjectId("r2".into()),
            ObjectId("r3".into()),
        ],
    );
    assert_eq!(got, vec![ObjectId("r1".into())]);

    let union = SetExpr::Union(vec![
        SetExpr::Ids(vec![ObjectId("a".into())]),
        SetExpr::Ids(vec![ObjectId("b".into())]),
    ]);
    let lu = lower_over_run_id(&union, &alice);
    let gu = idx.evaluate(
        &tenant(),
        &region(),
        &alice,
        &lu,
        &[
            ObjectId("a".into()),
            ObjectId("b".into()),
            ObjectId("c".into()),
        ],
    );
    assert_eq!(gu, vec![ObjectId("a".into()), ObjectId("b".into())]);

    let inter = SetExpr::Intersect(vec![
        SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: ci_run_id_colref(),
        },
        SetExpr::Ids(vec![ObjectId("r1".into())]),
    ]);
    let li = lower_over_run_id(&inter, &alice);
    let gi = idx.evaluate(
        &tenant(),
        &region(),
        &alice,
        &li,
        &[ObjectId("r1".into()), ObjectId("r2".into())],
    );
    assert_eq!(
        gi,
        vec![ObjectId("r1".into())],
        "the Intersect keeps only r1"
    );
}
