# Phase 5 — Refined Cross-Artifact Reference Graph (`myelin-refs`)

> Phase: `05-refined-shared-systems-architecture` (the refined, canonical shared-system architecture Phase
> 6/7 build on). Canonical brief: [`VISION.md`](../../VISION.md) §1 (the reference graph as the connective
> tissue / core moat). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) **§7** ("one
> canonical reference graph"; backlinks are event-sourced projections; lifecycle edges mirrored to a typed
> table owned by the authoritative service), §1/§8/§10; and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure vs
> immutability), §5.3 (reindex-from-source).
>
> **Reconciliation spine (binding):**
> [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) (X-1..X-7, OQ-A..OQ-L) +
> [`contract-index.md`](./contract-index.md) (the frozen build-to surface; this doc's contracts §5 match it
> exactly). **Phase-3 base carried forward:**
> [`../03-shared-systems-architecture/reference-graph.md`](../03-shared-systems-architecture/reference-graph.md).
> Change-requests applied: [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
> §3 (Reference Graph) + the cross-cutting asks (§1 `list_objects`, §10 cross-cell). Spine: **ADR-13**
> (the three glue contracts; the Reference Graph clause; TE-7), **ADR-14** (PG-by-default), ADR-03/04/05/11/12.
> Directives: **REF-1..REF-4**, X-1, ID-3, GD-3. Date: 2026-06-19.
>
> **What this doc is.** The Phase-3 `myelin-refs` design, **confirmed and additively sharpened** by the
> Phase-5 reconciliation. **No ADR is reversed** (none was requested). The Phase-3 structure is preserved
> (purpose; prior art; data model; algorithms; contracts exposed & consumed; scaling; failure modes + drills;
> open questions). Where a section is unchanged from Phase 3 it says so and cites the Phase-3 section rather
> than restating it.

---

## Changes vs Phase 3 (every change, itemised)

The reconciliation touches Refs in **five** places. Three are SHARPEN (an open Phase-3 encoding is now frozen
concrete); two are CONFIRM-with-pinned-shape; the rest of the Phase-3 design is **CONFIRMED unchanged**.

| # | Change | Kind | Recon source | Contract | Phase-3 base |
|---|---|---|---|---|---|
| **C-1** | **The unified `#sub` sub-artifact grammar is frozen** — one vocabulary (`comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-`) covering all three content shapes; stable opaque ids minted by each owner; Refs stores the full sub-URN **and** the `#sub`-stripped root. | **SHARPEN → frozen** (was `[OPEN → P4]` per-subsystem mint scheme) | X-4 / OQ-D | **5.7** | §3.1, §3.5 |
| **C-2** | **One 4-step tombstone / graceful-degradation ladder is frozen** (permission → root → sub-resolve {live/moved/outdated/gone} → erased); a tombstone **always carries the root**. **Git line-ranges are content-anchored** (BLAKE3 fingerprint + 3-way context match → exact/rebased/partial/tombstone), replacing the Phase-3 "partial/relocated projection" sketch with the concrete resolver. | **SHARPEN → frozen** | X-4 / OQ-D | **5.7** | §3.5, §4.6 |
| **C-3** | **The Issues `<id>` grammar `<PROJECTKEY>-<seqno>` is frozen as the stored canonical key**; `#1421` is the render-time display projection. REF-3's "display keys are render-time, never stored" is reconciled with "the human key IS the id" — no contradiction, the *canonical* key is stored, the *short* form is projected. | **SHARPEN → frozen** (was the §3-blocking RECONCILE item) | recon §3 / REF-3 | **5.1** | §3.1, §4.8 |
| **C-4** | **The `list_objects` `Filter{set_expr, zookie}` is now a concrete `SetExpr`** Refs lowers into its backlink/traverse SQL (`WHERE source IN …` / a JOIN against the per-tenant authz reverse index over the `source`/`source_root` column). Phase-3 named the *contract*; Phase-5 froze the *encoding* — Refs is a first-class consumer of it. | **SHARPEN (consumes the frozen 4.3)** (was `[OPEN → P4]` encoding) | OQ-E | **4.3** (consumed), **5.3/5.4** | §4.4, §8.1 |
| **C-5** | **Cross-cell resolution is pinned cell-local** — a cross-cell `target` resolves **in the home cell** (the home cell renders + permission-checks; only the already-filtered projection or a tombstone crosses), over the frozen `CrossCellPointer{subject, type, correlation_id, home_cell}` frame. This closes the Phase-3 "designed-not-built" cross-cell fan-out floor's *resolution semantics* (the build stays a named floor). | **CONFIRM (frame pinned)** | OQ-I | **12.6** (consumed), **5.2** | §6.4, §6.5 |
| **C-6** | **`details_ref` `#step-<n>` / `check-<context>` sub-anchors are first-class `#sub` kinds** in the unified grammar (so CI's jump-to-failure anchor and the `CheckStatus.details_ref` resolve through the same ladder as every other sub-anchor). | **CONFIRM (vocabulary extended)** | X-1, X-4 | **5.7** | §3.5 |

**CONFIRMED unchanged from Phase 3** (cited, not restated): the URN value type + parse/format ambiguity
rejection (§3.1); the event-sourced `edge` inverse index + deterministic `edge_id` idempotent rebuild (§3.2,
§4.1, §4.3); the **TE-7 hybrid** typed-edge mirror (typed table = source of truth, Refs = rebuildable
projection, REF-1/D11; §3.3); PG + recursive CTEs over a graph DB with the named `EdgeStore` escape hatch
(§2.4, §3.4, §4.5); the projection cache as a bounded invalidatable holder (§3.6); reindex-from-source as the
only recovery path (§4.7); the `PersonalDataHolder` erasure surface (§4.6); the hot-artifact Leopard-style
reach index as a measured-trigger floor (§6.3); cross-tenant gating via the `public` userset, never a
cross-tenant join (§6.4); all ten Phase-3 drills D-1..D-10 (§7).

**Net for Refs:** Refs gains **no new exposed contract and no new engine**. It freezes one grammar + one
ladder it already owned (5.7), reconciles one id-grammar question (5.1), and confirms it is a clean *consumer*
of the now-frozen `list_objects` `SetExpr` (4.3) and cross-cell bridge (12.6). The moat thesis (§1) is intact.

---

## 0. Reading map

- **§1** — purpose, the moat thesis, what Refs is NOT. **Unchanged** (Phase-3 §1); restated in brief.
- **§2** — prior art. **Unchanged** (Phase-3 §2); cited, not repeated.
- **§3** — data model: the URN (+ frozen Issues key grammar, C-3), the edge index, the TE-7 mirror, the
  **frozen unified `#sub` grammar** (C-1/C-6).
- **§4** — algorithms: extraction→emit, resolution (+ cell-local cross-cell, C-5), the leak-free backlink read
  (+ the frozen `SetExpr` lowering, C-4), the **frozen tombstone ladder** (C-2), reindex-from-source.
- **§5** — the contracts exposed & consumed. **Stable, matching the refined contract index.**
- **§6** — scaling / hot-artifact / cross-tenant / cross-cell. **Unchanged**; the cross-cell resolution
  semantics pinned (C-5).
- **§7** — failure modes + drills (the Phase-3 register, confirmed; the tombstone-ladder drill sharpened).
- **§8** — required changes to foundational systems (now mostly satisfied by the reconciliation).
- **§9** — open questions remaining for Phase 6.
- **§10** — cited prior art (full). **Unchanged** (Phase-3 §10).

**Floors named up front** (VISION §3 / EI-04 §4): in-cell graph build is the design; **cross-cell backlink
fan-out (build) is a named floor** (§6.5) — but its *resolution semantics* are now pinned cell-local (C-5).
The **hot-artifact reach index (R4)** is a measured-trigger floor (§6.3). All unchanged from Phase 3.

---

## 1. Purpose, responsibilities, the one-paragraph thesis — CONFIRMED (Phase-3 §1)

Unchanged from Phase 3 §1. In brief, for self-containment:

> *The same graph that lets one action cascade across six subsystems with no human effort is the graph that
> lets a person jump from a failing test to the line of code to the issue to the conversation in four
> keystrokes. Build the graph once; both the machines and the humans win.* (EI-02 §7; VISION §1.)

`myelin-refs` makes "everything is addressable, and everything that references anything is traversable, **live
and permission-aware**" true by construction. It owns: (1) the URN `ArtifactRef` library (parse/format/resolve,
rejects ambiguity — REF-3); (2) the resolution service (current per-viewer projection + permission check +
update events; stores no content); (3) the event-sourced backlink inverse index (rebuilt from
`refs.edge.created`/`refs.edge.removed`, permission-filtered at read time via Id `list_objects` — REF-1);
(4) the TE-7 typed-edge mirror (lifecycle edges dual-homed; the typed table is truth, Refs is the projection —
D11); (5) sub-artifact addressing + graceful tombstoning; (6) cross-tenant visibility gating; (7) reindex-
from-source. **What Refs is NOT** is unchanged (Phase-3 §1.4): not the lifecycle-edge authority, not a content
store, not the permission authority, not a graph DB, not a second emit path.

The one-paragraph thesis (Phase-3 §1.3) stands verbatim: *Refs is a thin, event-sourced projection over the
platform's edge facts; an edge exists because a `refs.edge.created` event exists; the backlink index is the
denormalised inverse of that log; the forward graph is a Postgres adjacency list walked with recursive CTEs;
every backlink read is gated by Id's `list_objects` pre-filter so the answer is leak-free by construction;
lifecycle edges that drive behaviour are also owned as typed rows by the enforcing subsystem; Refs never
invents an edge, never stores content, never reads another store, and is fully rebuildable from the log.*

---

## 2. Prior art — CONFIRMED unchanged (Phase-3 §2)

Unchanged. The design stands on: adjacency-list-in-RDBMS + recursive CTE (Celko; PostgreSQL §7.8); closure
table for hot/deep subtrees (Karwin); the log-as-truth + reindex-from-source (Kreps; Kleppmann DDIA ch. 11);
permission-filtered set reads + the **Leopard** set-flattened index + **zookies** (Zanzibar, USENIX ATC 2019);
content-addressed URN identity (Git object model; RFC 8141); backlinks as the associative trail (Bush 1945;
Nelson/Xanadu; Roam/Obsidian); tombstoning/pseudonymisation for erasure over immutable structure (Kleppmann
ch. 5; EI-04 §1). The **why-PG-not-graph-DB** justification (Phase-3 §2.4) is unchanged: shallow bounded
graphs, no dual-write, the hot path is the permission filter not the traversal, operational minimalism; the
`EdgeStore` trait is the named, measured-only escape hatch. Full list in §10.

---

## 3. The data model / schema

All tables: `(tenant, region)` first columns / partition prefix, RLS-enforced, **no cross-tenant query path**
(EI-02 §1; ID-3). Every store is residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, and a
`PersonalDataHolder` auto-registered by the bootstrap harness (substrate §3.4; ADR-12; contract 1.4).
**Unchanged from Phase 3 §3** except §3.1 (the Issues key grammar, C-3) and §3.5 (the frozen `#sub` grammar,
C-1/C-6).

### 3.1 The URN `ArtifactRef` — CONFIRMED + the Issues key grammar frozen (C-3, contract 5.1)

```
myelin://<tenant>/<subsystem>/<type>/<id>[#<sub>]
        └tenant┘ └ event-bus §6.2 (canonical singular tokens) ┘ └ §3.5 ┘
```

The `<subsystem>`/`<type>` token table is **owned by event-bus §6.2** (the reconciliation anchor, CONFIRMED
unchanged); Refs is the **validator and primary consumer**, not a second authority (contract index §14). Refs'
`parse` enforces (REF-3, unchanged): scope is explicit and total (`tenant`/`subsystem`/`type`/`id` all
required); a scope-less / short-hash ref (`#42`, `@alice`, `~general`, a 7-char prefix) is **rejected, never
guessed** — those are display projections (§4.8); the `<sub>` grammar is §3.5; the `tenant` segment pins the
cell/region (residency invariant; resolution happens in the holding cell — §4.2/§6).

**The Issues `<id>` grammar is now frozen (C-3, the resolved §3-blocking RECONCILE item).** The canonical
`<id>` segment for an issue is **`<PROJECTKEY>-<seqno>`** (e.g. `ENG-1421`) — the project-prefix + monotonic
number **is** the stored id in `myelin://<tenant>/issue/issue/ENG-1421`. This is **not** a contradiction of
REF-3 ("display keys are render-time, never stored as the link"):

- the **canonical key** `ENG-1421` is the *stable, mintable, human-readable id the subsystem owns* and the
  stored `<id>` segment — it is a real URN component, not a display alias;
- the **short display form** `#1421` (dropping the project prefix when the project is contextually obvious) is
  the **render-time projection** (§4.8), exactly as `@alice`/`~general` are. The UI projects `#1421` per
  viewer per context; Refs never stores or resolves `#1421` as a scope.

So "the human key IS the ArtifactRef id" (Issues) and "display keys are render-time" (REF-3) are both true:
the *canonical* human key is stored; the *short* human form is projected. (TE-14 resolved; contract 5.1.)

### 3.2 The edge fact (event-sourced; the inverse index source) — CONFIRMED unchanged (Phase-3 §3.2)

Unchanged. The `edge` table is the **materialised projection** of the `refs.edge.created`/`refs.edge.removed`
log (REF-1/REF-4), rebuildable by replay (§4.7). Key columns (Phase-3 §3.2, verbatim intent):

```sql
CREATE TABLE edge (
  tenant uuid NOT NULL, region text NOT NULL,
  edge_id text NOT NULL,            -- deterministic: hash(tenant, source, target, rel) → idempotent rebuild
  source text NOT NULL,             -- ArtifactRef of the referencing artifact / sub-artifact (full #sub URN)
  source_root text NOT NULL,        -- source with #sub stripped (the outbound walk + the C-4 filter column)
  target text NOT NULL,             -- ArtifactRef of the referenced artifact / sub-artifact (full #sub URN)
  target_root text NOT NULL,        -- target with #sub stripped (the parent) — the hot inbound index key
  rel edge_rel NOT NULL,            -- 'mentions'|'embeds'|'links' | lifecycle rels (§3.3)
  rel_class rel_class NOT NULL,     -- 'reference' (Refs-authoritative) | 'lifecycle' (typed-mirror; §3.3)
  origin_event text NOT NULL,       -- provenance (audit)
  origin_actor text NOT NULL,       -- PSEUDONYMOUS Principal ArtifactRef (erasure-safe; EI-04 §1)
  created_at timestamptz NOT NULL,
  zookie text,                      -- consistency token at edge-write time (§4.4)
  tombstoned boolean NOT NULL DEFAULT false,
  PRIMARY KEY (tenant, edge_id),
  UNIQUE (tenant, source, target, rel)
);
CREATE INDEX edge_inbound  ON edge (tenant, target_root) WHERE NOT tombstoned;  -- "what references this (+children)?"
CREATE INDEX edge_outbound ON edge (tenant, source_root);
CREATE INDEX edge_by_rel   ON edge (tenant, target_root, rel) WHERE rel_class = 'lifecycle';
```

`target_root` as a stored column makes "all backlinks to this artifact and its sub-artifacts" one index range
scan (not a `LIKE` prefix). `rel_class` is the TE-7 seam: `reference` edges are Refs-authoritative projections;
`lifecycle` edges have their **source of truth in the subsystem's typed table** (§3.3) and Refs reconciles by
reindex. (Unchanged; the `source_root` column is the naming the C-4 `SetExpr` lowering targets — §4.4.)

### 3.3 The TE-7 typed-edge mirror — CONFIRMED unchanged (Phase-3 §3.3; REF-1/D11; contract 5.5)

Unchanged. The hybrid resolves TE-7 (D11): lifecycle/semantic edges are **dual-homed** —

- **source of truth = a typed relation table owned by the authoritative subsystem** (Issues `issue_relation`;
  Knowledge `db_relation`/`page_parent`). "Issue #88 is `blocked_by` #91" is a transactional row in the system
  that *enforces* it (referential integrity + transition guards + the ADR-06 relation field type). The URN
  string is **not** the source of truth (REF-1);
- **projection into Refs** — the same transaction emits the typed lifecycle event
  (`issue.relation.created`, `knowledge.page.parent_set`, …) which Refs consumes (§4.1) and projects as a
  `lifecycle`-class `edge` row, so cross-subsystem traversal is one Refs query, not a five-way fan-out.

The lifecycle relation set (CONFIRMED for the mirror contract): `closes`/`blocks`/`blocked_by`/`depends_on`/
`parent`/`assigns`/`relates`. Refs fixes the `rel` **vocabulary**, the `rel_class='lifecycle'` mirror
discipline, and the **inverse pairing** (`blocks`↔`blocked_by`, `parent`↔`child`); the per-subsystem
enumeration + typed-table columns are the subsystem's deliverable (Issues `issue_relation`, Knowledge
`db_relation`/`page_parent`; contract 5.5, REF-1/ISS-1). On any Refs↔typed-table drift, a scoped `reindex`
re-emits the typed snapshots and Refs reconverges — **the typed table always wins** (§4.7; drill D-4).

