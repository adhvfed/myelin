# Knowledge Platform — 01 · Technology & Data Model

> See [`00-overview.md`](./00-overview.md) for framing, the document split, and §0 (the reconciliation
> deltas). This doc commits the language/DB choice (with written justification) and the complete data model:
> the block tree (over the **frozen `myelin-content` taxonomy**), the op-log + snapshots, the flexible
> database (over the **frozen `myelin-query` shapes**), the page-tree ReBAC fragment, and the
> stateful-component register. Schemas are illustrative Postgres/Rust; the *shape* of the shared crates is the
> FROZEN contract (13.1 / 13.3) — Knowledge does not redefine those shapes, it stores and executes them.

---

## 1. The language / tools / database choice (with written justification)

**Decision (carried forward from Phase 4, no reconciliation change): Rust for all services; PostgreSQL-class
OLTP as the system of record; an S3-compatible object store for media + CRDT snapshots; Yrs (Rust Yjs) as the
eventual CRDT; Tantivy via the shared Search service; the editor as a Rust `myelin-content` core compiled to
WASM behind a TypeScript/React frontend.** No divergence from the Rust default is requested — and the one
"specialised" choice (Yrs) is itself Rust-native, which *reinforces* the default (VISION §4). Reconciliation
forced no change here (recon §0: the Phase-4 language/DB choice stands).

### 1.1 Service language — Rust (ADR-02 default, no reason to diverge)

| Concern | Choice | Written justification |
|---|---|---|
| Service language | **Rust** | ADR-02 default; the substrate glue crates are Rust (`serve(AppSpec)`, the consumer template, the resilient client, the outbox helper — contract 1.1/1.9/2.2/2.4). No material reason to diverge; the CRDT story *favours* Rust (Yrs is Rust-native, §1.3). Cross-language would mean satisfying the cross-language harness shim (contract 1.7) for no benefit. |
| EU-deployable / self-hostable | **Confirmed** | Every component self-hostable: PostgreSQL, an S3-compatible store (MinIO/Ceph/Garage), the bus (in-cell), Tantivy (embedded, no JVM), Yrs (a library). No US-controlled SaaS dependency. The cell is "one set of artifacts" (ADR-11); self-host = one cell. |
| Glue-contract implementability | **Confirmed** | Knowledge stays in Rust, so the glue contracts are linked types, not a wire shim. The WASM editor core consumes the *same* `myelin-content` Rust crate compiled to WASM, eliminating client/server parser drift (contract 13.1 WASM target). |

### 1.2 Database — PostgreSQL-class OLTP as the system of record (ADR-10 / ADR-14, contract 11.1)

**Decision: one PostgreSQL-class database per Knowledge service** (the `no-cross-db` boundary, ADR-01),
holding the block tree, rows, field/view definitions, the op-log + snapshot metadata, the typed relation
tables, and the per-service `outbox` table (the cross-seam anchor, contract 2.3).

**Why Postgres, not a document store or a per-database materialised SQL table** (the *written why*):

- **The block tree is an adjacency list — Postgres serves it natively.** Per-block rows (`parent_id` + a
  fractional `order_key`) scale to huge documents and enable block-level references/permissions, whereas a
  single document blob caps doc size and couples permissions to whole docs (Celko, *Trees and Hierarchies in
  SQL*, 2012). Subtree reads are an index range; moves are an `order_key` write. Recursive CTEs handle the
  rare deep subtree walk — the same graph-in-RDBMS pattern Refs uses.
- **The flexible database is JSONB + derived projections — not per-tenant DDL.** A real SQL table per
  user-defined database means **DDL-per-tenant-database at world scale**, operationally heavy and fighting
  multi-tenancy. JSONB property-bag rows (source of truth) + Postgres GIN/expression indexes + generated
  columns for the **measured-hot** facets (the derived indexable projection, maintained off the bus) is the
  proven pragmatic answer (Karwin, *SQL Antipatterns*, 2010, on EAV trade-offs). The **measured-promotion
  threshold** is now the frozen Search-owned tunable (contract 6.3 / OQ-C: a facet in `> 5%` of a collection's
  view executions over a rolling window promotes from a GIN scan to a generated index — measured, never
  predicted).
- **A dedicated document store (Mongo-class) is rejected by default.** It buys schema-flexibility we already
  get from JSONB, while losing transactional outbox co-commit (the dual-write hazard EI-02 §4 warns of),
  losing recursive-CTE tree walks, and adding a second residency-pinned, crypto-shred-capable, backup-verified
  engine per cell. No measured reason beats Postgres here.

