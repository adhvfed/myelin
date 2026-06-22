//! Unit + drill tests for the bounded, cycle-safe recursive-CTE traverse (REF-P13 / P-162;
//! contract 5.3). The drills: **REF-D8** (a cycle + a 1000-deep chain → the walk terminates, the
//! cycle is a diagnostic not a hang, the depth ceiling is honoured) and the **REF-D1 traverse half**
//! (a hop into an unreadable artifact PRUNES the branch — 0 leak). The CDC pair for 5.3 (traverse)
//! lives in `tests/cdc_5_3_traverse.rs`. The mutation floor is stated in the module doc + met here:
//! every boundary (the `>= ceiling` cutoff, the `>= max_nodes` cutoff, the visited-set membership,
//! the prune-the-branch admit, the `rel`/`rel_class` filter, the cycle diagnostic) has a test a
//! mutation flips.

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

/// Insert ONE live edge `source --rel(class)--> target` into the projection. The roots are the
/// (already-stripped) node names — the traverse walks roots (§3.2). PII-free opaque ids.
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

/// A `list_objects` result that admits EVERY node (the unrestricted viewer) — so a test that is NOT
/// about the permission post-filter isolates the WALK bounds. Built as an explicit `Ids` of the
/// named nodes.
fn admit_all(nodes: &[&str]) -> ListObjectsResult {
    ids_result(nodes, "zk-1")
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The straight-line walk + the rel/rel_class filter (§4.5)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **A simple chain `A → B → C` is walked to depth 2; the nodes carry their depth + parent.** The
/// baseline: a bounded outbound BFS over the adjacency list.
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

/// **The `rel` filter is honoured — a `blocks` walk never follows a `mentions` edge.** A mutation
/// that drops the `rel` filter would walk the wrong edges (the cardinal "blocked_by wandered into
/// mentions" sin).
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

/// **The `rel_class` filter is honoured — a `reference`-class walk never follows a `lifecycle`
/// mirror edge.** Distinguishes the two edge families (§3.3).
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// REF-D8 — the traversal bound (cycle → diagnostic, depth ceiling, the 1000-deep chain)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **REF-D8 (cycle guard): a self-referential graph `A → B → A` TERMINATES and reports a cycle.**
/// The visited-set guard stops the re-expansion of `A` — the walk does NOT hang — and the cycle is
/// surfaced as a DIAGNOSTIC. A mutation that skips the visited-set membership check hangs (caught by
/// this test's termination).
#[test]
fn self_referential_graph_terminates_and_reports_cycle() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    link(&proj, "B", "A", "blocks", RelClass::Lifecycle); // the back-edge → a cycle.
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
    // Termination: reaching this assertion at all proves the walk did not hang.
    assert!(
        r.cycle_detected,
        "the A→B→A back-edge is surfaced as a cycle DIAGNOSTIC"
    );
    // B is discovered (depth 1); A is NOT re-discovered (the visited-set guard; it is the root).
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert_eq!(
        ids,
        vec!["B"],
        "B once; the root A is never re-expanded (no infinite walk)"
    );
}

/// **REF-D8 (depth ceiling): a 1000-deep chain truncates at the ceiling (default 16) with the
/// `truncated` marker — never an unbounded scan.** The deepest discovered node sits at the ceiling;
/// the result is PARTIAL + marked. A mutation to the `>=` cutoff (`>`) would descend one hop too
/// deep — caught by the exact-depth assertion.
#[test]
fn depth_ceiling_truncates_with_marker() {
    let proj = EdgeProjection::new();
    // a 1000-deep chain n0 → n1 → … → n1000.
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

    // admit every node so the bound — not permission — is what truncates.
    let all: Vec<String> = (0..=1000).map(|i| format!("n{i}")).collect();
    let all_refs: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    let r = t.traverse(
        &tenant(),
        &region(),
        &aref("n0"),
        &viewer(),
        &TraverseFilter::any(),
        1000, // the caller asks for 1000 — CLAMPED to the ceiling.
        &admit_all(&all_refs),
    );
    assert!(
        r.truncated,
        "a 1000-deep chain under a 16 ceiling is a PARTIAL result"
    );
    assert!(!r.cycle_detected, "a chain (no back-edge) is not a cycle");
    // the deepest discovered node is exactly at the ceiling (16), never 17.
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert_eq!(
        max_depth, TRAVERSE_DEPTH_CEILING,
        "the walk stops EXACTLY at depth 16"
    );
    // exactly 16 nodes discovered (n1..n16); n0 is the root.
    assert_eq!(
        r.nodes.len(),
        TRAVERSE_DEPTH_CEILING as usize,
        "16 nodes, never 17"
    );
}