### 3.4 The adjacency structure walked by recursive CTEs — CONFIRMED unchanged (Phase-3 §3.4)

Unchanged. The `edge` table **is** the adjacency list; a traversal is a `WITH RECURSIVE` walk filtered by
`rel`/`rel_class`, with a visited-set cycle guard and a depth ceiling (default 16). No separate graph-structure
table. The `edge_by_rel` and `edge_outbound` indexes make both walk directions indexed (Celko; PostgreSQL §7.8).

### 3.5 Sub-artifact refs — the unified `#sub` grammar **frozen** (C-1/C-6; X-4/OQ-D; contract 5.7)

This is the section the reconciliation sharpens. Phase 3 fixed only that `#sub` is "a stable opaque token after
`#`" and left the **per-subsystem mint scheme `[OPEN → P4]`** (Phase-3 §3.5). The three content shapes (Git
content-anchored line ranges; Knowledge block/heading/row anchors; Chat message/thread anchors) were each
specified separately. X-4/OQ-D unifies them: **Refs owns one `#sub` grammar and one resolution ladder; each
subsystem mints stable opaque sub-ids of its declared kinds.**

#### The unified `#sub` grammar (frozen — the complete v1 vocabulary)

`<sub>` is one of these **kinds**; the kind prefix makes the grammar self-describing and lets Refs pick the
resolver (and rejects ambiguity — REF-3):

