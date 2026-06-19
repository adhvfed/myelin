# Sketch 05 — Synced blocks / transclusion + embed liveness

> Phase 4, Knowledge, **exploration**. Canonical: deep-dive Q11 (synced blocks break the clean
> tree — possibly defer), Q13 (how "live" are embedded views/refs), `reference-graph.md` §4.2
> (resolve + update events), ADR-13 (the projection API; live-not-snapshot default), design-language
> §5.3 (the unfurl: live, permission-aware, tombstones gracefully).

---

## 1. Synced blocks / transclusion (deep-dive Q11)

A **synced block** = the same block content appears in multiple places (one canonical home, many
render sites). It **breaks the pure-tree assumption** (a block has one parent but many render sites)
and complicates permissions, erasure, and reference counting (deep-dive §2.1).

### Options

- **A — Build full synced blocks in v1.** A block can be transcluded anywhere; edits propagate to all
  sites. Cost: every block-tree invariant (one parent, partial-load, block-level permission, erasure
  reference-counting) gets a "but synced blocks" caveat; erasure must reach every render site; the
  permission model must reconcile "the home page is private but it's transcluded into a public page"
  (a leak vector). **High complexity, high leak risk.**
- **B — Defer synced blocks; ship `embed(ArtifactRef)` instead.** The platform already has a stronger,
  cleaner primitive: **`embed(ArtifactRef)`** (a structured inline/block node, ADR-05) that renders
  *another artifact's* live, per-viewer-permission-checked projection (the reference graph + projection
  API). "Transclude page B into page A" = an `embed(myelin://…/knowledge/page/B)` node — which is
  **already permission-aware per viewer** (B's ACL is checked at render, identity §5 / `reference-
  graph.md` §4.2), **already tombstones gracefully**, and **already reference-counted by the edge
  index**. No tree-invariant breakage; no separate erasure path; no leak (B renders as a "no access"
  card to a viewer who can't see B).
- **C — Hybrid: defer *block*-level transclusion, support *page/view* embeds (B), and add a narrow
  "reference content" read-only embed for a sub-block later if measured-needed.**

### Leaning

**Option B (defer synced blocks; use `embed(ArtifactRef)` for transclusion-shaped needs), named as a
floor.** This is the deep-dive Q11 "possibly defer" answered: the platform's `embed` primitive already
gives the *valuable* part of transclusion (show content from elsewhere, live, permission-correct) via
the reference graph, **without** breaking the block tree or opening the
private-home-public-render-site leak. **Floor named, follow-on named**: true editable synced blocks
(edit-in-place-propagates-to-all-sites) are a deferred capability whose trigger is measured demand;
when built, they ride the same `block_id`-as-`ArtifactRef` addressing + the move-CRDT, with a
**most-restrictive-of-all-sites** permission rule and erasure reaching every site via the edge index.

This keeps the v1 block tree a clean tree (sketch 02), keeps erasure tractable (sketch 06), and keeps
permissions leak-free — at the cost of "synced blocks aren't editable-in-place across sites in v1,"
which is an honest, named limitation.

## 2. Embed liveness (deep-dive Q13) — how "live" are embedded views and artifact refs?

The design language (§5.3) makes **live, not snapshot** the default (for correctness + erasure-safety):
an unfurl/embed is a *current* projection, kept fresh by bus update events. The cost/consistency
trade-off (deep-dive Q13) is *how* live, at what cost.

### The mechanism (consuming Phase-3 contracts, not re-inventing)

- **Resolution**: an `embed(ArtifactRef)` / `artifact_ref` node renders via **Refs `resolve(ref,
  viewer, mode)`** (`reference-graph.md` §4.2), which calls the *owning subsystem's* `project(ref,
  viewer)` API on cache miss (through the resilient client), returns the projection or a per-viewer
  tombstone (denied → no leak). Knowledge **never reads the other subsystem's DB** — only its
  projection.
- **Liveness**: the rendering client **subscribes to `*.updated`/`*.erased` on the embedded subject**
  (the update-events hook, ADR-13.1c / `reference-graph.md` §4.2 step 4). A `ci.run.passed` /
  `issue.transitioned` / `knowledge.row.updated` event busts the cached projection and re-resolves —
  the embedded CI run goes green, the embedded issue board updates, the incident runbook's failing-run
  embed refreshes (the wedge flagship, `knowledge-platform.md` §6.1). This rides the durable bus's
  *semantic* events (not the firehose; an embed of a doc reacts to the coalesced
  `knowledge.doc.updated` pointer, never per-op).
- **The three liveness tiers** (so cost is bounded, deep-dive Q13):
  1. **Live (default)** for *on-screen* embeds/refs in an open doc — subscribed to update events,
     re-resolved on change. Bounded: only what's rendered is subscribed.
  2. **On-load** for embeds in a doc being *fetched* (resolve once at load; subscribe once rendered).
  3. **Cached projection (R2 in Refs)** absorbs read storms (a doc embedded in 500 messages resolves
     from the Refs projection cache, `reference-graph.md` §6.2) — the cache is bounded, invalidatable,
     and a `PersonalDataHolder` (never a source of truth, STOR-3).

### The hot/scale case

A foundational design doc embedded in thousands of pages, or a doc embedding a live 10,000-row issue
board: the **embedded-view content is itself an ACL-pre-filtered, paginated query** (sketch 03 / §5.6),
so an embedded issue board shows only the rows the *viewer* can see, paginated — never a 10,000-row
materialisation. The Refs projection cache + the hot-artifact handling (`reference-graph.md` §6.3)
bound the resolution fan-out. So embed liveness inherits the platform's existing permission-aware,
paginated, cached read path — Knowledge adds no new scale surface here.

## 3. What this sketch commits to the findings

- **Synced blocks/transclusion**: **deferred** in favour of `embed(ArtifactRef)` (already
  permission-aware-per-viewer, tombstoning, reference-counted via the edge index) — named floor;
  editable-in-place synced blocks are the measured-demand follow-on (most-restrictive-of-sites
  permission, erasure via the edge index, ride `block_id`-as-`ArtifactRef`).
- **Embed liveness**: **live-by-default** via Refs `resolve` → owning subsystem's `project` +
  subscription to `*.updated`/`*.erased`; three tiers (live on-screen / on-load / Refs cache) bound
  the cost; embedded-view content is an ACL-pre-filtered paginated query, so liveness adds no new
  scale surface. Knowledge consumes the projection contract, never another subsystem's DB.

## Cited prior art

- Transclusion / bidirectional links: Nelson, Project Xanadu (transclusion as a first-class concept);
  Roam/Obsidian block-reference UX — the demand source, but the *clean* implementation here is the
  reference graph (Bush 1945 associative trail, `reference-graph.md` §2).
- The projection API + live unfurl + tombstone: ADR-13.1; `reference-graph.md` §4.2; design-language
  §5.3 (live-not-snapshot, permission-aware-per-viewer, tombstones gracefully).
- Permission-aware paginated embedded views: ADR-03 `list_objects`; the §5.6 views invariant.
