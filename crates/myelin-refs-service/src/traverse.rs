//! The **bounded, cycle-safe recursive-CTE traverse** — `traverse(root, rels, depth, viewer)`
//! (REF-P13 / P-162; contract 5.3 OWNED; consumes 4.3 `list_objects` as the COLLECTED-node
//! post-filter, Identity 4.3).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.5 (the bounded cycle-safe recursive-CTE traverse: a `WITH RECURSIVE` over the `edge`
//! adjacency list filtered by `rel`/`rel_class`, a `path`-array / SQL:2023 `CYCLE` **visited-set
//! cycle guard**, a **depth ceiling** (default 16, read from the thresholds file), a statement
//! timeout, and **ONE** `list_objects` post-filter over the COLLECTED node set — **not per-hop** —
//! where a hop into an unreadable artifact **prunes that branch** (the traversal is not a
//! side-channel); a request exceeding the budget returns a **PARTIAL result + a `truncated`
//! marker**, never an unbounded scan, X-3; a dependency cycle is surfaced as a **DIAGNOSTIC**, not a
//! hang, drill D-8), §3.4 (the `edge` table IS the adjacency list; `edge_outbound`/`edge_by_rel`
//! make the walk indexed). **External insight:** `01-process-and-quality-doctrine.md` §3 (prove-it;
//! the bounded artifact + the leak-free post-filter). **VISION §1** (the reference graph as
//! connective tissue).
//!
//! ## What a traverse IS — a bounded walk, then ONE permission filter
//! `traverse(root, rels, depth, viewer)` answers the multi-hop questions ("the epic tree",
//! "everything transitively `blocked_by` this", "impact if this page is erased"). It is:
//!
//! 1. a **bounded BFS over the `edge` adjacency list** ([`crate::edge_builder::EdgeProjection::outbound_live`]),
//!    following `source_root → target_root` from the `root`, filtered by the requested
//!    `rels`/`rel_class` ([`TraverseFilter`]) — so a `blocked_by` walk never wanders into `mentions`
//!    edges;
//! 2. **cycle-safe**: a `path`-array **visited-set** guard (§4.5) means a self-referential graph
//!    (`A → B → A`) **terminates** — a node already on the walk is NEVER re-expanded. A cycle is
//!    surfaced as a [`TraverseResult::cycle_detected`] DIAGNOSTIC, never a hang (drill REF-D8);
//! 3. **depth-bounded**: the walk descends at most [`TRAVERSE_DEPTH_CEILING`] hops (default 16, read
//!    from the thresholds file — [`crate::traverse::depth_ceiling_from_thresholds`]). A request that
//!    would exceed the ceiling returns a **PARTIAL result + a `truncated` marker**
//!    ([`TraverseResult::truncated`]), never an unbounded scan (X-3);
//! 4. **node-bounded**: even a SHALLOW but WIDE (high-fan-out) graph is bounded by
//!    [`TRAVERSE_MAX_NODES`] (the collected-node budget) — past it the walk truncates with the SAME
//!    `truncated` marker (X-3);
//! 5. **leak-free**: ONE `list_objects` post-filter over the COLLECTED node set — **not per-hop**
//!    (§4.5). A hop into an artifact the `viewer` cannot `view` **PRUNES that branch** (the node AND
//!    everything reachable ONLY through it is dropped) — the traversal is not a side-channel that
//!    reveals an edge into an unreadable artifact (drill REF-D1 traverse half: 0 leak). The filter
//!    reuses the FROZEN `SetExpr` admit decision ([`crate::backlinks::set_expr_admits`]) — the SAME
//!    algebra the backlink read lowers (one source of truth, no second filter).
//!
//! ## The post-filter is ONE pass over the collected set, NOT per-hop (§4.5 — performance + the prune)
//! The walk first collects the bounded reachable node set (the `edge` adjacency BFS), THEN applies
//! the `list_objects` filter ONCE over that set ([`apply_post_filter`]). This is both a performance
//! property (no per-hop `check` — the no-N+1 discipline the whole subsystem holds) AND the prune
//! mechanism: an unreadable node is dropped, and because the BFS records each node's PARENT, a child
//! reachable ONLY through a dropped node is dropped too (the branch is pruned at the root of the
//! unreadable subtree — the traversal cannot reveal "there is SOMETHING past this artifact you can't
//! see"). The `root` itself is assumed already permitted by the caller (the resolution chokepoint
//! checks `view(root)` before a traverse — §4.2 / REF-P10); the post-filter governs the DISCOVERED
//! nodes.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The `rel_class`/lifecycle edges this traverse walks are minted by the TE-7 mirror discipline
//!   (REF-P14 / P-163).** The traverse filters by `rel`/`rel_class` and walks WHATEVER edges the
//!   builder has projected; the lifecycle-class edges (`issue.relation.*`, `knowledge.page.*`) are
//!   only minted as a DISCIPLINED, inverse-paired vocabulary in REF-P14. Named so a `blocked_by`
//!   traverse is NOT mistaken for cross-subsystem-lifecycle-aware before the mirror discipline lands
//!   — the MECHANISM (walk + filter by `rel_class`) is real here; the disciplined lifecycle VOCABULARY
//!   it filters on is REF-P14.
//! - **The row source is the in-memory [`crate::edge_builder::EdgeProjection`] now; the real
//!   per-tenant-DEK-encrypted Postgres `edge` table (REF-P5 schema) + the real `WITH RECURSIVE` CTE
//!   replace the in-process BFS when the OLTP store is wired into `serve`.** The BOUND DISCIPLINE
//!   (depth ceiling, visited-set cycle guard, node budget, partial+truncated, ONE collected-node
//!   post-filter, branch-prune) is real and proven here; the in-process BFS stands in for the SQL
//!   `WITH RECURSIVE … CYCLE … LIMIT` + the statement timeout. The statement-timeout is the SQL-side
//!   belt the depth+node budget is the braces for; it is named here and lands with the live OLTP store
//!   in `serve` (REF-P5+). The world-scale traverse-at-scale drill (REF-D3 reach / REF-P22 surge) is a
//!   later band.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The traverse is a **bound-critical + leak-critical** path: a mutant that wraps the depth (one hop
//! too deep), skips the visited-set (a cycle hangs / a node is double-counted), drops the node budget
//! (an unbounded wide scan), or flips the post-filter prune (a confidential artifact leaks through the
//! traverse) is a cardinal sin. Floor: **≥ 80% of viable mutants caught**
//! (`cargo mutants -p myelin-refs-service -f crates/myelin-refs-service/src/traverse.rs`). Every
//! boundary (the `>= ceiling` cutoff, the `> max_nodes` cutoff, the visited-set membership, the
//! prune-the-branch admit, the `rel`/`rel_class` filter, the cycle diagnostic) has a test a mutation
//! flips. **Measured 2026-06-20: 35 mutants → 6 unviable, 29 viable, 26 caught, 3 missed = 89.7% of
//! viable** — floor met. The 3 missed are documented NON-CORE / EQUIVALENT: (1)
//! `TraverseFilter::any -> Default::default()` is an EQUIVALENT mutant (`any()` IS
//! `TraverseFilter::default()` — no observable difference); (2)/(3) the `edge_projection` /
//! `authz_index` accessors replaced by a fresh empty value are trivial test/wiring accessors, NOT the
//! bound/leak decision logic the floor governs (every depth/cycle/budget/prune/filter boundary is
//! caught).