Large/binary content lives in the **object tier** (§1.4), keyed content-addressed (BLAKE3, contract 11.2) —
the OLTP row holds the pointer + metadata, not the bytes.

### 1.3 Collaboration engine — Yrs (Rust Yjs), CAS floor first (TE-15, KN-1)

**Decision: a CRDT, leading implementation Yrs (Rust Yjs), is the committed *eventual* engine; the v1 floor
is per-block optimistic compare-and-swap (CAS).** This is the doctrine ladder verbatim (KN-1; EI-04 §2):
resume-cursor durable transport → CAS floor → CRDT. **Why CRDT over OT** is in [05 §1](./05-hard-problems.md)
(cited prior art). Yrs is **Rust-native** (reinforcing ADR-02), the server is a "dumb relay + persistence +
authority," and offline-first aligns with the UX goal; uncertainty is high enough that the CAS floor ships
first (EI-04 §2). The transport (built first, KN-1) is the slot the CRDT drops into.

### 1.4 Media, snapshots, search, frontend

| Concern | Choice | Justification / spine tie |
|---|---|---|
| Media / blobs / op-log archive | **S3-compatible object store**, content-addressed (BLAKE3), residency-pinned, behind `BlobStore{put,get,head,delete}` (contract 11.2) | content-addressing gives dedup + integrity (Git object model / Venti / IPFS CID). Immutable-blob erasure is crypto-shred (destroy the key), not `delete`. |
| CRDT snapshots / op-log persistence | OLTP (op-log rows, bounded) + **object tier** (compacted snapshots) | op-logs grow unbounded; periodic compaction → a content-addressed snapshot in the object tier; the op-log table keeps the live tail (§3). |
| Search | **Shared Search** (Tantivy), block- *and* page-level index docs, multilingual, **vector-in-v1** for RAG | contract 6.1–6.3; permission-aware via the `list_objects` `Filter` conjoin (the `search-requires-acl-filter` lint); Knowledge declares its `IndexSpec` + `project`s text. |
| Editor / renderer (frontend) | **One Rust `myelin-content` core compiled to WASM**, behind a TypeScript + React-class shell consuming the shared design-system package | contract 13.1 WASM target: share the *implementation* of the parser (the round-trip gate holds on identical client+server code), not just the spec. |
| Content model | **`myelin-content`** shared crate (contract 13.1) — **Knowledge LEADS + FREEZES the taxonomy** | Knowledge owns the block/inline node taxonomy; Chat/Issues consume strict subsets; concurrency stays Knowledge-owned. |
| Structured-collection primitive | **`myelin-query`** shared crate (contract 13.3) — **Knowledge co-owns with Issues** | the `FieldType`/`ViewSpec`/`QueryAst`/`order_key` are byte-identical; Knowledge owns its flexible-field executor + the formula engine. |
| Durable timers / scheduled automations | **`myelin-flow`** (`DurableExecutor`, contract 9.x) | daily-notes, living-doc maintenance (as `SCHEDULE_AND_RUN_JOB` jobs), the HITL approval-card resume (per-effect `idem_key`). |

---

## 2. The content model — the FROZEN `myelin-content` taxonomy (contract 13.1, X-2/OQ-B)

Knowledge **leads and freezes** the canonical taxonomy; this is the complete v1 set. Chat and Issues declare
*subsets* (neither adds a node type). The shared crate defines these shapes; Knowledge stores them.

### 2.1 The canonical block taxonomy (frozen — `myelin-content` v1)

```
Block =
  | paragraph        { inline }
  | heading          { level: 1..6, inline }
  | bullet_list      { items: [list_item] }
  | ordered_list     { items: [list_item], start: u32 }
  | task_list        { items: [task_item{ checked: bool, inline }] }
  | blockquote       { blocks: [Block] }
  | code_block       { lang: Option<String>, text: String }   // text is raw, NOT markdown-parsed
  | callout          { tone: info|warn|success|danger|note, blocks: [Block] }
  | table            { columns: [col], rows: [[cell{ blocks }]] }
  | divider
  | image            { blob: ArtifactRef, alt: String, caption: Option<inline> }
  | embed            { ref: ArtifactRef, display: inline|card|preview }   // structured node (load-bearing)
  | db_view          { db: ArtifactRef, view: ViewSpec }       // Knowledge-only in v1; a myelin-query view
  | toggle           { summary: inline, blocks: [Block] }
  | sync_block       { source: ArtifactRef }                   // Knowledge-only; transclusion (FLOOR — Δ3, §2.4)
```

