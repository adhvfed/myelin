# Knowledge Platform — 01 · Technology & Data Model

> See [`00-overview.md`](./00-overview.md) for the framing and the document split. This doc commits the
> language/DB choice (with written justification) and the complete data model: the block tree, the op-log
> + snapshots, the flexible database (rows/fields/views), the page-tree ACL projection, and the
> stateful-component register (X-4). Schemas are illustrative Postgres/Rust; the *shape* is the contract.

---

## 1. The language / tools / database choice (with written justification)

**Decision: Rust for all services; PostgreSQL-class OLTP as the system of record; an S3-compatible object
store for media + CRDT snapshots; Yrs (Rust Yjs) as the eventual CRDT; Tantivy via the shared Search
service; the editor as a Rust core compiled to WASM behind a TypeScript/React frontend.** No divergence
from the Rust default is requested — and the one "specialised" choice (Yrs) is itself Rust-native, which
*reinforces* the default (Phase-2 §3; VISION §4).

### 1.1 Service language — Rust (ADR-02 default, no reason to diverge)

| Concern | Choice | Written justification |
|---|---|---|
| Service language | **Rust** | ADR-02 default; the substrate glue crates are Rust (`serve(AppSpec)`, the consumer template, the resilient client, the outbox helper — substrate §2). No material reason to diverge; the CRDT story *favours* Rust (Yrs is Rust-native, §1.3). Cross-language would mean re-implementing the cross-language harness parity (substrate §13 Q1) for no benefit. |
| EU-deployable / self-hostable | **Confirmed** | Every component is self-hostable: PostgreSQL, an S3-compatible store (MinIO/Ceph/Garage), NATS JetStream (the bus, in-cell), Tantivy (embedded, no JVM), Yrs (a library). No US-controlled SaaS dependency. The cell is "one set of artifacts" (ADR-11); self-host = one cell. |
| Glue-contract implementability | **Confirmed across any boundary** | Knowledge stays in Rust, so the glue contracts are linked types, not a wire shim. Even the WASM editor core consumes the *same* `myelin-content` Rust crate compiled to WASM, eliminating client/server drift in the parser (DL §8.1 "share the implementation, not the spec"). |

### 1.2 Database — PostgreSQL-class OLTP as the system of record (ADR-10 / ADR-14)

**Decision: one PostgreSQL-class database per Knowledge service** (the `no-cross-db` boundary, ADR-01),
holding the block tree, rows, field/view definitions, the op-log + snapshot metadata, the page-tree tuples'
*authoring* records, and the per-service `outbox` table (the cross-seam anchor, substrate §3.3).

**Why Postgres, not a document store or a per-database materialised SQL table** (the *written why* TE-16/
TE-17 demand):

- **The block tree is an adjacency list — Postgres serves it natively.** Per-block rows (`parent_id` + a
  fractional ordering key) scale to huge documents and enable block-level references/permissions, whereas a
  single document blob caps doc size and couples permissions to whole docs (deep-dive §2.1; Celko, *Trees and
  Hierarchies in SQL*, 2012). Subtree reads are an index range; moves are an order-key write. Recursive CTEs
  (`WITH RECURSIVE`, SQL:1999) handle the rare deep subtree walk — the same proven graph-in-RDBMS pattern Refs
  uses (REF-2).
- **The flexible database is JSONB + derived projections — not per-tenant DDL.** A real SQL table per
  user-defined database means **DDL-per-tenant-database at world scale**, which is operationally heavy and
  fights multi-tenancy (deep-dive §2.4). JSONB property-bag rows (source of truth) + Postgres GIN/expression
  indexes + generated columns for the hot facets (the derived indexable projection, maintained off the bus)
  is the proven pragmatic answer (deep-dive §2.4; Karwin, *SQL Antipatterns*, 2010, on EAV trade-offs). This
  inverts our earlier "materialised-first" lean to the doctrine's **floor-first**: JSONB + read-time +
  derived projection, materialise more only when measured (KN-3; decision-record §(d) tension 2).
