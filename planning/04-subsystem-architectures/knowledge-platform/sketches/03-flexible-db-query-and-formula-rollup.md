# Sketch 03 — Flexible-DB query model (TE-17) + formula/rollup engine (TE-18)

> Phase 4, Knowledge, **exploration**. Canonical: KN-3 (property bag per row; rollups/formulas at
> READ TIME, never stored; materialise only when measured too slow), ADR-06 (shared field/view/AST
> primitive, engines subsystem-owned), ADR-07 (one query AST), EI-04 §2.5, decision-record §(d).2.
> I **co-own the field-definition/view primitive (ADR-06) with Issues**; the *query execution* and
> the *formula/rollup engine* are Knowledge-owned (the "JQL performance trap" is mine, not shared).

---

## 0. What KN-3 already fixes (the floor-first inversion)

KN-3 / EI-04 §2.5 / decision-record §(d).2 commit, **inverting the Phase-2 materialised-first lean**:

- **In-document databases are a property bag per row.**
- **Rollups and formulas are computed at READ TIME, never stored.**
- **Materialise only when read-time recompute is *measured* too slow** (R5 named promotion trigger,
  not a vague "v2").

So the engine question is not "build a stored incremental dataflow engine" (the Phase-2 lean) — it is
"build a read-time evaluator + a JSONB query model, and add a derived projection / materialisation
*only where measured*." This is the doctrine's own "don't add the engine before the volume is
measured" applied to formulas.

---

## 1. Flexible-DB query model (TE-17) — candidates

A database = a schema of typed properties (columns) + rows; each row conforms to the schema; views
are query-AST projections. The hard part: schemaless flexibility *and* fast filter/sort/group/aggregate
at multi-tenant world scale, without per-tenant DDL.

### Candidate A — JSONB property bag + derived indexable projection (the KN-3 model)