> **Δ2/Δ3 reconciliation note:** the Phase-4 `block_type` enum (a Knowledge-local list) is replaced by this
> frozen set. `sync_block` is now a **node type** in the taxonomy; its *engine* is a read-projection floor
> ([05 §7](./05-hard-problems.md)), not editable-in-place multi-home. `db_view`/`sync_block` are Knowledge-only
> (Chat/Issues subsets exclude them, X-2).

### 2.2 The canonical inline grammar (markdown-subset string + three structured nodes)

The inline content of a block is a **markdown-subset string** (KN-2). The subset: `**bold**`, `*italic*`,
`` `code` ``, `~~strike~~`, `[text](url)`, plus three **structured inline nodes** that are NOT markdown text —
they round-trip as opaque sentinels in the string and resolve to objects:

```
mention(Principal)        // @alice — renders to display name per-viewer (REF-3)
artifact_ref(ArtifactRef) // a typed reference to any artifact; the PRODUCER of refs.edge.created (5.4)
embed(ArtifactRef)        // an inline unfurl/transclusion request
```

The round-trip invariant `render(parse(md)) === md` holds over this subset (the editor gate, [02 §8](./02-internals-and-algorithms.md));
the three structured nodes are stored structured precisely so **reference-extraction stays reliable** — they
are the producers of `refs.edge.created` (contract 5.4), **uniformly across Chat, Issues, and Knowledge**.

**The placeholder encoding (carried forward from Phase-4 CR-C, now phrased against the frozen grammar):** the
inline string holds a single-character logical placeholder per structured node — the Unicode **Object
Replacement Character `U+FFFC`** (the standard "an object goes here" code point) — at the node's offset; the
binding is **positional** (the i-th `U+FFFC` ⇒ `inline_nodes[i]`). The node carries its own `kind` + `target`
in the array, so the string never carries the id. This satisfies the frozen "round-trip as opaque sentinels"
rule, keeps the caret offset model uniform (a mention is one caret position), and makes reference-extraction a
**node-array walk, never a regex over prose** (so rename/erase never touch stored prose).

### 2.3 The block row (storage)

```sql
CREATE TABLE block (
  tenant         uuid        NOT NULL,
  region         text        NOT NULL,                 -- residency-pinned (ADR-11); == cell.region (residency-pin lint)
  page_id        uuid        NOT NULL,                 -- the root page this block belongs to (partition helper)
  block_id       uuid        NOT NULL,                 -- STABLE opaque id: the #sub anchor (b<id>) and ref target (X-4)
  parent_id      uuid,                                 -- NULL for the page root block (adjacency list)
  order_key      text        NOT NULL,                 -- the FROZEN LexoRank order_key (§2.5)
  block_type     block_type  NOT NULL,                 -- one of the frozen myelin-content Block variants (§2.1)
  props          jsonb       NOT NULL DEFAULT '{}',    -- variant-specific: heading level, code lang/text, callout tone, etc.
  inline         text        NOT NULL DEFAULT '',      -- the markdown-subset string (U+FFFC placeholders for structured nodes)
  inline_nodes   jsonb       NOT NULL DEFAULT '[]',    -- the structured nodes (mention/artifact_ref/embed), positional
  contains_personal_data boolean NOT NULL DEFAULT false, -- routes GDPR
  data_role      text,                                 -- the envelope data_role (tenant-content default)
  pii_key_ref    text,                                 -- kms://<tenant>/<dek-epoch>/subject:<id> if inline holds a subject's PII (11.4)
  created_by     uuid        NOT NULL,                 -- pseudonymous principal_id (erasure-safe; <pseudonym>@<tenant>.noreply)
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

`version` is the CAS optimistic-concurrency guard ([02 §3](./02-internals-and-algorithms.md)).
`contains_personal_data` + `pii_key_ref` route erasure: free-text blocks holding a subject's PII are encrypted
under that **subject's DEK** (`<class> = subject:<id>`, contract 11.4) so a person's erasure crypto-shreds
exactly their content reachable in immutable history ([05 §6](./05-hard-problems.md)). `pii_key_ref` follows
the frozen `kms://<tenant>/<dek-epoch>/<class>` grammar (units anchor).

### 2.4 `sync_block` — transclusion as a read-projection FLOOR (Δ3)

