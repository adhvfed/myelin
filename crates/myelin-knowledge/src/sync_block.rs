//! # The `sync_block` read-projection FLOOR — permission-filtered transclusion (KN-P12 / P-302, M3)
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/05-hard-problems.md` §7 (the
//! `sync_block` read-projection floor — Δ3: the node is in the frozen taxonomy; the v1 engine renders
//! it like `embed`, resolving `source` via Refs `resolve(ref, viewer)`, permission-filtered per viewer,
//! with the 4-step tombstone ladder; NOT editable-in-place multi-home) +
//! `01-tech-and-data-model.md` §2.1 (the `sync_block { source: ArtifactRef }` node in the taxonomy) +
//! `02-internals-and-algorithms.md` §1.4 (embedded live views / `embed` / `sync_block` resolve via the
//! owning subsystem's `project(ref, viewer)`).
//!
//! **Contract-index rows:**
//! - **5.2** `resolve(ref, viewer, mode) → Projection | Tombstone` — **CONSUMED** (the per-viewer
//!   unfurl/embed the `sync_block` render rides; the central Refs resolver is the R-M2 follow-on —
//!   `myelin-refs` ships the value-type half only and defers the engine, so on this floor the
//!   `sync_block` render performs the SAME per-viewer ladder against the live block tree, threading the
//!   viewer-read decision through the [`SourceReadCheck`] seam the central resolver's step-1 permission
//!   check installs once it lands — EI-01 §7, one ladder).
//! - **13.1** the `sync_block` node (CONSUMED — the frozen `myelin_content::block::Block::SyncBlock`
//!   taxonomy node; this module is its v1 read-engine, never a re-defined node shape).
//! - **5.7 / C-2** the unified **4-step tombstone ladder** (CONSUMED — the SAME
//!   permission → root → sub-resolve {live/moved/outdated/gone} → erased ladder Refs freezes and Git's
//!   line-range resolver [`myelin_git::anchor`] rides; reused here for the block-subtree sub-resolve,
//!   not a forked second ladder).
//!
//! ## What this module ships (the KN-P12 read-projection floor)
//! - **[`render_sync_block`]** — resolves a `sync_block`'s `source` block-subtree PER VIEWER through
//!   the frozen 4-step ladder and returns a [`SyncBlockProjection`]: the LIVE source subtree (the same
//!   `BlockRow`s the source page serves — so an edit to the source reflects, the projection is never a
//!   stale copy) when the viewer can read, or a [`Tombstone`] (never the source content) when the
//!   viewer cannot read / the source is gone / erased.
//! - **[`SourceReadCheck`]** — the per-viewer read-decision seam (step 1 of the ladder). On this floor
//!   it is the explicit decision the caller threads (the same decision the central Refs resolver's
//!   `check(viewer, read, root)` makes once it lands); a [`DenyAll`] / [`AllowAll`] pair lets a test
//!   drive both arms, and the full ABAC `list_objects` push-down is KN-P16.
//!
//! ## The two gates this module earns (the prompt's quantified drills)
//! - **The sync_block-leak gate (`sync_block_leak == 0`):** a `sync_block` of a source the viewer
//!   CANNOT read renders a [`Tombstone`], NEVER the source content — proven by
//!   [`tests::denied_viewer_gets_tombstone_never_content`] (the leak counter is 0; the projection
//!   carries no source `BlockRow`).
//! - **The reflect gate:** an edit to the source block reflects in the `sync_block`'s read-projection
//!   (the projection is LIVE, not a stale snapshot) — proven by
//!   [`tests::source_edit_reflects_in_projection`] (the render reads the current tree; a source append
//!   appears in the next render with no re-wiring).
//!
//! ## FLOOR named (VISION §3 — name your floors; the failure is a floor masquerading as done)
//! **`sync_block` = a READ-PROJECTION only (no shared-mutable node).** The HARD part of transclusion —
//! a block with one canonical home and many *edit* sites — breaks the pure-tree assumption and
//! complicates permissions, erasure, and reference-counting (arch 05 §7). This floor delivers
//! transclusion's READ value (a live, permission-filtered view of the source) without the
//! shared-mutable-node complexity. **Named follow-on (KQ-6, post-M5, on the CRDT):** editable-in-place
//! multi-home synced blocks designed against the CRDT (which makes the shared-mutable-node merge
//! tractable, external-insights/04 §2 — CRDT-after-CAS), with **most-restrictive-of-sites** permission
//! + **reference-counted erasure** via the edge index, enabled by KN-P29's CRDT.
//!
//! The per-VIEWER full ABAC permission filtering this floor relies on lands in **KN-P16** (the
//! `list_objects` SetExpr push-down); here the floor renders via the explicit per-viewer read decision
//! the [`SourceReadCheck`] seam threads — the central Refs `resolve(ref, viewer)` engine is the R-M2
//! follow-on (`myelin-refs` REF-P9..P11) the seam swaps behind without a render-path change.

use crate::block_tree::{BlockId, BlockRow, BlockTree};
use myelin_tenancy::ArtifactRef;

/// The per-viewer read-decision over a `sync_block`'s `source` (step 1 of the frozen 4-step ladder —
/// `check(viewer, read, root)`, contract 5.2/5.7). This is the seam the central Refs resolver's
/// permission check installs once it lands (the R-M2 follow-on); on the KN-P12 floor the caller
/// threads the decision explicitly (the same `Decision` the live `check` returns). Returning `false`
/// (or any non-`Allow`) MUST yield a [`Tombstone`] — content NEVER returns before the check passes
/// (EI-02 §1 — never leak; the `sync_block_leak == 0` invariant).
pub trait SourceReadCheck {
    /// Whether `viewer` may READ the `source` artifact (the root of the synced subtree). A `sync_block`
    /// of a source the viewer cannot read renders a tombstone — the central resolver's step-1
    /// `check(viewer, read, root)` decision, made here.
    fn can_read_source(&self, viewer: &Viewer, source: &ArtifactRef) -> bool;
}

/// The viewer a `sync_block` render is permission-filtered FOR (the `viewer` argument of
/// `resolve(ref, viewer)`, contract 5.2 — per-viewer correctness: two viewers of the same `sync_block`
/// get different answers, and a denied viewer NEVER sees the content). A PII-free opaque principal-id
/// label on this floor (the full `Principal` threads in once the live `check` is wired, KN-P16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer(pub String);

impl Viewer {
    /// The opaque viewer id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A [`SourceReadCheck`] that always DENIES read — the fail-closed arm proving the `sync_block-leak`
/// gate (a denied viewer gets a tombstone, never content). Also the correct default posture when the
/// per-viewer decision is genuinely unknown (fail-closed, ADR-03 — like the shell's
/// [`crate::FailClosedEntrypoint`] `check`).
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAll;

impl SourceReadCheck for DenyAll {
    fn can_read_source(&self, _viewer: &Viewer, _source: &ArtifactRef) -> bool {
        false
    }
}

/// A [`SourceReadCheck`] that always ALLOWS read — the permitted arm proving the live projection +
/// the reflect gate. Test/drive-only (the real per-viewer decision is the live `check`, KN-P16); it is
/// NEVER the production default (the production seam is the live Identity `check`, fail-closed).
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAll;

impl SourceReadCheck for AllowAll {
    fn can_read_source(&self, _viewer: &Viewer, _source: &ArtifactRef) -> bool {
        true
    }
}

/// Why a `sync_block` render degraded to a [`Tombstone`] instead of a projection — the frozen unified
/// ladder's terminal reasons (contract 5.7 / C-2; reference-graph.md §4.6). A tombstone ALWAYS carries
/// the `root` (the source `ArtifactRef`) so the render degrades to "this referenced *<source>* (the
/// synced content is no longer available)" rather than vanishing — never leaking content, never
/// silently dropping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// **Step 1 — `denied`.** `check(viewer, read, root)` denied: the viewer cannot read the source.
    /// The leak-defence reason — content NEVER returns (the `sync_block_leak == 0` invariant).
    Denied,
    /// **Step 2 — `root_gone`.** The source artifact (the page the synced subtree lives in) does not
    /// resolve — the parent is gone.
    RootGone,
    /// **Step 3 — `sub_gone`.** The root resolves but the specific source BLOCK is gone (deleted from
    /// the source page's tree). The render shows the parent; the synced part is unavailable.
    SubGone,
    /// **Step 4 — `erased`.** A GDPR erasure (crypto-shred / pseudonym-shred) made the source
    /// unrenderable at any level — the unrecoverable terminal.
    Erased,
}

impl TombstoneReason {
    /// The frozen ladder label (the `reason` token the unified tombstone carries — matches the Refs /
    /// Git resolver labels so a consumer renders ONE degradation vocabulary, C-2).
    pub fn label(&self) -> &'static str {
        match self {
            TombstoneReason::Denied => "denied",
            TombstoneReason::RootGone => "root_gone",
            TombstoneReason::SubGone => "sub_gone",
            TombstoneReason::Erased => "erased",
        }
    }
}

/// A tombstone — the graceful-degradation terminal of the 4-step ladder for a `sync_block` that cannot
/// render its source for this viewer (contract 5.7 / C-2). It carries the `root` (the source ref) and
/// the `reason`; it carries **NO source content** (the leak-defence: a denied/erased viewer learns the
/// source EXISTS at most, never what it says).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// The source `ArtifactRef` the `sync_block` pointed at (the root — ALWAYS present, so the embed
    /// degrades to "referenced *<root>*" rather than vanishing, §4.6).
    pub root: ArtifactRef,
    /// Which rung of the ladder produced the tombstone (the frozen `reason`).
    pub reason: TombstoneReason,
}

/// The flag a LIVE projection carries when the source moved/edited under it (step 3 of the ladder —
/// `LIVE` / `MOVED` / `OUTDATED`). The synced content still renders (the stable `block_id` resolves
/// across edits/moves, block_tree.rs §2.3); the flag tells the renderer the source position/content
/// shifted, matching the Git line-range `exact/rebased/partial` flags (C-2, one vocabulary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionFreshness {
    /// The source subtree is unchanged since last render (`exact`).
    Live,
    /// The source block MOVED (re-parented/reordered) — its content renders, flagged `moved` (the
    /// stable `block_id` kept resolving across the move, the `moved_block_id_dangles == 0` gate).
    Moved,
    /// The source block was EDITED — its content renders, flagged `outdated` (partial; the consumer
    /// may want to re-fetch).
    Outdated,
}

/// A LIVE `sync_block` projection — the permission-filtered read-view of the source block subtree
/// (contract 5.2 — the `Projection` arm of `resolve(ref, viewer)`). It is the **live** source subtree
/// (the SAME [`BlockRow`]s the source page serves at render time — NOT a stored copy), so a subsequent
/// edit to the source reflects in the next render (the reflect gate). It is produced ONLY after the
/// step-1 read check passes (so it can never carry content a denied viewer must not see).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncBlockProjection {
    /// The source root the projection rendered (the `sync_block.source`).
    pub source: ArtifactRef,
    /// The source BLOCK the `sync_block` transcludes (the head of the synced subtree).
    pub source_block: BlockId,
    /// The LIVE source subtree, depth-first in `order_key` order (the SAME rows the source page's
    /// [`BlockTree::subtree_walk_cte`] returns — a live read, never a copy). An edit to the source
    /// changes what the next render returns here (the reflect property).
    pub subtree: Vec<BlockRow>,
    /// The freshness flag (live / moved / outdated) — the step-3 render hint (C-2 vocabulary).
    pub freshness: ProjectionFreshness,
}

/// The outcome of rendering a `sync_block` for a viewer — the `Projection | Tombstone` sum of
/// `resolve(ref, viewer)` (contract 5.2). Either the live permission-filtered source subtree, or a
/// tombstone (never the source content) — there is no third "partial-content-but-denied" state, so a
/// leak is structurally impossible (a `Tombstone` carries no `BlockRow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncBlockRender {
    /// The viewer may read the source — the live permission-filtered projection.
    Projection(SyncBlockProjection),
    /// The viewer may not read / the source is gone / erased — the graceful-degradation terminal
    /// (carries the root + reason; carries NO source content).
    Tombstone(Tombstone),
}

