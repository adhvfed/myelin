# Sketch 07 — `knowledge.*` taxonomy/events, Search/Refs integration, multi-region collab

> Phase 4, Knowledge, **exploration**. Canonical: `event-bus.md` §6 (taxonomy grammar; I own my
> complete list), `reference-graph.md` (edges + `#sub` + typed `db_relation`/`page_parent` tables I
> own), `search-and-indexing.md` (block-vs-page index docs; vector for RAG), ADR-11 (cell residency;
> collab-session locality), deep-dive Q10/Q14. These are the "consume the Phase-3 contracts" decisions
> plus the multi-region-collab hard case.

---

## 1. The complete `knowledge.*` event taxonomy (TE-10 completion, my P4 deliverable)

Under the bus grammar `<subsystem>.<artifact_type>.<event_name>` (singular, past-tense), the
`knowledge` token, semantic-events-not-keystrokes:

**Pages**: `knowledge.page.created|updated|moved|archived|deleted|restored|published|unpublished`
(coalesced; raw ops stay on the firehose). **Pointer**: `knowledge.doc.updated` (the firehose-pointer
event for collab, bus §4.3 — "lines/ops N..M ready", agents/Search react to this, never per-op).
**Databases/rows**: `knowledge.database.created`, `knowledge.database.schema_changed`,
`knowledge.view.created|updated`, `knowledge.row.created|updated|deleted|moved` (`row.updated` carries
the changed-property delta). **References** (= edge events, Refs consumes):
`refs.edge.created|removed` emitted from `mention`/`artifact_ref`/`embed` nodes (ADR-05 producers).
**Typed lifecycle** (the mirror feed, Refs §3.3): `knowledge.page.parent_set` (page-tree),
`knowledge.relation.created|removed` (db_relation). **Comments/mentions**:
`knowledge.comment.created|resolved`, `knowledge.mention.created`. **Permissions**:
`knowledge.access.granted|revoked`, `knowledge.page.published` (security-relevant + audit). **GDPR**:
`knowledge.subject.export_requested|completed`, `knowledge.subject.erased`. **Cross-cutting**:
`knowledge.*.erased` (tombstone), `knowledge.*.snapshot` (reindex-from-source).

Every envelope carries the non-negotiable fields (`event-bus.md` §3.1): `event_id`, `tenant`,
`region`, `actor` (incl. agent on-behalf-of), `subject` (`ArtifactRef`), nested causality,
`contains_personal_data`, `visibility`, `pii_key_ref`. Emitted **via the outbox only** (BUS-2), in the
same tx as the state change.

**Sub-artifact `#sub` scheme** (my P4 deliverable, `reference-graph.md` §9 OQ): `#b<block_id>` for a
block, `#row-<row_id>`, `#view-<view_id>`. **Stable across edits/moves** (a block that moves keeps its
id, sketch 02) so embeds/refs never dangle.

## 2. The typed relation tables I own (Refs TE-7 mirror)

Per the Phase-3 handoff I own **`db_relation`** and **`page_parent`** as the **source of truth** for
lifecycle edges (the typed table wins, REF-1; Refs holds a rebuildable projection):

- **`page_parent(tenant, region, page_id, parent_page_id, …)`** — the page-tree parent edge; the
  source of truth for `parent` lifecycle edges + the permission-inheritance hierarchy (sketch 04). A
  `page.move` updates this row + emits `knowledge.page.parent_set` + updates the `parent_page` ReBAC
  tuple, in lockstep.
- **`db_relation(tenant, region, relation_id, src_row, dst_ref, rel, …)`** — the two-way db relation
  field type (ADR-06), the source of truth for `relates`/`depends_on`/`parent` between rows (and
  cross-artifact, where `dst_ref` points at an issue/PR). The same write emits
  `knowledge.relation.created`, which Refs projects as a `lifecycle`-class edge so cross-subsystem
  traversal ("everything related to this doc across all five subsystems") is one Refs query
  (`reference-graph.md` §3.3). Referential integrity + the relation field type live here (Refs can't
  give them); Refs is the fast cross-subsystem reader.

## 3. Search integration (block-vs-page granularity — deep-dive Q10)

Search consumes off the bus; Knowledge declares its `IndexSpec` + implements `project` (no cross-DB,
`search-and-indexing.md` §5.3). The granularity question:

- **Decision: index at BLOCK granularity for pages, ROW granularity for databases**, with the page as
  the rollup. A block-level index doc (`knowledge/block/PAGE#b9`) enables **jump-to-block** search
  results (the deep-dive §2.9 want) and aligns with the `#sub` addressing + the reference graph's
  `target_root` rollup (a backlink to any block rolls up to the page, `reference-graph.md` §3.2). It
  multiplies index size (the deep-dive Q10 cost) — accepted because the value (precise jump + block
  embeds + per-block permission alignment) is high and the cost is bounded per-tenant. A page-level
  rollup doc gives whole-page relevance.