use std::collections::{HashSet, VecDeque};

use myelin_identity::{ListObjectsResult, Principal, SetExpr};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::backlinks::{set_expr_admits, AuthzVisibleIndex};
use crate::edge_builder::{EdgeProjection, RelClass};

/// **The traverse DEPTH CEILING seed (§4.5 default 16).** The walk descends at most this many hops
/// from the `root` before it returns a PARTIAL result + a `truncated` marker — never an unbounded
/// scan (X-3). The CANONICAL value is read from the thresholds file
/// ([`depth_ceiling_from_thresholds`]); this constant is the SEED the file mirrors (the thresholds
/// file is the single source of truth — a drill reads the file, not this literal). DISTINCT from the
/// agent CAUSAL depth ceiling ([`crate::loop_guard::CAUSAL_DEPTH_CEILING`] = 12): a deep dependency
/// tree is a legitimate 16-hop graph, not a runaway agent loop. Documented in [`crate::loop_guard`].
pub const TRAVERSE_DEPTH_CEILING: u32 = 16;

/// **The traverse COLLECTED-NODE budget seed (X-3 default 10 000).** Even a SHALLOW but WIDE
/// (high-fan-out) graph the depth ceiling alone would not bound is capped here: once the walk has
/// collected this many distinct nodes it stops and marks the result `truncated`. The CANONICAL value
/// is read from the thresholds file ([`max_nodes_from_thresholds`]); this constant is the SEED the
/// file mirrors. Re-measured at world-scale (REF-P22).
pub const TRAVERSE_MAX_NODES: u32 = 10_000;

