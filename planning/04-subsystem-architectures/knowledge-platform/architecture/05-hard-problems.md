# Knowledge Platform — 05 · Hard Problems (resolved, with cited prior art)

> See [`00-overview.md`](./00-overview.md) for framing. This doc resolves each subsystem-specific hard problem,
> citing prior art and naming the floor where v1 is partial (VISION §3). Mechanisms are in
> [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md); this doc is the decision + the
> literature + the floor. Each carries its drill ([`07`](./07-drills-and-open-questions.md)). Where a problem
> touches a now-FROZEN contract, the resolution is stated against that contract (no drift).

---

## 1. CRDT vs OT + granularity (TE-15) — CAS floor → Yrs CRDT; resume-cursor transport FIRST

**Decision: a CRDT (leading implementation Yrs, Rust Yjs) is the committed eventual engine; the v1 floor is
per-block optimistic compare-and-swap (CAS); the resume-cursor durable transport is item 0, built before
either** (KN-1; EI-04 §2), now over the **frozen `firehose::subscribe/resume(scope=doc:<id>)` protocol**
(contract 3.5). Granularity: **hybrid — a per-block content CRDT + a tree/move CRDT for block structure.**

**The cited prior art:**

- **OT family** — Operational Transformation powers Google Docs / `prosemirror-collab`; origin Google Wave OT
  and Ellis & Gibbs, *Concurrency Control in Groupware Systems* (SIGMOD 1989). **Rejected as the engine** —
  transform functions are notoriously hard for rich *trees* and effectively require a central authoritative
  transform server (weaker offline/scaling).
- **CRDT family** — **Logoot** (Weiss et al., 2009), **RGA** (Roh et al., JPDC 2011) are the foundational
  sequence CRDTs; **Yjs/Yrs** (Kevin Jahns) is the production-grade Rust-native implementation; **Automerge**
  the columnar peer; **Fugue** (Weidner et al., 2023) fixes the interleaving anomaly under concurrent
  same-position insert; **Peritext** (Litt et al., 2022) is the reference for CRDT rich-text marks;
  **Kleppmann's move operation** (*A highly-available move operation for replicated trees*, 2021) handles
  concurrent re-parenting without cycles. **Chosen** — Yrs is Rust-native (reinforces ADR-02), the server is a
  "dumb relay + persistence," offline-first aligns with UX.
- **The transport-first doctrine** — EI-04 §2.2: "build the durable, resume-cursor real-time transport *first*
  … because the CRDT slots into that transport." We honor this literally: the resume-cursor transport (over the
  frozen OQ-J protocol) with idempotent apply (Helland, 2012) and a durable op-log (Kreps, *The Log*, 2013) is
  item 0.

**The CAS floor (EI-04 §2.1):** per-block optimistic compare-and-swap; on a precondition miss the loser is
rejected and handed the current server state to reconcile. **Guarantees no *silent* overwrite — but does not
merge.** Shipped *named as a floor*, layered with advisory soft-locks + snapshot/restore. **The named promotion
(R5): the first true concurrent-edit conflict** triggers Yrs, slotted into the transport as a Layer-3 swap (the
op-log carries Yrs update bytes; the transport is unchanged), migrated per-doc online via the `engine_promote`
op ([02 §3.4](./02-internals-and-algorithms.md)). **Drill:** KD-1 (reconnect-loses-zero-ops, re-run across an
`engine_promote` boundary) + KD-2 (round-trip).

---

## 2. Block-tree storage (TE-16) — adjacency list + the FROZEN LexoRank; markdown-subset inline

**Decision: per-block rows in an adjacency list (`parent_id` + the frozen `order_key`), inline content as a
markdown-subset string with `mention`/`artifact_ref`/`embed` as structured nodes** ([01 §2](./01-tech-and-data-model.md);
TE-16, KN-2 / contract 13.1).

**Cited prior art:**

- **Adjacency list vs nested set vs closure table** — Celko, *Trees and Hierarchies in SQL* (2012); the
  adjacency-list "index the column you query" discipline. Per-block rows scale to huge docs and enable
  block-level references/permissions; a single document blob caps doc size.
- **Fractional indexing for concurrent ordering** — the **frozen LexoRank** (contract 13.3, X-3): base-62
  `0-9 A-Z a-z`, `"U"` first, midpoint bisection, **2-char jitter**, **48-char rebalance**, ULID tiebreak —
  **byte-identical with Issues' drag-rank**, so a future shared CRDT/render path treats ordering uniformly. Its
  interleaving/precision pitfalls under heavy concurrency are bounded by the jitter (no concurrent-same-gap
  collision) and resolved natively by the CRDT's list type / Fugue (§1).
