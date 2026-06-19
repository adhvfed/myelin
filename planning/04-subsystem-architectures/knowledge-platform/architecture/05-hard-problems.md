# Knowledge Platform — 05 · Hard Problems (resolved, with cited prior art)

> See [`00-overview.md`](./00-overview.md) for framing. This doc resolves each subsystem-specific hard
> problem the Phase-4 brief names, citing prior art for each and naming the floor where v1 is partial
> (VISION §3). Mechanisms are in [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md); this
> doc is the decision + the literature + the floor. Each carries its drill (see
> [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md)).

---

## 1. CRDT vs OT + granularity (TE-15) — CAS floor → Yrs CRDT; resume-cursor transport FIRST

**Decision: a CRDT (leading implementation Yrs, Rust Yjs) is the committed eventual engine; the v1 floor is
per-block optimistic compare-and-swap (CAS); the resume-cursor durable transport is item 0, built before
either** (KN-1; EI-04 §2). Granularity: **hybrid — a per-block content CRDT + a tree/move CRDT for block
structure** (deep-dive §6).

**The cited prior art (the literature the brief demands):**

- **OT family** — Operational Transformation powers Google Docs and `prosemirror-collab`; its origin is the
  **Google Wave OT** work and the foundational Ellis & Gibbs *Concurrency Control in Groupware Systems*
  (SIGMOD 1989). Verdict: **rejected as the engine** — transformation functions are notoriously hard to get
  correct for rich *trees* (each custom block type needs transform logic), and OT effectively requires a
  central authoritative transform server (a weaker offline/scaling story). We borrow nothing from it except
  the "central relay" fallback option, which we don't need.
- **CRDT family** — **Logoot** (Weiss et al., 2009) and **RGA** (Roh et al., *Replicated Abstract Data
  Types*, JPDC 2011) are the foundational sequence CRDTs; **Yjs/Yrs** (Kevin Jahns) is the
  production-grade, Rust-native modern implementation (efficient encoding, no per-char id overhead);
  **Automerge** is the columnar-encoded peer; **Fugue** (Weidner et al., 2023) fixes the interleaving
  anomaly that Logoot/RGA suffer under concurrent same-position insert; **Peritext** (Litt et al., 2022) is
  the reference research for **CRDT rich-text marks** (bold/italic across concurrent edits); **Kleppmann's
  move operation** (*A highly-available move operation for replicated trees*, 2021) handles concurrent
  re-parenting without cycles. Verdict: **chosen** — Yrs is Rust-native (reinforces ADR-02), the server is a
  "dumb relay + persistence" (scales horizontally, deep-dive §6), offline-first aligns with UX.
- **The transport-first doctrine** — EI-04 §2.2: "build the durable, resume-cursor real-time transport
  *first* … because the CRDT slots into that transport. A real-time relay *without* resume cursors is itself
  a floor that will silently lose the gap on a reconnect." We honor this literally:
  [02 §2](./02-internals-and-algorithms.md) — the resume-cursor transport with idempotent apply (Helland,
  *Idempotence Is Not a Medical Condition*, 2012) and a durable op-log (Kreps, *The Log*, 2013) is item 0.

**The CAS floor (EI-04 §2.1):** per-block optimistic compare-and-swap on a last-modified token; on a
precondition miss the loser is rejected and handed the current server state to reconcile. **Guarantees no
*silent* overwrite — but does not merge** (concurrent same-block editors get a conflict, not a blend).
Shipped *named as a floor*, layered with advisory soft-locks + snapshot/restore.

**The named promotion (R5 — a trigger, not "v2"): the first true concurrent-edit conflict** measured in
production triggers the Yrs CRDT, which slots into the transport from [02 §2](./02-internals-and-algorithms.md)
as a Layer-3 swap (the op-log carries Yrs update bytes; the transport is unchanged). **Drill:**
reconnect-loses-zero-ops (the transport, owed regardless of engine) + the editor round-trip gate (KN-4).

---

## 2. Block-tree storage (TE-16) — adjacency list + fractional ordering; markdown-subset inline (EI-04/EI-05)

