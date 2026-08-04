use super::*;
use crate::edge_builder::RelClass;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn typed_event(source: &str, target: &str, rel: LifecycleRel) -> SyntheticTypedEvent {
    SyntheticTypedEvent {
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel,
        origin_event: "01J-typed".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: Some("zk-1".into()),
    }
}

#[test]
fn vocabulary_is_the_frozen_set_and_rejects_unknown() {
    for &rel in LifecycleRel::FORWARD_VOCABULARY {
        assert_eq!(
            LifecycleRel::parse(rel.as_str()),
            Some(rel),
            "{} round-trips",
            rel.as_str()
        );
    }
    assert_eq!(
        LifecycleRel::FORWARD_VOCABULARY.len(),
        7,
        "the §3.3 vocabulary is 7 rels"
    );
    assert_eq!(LifecycleRel::parse("child"), Some(LifecycleRel::Child));
    assert_eq!(
        LifecycleRel::parse("unblocks"),
        None,
        "unknown token rejected"
    );
    assert_eq!(
        LifecycleRel::parse("mentions"),
        None,
        "a reference rel is not a lifecycle rel"
    );
    assert_eq!(LifecycleRel::parse(""), None, "empty token rejected");
}

#[test]
fn inverse_pairing_is_correct_across_the_relation_set() {
    assert_eq!(
        LifecycleRel::Blocks.inverse(),
        Inverse::Paired(LifecycleRel::BlockedBy)
    );
    assert_eq!(
        LifecycleRel::BlockedBy.inverse(),
        Inverse::Paired(LifecycleRel::Blocks)
    );
    assert_eq!(
        LifecycleRel::Parent.inverse(),
        Inverse::Paired(LifecycleRel::Child)
    );
    assert_eq!(
        LifecycleRel::Child.inverse(),
        Inverse::Paired(LifecycleRel::Parent)
    );
    for &rel in &[
        LifecycleRel::Blocks,
        LifecycleRel::BlockedBy,
        LifecycleRel::Parent,
        LifecycleRel::Child,
    ] {
        if let Inverse::Paired(inv) = rel.inverse() {
            assert_eq!(
                inv.inverse(),
                Inverse::Paired(rel),
                "{} ↔ inverse is reciprocal",
                rel.as_str()
            );
        } else {
            panic!("{} must have a paired inverse", rel.as_str());
        }
    }
    assert_eq!(LifecycleRel::Relates.inverse(), Inverse::Symmetric);
    assert_eq!(LifecycleRel::Closes.inverse(), Inverse::None);
    assert_eq!(LifecycleRel::DependsOn.inverse(), Inverse::None);
    assert_eq!(LifecycleRel::Assigns.inverse(), Inverse::None);
}

#[test]
fn blocks_event_yields_both_blocks_and_blocked_by_with_correct_direction() {
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let rows = mirror_edges(&tenant(), &typed_event(eng1, eng2, LifecycleRel::Blocks));

    assert_eq!(rows.len(), 2, "a blocks event projects BOTH directions");
    let blocks = rows
        .iter()
        .find(|r| r.rel == "blocks")
        .expect("the forward blocks edge");
    let blocked_by = rows
        .iter()
        .find(|r| r.rel == "blocked_by")
        .expect("the inverse blocked_by edge");

    assert_eq!(blocks.source.0, eng1);
    assert_eq!(blocks.target.0, eng2);
    assert_eq!(
        blocked_by.source.0, eng2,
        "inverse edge has the endpoints SWAPPED"
    );
    assert_eq!(blocked_by.target.0, eng1);

    assert_eq!(blocks.rel_class, RelClass::Lifecycle);
    assert_eq!(blocked_by.rel_class, RelClass::Lifecycle);

    assert_eq!(blocks.source_root.0, eng1);
    assert_eq!(blocked_by.source_root.0, eng2);

    assert_ne!(blocks.edge_id, blocked_by.edge_id);
}

#[test]
fn parent_relates_and_floor_rels_mirror_correctly() {
    let a = "myelin://acme/knowledge/page/A";
    let b = "myelin://acme/knowledge/page/B";

    let rows = mirror_edges(&tenant(), &typed_event(a, b, LifecycleRel::Parent));
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|r| r.rel == "parent" && r.source.0 == a && r.target.0 == b));
    assert!(rows
        .iter()
        .any(|r| r.rel == "child" && r.source.0 == b && r.target.0 == a));

    let rows = mirror_edges(&tenant(), &typed_event(a, b, LifecycleRel::Relates));
    assert_eq!(rows.len(), 2, "symmetric relates is visible from both ends");
    assert!(rows.iter().all(|r| r.rel == "relates"));
    assert!(rows.iter().any(|r| r.source.0 == a && r.target.0 == b));
    assert!(rows.iter().any(|r| r.source.0 == b && r.target.0 == a));

    for rel in [
        LifecycleRel::Closes,
        LifecycleRel::DependsOn,
        LifecycleRel::Assigns,
    ] {
        let rows = mirror_edges(&tenant(), &typed_event(a, b, rel));
        assert_eq!(
            rows.len(),
            1,
            "{} mirrors forward-only (floor: no inverse token)",
            rel.as_str()
        );
        assert_eq!(rows[0].rel_class, RelClass::Lifecycle);
    }
}

