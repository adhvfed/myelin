//! **REF-P14 / P-163 — the TE-7 typed-edge mirror discipline (contract 5.5) CDC pair + the chained
//! epic-tree inverse-pairing test + the drift-reconvergence (typed wins) drill.**
//!
//! These run on the default `cargo test --workspace` (DB-free): the synthetic typed events stand in
//! for the real `issue.relation.*` / `knowledge.page.*` events off the typed tables (which arrive in
//! REF-P18/REF-P20). What is proven here:
//!
//! - **CDC 5.5 (provider Refs + consumer cross-subsystem traversal):** the mirror discipline projects
//!   a typed lifecycle event into BOTH inverse-paired `lifecycle`-class edges, so a consumer (an
//!   epic-tree / impact renderer) can traverse the lifecycle graph in EITHER direction with ONE Refs
//!   query — the load-bearing 5.5 promise (cross-subsystem traversal is one query, not a five-way
//!   fan-out).
//! - **The inverse pairing across hops:** synthetic lifecycle events build an epic tree; a `child`
//!   traverse from the root walks DOWN, a `parent` traverse from a leaf walks UP — the SAME edges,
//!   reachable from both ends because the mirror projected both directions.
//! - **The TE-7 drift reconvergence (typed wins):** a drifted projection reconverges to the
//!   authoritative typed snapshot on a scoped reindex — the typed table always wins.
//!
//! The full reindex byte-parity drill (REF-D4 full) is REF-P16/REF-P24; here the TE-7 reconvergence
//! SEMANTICS are drilled. FLOOR: the producers are SYNTHETIC at M2 (real typed mirrors REF-P18/REF-P20).

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_refs_service::{
    ids_result, mirror_edges, project_typed_event, reconverge, AuthzVisibleIndex, EdgeProjection,
    Inverse, LifecycleRel, RelClass, SyntheticTypedEvent, Traverse, TraverseFilter,
    TRAVERSE_DEPTH_CEILING,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p-viewer".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

fn typed(source: &str, target: &str, rel: LifecycleRel) -> SyntheticTypedEvent {
    SyntheticTypedEvent {
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel,
        origin_event: "01J-typed".into(),
        origin_actor: "p-author".into(),
        zookie: Some("zk-1".into()),
    }
}

/// **CDC 5.5 (provider + consumer): a synthetic `blocks` event yields both directions; a consumer can
/// traverse the lifecycle graph either way with ONE Refs query.** ENG-1 blocks ENG-2 ⇒ a `blocks`
/// walk from ENG-1 reaches ENG-2 AND a `blocked_by` walk from ENG-2 reaches ENG-1 — the inverse
/// pairing makes cross-subsystem traversal one query in either direction.
#[test]
fn cdc_blocks_event_is_traversable_in_both_directions() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed(eng1, eng2, LifecycleRel::Blocks),
    )
    .unwrap();

    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    // forward: what does ENG-1 block? → ENG-2.
    let fwd = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(eng1.into()),
        &viewer(),
        &TraverseFilter::rels(&["blocks"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[eng2], "zk-1"),
    );
    assert_eq!(fwd.nodes.len(), 1, "ENG-1 blocks exactly ENG-2");
    assert_eq!(fwd.nodes[0].artifact.0, eng2);

    // inverse: what is ENG-2 blocked_by? → ENG-1 (the SAME logical relation, the other direction).
    let inv = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(eng2.into()),
        &viewer(),
        &TraverseFilter::rels(&["blocked_by"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[eng1], "zk-1"),
    );
    assert_eq!(
        inv.nodes.len(),
        1,
        "ENG-2 is blocked_by exactly ENG-1 (the inverse direction)"
    );
    assert_eq!(inv.nodes[0].artifact.0, eng1);
}