/// Read the canonical traverse depth ceiling from the loaded thresholds file (the single source of
/// truth — no hardcoded magic number in the traverse). The caller loads
/// [`myelin_substrate::Thresholds`] (`load_canonical`) once and passes the ceiling in; this helper
/// names the field so a drill reads `t.refs_traverse.depth_ceiling` from the FILE, never a literal.
pub fn depth_ceiling_from_thresholds(t: &myelin_substrate::Thresholds) -> u32 {
    t.refs_traverse.depth_ceiling
}

/// Read the canonical traverse collected-node budget from the loaded thresholds file (the single
/// source of truth). See [`depth_ceiling_from_thresholds`].
pub fn max_nodes_from_thresholds(t: &myelin_substrate::Thresholds) -> u32 {
    t.refs_traverse.max_nodes
}

/// **The `rel`/`rel_class` filter a traverse walks under (§4.5 — "filtered by `rel`/`rel_class`").**
/// A traverse follows ONLY the edges that match — so a `blocked_by` impact walk never wanders into
/// `mentions` edges, and a `reference`-class walk never follows a `lifecycle` mirror (and vice
/// versa). An empty `rels` + `None` class is the "all relations" walk (filtered only by the bounds).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraverseFilter {
    /// The relation tokens to follow (`blocks`/`embeds`/…); EMPTY means "any relation" (no `rel`
    /// restriction — the walk follows every relation, still bounded by depth/nodes + `rel_class`).
    pub rels: Vec<String>,
    /// The `rel_class` to restrict to (`reference` | `lifecycle`); `None` means "any class".
    pub rel_class: Option<RelClass>,
}

impl TraverseFilter {
    /// Follow every relation of every class (bounded only by depth/nodes) — the unrestricted walk.
    pub fn any() -> TraverseFilter {
        TraverseFilter::default()
    }

    /// Follow only these relation tokens (of any class).
    pub fn rels(rels: &[&str]) -> TraverseFilter {
        TraverseFilter {
            rels: rels.iter().map(|s| (*s).to_string()).collect(),
            rel_class: None,
        }
    }

    /// `true` iff an edge with `(rel, rel_class)` is one this filter follows. An empty `rels` matches
    /// any relation; a `None` `rel_class` matches any class. A mutation that flips this admits the
    /// WRONG edges (a `blocked_by` walk wandering into `mentions`) — asserted.
    fn admits(&self, rel: &str, rel_class: RelClass) -> bool {
        let rel_ok = self.rels.is_empty() || self.rels.iter().any(|r| r == rel);
        let class_ok = self.rel_class.map(|c| c == rel_class).unwrap_or(true);
        rel_ok && class_ok
    }
}

