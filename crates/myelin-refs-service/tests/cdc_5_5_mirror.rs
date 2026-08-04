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

#[test]
fn chained_epic_tree_inverse_pairing_across_hops() {
    let proj = EdgeProjection::new();
    let epic = "myelin://acme/issue/issue/ENG-EPIC";
    let story = "myelin://acme/issue/issue/ENG-STORY";
    let task = "myelin://acme/issue/issue/ENG-TASK";

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

#[test]
fn te7_drift_reconverges_to_the_typed_table() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let stale = "myelin://acme/issue/issue/ENG-STALE";

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
        "the drift is gone - the typed table wins"
    );
}

#[test]
fn mirror_rejects_unknown_lifecycle_token() {
    assert_eq!(LifecycleRel::parse("not_a_real_rel"), None);
    assert_eq!(LifecycleRel::parse("embeds"), None);
}

#[test]
fn every_mirror_edge_is_lifecycle_class() {
    for &rel in LifecycleRel::FORWARD_VOCABULARY {
        let rows = mirror_edges(&tenant(), &typed("s", "t", rel));
        assert!(
            rows.iter().all(|r| r.rel_class == RelClass::Lifecycle),
            "{} mirrors lifecycle-class",
            rel.as_str()
        );
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