```
#sub kinds (frozen vocabulary):
  comment-<opaqueid>   // a comment / review-thread node        (Git PR, Knowledge, Issues)
  thread-<opaqueid>    // a thread root                         (Chat, Git review thread)
  message-<opaqueid>   // a single chat message                 (Chat)
  b<opaqueid>          // a content block                       (Knowledge, Issue description block)
  h<opaqueid>          // a heading anchor                      (Knowledge)
  row-<opaqueid>       // a database row                        (Knowledge db, Issue-as-row)
  field-<opaqueid>     // a field within a row / issue          (Issues, Knowledge db)
  L<start>-L<end>      // a CONTENT-ANCHORED line range         (Git) — see anchoring below
  check-<context>      // a check status on a commit            (CI, X-1)
  step-<n>             // a CI run step (jump-to-failure)        (CI)
```

`<opaqueid>` is a subsystem-minted **stable opaque id (NOT a positional index — positions move)**. The
**stability obligation is each subsystem's** (Refs §3.5): a block id survives edits/moves; a message id is
immutable; a comment id is immutable. Refs validates the grammar and **rejects ambiguity; it never guesses
scope** (REF-3). Refs stores, in every `edge`, the **full sub-artifact URN** in `source`/`target` (so "this
chat message embeds *block b9 of page 7c2*" is exact) **and** the `#sub`-stripped `*_root` columns (so
backlinks roll up to the parent, §3.2). The `check-<context>` and `step-<n>` kinds make CI's `CheckStatus`
subject (`repo#commit-<oid>/check-<context>`, X-1) and the `details_ref` jump-to-failure anchor first-class
members of the same grammar — they resolve through the same ladder as every other sub-anchor (C-6).