`sync_block { source: ArtifactRef }` exists in the frozen taxonomy. The **v1 engine** renders it like `embed`:
the Projection/Render service resolves `source` via Refs `resolve(ref, viewer)` (contract 5.2), permission-
filtered per viewer, with the tombstone ladder on loss. This delivers transclusion's *read* value (show
content from elsewhere, live, permission-correct) **without** the shared-mutable-node complexity. The
**named follow-on** (editable-in-place, many edit sites of one canonical block) is designed against the CRDT
(which makes the shared-mutable-node merge tractable), with most-restrictive-of-sites permission + reference-
counted erasure ([05 §7](./05-hard-problems.md)). The node type is present so the taxonomy is complete; the
engine is a floor.

### 2.5 The FROZEN `order_key` / LexoRank encoding (contract 13.3, X-3 — byte-identical with Issues)

Manual drag-to-reorder uses a fractional index (LexoRank-class, the proven Jira ranking scheme). Knowledge
stores `order_key text`; the encoding is the **frozen shared shape**, identical to "an issue dragged in a
backlog":

- **Alphabet / base:** base-62, ordered `0-9 A-Z a-z` (ASCII-ordinal, so byte comparison == rank order).
- **Encoding:** an `order_key` is a non-empty string over the alphabet; ranking is **lexicographic string
  comparison**. Between two keys `a < b`, a new key is the midpoint via digit-wise bisection; when no digit
  fits between, **append** a midpoint digit (the key grows by one char) rather than rebalancing.
- **Initial spacing:** first item `"U"` (mid of the alphabet); bulk insert spreads evenly across the range.
- **Jitter:** a new key appends a **2-char random suffix** so two clients inserting "at the same midpoint"
  produce **distinct** keys — no two concurrent drags collide on an identical key (the concurrency-safety
  reason the jitter exists).
