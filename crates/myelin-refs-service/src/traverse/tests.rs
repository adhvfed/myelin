use super::*;
use crate::backlinks::ids_result;
use crate::edge_builder::{edge_id, EdgeRow, RelClass};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

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
fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(s.into())
}

fn link(proj: &EdgeProjection, source: &str, target: &str, rel: &str, class: RelClass) {
    let row = EdgeRow {
        edge_id: edge_id(&tenant(), source, target, rel),
        source: aref(source),
        source_root: aref(source),
        target: aref(target),
        target_root: aref(target),
        rel: rel.into(),
        rel_class: class,
        origin_event: "ev".into(),
        origin_actor: "p-author".into(),
        zookie: None,
        tombstoned: false,
    };
    proj.upsert(&tenant(), &region(), row);
}

fn admit_all(nodes: &[&str]) -> ListObjectsResult {
    ids_result(nodes, "zk-1")
}

#[test]
fn straight_chain_is_walked_with_depths_and_parents() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    link(&proj, "B", "C", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["B", "C"]),
    );
    assert!(!r.truncated, "a 2-hop chain under a 16 ceiling is complete");
    assert!(!r.cycle_detected, "no cycle in a straight chain");
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["B", "C"],
        "B (depth 1) then C (depth 2), root excluded"
    );
    assert_eq!(r.nodes[0].depth, 1);
    assert_eq!(r.nodes[0].parent.0, "A");
    assert_eq!(r.nodes[1].depth, 2);
    assert_eq!(r.nodes[1].parent.0, "B");
}

#[test]
fn rel_filter_does_not_follow_other_relations() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    link(&proj, "A", "X", "mentions", RelClass::Reference);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::rels(&["blocks"]),
        16,
        &admit_all(&["B", "X"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["B"],
        "only the `blocks` neighbour, never the `mentions` X"
    );
}

#[test]
fn rel_class_filter_separates_reference_from_lifecycle() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    link(&proj, "A", "C", "links", RelClass::Reference);
    let filter = TraverseFilter {
        rels: vec![],
        rel_class: Some(RelClass::Reference),
    };
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &filter,
        16,
        &admit_all(&["B", "C"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(ids, vec!["C"], "only the reference-class neighbour C");
}

#[test]
fn self_referential_graph_terminates_and_reports_cycle() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    link(&proj, "B", "A", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["A", "B"]),
    );
    assert!(
        r.cycle_detected,
        "the A→B→A back-edge is surfaced as a cycle DIAGNOSTIC"
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["B"],
        "B once; the root A is never re-expanded (no infinite walk)"
    );
}

#[test]
fn depth_ceiling_truncates_with_marker() {
    let proj = EdgeProjection::new();
    for i in 0..1000 {
        link(
            &proj,
            &format!("n{i}"),
            &format!("n{}", i + 1),
            "blocks",
            RelClass::Lifecycle,
        );
    }
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());
    assert_eq!(
        t.depth_ceiling(),
        TRAVERSE_DEPTH_CEILING,
        "the seed ceiling is 16"
    );

    let all: Vec<String> = (0..=1000).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        1000,
        &admit_all(&all_refs),
    );
    assert!(
        r.truncated,
        "a 1000-deep chain under a 16 ceiling is a PARTIAL result"
    );
    assert!(!r.cycle_detected, "a chain (no back-edge) is not a cycle");
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert_eq!(
        max_depth, TRAVERSE_DEPTH_CEILING,
        "the walk stops EXACTLY at depth 16"
    );
    assert_eq!(
        r.nodes.len(),
        TRAVERSE_DEPTH_CEILING as usize,
        "16 nodes, never 17"
    );
}

#[test]
fn caller_cannot_exceed_the_ceiling() {
    let proj = EdgeProjection::new();
    for i in 0..30 {
        link(
            &proj,
            &format!("n{i}"),
            &format!("n{}", i + 1),
            "blocks",
            RelClass::Lifecycle,
        );
    }
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());
    let all: Vec<String> = (0..=30).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        9999,
        &admit_all(&all_refs),
    );
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert!(
        max_depth <= TRAVERSE_DEPTH_CEILING,
        "the requested 9999 is clamped to 16"
    );
    assert!(r.truncated, "and the result is marked partial");
}

#[test]
fn caller_may_request_a_shallower_walk() {
    let proj = EdgeProjection::new();
    for i in 0..10 {
        link(
            &proj,
            &format!("n{i}"),
            &format!("n{}", i + 1),
            "blocks",
            RelClass::Lifecycle,
        );
    }
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());
    let all: Vec<String> = (0..=10).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        2,
        &admit_all(&all_refs),
    );
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert_eq!(max_depth, 2, "the caller's depth-2 request is honoured");
    assert!(r.truncated, "there are deeper edges past depth 2 → partial");
}

