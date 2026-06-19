# Sketch 03 — Flexible-field storage + query execution (TE-17, the JQL performance trap)

> Exploration note. Weighs the **dominant per-subsystem risk** (Phase-2 §11 Q3; deep-dive §6.7): arbitrary
> user-defined fields + a flexible query language = "the JQL performance trap." ADR-06 is explicit this is
> **NOT solved by sharing the field/view primitive** — the *physical storage + query execution* for flexible
> fields is tracker-owned. Leans; commit in `00-findings.md`. Co-read with Knowledge 01 §4 (it solved the
> *identical* problem for `db_row` — we should align, not diverge gratuitously).

## The trap, named (deep-dive §6.7; Karwin, *SQL Antipatterns*, 2010)

- **EAV (entity-attribute-value)** — one row per (issue, field, value). Maximally flexible; **query-hostile**:
  a filter on 3 fields is 3 self-joins; sorting by a custom field is a join + cast; it is the canonical SQL
  antipattern (Karwin ch. "Entity-Attribute-Value"). This is roughly what early Jira did and why JQL got slow.
- **Column-per-field** — a real SQL column per custom field. Fast to query; **does not scale**: thousands of
  field definitions across tenants → thousands of columns / **DDL-per-tenant** (deep-dive §3.4, §6.7) → migration
  pain, sparse tables, multi-tenancy hostility (Knowledge 01 §1.2 rejects this for the same reason).
- The flexible query language on top (`myelin-query` AST) makes either failure mode user-reachable at scale.

## The known-good answer (the hybrid) and the prior art

The "usual answer" (deep-dive §6.7; Phase-2 §3 table) and **exactly what Knowledge committed** (01 §4.1): a
**JSONB property bag as the source of truth + a derived indexable projection for the hot facets**, plus typed
columns for the always-present core. The prior art chain:

| Concern | Prior art | Lands |
|---|---|---|
| JSONB document column in an RDBMS, GIN-indexed | PostgreSQL JSONB + GIN (`jsonb_path_ops`); Chang et al. Bigtable (sparse column families, the conceptual ancestor) | the `props` column + `db_row_props_gin`-style index |
| EAV is an antipattern; prefer typed + JSON hybrid | Karwin, *SQL Antipatterns* (2010) | reject EAV; typed core + JSONB tail |
| Materialise hot facets, not everything; measure first | CQRS / read-model (Young; Fowler); KN-3 "compute at read time, materialise only when measured slow" | generated/expression indexes per hot facet, off the bus |
| Search-index projection for full-text + structured + cross-field | shared Search (Tantivy), `declare_indexable` (Search §5.3) | the issue projects into Search; complex/ad-hoc queries that exceed OLTP go to the index |

## Candidate A — JSONB-only property bag (everything flexible, including core)

Every field, including `state`/`assignee`/`priority`, lives in `props jsonb`.

- **For:** uniform, zero DDL ever, governance schemes and custom fields share one mechanism (sketch 02).
- **Against:** the **hot path** (board grouped by state, "my open issues sorted by priority") becomes a JSONB
  scan + GIN probe for fields that are *always* present and *always* queried — paying the flexibility tax on
  data that isn't flexible. Sketch 01/02's typed-core spine exists precisely to avoid this.

## Candidate B — Typed-core columns + JSONB tail + derived projection (the hybrid; align with Knowledge)

```sql
CREATE TABLE issue (
  tenant uuid, region text, id uuid,
  key text,                       -- human key ENG-1421 (typed; TE-14)
  type_id uuid,                   -- ranked type (sketch 01)
  state text, state_category state_cat,   -- typed core: the board/report hot path (sketch 02 invariant)
  priority smallint,              -- typed core
  assignee uuid, reporter uuid,   -- typed core (pseudonymous principal ids; erasure-safe)
  parent_id uuid,                 -- containment spine (also in issue_relation as the truth; sketch 01)
  rank text,                      -- LexoRank backlog order (sketch 06)
  created_at timestamptz, updated_at timestamptz, state_changed_at timestamptz,
  props jsonb NOT NULL DEFAULT '{}',  -- THE FLEXIBLE TAIL: custom fields (field_id → value)
  props_nodes jsonb,              -- structured ref/mention values kept OUT of free-text (REF-1 producer)
  contains_personal_data boolean, pii_key_ref text,
  PRIMARY KEY (tenant, id)
);
CREATE INDEX issue_board ON issue (tenant, project_id, state_category, rank);  -- the hot board scan = index range
CREATE INDEX issue_props_gin ON issue USING gin (props jsonb_path_ops);        -- custom-field filters
-- Per-hot-facet generated/expression index, provisioned off the bus when a custom field is filtered/sorted often:
--   CREATE INDEX issue_sev ON issue ((props->>'severity')) WHERE type_id = :bug;  (maintained by the projection feeder)
```