/// One discovered node on a traverse path — the artifact reached + the depth at which it was first
/// reached + the parent it was reached through (so the branch-prune can drop a whole subtree). This
/// is the `[Path]` element the contract 5.3 `traverse → [Path]` returns (here a node carrying its
/// minimal-depth path back to the root via `parent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraverseNode {
    /// The discovered artifact root (a `#sub`-stripped node — the traverse walks roots, §3.2).
    pub artifact: ArtifactRef,
    /// The minimal hop-count from the `root` at which this node was first reached (1 == a direct
    /// neighbour; the `root` itself is depth 0 and is NOT in the discovered set).
    pub depth: u32,
    /// The relation the edge that first reached this node carried (`blocks`/`embeds`/…).
    pub via_rel: String,
    /// The `rel_class` of that edge (`reference` | `lifecycle`).
    pub via_rel_class: RelClass,
    /// The parent node this was first reached through (the `root` for a depth-1 neighbour). The
    /// branch-prune walks parents: a node whose parent (or any ancestor) was pruned is itself pruned.
    pub parent: ArtifactRef,
}

/// **The bounded traverse result (contract 5.3 `traverse → [Path]`).** The PERMITTED discovered
/// nodes (the post-filter has already pruned the unreadable branches) + the bound/diagnostic markers
/// a caller (and a drill) reads off the result: `truncated` (the walk hit the depth/node budget — a
/// PARTIAL result, never an unbounded scan, X-3) and `cycle_detected` (a back-edge was seen and the
/// visited-set guard stopped it — a DIAGNOSTIC, never a hang, drill REF-D8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraverseResult {
    /// The PERMITTED discovered nodes (the `viewer` may `view` each; an unreadable node and the
    /// branch reachable ONLY through it have been PRUNED). Ordered by `(depth, artifact)` for a
    /// deterministic, reproducible result.
    pub nodes: Vec<TraverseNode>,
    /// `true` iff the walk hit the depth ceiling or the node budget and returned a PARTIAL result —
    /// the `truncated` marker (X-3). A caller renders "… and more (truncated)"; it is NEVER an
    /// unbounded scan.
    pub truncated: bool,
    /// `true` iff a back-edge (a node already on the walk) was encountered — the cycle DIAGNOSTIC
    /// (drill REF-D8). The visited-set guard stopped the re-expansion; this surfaces the cycle to the
    /// caller (e.g. "this dependency graph has a cycle") rather than hanging.
    pub cycle_detected: bool,
    /// The count of distinct nodes the BFS VISITED (before the post-filter pruned) — the bound
    /// observability sample. A drill reads this to assert the walk stayed under the node budget (or
    /// truncated exactly at it).
    pub visited_count: usize,
}

/// **The bounded, cycle-safe recursive traverse (contract 5.3 OWNED — the REF-P13 crux).** Holds the
/// [`EdgeProjection`] (the §3.2/§3.4 adjacency list — the in-memory model now, the real `edge`
/// table plus `WITH RECURSIVE` later) and the [`AuthzVisibleIndex`] (the §4.4 reverse index the
/// `InRelation`/`TupleSet` post-filter forms read). The `list_objects` result is SUPPLIED by the
/// caller (Refs is the CONSUMER of Identity's contract 4.3 — it does not re-derive the ACL; it
/// post-filters the COLLECTED set with the frozen shape Identity returns).
#[derive(Clone)]
pub struct Traverse {
    edges: EdgeProjection,
    authz: AuthzVisibleIndex,
    /// The depth ceiling this traverse enforces (read from the thresholds file at construction — the
    /// single source of truth; defaults to [`TRAVERSE_DEPTH_CEILING`]).
    depth_ceiling: u32,
    /// The collected-node budget this traverse enforces (read from the thresholds file at
    /// construction; defaults to [`TRAVERSE_MAX_NODES`]).
    max_nodes: u32,
}