#[test]
fn an_exhausted_walk_is_not_truncated() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["B"]),
    );
    assert!(
        !r.truncated,
        "the graph is exhausted before the ceiling → not truncated"
    );
    assert_eq!(r.nodes.len(), 1);
}

#[test]
fn leaf_exactly_at_the_ceiling_is_not_truncated() {
    let proj = EdgeProjection::new();
    let ceiling = TRAVERSE_DEPTH_CEILING;
    for i in 0..ceiling {
        link(
            &proj,
            &format!("n{i}"),
            &format!("n{}", i + 1),
            "blocks",
            RelClass::Lifecycle,
        );
    }
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());
    let all: Vec<String> = (0..=ceiling).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        ceiling,
        &admit_all(&all_refs),
    );
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert_eq!(max_depth, ceiling, "the leaf sits exactly at the ceiling");
    assert!(
        !r.truncated,
        "a leaf AT the ceiling exhausted the graph - not a truncation (has_followable_outbound=false)"
    );
}

#[test]
fn node_budget_truncates_a_wide_graph() {
    let proj = EdgeProjection::new();
    for i in 0..20 {
        link(&proj, "A", &format!("c{i}"), "links", RelClass::Reference);
    }
    let toml = small_budget_thresholds(8);
    let th = myelin_substrate::Thresholds::from_toml(&toml).expect("thresholds parse");
    let t = Traverse::new(proj, AuthzVisibleIndex::new(), &th);
    assert_eq!(t.max_nodes(), 8, "the budget read from the file");

    let all: Vec<String> = (0..20).map(|i| format!("c{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&all_refs),
    );
    assert!(
        r.truncated,
        "a wide graph past the node budget is a PARTIAL result"
    );
    assert_eq!(
        r.visited_count, 8,
        "the walk visited exactly the node budget, never more"
    );
}

#[test]
fn unreadable_node_is_pruned_no_leak() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "public", "links", RelClass::Reference);
    link(&proj, "A", "secret", "links", RelClass::Reference);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["public"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["public"],
        "secret is pruned - the traverse never reveals it"
    );
    assert!(
        !ids.contains(&"secret"),
        "0 leak: the unreadable node is absent"
    );
}

#[test]
fn branch_under_an_unreadable_node_is_pruned() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "secret", "blocks", RelClass::Lifecycle);
    link(&proj, "secret", "deep", "blocks", RelClass::Lifecycle);
    link(&proj, "A", "visible", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["visible", "deep"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["visible"],
        "deep is pruned with its unreadable ancestor `secret`"
    );
    assert!(
        !ids.contains(&"deep"),
        "0 leak: a node behind an unreadable hop is absent"
    );
    assert!(!ids.contains(&"secret"));
}

#[test]
fn node_with_a_readable_alternate_path_survives() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "ok", "blocks", RelClass::Lifecycle);
    link(&proj, "A", "secret", "blocks", RelClass::Lifecycle);
    link(&proj, "ok", "D", "blocks", RelClass::Lifecycle);
    link(&proj, "secret", "D", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["ok", "D"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert!(ids.contains(&"D"), "D survives via the readable `ok` path");
    assert!(ids.contains(&"ok"));
    assert!(!ids.contains(&"secret"));
}

#[test]
fn traverse_is_tenant_isolated() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    let other = TenantId("other".into());
    let row = EdgeRow {
        edge_id: edge_id(&other, "A", "evil", "blocks"),
        source: aref("A"),
        source_root: aref("A"),
        target: aref("evil"),
        target_root: aref("evil"),
        rel: "blocks".into(),
        rel_class: RelClass::Lifecycle,
        origin_event: "ev".into(),
        origin_actor: "p".into(),
        zookie: None,
        tombstoned: false,
    };
    proj.upsert(&other, &region(), row);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        16,
        &admit_all(&["B", "evil"]),
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["B"],
        "only acme's edge; the other tenant's `evil` is invisible"
    );
}

#[test]
fn bounds_read_from_the_thresholds_file() {
    let th = myelin_substrate::Thresholds::load_canonical().expect("canonical thresholds load");
    assert_eq!(
        depth_ceiling_from_thresholds(&th),
        TRAVERSE_DEPTH_CEILING,
        "the file's [refs_traverse] depth_ceiling mirrors the seed (16)"
    );
    assert_eq!(
        max_nodes_from_thresholds(&th),
        TRAVERSE_MAX_NODES,
        "the file's [refs_traverse] max_nodes mirrors the seed (10000)"
    );
}

fn small_budget_thresholds(max_nodes: u32) -> String {
    let canonical = myelin_substrate::Thresholds::load_canonical().expect("canonical load");
    let mut th = canonical;
    th.refs_traverse.max_nodes = max_nodes;
    th.to_toml().expect("re-serialize thresholds")
}
