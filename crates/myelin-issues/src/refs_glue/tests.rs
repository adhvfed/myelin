//! Unit tests for the Issues Refs wiring (ISS-P17 / P-383): the `#sub` mints (5.7), the inline-node
//! `refs.edge.created` producer (5.4), the TE-7 typed-edge mirror (5.5), the bounded cycle-safe
//! traverse (5.3), the `project(ref, viewer)` 4-step tombstone ladder (5.6 / 5.7), and the `issue.*`
//! Search projection emitter (6.3). The project-leak path is the mandatory-core leak surface — the
//! permission-deny / erased / restricted / sub-gone tombstone tests are the mutation-floor anchors.

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

// ───────────────────────────── shared fixtures ─────────────────────────────

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

/// A deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed); a toggle forces a
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

// ════════════════════════════ 1. the #sub mints (5.7) ════════════════════════════

/// **Each `#sub` mint produces the canonical full sub-URN through the ONE codec, round-trips, and
/// classifies to the right kind; `strip_sub` recovers the parent issue root.** The opaque id is stable
/// across edits (the embed never dangles, §2).
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

/// **A malformed opaque body / a sub-of-a-sub is rejected LOUDLY at mint time (0 ungrammatical mints by
/// construction).**
#[test]
fn issues_sub_mints_reject_a_malformed_body() {
    let root = issue("ENG-1");
    // an empty comment id
    assert!(comment_sub_ref(&root, "").is_err());
    // a sub-of-a-sub (the root must be a bare issue)
    let already = comment_sub_ref(&root, "c1").unwrap();
    assert!(comment_sub_ref(&already, "c2").is_err());
}

// ════════════════════════════ 2. the inline-node refs.edge.created producer (5.4) ════════════════

/// **Each `mention`/`artifact_ref`/`embed` node emits ONE `refs.edge.created` (reference-class), NOT
/// coalesced, with the references-not-payloads triple + the shared edge aggregate.** A `mention`
/// targets the opaque principal URN (no inline PII).
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

    assert_eq!(ids.len(), 3, "one edge per structured node — NOT coalesced");
    assert_eq!(store.outbox_depth(), 3);

    let rows: Vec<_> = store.committed_rows();
    // mention → mentions @ the opaque principal URN, reference-class, no inline PII.
    let mention = store.row(&ids[0]).unwrap();
    assert_eq!(mention.envelope.type_.0, REFS_EDGE_CREATED);
    assert_eq!(mention.envelope.payload["rel"], "mentions");
    assert_eq!(mention.envelope.payload["rel_class"], REL_CLASS_REFERENCE);
    assert_eq!(
        mention.envelope.payload["target"],
        "myelin://acme/identity/principal/alice"
    );
    assert!(!mention.envelope.contains_personal_data, "no inline PII");
    // artifact_ref → references @ the verbatim target.
    let aref = store.row(&ids[1]).unwrap();
    assert_eq!(aref.envelope.payload["rel"], "references");
    assert_eq!(aref.envelope.payload["target"], "myelin://acme/git/pr/4291");
    // embed → embeds.
    let embed = store.row(&ids[2]).unwrap();
    assert_eq!(embed.envelope.payload["rel"], "embeds");
    // every edge is reference-class (never lifecycle).
    assert!(rows
        .iter()
        .all(|r| r.envelope.payload["rel_class"] == REL_CLASS_REFERENCE));
}

/// **Emit-iff-committed: an aborted body persist drops the buffered edges with it (0 edge without its
/// committed node).**
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
    drop(tx); // abort — no commit
    assert_eq!(store.outbox_depth(), 0, "0 edge without a committed node");
}

// ════════════════════════════ 3. the TE-7 typed-edge mirror (5.5) ════════════════════════════

/// **An `issue_relation` write co-commits its TE-7 mirror event (`issue.relation.created`), lifecycle-
/// class, references-not-payloads.** ONE event yields BOTH directions (the Refs mirror fixes the
/// inverse).
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

/// **An unrelate co-commits `issue.relation.removed` on the SAME edge aggregate (the create → remove
/// sequence is per-aggregate ordered).**
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

/// **The lifecycle-rel vocabulary round-trips byte-identically to the `issue_relation.rel` CHECK
/// tokens (no second vocabulary).**
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

// ════════════════════════════ 4. the bounded cycle-safe traverse (5.3) ════════════════════════════