impl Traverse {
    /// Build a traverse over the edge adjacency list + the reverse index, with the depth ceiling +
    /// node budget read from the thresholds file (the single source of truth — no hardcoded magic).
    pub fn new(
        edges: EdgeProjection,
        authz: AuthzVisibleIndex,
        thresholds: &myelin_substrate::Thresholds,
    ) -> Traverse {
        Traverse {
            edges,
            authz,
            depth_ceiling: depth_ceiling_from_thresholds(thresholds),
            max_nodes: max_nodes_from_thresholds(thresholds),
        }
    }

    /// Build a traverse with the SEED bounds (the §4.5 defaults) — for tests/callers that have not
    /// loaded the thresholds file. The canonical path is [`Traverse::new`] (reads the file).
    pub fn with_default_bounds(edges: EdgeProjection, authz: AuthzVisibleIndex) -> Traverse {
        Traverse {
            edges,
            authz,
            depth_ceiling: TRAVERSE_DEPTH_CEILING,
            max_nodes: TRAVERSE_MAX_NODES,
        }
    }

    /// The depth ceiling this traverse enforces (the value read from the thresholds file).
    pub fn depth_ceiling(&self) -> u32 {
        self.depth_ceiling
    }

    /// The collected-node budget this traverse enforces.
    pub fn max_nodes(&self) -> u32 {
        self.max_nodes
    }

    /// The edge adjacency list this traverse walks (exposed so the caller wiring Refs into `serve` /
    /// the tests can seed it).
    pub fn edge_projection(&self) -> &EdgeProjection {
        &self.edges
    }

    /// The §4.4 reverse index the post-filter's `InRelation`/`TupleSet` forms read (exposed so the
    /// caller / tests can project grants into it).
    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    /// **`traverse(root, rels, depth, viewer)` (contract 5.3 — the crux).** Walk the `edge` adjacency
    /// list outbound from `root` following `filter`'s `rel`/`rel_class`, bounded by `min(depth,
    /// self.depth_ceiling)` hops + `self.max_nodes` nodes + the visited-set cycle guard, then apply
    /// the ONE `list_objects` post-filter over the COLLECTED node set (pruning unreadable branches).
    ///
    /// - `root` is the `#sub`-stripped node the walk departs from (the caller already permitted
    ///   `view(root)` at the resolution chokepoint — §4.2; the post-filter governs the DISCOVERED
    ///   nodes);
    /// - `filter` restricts the relations/class the walk follows (§4.5);
    /// - `depth` is the caller's requested ceiling — CLAMPED to `self.depth_ceiling` (a caller can ask
    ///   for FEWER hops, never MORE than the budget; a request past the budget truncates);
    /// - `list_objects` is the frozen 4.3 result Identity returned for `(viewer, view, type)` — the
    ///   post-filter admits a discovered node iff its artifact is in the permitted set.
    ///
    /// Returns a [`TraverseResult`]: the PERMITTED nodes + `truncated` (hit the depth/node budget — a
    /// PARTIAL result, never an unbounded scan) + `cycle_detected` (a back-edge was guarded — a
    /// DIAGNOSTIC, never a hang).
    #[allow(clippy::too_many_arguments)]
    pub fn traverse(
        &self,
        tenant: &TenantId,
        region: &Region,
        root: &ArtifactRef,
        viewer: &Principal,
        filter: &TraverseFilter,
        depth: u32,
        list_objects: &ListObjectsResult,
    ) -> TraverseResult {
        // The effective ceiling: the caller may ask for FEWER hops, never MORE than the budget. A
        // `depth` past `self.depth_ceiling` is clamped (and the truncation marker fires if the walk
        // would actually reach the budget). `min` — a mutation that drops the clamp lets a caller
        // exceed the budget (unbounded), asserted by `caller_cannot_exceed_the_ceiling`.
        let effective_ceiling = depth.min(self.depth_ceiling);

        // ── The bounded, cycle-safe BFS over the adjacency list (the `WITH RECURSIVE … CYCLE` walk). ──
        //    Visited-set guard: the `root` is seeded so a back-edge INTO the root is a cycle; a node
        //    already visited is NEVER re-expanded (terminates a self-referential graph). The node
        //    budget bounds a wide graph the depth ceiling alone would not.
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.0.clone());
        let mut discovered: Vec<TraverseNode> = Vec::new();
        // The BFS frontier: (node, depth) — we expand a node's outbound edges to find its neighbours.
        let mut frontier: VecDeque<(ArtifactRef, u32)> = VecDeque::new();
        frontier.push_back((root.clone(), 0));