- **A dedicated document store (Mongo-class) is rejected by default.** It buys schema-flexibility we already
  get from JSONB, while losing transactional outbox co-commit (the dual-write hazard EI-02 §4 warns of),
  losing recursive-CTE tree walks, and adding a second residency-pinned, crypto-shred-capable,
  backup-verified engine per cell (EI-02 §8 "every additional data engine is permanent operational cost").
  No measured reason beats Postgres here.

The op-log durability and large/binary content live in the **object tier** (§1.4), keyed content-addressed
(BLAKE3, Storage §3.2) — the OLTP row holds the pointer + metadata, not the bytes.

### 1.3 Collaboration engine — Yrs (Rust Yjs), CAS floor first (TE-15, KN-1)

**Decision: a CRDT, leading implementation Yrs (Rust Yjs), is the committed *eventual* engine; the v1
floor is per-block optimistic compare-and-swap (CAS).** This is the doctrine ladder verbatim (KN-1; EI-04
§2): resume-cursor durable transport → CAS floor → CRDT.

**Why CRDT over OT (the cited prior art, expanded in [05 §1](./05-hard-problems.md)):** a CRDT (Automerge/
Yjs-class) merges deterministically without a central transform server; modern sequence CRDTs (RGA; Yjs/Yrs;
**Fugue** to fix interleaving) handle text/lists, and a **move CRDT** (Kleppmann's tree-move operation)
handles concurrent re-parenting. OT (à la Google Wave / `prosemirror-collab`) needs notoriously-hard
transformation functions per block type and effectively requires a central authoritative server (weaker
offline). Yrs is **Rust-native** (reinforcing ADR-02), the server can be a **"dumb relay + persistence"**
(scales horizontally), and offline-first aligns with the UX goal. **But uncertainty is high enough that the
CAS floor ships first** (EI-04 §2: "the first true concurrent-edit conflict is the CRDT's trigger") — the
floor guarantees no *silent* overwrite without the merge complexity, and the resume-cursor transport (built
first, KN-1) is the slot the CRDT drops into.

### 1.4 Media, snapshots, search, frontend

| Concern | Choice | Justification / spine tie |
|---|---|---|
| Media / blobs / op-log archive | **S3-compatible object store** (MinIO/Ceph/Garage), content-addressed (BLAKE3), residency-pinned, behind the `BlobStore` `put/get/head/delete` trait (STOR-1) | ADR-10 object tier; content-addressing gives dedup + integrity for free (Git object model / Venti / IPFS CID). Erasure of an immutable blob is crypto-shred (destroy the key), not `delete` (Storage §3.2). |
| CRDT snapshots / op-log persistence | OLTP (op-log rows, bounded) + **object tier** (compacted snapshots) | Op-logs grow unbounded; periodic compaction → a content-addressed snapshot in the object tier; the op-log table keeps the live tail (§3). |
| Search | **Shared Search** (Tantivy), block- *and* page-level index docs, multilingual, vector for RAG | ADR-10/Search §2.1; permission-aware via `list_objects` pre-filter; Knowledge declares its `IndexSpec` and `project`s text (Search §5.3). Block-vs-page granularity decided in [02 §6](./02-internals-and-algorithms.md). |
| Editor / renderer (frontend) | **One Rust `myelin-content` core compiled to WASM**, behind a TypeScript + React-class shell consuming the shared design-system package | DL §8.1: share the *implementation* of the parser (the round-trip gate KN-4 holds on identical client+server code), not just the spec (ADR-05/07). |
| Content model | **`myelin-content`** shared crate (ADR-05) — **Knowledge leads the taxonomy** | Knowledge leads the block/inline node taxonomy; Chat/Issues consume; concurrency stays Knowledge-owned (ADR-05). |
| Structured-collection primitive | **`myelin-query`** shared crate (ADR-06/07) — **Knowledge co-owns with Issues** | Field defs + views + query AST shared; the flexible-field execution + formula engines are Knowledge-owned. |
| Durable timers / scheduled automations | **`myelin-flow`** (`DurableExecutor`) | daily-notes, living-doc maintenance, the HITL approval-card resume (Workflow §9). Knowledge does not reinvent durable waits. |

---

## 2. The content model — block tree + markdown-subset inline (ADR-05, KN-2)

### 2.1 The block — the atomic unit (deep-dive §2.1)

A **block** is a row in the adjacency-list tree. It carries a stable id (survives moves/edits/collaboration,
the reference target), a type, type-specific properties, **inline content as a markdown-subset string**
(KN-2), a parent + fractional ordering key, and metadata.

```sql
CREATE TABLE block (
  tenant         uuid        NOT NULL,
  region         text        NOT NULL,                 -- residency-pinned (ADR-11); == cell.region (residency-pin lint)
  page_id        uuid        NOT NULL,                 -- the root page this block belongs to (partition helper)
  block_id       uuid        NOT NULL,                 -- STABLE id: the #sub anchor (e.g. #b9) and ref target
  parent_id      uuid,                                 -- NULL for the page root block (adjacency list)
  order_key      text        NOT NULL,                 -- FRACTIONAL index (LexoRank-style) — concurrent insert, no renumber
  block_type     block_type  NOT NULL,                 -- paragraph|heading|list_item|to_do|toggle|quote|code|callout|
                                                       --   image|table|table_row|database_view|embed|column|divider|equation
  props          jsonb       NOT NULL DEFAULT '{}',    -- type-specific: heading level, code lang, checkbox state, caption
  inline         text        NOT NULL DEFAULT '',      -- MARKDOWN-SUBSET STRING (KN-2) — the inline runs
  inline_nodes   jsonb       NOT NULL DEFAULT '[]',    -- STRUCTURED nodes kept OUT of the string: mention/artifact_ref/embed
  contains_personal_data boolean NOT NULL DEFAULT false, -- routes GDPR (ADR-05 erasure hook)
  pii_key_ref    text,                                 -- per-subject DEK ref if the inline text holds PII (GD-4)
  created_by     uuid        NOT NULL,                 -- pseudonymous principal_id (erasure-safe; EI-04 §1)
  edited_by      uuid        NOT NULL,
  created_at     timestamptz NOT NULL,
  edited_at      timestamptz NOT NULL,
  version        bigint      NOT NULL,                 -- the CAS last-modified token (floor; [02 §3])
  PRIMARY KEY (tenant, block_id),
  FOREIGN KEY (tenant, parent_id) REFERENCES block(tenant, block_id) ON DELETE CASCADE
);
CREATE INDEX block_children ON block (tenant, page_id, parent_id, order_key);  -- ordered sibling read = index range
CREATE INDEX block_by_page  ON block (tenant, page_id);
```

**Why this shape** (the TE-16 resolution, [05 §3](./05-hard-problems.md)):

- **Adjacency list + fractional `order_key`** (LexoRank-style, the same family Issues uses for drag-rank,
  TE-19). Fractional indexing lets a concurrent insert pick a key *between* two siblings without renumbering
  the list (deep-dive §2.1). Its interleaving/precision pitfalls under heavy concurrency are bounded in the
  CAS floor (one writer per position wins; the loser re-bases) and resolved natively when the CRDT lands
  (Yrs's list type / Fugue, [05 §1](./05-hard-problems.md)).
- **Inline = a markdown-subset STRING, structured refs kept OUT of the string** (KN-2 / D10; EI-05 §2). The
  markdown-subset string (`**bold**`, `_italic_`, `` `code` ``, links) survives copy/paste, export, diff, and
  reference-extraction, and needs no server-side sanitisation pass. But `mention`/`artifact_ref`/`embed`
  are kept as **structured nodes** in `inline_nodes` (with a placeholder token in the string) so
  reference-extraction stays reliable (the producer of `refs.edge.created`, [03 §3](./03-events-contracts-and-glue.md))
  and is **never** a regex-over-prose. This is the AST-for-structure / markdown-string-for-runs reconciliation
  of ADR-05 (decision-record §(d) tension 3).
- **`version`** is the CAS last-modified token (the floor's optimistic-concurrency guard, [02 §3](./02-internals-and-algorithms.md)).
- **`contains_personal_data` + `pii_key_ref`** route erasure: free-text blocks holding PII are encrypted
  under a **per-subject DEK** (GD-4) so a person's erasure crypto-shreds exactly their content reachable in
  immutable history ([05 §6](./05-hard-problems.md)).

### 2.2 Pages, hierarchy, spaces

A **page** is a root block subtree, independently addressable/permissioned/referenceable. Pages nest
(sub-pages = the folder-like hierarchy — Notion's "everything is a page", deep-dive §2.3). **Decision
(deep-dive Q7): pure pages, with an optional `is_folder` render hint** — corporate-familiar "folders" are a
page with no body and a folder icon, not a second concept; this keeps one permission/reference model.

```sql
CREATE TABLE page (
  tenant       uuid NOT NULL,
  region       text NOT NULL,
  page_id      uuid NOT NULL,
  space_id     uuid NOT NULL,                          -- the workspace/teamspace grouping (maps to a project, §4.3)
  parent_page  uuid,                                   -- sub-page nesting → page_parent typed edge (TE-7, §4.2)
  title        text NOT NULL,                          -- rendered title; contains_personal_data possible
  icon         text,
  is_folder    boolean NOT NULL DEFAULT false,         -- render hint only (pure-pages model)
  published    boolean NOT NULL DEFAULT false,         -- public-publish (GDPR-flagged, deep-dive §8)
  archived     boolean NOT NULL DEFAULT false,
  acl_zookie   text,                                   -- the zookie stamped at the last ACL change (Id §8.4)
  created_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id)
);
```

A **space** maps to the platform org/project model (Id §5) — Knowledge does **not** invent a parallel
hierarchy (deep-dive §2.3): a space is a `project` object in the ReBAC namespace; its default permissions are
project-level tuples.

---

## 3. The op-log + snapshots (history, KN-1 transport substrate)

Live edits are **operations** on the resume-cursor transport ([02 §2](./02-internals-and-algorithms.md));
durably persisted as an append-only op-log, periodically compacted to snapshots. This is the deep-dive §2.8
hybrid: op-log for live collab + compacted snapshots for history/restore.

```sql
CREATE TABLE doc_op (
  tenant       uuid NOT NULL,
  region       text NOT NULL,
  page_id      uuid NOT NULL,                          -- the doc (aggregate) — ordering partition
  op_seq       bigint NOT NULL,                        -- per-doc monotonic; the RESUME CURSOR position (KN-1)
  op_id        text NOT NULL,                          -- deterministic (client_id + lamport) → idempotent apply
  actor        uuid NOT NULL,                          -- pseudonymous principal (human or agent — same protocol)
  op_kind      op_kind NOT NULL,                       -- insert|delete|format|move|set_prop|block_ins|block_del
  payload      jsonb NOT NULL,                         -- the op (CAS delta in floor; Yrs update bytes when CRDT lands)
  pii_key_ref  text,                                   -- per-subject DEK if the op carries inline PII (GD-4)
  applied_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id, op_seq),
  UNIQUE (tenant, page_id, op_id)                      -- idempotent apply: a re-delivered op is a no-op (KN-1)
);
CREATE INDEX doc_op_resume ON doc_op (tenant, page_id, op_seq);  -- the resume read: ops since the client's cursor

CREATE TABLE doc_snapshot (
  tenant       uuid NOT NULL,
  region       text NOT NULL,
  page_id      uuid NOT NULL,
  snap_seq     bigint NOT NULL,                        -- the op_seq this snapshot includes up to
  blob_hash    text NOT NULL,                          -- content-addressed snapshot in the object tier (BLAKE3)
  named_label  text,                                   -- NULL = auto-compaction snapshot; set = a named version (restore point)
  created_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id, snap_seq)
);
```

**Properties** (detail in [02 §2/§3](./02-internals-and-algorithms.md)):

- **`op_seq` is the resume cursor** (KN-1). A reconnecting client sends its last-seen `op_seq`; the transport
  replays `doc_op WHERE op_seq > cursor` — **reconnect loses zero ops** (the drill, [07](./07-drills-and-open-questions.md)).
- **`op_id` is deterministic** (`client_id` + lamport counter) → `UNIQUE` makes apply **idempotent**; an
  at-least-once redelivery is a no-op. This is the transport's correctness substrate, identical for CAS and
  CRDT.
- **Compaction**: a background job folds ops ≤ N into a snapshot blob and GCs `doc_op` rows below it
  (bounded op-log), preserving named-version restore points. Erasure reaches the op-log via **per-subject
  crypto-shred** (you cannot delete a merge-dependent op; you destroy the key, [05 §6](./05-hard-problems.md)).

---

## 4. The flexible database — rows, fields, views (ADR-06, TE-17)

### 4.1 The structured collection (JSONB property bag + derived projection)

```sql
CREATE TABLE db_collection (                            -- a "database" instance (shared primitive, ADR-06)
  tenant      uuid NOT NULL, region text NOT NULL,
  db_id       uuid NOT NULL,
  space_id    uuid NOT NULL,
  name        text NOT NULL,
  field_defs  jsonb NOT NULL,                           -- the shared myelin-query field definitions (typed columns)
  PRIMARY KEY (tenant, db_id)
);

CREATE TABLE db_row (
  tenant      uuid NOT NULL, region text NOT NULL,
  db_id       uuid NOT NULL,
  row_id      uuid NOT NULL,
  props       jsonb NOT NULL,                           -- THE PROPERTY BAG: { field_id → value } (TE-17 source of truth)
  body_page   uuid,                                     -- a row IS a page (open-as-page) — its body block subtree
  order_key   text NOT NULL,                            -- fractional rank for manual ordering (TE-19 family)
  version     bigint NOT NULL,                          -- CAS token for row edits
  contains_personal_data boolean NOT NULL DEFAULT false,
  pii_key_ref text,
  created_at  timestamptz NOT NULL,
  PRIMARY KEY (tenant, row_id)
);
-- The DERIVED indexable projection: generated columns + GIN for the HOT facets (filter/sort/group at scale).
CREATE INDEX db_row_props_gin ON db_row USING gin (props jsonb_path_ops);
-- Per-db hot facets get expression/generated-column indexes, provisioned off the bus when a field is filtered/sorted often:
--   e.g. CREATE INDEX db_row_status ON db_row ((props->>'status')) WHERE db_id = :db;  (maintained by the projection feeder)
```

**The TE-17 resolution** (detail [02 §4](./02-internals-and-algorithms.md), [05 §3](./05-hard-problems.md)):
JSONB property bag is the **source of truth**; a **derived indexable projection** (GIN + per-hot-facet
generated/expression indexes, maintained off the bus, *not* per-tenant DDL) serves filter/sort/group at
scale. Materialise a real columnar projection (the OLAP read store, Storage §3.4) only when read-time
filter/sort is *measured* too slow (KN-3-style measured promotion).

### 4.2 Relations — the `db_relation` / `page_parent` typed tables (TE-7, REF-1)

Knowledge owns the typed relation tables that are the **source of truth** for its lifecycle/semantic edges;
Refs holds a rebuildable projection (REF-1; the same hybrid Issues uses for `issue_relation`). The same
transaction that writes a typed row emits a typed lifecycle event the Refs edge-builder consumes ([03 §3](./03-events-contracts-and-glue.md)).

```sql
-- The SOURCE OF TRUTH for db relations (two-way relation field type, ADR-06). Refs mirrors via events (REF-1).
CREATE TABLE db_relation (
  tenant       uuid NOT NULL, region text NOT NULL,
  relation_id  uuid NOT NULL,
  src_row      uuid NOT NULL,                           -- internal id (Knowledge's own key — referential integrity)
  dst_ref      text NOT NULL,                           -- ArtifactRef of the other end (may be cross-subsystem: an issue, a doc)
  rel          db_rel NOT NULL,                         -- 'relates' | 'rollup_source' (the relation a rollup aggregates over)
  created_by   uuid NOT NULL, created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, relation_id),
  UNIQUE (tenant, src_row, dst_ref, rel),
  FOREIGN KEY (tenant, src_row) REFERENCES db_row(tenant, row_id) ON DELETE CASCADE  -- integrity Refs can't give
);

-- The SOURCE OF TRUTH for page-tree parent edges (page → sub-page). Mirrored to Refs as a 'parent' lifecycle edge.
CREATE TABLE page_parent (
  tenant       uuid NOT NULL, region text NOT NULL,
  page_id      uuid NOT NULL,
  parent_page  uuid NOT NULL,
  order_key    text NOT NULL,                           -- sibling order among sub-pages
  PRIMARY KEY (tenant, page_id),
  FOREIGN KEY (tenant, page_id)     REFERENCES page(tenant, page_id) ON DELETE CASCADE,
  FOREIGN KEY (tenant, parent_page) REFERENCES page(tenant, page_id)
);
```

Two-way relation consistency is maintained **transactionally for the forward edge** (the FK) and
**eventually-consistently for the inverse projection** in Refs (the best-effort bidirectional consistency
EI-04 §2 names; [02 §4](./02-internals-and-algorithms.md)).

### 4.3 Views — query-AST projections over the collection (ADR-06/07)

A **view** is a saved query + presentation over the same rows: filter predicate (the shared query AST,
ADR-07), sort, group-by, visible/hidden fields, per-view ordering/width, and (for board/calendar) the
grouping/date field. Views are **per-user-overridable vs shared** (deep-dive §2.4): a shared base definition
with optional personal tweaks layered on top.

```sql
CREATE TABLE db_view (
  tenant      uuid NOT NULL, region text NOT NULL,
  view_id     uuid NOT NULL, db_id uuid NOT NULL,
  view_type   view_type NOT NULL,                       -- table | board | calendar | list | gallery | timeline
  query_ast   jsonb NOT NULL,                           -- the shared myelin-query AST (filter/sort/group); permission-aware by construction (ADR-07)
  group_by    text,                                     -- a field id (board/calendar grouping)
  visible     jsonb NOT NULL,                           -- field ordering + widths
  shared      boolean NOT NULL DEFAULT true,
  PRIMARY KEY (tenant, view_id)
);
CREATE TABLE db_view_override (                          -- per-user personal tweaks layered on a shared view
  tenant uuid NOT NULL, view_id uuid NOT NULL, principal uuid NOT NULL,
  override jsonb NOT NULL,                               -- a partial query-AST/visible diff
  PRIMARY KEY (tenant, view_id, principal)
);
```

Every view query **always composes `list_objects(viewer, read, type)`** (ADR-07 permission-by-construction)
so a viewer sees only rows they may read — never post-filtered ([02 §5](./02-internals-and-algorithms.md)).

### 4.4 Field types (Notion-parity baseline, ADR-06)

Primitives (`title`, `text`, `number`, `checkbox`, `date`/`date-range`, `url`, `email`, `phone`); choice
(`select`, `multi-select`, `status`); people & files (`person`, `files`); **computed read-time** (`formula`,
`rollup`, `created/edited time`, `created/edited by`, auto-increment); **relation** (rides Refs cross-artifact,
the `db_relation` table intra-collection); and the Myelin-specific **`artifact_ref`** field (a typed pointer
to *any* platform artifact — issue/commit/CI run/chat thread/doc, deep-dive §2.4). `formula`/`rollup` are
**computed at read time, never stored** ([02 §4](./02-internals-and-algorithms.md), KN-3).

---

## 5. The page-tree ACL projection (the Knowledge ReBAC namespace fragment)

Knowledge **declares its ReBAC namespace fragment** to Id (Id §5 Knowledge clause) and **compiles the page
tree to tuples** via the Permission projector — no bespoke ACL (deep-dive §2.7; ADR-03). The fragment is
*page-tree inheritance with overrides*, the canonical Zanzibar pattern:

```
definition space {                                    // a space maps to a project (Id §5)
  relation parent_project: project
  relation member: user | team#member
  permission read = member + parent_project->read
}
definition page {
  relation parent_page:   page                        // sub-page nesting (page_parent typed table, §4.2)
  relation parent_space:  space
  relation direct_reader: user | team#member          // explicit grant on this page
  relation direct_editor: user | team#member
  relation direct_block:  user | team#member          // the OVERRIDE: narrows inherited access (exclusion)
  permission read    = (parent_page->read + parent_space->read + direct_reader) - direct_block
  permission comment = read - direct_block
  permission edit    = direct_editor + parent_page->edit + parent_space->member - direct_block
  permission manage  = direct_editor + parent_page->manage
}
definition database_row {                             // row-level visibility (a scoped feature, §6 below)
  relation parent_db: page                            // the db lives on a page
  relation row_reader: user | team#member             // explicit row grant (e.g. "see only your team's rows")
  permission read = parent_db->read + row_reader      // + an ABAC caveat for field-level (kept off the hot list_objects path)
}
```

**Permission granularity (deep-dive Q6, scope decision — DECIDED):**

- **Page/database-level** (v1, full): the page-tree `read`/`comment`/`edit`/`manage` above.
- **Row-level** (v1, via the `database_row` namespace + `row_reader` relation): corporate buyers want "see
  only your team's rows" — expressed as tuples, evaluated by `list_objects` like any other (Id §5).
- **Field-level** (v1, **ABAC caveat at the edge**, kept off the hot `list_objects` path): "hide the salary
  column" is a caveat on a `field.view` permission, evaluated at `check` time with context (Id §9), so the
  bulk pre-filter stays fast (deep-dive §2.7).

`parent_page->read` is the **tuple-to-userset rewrite** that makes the whole tree's inheritance one rule;
`- direct_block` is Zanzibar's **exclusion** userset — a sub-page that *narrows* inherited access disappears
from a normal reader's `list_objects` **by construction**, not by a post-filter (Id §5).

---

## 6. The stateful-component register + blast-radius note (X-4 / SUB-X)

Per X-4, every stateful component named with a shared-state/sharding/blast-radius plan; everything else is
stateless and replaceable.

| # | Component | Engine | Holds | Shard key | Blast radius if it dies | Crypto-shred unit |
|---|---|---|---|---|---|---|
| K1 | **Block tree + page (OLTP)** | Postgres-class | blocks, pages, `outbox` | `(tenant, region)` + page_id | one tenant's docs; recoverable (outbox drains; reindex rebuilds derived) | per-tenant DEK; **per-subject DEK for free-text columns** (GD-4) |
| K2 | **`db_collection`/`db_row`/`db_view`/`db_relation`/`page_parent`** | Postgres-class | structured data + typed edges (source of truth) | `(tenant, region)` + db_id | one tenant's databases | per-tenant DEK; per-subject for PII cells |
| K3 | **`doc_op` op-log** | Postgres-class (live tail) + object tier (snapshots) | the resume-cursor op-log; compacted snapshots | `(tenant, region)` + page_id | one doc's live session degrades; **reconnect replays from cursor — no op loss** (KN-1) | per-subject DEK on PII-bearing ops; snapshot blob = per-tenant DEK |
| K4 | **Collab session state (relay)** | in-memory + the op-log backing | the live awareness/presence + in-flight op buffer | `(tenant, region, page_id)` | a relay crash → clients reconnect, replay from `op_seq` cursor (no loss) | ephemeral |
| K5 | **Consumer dedup ledgers** | Postgres-class | the consumer template's idempotency (substrate §5) | `(tenant, consumer)` | re-process is idempotent → no loss | inherits K1 |
| K6 | **Media / snapshot blobs** | S3-compatible object store | images, files, CRDT snapshots (content-addressed) | `(tenant, region)` + hash | one tenant's media; OLTP rows point at it (cross-seam, STOR-4) | per-tenant DEK (per-blob CK wrapped) |
| K7 | **Derived projection indexes** | Postgres (GIN/generated cols) | the flexible-DB hot-facet projection | `(tenant, region)` + db_id | derived — rebuildable from K2 | inherits K2 |
| K8 | **Agent-trace documents (AG-7)** | Knowledge content (K1/K6) | content-addressed agent execution traces | `(tenant, region)` | an erasable holder; traces unavailable, runs continue | per-subject DEK |

The **page-tree ReBAC tuples are NOT Knowledge's component** — they live in Id's tuple store (S3); Knowledge
only *projects* into them (§5). All derived state (K7) is rebuildable by reindex-from-source. K3's whole
point is **no op loss on reconnect** — the resume-cursor drill ([07](./07-drills-and-open-questions.md)).

**Hot tables flagged for the `forward-only-migration` lint** (substrate §9): `block`, `db_row`, `doc_op`
(the high-write-volume tables) — schema changes use expand→backfill→contract, never a blocking `ALTER`.

Continue to [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) for the algorithms.
