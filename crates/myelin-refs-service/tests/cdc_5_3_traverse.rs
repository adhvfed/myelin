//! **REF-P13 / P-162 — the bounded, cycle-safe recursive-CTE traverse (contract 5.3) provider +
//! consumer CDC pair.**
//!
//! Contract 5.3 also names `traverse(root, rels, depth, viewer) -> [Path]` (OWNED by Refs, the
//! hierarchy / dependency / impact walk). This CDC pair pins the **bounded + leak-free contract** the
//! provider (Refs) promises and the consumers (the epic-tree view / the "everything transitively
//! blocked_by this" impact panel) depend on:
//!
//! - **PROVIDER (Refs):** `traverse` returns a BOUNDED set of discovered nodes — the walk is depth-
//!   capped (default 16), node-budgeted, and cycle-safe (a self-referential graph terminates and is
//!   surfaced as a DIAGNOSTIC, never a hang). A request that exceeds the budget returns a PARTIAL
//!   result + a `truncated` marker, NEVER an unbounded scan. The discovered set is permission-filtered
//!   by ONE `list_objects` post-filter over the COLLECTED node set — a hop into an unreadable artifact
//!   PRUNES that branch (the traversal is not a side-channel; 0 leak).
//! - **CONSUMER (an impact / epic-tree renderer):** a renderer that lists the returned
//!   [`TraverseNode`]s — it can render EVERY returned node WITHOUT a per-node permission re-check
//!   (the provider already pruned the unreadable branches) AND it reads the `truncated` / `cycle_detected`
//!   markers to render "… and more (truncated)" / "this dependency graph has a cycle" rather than
//!   hanging or showing a misleadingly-complete tree. This is the load-bearing 5.3 traverse promise.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    edge_builder::{edge_id, EdgeProjection, EdgeRow, RelClass},
    ids_result, AuthzVisibleIndex, Traverse, TraverseFilter, TraverseNode, TraverseResult,
    TRAVERSE_DEPTH_CEILING,
};
use myelin_tenancy::{Region, TenantId};

use myelin_events::ArtifactRef;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer() -> Principal {
    Principal::stub(PrincipalId("p-viewer".into()), PrincipalKind::Human, tenant())
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

/// A trivial impact-tree renderer (the CONSUMER) — it renders every returned node and reads the
/// bound/diagnostic markers WITHOUT re-checking permission. It returns the rendered node ids + the
/// flags it surfaced to a human.
struct ImpactRenderer;
impl ImpactRenderer {
    fn render(&self, r: &TraverseResult) -> (Vec<String>, bool, bool) {
        // The consumer trusts the provider's prune — no per-node check here.
        let ids: Vec<String> = r.nodes.iter().map(|n: &TraverseNode| n.artifact.0.clone()).collect();
        (ids, r.truncated, r.cycle_detected)
    }
}

/// **PROVIDER + CONSUMER CDC — the epic-tree walk: the provider returns the bounded readable subtree;
/// the consumer renders it without a re-check.** `epic → story1 → task1`, `epic → story2`. The
/// viewer may view all; the renderer lists the discovered nodes.
#[test]
fn cdc_provider_returns_bounded_readable_subtree_consumer_renders() {
    let proj = EdgeProjection::new();
    link(&proj, "epic", "story1", "parent_of", RelClass::Lifecycle);
    link(&proj, "story1", "task1", "parent_of", RelClass::Lifecycle);
    link(&proj, "epic", "story2", "parent_of", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    let result = t.traverse(
        &tenant(),
        &region(),
        &aref("epic"),
        &viewer(),
        &TraverseFilter::rels(&["parent_of"]),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&["story1", "story2", "task1"], "zk-1"),
    );

    let (mut ids, truncated, cycle) = ImpactRenderer.render(&result);
    ids.sort();
    assert_eq!(ids, vec!["story1", "story2", "task1"], "the whole readable epic tree");
    assert!(!truncated, "the tree fits under the ceiling → not truncated");
    assert!(!cycle, "no cycle in a tree");
}

/// **PROVIDER CDC — the leak-free promise: an unreadable hop is pruned, so the consumer can render
/// without a per-node check.** `epic → secret-story → secret-task`: the viewer cannot view
/// `secret-story`, so it AND `secret-task` are absent. The consumer never sees a node it should not.
#[test]
fn cdc_provider_prunes_unreadable_branch_consumer_never_sees_it() {
    let proj = EdgeProjection::new();
    link(&proj, "epic", "ok-story", "parent_of", RelClass::Lifecycle);
    link(&proj, "epic", "secret-story", "parent_of", RelClass::Lifecycle);
    link(&proj, "secret-story", "secret-task", "parent_of", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    // the viewer may view ok-story (and secret-task in isolation) but NOT secret-story.
    let result = t.traverse(
        &tenant(),
        &region(),
        &aref("epic"),
        &viewer(),
        &TraverseFilter::any(),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&["ok-story", "secret-task"], "zk-1"),
    );

    let (ids, _, _) = ImpactRenderer.render(&result);
    assert_eq!(ids, vec!["ok-story".to_string()], "the secret branch is pruned (0 leak)");
    assert!(!ids.contains(&"secret-story".to_string()));
    assert!(!ids.contains(&"secret-task".to_string()), "the branch under the unreadable hop is gone");
}

/// **PROVIDER + CONSUMER CDC — the cycle diagnostic + the truncated marker: the consumer reads them
/// instead of hanging / over-claiming.** A cyclic graph terminates with `cycle_detected`; a too-deep
/// chain terminates with `truncated`. The consumer surfaces both to a human.
#[test]
fn cdc_consumer_reads_cycle_and_truncated_markers() {
    // a cycle.
    let cyclic = EdgeProjection::new();
    link(&cyclic, "A", "B", "blocks", RelClass::Lifecycle);
    link(&cyclic, "B", "A", "blocks", RelClass::Lifecycle);
    let tc = Traverse::with_default_bounds(cyclic, AuthzVisibleIndex::new());
    let rc = tc.traverse(
        &tenant(),
        &region(),
        &aref("A"),
        &viewer(),
        &TraverseFilter::any(),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&["A", "B"], "zk-1"),
    );
    let (_, _, cycle) = ImpactRenderer.render(&rc);
    assert!(cycle, "the consumer is told the graph has a cycle (a diagnostic, not a hang)");

    // a too-deep chain.
    let deep = EdgeProjection::new();
    for i in 0..100 {
        link(&deep, &format!("n{i}"), &format!("n{}", i + 1), "blocks", RelClass::Lifecycle);
    }
    let td = Traverse::with_default_bounds(deep, AuthzVisibleIndex::new());
    let all: Vec<String> = (0..=100).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let rd = td.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        TRAVERSE_DEPTH_CEILING,
        &ids_result(&all_refs, "zk-1"),
    );
    let (ids, truncated, _) = ImpactRenderer.render(&rd);
    assert!(truncated, "the consumer is told the walk is PARTIAL (truncated), never an unbounded scan");
    assert_eq!(ids.len(), TRAVERSE_DEPTH_CEILING as usize, "exactly the depth-16 prefix is returned");
}