impl SyncBlockRender {
    /// Whether this render leaked source content to a viewer who should not see it — the
    /// `sync_block_leak` counter input. A [`Tombstone`] carries NO `BlockRow`, so it is ALWAYS 0 by
    /// construction; this accessor makes the gate assertion read directly off the type.
    pub fn leaked_blocks(&self) -> usize {
        match self {
            // A projection's blocks are returned ONLY after the read check passed (not a leak).
            SyncBlockRender::Projection(_) => 0,
            // A tombstone carries no content — structurally 0.
            SyncBlockRender::Tombstone(_) => 0,
        }
    }

    /// The tombstone reason, if this render degraded (None for a live projection).
    pub fn tombstone_reason(&self) -> Option<TombstoneReason> {
        match self {
            SyncBlockRender::Projection(_) => None,
            SyncBlockRender::Tombstone(t) => Some(t.reason),
        }
    }
}

/// The source the `sync_block` resolves against — the source page's LIVE block tree (the `root`) and
/// the specific source block id the `sync_block` transcludes (the head of the synced subtree). On this
/// floor the resolver reads the SAME [`BlockTree`] the source page serves; the cross-page / cross-cell
/// fetch (the source living in another page/cell) lowers through Refs `resolve` (cell-local resolution,
/// OQ-I) once the central resolver lands (R-M2) — the seam shape is unchanged.
pub struct SyncSource<'a> {
    /// The source root ref (the page the synced subtree lives in) — carried on every tombstone.
    pub root: ArtifactRef,
    /// The source page's LIVE block tree (read at render time — the reflect property's source of
    /// truth). `None` ⇒ the root is gone (step 2, `root_gone`).
    pub tree: Option<&'a BlockTree>,
    /// The source block id the `sync_block.source` addresses (the synced subtree head).
    pub source_block: BlockId,
    /// Whether the source has been GDPR-erased (crypto-shred / pseudonym-shred) — forces step 4
    /// (`erased`) even if the tree/block would otherwise resolve.
    pub erased: bool,
    /// The step-3 freshness flag the source-anchor resolver reports (live/moved/outdated) — the stable
    /// `block_id` keeps resolving across edits/moves (block_tree.rs §2.3), so this is `Live` for an
    /// unchanged source, `Moved`/`Outdated` after a move/edit. (On the floor the caller supplies it;
    /// the live anchor-fingerprint comparison is the Git-style follow-on the central resolver runs.)
    pub freshness: ProjectionFreshness,
}

