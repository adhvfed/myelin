use super::*;
use myelin_content::inline::InlineNode;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region as BusRegion,
    Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Credential, DataRole as IdDataRole, EffectivePolicy,
    ListObjectsResult, ObjectId, ObjectType, Precondition, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
};
use std::collections::HashSet;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn region() -> Region {
    Region("fr-par".into())
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

#[test]
fn foreign_commit_check_subanchors_preserve_their_canonical_opaque_body() {
    let check = myelin_refs::sub_kind(&ArtifactRef(
        "myelin://acme/issue/issue/ENG-1#commit-deadbeef/check-build".into(),
    ))
    .expect("commit check sub");
    let result = myelin_refs::sub_kind(&ArtifactRef(
        "myelin://acme/issue/issue/ENG-1#commit-deadbeef/ci-result".into(),
    ))
    .expect("commit result sub");
    assert_eq!(sub_opaque_id(&check), "commit-deadbeef/check-build");
    assert_eq!(sub_opaque_id(&result), "commit-deadbeef/ci-result");
}

fn z() -> Zookie {
    Zookie("z0".into())
}

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
        _caveat: Option<&CaveatContext>,
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

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: BusRegion("fr-par".into()),
        actor: Actor(viewer("p")),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

#[test]
fn issues_sub_mints_are_grammatical_and_strip_to_the_root() {
    let root = issue("ENG-1421");

    let c = comment_sub_ref(&root, "7f3a").unwrap();
    assert_eq!(c.0, "myelin://acme/issue/issue/ENG-1421#comment-7f3a");
    assert_eq!(
        myelin_refs::sub_kind(&c),
        Some(myelin_refs::Sub::Comment("7f3a".into()))
    );
    assert_eq!(myelin_refs::strip_sub(&c), root);

    let b = block_sub_ref(&root, "desc1").unwrap();
    assert_eq!(b.0, "myelin://acme/issue/issue/ENG-1421#bdesc1");
    assert_eq!(myelin_refs::strip_sub(&b), root);

    let f = field_sub_ref(&root, "status").unwrap();
    assert_eq!(f.0, "myelin://acme/issue/issue/ENG-1421#field-status");

    let r = row_sub_ref(&root, "r2").unwrap();
    assert_eq!(r.0, "myelin://acme/issue/issue/ENG-1421#row-r2");
}

#[test]
fn issues_sub_mints_reject_a_malformed_body() {
    let root = issue("ENG-1");
    assert!(comment_sub_ref(&root, "").is_err());
    let already = comment_sub_ref(&root, "c1").unwrap();
    assert!(comment_sub_ref(&already, "c2").is_err());
}

#[test]
fn each_inline_node_emits_one_reference_edge() {
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
    tx.stage_state_change("issue ENG-1 body persisted");
    let ids = emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).unwrap();
    tx.commit().unwrap();

    assert_eq!(ids.len(), 3, "one edge per structured node - NOT coalesced");
    assert_eq!(store.outbox_depth(), 3);

    let rows: Vec<_> = store.committed_rows();
    let mention = store.row(&ids[0]).unwrap();
    assert_eq!(mention.envelope.type_.0, REFS_EDGE_CREATED);
    assert_eq!(mention.envelope.payload["rel"], "mentions");
    assert_eq!(mention.envelope.payload["rel_class"], REL_CLASS_REFERENCE);
    assert_eq!(
        mention.envelope.payload["target"],
        "myelin://acme/identity/member/alice"
    );
    assert!(!mention.envelope.contains_personal_data, "no inline PII");
    let aref = store.row(&ids[1]).unwrap();
    assert_eq!(aref.envelope.payload["rel"], "links");
    assert_eq!(aref.envelope.payload["target"], "myelin://acme/git/pr/4291");
    let embed = store.row(&ids[2]).unwrap();
    assert_eq!(embed.envelope.payload["rel"], "embeds");
    assert!(rows
        .iter()
        .all(|r| r.envelope.payload["rel_class"] == REL_CLASS_REFERENCE));
}

#[test]
fn content_edges_are_dropped_on_an_aborted_persist() {
    let (store, minter) = store_and_minter();
    let source = issue("ENG-1");
    let nodes = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme/git/pr/1".into(),
    ))];
    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("staged");
    emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).unwrap();
    drop(tx);
    assert_eq!(store.outbox_depth(), 0, "0 edge without a committed node");
}