/// **A caller cannot exceed the ceiling — a requested depth > the budget is clamped.** The clamp is
/// `min(depth, self.depth_ceiling)`; a mutation that drops it lets a caller request an unbounded
/// walk. (Re-stated separately from the truncation test for the clamp boundary itself.)
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
        9999, // far past the ceiling — clamped.
        &admit_all(&all_refs),
    );
    let max_depth = r.nodes.iter().map(|n| n.depth).max().unwrap();
    assert!(
        max_depth <= TRAVERSE_DEPTH_CEILING,
        "the requested 9999 is clamped to 16"
    );
    assert!(r.truncated, "and the result is marked partial");
}

/// **A caller may ask for FEWER hops — a depth of 2 stops at 2 even when the ceiling is 16.** The
/// clamp lets the caller tighten the bound (a shallow impact view), and the result is marked
/// `truncated` because there ARE deeper followable edges past the requested depth.
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

/// **A walk that exhausts the graph BEFORE the ceiling is NOT marked truncated.** A leaf at depth 1
/// (no outbound edges) ends the walk cleanly — `truncated` distinguishes "hit the budget" from
/// "reached the end". A mutation that always sets `truncated` is caught.
#[test]
fn an_exhausted_walk_is_not_truncated() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle); // B is a leaf.
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

/// **A chain whose LAST node sits EXACTLY at the ceiling and is a LEAF is NOT truncated.** A walk
/// that reaches the depth ceiling but the node there has no followable outbound edges has reached
/// the graph's end, not the budget — `truncated` must be false. This pins
/// [`Traverse::has_followable_outbound`]: a mutation that always returns `true` would wrongly mark a
/// leaf-at-ceiling walk truncated. Build a chain of EXACTLY `ceiling` hops (n0..n16, a leaf at n16).
#[test]
fn leaf_exactly_at_the_ceiling_is_not_truncated() {
    let proj = EdgeProjection::new();
    let ceiling = TRAVERSE_DEPTH_CEILING; // 16.
    for i in 0..ceiling {
        link(
            &proj,
            &format!("n{i}"),
            &format!("n{}", i + 1),
            "blocks",
            RelClass::Lifecycle,
        );
    }
    // n{ceiling} is a LEAF (no outbound) sitting exactly at the ceiling.
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
        "a leaf AT the ceiling exhausted the graph — not a truncation (has_followable_outbound=false)"
    );
}

