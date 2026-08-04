use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events::RELATION_CREATED;
use myelin_refs::ArtifactRef;
use myelin_refs_service::{
    ids_result, project_issue_relation, AuthzVisibleIndex, EdgeProjection, IssueEdgeProducer,
    IssueRelationEvent, RelClass, Traverse, TraverseFilter, TRAVERSE_DEPTH_CEILING,
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

fn relation(src: &str, tgt: &str, rel: &str) -> IssueRelationEvent {
    IssueRelationEvent {
        source: IssueEdgeProducer::issue_root("acme", src),
        target: IssueEdgeProducer::issue_root("acme", tgt),
        rel: rel.into(),
        origin_event_id: format!("evt-{src}-{tgt}-{rel}"),
        origin_event_type: RELATION_CREATED.into(),
        origin_actor: "issue-pseudonym".into(),
        zookie: Some("zk-1".into()),
    }
}

#[test]
fn cdc_issue_relation_blocks_is_traversable_in_both_directions() {
    let proj = EdgeProjection::new();
    project_issue_relation(
        &proj,
        &tenant(),
        &region(),
        &relation("ENG-1", "ENG-2", "blocks"),
    )
    .expect("project the real issue_relation event");

    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
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
    assert_eq!(inv.nodes.len(), 1, "ENG-2 is blocked_by exactly ENG-1");
    assert_eq!(inv.nodes[0].artifact.0, eng1);
}

#[test]
fn cdc_spec_to_ship_lineage_is_one_traverse() {
    let proj = EdgeProjection::new();
    let parent = IssueRelationEvent {
        source: IssueEdgeProducer::initiative_root("acme", "PLAT-9"),
        target: IssueEdgeProducer::issue_root("acme", "ENG-1"),
        rel: "parent".into(),
        origin_event_id: "evt-parent".into(),
        origin_event_type: RELATION_CREATED.into(),
        origin_actor: "issue-pseudonym".into(),
        zookie: Some("zk-1".into()),
    };
    project_issue_relation(&proj, &tenant(), &region(), &parent).expect("project parent");
    project_issue_relation(
        &proj,
        &tenant(),
        &region(),
        &relation("ENG-1", "ENG-2", "blocks"),
    )
    .expect("project blocks");

    let plat9 = "myelin://acme/issue/initiative/PLAT-9";
    let eng1 = "myelin://acme/issue/issue/ENG-1";
    let eng2 = "myelin://acme/issue/issue/ENG-2";
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let down = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(plat9.into()),
        &viewer(),
        &TraverseFilter::rels(&["parent"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[eng1], "zk-1"),
    );
    assert_eq!(down.nodes.len(), 1, "PLAT-9 → ENG-1 (parent) is one hop");
    assert_eq!(down.nodes[0].artifact.0, eng1);

    let blocks = t.traverse(
        &tenant(),
        &region(),
        &ArtifactRef(eng1.into()),
        &viewer(),
        &TraverseFilter::rels(&["blocks"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&[eng2], "zk-1"),
    );
    assert_eq!(
        blocks.nodes.len(),
        1,
        "ENG-1 → ENG-2 (blocks) is the next hop"
    );
    assert_eq!(blocks.nodes[0].artifact.0, eng2);
}

#[test]
fn cdc_every_issue_relation_edge_is_lifecycle_class() {
    let proj = EdgeProjection::new();
    for rel in [
        "blocks",
        "parent",
        "relates",
        "closes",
        "depends_on",
        "assigns",
    ] {
        let ids = project_issue_relation(&proj, &tenant(), &region(), &relation("A", "B", rel))
            .unwrap_or_else(|e| panic!("`{rel}` is a known lifecycle rel: {e:?}"));
        assert!(
            !ids.is_empty(),
            "`{rel}` projects at least the forward edge"
        );
    }
    let target = IssueEdgeProducer::issue_root("acme", "B");
    let inbound = proj.inbound_live(&tenant(), &region(), &target);
    assert!(
        inbound.iter().all(|r| r.rel_class == RelClass::Lifecycle),
        "every mirror edge is lifecycle-class"
    );
}
