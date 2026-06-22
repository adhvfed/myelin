//! # The CDC pair for contract 5.2 — Knowledge's `sync_block` read-projection (KN-P12 / P-302, M3)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.2
//! (`resolve(ref, viewer, mode) → Projection | Tombstone` — live per-viewer unfurl/embed; denied →
//! tombstone) + row 5.7 / C-2 (the unified 4-step tombstone ladder). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-4 (the frozen `#sub` grammar + the one resolution ladder).
//! Owning architecture: Knowledge
//! `04-subsystem-architectures/knowledge-platform/architecture/05-hard-problems.md` §7 (the
//! `sync_block` read-projection FLOOR — Δ3: the node is in the frozen taxonomy; v1 renders it like
//! `embed`, resolving `source` via `resolve(ref, viewer)`, permission-filtered per viewer, with the
//! 4-step tombstone ladder; NOT editable-in-place multi-home).
//!
//! ## The seam this pair pins (Knowledge CONSUMES 5.2 to render `sync_block`)
//! Row 5.2 is the seam between the PROVIDER that resolves an `ArtifactRef` per viewer to a
//! `Projection | Tombstone`, and the CONSUMER that renders the outcome — degrading to a tombstone
//! notice (never leaking content) when the viewer cannot read / the source is gone / erased. Here
//! Knowledge is a CONSUMER of 5.2 for the `sync_block` node: it renders `sync_block` by resolving its
//! `source` through the SAME 4-step ladder Refs/Git freeze (permission → root → sub-resolve → erased),
//! returning the LIVE source subtree IFF the viewer can read it, else a content-free tombstone. The
//! central Refs `resolve` engine is the R-M2 follow-on (`myelin-refs` ships the value-type half only),
//! so on the floor the per-viewer read decision threads through the
//! [`myelin_knowledge::SourceReadCheck`] seam the central resolver's step-1 check installs once it
//! lands — one ladder, one degradation vocabulary (C-2).
//!
//! The frozen behaviour both sides agree on:
//! - the PROVIDER (the KN `sync_block` render) is PERMISSION-FIRST: a viewer who cannot read the
//!   source gets a `Tombstone{denied}` carrying ONLY the root — content NEVER returns before the
//!   step-1 check passes (the `sync_block_leak == 0` invariant, the M3 leak gate);
//! - the PROVIDER's projection is LIVE: it reads the current source block tree, so a source edit
//!   reflects in the next render (the reflect gate — never a stale copy);
//! - the CONSUMER (a downstream renderer) degrades a tombstone to "referenced *<root>* (the synced
//!   content is no longer available)" using the FROZEN reason vocabulary (`denied`/`root_gone`/
//!   `sub_gone`/`erased`), and renders the live subtree on a projection — it NEVER sees source content
//!   for a denied viewer (0 leak), and it surfaces the SAME degradation vocabulary as Git line-ranges.
//!
//! (No cargo-mutants floor on this consumer glue: the per-viewer ABAC permission BODY is KN-P16 and
//! the central Refs resolver engine is the R-M2 follow-on — the load-bearing leak-defence proven here
//! is the ladder ORDER (permission-before-content) + the structural "a tombstone carries no BlockRow",
//! both exhaustively asserted. The editable-multi-home engine is the named KQ-6 follow-on, post-M5 on
//! the CRDT, KN-P29.)

use myelin_knowledge::block_tree::{BlockId, BlockTree, PageId};
use myelin_knowledge::{
    render_sync_block, AllowAll, DenyAll, ProjectionFreshness, SyncBlockRender, SyncSource,
    TombstoneReason, Viewer,
};
use myelin_query::field::Jitter;
use myelin_tenancy::ArtifactRef;

fn jit(a: usize, b: usize) -> Jitter {
    Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
}

fn bid(s: &str) -> BlockId {
    BlockId(s.to_string())
}

fn source_root() -> ArtifactRef {
    ArtifactRef("myelin://acme/knowledge/page/handbook".into())
}

/// A source page tree with a `head` block whose subtree is the synced content.
fn source_tree() -> BlockTree {
    let mut t = BlockTree::new(PageId("handbook".into()));
    t.insert_root(bid("page-root"), "paragraph", jit(0, 0)).unwrap();
    t.insert_block(bid("head"), &bid("page-root"), "heading", jit(0, 1)).unwrap();
    t.insert_block(bid("policy"), &bid("head"), "paragraph", jit(0, 0)).unwrap();
    t
}

fn live_source<'a>(tree: &'a BlockTree) -> SyncSource<'a> {
    SyncSource {
        root: source_root(),
        tree: Some(tree),
        source_block: bid("head"),
        erased: false,
        freshness: ProjectionFreshness::Live,
    }
}

/// **PROVIDER side of 5.2** — the KN `sync_block` render is permission-first: a denied viewer gets a
/// content-free `Tombstone{denied}` (the leak gate), a permitted viewer gets the LIVE source subtree.
/// This is the `resolve(ref, viewer) → Projection | Tombstone` contract, rendered for the `sync_block`
/// node: the provider NEVER returns content before the step-1 read check passes.
#[test]
fn provider_sync_block_render_is_permission_first() {
    let tree = source_tree();

    // A viewer who CANNOT read the source → Tombstone{denied}, carrying only the root, NO content.
    let denied = render_sync_block(&DenyAll, &Viewer("p-anon".into()), &live_source(&tree));
    match &denied {
        SyncBlockRender::Tombstone(t) => {
            assert_eq!(t.reason, TombstoneReason::Denied, "step-1 denied (permission-first)");
            assert_eq!(t.root, source_root(), "the tombstone carries the root (§4.6)");
        }
        SyncBlockRender::Projection(_) => panic!("a denied viewer must NEVER get a projection"),
    }
    // THE LEAK GATE: 0 source blocks reach a denied viewer (structurally — a tombstone holds none).
    assert_eq!(denied.leaked_blocks(), 0, "sync_block_leak == 0 (the M3 leak gate)");

    // A viewer who CAN read → the LIVE source subtree projection.
    let allowed = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    match allowed {
        SyncBlockRender::Projection(p) => {
            assert_eq!(p.source, source_root());
            let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
            assert_eq!(ids, vec!["head", "policy"], "the permitted viewer gets the live subtree");
        }
        SyncBlockRender::Tombstone(_) => panic!("a permitted, resolvable source projects"),
    }
}