- **Markdown-subset inline string** — EI-04 §2.4 / EI-05 §2 / KN-2 / contract 13.1: store inline as a
  markdown-subset *string* (not inline-range JSON) — survives copy/paste, export, diff, reference-extraction;
  needs no server sanitisation; survives an editor rewrite with zero schema migration. The three structured
  ref nodes are kept *out* of the string (the `U+FFFC` positional anchor, [01 §2.2](./01-tech-and-data-model.md))
  so reference-extraction is a node-array walk.

**No floor here** — committed (the CRDT lands *over* it, §1).

---

## 3. Flexible-DB query model (TE-17) — JSONB property bag + derived projection

**Decision: a JSONB property bag per row as the source of truth + a derived, indexable projection (GIN +
per-measured-hot-facet generated/expression-column indexes maintained off the bus), NOT per-database
materialised SQL tables** ([01 §4](./01-tech-and-data-model.md), [02 §4.1](./02-internals-and-algorithms.md);
TE-17), queried over the **frozen `myelin-query` shapes** with the **frozen `SetExpr` push-down** conjoined.

**Cited prior art:**

- **EAV / JSONB property bag vs per-database materialised table** — Karwin, *SQL Antipatterns* (2010); the
  pragmatic Postgres `jsonb` + GIN + generated-columns answer. A real SQL table per user-defined database means
  DDL-per-tenant-database at world scale — operationally heavy, fights multi-tenancy.