**Decision: per-block rows in an adjacency list (`parent_id` + a fractional `order_key`), inline content as
a markdown-subset string with `mention`/`artifact_ref`/`embed` as structured nodes** ([01 §2](./01-tech-and-data-model.md);
TE-16, KN-2).

**Cited prior art:**

- **Adjacency list vs nested set vs closure table** — Celko, *Joe Celko's Trees and Hierarchies in SQL for
  Smarties* (2nd ed., 2012); the adjacency-list "index the column you query" discipline. Per-block rows scale
  to huge docs and enable block-level references/permissions; a single document blob caps doc size and
  couples permissions to whole docs (deep-dive §2.1). Notion historically used adjacency list with a
  `content: [ids]` ordering array.
- **Fractional indexing for concurrent ordering** — LexoRank / fractional keys (the family Issues uses for
  drag-rank, TE-19): a concurrent insert picks a key *between* siblings without renumbering. Its
  interleaving/precision pitfalls under heavy concurrency are bounded in the CAS floor and resolved natively
  by the CRDT's list type / Fugue (§1).
- **Markdown-subset inline string** — EI-04 §2.4 / EI-05 §2 / KN-2: store inline content as a markdown-subset
  *string*, not an inline-range JSON model — it survives copy/paste, export, diff, and reference-extraction,
  needs no server sanitisation, and survives an editor rewrite with zero schema migration. Reconciled with
  the `myelin-content` AST (ADR-05): **AST for block structure, markdown-subset string for inline runs**,
  with `mention`/`artifact_ref`/`embed` kept as **structured nodes** so reference-extraction is reliable
  (decision-record §(d) tension 3).

**No floor here** — this is a committed model. (The CRDT lands *over* it, §1.)

---

## 3. Flexible-DB query model (TE-17) — JSONB property bag + derived projection

**Decision: a JSONB property bag per row as the source of truth + a derived, indexable projection (GIN +
per-hot-facet generated/expression-column indexes maintained off the bus), NOT per-database materialised SQL
tables** ([01 §4](./01-tech-and-data-model.md), [02 §4.1](./02-internals-and-algorithms.md); TE-17).

**Cited prior art:**

- **EAV / JSONB property bag vs per-database materialised table** — Karwin, *SQL Antipatterns* (2010) on EAV
  trade-offs; the pragmatic Postgres `jsonb` + GIN + generated-columns answer (deep-dive §2.4). A real SQL
  table per user-defined database means **DDL-per-tenant-database at world scale** — operationally heavy,
  fights multi-tenancy.
- **The "JQL performance trap"** (TE-17, the dominant per-subsystem risk, ADR-06): schema-flexibility *and*
  fast filter/sort/group/aggregate at scale. The committed answer: JSONB source of truth + a derived
  indexable projection (generated/expression columns + GIN) **provisioned off the bus** when a field is
  *measured* to be frequently filtered/sorted (a `knowledge.database.schema.changed` event triggers an
  expand→backfill→contract index add, substrate §9) — not blanket DDL.

**The named floor (KN-3-style measured promotion):** the bulk stays JSONB read-time; a *specific* facet or
rollup over a large set is materialised (the OLAP read store, Storage §3.4) **only when measured too slow**.
Not solved by sharing — it is Knowledge's P4 problem (ADR-06). **Drill:** the flexible-DB query latency under
a large multi-tenant corpus (a performance gate, [07](./07-drills-and-open-questions.md)).

---

## 4. Formula / rollup engine (TE-18) — READ-TIME, never stored

**Decision: formulas and rollups are computed at READ TIME, never stored; materialise a specific rollup only
when read-time recompute is measured too slow** ([02 §4.2](./02-internals-and-algorithms.md); TE-18, KN-3).

**Cited prior art + reasoning:**

- **Incremental computation / dataflow (the spreadsheet model)** — computed properties form a dependency
  graph; editing one cell can cascade recomputation across many rows in other databases (deep-dive §2.4) — a
  known Notion scaling pain point. The doctrine inverts our earlier materialised-first lean: **read-time-only
  rollups/formulas, never stored; materialise only when read-time recompute is measured too slow** (EI-04
  §2.5; KN-3; decision-record §(c) D11/TE-18).