        let mut truncated = false;
        let mut cycle_detected = false;

        while let Some((node, node_depth)) = frontier.pop_front() {
            // The node budget (X-3) checked at the TOP of the loop so a break truly STOPS the walk —
            // we never drain the rest of the frontier past the budget (a wide graph is bounded, never
            // an unbounded scan). `>=` — once we have collected the budget of distinct nodes we stop.
            if visited.len() as u32 >= self.max_nodes {
                truncated = true;
                break;
            }

            // At the ceiling we do NOT expand further (the partial-result boundary). `>=` — a node AT
            // the ceiling depth contributes no further hops; a mutation to `>` would descend one hop
            // too deep (unbounded by one), asserted by `depth_ceiling_truncates_with_marker`.
            if node_depth >= effective_ceiling {
                // There are (or may be) deeper neighbours we are intentionally not visiting → the
                // result is PARTIAL. Mark truncated only if this node actually HAS outbound edges the
                // filter would follow (else the walk simply ended — not a truncation).
                if self.has_followable_outbound(tenant, region, &node, filter) {
                    truncated = true;
                }
                continue;
            }

            for edge in self.edges.outbound_live(tenant, region, &node) {
                if !filter.admits(&edge.rel, edge.rel_class) {
                    continue; // not a relation/class this walk follows.
                }
                let neighbour = edge.target_root.clone();

                // The visited-set cycle guard (§4.5): a node already on the walk is a back-edge — we
                // do NOT re-expand it (terminates A→B→A) and we surface the CYCLE diagnostic. A
                // mutation that skips this membership check hangs on a cycle, asserted by
                // `self_referential_graph_terminates_and_reports_cycle`.
                if visited.contains(&neighbour.0) {
                    cycle_detected = true;
                    continue;
                }

                // The node budget (X-3): once we have collected the budget of distinct nodes we stop
                // and mark truncated — never an unbounded wide scan. `>=` against the budget AFTER
                // counting the root: visited already holds the root + every discovered node. A
                // mutation to `>` would admit one node over budget, asserted by
                // `node_budget_truncates_a_wide_graph`.
                if visited.len() as u32 >= self.max_nodes {
                    truncated = true;
                    break;
                }

                visited.insert(neighbour.0.clone());
                let child_depth = node_depth + 1;
                discovered.push(TraverseNode {
                    artifact: neighbour.clone(),
                    depth: child_depth,
                    via_rel: edge.rel.clone(),
                    via_rel_class: edge.rel_class,
                    parent: node.clone(),
                });
                frontier.push_back((neighbour, child_depth));
            }
        }

        let visited_count = visited.len();

        // ── ONE `list_objects` post-filter over the COLLECTED node set (§4.5 — NOT per-hop). A node ──
        //    the viewer cannot `view` is dropped, and any node reachable ONLY through a dropped node
        //    is pruned too (the branch-prune — the traversal is not a side-channel).
        let nodes = apply_post_filter(
            &discovered,
            list_objects,
            &self.authz,
            viewer,
            tenant,
            region,
        );