Row = `db_row(tenant, db_id, row_id, props jsonb, …)` where `props` is `{ field_id: typed_value }`.
Querying/filter/sort/group is over `props` with **expression/GIN indexes and generated columns** for
the hot fields, maintained as a **derived projection off the bus** (the deep-dive §2.4 "JSONB
source-of-truth + derived indexable projection" middle path).

- **Pro**: trivially flexible (add/remove a field = no DDL, just a schema-def row + a `props` key).
  No per-tenant table sprawl (the multi-tenancy-killer of materialised-table-per-db). The query AST
  (ADR-07) compiles to a `WHERE props @> …` / generated-column predicate, **always conjoined with
  Id's `list_objects` filter** (permission-aware by construction, ADR-03/07). Generated columns /
  partial indexes for the *measured-hot* filter/sort fields give native-ish speed where it matters.
- **Con**: the "JQL performance trap" — naive `props->>'x'` filter/sort over millions of rows without
  an index is slow. Mitigation: **generated columns + expression indexes for declared-sortable/
  filterable fields**, and **push heavy structured queries to the Search structured index** (Tantivy
  fast-fields, `search-and-indexing.md` §2.2/§3.1) which is *already* the platform's permission-aware
  structured-query surface fed off the bus. So a big DB view = a Search structured query (ACL-pre-
  filtered, paginated), not an OLTP scan. This is the deep-dive §2.9 "structured query over db
  properties" routed to the shared Search tier.

### Candidate B — Per-database materialised SQL table (DDL per db)

Each database is a real table; adding a property is `ALTER TABLE`.

- **Pro**: native query speed + constraints.
- **Con**: **DDL-per-tenant-database at world scale is operationally brutal** (millions of tables;
  the forward-only/no-blocking-`ALTER` discipline, STOR-2, fights per-row-add DDL) and fights
  multi-tenancy (the deep-dive §2.4 + Phase-2 explicit rejection). **Rejected** — this is exactly the
  pattern KN-3 inverts away from.

### Candidate C — External columnar query store (ClickHouse) as the primary db store

Put db rows in the OLAP tier.

- **Pro**: fast analytic scans.
- **Con**: ClickHouse is the **derived OLAP read model fed off the bus** (`storage.md` §3.4), *not* a
  transactional source of truth, and it's a cross-tenant analytics store — wrong tier for the
  authoritative, transactional, per-tenant row state (inline edit, two-way relations need
  transactional writes). **Rejected as the source of truth**; it remains the home for *cross-db
  analytics/dashboards* (a measured-promotion, R5).

### Query-model leaning

**Candidate A: JSONB property bag as source of truth (OLTP, transactional, per-tenant) + a derived
indexable projection — generated columns/expression indexes for measured-hot fields locally, and the
shared Search structured index (off the bus) as the scalable filter/sort/group surface for large
views.** This is the KN-3 model, with the scaling answer being "route the heavy structured query to
the platform's existing permission-aware structured index" rather than building a second query engine.
Materialised-table-per-db (B) and ClickHouse-as-truth (C) are rejected for the source of truth;
ClickHouse stays the *measured-promotion* cross-db analytics home.

**The ADR-06 co-ownership line with Issues**: the *field-definition system* (types: text/number/
select/multi-select/date/person/relation/formula/rollup, + per-field personal-data classification)
and the *view abstraction* (table/board/calendar/timeline as query-AST + grouping + sort +
visible-fields) live in **`myelin-query` (shared, co-owned)**. The *query execution over flexible
fields* (TE-17) and the *formula/rollup engine* (TE-18) are **Knowledge-owned**; Issues owns its own
execution + workflow/SLA engine. We share the schema language + view model + AST, not the planner —
exactly ADR-06's line. The relation field type rides Refs where cross-artifact (TE-7), or the local
`db_relation` typed table where intra-collection (which I own, per the Phase-3 handoff).

---

## 2. Formula/rollup engine (TE-18) — read-time-first

### The KN-3 decision: read-time evaluation, not stored dataflow

A `formula` field is an expression over other properties; a `rollup` aggregates over a `relation`.
**Computed at read time, never stored** (KN-3). So:

- A view that displays a formula/rollup column **evaluates it as part of the read**, over the rows the
  query returns (already ACL-pre-filtered + paginated, so the working set is bounded — you never
  evaluate a formula over 10,000 hidden rows).
- The formula expression language is the **query AST predicate/expression core** (ADR-07 / the same
  `myelin-query` evaluator) — *one* safe, non-Turing-complete, statically-cost-bounded evaluator
  (AG-7 / substrate §7.5), so a crafted formula cannot DoS (no UDFs, no loops, no unbounded recursion).
  This reuses the bus's `EventMatcher` discipline — one evaluation engine, one DoS-hardening surface.
- **Cycle detection** is structural: the formula dependency graph (field A references field B) is a
  DAG checked at *schema-definition* time (you cannot save a formula that creates a cycle), so
  read-time evaluation is always a bounded topological walk. Cross-relation rollups are
  depth-bounded (a rollup over a relation is one hop; nested rollups are depth-capped like the Refs
  traversal, `reference-graph.md` §4.5).

### Why read-time, not the Phase-2 async-incremental-dataflow lean

- **No stored derived state to keep consistent** → no incremental-recompute fan-out cascade (the
  Notion scaling pain point, deep-dive §5/§2.4), no "edit one cell cascades recompute across many
  rows" storm, no eventual-consistency-of-rollups correctness bugs. The value is *always* correct
  because it's computed from current inputs on read.
- The working set is **bounded by the ACL-pre-filtered, paginated view** (you only evaluate formulas
  for the rows on screen), so read-time cost is bounded by page size, not table size.
- It's the **honest floor** the doctrine wants: simpler, correct, named, with a measured promotion.

### The promotion trigger (R5, named): measured read-time-too-slow

**When** a rollup/formula over a *large related set* (e.g. a rollup summing a field across 50,000
related rows, where pagination can't bound it because the aggregate spans the whole set) is **measured**
to exceed the read budget, *that specific rollup* is promoted to a **materialised incremental
aggregate**: a derived value maintained off the bus (the consumer template, `row.updated` → update the
aggregate), stored, and read directly. This is the deep-dive §2.4 "incremental aggregation, not
recompute-on-read at scale" — but **only for the measured-hot aggregate**, not the whole engine. The
two-way relation inverse-link maintenance is the same: best-effort eventual consistency via events in
the floor (KN-3 / EI-04 §2.5 "expect relation columns to need careful, initially best-effort
bidirectional consistency"), materialised where measured.

## 3. Views (the shared ADR-06 view abstraction, Knowledge-rendered)

A view = `{ query_ast, group_by, sort, visible_fields, view_type }`. View types: table / board /
calendar / list / gallery / timeline (the shared `§5.6` views component, one component for both
Issues and Knowledge). Per-view filter/sort/group; **shared-vs-personal split** (a shared view def +
optional per-user overrides layered on top — the recurring platform pattern, deep-dive §2.4). A view
is **permission-aware by construction** (ADR-03/07): rows the viewer can't see are simply *absent*
(pre-filtered via `list_objects`), never post-filtered (the §5.6 / system-overview §5.2 invariant) —
so "permission-filtered" is a structural property of the view query, not a UI step.

## 4. What this sketch commits to the findings

- **TE-17**: JSONB property bag per row as the transactional source of truth (OLTP, per-tenant, no
  per-db DDL) + a derived indexable projection (generated columns/expression indexes locally for
  measured-hot fields; the shared Search structured index off the bus for large views). Materialised-
  table-per-db rejected; ClickHouse stays the measured-promotion cross-db analytics home.
- **TE-18**: formulas/rollups computed at **read time** over the ACL-pre-filtered, paginated working
  set, using the one safe `myelin-query` evaluator (no UDFs/loops/recursion; cycle-checked at schema-
  def time). **No stored dataflow engine in v1.** Promotion (R5, named): a *specific* measured-too-slow
  rollup over a large related set → a materialised incremental aggregate maintained off the bus.
- **ADR-06 co-ownership**: field-definition system + view abstraction + AST shared in `myelin-query`
  (co-owned with Issues); query execution over flexible fields (TE-17) + the formula/rollup engine
  (TE-18) are Knowledge-owned. The `db_relation` typed table is mine (Phase-3 handoff); cross-artifact
  relations ride Refs (TE-7).

## Cited prior art

- JSONB property bag + GIN/generated columns: PostgreSQL `jsonb`/GIN docs; the EAV-vs-JSONB-vs-
  materialised trade-off (deep-dive §2.4); Karwin, *SQL Antipatterns* (the EAV trap).
- Read-time computed vs stored: KN-3 / EI-04 §2.5 (rollups/formulas at read time, never stored);
  spreadsheet dependency-graph / incremental computation as the *promotion* model (differential
  dataflow / incremental view maintenance literature — the named follow-on, not v1).
- Safe expression evaluation: ADR-07 / AG-7 (one non-Turing-complete AST evaluator); CEL's
  cost-bounded-total-function discipline borrowed (bus §4.5).
- Permission-aware views: ADR-03 `list_objects` pre-filter; Zanzibar (Pang et al., ATC 2019).