/// **The traverse walks the forward edges, is rel-filtered, and returns the reachable set (not the
/// seed) with the BFS depth.**
#[test]
fn traverse_walks_forward_edges_rel_filtered() {
    let a = issue("ENG-1");
    let b = issue("ENG-2");
    let c = issue("ENG-3");
    let mut g = IssueRelationGraph::new();
    g.add_edge(&a, &b, IssueLifecycleRel::BlockedBy);
    g.add_edge(&b, &c, IssueLifecycleRel::BlockedBy);
    g.add_edge(&a, &c, IssueLifecycleRel::Relates); // a different rel

    // only the blocked_by chain a → b → c
    let reached = g.traverse(&a, Some(IssueLifecycleRel::BlockedBy));
    let nodes: Vec<&str> = reached.iter().map(|n| n.node.0.as_str()).collect();
    assert_eq!(nodes, vec![b.0.as_str(), c.0.as_str()]);
    assert_eq!(reached[0].depth, 1);
    assert_eq!(reached[1].depth, 2);
    // the seed is never returned
    assert!(reached.iter().all(|n| n.node != a));
}

/// **The traverse is CYCLE-SAFE (a `blocked_by` cycle terminates) and DEPTH-BOUNDED (a node past depth
/// 16 is not expanded).** This is the mutation-floor anchor for the bound + the visited-set.
#[test]
fn traverse_is_cycle_safe_and_depth_bounded() {
    // A cycle A → B → A: the walk visits B once and terminates (no infinite loop).
    let a = issue("CY-A");
    let b = issue("CY-B");
    let mut cyclic = IssueRelationGraph::new();
    cyclic.add_edge(&a, &b, IssueLifecycleRel::BlockedBy);
    cyclic.add_edge(&b, &a, IssueLifecycleRel::BlockedBy);
    let reached = cyclic.traverse(&a, None);
    assert_eq!(reached.len(), 1, "B is reached once; the cycle terminates");
    assert_eq!(reached[0].node, b);

    // A long chain deeper than the bound: every reached node is at depth ≤ 16, and the chain stops
    // expanding at the bound (the 18th node is never reached).
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
    // exactly TRAVERSE_MAX_DEPTH nodes reachable (depths 1..=16); the root is depth 0 (not returned).
    assert_eq!(reached.len(), TRAVERSE_MAX_DEPTH);
    assert!(
        !reached.iter().any(|n| n.node == nodes[17]),
        "the node past the bound is not reached"
    );
}

// ════════════════════════════ 5. project(ref, viewer) — the 4-step ladder (5.6 / 5.7) ════════════

/// **A permitted viewer gets the per-viewer projection (title/state/category/icon/render_hint).**
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

/// **MANDATORY-CORE: an unauthorized viewer (a confidential issue) gets a Tombstone carrying the ROOT,
/// NEVER the title (the ISS-D3 0-leak unfurl property re-asserted at the project() boundary).** A
/// deny / a transport hiccup both fail CLOSED.
#[test]
fn unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title() {
    let root = issue("ENG-7");
    // The viewer is NOT on the allow-list (a confidential issue) — Deny.
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

    // A transport hiccup ALSO fails closed to a tombstone (never a leak).
    let mut store2 = IssueProjectionStore::new();
    store2.put_issue(&root, meta("secret"));
    let proj2 = Projector::new(StubId::new().allow_view(&root).with_hiccup(), store2);
    let out2 = proj2.project(&root, &viewer("v"), z()).unwrap();
    assert!(out2.is_tombstone());
    assert_eq!(out2.title(), None);
}

/// **An erased issue projects to an `Erased` tombstone carrying the root (the per-subject DEK shred /
/// the `issue.*.erased` tombstone) — even for a permitted viewer.**
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

/// **MANDATORY-CORE: a RESTRICTED sub-URN tombstones even when the root is not restricted (the `||`
/// guard over root-OR-full-ref). The restriction window suppresses the specific part.**
#[test]
fn a_restricted_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = issue("ENG-11");
    let sub = comment_sub_ref(&root, "abc").unwrap();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("the parent is fine"));
    store.mark_restricted(&sub); // the SUB is restricted, not the root
    let proj = Projector::new(StubId::new().allow_view(&root), store);

    // the bare root still projects (the root is not restricted)
    assert!(proj.project(&root, &viewer("v"), z()).unwrap().is_visible());
    // the restricted sub-URN tombstones
    let out = proj.project(&sub, &viewer("v"), z()).unwrap();
    assert!(out.is_tombstone());
    assert_eq!(out.title(), None);
}

/// **MANDATORY-CORE: an ERASED sub-URN tombstones even when the root is not erased (the `||` guard
/// over root-OR-full-ref on the erasure arm).** Kills the `|| → &&` mutant on the erased guard.
#[test]
fn an_erased_sub_urn_tombstones_even_when_the_root_is_not() {
    let root = issue("ENG-12");
    let sub = comment_sub_ref(&root, "gone").unwrap();
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("the parent is fine"));
    store.mark_erased(&sub); // the SUB is erased, not the root
    let proj = Projector::new(StubId::new().allow_view(&root), store);

    // the bare root still projects (the root is not erased)
    assert!(proj.project(&root, &viewer("v"), z()).unwrap().is_visible());
    // the erased sub-URN tombstones with the Erased reason; the title never leaks
    let out = proj.project(&sub, &viewer("v"), z()).unwrap();
    assert_eq!(out.title(), None);
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("erased sub tombstones"),
    };
    assert_eq!(t.reason, TombstoneReason::Erased);
}