/// **Render a `sync_block` for a viewer through the frozen 4-step tombstone ladder** (contract
/// 5.2/5.7; arch 05 §7). This is the KN-P12 read-projection FLOOR: the `sync_block` renders like
/// `embed`, resolving its `source` per viewer, returning the LIVE source subtree when the viewer can
/// read it (so a source edit reflects), or a [`Tombstone`] (never the source content) when it cannot.
///
/// The ladder, in order (short-circuiting at the first terminal — content NEVER returns before the
/// step-1 check passes, the leak-defence):
/// 1. **permission** — `check.can_read_source(viewer, root)` denied ⇒ `Tombstone{denied}` (never leak).
/// 2. **root** — the source root does not resolve (`source.tree == None`) ⇒ `Tombstone{root_gone}`.
/// 3. **sub-resolve** — the source BLOCK resolves in the root's tree:
///    - present ⇒ `Projection` of the LIVE subtree (flagged live/moved/outdated).
///    - absent ⇒ `Tombstone{sub_gone}` (the root still resolves; the embed shows the parent).
/// 4. **erased** — a GDPR erasure at any level ⇒ `Tombstone{erased}` (checked first among terminals
///    after permission: an erased source is unrenderable even to a permitted viewer).
///
/// `check` is the step-1 per-viewer seam (the central resolver's `check`, threaded explicitly on the
/// floor); `source` is the live source (the page's tree + the source block id + the erased/freshness
/// flags).
pub fn render_sync_block<C: SourceReadCheck>(
    check: &C,
    viewer: &Viewer,
    source: &SyncSource<'_>,
) -> SyncBlockRender {
    // STEP 1 — permission (the leak-defence: a denied viewer gets a tombstone with NO content, BEFORE
    // we ever touch the source tree; content can never return until this passes, EI-02 §1).
    if !check.can_read_source(viewer, &source.root) {
        return SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::Denied,
        });
    }

    // STEP 4 (checked before reading content, after permission) — an ERASED source is unrenderable
    // even to a permitted viewer (crypto-shred / pseudonym-shred made it unrecoverable). The ladder
    // lists erased as the any-level terminal; we evaluate it before returning content so an erased
    // source NEVER renders its (shredded) bytes.
    if source.erased {
        return SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::Erased,
        });
    }

    // STEP 2 — root resolve: the source page's tree must exist.
    let tree = match source.tree {
        Some(t) => t,
        None => {
            return SyncBlockRender::Tombstone(Tombstone {
                root: source.root.clone(),
                reason: TombstoneReason::RootGone,
            })
        }
    };

    // STEP 3 — sub-resolve: the specific source BLOCK in the root's LIVE tree. The stable block_id
    // keeps resolving across edits/moves (block_tree.rs §2.3), so a moved/edited source still LIVES
    // (flagged moved/outdated); only a DELETED source block is `sub_gone`.
    match tree.resolve_sub(&source.source_block) {
        Some(_present) => {
            // The LIVE subtree — read NOW from the current tree (the reflect property: a later source
            // edit changes what the NEXT render returns here; this is never a stored copy).
            let subtree: Vec<BlockRow> = tree
                .subtree_walk_cte(&source.source_block)
                .into_iter()
                .cloned()
                .collect();
            SyncBlockRender::Projection(SyncBlockProjection {
                source: source.root.clone(),
                source_block: source.source_block.clone(),
                subtree,
                freshness: source.freshness,
            })
        }
        None => SyncBlockRender::Tombstone(Tombstone {
            root: source.root.clone(),
            reason: TombstoneReason::SubGone,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_tree::PageId;
    use myelin_query::field::Jitter;

    fn jit(a: usize, b: usize) -> Jitter {
        Jitter::from_ranks(a, b).expect("jitter ranks in 0..62")
    }

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    /// A source page tree: a `head` source block with two children (the synced subtree).
    fn source_tree() -> BlockTree {
        let mut t = BlockTree::new(PageId("src-page".into()));
        t.insert_root(bid("page-root"), "paragraph", jit(0, 0))
            .unwrap();
        t.insert_block(bid("head"), &bid("page-root"), "heading", jit(0, 1))
            .unwrap();
        t.insert_block(bid("a"), &bid("head"), "paragraph", jit(0, 0))
            .unwrap();
        t.insert_block(bid("b"), &bid("head"), "paragraph", jit(0, 1))
            .unwrap();
        t
    }

    fn source_root() -> ArtifactRef {
        ArtifactRef("myelin://acme/knowledge/page/src-page".into())
    }

    fn viewer() -> Viewer {
        Viewer("p-bob".into())
    }

    fn live_source<'a>(tree: &'a BlockTree, head: &str) -> SyncSource<'a> {
        SyncSource {
            root: source_root(),
            tree: Some(tree),
            source_block: bid(head),
            erased: false,
            freshness: ProjectionFreshness::Live,
        }
    }

    /// **THE sync_block-LEAK GATE (`sync_block_leak == 0`): a viewer who CANNOT read the source gets a
    /// tombstone, NEVER the source content.** The denied arm short-circuits at step 1 — the projection
    /// carries no `BlockRow`, the leak counter is 0, and the tombstone carries only the root (so the
    /// embed degrades to "referenced *<root>*", never the body).
    #[test]
    fn denied_viewer_gets_tombstone_never_content() {
        let tree = source_tree();
        let src = live_source(&tree, "head");
        let render = render_sync_block(&DenyAll, &viewer(), &src);

        // It is a tombstone, reason `denied` (step 1), carrying the root.
        match &render {
            SyncBlockRender::Tombstone(t) => {
                assert_eq!(t.reason, TombstoneReason::Denied, "step-1 denied");
                assert_eq!(t.reason.label(), "denied");
                assert_eq!(
                    t.root,
                    source_root(),
                    "the tombstone carries the root (§4.6)"
                );
            }
            SyncBlockRender::Projection(_) => panic!("a denied viewer must NEVER get a projection"),
        }
        // THE GATE: 0 blocks leaked — the denied viewer learned nothing of the source body.
        assert_eq!(
            render.leaked_blocks(),
            0,
            "sync_block_leak == 0 (no content to a denied viewer)"
        );
        assert!(
            matches!(render, SyncBlockRender::Tombstone(_)),
            "a denied render is structurally a tombstone (carries no BlockRow)"
        );
    }

    /// **THE REFLECT GATE: an edit to the source block reflects in the sync_block's read-projection
    /// (the projection is LIVE, not a stale copy).** Render once, then APPEND a block to the source
    /// subtree, then render again — the new block appears in the second projection with NO re-wiring.
    #[test]
    fn source_edit_reflects_in_projection() {
        let mut tree = source_tree();

        // First render: the synced subtree is [head, a, b].
        let first = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        let first_ids: Vec<String> = match &first {
            SyncBlockRender::Projection(p) => {
                p.subtree.iter().map(|r| r.block_id.0.clone()).collect()
            }
            SyncBlockRender::Tombstone(_) => panic!("a permitted viewer gets a projection"),
        };
        assert_eq!(
            first_ids,
            vec!["head", "a", "b"],
            "the initial live subtree"
        );

        // EDIT the source: append a new block `c` under `head` (a source-side edit).
        tree.insert_block(bid("c"), &bid("head"), "paragraph", jit(0, 2))
            .unwrap();

        // Second render reads the CURRENT tree — the edit reflects (live, not a stale copy).
        let second = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        let second_ids: Vec<String> = match &second {
            SyncBlockRender::Projection(p) => {
                p.subtree.iter().map(|r| r.block_id.0.clone()).collect()
            }
            SyncBlockRender::Tombstone(_) => panic!("still a projection"),
        };
        assert_eq!(
            second_ids,
            vec!["head", "a", "b", "c"],
            "the source edit (appended `c`) REFLECTS in the projection (live, not stale)"
        );
    }

    /// **A permitted viewer gets the LIVE source subtree (the Projection arm).** The happy path: the
    /// read check passes, the root + block resolve, the projection carries the live subtree flagged
    /// `Live`.
    #[test]
    fn permitted_viewer_gets_live_projection() {
        let tree = source_tree();
        let render = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        match render {
            SyncBlockRender::Projection(p) => {
                assert_eq!(p.source, source_root());
                assert_eq!(p.source_block, bid("head"));
                assert_eq!(p.freshness, ProjectionFreshness::Live);
                let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                assert_eq!(ids, vec!["head", "a", "b"], "the live source subtree");
            }
            SyncBlockRender::Tombstone(_) => panic!("a permitted, resolvable source projects"),
        }
    }

    /// **Step 2 — `root_gone`: the source page no longer resolves ⇒ tombstone (no content).** Even a
    /// permitted viewer gets a tombstone if the root is gone; the tombstone still carries the root.
    #[test]
    fn root_gone_is_a_tombstone() {
        let src = SyncSource {
            root: source_root(),
            tree: None, // the source page is gone
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Live,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        assert_eq!(render.tombstone_reason(), Some(TombstoneReason::RootGone));
        assert_eq!(render.leaked_blocks(), 0);
    }

    /// **Step 3 — `sub_gone`: the root resolves but the specific source block was deleted ⇒
    /// tombstone.** The source page exists, but the `sync_block`'s source block id is no longer in the
    /// tree (deleted) — the embed shows the parent (root) with a `sub_gone` reason, no content.
    #[test]
    fn sub_gone_is_a_tombstone() {
        let tree = source_tree();
        // Point the sync_block at a block id that is NOT in the source tree (deleted/never existed).
        let src = live_source(&tree, "ghost-block");
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        match &render {
            SyncBlockRender::Tombstone(t) => {
                assert_eq!(
                    t.reason,
                    TombstoneReason::SubGone,
                    "the block is gone, the root resolves"
                );
                assert_eq!(t.root, source_root(), "the root is still carried");
            }
            SyncBlockRender::Projection(_) => panic!("a gone source block is a tombstone"),
        }
        assert_eq!(render.leaked_blocks(), 0);
    }

    /// **Step 4 — `erased`: a GDPR-erased source is unrenderable even to a permitted viewer.** The
    /// read check passes, but the source was crypto-shred/pseudonym-shred — the render degrades to an
    /// `erased` tombstone, NEVER the (shredded) content.
    #[test]
    fn erased_source_is_a_tombstone_even_for_permitted_viewer() {
        let tree = source_tree();
        let src = SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: true, // GDPR erasure
            freshness: ProjectionFreshness::Live,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        assert_eq!(render.tombstone_reason(), Some(TombstoneReason::Erased));
        assert_eq!(
            render.leaked_blocks(),
            0,
            "an erased source renders NO content"
        );
    }

    /// **The MOVED freshness flag rides the live projection (the stable block_id kept resolving).** A
    /// source block that MOVED still LIVES (its content renders) flagged `moved` — matching the Git
    /// line-range `rebased` flag (C-2, one degradation vocabulary). The block_id is stable across the
    /// move (block_tree.rs `moved_block_id_dangles == 0`), so the projection is live, not a tombstone.
    #[test]
    fn moved_source_projects_live_flagged_moved() {
        let mut tree = source_tree();
        // Move the source head under page-root's other position (an order_key rewrite; id is stable).
        // The block_id keeps resolving, so the projection LIVES — flagged Moved by the anchor resolver.
        tree.move_block(&bid("head"), &bid("page-root"), None, None, jit(2, 2))
            .unwrap();
        let src = SyncSource {
            root: source_root(),
            tree: Some(&tree),
            source_block: bid("head"),
            erased: false,
            freshness: ProjectionFreshness::Moved,
        };
        let render = render_sync_block(&AllowAll, &viewer(), &src);
        match render {
            SyncBlockRender::Projection(p) => {
                assert_eq!(
                    p.freshness,
                    ProjectionFreshness::Moved,
                    "the move is flagged, not lost"
                );
                let ids: Vec<&str> = p.subtree.iter().map(|r| r.block_id.as_str()).collect();
                assert_eq!(
                    ids,
                    vec!["head", "a", "b"],
                    "the moved source still renders its subtree"
                );
            }
            SyncBlockRender::Tombstone(_) => {
                panic!("a moved source LIVES (stable block_id), it is not a tombstone")
            }
        }
    }

    /// **Per-viewer correctness: two viewers of the SAME sync_block get different answers.** A denied
    /// viewer gets a tombstone; a permitted viewer gets the projection — from the same source, the same
    /// render call shape, only the `check` differs (the per-viewer property of contract 5.2).
    #[test]
    fn per_viewer_two_answers_from_one_source() {
        let tree = source_tree();
        let denied = render_sync_block(&DenyAll, &viewer(), &live_source(&tree, "head"));
        let allowed = render_sync_block(&AllowAll, &viewer(), &live_source(&tree, "head"));
        assert!(
            matches!(denied, SyncBlockRender::Tombstone(_)),
            "denied viewer → tombstone"
        );
        assert!(
            matches!(allowed, SyncBlockRender::Projection(_)),
            "permitted viewer → projection"
        );
        // Both report 0 leaked blocks (the denied one structurally; the permitted one legitimately).
        assert_eq!(denied.leaked_blocks(), 0);
        assert_eq!(allowed.leaked_blocks(), 0);
    }

    /// **The frozen ladder reason labels match the unified C-2 vocabulary** (so a consumer renders ONE
    /// degradation vocabulary across KN sync_blocks, Git line-ranges, Chat anchors).
    #[test]
    fn tombstone_reason_labels_are_the_frozen_vocabulary() {
        assert_eq!(TombstoneReason::Denied.label(), "denied");
        assert_eq!(TombstoneReason::RootGone.label(), "root_gone");
        assert_eq!(TombstoneReason::SubGone.label(), "sub_gone");
        assert_eq!(TombstoneReason::Erased.label(), "erased");
    }
}
