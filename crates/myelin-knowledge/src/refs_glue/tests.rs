use super::*;
use crate::block_tree::PageTree;
use crate::database::RelationStore;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Credential, DataRole as IdDataRole, EffectivePolicy,
    ListObjectsResult, ObjectId, ObjectType, Precondition, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
};
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

fn page_root(page_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/knowledge/page/{page_id}"))
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
    fn allow_read(mut self, object: &ArtifactRef) -> Self {
        self.allow.insert(format!("read@{}", object.0));
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
fn each_inline_node_emits_one_reference_edge() {
    let (store, minter) = store_and_minter();
    let source = page_root("7c2");
    let nodes = vec![
        InlineNode::Mention(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(page_root("incident-runbook")),
    ];

    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let ids = emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).expect("emit edges");
    assert_eq!(ids.len(), 3, "one edge per node - NOT coalesced");
    tx.commit().expect("commit the block persist + its edges");

    let m = store.row(&ids[0]).expect("mention edge row");
    assert_eq!(m.envelope.type_.0, "refs.edge.created");
    assert_eq!(m.envelope.payload["rel"], "mentions");
    assert_eq!(m.envelope.payload["rel_class"], "reference");
    assert_eq!(
        m.envelope.payload["target"],
        "myelin://acme/identity/member/alice"
    );
    assert!(
        !m.envelope.contains_personal_data,
        "references-not-payloads: opaque principal id, no PII"
    );

    let a = store.row(&ids[1]).expect("artifact_ref edge row");
    assert_eq!(a.envelope.payload["rel"], "links");
    assert_eq!(
        a.envelope.payload["target"],
        "myelin://acme/issue/issue/ENG-1"
    );

    let e = store.row(&ids[2]).expect("embed edge row");
    assert_eq!(e.envelope.payload["rel"], "embeds");
    assert_eq!(
        e.aggregate,
        edge_aggregate_key(
            &source,
            &ArtifactRef("myelin://acme/knowledge/page/incident-runbook".into())
        )
    );
    assert_eq!(e.envelope.payload["source"], source.0);
}

#[test]
fn content_edges_are_emit_iff_committed() {
    let (store, minter) = store_and_minter();
    let source = page_root("7c2");
    let nodes = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme/issue/issue/ENG-1".into(),
    ))];
    {
        let mut tx = store.begin(Arc::clone(&minter), ctx_base());
        emit_content_edges(&mut tx, &tenant(), &source, &nodes, None).expect("emit");
    }
    assert_eq!(
        store.outbox_depth(),
        0,
        "an aborted block persist wrote 0 edges (no edge without its node)"
    );
}

#[test]
fn te7_page_parent_set_mirrors_the_typed_row_in_the_same_tx() {
    let (store, minter) = store_and_minter();
    let mut pages = PageTree::new();
    pages
        .set_parent(
            PageId("project".into()),
            PageId("team".into()),
            myelin_query::field::OrderKey::rank_first(
                myelin_query::field::Jitter::from_ranks(0, 0).unwrap(),
            ),
        )
        .expect("set_parent (the typed table is truth)");

    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let id = emit_page_parent_set(
        &mut tx,
        &tenant(),
        &PageId("project".into()),
        &PageId("team".into()),
        None,
    )
    .expect("emit the TE-7 parent_set mirror");
    assert_eq!(
        store.outbox_depth(),
        0,
        "buffered into the open tx (co-commits with the typed row)"
    );
    tx.commit().expect("the typed row + its mirror co-commit");

    let row = store.row(&id).expect("the parent_set row");
    assert_eq!(row.envelope.type_.0, "knowledge.page.parent_set");
    assert_eq!(row.envelope.payload["rel"], "parent");
    assert_eq!(row.envelope.payload["rel_class"], "lifecycle");
    assert_eq!(
        row.envelope.payload["source"],
        "myelin://acme/knowledge/page/project"
    );
    assert_eq!(
        row.envelope.payload["target"],
        "myelin://acme/knowledge/page/team"
    );
    assert_eq!(row.aggregate.0, "myelin://acme/knowledge/page/project");
}