/// **The chained epic-tree test: synthetic lifecycle events → traverse an epic tree → correct inverse
/// pairing across hops.** Build epic → story → task with `parent` events; a `child` walk from the epic
/// descends to every descendant; a `parent` walk from the task ascends to the epic — the SAME edges,
/// reachable from both ends because the mirror projected `parent` AND `child`.
#[test]
fn chained_epic_tree_inverse_pairing_across_hops() {
    let proj = EdgeProjection::new();
    let epic = "myelin://acme/issue/issue/ENG-EPIC";
    let story = "myelin://acme/issue/issue/ENG-STORY";
    let task = "myelin://acme/issue/issue/ENG-TASK";

    // synthetic typed lifecycle events: epic is the parent of story; story is the parent of task.
    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed(epic, story, LifecycleRel::Parent),
    )
    .unwrap();
    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed(story, task, LifecycleRel::Parent),
    )
    .unwrap();

    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    // DOWN the tree: a `child` walk from the epic reaches story THEN task (the mirror projected
    // child edges epic→? no — child runs leaf→parent; the DOWNWARD walk follows `parent` edges
    // epic→story→task). Walk `parent` from epic to descend.
    let down = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(epic.into()),
        &viewer(),
        &TraverseFilter::rels(&["parent"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[story, task], "zk-1"),
    );
    let mut down_ids: Vec<String> = down.nodes.iter().map(|n| n.artifact.0.clone()).collect();
    down_ids.sort();
    assert_eq!(
        down_ids,
        vec![story.to_string(), task.to_string()],
        "parent walk descends the whole tree"
    );

    // UP the tree: a `child` walk from the task ascends task→story→epic (the inverse `child` edges
    // the mirror projected — task is the child of story, story is the child of epic).
    let up = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(task.into()),
        &viewer(),
        &TraverseFilter::rels(&["child"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[story, epic], "zk-1"),
    );
    let mut up_ids: Vec<String> = up.nodes.iter().map(|n| n.artifact.0.clone()).collect();
    up_ids.sort();
    assert_eq!(
        up_ids,
        vec![epic.to_string(), story.to_string()],
        "child walk ascends to the epic"
    );
}

/// **The TE-7 drift-reconvergence half of REF-D4: a synthetic drift reconverges to the typed table
/// (typed wins) on a scoped reindex.** A stale lifecycle edge (the projection disagrees with the typed
/// table) is tombstoned when the authoritative typed snapshot is re-emitted; the typed truth is live.
#[test]
fn te7_drift_reconverges_to_the_typed_table() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let stale = "myelin://acme/issue/issue/ENG-STALE";

    // DRIFT: the projection holds a stale ENG-STALE blocks ENG-2 the typed table no longer backs.
    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed(stale, eng2, LifecycleRel::Blocks),
    )
    .unwrap();
    let before = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng2.into()));
    assert!(
        before.iter().any(|r| r.source.0 == stale),
        "the drifted edge is live before reindex"
    );

    // the AUTHORITATIVE typed snapshot for target ENG-2: ENG-1 blocks ENG-2 (NOT ENG-STALE).
    let snapshot = vec![typed(eng1, eng2, LifecycleRel::Blocks)];
    let covered = vec![ArtifactRef(eng2.into())];
    let (reprojected, tombstoned) = reconverge(
        &proj,
        &tenant(),
        &region(),
        &snapshot,
        &covered,
        "01J-reindex",
    )
    .unwrap();

    assert_eq!(
        reprojected, 2,
        "the typed snapshot re-projects both directions of ENG-1 blocks ENG-2"
    );
    assert_eq!(
        tombstoned, 1,
        "the drifted ENG-STALE edge is tombstoned (typed wins)"
    );

    let after = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng2.into()));
    assert!(
        after.iter().any(|r| r.source.0 == eng1),
        "the typed truth is live after reconvergence"
    );
    assert!(
        !after.iter().any(|r| r.source.0 == stale),
        "the drift is gone — the typed table wins"
    );
}

/// **The mirror discipline rejects an unknown lifecycle token (REF-3 — never guesses).** This guards
/// the vocabulary chokepoint a real consumer would run a typed event through.
#[test]
fn mirror_rejects_unknown_lifecycle_token() {
    assert_eq!(LifecycleRel::parse("not_a_real_rel"), None);
    // and a reference-class rel is NOT a lifecycle rel (the two classes never alias).
    assert_eq!(LifecycleRel::parse("embeds"), None);
}

/// **Every mirrored edge is `lifecycle`-class (the discipline), and the inverse shapes are exhaustive.**
/// A belt-and-braces invariant over the whole vocabulary: no mirror edge is ever `reference`-class.
#[test]
fn every_mirror_edge_is_lifecycle_class() {
    for &rel in LifecycleRel::FORWARD_VOCABULARY {
        let rows = mirror_edges(&tenant(), &typed("s", "t", rel));
        assert!(
            rows.iter().all(|r| r.rel_class == RelClass::Lifecycle),
            "{} mirrors lifecycle-class",
            rel.as_str()
        );
        // the row count matches the inverse shape: Paired/Symmetric → 2, None → 1.
        let expected = match rel.inverse() {
            Inverse::Paired(_) | Inverse::Symmetric => 2,
            Inverse::None => 1,
        };
        assert_eq!(
            rows.len(),
            expected,
            "{} projects {} edge(s)",
            rel.as_str(),
            expected
        );
    }
}
