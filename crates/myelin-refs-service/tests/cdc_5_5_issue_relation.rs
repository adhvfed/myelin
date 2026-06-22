//! **REF-P20 / P-336 — the SECOND real TE-7 mirror (Issues `issue_relation`, contract 5.5) CDC pair.**
//!
//! Runs on the default `cargo test --workspace` (DB-free): the [`IssueRelationEvent`] is the REAL
//! typed-lifecycle event off the Issues `issue_relation` table (the M2 `SyntheticTypedEvent` stand-in
//! is RETIRED for Issues relations). This proves the **Refs consumer side of 5.5 for Issues** — the
//! provider/consumer CDC pair for the second real mirror:
//!
//! - **provider (Refs):** `project_issue_relation` projects a real `issue.relation.*` event into BOTH
//!   inverse-paired `lifecycle`-class edges (`blocks→blocked_by`, `parent→child`, symmetric `relates`),
//!   over the WHOLE lifecycle vocabulary — not just KN's single `parent` rel;
//! - **consumer (cross-subsystem traversal):** a consumer (the spec-to-ship lineage / impact renderer)
//!   walks the lifecycle graph in EITHER direction with ONE Refs query — the load-bearing 5.5 promise.
//!
//! The TE-7 second-mirror reconvergence (the typed table wins on a scoped reindex) is drilled in the
//! lib unit tests (`issues_producer::tests`) + proven over real Postgres in the integration test;
//! here the CDC pair anchors the consumer-side traversal contract. FLOOR: Chat unfurls (the maximal
//! consumer; the complete five-producer corpus) are REF-P21.

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

/// **CDC 5.5 (provider + consumer): a real `issue.relation.created` `blocks` event yields both
/// directions; a consumer traverses the lifecycle graph either way with ONE Refs query.** ENG-1 blocks
/// ENG-2 ⇒ a `blocks` walk from ENG-1 reaches ENG-2 AND a `blocked_by` walk from ENG-2 reaches ENG-1 —
/// the inverse pairing makes cross-subsystem traversal one query in either direction. This is the
/// SECOND real mirror (the first was KN `page_parent`, REF-P18).
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
    assert_eq!(inv.nodes.len(), 1, "ENG-2 is blocked_by exactly ENG-1");
    assert_eq!(inv.nodes[0].artifact.0, eng1);
}

/// **The spec-to-ship lineage is ONE Refs traverse (§4.5): initiative → child issue → blocked issue.**
/// A real `parent` relation (`initiative PLAT-9 parent ENG-1`) + a real `blocks` relation
/// (`ENG-1 blocks ENG-2`), both projected by the second mirror, let a single recursive walk follow the
/// lifecycle chain — not a five-way fan-out. This is exactly what the second mirror buys (the prompt's
/// headline: ONE Refs traverse, not a five-way fan-out).
#[test]
fn cdc_spec_to_ship_lineage_is_one_traverse() {
    let proj = EdgeProjection::new();
    // initiative PLAT-9 parents ENG-1; ENG-1 blocks ENG-2.
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

    // ONE `parent` walk from the initiative descends to ENG-1 (the lineage's first hop).
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

    // ONE `blocks` walk from ENG-1 reaches ENG-2 (the next hop of the same lineage).
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

/// **Every mirrored Issues relation edge is `lifecycle`-class (the discipline) — never `reference`.**
/// Belt-and-braces over the second real mirror: the typed-table mirror never aliases a reference edge.
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
    // Every live edge in the projection is lifecycle-class (the mirror never emits a reference edge).
    let target = IssueEdgeProducer::issue_root("acme", "B");
    let inbound = proj.inbound_live(&tenant(), &region(), &target);
    assert!(
        inbound.iter().all(|r| r.rel_class == RelClass::Lifecycle),
        "every mirror edge is lifecycle-class"
    );
}