#[test]
fn te7_db_relation_mirrors_created_and_removed() {
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
        "a new edge was created"
    );
    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let cid = emit_relation_edge(&mut tx, &tenant(), &relation, true, None)
        .expect("emit relation.created");
    tx.commit().expect("commit");

    let c = store.row(&cid).expect("relation.created row");
    assert_eq!(c.envelope.type_.0, "knowledge.relation.created");
    assert_eq!(c.envelope.payload["rel"], "relates");
    assert_eq!(c.envelope.payload["rel_class"], "lifecycle");
    assert_eq!(
        c.envelope.payload["source"],
        "myelin://acme/knowledge/row/row7"
    );
    assert_eq!(
        c.envelope.payload["target"],
        "myelin://acme/knowledge/row/row9"
    );

    let mut tx2 = store.begin(Arc::clone(&minter), ctx_base());
    let rid =
        emit_relation_edge(&mut tx2, &tenant(), &relation, false, None).expect("emit removed");
    tx2.commit().expect("commit");
    let r = store.row(&rid).expect("relation.removed row");
    assert_eq!(r.envelope.type_.0, "knowledge.relation.removed");
    assert_eq!(
        r.aggregate.0, c.aggregate.0,
        "create + remove share the edge aggregate (ordered)"
    );
}

#[test]
fn te7_rollup_source_rel_token() {
    let (store, minter) = store_and_minter();
    let relation = DbRelation {
        relation_id: "rel2".into(),
        src_row: "row7".into(),
        dst_ref: ArtifactRef("myelin://acme/knowledge/row/agg".into()),
        rel: RelationKind::RollupSource,
    };
    let mut tx = store.begin(Arc::clone(&minter), ctx_base());
    let id = emit_relation_edge(&mut tx, &tenant(), &relation, true, None).expect("emit");
    tx.commit().expect("commit");
    assert_eq!(
        store.row(&id).unwrap().envelope.payload["rel"],
        "rollup_source"
    );
}

fn seeded_projector(allow: bool) -> Projector<StubId> {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "Incident runbook".into(),
            state: "live".into(),
        },
    );
    let id = if allow {
        StubId::new().allow_read(&root)
    } else {
        StubId::new()
    };
    Projector::new(id, store)
}

#[test]
fn authorized_viewer_gets_the_page_projection() {
    let p = seeded_projector(true);
    let got = p.project(&page_root("7c2"), &viewer("alice"), z()).unwrap();
    assert!(got.is_visible());
    assert_eq!(got.title(), Some("Incident runbook"));
    if let Projected::Visible(proj) = got {
        assert_eq!(proj.state, "live");
        assert_eq!(proj.icon, "page");
        assert_eq!(proj.render_hint, "page");
        assert!(
            proj.sub_anchor.is_none(),
            "a bare-root page has no sub-anchor"
        );
    }
}

#[test]
fn unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title() {
    let p = seeded_projector(false);
    let got = p
        .project(&page_root("7c2"), &viewer("mallory"), z())
        .unwrap();
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
        assert_eq!(t.reason, TombstoneReason::Denied);
        assert_eq!(t.root, page_root("7c2"));
        assert!(
            !t.root.0.contains("Incident"),
            "the root URN is opaque scope, never the title"
        );
        assert_eq!(t.display_text(), "(not available)");
    }
}

#[test]
fn an_id_hiccup_fails_closed_to_a_tombstone() {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "secret".into(),
            state: "live".into(),
        },
    );
    let p = Projector::new(StubId::new().allow_read(&root).with_hiccup(), store);
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert!(
        got.is_tombstone(),
        "an Id hiccup fails closed (never a leak)"
    );
    assert_eq!(got.title(), None);
}

#[test]
fn an_erased_page_projects_to_an_erased_tombstone() {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "gone".into(),
            state: "live".into(),
        },
    );
    store.mark_erased(&root);
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(
        got.title(),
        None,
        "an erased page never leaks its (shredded) title"
    );
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Erased);
        assert_eq!(t.root, root);
    }
}