#### Git line-ranges — content-anchored, not positional (the new specificity)

`#L42-L88` is **content-anchored**, not a raw line number. Git stores, alongside the range, a **content
fingerprint** (the BLAKE3 hash of the anchored lines + a small context window, plus the blob oid at mint
time). On resolution against a newer blob the resolver returns one of four states (the standard diff-anchor
technique) — these map directly onto the ladder below:

1. **exact** — the blob oid matches → the exact range (live).
2. **rebased** — blob changed but the fingerprinted lines are found at a shifted position (3-way context
   match) → the shifted range, flagged `moved`.
3. **partial** — some anchored lines survive, some are gone → the surviving sub-range, flagged `outdated`
   (Git's named "outdated-line-range" case).
4. **tombstone** — the anchored content is entirely gone → `Tombstone{root, reason: content_gone}`.

The same `live/moved/outdated/gone` shape is what Knowledge's block/heading/row anchors and Chat's
message/thread anchors return — one ladder for all three shapes (§4.6). (Contract 5.7.)

### 3.6 The projection cache — CONFIRMED unchanged (Phase-3 §3.6)

Unchanged. A bounded, invalidatable, event-busted projection cache per `ArtifactRef` (current title/state/icon/
render hint), keyed `(tenant, ref)`, TTL + `*.updated`/`*.erased` invalidation. A `PersonalDataHolder` (may
hold a title containing a name), **never a source of truth** (STOR-3); on miss/erasure it re-resolves via the
projection API. The Redis/Valkey-class store of S-Refs (§6.6) — ephemeral, residency-pinned, crypto-shred-able.

### 3.7 Stateful-component register — CONFIRMED unchanged (Phase-3 §3.7)

Unchanged: R1 `edge` projection (Postgres-class, derived/rebuildable, per-tenant DEK); R2 projection cache
(Valkey-class, never truth, ephemeral); R3 consumer-dedup ledger; R4 hot-reach flattened index (FLOOR, §6.3).
The **typed relation tables are NOT Refs' components** — they belong to Issues/Knowledge. Everything else in
Refs is stateless and horizontally replaceable, recoverable by reconnecting to the log + reindex-from-source.

---

## 4. The algorithms

**Unchanged from Phase 3 §4** except §4.2 (cross-cell resolution pinned, C-5), §4.4 (the `SetExpr` lowering
frozen, C-4), and §4.6 (the unified tombstone ladder, C-2).

### 4.1 Edge extraction → emit — CONFIRMED unchanged (Phase-3 §4.1; BUS-2)

Unchanged. Edges are born from two producers, both via the **outbox only** (no standalone edge-write API):

1. **Content-node extraction (reference edges).** The three structured inline nodes of `myelin-content`
   (`mention(Principal)`, `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)` — ADR-05; X-2/OQ-B freezes them
   identical across Chat/Issues/Knowledge) are the producers: the **same transaction** that writes content
   emits one `refs.edge.created` per structured ref node (`rel ∈ {mentions, links, embeds}`,
   `rel_class='reference'`). These are *structured nodes*, not a regex over prose — which is why extraction is
   reliable (EI-04 §2.4). The reconciliation confirms (X-2) these three nodes are byte-identical everywhere,
   so the producer is uniform.
2. **Typed-relation writes (lifecycle edges).** When Issues/Knowledge writes a typed relation row (§3.3) the
   same transaction emits a typed lifecycle event; Refs' extraction consumer maps it to a `lifecycle`-class
   edge (both inverse directions).

`edge_id = hash(tenant, source, target, rel)` is deterministic → replay/redelivery upserts the same row
(`ON CONFLICT DO NOTHING/UPDATE`): idempotent rebuild is free. Causality: every `refs.edge.*` is emitted with
`OutboxTx::emit(draft, cause = Some(content_event))` (correlation root carries, causation = the content event,
`depth +1` — BUS-5), which is what lets the loop guard treat only a structured `artifact_ref` node as a
re-trigger source (AG-6).

### 4.2 Resolution: ref → (projection, permission, update-events) — CONFIRMED + cell-local cross-cell (C-5)

The in-cell algorithm is unchanged from Phase-3 §4.2: (1) parse + validate; (2) `Id.check(viewer, view, ref)`
→ denied returns a **tombstone projection**, never a leak (this is why Refs is the chokepoint that makes
unfurls non-leaking — a confidential issue degrades to a placeholder); (3) projection via cache hit, else the
**owning subsystem's `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}`** API through the
resilient client (Refs never reads the owner's DB); (4) the caller subscribes to `*.updated`/`*.erased` so the
rendered ref stays live. Per-viewer correctness: the per-viewer check (step 2) gates a viewer-independent,
ref-keyed cache (step 3) — shared without leaking because no content returns until the check passes.

**Cross-cell resolution is now pinned cell-local (C-5; OQ-I; contract 12.6 consumed, 5.2).** Phase-3 §6.4/§6.5
named the cross-cell case as designed-not-built and said resolution "happens in the cell that holds the
target." The reconciliation freezes the *mechanism*: a viewer in cell A wanting to render a pointer to an
artifact homed in cell B does **not** fetch B's rows into A. Instead A's gateway, holding the viewer's
identity, asks **cell B** to `resolve(ref, viewer, mode)` **in B**, permission-checked **in B** against B's
tuples, and B returns only the **already-rendered, already-permission-filtered projection** (or a tombstone) —
never raw rows, never PII that should stay in B (EI-02 §1; ADR-11 no-cross-region-PII). The control plane
carries only the frozen PII-free pointer:

```
CrossCellPointer { subject: OpaqueSubjectId, type: ArtifactType, correlation_id: CorrelationId, home_cell: CellId }
```

So a cross-cell backlink/embed resolves as a **per-viewer, cell-local projection fetch in the home cell**. The
build of the cross-cell backlink *fan-out* stays the §6.5 named floor; its *resolution semantics* are frozen.

### 4.3 The inverse-index build (consumer side) — CONFIRMED unchanged (Phase-3 §4.3)

Unchanged. Two consumers off the substrate `EventHandler` template: `refs-edge-builder` (whitelists
`evt.*.*.refs.edge.>` + the typed-lifecycle subjects `evt.*.*.issue.relation.>`,
`evt.*.*.knowledge.page.>` — one of the **explicitly reviewed firehose-class infra consumers**, BUS-4; upsert
on `created`, soft/hard-delete on `removed`, tombstone on `*.erased`; ack-after-apply, idempotent on
`event_id`); and `refs-projection-invalidator` (busts R2 on `*.updated`/`*.erased`). Steady-state ingestion
and cold rebuild are the **same code path** → they cannot drift (the reindex parity drill, §7 D-4).

### 4.4 Permission-filtered backlink read — CONFIRMED + the `SetExpr` lowering frozen (C-4; OQ-E; consumes 4.3)

The leak-free-by-construction backlink read (Phase-3 §4.4) is unchanged in *shape*; the reconciliation freezes
the `list_objects` **encoding** Refs composes (Phase 3 left it `[OPEN → P4]`). `backlinks(target, viewer)`:

```
1. target_root := strip_sub(target)
2. result := Id.list_objects(viewer, perm=view, type=<inferred|all>, zookie?)
              → Ids{ids, zookie}  |  Filter{set_expr, zookie}        (contract 4.3 — now frozen, OQ-E)
3. SELECT source, rel, rel_class, origin_actor, tombstoned, zookie
     FROM edge
    WHERE tenant = :viewer.tenant            -- NO cross-tenant path (ID-3)
      AND target_root = :target_root
      AND NOT tombstoned
      AND  <SetExpr lowered over `source_root`>   -- see below
    ORDER BY created_at DESC
    LIMIT :page;                              -- always paginated (hot-artifact safety, §6.3)
```

**The frozen `SetExpr` lowering (C-4).** The filter is **applied to the `source` of each inbound edge** — you
see a backlink iff you can see the artifact that made the reference (REF-1). Refs is a first-class consumer of
the now-frozen `SetExpr` (contract 4.3): it lowers `set_expr` into a SQL predicate / JOIN over its **own
`source_root` id column** (`ColRef{ table: "edge", column: "source_root" }`), exactly as Search composes it
over its id column. The three lowering forms:

- **`Ids` / `NotIds`** → `... AND source_root IN (…)` / `NOT IN (…)` (inlined under the cardinality cap).
- **`InRelation{relation, via_column}`** / **`TupleSet{index}`** → a JOIN against the **per-tenant,
  residency-pinned authz reverse index** Identity maintains (the materialised `(subject, relation, object_id)`
  projection of the ReBAC tuples, kept fresh off the bus — the Zanzibar/Leopard `LookupResources` reverse index
  realised as a co-located JOIN target): `... JOIN authz_visible av ON av.object_id = edge.source_root AND
  av.subject = :viewer AND av.relation = view`. **One query, no N+1, no post-filter.**
- **`Union`/`Intersect`/`Difference`** → `AND`/`OR`/`EXCEPT` composition of the above.

`All` → no predicate (admin sees all); `None` → `WHERE false` (deny). **N+1 is forbidden** (Refs never loops
`check` per inbound edge — the leak-prone, slow anti-pattern). **Zookie carried** so a just-revoked grant can't
read stale ("new enemy", Zanzibar §2.4.4): a security-sensitive backlink read passes the zookie → the JOIN
reads the authz reverse index at-or-after the zookie's revision watermark, bypassing Id's fail-static cache
(contract 4.10). This is the "single most load-bearing inter-system contract" (4.3) and Refs is a clean
consumer of its frozen form. (The §8.1 Phase-3 "required Id change" is now satisfied — see §8.)

### 4.5 Recursive-CTE traversal — CONFIRMED unchanged (Phase-3 §4.5)

Unchanged. Multi-hop questions ("the epic tree", "everything transitively `blocked_by` this", "impact if this
page is erased") walk the adjacency list with a bounded, cycle-safe `WITH RECURSIVE` over `edge` filtered by
`rel`/`rel_class`, with: a `path`-array (or SQL:2023 `CYCLE`) **visited-set cycle guard**; a **depth ceiling**
(default 16); a statement timeout; and **one** `list_objects` post-filter over the collected node set (not
per-hop) where a hop into an unreadable artifact **prunes that branch** (the traversal is not a side-channel).
A request that would exceed the budget returns a **partial result + a "truncated" marker**, never an unbounded
scan (X-3). The CTE is the safe-by-default path; R4 (§6.3) is the measured-hot escalation. A dependency cycle
is surfaced as a **diagnostic**, not a hang (drill D-8).

### 4.6 Tombstoning on erasure / deletion — the unified 4-step ladder **frozen** (C-2; X-4/OQ-D; contract 5.7)

Phase 3 §4.6 specified two cases (artifact deleted → tombstone inbound edges; subject erased → no edge mutation
needed because `origin_actor` is already a pseudonym, plus Id's pseudonym-map shred). The reconciliation
**freezes one resolution ladder** that covers all three content shapes and both cases. The Phase-3 behaviour is
unchanged; the ladder makes it concrete and uniform.

#### The one resolution ladder (frozen) — `resolve(ref, viewer, mode)` for any `#sub`

```
1. permission: check(viewer, read, root)  → Deny  ⇒ Tombstone{ reason: denied }      (never leak — EI-02 §1)
2. root resolve: the parent artifact exists? → No  ⇒ Tombstone{ reason: root_gone }
3. sub resolve via the owner's project(ref, viewer) sub-anchor resolver:
     LIVE      → Projection (the unfurl / embed)
     MOVED     → Projection + flag `moved`           (Git rebased range; KN block moved)
     OUTDATED  → Projection(partial) + flag `outdated`  (Git partial range; KN edited block)
     GONE      → Tombstone{ reason: sub_gone, root }    (root still resolves; the embed shows the parent)
4. ERASED (any level): Tombstone{ reason: erased }   (pseudonym-shred / crypto-shred made it unrenderable)
```

**A tombstone always carries the root** so an embed degrades to "this referenced *&lt;parent artifact&gt;* (the
specific part is no longer available)" rather than vanishing. This is the single graceful-degradation rule for
Git line-ranges (§3.5: exact→LIVE, rebased→MOVED, partial→OUTDATED, content_gone→GONE), Knowledge
block/heading/row anchors (stable across edits → LIVE; edited → OUTDATED; deleted → GONE), and Chat
message/thread anchors (immutable → LIVE; deleted → GONE). The two Phase-3 cases map onto it: **artifact
deleted** → step 2 (`root_gone`) or step 3 (`sub_gone`); **subject erased** → step 4 (`erased`), and because
`origin_actor` is a stable opaque pseudonym (EI-04 §1), erasing the *person* needs **no edge mutation** in the
common case (Id's pseudonym-map shred, contract 4.8, makes the id unresolvable to a human — DSR step 1).

**Refs as a `PersonalDataHolder`** is unchanged (Phase-3 §4.6; ADR-12; GD-3): `locate(subject)` →
edges/cache entries naming the subject; `erase(subject)` → purge R2 cache PII + rely on Id's pseudonym shred
for `origin_actor` (the edge keeps the opaque id; the human becomes unresolvable); content-erased targets are
tombstoned via the `*.erased` consumer. Refs **never holds the PII itself** for the references-not-payloads
case — its erasure surface is small and structural. This is the platform free-text/immutable erasure posture
**instantiated by reference** (contract 10.9 / recon §X-7): Refs does not restate the posture; its residual is
handled per the one platform posture (the `restrict` suppression keeps a restricted subject's references out of
indexing/agent-use/analytics; Refs holds only pseudonymous opaque ids, never the third-party free-text body).

### 4.7 Reindex-from-source — CONFIRMED unchanged (Phase-3 §4.7; REF-4)

Unchanged. The `edge` index + projection cache are **fully reconstructible by asking owners to re-emit through
the live consumer path** (REF-4; EI-04 §5.3; event-bus §4.9): `events::reindex(scope)` → each owner's
`replay(scope, since)` emits `*.snapshot` (content nodes + typed relations, **sub-artifact-granular** — contract
2.6) → `refs-edge-builder` ingests idempotently → the rebuilt index byte-matches the live index (drill D-4).
One code path for steady-state + cold rebuild (no drift). The TE-7 drift-correction: on Refs↔typed-table
disagreement a scoped `reindex` re-converges Refs to the typed table (which always wins). New-consumer
bootstrap and upcaster backfill are the same path. Refs **never reads an owner's DB**.

### 4.8 Display keys are render-time only — CONFIRMED unchanged (Phase-3 §4.8; REF-3)

Unchanged, and now the explicit home of the C-3 reconciliation: `#42`/`#1421`, `@alice`, `~general`, a 7-char
commit prefix are **never stored** in an edge or resolved as a scope — they are derived at render time from the
canonical URN per viewer per locale. For Issues specifically: the stored canonical `<id>` is `ENG-1421`; the
short display `#1421` is the render-time projection (C-3, §3.1). This keeps the stored graph unambiguous and
erasure-stable.

---

## 5. Contracts / APIs exposed and consumed (the glue — STABLE; matches the refined contract index)

Field names + units align to the reconciliation anchors (the `EventEnvelope` field list + units `00 §2.10`;
the `ArtifactRef` token table event-bus §6.2 — both CONFIRMED unchanged). `myelin-refs` (Rust, ADR-01/02)
carries the types + the edge/backlink client; cross-language services consume the same surface over the
internal RPC.

### 5.1 Exposed (what other systems link)

| Contract | Signature (illustrative) | Consumed by | Status | Contract-index # |
|---|---|---|---|---|
| **parse / format** | `parse(&str) → Result<ArtifactRef>` ; `format(&ArtifactRef) → String` | every service | SHARPENED (Issues `<PROJECTKEY>-<seqno>` key frozen; REF-3 reconciled, C-3) | 5.1 |
| **resolve** | `resolve(ref, viewer, mode) → Projection \| Tombstone` | Chat unfurl, PR context pane, KN embeds, Notif (Display) | SHARPENED (cross-cell resolution pinned cell-local, C-5) | 5.2 |
| **edges (outbound)** | `edges(ref, viewer) → [Edge]` | "references" pane, agents | CONFIRMED | 5.3 |
| **backlinks (inbound)** | `backlinks(ref, viewer, page) → [Edge]` | "what references this", impact view, Notif | SHARPENED (consumes the frozen `SetExpr` push-down, C-4) | 5.3 |
| **traverse** | `traverse(root, rels, depth, viewer) → [Path]` | hierarchy / dependency / impact | CONFIRMED (bounded, cycle-safe) | 5.3 |
| **resolve `#sub`** | the unified ladder (§4.6) over the frozen grammar (§3.5) | every unfurl/embed of a sub-anchor | **SHARPENED → frozen** (X-4/OQ-D, C-1/C-2/C-6) | 5.7 |
| **reindex** | `reindex(scope)` | ops, GDPR re-erasure, new consumers | CONFIRMED | 5.8 |
| **project (required ON every subsystem)** | `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}` | Refs/Search/Notif (the only way to read another subsystem's artifact) | CONFIRMED | 5.6 |
| **PersonalDataHolder** | `locate/export/erase(subject)` | DSR orchestrator | CONFIRMED (residual per the one posture, 10.9) | 5.7-class / 10.1 |
| **telemetry** | `backlink_read_latency`, `resolve_cache_hit_ratio`, `index_lag`, `hot_artifact_fanout`, `tombstone_count`, `reindex_parity` | Phase-5/6 drills | CONFIRMED | 1.8 |

**The two emitted events (the edge contract — emitted by producers via the outbox, NOT a Refs write API):**
`refs.edge.created` / `refs.edge.removed` (= `ref.created`/`ref.removed`), envelope `subject` = the **source**
artifact, payload `{ target, rel, rel_class }`. CONFIRMED unchanged (contract 5.4; the `mention`/`artifact_ref`/
`embed` content nodes are the producers; no standalone edge-write API).

### 5.2 Consumed (what Refs depends on — the inbound contracts)

| Dependency | Contract consumed | From | Status vs Phase 3 |
|---|---|---|---|
| **Permission filter** | `check(subject, perm, object, zookie?, caveat?)` + **`list_objects(...) → Ids \| Filter{set_expr, zookie}`** (the frozen `SetExpr`) + `Consistency`/zookie | Identity (contracts 4.2, **4.3**, 4.10) | SHARPENED — the `SetExpr` encoding is now frozen (C-4); Refs lowers it over `source_root` |
| **Edge facts** | `EventEnvelope` + `refs.edge.*` / typed-lifecycle events + the `EventHandler` consumer template | Event Bus (contracts 2.1–2.6) | CONFIRMED |
| **Re-emit** | `events::reindex(scope)` + each owner's `replay(scope, since)` (sub-artifact-granular `*.snapshot`) | Event Bus + every subsystem (contract 2.6) | CONFIRMED |
| **Projection content** | each subsystem's `project(ref, viewer)` API (incl. the `sub_anchor` resolver for the §4.6 ladder) | every subsystem (contract 5.6) | CONFIRMED — the `sub_anchor` resolver now returns the frozen `live/moved/outdated/gone` state (C-2) |
| **Typed-edge truth** | the typed relation tables, read **only via their events** | Issues + Knowledge (contract 5.5; REF-1/ISS-1) | CONFIRMED |
| **Cross-cell pointer** | `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution always cell-local | Control plane / Tenancy (contract 12.6) | CONFIRMED — frame frozen (C-5) |
| **Substrate** | `serve(AppSpec)`, resilient client, fail-static, forward-only migrations, holder auto-registration | substrate (contracts 1.1, 1.4, 1.9, 1.10) | CONFIRMED |

---

## 6. Scaling / sharding in the cell topology — CONFIRMED unchanged (Phase-3 §6); cross-cell semantics pinned

**Unchanged from Phase 3 §6** except the cross-cell resolution semantics (C-5, folded into §4.2). In brief:

- **§6.1 In-cell, tenant-partitioned, async off the bus** — unchanged. Refs is cell-local; the heavy work
  (index build, resolution, traversal) is async off the bus, never synchronous in a write path. `(tenant,
  region)`-partitioned; no cross-tenant edge/query (EI-02 §1; ID-3); per-tenant in-flight caps for fairness.
- **§6.2 Measure before you shard** — unchanged. First moves in order: a **read replica** for the hot backlink
  read (ID-4-class, the doctrine's named first scaling need); bounded pools + statement timeouts + pagination;
  the projection cache (R2) for unfurl/embed read storms. Sharding `edge` is deferred until a measured hot
  tenant outgrows one shard (shard key already `(tenant, region)` + `target_root` hash → re-home, not redesign).
- **§6.3 Hot-artifact backlink scale (the "viral PR" case)** — unchanged FLOOR. **Built:** read-time CTE +
  `list_objects` filter + pagination + replica (you never materialise 10,000 backlinks — you page them).
  **Follow-on (measured-trigger):** a Leopard-style flattened reach index (R4), derived/rebuildable from R1,
  incrementally maintained from `refs.edge.*`, gated by the same `list_objects` filter; promotion trigger =
  **measured hot-fanout exceeding the read budget** (R5), not predicted. Mirrors Id's hot-tuple fan-in handling.
- **§6.4 Cross-tenant edge visibility gating** — unchanged. No cross-tenant edge row ever; a cross-tenant
  `target` resolves only via Id's narrow `public` userset (never a cross-tenant join); backlinks do **not**
  cross tenants by default (showing "tenant B references you" leaks B — a PII/competitive side-channel); the
  *mechanism* (`public` userset) is DECIDED, the *inbound-visibility policy* is a P4/legal call (§9).
- **§6.5 Cross-cell backlink fan-out for multi-cell tenants** — **FLOOR (build) unchanged**, but the
  **resolution semantics are now pinned cell-local** (C-5, §4.2): the home cell renders + permission-checks +
  returns only the projection (or tombstone); the control plane carries only the frozen `CrossCellPointer`
  (no payload, no PII; residency-preserving). The contracts (§5) are cell-agnostic, so the fan-out build
  extends without a rewrite. Follow-on owner: P4/P6 control-plane + multi-cell tenancy (SC-2/SC-3), co-designed
  with the bus's identical cross-cell bridge (contract 12.6).
- **§6.6 Stateful-component register** — unchanged (§3.7): R1/R3 are the projection systems of record
  (rebuildable from the log); R2/R4 are derived/ephemeral; the typed tables belong to Issues/Knowledge.

---

## 7. Failure modes + the drills owed — CONFIRMED (Phase-3 §7); the tombstone-ladder drill sharpened

The ten Phase-3 drills (D-1..D-10) are **CONFIRMED unchanged** as the obligation register; each property that
can fail names its quantified drill (a green artifact ⇒ "proven", T-2/T-5). Summarised, with the one drill the
reconciliation sharpens (D-9):

| # | Property / failure mode | Drill (quantified gate) | Status |
|---|---|---|---|
| D-1 | **Backlink leak** (see a reference from an artifact you can't read) | zero-escape: 0 unauthorized backlinks/traverse, incl. under zookie staleness + filter-mode | CONFIRMED |
| D-2 | **Cross-tenant edge / IDOR** | 0 cross-tenant edge readable; `tenant`-predicate enforced | CONFIRMED |
| D-3 | **Hot-artifact backlink falls over** | "referenced-by-50,000" under concurrent filtered reads: p99 within budget; hot-fanout telemetry fires; R4 serves post-promotion | CONFIRMED |
| D-4 | **Index drift / unrecoverable graph** | reindex-from-cold parity: rebuilt == live (byte-match); a TE-7 drift reconverges to the typed table | CONFIRMED |
| D-5 | **Erasure leaves a dangling/leaking edge** | erase a subject + a referenced artifact: tombstones present, person unresolvable, 0 recoverable PII in edge/cache/backups; no 500 on resolve | CONFIRMED |
| D-6 | **Stale re-grant via backlinks ("new enemy")** | revoke + immediately re-read with the post-revoke zookie: no stale allow (bypasses fail-static) | CONFIRMED |
| D-7 | **Edge loss / no-ghost (dual-write)** | crash a producer between the content/relation commit and the relay publish: edge still delivered (outbox); never an edge without its content. 0 ghost, 0 lost | CONFIRMED |
| D-8 | **Traversal cycle / unbounded walk** | a cycle + a 1000-deep chain: CTE terminates (visited-set + depth ceiling), cycle reported as a diagnostic, statement timeout respected | CONFIRMED |
| **D-9** | **Sub-artifact tombstone (the unified ladder)** | delete a doc block / PR comment / chat message / make a Git line-range outdated that others embed; assert each degrades through the frozen ladder to the **correct state** (`moved`/`outdated`/`sub_gone`) with the **root carried** — **0 dangling embed, 0 hard 404, no leak** | **SHARPENED** — now asserts the frozen `live/moved/outdated/gone` ladder (§4.6, C-2) across all three content shapes + Git's content-anchored states |
| D-10 | **30× agent-surge on the graph** | 30× agent reference-creation + backlink-read surge on one tenant: human read lane holds (protected lane); agent lane sheds (429+Retry-After honoured); other tenants unaffected | CONFIRMED (per OQ-K shed-budget floors) |

Each drill asserts against the §5.1 telemetry (observability is part of the pass condition — T-1). The
substrate exposes the scoped, reversible failure-injection seams (T-3). These drills are Phase-6 deliverables.

---

## 8. Required changes to foundational systems — now SATISFIED by the reconciliation

Phase-3 §8 named four required changes; the reconciliation **resolves the two that were open**:

1. **Identity — `list_objects` `Filter` composable over the edge's `source` column.** Phase 3 flagged this as
   "usage confirmation, must be named in reconciliation." **Now satisfied:** OQ-E froze the `SetExpr` as a
   consumer-composable set algebra lowered over an arbitrary consumer id column (contract 4.3); Refs lowers it
   over `source_root` (§4.4). **No Id signature change** — Refs is one of the five named consumers.
2. **Event Bus — `refs-edge-builder` is an explicitly reviewed firehose-class consumer.** CONFIRMED unchanged
   (BUS-4; the bus's reviewed-firehose-consumer list includes it). No new transport.
3. **Every subsystem — the `project(ref, viewer)` API + `replay(scope, since)`.** CONFIRMED (contract 5.6 +
   2.6); the shape is frozen `{title, state, icon, render_hint, sub_anchor?}`. The reconciliation adds that the
   `sub_anchor` resolver returns the frozen `live/moved/outdated/gone` state (C-2). Subsystem deliverable.
4. **Issues + Knowledge — own the TE-7 typed relation tables + emit typed lifecycle events.** CONFIRMED
   (contract 5.5; REF-1/ISS-1). Refs fixes the `rel` vocabulary + mirror discipline + inverse pairing; the
   subsystems own the rows, guards, and the ADR-06 relation field type.

No change is required to the substrate, the envelope, the outbox, the consumer template, fail-static, or the
migration discipline — Refs remains a **conforming consumer** of all of them.

---

## 9. Open questions for Phase 6

The reconciliation **closed** the Phase-3 `[OPEN → P4]` items that were Refs' (the `#sub` mint grammar → C-1;
the `list_objects` push-down encoding → C-4; the Issues key-vs-display reconciliation → C-3). What remains:

- **[OPEN → P6, control-plane + Refs]** **Cross-cell backlink fan-out (build).** The *resolution semantics* are
  frozen cell-local (C-5, §4.2); the *fan-out build* for multi-cell tenants is the named floor (§6.5),
  co-owned with the bus's cross-cell pointer bridge (contract 12.6) and SC-2/SC-3. The deepest remaining Refs
  unknown; the contracts are cell-agnostic so it extends without a rewrite. (Gap report E-3: single-home-cell →
  multi-cell.)
- **[OPEN → P6, Legal + product]** **Cross-tenant *inbound* reference visibility policy** (§6.4) — *when* a
  public-OSS inbound ref is shown to the target tenant. The **mechanism** (a `public` userset, never a
  cross-tenant join) is DECIDED; the **policy** is product/legal (ties to identity-and-access §15 and the
  EU-sovereignty posture). The structural floor (no cross-tenant leak) ships regardless.
- **[OPEN → P6, measured]** **The drill thresholds:** the hot-fanout read budget that triggers R4 promotion;
  the traverse depth ceiling (proposed 16); the surge multiplier; the index-lag alarm; the R2 cache TTL. All
  **measured-not-predicted** (EI-02 §8); this doc proposes the defaults-to-beat.
- **[FLOOR → P6]** **The Leopard-style hot-reach index (R4)** is specified-not-built; promotion trigger =
  measured hot-fanout exceeding the read budget (R5). Tracked in the gap report (E-3).
- **[OPEN → P6, subsystem]** **Each subsystem's stable `#sub` mint** must satisfy the stability obligation
  (§3.5): a block id survives moves, a message/comment id is immutable, a Git line-range carries the BLAKE3
  fingerprint. Refs froze the *grammar + ladder* (C-1/C-2); the *stability guarantee* is each subsystem's P6
  deliverable, asserted by drill D-9.

No `[OPEN — LEGAL]` item is Refs-owned: Refs holds only pseudonymous opaque ids (never third-party free-text
bodies), so the platform free-text/immutable erasure posture (contract 10.9, X-7) is instantiated in Refs **by
reference** (§4.6) — Refs adds no new residual.

---

## 10. Cited prior art — CONFIRMED unchanged (Phase-3 §10)

Unchanged. Graph-in-RDBMS / adjacency list / recursive CTE: Celko (*Trees and Hierarchies in SQL*, 2012),
Karwin (*SQL Antipatterns*, 2010, closure-table remedy), ISO SQL:1999 `WITH RECURSIVE` / SQL:2023 `CYCLE`,
PostgreSQL §7.8. Log-as-truth + reindex-from-source: Kreps (*The Log*, 2013), Kleppmann (*DDIA*, 2017, ch. 11
+ ch. 5 tombstones), EI-04 §5.3. Permission-filtered set reads + hot fan-in + consistency tokens: Pang et al.
(*Zanzibar*, USENIX ATC 2019 — `check`/`expand`, the **Leopard** index §3.2.1, **zookies** §2.4.4),
SpiceDB/OpenFGA as the EU-self-hostable implementations Refs consumes via Id. Addressing / the associative
trail: Git object model, RFC 8141 (URN syntax), Bush (*As We May Think*, 1945), Nelson/Xanadu, Roam/Obsidian.
Doctrine: EI-02 §7/§1/§8/§10; EI-04 §1/§5.3; decision-record §(c) D11/TE-7, §(e); directives REF-1..REF-4.

---

## 11. Cross-references

- [`VISION.md`](../../VISION.md) §1 — the reference graph as the connective tissue / core moat.
- [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §7 (binding
  doctrine), §1/§8/§10; [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
  §1 (erasure vs immutability), §5.3 (reindex-from-source).
- [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) — X-4/OQ-D (C-1/C-2/C-6), recon §3
  (C-3), OQ-E (C-4), OQ-I (C-5), X-7 (the erasure posture instantiated by reference).
- [`contract-index.md`](./contract-index.md) — the frozen surface (5.1, 5.2, 5.3, 5.4, 5.5, 5.6, **5.7**; the
  consumed 4.3 `SetExpr`, 12.6 cross-cell bridge).
- [`../03-shared-systems-architecture/reference-graph.md`](../03-shared-systems-architecture/reference-graph.md)
  — the Phase-3 base this refines (carried forward; unchanged sections cited, not restated).
- Spine: ADR-13 (the three glue contracts; TE-7), ADR-14 (PG-by-default), ADR-03 (`list_objects`), ADR-04
  (events authoritative), ADR-05 (the `mention`/`artifact_ref`/`embed` nodes produce edges), ADR-06 (relation
  field type → typed tables), ADR-11 (cells), ADR-12 (PersonalDataHolder). Directives: REF-1..REF-4, X-1, ID-3,
  GD-3.
