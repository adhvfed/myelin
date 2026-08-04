use myelin_content::inline::InlineNode;
use myelin_events::{
    Actor, ArtifactRef, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region,
    TenantId, Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole as IdDataRole, Decision,
    EffectivePolicy, IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission,
    Precondition, Principal, PrincipalId, PrincipalKind, PrincipalStatus, Result as IdResult,
    RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::refs_glue::{
    IssueMeta, IssueProjectFetcher, IssueProjectionStore, IssueRelationGraph, LadderRung,
    Projected, Projector, SubState, TombstoneReason,
};
use myelin_issues::{
    comment_sub_ref, edge_aggregate_key, emit_content_edges, emit_relation_edge, issue_root_ref,
    IssueLifecycleRel, REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE, REL_CLASS_REFERENCE,
    TRAVERSE_MAX_DEPTH,
};
use myelin_query::FieldValue;
use myelin_search::ProjectFetcher;
use myelin_tenancy::Region as TenancyRegion;
use std::collections::HashSet;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn viewer(id: &str) -> Principal {
    Principal::new(
        tenant(),
        Region("fr-par".into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        IdDataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn issue(key: &str) -> ArtifactRef {
    issue_root_ref("acme", key)
}

fn z() -> Zookie {
    Zookie("z0".into())
}

fn meta(title: &str) -> IssueMeta {
    IssueMeta {
        title: title.into(),
        state: "In Progress".into(),
        state_category: "started".into(),
        icon: "issue".into(),
        assignee: Some("psn:alice".into()),
        priority: 2,
        type_rank: 1,
        project_id: "myelin://acme/identity/project/eng".into(),
    }
}

struct StubId {
    allow_any: HashSet<String>,
    allow_subject: HashSet<String>,
}

impl StubId {
    fn new() -> Self {
        Self {
            allow_any: HashSet::new(),
            allow_subject: HashSet::new(),
        }
    }
    fn allow_view(mut self, object: &ArtifactRef) -> Self {
        self.allow_any.insert(format!("view@{}", object.0));
        self
    }
    fn allow_view_for(mut self, subject: &str, object: &ArtifactRef) -> Self {
        self.allow_subject
            .insert(format!("{subject}:view@{}", object.0));
        self
    }
}

impl IdentityService for StubId {
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
        let any = format!("{}@{}", permission.0, object.0);
        let specific = format!("{}:{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(
            if self.allow_any.contains(&any) || self.allow_subject.contains(&specific) {
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

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(viewer("p")),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

#[test]
fn cdc_5_4_content_nodes_produce_reference_edges() {
    let (store, minter) = store_and_minter();
    let source = issue("ENG-1");
    let nodes = vec![
        InlineNode::Mention(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/git/pr/4291".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
    ];

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("issue body persisted");
    let ids = emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).unwrap();
    tx.commit().unwrap();

    assert_eq!(ids.len(), 3);
    assert_eq!(store.outbox_depth(), 3);
    for id in &ids {
        let row = store.row(id).unwrap();
        assert_eq!(row.envelope.type_.0, REFS_EDGE_CREATED);
        assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_REFERENCE);
        assert!(row.envelope.payload["source"].is_string());
        assert!(row.envelope.payload["target"].is_string());
    }
    let mention = store.row(&ids[0]).unwrap();
    let expected_agg = edge_aggregate_key(
        &source,
        &ArtifactRef("myelin://acme/identity/principal/alice".into()),
    );
    assert_eq!(mention.aggregate, expected_agg);
}

#[test]
fn cdc_5_5_issue_relation_mirror_forward_only_lifecycle() {
    let (store, minter) = store_and_minter();
    let src = issue("ENG-1");
    let dst = issue("ENG-2");

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("issue_relation blocked_by written");
    let eid = emit_relation_edge(
        &mut tx,
        &src,
        &dst,
        IssueLifecycleRel::BlockedBy,
        true,
        None,
    )
    .unwrap();
    tx.commit().expect("the typed row + its mirror co-commit");

    let row = store.row(&eid).unwrap();
    assert_eq!(row.envelope.type_.0, "issue.relation.created");
    assert_eq!(row.envelope.payload["rel"], "blocked_by");
    assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
    assert!(row.envelope.payload.get("inverse").is_none());
}

#[test]
fn cdc_5_5_issue_relation_created_then_removed() {
    let (store, minter) = store_and_minter();
    let src = issue("ENG-1");
    let dst = ArtifactRef("myelin://acme/git/pr/99".into());
    let agg = edge_aggregate_key(&src, &dst);

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("closes relate");
    let created =
        emit_relation_edge(&mut tx, &src, &dst, IssueLifecycleRel::Closes, true, None).unwrap();
    let removed =
        emit_relation_edge(&mut tx, &src, &dst, IssueLifecycleRel::Closes, false, None).unwrap();
    tx.commit().unwrap();

    assert_eq!(
        store.row(&created).unwrap().envelope.type_.0,
        "issue.relation.created"
    );
    assert_eq!(
        store.row(&removed).unwrap().envelope.type_.0,
        "issue.relation.removed"
    );
    assert_eq!(store.row(&created).unwrap().aggregate, agg);
    assert_eq!(store.row(&removed).unwrap().aggregate, agg);
}

#[test]
fn cdc_5_3_traverse_is_bounded_and_cycle_safe() {
    let a = issue("CY-A");
    let b = issue("CY-B");
    let mut cyclic = IssueRelationGraph::new();
    cyclic.add_edge(&a, &b, IssueLifecycleRel::BlockedBy);
    cyclic.add_edge(&b, &a, IssueLifecycleRel::BlockedBy);
    assert_eq!(cyclic.traverse(&a, None).len(), 1, "the cycle terminates");

    let mut chain = IssueRelationGraph::new();
    let nodes: Vec<ArtifactRef> = (0..20).map(|i| issue(&format!("CH-{i}"))).collect();
    for w in nodes.windows(2) {
        chain.add_edge(&w[0], &w[1], IssueLifecycleRel::DependsOn);
    }
    let reached = chain.traverse(&nodes[0], None);
    assert_eq!(reached.len(), TRAVERSE_MAX_DEPTH);
    assert!(reached.iter().all(|n| n.depth <= TRAVERSE_MAX_DEPTH));
}

#[test]
fn cdc_5_6_project_authorized_shape() {
    let root = issue("ENG-1421");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("Fix the login flow"));
    let p = Projector::new(StubId::new().allow_view(&root), store);

    let out = p.project(&root, &viewer("v"), z()).unwrap();
    let proj = match out {
        Projected::Visible(p) => p,
        _ => panic!("authorized → visible"),
    };
    assert_eq!(proj.title, "Fix the login flow");
    assert_eq!(proj.category, "started");
    assert_eq!(proj.render_hint, "issue");
}

#[test]
fn cdc_5_6_confidential_issue_tombstones_never_leaks() {
    let root = issue("ENG-7");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("CONFIDENTIAL acquisition codename"));
    let p = Projector::new(StubId::new(), store);

    let out = p.project(&root, &viewer("intruder"), z()).unwrap();
    assert!(out.is_tombstone());
    assert_eq!(out.title(), None, "the title NEVER leaks");
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => unreachable!(),
    };
    assert_eq!(t.reason, TombstoneReason::Denied);
    assert_eq!(t.root, root, "the tombstone carries the root");
}

#[test]
fn cdc_5_7_chained_mint_resolve_per_viewer() {
    let root = issue("ENG-50");
    let sub = comment_sub_ref(&root, "7f3a").unwrap();
    assert_eq!(sub.0, "myelin://acme/issue/issue/ENG-50#comment-7f3a");

    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("the review thread"));
    store.put_sub_state(&sub, SubState::Live);
    let alice = viewer("alice");
    let p = Projector::new(StubId::new().allow_view_for("alice", &root), store);

    let permitted = p.project(&sub, &alice, z()).unwrap();
    let proj = match permitted {
        Projected::Visible(p) => p,
        _ => panic!("alice may view → visible"),
    };
    let anchor = proj.sub_anchor.expect("the #sub resolved");
    assert_eq!(anchor.kind, "comment-");
    assert_eq!(anchor.sub_id, "7f3a");
    assert_eq!(anchor.rung, LadderRung::Live);

    let bob = viewer("bob");
    let denied = p.project(&sub, &bob, z()).unwrap();
    assert!(denied.is_tombstone());
    assert_eq!(denied.title(), None);
    if let Projected::Tombstoned(t) = denied {
        assert_eq!(t.root, root, "the sub-tombstone carries the issue root");
    }
}

#[test]
fn cdc_6_3_issue_search_projection_emitter() {
    let spec = myelin_issues::declares::issue_facets_projection_spec();
    let root = issue("ENG-1421");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("Fix the login flow"));
    let fetcher = IssueProjectFetcher::new(store);

    let proj = fetcher
        .project(&tenant(), &TenancyRegion("fr-par".into()), &root)
        .unwrap();
    assert_eq!(proj.text, "Fix the login flow");
    assert_eq!(
        proj.fields
            .get(myelin_issues::declares::FACET_STATE_CATEGORY),
        Some(&FieldValue::Select("started".into()))
    );
    for (key, value) in &proj.fields {
        assert_eq!(
            spec.struct_fields.get(key),
            Some(&value.field_type()),
            "facet `{key}` is within the declared 6.3 spec"
        );
    }
}

#[test]
fn cdc_6_3_emitter_excludes_restricted_and_erased() {
    let restricted = issue("ENG-R");
    let erased = issue("ENG-E");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&restricted, meta("restricted"));
    store.put_issue(&erased, meta("erased"));
    store.mark_restricted(&restricted);
    store.mark_erased(&erased);
    let fetcher = IssueProjectFetcher::new(store);
    let region = TenancyRegion("fr-par".into());

    assert_eq!(
        fetcher.project(&tenant(), &region, &restricted),
        Err(myelin_search::ProjectFetchError::Gone)
    );
    assert_eq!(
        fetcher.project(&tenant(), &region, &erased),
        Err(myelin_search::ProjectFetchError::Gone)
    );
}
