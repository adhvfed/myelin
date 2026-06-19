# Sketch 02 — Block-tree storage (TE-16) + the content model (ADR-05 / KN-2)

> Phase 4, Knowledge, **exploration**. Canonical: KN-2 (markdown-subset inline string), ADR-05
> (shared content/block model, Knowledge leads), KN-4 (one editor render path), EI-04 §2.4,
> decision-record §(d).3 (the AST/string representation seam). I LEAD the shared block taxonomy;
> Chat/Issues consume it.

---

## 0. The two-layer rule the doctrine already fixed

decision-record §(d).3 resolves the only tension as a **representation seam, not a disagreement**:

- **Block structure stays an AST** (typed nodes in a tree) — `myelin-content` (ADR-05).
- **Inline runs are a markdown-subset STRING** (KN-2/D10/EI-04 §2.4) — *not* an inline-range JSON
  model — because it survives copy/paste, export, diff, and reference-extraction, needs no
  server-side sanitisation pass, keeps the reference grammar server-side, and "survived an entire
  editor rewrite with zero schema migration."
- **`mention(Principal)` / `artifact_ref(ArtifactRef)` / `embed(ArtifactRef)` stay STRUCTURED
  NODES** — never collapsed into the inline string — so reference-extraction (the ADR-05 producer of
  `ref.created`) stays reliable and is a structured-node walk, not a regex over prose.

So the content model is **decided in shape**; this sketch is about (a) how the block *tree* is
*stored* (TE-16) and (b) pinning the taxonomy + the inline-node embedding rule I must lead for Chat
and Issues.

---

## 1. Block-tree storage (TE-16) — candidates

A block: stable `block_id` (survives moves/edits/collab; a reference target), `type`, type-props,
inline content (the md-subset string + structured inline nodes), ordered children → the page is a
tree. The question is the *physical* model.

### Candidate A — Per-block rows (adjacency list + fractional order key)

Each block is a Postgres row: `block(tenant, doc_id, block_id, parent_id, order_key, type, props
jsonb, inline_md text, inline_nodes jsonb, …)`. Sibling order via a **fractional index** (LexoRank /
fractional keys) so concurrent insert needs no renumber. This is Notion's historical model.

- **Pro**: scales to huge docs (thousands of blocks) with **lazy/partial load** (fetch a subtree, not
  the whole doc — the loading-state requirement, §wireframes). **Block-level refs and permissions**
  are natural (a `block_id` is an `ArtifactRef` sub-anchor `#b9`; a block can be a permission/erasure
  boundary). Cross-block queries (backlinks to a block, search per block) work. Aligns with the
  reference-graph's `#sub` sub-artifact addressing (`reference-graph.md` §3.5) and Search's
  block-level index docs.
- **Con**: whole-doc atomicity + collaboration are harder (a tree spread over rows vs one mergeable
  blob). Fractional indexing has **interleaving/precision pitfalls** under heavy concurrency (the
  deep-dive §2.1 warning) — mitigated by the move-CRDT (sketch 01) when promoted, and by occasional
  rebalancing of order keys.

### Candidate B — Document-as-single-CRDT-blob

The whole page is one Yrs/Automerge document (one CRDT blob, op-log + snapshot).

- **Pro**: simplest collaboration (one CRDT per doc, the common Yjs deployment). The op-log/snapshot
  transport (sketch 01) maps directly.
- **Con**: **caps document size** (a 5,000-block all-hands doc is one giant blob to load/merge — the
  hot-document case, deep-dive §5.9), makes **block-level refs/permissions/search** awkward (the
  block id is inside the blob, not a queryable row), and makes cross-block queries hard. The wedge
  (embed/reference *a block*) wants block addressability the blob fights.

### Candidate C — Hybrid: per-block rows as source of truth + per-block inline-content op-stream

Block *structure* (tree shape, order, type) lives as **per-block rows** (Candidate A: adjacency list
+ fractional order, the queryable, addressable, partially-loadable form). Block *inline content*
(the md-subset string) is, in the floor, a column; on CRDT promotion, the inline content of a block
becomes a **small per-block Yrs document** (sketch 01) addressed by `block_id`. The tree structure
itself is governed by the **move-CRDT** (or CAS in the floor) over the op-log.

- **Pro**: gets Candidate A's addressability/partial-load/block-level-everything *and* a clean CRDT
  promotion path (per-block text CRDT keeps each mergeable unit small — no giant blob), and the tree
  CRDT handles moves. This is exactly the deep-dive §6 "tree/move CRDT for structure + per-block
  content CRDT" leading candidate, made concrete.
- **Con**: most moving parts. But the parts are *separable* and individually testable (the structure
  layer, the inline layer), which suits the floor→promotion ladder (the structure CAS and the inline
  string ship first; the per-block text CRDT is the promotion).

### Storage leaning

**Candidate C (hybrid): per-block rows for structure (adjacency list + fractional order key, the
source of truth for tree shape/order/type) + inline content as the md-subset string in the floor,
promotable to a per-block Yrs text-CRDT addressed by `block_id`.** This is the only model that
satisfies all four hard requirements simultaneously: huge-doc partial load, block-level
refs/permissions/erasure, the reference-graph `#sub` addressing, and a clean CRDT promotion that
keeps each mergeable unit small. The single-blob (B) caps doc size and fights the wedge; pure rows
without the per-block-CRDT path (A alone) re-opens the merge problem at promotion.

`block_id` is **stable across moves/edits** (the `#sub` stability the reference graph requires,
`reference-graph.md` §9 OQ) — a moved block keeps its id, so an embed of `…#b9` never dangles on
reorder. Order is a fractional key (rebalanced occasionally); the move-CRDT (on promotion) handles
concurrent re-parent + cycle-break (Kleppmann move op).