#[test]
fn a_restricted_subject_projects_to_a_tombstone() {
    let root = page_root("7c2");
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "restricted".into(),
            state: "live".into(),
        },
    );
    store.mark_restricted(&root);
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(got.title(), None);
}

#[test]
fn a_missing_root_projects_to_a_root_gone_tombstone() {
    let root = page_root("does-not-exist");
    let p = Projector::new(StubId::new().allow_read(&root), PageStore::new());
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::RootGone);
        assert_eq!(t.root, root);
    }
}

#[test]
fn the_sub_anchor_ladder_lives_moves_outdates_and_gones() {
    let root = page_root("7c2");
    let live = myelin_refs::mint(&root, myelin_refs::Sub::Block("b9".into())).unwrap();
    let moved = myelin_refs::mint(&root, myelin_refs::Sub::Block("b10".into())).unwrap();
    let outdated = myelin_refs::mint(&root, myelin_refs::Sub::Heading("hIntro".into())).unwrap();
    let gone = myelin_refs::mint(&root, myelin_refs::Sub::Block("b99".into())).unwrap();

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
    let p = Projector::new(StubId::new().allow_read(&root), store);

    let l = p.project(&live, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = l {
        let a = proj.sub_anchor.expect("a #sub carries a sub_anchor");
        assert_eq!(a.kind, "b");
        assert_eq!(a.sub_id, "b9");
        assert_eq!(a.rung, LadderRung::Live);
        assert_eq!(proj.title, "Incident runbook");
    } else {
        panic!("LIVE block must be visible");
    }

    let m = p.project(&moved, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = m {
        assert_eq!(proj.sub_anchor.unwrap().rung, LadderRung::Moved);
    } else {
        panic!("MOVED block must still resolve");
    }

    let o = p.project(&outdated, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = o {
        assert_eq!(proj.sub_anchor.unwrap().rung, LadderRung::Outdated);
    } else {
        panic!("OUTDATED heading must resolve partially");
    }

    let g = p.project(&gone, &viewer("alice"), z()).unwrap();
    assert!(g.is_tombstone());
    assert_eq!(g.title(), None);
    if let Projected::Tombstoned(t) = g {
        assert_eq!(t.reason, TombstoneReason::SubGone);
        assert_eq!(
            t.root, root,
            "a SubGone tombstone carries the root (the embed shows the page)"
        );
    }
}

#[test]
fn ci_subject_sub_ids_preserve_commit_and_context() {
    assert_eq!(
        sub_opaque_id(&myelin_refs::Sub::CommitCheck {
            commit_oid: "abc123".into(),
            context: "ci-test".into(),
        }),
        "commit-abc123/check-ci-test"
    );
    assert_eq!(
        sub_opaque_id(&myelin_refs::Sub::CommitCiResult {
            commit_oid: "abc123".into(),
        }),
        "commit-abc123/ci-result"
    );
}

#[test]
fn a_sub_anchor_of_a_confidential_page_is_denied_carrying_the_root() {
    let root = page_root("7c2");
    let block = myelin_refs::mint(&root, myelin_refs::Sub::Block("b9".into())).unwrap();
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "secret".into(),
            state: "live".into(),
        },
    );
    store.put_sub_state(&block, SubState::Live);
    let p = Projector::new(StubId::new(), store);
    let got = p.project(&block, &viewer("mallory"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(got.title(), None, "the confidential block never leaks");
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Denied);
        assert_eq!(t.root, root, "the tombstone carries the (stripped) root");
    }
}

#[test]
fn an_untracked_sub_defaults_to_live() {
    let root = page_root("7c2");
    let block = myelin_refs::mint(&root, myelin_refs::Sub::Block("bnew".into())).unwrap();
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "t".into(),
            state: "live".into(),
        },
    );
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&block, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = got {
        assert_eq!(proj.sub_anchor.unwrap().rung, LadderRung::Live);
    } else {
        panic!("an un-tracked sub of a live page resolves LIVE");
    }
}