- **Rebalance:** when a key exceeds **48 chars** (measured pathology), a background rebalance re-spaces the
  collection's keys; it is a `myelin-flow` activity, idempotent, emitted via outbox so views resubscribe. In
  Knowledge a rebalance touches **one sibling list** (one parent's children), blast radius one list.
- **Tiebreak:** when two `order_key`s compare equal (should not happen with jitter), the deterministic
  tiebreak is `created_at` then `block_id`/`row_id` (ULID) — total order guaranteed.

> **Δ5 reconciliation note:** the Phase-4 "measured key-length threshold + jitter" is replaced by these frozen
> constants (base-62 alphabet, `"U"` start, 2-char jitter, 48-char rebalance). This makes Knowledge's "drag a
> row in a db" and "drag a block in a page" produce byte-identical keys to Issues' drag-rank, so a future
> shared CRDT/render path treats ordering uniformly. Once the move-CRDT lands, the CRDT's list type owns
> sibling ordering and the `order_key` becomes a derived OLTP-index ordering hint recomputed from CRDT state
> (CR-A interaction, [02 §3.5](./02-internals-and-algorithms.md)).

### 2.6 Pages, hierarchy, spaces

A **page** is a root block subtree, independently addressable/permissioned/referenceable. Pages nest
(sub-pages = the folder-like hierarchy). **Pure pages with an `is_folder` render hint** — a "folder" is a page
with no body and a folder icon, not a second concept (one permission/reference model).

```sql
CREATE TABLE page (
  tenant       uuid NOT NULL, region text NOT NULL,
  page_id      uuid NOT NULL,
  space_id     uuid NOT NULL,                          -- the workspace/teamspace grouping (maps to a project, §5)
  parent_page  uuid,                                   -- sub-page nesting → page_parent typed edge (TE-7, §4.2)
  title        text NOT NULL,
  icon         text,
  is_folder    boolean NOT NULL DEFAULT false,         -- render hint only (pure-pages model)
  published    boolean NOT NULL DEFAULT false,         -- public-publish (GDPR-flagged)
  archived     boolean NOT NULL DEFAULT false,
  acl_zookie   text,                                   -- the zookie stamped at the last ACL change (contract 4.6/4.10)
  created_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id)
);
```

A **space** maps to the platform org/project model (contract 4.9) — Knowledge does **not** invent a parallel
hierarchy: a space is a `project` object in the ReBAC namespace; its default permissions are project-level
tuples.

---

## 3. The op-log + snapshots (history, the KN-1 transport substrate)

Live edits are **operations** on the resume-cursor transport ([02 §2](./02-internals-and-algorithms.md));
durably persisted as an append-only op-log, periodically compacted to snapshots.

```sql
CREATE TABLE doc_op (
  tenant       uuid NOT NULL, region text NOT NULL,
  page_id      uuid NOT NULL,                          -- the doc (aggregate) — the firehose scope = doc:<page_id>
  op_seq       bigint NOT NULL,                        -- per-doc monotonic; == the firehose per-(stream,scope) seq (OQ-J)
  op_id        text NOT NULL,                          -- deterministic (client_id + lamport) → idempotent apply
  actor        uuid NOT NULL,                          -- pseudonymous principal (human or agent — same protocol)
  op_kind      op_kind NOT NULL,                       -- insert|delete|format|move|set_prop|block_ins|block_del|engine_promote
  payload      jsonb NOT NULL,                         -- the op (CAS delta in floor; Yrs update bytes when CRDT lands)
  pii_key_ref  text,                                   -- per-subject DEK if the op carries inline PII (11.4)
  applied_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id, op_seq),
  UNIQUE (tenant, page_id, op_id)                      -- idempotent apply: a re-delivered op is a no-op
);
CREATE INDEX doc_op_resume ON doc_op (tenant, page_id, op_seq);  -- the resume read: ops since the client's cursor

CREATE TABLE doc_snapshot (
  tenant       uuid NOT NULL, region text NOT NULL,
  page_id      uuid NOT NULL,
  snap_seq     bigint NOT NULL,                        -- the op_seq this snapshot includes up to
  blob_hash    text NOT NULL,                          -- content-addressed snapshot in the object tier (BLAKE3)
  named_label  text,                                   -- NULL = auto-compaction; set = a named version (restore point)
  created_at   timestamptz NOT NULL,
  PRIMARY KEY (tenant, page_id, snap_seq)
);
```

**Properties** (detail in [02 §2/§3](./02-internals-and-algorithms.md)):

- **`op_seq` is the resume cursor**, and is the **firehose `seq` for `scope = doc:<page_id>`** (OQ-J). A
  reconnecting client `resume(stream, scope=doc:<page_id>, last_seq=cursor)` and the transport backfills
  `(cursor, now]` from `doc_op` — **reconnect loses zero ops** (KD-1). If the cursor predates the retention/GC
  window, the client gets `resync_required` → a `knowledge.page.snapshot` replay (the cold path, §4.9 of the
  bus; the snapshot is sub-artifact-granular at block level).
- **`op_id` is deterministic** → `UNIQUE` makes apply **idempotent**; at-least-once redelivery is a no-op.
  Identical for CAS and CRDT — the transport's correctness substrate.
- **The snapshot is the one format serving three masters** (CR-B, carried forward): the `replay` source (emits
  `knowledge.page.snapshot` at block granularity), the history restore point, and the **crypto-shred unit
  boundary** (PII-bearing inline runs inside the snapshot are encrypted under the per-subject DEK, so
  destroying the key shreds the subject's content inside every snapshot without rewriting the immutable blob).
  The snapshot blob is per-tenant-DEK wrapped (K6).
- **Compaction cadence (measured floor):** op-count trigger (compact when live-tail rows exceed a budget),
  quiescence trigger (idle doc), named-version trigger (user "save version"/publish mints a non-GC'd snapshot).
  GC deletes `doc_op` rows ≤ `snap_seq` **except** those an open client's resume cursor still trails (the
  cursor is the GC watermark — the rule that makes KD-1 survive compaction). The exact N + idle window are
  measured thresholds (KQ-4).

---

## 4. The flexible database — over the FROZEN `myelin-query` shapes (contract 13.3, X-3/OQ-C)

### 4.1 The frozen `FieldType` enum (Knowledge stores + executes; the shape is shared, byte-identical with Issues)

```
FieldType =
  | text | rich_text(myelin-content) | number{ precision } | checkbox
  | select{ options:[Opt] } | multi_select{ options:[Opt] }
  | date{ has_time: bool } | datetime
  | principal      // user/agent/service ref
  | relation{ target_type: ArtifactType, cardinality: one|many }   // rides Refs cross-artifact, local index intra-collection
  | rollup{ via: FieldId(relation), target: FieldId, fn: RollupFn } // computed at READ TIME, never stored (KN-3)
  | formula{ expr: FormulaAst, result_type: FieldType }            // computed at READ TIME, never stored
  | url | email | phone | file(ArtifactRef) | created_at | updated_at | created_by | updated_by
Opt = { id: OpaqueId, label: String, color: Token }
```

Personal-data classification (`#[personal_data]`, contract 10.2) attaches **per field definition**, so a
`principal`/`email`/`text` field carrying PII is tagged at the schema level — this is how field-level erasure
and the field-level ABAC caveat (§5) find their targets. `rollup`/`formula` are read-time-computed **by
contract** ([02 §4.2](./02-internals-and-algorithms.md)).

> **Δ6 reconciliation note:** the Phase-4 Knowledge-local field-type list is replaced by this frozen enum.
> Knowledge owns its *executor* (the JSONB query lowering, the read-time formula engine); the *definitions* are
> identical to Issues' so a future shared render/CRDT path is uniform.

### 4.2 The structured collection (JSONB property bag + derived projection)

```sql
CREATE TABLE db_collection (
  tenant      uuid NOT NULL, region text NOT NULL,
  db_id       uuid NOT NULL, space_id uuid NOT NULL,
  name        text NOT NULL,
  field_defs  jsonb NOT NULL,                           -- the frozen myelin-query FieldType definitions (§4.1)
  PRIMARY KEY (tenant, db_id)
);

CREATE TABLE db_row (
  tenant      uuid NOT NULL, region text NOT NULL,
  db_id       uuid NOT NULL,
  row_id      uuid NOT NULL,                            -- the consumer id column for the SetExpr push-down (OQ-E)
  props       jsonb NOT NULL,                           -- THE PROPERTY BAG: { field_id → value } (source of truth)
  body_page   uuid,                                     -- a row IS a page (open-as-page) — its body block subtree
  order_key   text NOT NULL,                            -- the frozen LexoRank order_key (§2.5)
  version     bigint NOT NULL,                          -- CAS token for row edits
  contains_personal_data boolean NOT NULL DEFAULT false,
  data_role   text,
  pii_key_ref text,
  created_at  timestamptz NOT NULL,
  PRIMARY KEY (tenant, row_id)
);
-- The DERIVED indexable projection: GIN for the bulk + generated/expression-column indexes for MEASURED-hot facets.
CREATE INDEX db_row_props_gin ON db_row USING gin (props jsonb_path_ops);
-- A facet crossing the frozen >5% view-execution threshold (contract 6.3) gets a generated-column index,
-- provisioned off the bus (knowledge.database.schema.changed → expand→backfill→contract). Measured, not predicted.
```

JSONB is the **source of truth**; the **derived indexable projection** (GIN + per-measured-hot-facet
generated/expression indexes, maintained off the bus, *not* per-tenant DDL) serves filter/sort/group at scale.
Materialise a real columnar projection (the OLAP read store, contract 11.6) only when read-time recompute is
*measured* too slow (KN-3-style measured promotion).

### 4.3 The typed relation tables — TE-7 source of truth (contract 5.5, REF-1)

Knowledge owns the typed relation tables that are the **source of truth** for its lifecycle/semantic edges;
Refs holds a rebuildable projection. The same transaction that writes a typed row emits a typed lifecycle event
the Refs edge-builder consumes ([03 §3](./03-events-contracts-and-glue.md)).

```sql
CREATE TABLE db_relation (                              -- two-way relation field (FieldType::relation), source of truth
  tenant       uuid NOT NULL, region text NOT NULL,
  relation_id  uuid NOT NULL,
  src_row      uuid NOT NULL,                           -- Knowledge's own key (referential integrity)
  dst_ref      text NOT NULL,                           -- ArtifactRef of the other end (may be cross-subsystem)
  rel          db_rel NOT NULL,                         -- 'relates' | 'rollup_source'
  created_by   uuid NOT NULL, created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, relation_id),
  UNIQUE (tenant, src_row, dst_ref, rel),
  FOREIGN KEY (tenant, src_row) REFERENCES db_row(tenant, row_id) ON DELETE CASCADE
);

CREATE TABLE page_parent (                              -- page → sub-page; mirrored to Refs as a 'parent' lifecycle edge
  tenant       uuid NOT NULL, region text NOT NULL,
  page_id      uuid NOT NULL,
  parent_page  uuid NOT NULL,
  order_key    text NOT NULL,                           -- sibling order (the frozen LexoRank, §2.5)
  PRIMARY KEY (tenant, page_id),
  FOREIGN KEY (tenant, page_id)     REFERENCES page(tenant, page_id) ON DELETE CASCADE,
  FOREIGN KEY (tenant, parent_page) REFERENCES page(tenant, page_id)
);
```

Two-way relation consistency is maintained **transactionally for the forward edge** (the FK) and
**eventually-consistently for the inverse projection** in Refs (the best-effort bidirectional consistency
EI-04 §2 names; contract 5.5).

### 4.4 Views — the frozen `ViewSpec` + `QueryAst` (contract 13.3)

A **view** is the frozen `ViewSpec`: kind + filter (`QueryAst`) + group_by + sort + visible + `order_field`.

```
ViewSpec {
  kind:    table | board | calendar | timeline | gallery | list,
  filter:  QueryAst,                 // the shared AST; ALWAYS conjoined with list_objects (ADR-07)
  group_by:    Option<FieldId>,
  sort:    [ { field: FieldId, dir: asc|desc } ],   // the LAST-resort tiebreak is order_key
  visible: [FieldId],
  order_field: FieldId(order_key),    // the manual drag-order field (the frozen LexoRank)
}
QueryAst =
  | And([QueryAst]) | Or([QueryAst]) | Not(QueryAst)
  | Cmp { field: FieldPath, op: Op, value: Literal }
  | In  { field: FieldPath, values: [Literal] }
  | Has { field: FieldPath }                  // relation/multi-select membership
  | Text{ query: String, fields: [FieldPath] } // compiles to FT on the search backend
  | Ref { field: FieldPath, target: ArtifactRef }
Op = eq | ne | lt | lte | gt | gte | contains | starts_with | within
```

```sql
CREATE TABLE db_view (
  tenant      uuid NOT NULL, region text NOT NULL,
  view_id     uuid NOT NULL, db_id uuid NOT NULL,
  spec        jsonb NOT NULL,                           -- the frozen ViewSpec (kind/filter:QueryAst/group_by/sort/visible/order_field)
  shared      boolean NOT NULL DEFAULT true,
  PRIMARY KEY (tenant, view_id)
);
CREATE TABLE db_view_override (                          -- per-user personal tweaks layered on a shared view
  tenant uuid NOT NULL, view_id uuid NOT NULL, principal uuid NOT NULL,
  override jsonb NOT NULL,                               -- a partial ViewSpec diff
  PRIMARY KEY (tenant, view_id, principal)
);
```

Every view query **always conjoins the `list_objects` `Filter`** (the `SetExpr` push-down, [02 §4.1](./02-internals-and-algorithms.md))
so a viewer sees only rows they may read — never post-filtered. The `QueryAst` is the **same grammar that is
the bus `EventMatcher` core** (contract 3.4) — one grammar, multiple compile targets.

---

## 5. The page-tree ReBAC namespace fragment (contract 4.9 — Knowledge declares, Id compiles)

Knowledge **declares its ReBAC namespace fragment** to Id and **compiles the page tree to tuples** via the
Permission projector — no bespoke ACL (ADR-03). The fragment is *page-tree inheritance with overrides +
row-level + field-level caveat* (the canonical Zanzibar pattern):

```
definition space {                                    // a space maps to a project (contract 4.9)
  relation parent_project: project
  relation member: user | team#member
  permission read = member + parent_project->read
}
definition page {
  relation parent_page:   page                        // sub-page nesting (page_parent typed table, §4.3)
  relation parent_space:  space
  relation direct_reader: user | team#member          // explicit grant on this page
  relation direct_editor: user | team#member
  relation direct_block:  user | team#member          // the OVERRIDE: narrows inherited access (exclusion)
  relation watcher:       user | team#member          // Notif read-fanout (contract 4.9 watcher-per-watchable)
  permission read    = (parent_page->read + parent_space->read + direct_reader) - direct_block
  permission comment = read - direct_block
  permission edit    = direct_editor + parent_page->edit + parent_space->member - direct_block
  permission manage  = direct_editor + parent_page->manage
}
definition database_row {                             // row-level visibility (§5.1)
  relation parent_db: page
  relation row_reader: user | team#member             // group-grant "see only your team's rows"
  permission read = parent_db->read + row_reader      // + a CaveatContext for field-level (off the hot list_objects path)
  permission view_field = read                        // gated by the CaveatContext{field} at check-time (OQ-E)
}
```

`parent_page->read` is the **tuple-to-userset rewrite** that makes the whole tree's inheritance one rule;
`- direct_block` is Zanzibar's **exclusion** userset — a narrowed sub-page disappears from a reader's
`list_objects` **by construction**, not by a post-filter.

### 5.1 The permission-granularity split (rows = tuples; fields = `CaveatContext` caveat) — DECIDED (CR-D)

- **Row-level = tuples, pushed down via `InRelation`** (OQ-E). "See only your team's rows" is a
  `row_reader: team#member` **group grant** on the database (not one tuple per row); a single grant covers
  thousands of rows via tuple-to-userset rewrite. This stays **on** the `list_objects` push-down path — the
  `SetExpr` is `InRelation { relation: row_reader, via_column: db_row.id }`, lowered to a JOIN against the
  per-tenant authz reverse index ([02 §4.1](./02-internals-and-algorithms.md)). Per-row tuples are reserved
  for the rare explicit single-row grant (a bounded `Ids` inline).
- **Field-level = the frozen `CaveatContext` caveat, off the hot path** (OQ-E). "Hide the salary column" is a
  caveat on `view_field`, evaluated at `check`-time with `CaveatContext{ object, field, attrs }` on the
  **already-filtered, already-fetched rows** — never on the hot `list_objects` path (it would defeat the
  conjoin). Field-hiding is a render-time projection on the permitted page of rows.
- **Per-database opt-in UX:** row-level visibility is **off by default** (a database inherits page-level read);
  a db owner opts in per-database, which declares the `row_reader` grant rule (the UI writes the group tuples,
  not the owner hand-tupling rows).

> **Δ7 reconciliation note:** the field-level caveat is now the frozen `CaveatContext{object, field?,
> transition?, attrs}` (contract 4.2). The row-vs-field mechanism split (rows=tuples, fields=caveats) is
> unchanged from CR-D; it is now expressed against the frozen `SetExpr`/`CaveatContext` shapes.

---

## 6. The stateful-component register + blast-radius note

| # | Component | Engine | Holds | Shard key | Blast radius if it dies | Crypto-shred unit |
|---|---|---|---|---|---|---|
| K1 | **Block tree + page (OLTP)** | Postgres-class | blocks, pages, `outbox` | `(tenant, region)` + page_id | one tenant's docs; recoverable (outbox drains; reindex rebuilds derived) | per-tenant DEK; **per-subject DEK for free-text columns** (11.4) |
| K2 | **`db_collection`/`db_row`/`db_view`/`db_relation`/`page_parent`** | Postgres-class | structured data + typed edges (source of truth) | `(tenant, region)` + db_id | one tenant's databases | per-tenant DEK; per-subject for PII cells |
| K3 | **`doc_op` op-log** | Postgres-class (live tail) + object tier (snapshots) | the resume-cursor op-log; compacted snapshots | `(tenant, region)` + page_id | one doc's live session degrades; **reconnect replays from cursor — no op loss** (KN-1) | per-subject DEK on PII ops; snapshot blob = per-tenant DEK |
| K4 | **Collab session state (relay)** | in-memory + the op-log backing | live awareness/presence + in-flight op buffer | `(tenant, region, page_id)` | a relay crash → clients reconnect, `resume` from `op_seq` cursor (no loss) | ephemeral |
| K5 | **Consumer dedup ledgers** | Postgres-class | the consumer template's idempotency | `(tenant, consumer)` | re-process is idempotent → no loss | inherits K1 |
| K6 | **Media / snapshot blobs** | S3-compatible object store | images, files, CRDT snapshots (content-addressed) | `(tenant, region)` + hash | one tenant's media; OLTP rows point at it | per-tenant DEK (per-blob CK wrapped) |
| K7 | **Derived projection indexes** | Postgres (GIN/generated cols) | the flexible-DB measured-hot-facet projection | `(tenant, region)` + db_id | derived — rebuildable from K2 | inherits K2 |
| K8 | **Agent-trace documents (AG-7)** | Knowledge content (K1/K6) | content-addressed agent execution traces | `(tenant, region)` | an erasable holder; traces unavailable, runs continue | per-subject DEK |

The **page-tree ReBAC tuples are NOT Knowledge's component** — they live in Id's tuple store; Knowledge only
*projects* into them (§5). The **per-tenant authz reverse index** (the `list_objects` JOIN target, OQ-E) is
also Id's, not Knowledge's. All derived state (K7) is rebuildable by reindex-from-source. K3's whole point is
**no op loss on reconnect** — the resume-cursor drill (KD-1).

**Hot tables flagged for the `forward-only-migration` lint** (contract 1.5): `block`, `db_row`, `doc_op` —
schema changes use expand→backfill→contract, never a blocking `ALTER`.

Continue to [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md).