- **Bounded, cycle-safe evaluation** — the dependency graph is walked depth-bounded with a visited-set; a
  formula cycle surfaces as `#CYCLE` (a diagnostic cell), never an infinite loop (mirrors ADR-08 loop
  governance). The expression language is the **safe `myelin-query` predicate/expression core** (ADR-07;
  AG-7) — no UDFs, no loops, statically cost-bounded — so a crafted formula cannot DoS a render (substrate
  §7.5).
- **Eventual consistency, stated** (deep-dive §2.4): a rollup reflects related rows as of the read; cross-DB
  relation propagation is eventual (the Refs inverse-edge projection lags the typed table).

**The named floor:** read-time is v1; **per-rollup measured materialisation** (incrementally-maintained
aggregate fed off `knowledge.row.updated` deltas) is the promotion-triggered follow-on (R4/R5). **Drill:**
read-time rollup latency over a large related set (a measured-promotion trigger, [07](./07-drills-and-open-questions.md)).

---

## 5. Permission granularity (page / row / field) — DECIDED

**Decision: page/database-level (full v1) + row-level (v1, via a `database_row` ReBAC namespace +
`row_reader` relation) + field-level (v1, ABAC caveat at the edge, off the hot path)** ([01 §5](./01-tech-and-data-model.md)).

**Cited prior art:** Zanzibar (Pang et al., USENIX ATC 2019) usersets — union/intersection/exclusion +
tuple-to-userset rewrite (the four operators every visibility need reduces to, Id §5); the page-tree
inheritance-with-overrides pattern is `page.read = parent_page->read + direct_reader - direct_block` (the
exclusion userset makes a narrowed sub-page disappear from `list_objects` by construction). Field-level
("hide salary") is a **caveat** (SpiceDB caveats / OpenFGA conditions; NIST SP 800-162 ABAC) on a `field.view`
permission, evaluated at `check` time with context — **kept off the hot `list_objects` path** so the bulk
pre-filter stays fast (deep-dive §2.7; Id §9). Corporate buyers want row + field; the cost is contained by
expressing both as tuples/caveats the *existing* engine answers (no bespoke check path).

**No floor on page/row level; field-level is full but the predicate catalogue per database is a P5 detail.**

---

## 6. GDPR erasure from immutable history + free-text PII — committed mechanism + honest limit

**Decision: per-subject crypto-shred + pseudonymous attribution + tombstoning; structured PII reliable,
free-text best-effort + tooling** ([03 §6](./03-events-contracts-and-glue.md); deep-dive §8; KN-3 KMS
granularity = GD-4).

**Cited prior art:**

- **Erasure vs immutability** — EI-04 §1: "delete the identity, not the fact." Tombstoning / pseudonymisation
  (Kleppmann, *DDIA* ch. 5, 2017); attribution by stable opaque `principal_id`, the person in Id's erasable
  pseudonym map (Id §11).
- **Crypto-shred from append-only logs** — you cannot delete a merge-dependent CRDT/CAS op; encrypt
  PII-bearing ops/blocks under a **per-subject DEK** (GD-4 granularity rule, Storage §5.1) and **destroy the
  key** on erasure → the ciphertext in the op-log, snapshots, and backups becomes unrecoverable (Boneh &
  Lipton, *A Revocable Backup System*, 1996, the crypto-shred origin; NIST SP 800-88r1 media-sanitisation
  framing). Per-subject (not per-tenant) DEK is *exactly* the GD-4 resolution: free-text/profile content
  whose erasure must be *individual* (Storage §5.1).
- **Search/embedding lockstep** — embeddings of personal data are personal data (Search §4.8; EI-04 §1); the
  `knowledge.*.erased` event purges + re-indexes including vectors. No leak via search.

**The honest limitation (named floor, GD-6 `[OPEN → LEGAL]`):** full automated free-text PII detection is
**not perfectly solvable** (deep-dive §8). Knowledge is **reliable for structured personal references**
(person props, mentions, attribution) and provides **tooling + a documented process** (search, DSAR export,
flagged-content review) for free-text — stated, not over-promised. This is the named co-owned Knowledge/Legal
write-up; the residual limit on free-text is documented, not pretended away. **Drill:**
erasure-reaches-every-holder (structured PII purged, embeddings purged, per-subject key shredded → 0
recoverable structured PII; free-text covered by the residual-limit write-up).