/// **PROVIDER side of 5.2 (reflect)** — the projection is LIVE: an edit to the source reflects in the
/// next render (never a stale copy). This is the "live unfurl/embed" half of `resolve` — the provider
/// reads the current source tree at render time.
#[test]
fn provider_projection_is_live_reflects_source_edit() {
    let mut tree = source_tree();
    let first = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let first_ids: Vec<String> = match &first {
        SyncBlockRender::Projection(p) => p.subtree.iter().map(|r| r.block_id.0.clone()).collect(),
        SyncBlockRender::Tombstone(_) => panic!("projection"),
    };
    assert_eq!(first_ids, vec!["head", "policy"]);

    // A SOURCE edit: append `addendum` under `head`.
    tree.insert_block(bid("addendum"), &bid("head"), "paragraph", jit(0, 1)).unwrap();

    let second = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let second_ids: Vec<String> = match &second {
        SyncBlockRender::Projection(p) => p.subtree.iter().map(|r| r.block_id.0.clone()).collect(),
        SyncBlockRender::Tombstone(_) => panic!("projection"),
    };
    assert_eq!(
        second_ids,
        vec!["head", "policy", "addendum"],
        "the source edit reflects in the projection (live, the reflect gate)"
    );
}

/// **CONSUMER side of 5.2** — a downstream renderer degrades a tombstone to a content-free notice
/// using the FROZEN reason vocabulary, and renders the live subtree on a projection. It NEVER sees
/// source content for a denied viewer (0 leak), and surfaces the SAME degradation vocabulary as Git
/// line-ranges (C-2). This models how an embed/page renderer consumes `resolve`'s outcome.
#[test]
fn consumer_renders_projection_or_degrades_tombstone() {
    // A tiny consumer: render a `SyncBlockRender` to display text the way an embed renderer would.
    fn render_for_display(outcome: &SyncBlockRender) -> String {
        match outcome {
            SyncBlockRender::Projection(p) => {
                // The consumer renders the live subtree (here: the block ids, standing for content).
                let body: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                format!("[synced from {}] {}", p.source.0, body.join(","))
            }
            SyncBlockRender::Tombstone(t) => {
                // The consumer degrades to "referenced *<root>* (unavailable)" — NO content, the
                // frozen reason label drives the UI affordance (the one C-2 vocabulary).
                format!("[referenced {} — unavailable: {}]", t.root.0, t.reason.label())
            }
        }
    }

    let tree = source_tree();

    // Denied → the consumer shows the degradation notice, carrying the root + the `denied` reason,
    // and NEVER any source body.
    let denied = render_sync_block(&DenyAll, &Viewer("p-anon".into()), &live_source(&tree));
    let denied_view = render_for_display(&denied);
    assert_eq!(
        denied_view,
        "[referenced myelin://acme/knowledge/page/handbook — unavailable: denied]",
        "the consumer degrades to a content-free notice on deny"
    );
    assert!(!denied_view.contains("policy"), "the consumer NEVER renders source content on deny");

    // Permitted → the consumer renders the live subtree.
    let allowed = render_sync_block(&AllowAll, &Viewer("p-alice".into()), &live_source(&tree));
    let allowed_view = render_for_display(&allowed);
    assert_eq!(
        allowed_view,
        "[synced from myelin://acme/knowledge/page/handbook] head,policy",
        "the consumer renders the live subtree on a projection"
    );
}

/// **CONSUMER side of 5.2 (the full ladder vocabulary)** — root_gone / sub_gone / erased each degrade
/// to a content-free tombstone with the frozen reason, so the consumer renders ONE degradation
/// vocabulary across every rung of the ladder (the unified C-2 ladder Knowledge consumes).
#[test]
fn consumer_sees_the_full_frozen_ladder_vocabulary() {
    // root_gone: the source page no longer resolves (tree == None).
    let root_gone = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: None,
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(root_gone.tombstone_reason(), Some(TombstoneReason::RootGone));
    assert_eq!(root_gone.leaked_blocks(), 0);

    // sub_gone: the root resolves but the source block was deleted.
    let tree = source_tree();
    let sub_gone = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("deleted-block"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(sub_gone.tombstone_reason(), Some(TombstoneReason::SubGone));
    assert_eq!(sub_gone.leaked_blocks(), 0);

    // erased: a GDPR erasure makes the source unrenderable even for a permitted viewer.
    let erased = render_sync_block(
        &AllowAll,
        &Viewer("p-alice".into()),
        &SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: true,
            freshness: ProjectionFreshness::Live,
        },
    );
    assert_eq!(erased.tombstone_reason(), Some(TombstoneReason::Erased));
    assert_eq!(erased.leaked_blocks(), 0, "an erased source renders NO content");
}
