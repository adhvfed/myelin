//! Unit tests for the Knowledge Refs glue (KN-P19 / P-309): the inline-node `refs.edge.created`
//! producer (5.4), the TE-7 typed-edge mirror (5.5), and the `project(ref, viewer)` 4-step tombstone
//! ladder (5.6 / 5.7 / §2.1). The project-leak path is the mandatory-core leak surface — the
//! permission-deny / erased / sub-gone tombstone tests are the mutation-floor anchors.

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

// ───────────────────────────── shared fixtures ─────────────────────────────

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

/// A deterministic Id stub: a `read@object` allow-list (absent ⇒ Deny, fail-closed); a toggle forces a
/// transport hiccup (the projector must then fail CLOSED to a tombstone).
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

// ════════════════════════════ 1. the inline-node refs.edge.created producer (5.4) ═══════════════

/// **Each `mention`/`artifact_ref`/`embed` node emits ONE `refs.edge.created` (reference-class), NOT
/// coalesced, with the references-not-payloads triple + the shared edge aggregate.** A `mention`
/// targets the opaque principal URN (no inline PII).
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
    assert_eq!(ids.len(), 3, "one edge per node — NOT coalesced");
    tx.commit().expect("commit the block persist + its edges");

    // mention → mentions edge → the opaque principal URN.
    let m = store.row(&ids[0]).expect("mention edge row");
    assert_eq!(m.envelope.type_.0, "refs.edge.created");
    assert_eq!(m.envelope.payload["rel"], "mentions");
    assert_eq!(m.envelope.payload["rel_class"], "reference");
    assert_eq!(
        m.envelope.payload["target"],
        "myelin://acme/identity/principal/alice"
    );
    assert!(
        !m.envelope.contains_personal_data,
        "references-not-payloads: opaque principal id, no PII"
    );

    // artifact_ref → references edge → the issue URN verbatim.
    let a = store.row(&ids[1]).expect("artifact_ref edge row");
    assert_eq!(a.envelope.payload["rel"], "references");
    assert_eq!(
        a.envelope.payload["target"],
        "myelin://acme/issue/issue/ENG-1"
    );

    // embed → embeds edge → the embedded page URN; the aggregate is the shared edge convention.
    let e = store.row(&ids[2]).expect("embed edge row");
    assert_eq!(e.envelope.payload["rel"], "embeds");
    assert_eq!(
        e.aggregate.0,
        format!(
            "edge:{}->{}",
            source.0, "myelin://acme/knowledge/page/incident-runbook"
        )
    );
    assert_eq!(e.envelope.payload["source"], source.0);
}

/// **Emit-iff-committed (KN-D7): an aborted block persist drops the buffered edges (no edge without
/// its committed node).**
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
        // tx dropped WITHOUT commit — the crash between the block commit and the relay.
    }
    assert_eq!(
        store.outbox_depth(),
        0,
        "an aborted block persist wrote 0 edges (no edge without its node)"
    );
}

// ════════════════════════════ 2. the TE-7 typed-edge mirror (5.5) ═══════════════════════════════

/// **A `page_parent` typed-row write emits `knowledge.page.parent_set` (lifecycle-class) in the SAME
/// transaction (0 typed-row-without-edge).** The forward `parent` edge; the aggregate is the child
/// page; Refs fixes the inverse `child`.
#[test]
fn te7_page_parent_set_mirrors_the_typed_row_in_the_same_tx() {
    let (store, minter) = store_and_minter();
    // Write the page_parent typed row (the source of truth) + emit its mirror in ONE tx.
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
    // the aggregate is the child page (per-page ordering).
    assert_eq!(row.aggregate.0, "myelin://acme/knowledge/page/project");
}

/// **A `db_relation` typed-row write emits `knowledge.relation.created`/`.removed` (lifecycle-class) in
/// the SAME transaction; relate then unrelate emit on the SAME edge aggregate (ordered).**
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
    // relate (the forward typed row is the source of truth) + emit its mirror.
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

    // unrelate → relation.removed on the SAME edge aggregate (the create→remove sequence is ordered).
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

/// **The rollup_source relation maps to its own lifecycle rel token.**
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

// ════════════════════════════ 3. project(ref, viewer) — the 4-step tombstone ladder (5.6/5.7) ═══

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

/// **Step 1 — an authorized viewer gets the page projection (the frozen `{title,state,icon,
/// render_hint}` shape).**
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

/// **THE PROJECT-LEAK GATE (project-leak counter = 0): a confidential page → a tombstone CARRYING THE
/// ROOT, never the title, for an unauthorized viewer.** Step 1 of the ladder — the deny path never
/// reads the title.
#[test]
fn unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title() {
    let p = seeded_projector(false); // the Id allows NOBODY → every check denies.
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
        "0 title leak — the denied viewer never gets the title"
    );
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Denied);
        // a tombstone ALWAYS carries the root (§2.1) — an opaque scope, never the title.
        assert_eq!(t.root, page_root("7c2"));
        assert!(
            !t.root.0.contains("Incident"),
            "the root URN is opaque scope, never the title"
        );
        assert_eq!(t.display_text(), "(not available)");
    }
}

/// **An Id transport hiccup fails CLOSED to a tombstone (never a leak), even for an allowed viewer.**
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

/// **Step 4 (erased) — an erased page projects to an `Erased` tombstone carrying the root, even for an
/// authorized viewer (the content is shredded).**
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

/// **A restricted subject degrades to the same content-free tombstone (the GDPR suppression window).**
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