#[test]
fn te7_relation_create_emits_the_lifecycle_mirror_edge() {
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
    tx.commit().unwrap();

    let row = store.row(&eid).unwrap();
    assert_eq!(row.envelope.type_.0, crate::events::RELATION_CREATED);
    assert_eq!(row.envelope.payload["rel"], "blocked_by");
    assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
    assert_eq!(row.envelope.payload["source"], src.0);
    assert_eq!(row.envelope.payload["target"], dst.0);
    assert!(!row.envelope.contains_personal_data);
}

#[test]
fn te7_relation_remove_emits_removed_on_the_same_aggregate() {
    let (store, minter) = store_and_minter();
    let src = issue("ENG-1");
    let dst = issue("ENG-2");
    let agg = edge_aggregate_key(&src, &dst);

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("relation removed");
    let eid =
        emit_relation_edge(&mut tx, &src, &dst, IssueLifecycleRel::Relates, false, None).unwrap();
    tx.commit().unwrap();

    let row = store.row(&eid).unwrap();
    assert_eq!(row.envelope.type_.0, crate::events::RELATION_REMOVED);
    assert_eq!(row.aggregate, agg, "same edge aggregate as the create");
}

#[test]
fn lifecycle_rel_tokens_round_trip() {
    for rel in [
        IssueLifecycleRel::Parent,
        IssueLifecycleRel::Blocks,
        IssueLifecycleRel::BlockedBy,
        IssueLifecycleRel::Closes,
        IssueLifecycleRel::DependsOn,
        IssueLifecycleRel::Relates,
    ] {
        assert_eq!(IssueLifecycleRel::from_token(rel.as_str()), Some(rel));
    }
    assert_eq!(IssueLifecycleRel::from_token("not_a_rel"), None);
}

#[test]
fn traverse_walks_forward_edges_rel_filtered() {
    let a = issue("ENG-1");
    let b = issue("ENG-2");
    let c = issue("ENG-3");
    let mut g = IssueRelationGraph::new();
    g.add_edge(&a, &b, IssueLifecycleRel::BlockedBy);
    g.add_edge(&b, &c, IssueLifecycleRel::BlockedBy);
    g.add_edge(&a, &c, IssueLifecycleRel::Relates);

    let reached = g.traverse(&a, Some(IssueLifecycleRel::BlockedBy));
    let nodes: Vec<&str> = reached.iter().map(|n| n.node.0.as_str()).collect();
    assert_eq!(nodes, vec![b.0.as_str(), c.0.as_str()]);
    assert_eq!(reached[0].depth, 1);
    assert_eq!(reached[1].depth, 2);
    assert!(reached.iter().all(|n| n.node != a));
}

#[test]
fn traverse_is_cycle_safe_and_depth_bounded() {
    let a = issue("CY-A");
    let b = issue("CY-B");
    let mut cyclic = IssueRelationGraph::new();
    cyclic.add_edge(&a, &b, IssueLifecycleRel::BlockedBy);
    cyclic.add_edge(&b, &a, IssueLifecycleRel::BlockedBy);
    let reached = cyclic.traverse(&a, None);
    assert_eq!(reached.len(), 1, "B is reached once; the cycle terminates");
    assert_eq!(reached[0].node, b);

    let mut chain = IssueRelationGraph::new();
    let nodes: Vec<ArtifactRef> = (0..20).map(|i| issue(&format!("CH-{i}"))).collect();
    for w in nodes.windows(2) {
        chain.add_edge(&w[0], &w[1], IssueLifecycleRel::DependsOn);
    }
    let reached = chain.traverse(&nodes[0], None);
    assert!(
        reached.iter().all(|n| n.depth <= TRAVERSE_MAX_DEPTH),
        "no node past the depth-16 bound"
    );
    assert_eq!(reached.len(), TRAVERSE_MAX_DEPTH);
    assert!(
        !reached.iter().any(|n| n.node == nodes[17]),
        "the node past the bound is not reached"
    );
}

#[test]
fn a_permitted_viewer_gets_the_projection() {
    let root = issue("ENG-1421");
    let id = StubId::new().allow_view(&root);
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("Fix the login flow"));
    let proj = Projector::new(id, store);

    let out = proj.project(&root, &viewer("v"), z()).unwrap();
    assert!(out.is_visible());
    let p = match out {
        Projected::Visible(p) => p,
        _ => panic!("expected a visible projection"),
    };
    assert_eq!(p.title, "Fix the login flow");
    assert_eq!(p.state, "In Progress");
    assert_eq!(p.category, "started");
    assert_eq!(p.render_hint, "issue");
    assert!(p.sub_anchor.is_none());
}