---

## 7. Synced blocks / transclusion — DEFERRED (named floor)

**Decision: v1 ships embeds (a live `ArtifactRef` rendered inline) but NOT synced blocks (a block with one
canonical home and many edit sites).** Synced blocks break the pure-tree assumption (a block has one home but
many render sites) and complicate permissions, erasure, and reference-counting (deep-dive §2.1/Q11).

**Reasoning:** an embed is a *read* projection (resolve `project(ref, viewer)`), which is fully supported and
permission-correct. A synced block is a *shared mutable* node — it would require reference-counted erasure,
per-render-site permission reconciliation, and a tree model that admits a DAG. This is a genuine complexity
multiplier with a contained product loss. **Named follow-on:** synced blocks/transclusion as a scheduled
post-v1 feature, designed against the CRDT (which makes the shared-mutable-node merge tractable) — not on the
CAS floor.

---

## 8. Offline depth — read + queued light-edit (floor) → full offline-first (with the CRDT)

**Decision: v1 offline = read + queued optimistic light-edit reconciled via the CAS floor; full offline-first
arrives with the CRDT** (deep-dive Q9).

**Reasoning:** the CRDT is what makes deep offline *correct* (offline-first is its native strength — deep-dive
§6). On the CAS floor, an offline client queues optimistic edits and reconciles on reconnect via the
resume-cursor transport (idempotent apply) + CAS (a conflict on a since-changed block is surfaced, not
silently merged). This is honest: deep multi-hour offline divergence is **not** correct on the floor (CAS
does not merge), so v1 scopes offline to read + light queued edit, and the **promotion to the CRDT (§1) is
also the promotion to full offline-first.** Drives the same trigger as TE-15.

---

## 9. Multi-region collab + EU residency — single-cell (floor), cross-cell designed-not-built

**Decision: v1 pins a doc's authoritative collab session to the tenant's cell (residency by construction);
cross-cell collab for a multi-cell tenant is designed-not-built** (inherits the bus cross-cell floor,
event-bus §7.4; deep-dive Q14).

**Reasoning:** a collaboration session is latency-sensitive *and* stateful; pinning the authoritative collab
server for a doc to the tenant's region keeps **collab-session state residency-pinned** (a GDPR concern, not
just latency — deep-dive §8). Single-cell is complete; a 10,000-person org spanning cells (SC-2/SC-3) needs
cross-cell op propagation — the **named floor**: the control plane (PII-free, ADR-11.4) carries a minimal
pointer bridge between a tenant's cells; each cell resolves `ArtifactRef`s locally per viewer (event-bus
§7.4). **Follow-on owner: P5/control-plane + multi-cell tenancy (SC-2/SC-3).** The contracts are cell-agnostic
so this extends without a rewrite.

---

## 10. Summary — the floors and their named follow-ons (the gap report seed, E-3)

| Hard problem | v1 ships | Named follow-on / promotion trigger |
|---|---|---|
| TE-15 collab | resume-cursor transport (FIRST) + CAS floor (no merge) | Yrs CRDT — first true concurrent-edit conflict |
| TE-16 block tree | adjacency list + fractional key + markdown-subset inline | (committed; CRDT lands over it) |
| TE-17 flexible-DB | JSONB bag + derived projection | per-facet materialisation — measured too slow |
| TE-18 formula/rollup | read-time, never stored | per-rollup materialised aggregate — measured too slow |
| permission granularity | page + row + field (ABAC caveat) | (committed; predicate catalogue per db = P5) |
| GDPR free-text PII | structured reliable + per-subject crypto-shred | free-text = tooling + documented residual (GD-6, Legal) |
| synced blocks | embeds only | transclusion — post-v1, on the CRDT |
| offline depth | read + queued light-edit (CAS) | full offline-first — with the CRDT |
| multi-region collab | single-cell, residency-pinned | cross-cell — control-plane / SC-2/SC-3 |

Continue to [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md).