- **Vector/semantic for agent RAG (deep-dive Q10): YES in v1** — semantic search is needed for agent
  RAG over the knowledge base (the obvious agent use case, deep-dive §2.9). Search's vector tier is
  already built (`search-and-indexing.md` §3.3, HNSW, ACL-filtered-during-traversal, erasable
  embeddings); Knowledge declares `semantic: true` for page/block content. Embeddings are personal
  data → erased with source (sketch 06). The embedding model is the swappable EU-hostable adapter
  (Search's floor).
- **Permission-aware**: every search/embedded-view/backlink read pre-filters via `list_objects`
  (sketch 04) — leak-free by construction.

## 4. Multi-region collab + EU residency (deep-dive Q14 — the hard case)

The hard case (deep-dive §5.7, §10 Q14): a collab session is latency-sensitive and stateful; where does
the **authoritative collab server for a doc** live, and how is session state sharded under residency?

- **The doc's authoritative collab server lives in the doc's tenant's cell/region** (ADR-11 collab-
  session locality; the op-log + session actor, sketch 01, are cell-local + residency-pinned). A doc
  belongs to a tenant; the tenant is region-pinned; so the doc's op-log and session authority are
  **pinned to that region** — residency is structural, not a routing choice. There is no cross-region
  authoritative collab server.
- **Latency for a globally-distributed team editing an EU-pinned doc** is bought with **optimistic
  local apply + in-region edge + the resume-cursor transport** (design-language P2: "perceived speed is
  bought with optimistic UI, in-region edge, and prefetch, *not* global replication"), **not** by
  replicating the authoritative state across regions (which would violate residency). A user in another
  region applies ops optimistically locally and syncs to the EU-pinned authority over the
  resume-cursor transport (zero-loss on the higher-latency link is exactly what the cursor guarantees).
- **Multi-cell tenant (a 10,000-person org spanning cells, SC-2/SC-3)**: a doc lives in *one* cell
  (its tenant's home cell for that doc); cross-cell *reads/embeds* ride the **control-plane PII-free
  pointer bridge** (`event-bus.md` §7.4, `reference-graph.md` §6.5) — resolved per-viewer locally,
  never moving the doc's personal data across cells. **This cross-cell collab fan-out is a named
  floor** (inherits the bus/Refs cross-cell floor); single-cell collab is the v1 design. The contracts
  are cell-agnostic so it extends without a rewrite.

## 5. Comments: reuse Chat threading or KB-native? (deep-dive Q12, cross-subsystem)

- **Leaning: KB-native anchored comments over the shared content model + the shared comment/thread
  primitive (design-language §5.5)**, NOT a dependency on the Chat subsystem's storage. Comments anchor
  to a text range / block / sub-artifact (`ArtifactRef#sub`) and survive/relocate sensibly (the
  diff-anchoring-shaped problem). They reuse the *shared* comment/thread component + the
  `mention`/`artifact_ref` nodes (so a comment is the same conversation primitive as a PR review or an
  issue discussion, §5.5), but Knowledge owns the *anchoring + storage* (a comment is anchored to a
  block id, which Knowledge owns). Whether the *thread rendering* reuses Chat's component is a frontend
  reuse detail; the *data + anchoring* is KB-native. This keeps the deep-dive Q12 boundary clean: share
  the conversation primitive, own the anchor.

## 6. What this sketch commits to the findings

- **Taxonomy**: the complete `knowledge.*` list above (semantic events, outbox-only, coalesced);
  `knowledge.doc.updated` firehose-pointer for collab; `#sub` scheme `#b<id>`/`#row-<id>`/`#view-<id>`,
  stable across edits.
- **Typed tables I own**: `page_parent` (page-tree, parent lifecycle edge) + `db_relation` (two-way
  relation field, source of truth; Refs projects the lifecycle edge for cross-subsystem traversal).
- **Search**: block-granular index for pages + row-granular for DBs (page rollup), enabling
  jump-to-block; vector/semantic for agent RAG in v1; ACL-pre-filtered; embeddings erased with source.
- **Multi-region collab**: the doc's authoritative collab server + op-log are pinned to the doc's
  tenant's region (residency structural); cross-region latency bought with optimistic local apply +
  in-region edge + the resume-cursor transport, never cross-region replication. Cross-cell collab
  fan-out is a named floor (control-plane PII-free pointer bridge); single-cell is v1.
- **Comments**: KB-native anchored comments (own the anchor) over the shared comment/thread primitive +
  shared inline nodes (share the conversation primitive). Not a Chat-storage dependency.

## Cited prior art

- Taxonomy grammar: `event-bus.md` §6 (subsystem-prefixed singular dotted, past-tense). Outbox-only:
  BUS-2. Firehose pointer split: bus §4.3 / ADR-04.5.
- Block-level search: the deep-dive §2.9 jump-to-block; Tantivy block docs (`search-and-indexing.md`).
- Vector RAG: HNSW (Malkov & Yashunin 2018, `search-and-indexing.md` §3.3); embeddings-as-personal-data
  (gdpr §6.6).
- Collab-session residency: ADR-11 (cell locality); the resume-cursor transport (sketch 01) for the
  higher-latency cross-region link; design-language P2 (optimistic-not-replicated).
- Multi-cell pointer bridge: `event-bus.md` §7.4; `reference-graph.md` §6.5 (the named floor).