---

## 2. The shared block/inline taxonomy I lead (ADR-05)

I own the canonical taxonomy; Chat/Issues consume a subset. The split:

**Block node types (the tree):** `paragraph`, `heading{1..3}`, `bulleted_list_item`,
`numbered_list_item`, `to_do`, `toggle`, `quote`, `callout`, `code{lang}`, `divider`, `equation`,
`image`, `file`, `table`/`table_row`/`table_cell`, `column_list`/`column`, `database_view`
(an inline DB view, `embed(ArtifactRef→knowledge/view)`), `embed` (any `ArtifactRef`),
`synced_block` (transclusion — **deferred**, sketch 09). Each block: `type` + type-props +
inline-content + ordered children.

**Inline content of a block = `{ md: string, nodes: [InlineNode] }`** where:
- `md` is the **markdown-subset string** (KN-2): `**bold**`, `*italic*`, `` `code` ``, `~~strike~~`,
  `[text](url)`, headings/lists are *block* concerns not inline. This is the string KN-4's
  `render(parse(md)) === md` gate runs over.
- `nodes` is the array of **structured inline nodes** that the md string *cannot* safely carry as
  text: `mention(Principal)`, `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)`, `date_mention`,
  `inline_equation`. They are anchored into the md string by a **stable placeholder token** (e.g. a
  private-use-area sentinel char or an explicit `{{ref:N}}` marker the parser owns) so the string
  stays human-readable-ish and round-trips, while the structured node carries the real
  `ArtifactRef`/`Principal`. This is the "kept as structured nodes, never collapsed into the string"
  rule (decision-record §(d).3) made concrete — and it is what makes reference-extraction a
  **node-array walk** (reliable) rather than a regex over prose (unreliable).

**Why this anchoring, not pure-string-with-markdown-links**: a markdown link `[#42](myelin://…)` *could*
encode a ref, but then erasure/rename/tombstoning would have to rewrite prose bytes, and an `@alice`
mention would be a display string baked into content (re-introducing the PII-in-bytes problem). Keeping
the ref as a structured node with a placeholder means **rename/erase/tombstone never touch the stored
string** — the node resolves to the current display via Refs at render time (REF-3 display-keys-are-
render-time). This is the erasure-stability + reference-reliability win in one.

**Chat/Issues consume**: the same block + inline taxonomy, the same three structured inline nodes
(so a `#issue` mention in a chat message, an issue comment, and a doc block are the *same* node,
resolved the same way by Refs, rendered by the same §5.3 chip). Chat restricts to a small block
subset (paragraph/code/quote/list + the inline nodes); Issues uses descriptions/comments. **Concurrency
is NOT shared** (ADR-05): Knowledge gets collab; chat messages are mostly-immutable; issue descriptions
single-author — they share the *AST + the string + the nodes*, not the editing engine.

---

## 3. The one editor render path (KN-4) — the storage consequence

KN-4: read and edit run the **same inline parser**; `render(parse(md)) === md` over a corpus is a hard
gate; controlled `contenteditable` (not `<textarea>`); **caret = char offset into the serialised
markdown**, bridged to/from the DOM. The storage choice supports this directly because:

- inline content **is** the md-subset string → there is one `parseInline(md) → render tree` pipeline,
  used by both read and edit mode (no two divergent renderers — the EI-05 §2 trap avoided
  structurally).
- the caret offset is **an index into that same string** → the offset model is well-defined and
  unit-testable standalone (KN-4: "the serializer, the offset model, and the DOM-surgery for
  Enter-splits-block / caret-after-split are independently tested before the integrated editor").
- structured inline nodes occupy **a single offset position** (the placeholder token) in the string,
  so the caret model treats a mention/ref as one atomic character — clean caret behaviour around
  chips.

The **Enter-splits-block** and **caret-after-split** primitives are block-tree operations (split a
block row into two at a caret offset, re-parent children) — they live in the structure layer
(Candidate C's per-block rows), unit-tested standalone (KN-4). "Enter just inserts a newline" is the
#1 "not a real editor" tell; splitting a block is the real behaviour.

## 4. What this sketch commits to the findings

- **TE-16**: hybrid — per-block rows (adjacency list + fractional order key, stable `block_id`) as
  the source of truth for tree structure; inline content as the md-subset string (floor),
  promotable to a per-block Yrs text-CRDT. Block-level addressability/permissions/erasure;
  partial/lazy load for huge docs.
- **ADR-05 (KN-2)**: inline content = `{ md: markdown-subset string, nodes: [structured inline
  nodes] }`; `mention`/`artifact_ref`/`embed` are structured nodes anchored by a stable placeholder,
  never collapsed into the string. I lead the block + inline taxonomy; Chat/Issues consume the AST +
  string + nodes, not the editing engine.
- **KN-4**: one `parseInline` pipeline for read+edit; caret = offset into the md string; structured
  nodes are atomic single-offset placeholders; serializer/offset/DOM-surgery ship + unit-test
  standalone before the integrated editor; `render(parse(md)) === md` is the corpus gate.

## Cited prior art

- Block model + adjacency list: Notion's per-block-row model; Celko, *Trees and Hierarchies in SQL*
  (2012, adjacency list); ProseMirror node/mark schema; BlockNote (block editor over ProseMirror+Yjs).
- Fractional indexing: Figma's fractional-index / LexoRank ordering for concurrent insert.
- Markdown-subset inline string + one render path: EI-05 §2 / KN-2 (the editor-rewrite-survival
  argument); CommonMark as the md-subset reference grammar.
- Structured inline reference nodes: ADR-05 (`mention`/`artifact_ref`/`embed` as producers of
  `ref.created`); the deep-dive §2.2 "inline references must be structured tokens, not hyperlinks."