#[test]
fn a_non_knowledge_ref_is_a_loud_error() {
    let git = ArtifactRef("myelin://acme/git/pr/repo:42".into());
    let p = Projector::new(StubId::new().allow_read(&git), PageStore::new());
    assert!(matches!(
        p.project(&git, &viewer("alice"), z()),
        Err(ProjectError::NotAKnowledgeArtifact { .. })
    ));
}

#[test]
fn frozen_rel_class_and_rel_tokens() {
    assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
    assert_ne!(REL_CLASS_REFERENCE, REL_CLASS_LIFECYCLE);
    assert_eq!(KnowledgeLifecycleRel::Parent.as_str(), "parent");
    assert_eq!(KnowledgeLifecycleRel::Relates.as_str(), "relates");
    assert_eq!(
        KnowledgeLifecycleRel::RollupSource.as_str(),
        "rollup_source"
    );
}

#[test]
fn ladder_rung_tokens_are_frozen() {
    assert_eq!(LadderRung::Live.as_str(), "live");
    assert_eq!(LadderRung::Moved.as_str(), "moved");
    assert_eq!(LadderRung::Outdated.as_str(), "outdated");
    assert_ne!(LadderRung::Live.as_str(), LadderRung::Moved.as_str());
    assert_ne!(LadderRung::Moved.as_str(), LadderRung::Outdated.as_str());
}

#[test]
fn projected_predicates_are_exact_complements() {
    let p = seeded_projector(true);
    let visible = p.project(&page_root("7c2"), &viewer("alice"), z()).unwrap();
    assert!(visible.is_visible());
    assert!(
        !visible.is_tombstone(),
        "a visible projection is NOT a tombstone"
    );

    let denied = seeded_projector(false)
        .project(&page_root("7c2"), &viewer("mallory"), z())
        .unwrap();
    assert!(denied.is_tombstone());
    assert!(!denied.is_visible(), "a tombstone is NOT visible");
}

#[test]
fn a_restricted_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = page_root("7c2");
    let block = myelin_refs::mint(&root, myelin_refs::Sub::Block("b9".into())).unwrap();
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "page".into(),
            state: "live".into(),
        },
    );
    store.put_sub_state(&block, SubState::Live);
    store.mark_restricted(&block);
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&block, &viewer("alice"), z()).unwrap();
    assert!(
        got.is_tombstone(),
        "a restricted sub-URN tombstones even when its root is not restricted"
    );
    assert_eq!(got.title(), None);
}

#[test]
fn an_erased_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = page_root("7c2");
    let block = myelin_refs::mint(&root, myelin_refs::Sub::Block("b9".into())).unwrap();
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: "page".into(),
            state: "live".into(),
        },
    );
    store.put_sub_state(&block, SubState::Live);
    store.mark_erased(&block);
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&block, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Erased);
    }
}

#[test]
fn project_error_display_is_loud() {
    let e = ProjectError::NotAKnowledgeArtifact {
        reference: "myelin://acme/git/pr/r:1".into(),
    };
    let s = e.to_string();
    assert!(!s.is_empty());
    assert!(
        s.contains("myelin://acme/git/pr/r:1"),
        "the error names the offending ref"
    );
    let u = ProjectError::UnknownKnowledgeType {
        ty: "widget".into(),
    };
    assert!(u.to_string().contains("widget"));
}

#[test]
fn store_mut_borrows_the_live_store() {
    let root = page_root("seeded-via-mut");
    let mut p = Projector::new(StubId::new().allow_read(&root), PageStore::new());
    p.store_mut().put_root(
        &root,
        PageMeta {
            title: "via store_mut".into(),
            state: "live".into(),
        },
    );
    let got = p.project(&root, &viewer("alice"), z()).unwrap();
    assert_eq!(
        got.title(),
        Some("via store_mut"),
        "the projector reads the store seeded via store_mut"
    );
}