/// **The collected-node budget truncates a SHALLOW but WIDE graph (X-3).** A root with thousands of
/// direct neighbours is bounded by the node budget even at depth 1 — never an unbounded wide scan. A
/// mutation to the `>= max_nodes` cutoff is caught by the exact-count assertion.
#[test]
fn node_budget_truncates_a_wide_graph() {
    let proj = EdgeProjection::new();
    // a star: A → c0, c1, …, c19 ; budget set to 8 so the walk truncates mid-fan-out.
    for i in 0..20 {
        link(&proj, "A", &format!("c{i}"), "links", RelClass::Reference);
    }
    // a small budget so we hit it: with_default_bounds is 10_000 — build a bounded traverse via new()
    // over a thresholds file with a tiny budget.
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
    // visited holds the root + at most (budget-1) neighbours = 8 total.
    assert_eq!(
        r.visited_count, 8,
        "the walk visited exactly the node budget, never more"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// REF-D1 (traverse half) — the leak-free post-filter + the branch-prune (§4.5)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **REF-D1 traverse half: a hop into an UNREADABLE artifact is PRUNED — the traverse never reveals
/// a node the viewer cannot `view`.** `A → secret`: the viewer's `list_objects` admits only `A`'s
/// other neighbour, never `secret`. A mutation that flips the post-filter admit leaks `secret`.
#[test]
fn unreadable_node_is_pruned_no_leak() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "public", "links", RelClass::Reference);
    link(&proj, "A", "secret", "links", RelClass::Reference);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    // the viewer may view `public` but NOT `secret` (it is absent from the admit set).
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
        "secret is pruned — the traverse never reveals it"
    );
    assert!(
        !ids.contains(&"secret"),
        "0 leak: the unreadable node is absent"
    );
}

/// **REF-D1 branch-prune: a node reachable ONLY through an unreadable artifact is ALSO pruned.**
/// `A → secret → deep`: even though the viewer might be able to `view` `deep` in isolation, it is
/// reachable only via the unreadable `secret`, so the WHOLE branch is dropped (the traverse must not
/// reveal "there is something past the artifact you can't see"). A mutation that drops the
/// parent-reachability check leaks `deep`.
#[test]
fn branch_under_an_unreadable_node_is_pruned() {
    let proj = EdgeProjection::new();
    link(&proj, "A", "secret", "blocks", RelClass::Lifecycle);
    link(&proj, "secret", "deep", "blocks", RelClass::Lifecycle);
    link(&proj, "A", "visible", "blocks", RelClass::Lifecycle);
    let t = Traverse::with_default_bounds(proj, AuthzVisibleIndex::new());

    // the viewer may view `visible` AND `deep` in isolation — but NOT `secret`. `deep` is reachable
    // only through `secret`, so it must be pruned with the branch.
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

/// **A node reachable through BOTH a readable and an unreadable parent survives via the readable
/// one.** `A → secret → D` and `A → ok → D`: `D` is reachable via `ok` (readable), so it is admitted
/// — the prune drops a branch only when ALL paths to a node pass through unreadable ancestors. (The
/// BFS reaches `D` first via its minimal-depth readable parent.)
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
        &admit_all(&["ok", "D"]), // secret is unreadable.
    );
    let ids: Vec<&str> = r.nodes.iter().map(|n| n.artifact.0.as_str()).collect();
    assert!(ids.contains(&"D"), "D survives via the readable `ok` path");
    assert!(ids.contains(&"ok"));
    assert!(!ids.contains(&"secret"));
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Tenant isolation + the thresholds source of truth
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **A traverse is tenant-first — it never crosses into another tenant's edges (ID-3).** A node with
/// the same name in another tenant is invisible (the adjacency scan is `(tenant, region)`-keyed).
#[test]
fn traverse_is_tenant_isolated() {
    let proj = EdgeProjection::new();
    // acme's edge A → B.
    link(&proj, "A", "B", "blocks", RelClass::Lifecycle);
    // another tenant's edge A → evil (same node name, different tenant partition).
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

/// **The depth ceiling + node budget are READ FROM the thresholds file (the single source of
/// truth).** A drill reads `t.refs_traverse.{depth_ceiling,max_nodes}` from the FILE, never a
/// literal — and the seed constants mirror the file.
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

/// Build a minimal thresholds TOML with a tiny traverse node budget (for the wide-graph drill) — all
/// other sections are the canonical seeds so the file parses.
fn small_budget_thresholds(max_nodes: u32) -> String {
    let canonical = myelin_substrate::Thresholds::load_canonical().expect("canonical load");
    let mut th = canonical;
    th.refs_traverse.max_nodes = max_nodes;
    th.to_toml().expect("re-serialize thresholds")
}