#[test]
fn unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title() {
    let root = issue("ENG-7");
    let id = StubId::new();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("CONFIDENTIAL acquisition codename"));
    let proj = Projector::new(id, store);

    let out = proj.project(&root, &viewer("intruder"), z()).unwrap();
    assert!(out.is_tombstone());
    assert_eq!(out.title(), None, "the title NEVER leaks on the deny path");
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("expected a tombstone"),
    };
    assert_eq!(t.reason, TombstoneReason::Denied);
    assert_eq!(t.root, root, "the tombstone carries the root");
    assert_eq!(t.display_text(), "(not available)");

    let mut store2 = IssueProjectionStore::new();
    store2.put_issue(&root, meta("secret"));
    let proj2 = Projector::new(StubId::new().allow_view(&root).with_hiccup(), store2);
    let out2 = proj2.project(&root, &viewer("v"), z()).unwrap();
    assert!(out2.is_tombstone());
    assert_eq!(out2.title(), None);
}

#[test]
fn an_erased_issue_projects_to_an_erased_tombstone() {
    let root = issue("ENG-9");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("now-shredded"));
    store.mark_erased(&root);
    let proj = Projector::new(StubId::new().allow_view(&root), store);
    let out = proj.project(&root, &viewer("v"), z()).unwrap();
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("expected a tombstone"),
    };
    assert_eq!(t.reason, TombstoneReason::Erased);
    assert_eq!(t.root, root);
}

#[test]
fn a_restricted_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = issue("ENG-11");
    let sub = comment_sub_ref(&root, "abc").unwrap();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("the parent is fine"));
    store.mark_restricted(&sub);
    let proj = Projector::new(StubId::new().allow_view(&root), store);

    assert!(proj.project(&root, &viewer("v"), z()).unwrap().is_visible());
    let out = proj.project(&sub, &viewer("v"), z()).unwrap();
    assert!(out.is_tombstone());
    assert_eq!(out.title(), None);
}

#[test]
fn an_erased_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = issue("ENG-12");
    let sub = comment_sub_ref(&root, "gone").unwrap();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("the parent is fine"));
    store.mark_erased(&sub);
    let proj = Projector::new(StubId::new().allow_view(&root), store);

    assert!(proj.project(&root, &viewer("v"), z()).unwrap().is_visible());
    let out = proj.project(&sub, &viewer("v"), z()).unwrap();
    assert_eq!(out.title(), None);
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("erased sub tombstones"),
    };
    assert_eq!(t.reason, TombstoneReason::Erased);
}

#[test]
fn a_dangling_root_projects_to_root_gone() {
    let root = issue("ENG-404");
    let proj = Projector::new(StubId::new().allow_view(&root), IssueProjectionStore::new());
    let out = proj.project(&root, &viewer("v"), z()).unwrap();
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("expected a tombstone"),
    };
    assert_eq!(t.reason, TombstoneReason::RootGone);
}

#[test]
fn the_sub_anchor_ladder_lives_moves_outdates_and_gones() {
    let root = issue("ENG-50");
    let live = comment_sub_ref(&root, "c-live").unwrap();
    let moved = block_sub_ref(&root, "bmoved").unwrap();
    let outdated = field_sub_ref(&root, "f-old").unwrap();
    let gone = row_sub_ref(&root, "r-dead").unwrap();

    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("parent"));
    store.put_sub_state(&moved, SubState::Moved);
    store.put_sub_state(&outdated, SubState::Outdated);
    store.put_sub_state(&gone, SubState::Gone);
    let proj = Projector::new(StubId::new().allow_view(&root), store);
    let v = viewer("v");

    let p = match proj.project(&live, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("live sub visible"),
    };
    assert_eq!(p.sub_anchor.as_ref().unwrap().rung, LadderRung::Live);
    assert_eq!(p.sub_anchor.as_ref().unwrap().kind, "comment-");
    assert_eq!(p.sub_anchor.as_ref().unwrap().sub_id, "c-live");

    let p = match proj.project(&moved, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("moved sub visible"),
    };
    assert_eq!(p.sub_anchor.unwrap().rung, LadderRung::Moved);

    let p = match proj.project(&outdated, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("outdated sub visible"),
    };
    assert_eq!(p.sub_anchor.unwrap().rung, LadderRung::Outdated);

    let out = proj.project(&gone, &v, z()).unwrap();
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("gone sub tombstones"),
    };
    assert_eq!(t.reason, TombstoneReason::SubGone);
    assert_eq!(t.root, root);
}

#[test]
fn a_non_issue_ref_is_a_loud_error() {
    let proj = Projector::new(StubId::new(), IssueProjectionStore::new());
    let git = ArtifactRef("myelin://acme/git/pr/1".into());
    assert!(matches!(
        proj.project(&git, &viewer("v"), z()),
        Err(ProjectError::NotAnIssueArtifact { .. })
    ));
}

