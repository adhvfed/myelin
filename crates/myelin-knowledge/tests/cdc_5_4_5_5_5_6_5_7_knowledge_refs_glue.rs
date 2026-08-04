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
use myelin_knowledge::block_tree::PageId;
use myelin_knowledge::database::{DbRelation, RelationKind, RelationStore};
use myelin_knowledge::refs_glue::{
    LadderRung, PageMeta, PageStore, Projected, Projector, SubState, TombstoneReason,
};
use myelin_knowledge::{
    emit_content_edges, emit_page_parent_set, emit_relation_edge, REL_CLASS_LIFECYCLE,
    REL_CLASS_REFERENCE,
};
use std::collections::HashSet;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn page_root(id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/knowledge/page/{id}"))
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

fn z() -> Zookie {
    Zookie("z0".into())
}

struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_read(mut self, o: &ArtifactRef) -> Self {
        self.allow.insert(format!("read@{}", o.0));
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
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(if self.allow.contains(&format!("{}@{}", p.0, o.0)) {
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
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

#[test]
fn cdc_5_4_content_nodes_produce_reference_edges() {
    let (store, minter) = store_and_minter();
    let source = page_root("7c2");
    let nodes = vec![
        InlineNode::Mention(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(page_root("runbook")),
    ];
    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let ids = emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).expect("emit");
    tx.commit().expect("commit");
    assert_eq!(
        ids.len(),
        3,
        "one edge per node - NOT coalesced (a discrete edge fact)"
    );

    for id in &ids {
        let row = store.row(id).expect("edge row");
        assert_eq!(row.envelope.type_.0, "refs.edge.created");
        assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_REFERENCE);
        assert_ne!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
        assert_eq!(row.envelope.payload["source"], source.0);
    }
}

#[test]
fn cdc_5_5_page_parent_mirror_forward_only_lifecycle() {
    let (store, minter) = store_and_minter();
    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let id = emit_page_parent_set(
        &mut tx,
        &tenant(),
        &PageId("project".into()),
        &PageId("team".into()),
        None,
    )
    .expect("emit parent_set");
    tx.commit().expect("the typed row + its mirror co-commit");
    let row = store.row(&id).expect("parent_set row");
    assert_eq!(row.envelope.type_.0, "knowledge.page.parent_set");
    assert_eq!(row.envelope.payload["rel"], "parent");
    assert_eq!(row.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);
    assert_eq!(
        row.envelope.payload["source"],
        "myelin://acme/knowledge/page/project"
    );
    assert_eq!(
        row.envelope.payload["target"],
        "myelin://acme/knowledge/page/team"
    );
}

#[test]
fn cdc_5_5_db_relation_mirror_created_then_removed() {
    let (store, minter) = store_and_minter();
    let rels = RelationStore::new();
    let relation = DbRelation {
        relation_id: "rel1".into(),
        src_row: "row7".into(),
        dst_ref: ArtifactRef("myelin://acme/knowledge/row/row9".into()),
        rel: RelationKind::Relates,
    };
    assert!(
        rels.relate(&tenant(), relation.clone()),
        "the typed table is truth: a new forward row"
    );

    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let cid = emit_relation_edge(&mut tx, &tenant(), &relation, true, None).expect("created");
    tx.commit().expect("commit");
    let c = store.row(&cid).expect("created row");
    assert_eq!(c.envelope.type_.0, "knowledge.relation.created");
    assert_eq!(c.envelope.payload["rel"], "relates");
    assert_eq!(c.envelope.payload["rel_class"], REL_CLASS_LIFECYCLE);

    let mut tx2 = store.begin(Arc::clone(&minter), ctx_base());
    let rid = emit_relation_edge(&mut tx2, &tenant(), &relation, false, None).expect("removed");
    tx2.commit().expect("commit");
    let r = store.row(&rid).expect("removed row");
    assert_eq!(r.envelope.type_.0, "knowledge.relation.removed");
    assert_eq!(
        r.aggregate.0, c.aggregate.0,
        "create + remove share the edge aggregate (ordered)"
    );
}

#[test]
fn cdc_5_6_project_authorized_shape() {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "Incident runbook".into(),
            state: "live".into(),
        },
    );
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert!(got.is_visible());
    assert_eq!(got.title(), Some("Incident runbook"));
    if let Projected::Visible(proj) = got {
        assert_eq!(proj.icon, "page");
        assert_eq!(proj.render_hint, "page");
    }
}

#[test]
fn cdc_5_6_confidential_page_tombstones_never_leaks() {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "TOP SECRET".into(),
            state: "live".into(),
        },
    );
    let p = Projector::new(StubId::new(), store);
    let got = p.project(&root, &viewer("mallory"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(got.title(), None, "0 title leak");
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Denied);
        assert_eq!(t.root, root, "a tombstone ALWAYS carries the root (§2.1)");
        assert!(
            !t.root.0.contains("SECRET"),
            "the root URN is opaque scope, never the title"
        );
        assert_eq!(t.display_text(), "(not available)");
    }
}

#[test]
fn cdc_5_7_the_four_step_ladder_all_rungs() {
    let root = page_root("7c2");
    let live = myelin_refs::mint(&root, myelin_refs::Sub::Block("b9".into())).unwrap();
    let moved = myelin_refs::mint(&root, myelin_refs::Sub::Block("b10".into())).unwrap();
    let outdated = myelin_refs::mint(&root, myelin_refs::Sub::Block("b11".into())).unwrap();
    let gone = myelin_refs::mint(&root, myelin_refs::Sub::Block("b99".into())).unwrap();
    let erased = myelin_refs::mint(&root, myelin_refs::Sub::Block("b7".into())).unwrap();

    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "Incident runbook".into(),
            state: "live".into(),
        },
    );
    store.put_sub_state(&live, SubState::Live);
    store.put_sub_state(&moved, SubState::Moved);
    store.put_sub_state(&outdated, SubState::Outdated);
    store.put_sub_state(&gone, SubState::Gone);
    store.mark_erased(&erased);
    let p = Projector::new(StubId::new().allow_read(&root), store);

    for (r, rung) in [
        (&live, LadderRung::Live),
        (&moved, LadderRung::Moved),
        (&outdated, LadderRung::Outdated),
    ] {
        match p.project(r, &viewer("alice"), z()).unwrap() {
            Projected::Visible(proj) => assert_eq!(proj.sub_anchor.unwrap().rung, rung),
            Projected::Tombstoned(_) => panic!("{rung:?} must resolve to a visible sub-anchor"),
        }
    }
    match p.project(&gone, &viewer("alice"), z()).unwrap() {
        Projected::Tombstoned(t) => {
            assert_eq!(t.reason, TombstoneReason::SubGone);
            assert_eq!(t.root, root);
        }
        Projected::Visible(_) => panic!("a GONE block must tombstone"),
    }
    match p.project(&erased, &viewer("alice"), z()).unwrap() {
        Projected::Tombstoned(t) => {
            assert_eq!(t.reason, TombstoneReason::Erased);
            assert_eq!(t.root, root);
        }
        Projected::Visible(_) => panic!("an ERASED block must tombstone"),
    }
}