/// **Step 2 (root gone) — an authorized viewer of a non-existent root gets a `RootGone` tombstone
/// carrying the (gone) root URN.**
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

/// **The full ladder for a `#sub` block anchor — LIVE / MOVED / OUTDATED each project a SubAnchor on
/// the right rung; GONE tombstones carrying the root (the page resolves, the block is dead).**
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
    // allow_read on the ROOT — the sub inherits the parent permission (checked on the stripped root).
    let p = Projector::new(StubId::new().allow_read(&root), store);

    // LIVE: a SubAnchor on the live rung, the stable block_id.
    let l = p.project(&live, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = l {
        let a = proj.sub_anchor.expect("a #sub carries a sub_anchor");
        assert_eq!(a.kind, "b");
        assert_eq!(a.sub_id, "b9");
        assert_eq!(a.rung, LadderRung::Live);
        // the parent-page title is the projection title.
        assert_eq!(proj.title, "Incident runbook");
    } else {
        panic!("LIVE block must be visible");
    }

    // MOVED: the stable block_id still resolves (a tree move, not a 3-way diff).
    let m = p.project(&moved, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = m {
        assert_eq!(proj.sub_anchor.unwrap().rung, LadderRung::Moved);
    } else {
        panic!("MOVED block must still resolve");
    }

    // OUTDATED: resolves partially, flagged outdated.
    let o = p.project(&outdated, &viewer("alice"), z()).unwrap();
    if let Projected::Visible(proj) = o {
        assert_eq!(proj.sub_anchor.unwrap().rung, LadderRung::Outdated);
    } else {
        panic!("OUTDATED heading must resolve partially");
    }

    // GONE: the root resolves, the block is dead → a SubGone tombstone carrying the root.
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

/// **A `#sub` block of a CONFIDENTIAL page is tombstoned (Denied) — the sub inherits the parent
/// permission; the excerpt/title never leaks.** (A sub is never more visible than its root, §2.1.)
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
    // the Id allows NOBODY → the parent page is denied → the block sub is tombstoned.
    let p = Projector::new(StubId::new(), store);
    let got = p.project(&block, &viewer("mallory"), z()).unwrap();
    assert!(got.is_tombstone());
    assert_eq!(got.title(), None, "the confidential block never leaks");
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Denied);
        assert_eq!(t.root, root, "the tombstone carries the (stripped) root");
    }
}

/// **An un-tracked sub defaults to LIVE (a freshly-minted anchor the store has no state for yet).**
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

/// **A non-Knowledge ref is a loud error, NOT a tombstone (a tombstone is for a hidden Knowledge
/// artifact).**
#[test]
fn a_non_knowledge_ref_is_a_loud_error() {
    let git = ArtifactRef("myelin://acme/git/pr/repo:42".into());
    let p = Projector::new(StubId::new().allow_read(&git), PageStore::new());
    assert!(matches!(
        p.project(&git, &viewer("alice"), z()),
        Err(ProjectError::NotAKnowledgeArtifact { .. })
    ));
}

/// **The frozen rel-class tokens never alias (reference vs lifecycle); the lifecycle rel tokens match
/// the Refs mirror vocabulary.**
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

/// **The ladder-rung tokens are the frozen `live`/`moved`/`outdated` (a mutation that blanks them is
/// caught — the rung token is the consumer's render flag).**
#[test]
fn ladder_rung_tokens_are_frozen() {
    assert_eq!(LadderRung::Live.as_str(), "live");
    assert_eq!(LadderRung::Moved.as_str(), "moved");
    assert_eq!(LadderRung::Outdated.as_str(), "outdated");
    // the three rungs are distinct (no two alias).
    assert_ne!(LadderRung::Live.as_str(), LadderRung::Moved.as_str());
    assert_ne!(LadderRung::Moved.as_str(), LadderRung::Outdated.as_str());
}

/// **`is_visible`/`is_tombstone` are exact complements over the two-variant result (a mutation that
/// pins either to `true` is caught — a tombstone is NOT visible and vice-versa).**
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

/// **A RESTRICTED *sub-URN* (the root NOT restricted) still tombstones — the restriction check is an
/// OR over (root, full-ref), not an AND.** This kills the `||`→`&&` mutant on the restriction guard: a
/// sub-anchor whose specific URN is restricted must tombstone even when the parent page is not.
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
    // ONLY the sub-URN is restricted; the root is NOT.
    store.mark_restricted(&block);
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&block, &viewer("alice"), z()).unwrap();
    assert!(
        got.is_tombstone(),
        "a restricted sub-URN tombstones even when its root is not restricted"
    );
    assert_eq!(got.title(), None);
}

/// **An ERASED *sub-URN* (the root NOT erased) tombstones — the erasure check is an OR over (root,
/// full-ref).** The companion to the restriction OR (the erasure guard's `||`).
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
    store.mark_erased(&block); // ONLY the sub-URN is erased.
    let p = Projector::new(StubId::new().allow_read(&root), store);
    let got = p.project(&block, &viewer("alice"), z()).unwrap();
    assert!(got.is_tombstone());
    if let Projected::Tombstoned(t) = got {
        assert_eq!(t.reason, TombstoneReason::Erased);
    }
}

/// **The `ProjectError` Display is non-empty + names the offending ref (a mutation that blanks the
/// formatter is caught).**
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

/// **`store_mut` returns a live borrow of the SAME store the projector reads (a seed through it is
/// observed by `project`).** Kills the `store_mut -> Box::leak(default)` mutant.
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