#[test]
fn issue_search_projection_emits_text_and_typed_facets() {
    let root = issue("ENG-1421");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("Fix the login flow"));
    let fetcher = IssueProjectFetcher::new(store);

    let proj = fetcher.project(&tenant(), &region(), &root).unwrap();
    assert_eq!(proj.text, "Fix the login flow");
    assert_eq!(
        proj.fields.get(crate::declares::FACET_STATE_CATEGORY),
        Some(&FieldValue::Select("started".into()))
    );
    assert_eq!(
        proj.fields.get(crate::declares::FACET_PRIORITY),
        Some(&FieldValue::Int(2))
    );
    assert_eq!(
        proj.fields.get(crate::declares::FACET_ASSIGNEE),
        Some(&FieldValue::Principal("psn:alice".into()))
    );
    assert_eq!(
        proj.fields.get(crate::declares::FACET_TYPE_RANK),
        Some(&FieldValue::Int(1))
    );
    assert_eq!(
        proj.fields.get(crate::declares::FACET_PROJECT_ID),
        Some(&FieldValue::Relation(
            "myelin://acme/identity/project/eng".into()
        ))
    );
}

#[test]
fn emitter_excludes_an_erased_or_restricted_sub_even_when_root_is_clean() {
    let root = issue("ENG-S");
    let erased_sub = comment_sub_ref(&root, "esub").unwrap();
    let restricted_sub = field_sub_ref(&root, "rfield").unwrap();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("clean parent"));
    store.mark_erased(&erased_sub);
    store.mark_restricted(&restricted_sub);
    let fetcher = IssueProjectFetcher::new(store);

    assert!(fetcher.project(&tenant(), &region(), &root).is_ok());
    assert_eq!(
        fetcher.project(&tenant(), &region(), &erased_sub),
        Err(ProjectFetchError::Gone)
    );
    assert_eq!(
        fetcher.project(&tenant(), &region(), &restricted_sub),
        Err(ProjectFetchError::Gone)
    );
}

#[test]
fn rung_tokens_and_projected_accessors_are_pinned() {
    assert_eq!(LadderRung::Live.as_str(), "live");
    assert_eq!(LadderRung::Moved.as_str(), "moved");
    assert_eq!(LadderRung::Outdated.as_str(), "outdated");

    let visible = Projected::Visible(Projection {
        title: "t".into(),
        state: "s".into(),
        category: "started".into(),
        icon: "issue".into(),
        render_hint: "issue".into(),
        sub_anchor: None,
    });
    assert!(visible.is_visible());
    assert!(!visible.is_tombstone());
    assert_eq!(visible.title(), Some("t"));

    let tomb = Projected::Tombstoned(Tombstone {
        reason: TombstoneReason::Denied,
        root: issue("ENG-1"),
    });
    assert!(!tomb.is_visible());
    assert!(tomb.is_tombstone());
    assert_eq!(tomb.title(), None);
}

#[test]
fn issue_search_projection_excludes_restricted_and_erased() {
    let restricted_root = issue("ENG-R");
    let erased_root = issue("ENG-E");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&restricted_root, meta("restricted subject"));
    store.put_issue(&erased_root, meta("erased"));
    store.mark_restricted(&restricted_root);
    store.mark_erased(&erased_root);
    let fetcher = IssueProjectFetcher::new(store);

    assert_eq!(
        fetcher.project(&tenant(), &region(), &restricted_root),
        Err(ProjectFetchError::Gone),
        "a restricted issue is excluded from the index"
    );
    assert_eq!(
        fetcher.project(&tenant(), &region(), &erased_root),
        Err(ProjectFetchError::Gone),
        "an erased issue is excluded from the index"
    );
    assert_eq!(
        fetcher.project(&tenant(), &region(), &issue("ENG-404")),
        Err(ProjectFetchError::Gone)
    );
}

#[test]
fn emitter_facets_are_within_the_declared_6_3_spec() {
    let spec = crate::declares::issue_facets_projection_spec();
    let root = issue("ENG-1");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("t"));
    let fetcher = IssueProjectFetcher::new(store);
    let proj = fetcher.project(&tenant(), &region(), &root).unwrap();
    for (key, value) in &proj.fields {
        let declared = spec
            .struct_fields
            .get(key)
            .unwrap_or_else(|| panic!("emitter facet `{key}` must be in the declared 6.3 spec"));
        assert_eq!(
            *declared,
            value.field_type(),
            "facet `{key}` type matches the declared spec"
        );
    }
}