        TraverseResult {
            nodes,
            truncated,
            cycle_detected,
            visited_count,
        }
    }

    /// Does `node` have at least one outbound edge the `filter` would follow? (used at the depth
    /// ceiling to decide whether the result is genuinely PARTIAL — a leaf at the ceiling is not a
    /// truncation, a node with deeper followable edges IS).
    fn has_followable_outbound(
        &self,
        tenant: &TenantId,
        region: &Region,
        node: &ArtifactRef,
        filter: &TraverseFilter,
    ) -> bool {
        self.edges
            .outbound_live(tenant, region, node)
            .iter()
            .any(|e| filter.admits(&e.rel, e.rel_class))
    }
}

/// **Apply the ONE `list_objects` post-filter over the COLLECTED node set + prune the unreadable
/// branches (§4.5 — the leak-free, not-per-hop discipline).** A discovered node is ADMITTED iff (a)
/// the frozen `SetExpr` admits its artifact (the viewer may `view` it) AND (b) its parent was
/// admitted (the branch-prune — a node reachable ONLY through an unreadable artifact is dropped, so
/// the traverse never reveals "there is something past this artifact you can't see"). Because the BFS
/// emits nodes in increasing depth order, a single forward pass suffices: a node's parent is decided
/// before the node. The `root` is implicitly admitted (the caller permitted it).
///
/// This reuses the SAME [`set_expr_admits`] the backlink read lowers (one source of truth, no second
/// filter algebra) — `Ids`/`NotIds`/`InRelation`/`TupleSet`/`Union`/`Intersect`/`Difference`/`All`/
/// `None` all decide the SAME way they would in the lowered SQL `WHERE`.
pub fn apply_post_filter(
    discovered: &[TraverseNode],
    list_objects: &ListObjectsResult,
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    tenant: &TenantId,
    region: &Region,
) -> Vec<TraverseNode> {
    let set_expr = set_expr_of(list_objects);

    // The set of admitted artifact ids (the root is implicitly admitted — the caller permitted it; a
    // depth-1 node whose parent is the root passes the parent-check). We thread the admitted set
    // forward so the branch-prune is a single pass (nodes arrive in increasing depth order).
    let mut admitted_ids: HashSet<String> = HashSet::new();
    let mut out: Vec<TraverseNode> = Vec::new();

    for node in discovered {
        // (b) the branch-prune: a node is reachable iff its PARENT survived. A depth-1 node's parent
        // is the root (always reachable — the caller permitted it). A deeper node's parent must be in
        // `admitted_ids`. A mutation that drops this lets a child of an unreadable node leak.
        let parent_reachable = node.depth == 1 || admitted_ids.contains(&node.parent.0);
        if !parent_reachable {
            continue; // the whole branch under an unreadable ancestor is pruned.
        }
        // (a) the permission post-filter: the viewer may `view` this discovered artifact.
        let readable = set_expr_admits(&set_expr, authz, viewer, tenant, region, &node.artifact);
        if !readable {
            // The node is NOT admitted AND it is NOT recorded as reachable — so its own children
            // (deeper nodes whose parent is THIS node) are pruned (the branch-prune root).
            continue;
        }
        admitted_ids.insert(node.artifact.0.clone());
        out.push(node.clone());
    }

    // Deterministic order: by (depth, artifact) so the result is reproducible (a drill asserts the
    // exact admitted set).
    out.sort_by(|a, b| (a.depth, &a.artifact.0).cmp(&(b.depth, &b.artifact.0)));
    out
}

/// The frozen `SetExpr` the `list_objects` result carries (the SAME mapping the backlink read uses,
/// §4.4): `Ids{}` → an explicit `SetExpr::Ids`; `Filter{set_expr}` → the carried expression.
fn set_expr_of(list_objects: &ListObjectsResult) -> SetExpr {
    match list_objects {
        ListObjectsResult::Ids { ids, .. } => SetExpr::Ids(ids.clone()),
        ListObjectsResult::Filter { set_expr, .. } => set_expr.clone(),
    }
}

#[cfg(test)]
mod tests;