- **The "JQL performance trap"** (TE-17): schema-flexibility *and* fast filter/sort/group/aggregate at scale.
  The committed answer: JSONB source of truth + a derived indexable projection **provisioned off the bus** when
  a field crosses the **frozen measured-promotion threshold** (contract 6.3 / OQ-C: a facet in `> 5%` of a
  collection's view executions over a rolling window) — not blanket DDL.

**The named floor (KN-3-style measured promotion):** the bulk stays JSONB read-time; a specific facet/rollup
over a large set is materialised (the OLAP read store, contract 11.6) **only when measured too slow**. **Drill:**
KD-9 (flexible-DB query latency at scale).

---

## 4. Formula / rollup engine (TE-18) — READ-TIME, never stored (by contract)

**Decision: formulas and rollups are computed at READ TIME, never stored; materialise a specific rollup only
when read-time recompute is measured too slow** ([02 §4.2](./02-internals-and-algorithms.md); TE-18, KN-3). This
is now the **frozen contract** (13.3: `rollup`/`formula` are "computed at READ TIME, never stored").

**Cited prior art + reasoning:**

- **Incremental computation / dataflow (the spreadsheet model)** — computed properties form a dependency graph;
  editing one cell can cascade recomputation across many rows (a known Notion scaling pain). The doctrine
  inverts the materialised-first lean: read-time-only, never stored; materialise only when measured too slow
  (EI-04 §2.5; KN-3).
- **Bounded, cycle-safe evaluation** — the dependency graph is walked depth-bounded with a visited-set; a cycle
  surfaces as `#CYCLE`, never an infinite loop. The `FormulaAst` is the bounded `myelin-query` expression core
  (the same cost-bounded discipline as the bus `EventMatcher`, contract 3.4) — no UDFs, no loops, statically
  cost-bounded — so a crafted formula cannot DoS a render.
- **Eventual consistency, stated** — a rollup reflects related rows as of the read; cross-DB relation
  propagation is eventual (the Refs inverse-edge projection lags the typed table).

**The named floor:** read-time is v1; **per-rollup measured materialisation** (incrementally-maintained
aggregate fed off `knowledge.row.updated` deltas) is the promotion-triggered follow-on. **Drill:** KD-10
(read-time rollup latency).

---

## 5. Permission granularity (page / row / field) — DECIDED (over the frozen shapes)

**Decision: page/database-level (full v1, the page-tree inherit-with-overrides fragment) + row-level (v1, via a
`row_reader` userset relation pushed down by `InRelation`) + field-level (v1, the frozen `CaveatContext` caveat
at `check`-time, off the hot path)** ([01 §5](./01-tech-and-data-model.md)).

**Cited prior art:** Zanzibar (Pang et al., USENIX ATC 2019) usersets — union/intersection/exclusion +
tuple-to-userset rewrite; the page-tree inheritance-with-overrides pattern is
`page.read = (parent_page->read + direct_reader) - direct_block` (the exclusion userset makes a narrowed
sub-page disappear from `list_objects` by construction). Field-level ("hide salary") is the frozen
`CaveatContext{object, field, attrs}` caveat (contract 4.2; SpiceDB caveats / OpenFGA conditions; NIST SP
800-162 ABAC) on `view_field`, evaluated at `check` time — **kept off the hot `list_objects` path** so the bulk
pre-filter stays fast (OQ-E). Row-level lowers to `InRelation { relation: row_reader, via_column: db_row.id }` —
a JOIN against the per-tenant authz reverse index (OQ-E), not a per-row check.

**No floor on page/row level; field-level is full but the per-database predicate catalogue is a Phase-6 detail**
(KQ-5, co-designed with Id's role-bundle catalogue).

---

## 6. GDPR erasure from immutable history + free-text PII — structural floor + the platform posture (Δ8)

**Decision: per-subject crypto-shred + pseudonym-map shred + tombstoning is the fully-built structural floor;
structured PII reliable, residual third-party/immutable free-text handled per the ONE platform-wide erasure
posture by reference** ([03 §6](./03-events-contracts-and-glue.md); contract 10.9 / X-7 / OQ-G).

**Cited prior art:**

- **Erasure vs immutability** — EI-04 §1: "delete the identity, not the fact." Tombstoning / pseudonymisation
  (Kleppmann, *DDIA* ch. 5, 2017); attribution by stable opaque `principal_id`, the person in Id's erasable
  pseudonym map (grammar `<pseudonym>@<tenant>.noreply`, contract 4.8).
- **Crypto-shred from append-only logs** — you cannot delete a merge-dependent CRDT/CAS op; encrypt PII-bearing
  ops/blocks under a **per-subject DEK** (contract 11.4, `<class> = subject:<id>`) and **destroy the key** →
  the ciphertext in the op-log, snapshots, and backups becomes unrecoverable (Boneh & Lipton, *A Revocable
  Backup System*, 1996; NIST SP 800-88r1 media-sanitisation framing). One DEK per (subject, tenant), applied
  selectively (CR-I) — key count O(subjects with inline PII), not O(blocks).
- **Search/embedding lockstep** — embeddings of personal data are personal data; the `knowledge.*.erased` event
  purges + re-indexes including vectors. HYOK `can_derive_plaintext_index()=false` structurally skips indexing
  (contract 11.3).

**The residual (the platform posture, NOT a Knowledge-specific write-up):** third-party free-text PII (a name
typed by someone else into that other person's content) is encrypted under the *author's* DEK, so the subject's
erasure does not crypto-shred it. Per the **ONE platform-wide posture (contract 10.9, X-7)**: handled under a
documented lawful-basis limit — best-effort `rectify`/tombstone of the specific span where the subject
identifies it, plus the standing structural guarantee that the residual is never indexed/agent-readable/in
analytics for a restricted subject. `[OPEN — LEGAL]`: counsel/DPO ratify the residual basis in **one
statement** (10.9). The structural floor ships regardless. **Drill:** KD-4.

---

## 7. Synced blocks / transclusion — a read-projection FLOOR (the node is in the taxonomy; the engine is floored — Δ3)

**Decision: `sync_block { source: ArtifactRef }` is a node type IN the frozen `myelin-content` taxonomy
(contract 13.1); v1 renders it as a live read-projection (like `embed`), NOT editable-in-place multi-home.**

**Reasoning:** the frozen taxonomy includes `sync_block` (Knowledge-only transclusion), so the node must exist
for the AST to be complete (X-2). But the *hard* part of transclusion — a block with one canonical home and
many *edit* sites — breaks the pure-tree assumption and complicates permissions, erasure, and reference-
counting. The v1 engine therefore renders `sync_block` by resolving `source` via Refs `resolve(ref, viewer)`
(contract 5.2), permission-filtered per viewer, with the 4-step tombstone ladder on loss — delivering
transclusion's *read* value without the shared-mutable-node complexity. **Named follow-on:** editable-in-place
synced blocks designed against the CRDT (which makes the shared-mutable-node merge tractable), with
most-restrictive-of-sites permission + reference-counted erasure via the edge index (KQ-6).

> **Δ3 reconciliation note:** Phase 4 deferred synced blocks *entirely*; the reconciled taxonomy makes the node
> type frozen, so v1 ships the **node + a read-projection engine** (floor) rather than nothing. The editable
> multi-home engine remains the named CRDT-era follow-on.

---

## 8. Offline depth — read + queued light-edit (floor) → full offline-first (with the CRDT)

**Decision: v1 offline = read + queued optimistic light-edit reconciled via the CAS floor; full offline-first
arrives with the CRDT.** The CRDT is what makes deep offline *correct* (offline-first is its native strength).
On the CAS floor, an offline client queues optimistic edits and reconciles on reconnect via the resume-cursor
transport (`firehose::resume`, idempotent apply) + CAS (a conflict on a since-changed block is surfaced, not
silently merged). Deep multi-hour offline divergence is **not** correct on the floor — so v1 scopes offline to
read + light queued edit; the promotion to the CRDT (§1) is also the promotion to full offline-first.

---

## 9. Multi-region collab + EU residency — single-cell (floor), cross-cell designed-not-built (OQ-I)

**Decision: v1 pins a doc's authoritative collab session to the tenant's cell (residency by construction);
cross-cell collab for a multi-cell tenant is designed-not-built** (the cross-cell PII-free pointer bridge,
contract 12.6 / OQ-I; CR-H).

**Reasoning:** a collaboration session is latency-sensitive *and* stateful; pinning the authoritative collab
server for a doc to the tenant's region keeps **collab-session state residency-pinned** (a GDPR property, not
just latency). The op-stream never crosses a cell — a doc lives in exactly one cell; a user in another cell who
opens the doc is **routed to the doc's home cell** for the live session (the session follows the data). The
control plane carries only a **PII-free `CrossCellPointer{subject(opaque), type, correlation_id, home_cell}`**
(contract 12.6); resolution is always **cell-local** (the home cell renders + permission-checks via `project`;
only the projection crosses, never raw rows/PII). **What's deferred (the floor):** *simultaneous* low-latency
co-editing of one doc by users physically in two cells (true cross-cell op fan-out) — the named follow-on,
owned by control-plane / multi-cell tenancy (KQ-7). The contracts are cell-agnostic so this extends without a
rewrite.

---

## 10. Comment threading — KB-native v1 (one scheme, two stores); Chat-primitive consolidation named (OQ-L — Δ9)

**Decision: v1 ships two threading implementations over ONE shared sub-artifact + content + ref scheme;
consolidation onto the Chat threading primitive is the named follow-on** (OQ-L; CR-G).

**Reasoning (OQ-L):** both Chat threads and Knowledge comment threads use the **same `#thread-`/`#comment-`
`#sub` grammar** (X-4), the **same `myelin-content` AST** (X-2), and emit the **same `refs.edge.created`**
events (contract 5.4) — so a thread is addressable, referenceable, and renderable identically regardless of
host. v1 keeps them **separate stores** because their concurrency/transport profiles differ (Chat: the firehose
live tier; Knowledge: a comment on a CAS-guarded block, with a block-id + relative offset anchor that the CRDT
re-anchors once promoted). The *thread render component* is the **shared design-system thread primitive**
(CR-G), so Knowledge owns the anchor data, not a forked UI. **The named follow-on:** when document-anchored
comments need real-time multi-party presence (the trigger), promote them onto the Chat threading primitive +
the firehose resume-cursor transport (OQ-J) — because they already share `#sub` + content + refs, the promotion
swaps the store/transport, not the data model (the gap-report item "KB-native comments → Chat-threading
consolidation").

---

## 11. Summary — the floors and their named follow-ons (the gap report seed, E-3)

| Hard problem | v1 ships | Named follow-on / promotion trigger |
|---|---|---|
| TE-15 collab | resume-cursor transport (FIRST, frozen firehose protocol) + CAS floor (no merge) | Yrs CRDT — first true concurrent-edit conflict |
| TE-16 block tree | adjacency list + frozen LexoRank + markdown-subset inline | (committed; CRDT lands over it) |
| TE-17 flexible-DB | JSONB bag + derived projection + frozen `SetExpr` conjoin | per-facet materialisation — measured >5% / too slow |
| TE-18 formula/rollup | read-time, never stored (by contract) | per-rollup materialised aggregate — measured too slow |
| permission granularity | page + row (`InRelation`) + field (`CaveatContext`) | (committed; per-db predicate catalogue = P6) |
| GDPR free-text PII | structural floor (per-subject crypto-shred + pseudonym shred) reliable | residual per the platform posture 10.9 (counsel ratifies, `[OPEN — LEGAL]`) |
| synced blocks | the `sync_block` node + a read-projection engine (floor) | editable-in-place multi-home — post-v1, on the CRDT |
| offline depth | read + queued light-edit (CAS) | full offline-first — with the CRDT |
| multi-region collab | single-cell, residency-pinned + the PII-free pointer bridge frame | true cross-cell op fan-out — control-plane / multi-cell |
| comment threading | KB-native store, one shared scheme | Chat-threading primitive consolidation — real-time-presence trigger |

Continue to [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md).