#[test]
fn mirror_strips_sub_for_roots_keeps_full_urn() {
    let src = "myelin://acme/knowledge/page/7c2#block-9";
    let tgt = "myelin://acme/knowledge/page/abc#block-3";
    let rows = mirror_edges(&tenant(), &typed_event(src, tgt, LifecycleRel::Parent));
    let parent = rows.iter().find(|r| r.rel == "parent").unwrap();
    assert_eq!(parent.source.0, src, "full #sub URN retained");
    assert_eq!(
        parent.source_root.0, "myelin://acme/knowledge/page/7c2",
        "root strips #sub"
    );
    assert_eq!(parent.target_root.0, "myelin://acme/knowledge/page/abc");
}

#[test]
fn project_typed_event_is_idempotent_on_both_directions() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let ev = typed_event(eng1, eng2, LifecycleRel::Blocks);

    project_typed_event(&proj, &tenant(), &region(), &ev).unwrap();
    project_typed_event(&proj, &tenant(), &region(), &ev).unwrap();
    assert_eq!(
        proj.live_count(&tenant(), &region()),
        2,
        "forward + inverse, idempotent"
    );

    let inbound = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng1.into()));
    assert!(
        inbound.iter().any(|r| r.rel == "blocked_by"),
        "the inverse blocked_by edge is inbound to ENG-1 (one-query reverse traversal)"
    );
}

#[test]
fn drift_reconverges_to_the_typed_table_typed_wins() {
    let proj = EdgeProjection::new();
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let eng3 = "myelin://acme/issue/issue/ENG-3";

    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed_event(eng3, eng2, LifecycleRel::Blocks),
    )
    .unwrap();
    proj.upsert(
        &tenant(),
        &region(),
        EdgeRow {
            edge_id: edge_id(&tenant(), "myelin://acme/chat/message/m1", eng2, "mentions"),
            source: ArtifactRef("myelin://acme/chat/message/m1".into()),
            source_root: ArtifactRef("myelin://acme/chat/message/m1".into()),
            target: ArtifactRef(eng2.into()),
            target_root: ArtifactRef(eng2.into()),
            rel: "mentions".into(),
            rel_class: RelClass::Reference,
            origin_event: "01J-ref".into(),
            origin_actor: "p-opaque-2".into(),
            zookie: None,
            tombstoned: false,
        },
    );

    let snapshot = vec![typed_event(eng1, eng2, LifecycleRel::Blocks)];
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
        "the typed snapshot re-projects both directions"
    );
    assert_eq!(
        tombstoned, 1,
        "the drifted ENG-3 blocks ENG-2 edge is tombstoned (typed wins)"
    );

    let inbound = proj.inbound_live(&tenant(), &region(), &ArtifactRef(eng2.into()));
    assert!(
        inbound
            .iter()
            .any(|r| r.rel == "blocks" && r.source.0 == eng1),
        "the typed truth (ENG-1 blocks ENG-2) is live"
    );
    assert!(
        !inbound
            .iter()
            .any(|r| r.rel == "blocks" && r.source.0 == eng3),
        "the drifted ENG-3 edge is tombstoned (typed table wins)"
    );
    assert!(
        inbound.iter().any(|r| r.rel == "mentions"),
        "the reference-class edge is Refs-authoritative - NOT touched by reconvergence"
    );
}

#[test]
fn reconverge_leaves_out_of_scope_edges_alone() {
    let proj = EdgeProjection::new();
    let a = "myelin://acme/issue/issue/A";
    let b = "myelin://acme/issue/issue/B";
    let x = "myelin://acme/issue/issue/X";
    let y = "myelin://acme/issue/issue/Y";

    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed_event(a, b, LifecycleRel::Blocks),
    )
    .unwrap();
    project_typed_event(
        &proj,
        &tenant(),
        &region(),
        &typed_event(x, y, LifecycleRel::Blocks),
    )
    .unwrap();

    let snapshot = vec![typed_event(a, b, LifecycleRel::Blocks)];
    let covered = vec![ArtifactRef(b.into())];
    let (_, tombstoned) = reconverge(
        &proj,
        &tenant(),
        &region(),
        &snapshot,
        &covered,
        "01J-reindex",
    )
    .unwrap();

    assert_eq!(tombstoned, 0, "no drift in the covered scope");
    let inbound_y = proj.inbound_live(&tenant(), &region(), &ArtifactRef(y.into()));
    assert!(
        inbound_y.iter().any(|r| r.rel == "blocks"),
        "out-of-scope edge left alone"
    );
}