- **For:** the **typed core carries the hot path** (board/list/report) as indexed columns — Linear-fast for the
  90% (design-language P2). The **JSONB tail carries the long-tail custom fields** — zero DDL, governance-scheme
  and custom-field unified (sketch 02). The **derived projection** (generated/expression indexes on *measured*
  hot custom facets, maintained off the bus — never per-tenant DDL bloat) handles "this org filters on
  `severity` constantly." This is *byte-for-byte the shape Knowledge committed* (01 §4.1 `db_row`) — co-owning
  the primitive means we share the *storage discipline*, not just the field-type enum (ADR-06).
- **For:** **query execution** has a clean three-tier escalation, decided by the query planner (the tracker-owned
  AST→store compiler, Phase-2 §1.4):
  1. typed-core-only filter/sort → indexed OLTP scan (fast).
  2. custom-field filter on a *hot* facet → the generated/expression index.
  3. custom-field filter on a *cold* facet, or full-text, or cross-artifact, or huge result → **the Search
     index** (Tantivy, ACL-pre-filtered via `list_objects` — Search §4.2/§5.1). Search is the pressure-release
     valve that stops a cold ad-hoc JSONB scan from killing OLTP. This is the deep-dive §6.7 "search-index
     projection" leg made concrete.
- **For:** analytics (CFD, cycle-time, velocity over years) never touch this table — they hit the **OLAP read
  store** (CQRS, Storage §3.4; deep-dive §6.5) fed by the clean change-event stream. OLTP stays lean.
- **Against / cost:** maintaining the derived per-facet indexes off the bus (the "projection feeder") is moving
  parts — but it's the *measured-promotion* discipline (KN-3 / R5), not speculative. Until a facet is measured
  hot, the GIN index serves it. The feeder is shared conceptually with Knowledge's K7.

## Candidate C — Distributed-SQL (CockroachDB/Yugabyte) from day one

Skip the PG-hybrid; put it on a distributed-SQL engine so a single shard never bottlenecks.

- **Against:** premature (Phase-2 §3 / EI-02 §8 "measure before you shard; every engine is permanent cost").
  PG-with-tenant-sharding is the floor; distributed-SQL is the named follow-on **only if a single tenant's shard
  is measured to outgrow PG** (Phase-2 §3 "Distributed-SQL only if a single shard outgrows PG"). Not now.

## The `myelin-query` AST execution boundary (what we own vs consume — ADR-06/07)

- **Consume:** the AST *grammar*, the *validator*, the field-*type system*, the *view model* (table/board/etc.) —
  `myelin-query` shared crate, co-owned with Knowledge.
- **Own:** the **compiler from AST → our OLTP store** (which tier: typed column / GIN / generated index / Search)
  and the **cost-bounding** (every query is paginated, statement-timeout'd, and a query that would scan too much
  is pushed to Search or returns a "refine your filter" — X-3). This is ADR-06's exact line: "share the schema
  language and the view model, **not the query planner**." Knowledge owns *its* planner; we own *ours*; the AST
  is identical. The `EventMatcher` (trigger/automation) path reuses the *same* AST core (event-bus §4.5) so a
  saved view, a CLI query, an automation predicate, and an agent trigger are one language, one validator.

## Leaning

**Candidate B** — typed-core columns (hot path) + JSONB property-bag tail (custom fields, zero DDL) + a derived
indexable projection for *measured*-hot facets + Search as the escalation valve for cold/ad-hoc/full-text +
OLAP for analytics. **Deliberately aligned with Knowledge's `db_row` (01 §4.1)** so the two co-owners of ADR-06
share the storage discipline. We own the AST→store compiler and the cost-bounding; we consume the AST/types/view
model. PG-sharded-by-tenant is the floor; distributed-SQL is the named, measured follow-on.

## Hands forward

- The exact typed-core column list (which "always-present" fields earn a column) vs generated-column vs JSONB —
  architecture, co-reviewed with Knowledge for primitive parity.
- The projection-feeder design (how a field gets promoted to a generated index off the bus) — architecture.
- The AST→store cost model + the OLTP↔Search escalation threshold — architecture; PROVE-IT drill = a
  large-custom-field tenant board query under the latency budget (sketch in findings §drills).