/// **A dangling root projects to a `RootGone` tombstone (the issue does not exist).**
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

/// **The sub-anchor ladder lives / moves / outdates / gones (the 4-step ladder's step 3).** A live/
/// moved/outdated sub projects a `SubAnchor` on that rung; a gone sub is a `SubGone` tombstone carrying
/// the root.
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

    // LIVE (untracked sub defaults to live)
    let p = match proj.project(&live, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("live sub visible"),
    };
    assert_eq!(p.sub_anchor.as_ref().unwrap().rung, LadderRung::Live);
    assert_eq!(p.sub_anchor.as_ref().unwrap().kind, "comment-");
    assert_eq!(p.sub_anchor.as_ref().unwrap().sub_id, "c-live");

    // MOVED
    let p = match proj.project(&moved, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("moved sub visible"),
    };
    assert_eq!(p.sub_anchor.unwrap().rung, LadderRung::Moved);

    // OUTDATED
    let p = match proj.project(&outdated, &v, z()).unwrap() {
        Projected::Visible(p) => p,
        _ => panic!("outdated sub visible"),
    };
    assert_eq!(p.sub_anchor.unwrap().rung, LadderRung::Outdated);

    // GONE → SubGone tombstone carrying the root
    let out = proj.project(&gone, &v, z()).unwrap();
    let t = match out {
        Projected::Tombstoned(t) => t,
        _ => panic!("gone sub tombstones"),
    };
    assert_eq!(t.reason, TombstoneReason::SubGone);
    assert_eq!(t.root, root);
}

/// **A non-Issues / unknown-type ref is a LOUD error, not a tombstone (the projector owns only Issues
/// types).**
#[test]
fn a_non_issue_ref_is_a_loud_error() {
    let proj = Projector::new(StubId::new(), IssueProjectionStore::new());
    let git = ArtifactRef("myelin://acme/git/pr/1".into());
    assert!(matches!(
        proj.project(&git, &viewer("v"), z()),
        Err(ProjectError::NotAnIssueArtifact { .. })
    ));
}

// ════════════════════════════ 6. the issue.* Search projection emitter (6.3 / 6.4) ════════════════

/// **The LIVE `issue.*` Search projection: a permitted issue projects the title text + the typed facets
/// (the keys are byte-identical to the declared 6.3 spec).** The emitter is the SAME store the
/// projector reads (no second projection path).
#[test]
fn issue_search_projection_emits_text_and_typed_facets() {
    let root = issue("ENG-1421");
    let mut store = IssueProjectionStore::new();
    store.put_issue(&root, meta("Fix the login flow"));
    let fetcher = IssueProjectFetcher::new(store);

    let proj = fetcher.project(&tenant(), &region(), &root).unwrap();
    assert_eq!(proj.text, "Fix the login flow");
    // the typed facets match the declared 6.3 spec keys + FieldTypes.
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

/// **The EMITTER honours an erased/restricted SUB-URN even when the root is not (the `||` guards over
/// the `reference.0` arm).** Kills the `|| → &&` mutants on the emitter's erasure/restriction arms.
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

    // the root projects fine (it is neither erased nor restricted)
    assert!(fetcher.project(&tenant(), &region(), &root).is_ok());
    // the erased sub-URN is Gone (the `erased.contains(&reference.0)` arm)
    assert_eq!(
        fetcher.project(&tenant(), &region(), &erased_sub),
        Err(ProjectFetchError::Gone)
    );
    // the restricted sub-URN is Gone (the `restricted.contains(&reference.0)` arm)
    assert_eq!(
        fetcher.project(&tenant(), &region(), &restricted_sub),
        Err(ProjectFetchError::Gone)
    );
}

/// **The ladder rung tokens are the frozen `live`/`moved`/`outdated` strings + the `Projected`
/// accessors discriminate visible-vs-tombstone (kills the accessor/token mutants).**
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

/// **The projection EMITTER is restriction-/erasure-safe: a restricted/erased issue projects to `Gone`
/// (the index removes the doc — no leak via a search result/count/rank), exactly mirroring the
/// project-time tombstone.**
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
    // a missing issue is likewise Gone (the indexer removes/skips the doc).
    assert_eq!(
        fetcher.project(&tenant(), &region(), &issue("ENG-404")),
        Err(ProjectFetchError::Gone)
    );
}

/// **The declared 6.3 spec ACCEPTS the emitter's projection (the schema/row pairing — the spec is the
/// columnar schema, the emitter is the row).** The facet keys the emitter sets are a subset of the
/// declared `struct_fields`, so the indexer admits the projection without a schema mismatch.
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
