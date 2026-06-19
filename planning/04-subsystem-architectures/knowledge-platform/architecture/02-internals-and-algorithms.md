# Knowledge Platform — 02 · Internals & Algorithms

> See [`00-overview.md`](./00-overview.md) for framing, [`01-tech-and-data-model.md`](./01-tech-and-data-model.md)
> for the schema. This doc is the algorithmic depth: the resume-cursor durable transport over the **frozen
> firehose `subscribe/resume/scope` protocol** (KN-1, built FIRST), the CAS floor → CRDT ladder, the
> block-tree storage algorithms over the **frozen LexoRank**, the flexible-DB query with the **frozen
> `SetExpr` push-down**, the read-time formula/rollup engine, and the one editor render path (KN-4). Each hard
> problem's full resolution + cited prior art is in [`05-hard-problems.md`](./05-hard-problems.md).

---

## 1. The collaboration stack, layered (the doctrine ladder)

```
   ┌─────────────────────────────────────────────────────────────────────────┐
   │ LAYER 3  MERGE         CAS floor (v1)  →  Yrs CRDT (promotion-triggered)  │  ← what merges concurrent edits
   ├─────────────────────────────────────────────────────────────────────────┤
   │ LAYER 2  AUTHORITY     server-side enforcement of what merge can't:       │  ← perms / schema / erasure
   │                        permission check on each op, schema validation,     │
   │                        erasure — the Collaboration/Sync Engine            │
   ├─────────────────────────────────────────────────────────────────────────┤
   │ LAYER 1  TRANSPORT     the RESUME-CURSOR DURABLE TRANSPORT (KN-1, FIRST)   │  ← reconnect loses zero ops
   │                        over firehose::subscribe/resume(scope=doc:<id>)     │
   └─────────────────────────────────────────────────────────────────────────┘
```

The **transport (Layer 1) is built first** — before any merge engine — because a real-time relay *without*
resume cursors silently loses the gap on a reconnect (EI-04 §2.2). The CRDT (or the CAS floor) **slots into**
this transport; swapping Layer 3 does not touch Layers 1–2.

---

## 2. Layer 1 — the resume-cursor durable transport (KN-1, over the FROZEN firehose protocol)

### 2.1 The protocol (conforms to contract 3.5 / OQ-J — Δ1)

The transport rides the **frozen firehose resume-cursor protocol** (contract 3.5): `firehose::subscribe(stream,
scope, cursor?)` / `firehose::resume(stream, scope, last_seq)`, with `stream = fan.<tenant>.knowledge` and
**`scope = doc:<page_id>`** (the bounded selector — a doc's block subtree, never `*`). Each frame carries a
per-`(stream, scope)` monotonic `seq` which **is** the doc's `op_seq` ([01 §3](./01-tech-and-data-model.md)).
The durable bus carries only the `knowledge.doc.updated` pointer; the collab op-stream never melts the durable
control bus (ADR-04.5). **Knowledge owns the resume-cursor + idempotent-apply discipline + the CRDT**; the bus
provides the seam (contract 3.5: "the collab transport's resume-cursor + idempotent-apply property and the
CRDT are Knowledge's deliverable").

```
CONNECT(page_id, cursor):
  1. authorize: Id.check(viewer, edit|comment, page_ref, zookie)            -- Layer 2 (no op without authz)
  2. resume:    sub := firehose.resume(stream=fan.<tenant>.knowledge,
                                       scope=doc:<page_id>, last_seq=cursor)  -- backfills (cursor, now] THEN live
                if resync_required (cursor < retention window):
                     load knowledge.page.snapshot (block-granular *.snapshot replay) THEN sub.live  -- cold path, named
                client applies the backfilled ops idempotently (op_id dedup), then receives live frames
  3. presence:  join the awareness channel (ephemeral; NOT persisted)

SEND_OP(page_id, op):                                                       -- a client emits an edit
  1. authorize the op (Layer 2)
  2. assign op_seq (per-doc monotonic, == the firehose seq), op_id = (client_id, lamport)
  3. PERSIST: INSERT INTO doc_op ... ON CONFLICT (tenant,page_id,op_id) DO NOTHING  -- IDEMPOTENT APPLY
  4. apply to the live doc state (Layer 3 merge); bump block.version (CAS)
  5. firehose.publish(scope=doc:<page_id>, frame{seq=op_seq, op})           -- fan out to other subscribers
  6. coalesce → emit knowledge.doc.updated (pointer) + semantic event via OUTBOX (debounced, §7)

RECONNECT (dropped connection):
  client re-runs CONNECT(page_id, last_durably_applied_op_seq)
  → firehose.resume replays EXACTLY (last_seq, now]; the UNIQUE(op_id) makes re-sends of in-flight ops no-ops
  → ZERO ops lost, ZERO duplicate effects   ← the KN-1 drill (KD-1)
```

### 2.2 Why this is correct (the cited mechanism)

- **`op_seq`/`seq` is a durable, per-`(stream, scope)` monotonic cursor.** A reconnecting client never guesses
  "what did I miss?" — it states its `last_seq` and `resume` returns exactly `(last_seq, now]`. This is the
  log-as-source-of-truth pattern (Kreps, *The Log*, 2013) applied to one doc: the op-log is authoritative, the
  live state a projection. The `resync_required` → `*.snapshot` fallback is the named cold-rebuild path
  (contract 3.5 / §4.9 of the bus), never a silent gap.
- **Idempotent apply on a deterministic `op_id`.** `op_id = (client_id, lamport)` is collision-free per client
  and re-derivable; `UNIQUE(tenant, page_id, op_id)` makes a re-delivered op a no-op (Helland, *Idempotence Is
  Not a Medical Condition*, 2012). At-least-once delivery + idempotent apply ≈ effectively-once — the same
  posture the bus consumer template uses (contract 2.4/2.5), so a flaky network or relay restart cannot
  double-apply or drop.
- **Bounded scope (the head-of-line + cost discipline, OQ-J).** `scope = doc:<page_id>` is the bounded selector
  the frozen protocol mandates; the transport rejects an unbounded scope (the whitelist-not-`*` rule
  generalised to the firehose). A huge doc **paginates its scope** to the visible block window + a margin, so a
  giant doc does not stream every block's live frames to one client (the KD-8 hot-document discipline).
- **The firehose, not the durable bus.** Op frames ride the firehose tier; the durable bus gets only the
  coalesced `knowledge.doc.updated` pointer + the semantic `knowledge.page.updated`. An agent is **never woken
  per op** (it subscribes to curated Signals, contract 3.1, not the op-stream).
- **Agents use the identical protocol.** A mock or real agent editing a doc calls `SEND_OP` exactly as a human
  (the "collaborator client" interface is peer-agnostic), so attribution/undo/history treat agent edits as
  first-class.

### 2.3 Presence / awareness (ephemeral, never persisted)

Cursors, selections, "who's here" ride the **ephemeral presence tier** of the firehose (at-most-once is fine),
throttled, **not persisted**. A coarse `knowledge.read_state`-style summary is the only durable trace. Presence
is residency-pinned (the session is in the doc's cell, [05 §9](./05-hard-problems.md)).

---

## 3. Layers 2–3 — the merge ladder: CAS floor → CRDT (TE-15, KN-1)

### 3.1 Layer 2 — the authority the merge layer cannot enforce

The Collaboration/Sync Engine runs on **every incoming op**, before merge:

- **Permission check** — `Id.check(actor, edit|comment, block/page_ref, zookie)`. A revoked editor's op is
  rejected (the zookie carries read-your-writes; a just-revoked grant cannot be read stale, contract 4.10 — the
  zookie is stamped on `page.acl_zookie` at the last ACL change).
- **Schema validation** — a db-row op must satisfy the `FieldType` definitions (contract 13.3); a malformed op
  is rejected, not merged.
- **Erasure** — an op touching erased content degrades (the `*.erased` tombstone consumer, [03 §6](./03-events-contracts-and-glue.md)).

CRDTs guarantee *convergence, not application-level invariants* — so these live **above** the merge layer.

### 3.2 Layer 3a — the CAS floor (v1, the named floor that does not merge)

Per-block **optimistic compare-and-swap** on `block.version`:

```
EDIT_BLOCK(block_id, expected_version, new_inline, new_props):
  UPDATE block SET inline=?, props=?, version=version+1, edited_by=?, edited_at=now()
    WHERE tenant=? AND block_id=? AND version = expected_version       -- the CAS guard
  if rows_affected == 0:                                               -- a concurrent writer won
     return Conflict { current: <the server's current block state> }   -- the loser RECONCILES, never silently overwritten
```

- **Guarantee: no *silent* overwrite** (EI-04 §2.1). On a precondition miss the loser is rejected and handed
  the current server state to reconcile. It **does not merge** — concurrent editors of the *same block* get a
  conflict, not a blend. Named as a floor, layered with **advisory soft-locks** ("someone is editing this
  block," over the awareness channel) and **snapshot/restore**.
- **Different blocks edit freely in parallel** — the CAS guard is per-block, so two people editing different
  paragraphs never conflict. The `rows_affected == 0` rate is the **CRDT-promotion trigger metric** (§3.4).

### 3.3 Layer 3b — the CRDT promotion (Yrs; triggered by the first true concurrent-edit conflict)

**Promotion trigger (R5, named): the first true concurrent-edit conflict** — the per-doc CAS conflict rate
(fraction of `EDIT_BLOCK` ops that miss the precondition) crossing a measured threshold (KQ-1), or sustained
multi-author simultaneous presence on one block. The CRDT then slots into the transport from §2:

- **Per-block content CRDT** (Yrs `Y.Text`/`Y.XmlFragment`) for inline runs + **a tree/move CRDT**
  (Kleppmann's move operation) for block structure — the hybrid granularity. **Rich-text marks** across
  concurrent edits → **Peritext** is the reference research.
- **The op-log becomes Yrs updates.** `doc_op.payload` carries Yrs update bytes; the transport (§2) is
  unchanged — the resume cursor, idempotent apply, and op-log are identical. This is *why* the transport is
  built first: the CRDT is a Layer-3 swap, not a rewrite (KN-1).
- **Server stays a "dumb relay + persistence + authority"** — it persists ops, relays them, enforces Layer 2 —
  it does not transform (the OT burden it avoids). Yrs being Rust-native keeps this in-process.

### 3.4 The online CAS→CRDT migration (per-doc, no stop-the-world — CR-F)

Because the transport is **engine-agnostic** (the op-log, `op_seq` cursor, and idempotent apply are identical
for CAS and Yrs), promotion is a **Layer-3 swap on one doc at a time**, run as an ordinary op sequence:

1. **Quiesce-lite**: at a compaction boundary, snapshot the doc's current materialised state.
2. **Seed**: construct the Yrs document deterministically from that snapshot (block tree → move-CRDT; each
   block's inline md-subset string → `Y.Text`). Deterministic ⇒ reproducible + replay-safe.
3. **Cutover op**: append a single `engine_promote` op at the next `op_seq`. From that `op_seq` forward,
   `doc_op.payload` carries Yrs update bytes; before it, CAS deltas. A reconnecting client `resume`s across the
   boundary, loads the seeded Yrs state once, applies the tail — **zero ops lost**.
4. In-flight CAS edits straddling the cutover reconcile via the last CAS conflict check (no silent drop); the
   editor client (CRDT-ready from day one, KN-4) switches its merge module at the boundary with no schema
   change.

**Reversibility:** the step-1 snapshot predates the cutover, so a botched promotion rolls back to it (forward
history). **Drill:** KD-1 is re-run *across* an `engine_promote` boundary.

### 3.5 LexoRank rebalancing under the CRDT (CR-A interaction)

On the CAS floor, the frozen 2-char jitter ([01 §2.5](./01-tech-and-data-model.md)) keeps concurrent same-gap
inserts from colliding; the 48-char rebalance is a lazy per-sibling-list `myelin-flow` activity emitted as
`move` ops (idempotent, replayable). Once the move-CRDT lands, **the CRDT's list type owns sibling ordering**
and the `order_key` becomes a *derived* OLTP-index ordering hint recomputed from CRDT state — the bespoke
jitter/rebalance logic retires. **Drill:** KD-8 includes a concurrent-same-gap-insert storm asserting no
key-collision reorder and bounded rebalance cost.

### 3.6 The correctness bar regardless of engine (KN-4)

Whatever Layer 3 is, **`render(parse(md)) === md` over a corpus is a hard gate** (§8). The concurrency engine
changes how ops merge; it never changes that a serialized doc round-trips losslessly.

---

## 4. The flexible-DB query model + the read-time formula/rollup engine (over the frozen shapes, KN-3)

### 4.1 Flexible-field query execution with the FROZEN `SetExpr` push-down (OQ-E — Δ7)

The JSONB property bag is the source of truth; query execution conjoins the **frozen `list_objects` `Filter`**:

```
VIEW_QUERY(db_id, view, viewer):
  1. result := Id.list_objects(viewer, read, 'database_row', zookie)       -- Ids{...} | Filter{set_expr, zookie}
  2. acl    := lower_set_expr(result.set_expr OR result.ids) over db_row.id  -- the FROZEN lowering (below)
  3. ast    := compile(view.spec.filter + view_override[viewer])           -- the frozen QueryAst (ADR-07)
  4. sql    := lower(ast)                                                   -- predicates → JSONB ops over `props`
                  · measured-hot facets → the generated/expression-column index
                  · cold facets → a GIN jsonb_path_ops scan, bounded + paginated
                  AND acl                                                   -- CONJOINED (permission by construction)
  5. rows   := execute(sql) LIMIT page                                      -- paginated; row-capped; statement-timeout
  6. fields := for each row, apply CaveatContext{field} hiding (off the hot path — §5 of doc 01)
  7. compute read-time formulas/rollups for the page of rows (§4.2)        -- never stored
  8. return
```

**The frozen `SetExpr` lowering over `db_row.id`** (the consumer's own id column, contract 4.3 / OQ-E):

| `SetExpr` | Lowering |
|---|---|
| `All` | no ACL conjunct (the viewer can read the whole db via page-level inheritance — the common case) |
| `None` | `WHERE false` (deny) |
| `Ids(v)` / `NotIds(v)` | `db_row.id IN (...)` / `NOT IN (...)` (inlined when under the cardinality cap — the rare explicit per-row grant) |
| `InRelation { relation: row_reader, via_column: db_row.id }` / `TupleSet { index }` | `... JOIN authz_visible av ON av.object_id = db_row.id AND av.subject = $viewer AND av.relation = read` — a JOIN against the **per-tenant, residency-pinned authz reverse index** Identity maintains (the SpiceDB/Zanzibar reverse-index / `LookupResources` pattern realised as a co-located JOIN target). One query, **no N+1, no post-filter.** |
| `Union/Intersect/Difference` | the boolean composition compiled to `AND`/`OR`/`EXCEPT` |

- **The row-restricted case** (CR-D opt-in) lowers to `InRelation { relation: row_reader, via_column: db_row.id }`
  → the JOIN — index-served, not a scan; closing the **count-leak** (even a `COUNT` over the view is
  permission-correct because the ACL conjunct is *inside* the query, KD-5).
- **Consistency:** the returned zookie bounds staleness; a security-sensitive scan passes the zookie so the
  JOIN reads the tuple index at-or-after the zookie's revision (read-your-writes — a just-revoked grant is
  reflected, contract 4.10).

> **Δ7 reconciliation note:** the Phase-4 "`Filter{set_expr}` facet-expressible" is now the concrete frozen
> `SetExpr` + the named `authz_visible` JOIN target. The mechanism (no post-filter, conjoined into the SQL) is
> unchanged from CR-E; it now uses the frozen reverse-index JOIN, the same mechanism all five subsystems share.

### 4.2 The read-time formula/rollup engine (KN-3 — never stored, by contract)

**Decision (contract 13.3: `rollup`/`formula` are "computed at READ TIME, never stored"):** the engine is a
**bounded dependency-graph evaluator** (the spreadsheet model) run per page of rows:

```
COMPUTE_ROW(row, formula_field):
  deps := static_dependency_set(formula_field.expr)        -- which props/relations this formula reads
  for each dep:
     if dep is a property → read row.props[dep]
     if dep is a rollup over a relation → aggregate over db_relation(src_row=row, rel='rollup_source'):
          targets := the related rows/artifacts (permission-filtered via list_objects)
          value   := RollupFn(targets.map(target.props[rollup_target]))   -- COUNT/SUM/MIN/MAX/... at read time
     if dep is itself a formula → recurse (depth-bounded; cycle-detected)
  evaluate the FormulaAst (the bounded myelin-query expression core — no UDFs/loops/recursion, statically cost-bounded)
  return the value (NOT written back to db_row)
```

- **Read-time, never stored** (KN-3): editing one cell does **not** cascade a stored recompute across many
  rows/databases (the Notion scaling pain). The cost is paid at read, bounded to the page of rows, permission-
  filtered (rollups over related sets conjoin `list_objects`).
- **Cycle detection + fan-out caps:** the dependency graph is walked depth-bounded with a visited-set; a cycle
  surfaces as `#CYCLE` (a diagnostic cell), never an infinite loop. The `FormulaAst` is the **bounded
  `myelin-query` expression core** — the same cost-bounded discipline as the bus `EventMatcher` (contract 3.4).
- **The named materialised follow-on:** when a rollup over a *large* related set is *measured* too slow, that
  specific rollup is promoted to an **incrementally-maintained materialised aggregate** fed off the bus
  (`knowledge.row.updated` deltas) — the OLAP read store (contract 11.6). Per-rollup measured promotion, not a
  wholesale switch.
- **Eventual consistency, stated:** a rollup reflects the related rows as of the read; cross-database
  relation/rollup propagation is eventual (the Refs inverse-edge projection lags the typed table).

---

## 5. Permission-filtered reads everywhere (the cross-cutting invariant)

Backlinks, database views, search results, and embedded-view contents are **pre-filtered by the frozen
`list_objects` `Filter`, never post-filtered** (ADR-03). A leak is both a security and a GDPR breach.

- **Database views / rows** — the `SetExpr` conjoined into the SQL (§4.1).
- **Backlinks panel** — `Refs.backlinks(page_ref, viewer)` is leak-free by construction (contract 5.3): you see
  a backlink iff you can see the artifact that made the reference.
- **Embedded live views / `embed` / `sync_block`** — resolve via the *owning* subsystem's `project(ref, viewer)`
  (contract 5.2/5.6), per-viewer permission-checked; a confidential issue degrades to a tombstone via the
  4-step ladder, never leaks.
- **Search** — Search conjoins the **same** `list_objects` `Filter` before scoring (contract 6.1, the
  `search-requires-acl-filter` lint).
- **The page-tree ACL with overrides** compiles to tuples (§5 of doc 01); a `direct_block` exclusion makes a
  narrowed sub-page disappear from a reader's `list_objects` **by construction**.

---

## 6. Search indexing granularity (DECIDED)

**Decision: index at BOTH page-level and block-level**, with page as the default unit and block-level for
jump-to-block + heading anchors (the `h<opaqueid>` `#sub` targets). Knowledge declares two `IndexSpec`s
(contract 6.3): a page doc (title + concatenated body, language-tagged) and a per-significant-block doc
(heading/callout/code blocks that are useful jump targets).

- **Semantic/vector in v1: YES** (contract 6.3 names KN "vector-in-v1") — Knowledge is the prime RAG corpus;
  the embedding adapter is the swappable EU-hostable adapter Search owns. Embeddings of personal data are
  personal data → erased with their source (purged on `knowledge.*.erased`, contract 10.1). HYOK
  `can_derive_plaintext_index()=false` **structurally** skips indexing (contract 11.3).
- **Multilingual** (EU): per-language analyzers — Knowledge tags each page's `lang`.
- **Permission-aware** via the `Filter` conjoin (contract 6.1) — no leak via search.

Knowledge **never indexes itself** — it `project`s text and Search consumes off the bus (no cross-DB).

---

## 7. Event coalescing (semantic events, never keystrokes)

The Indexing/Outbox feeder **coalesces** high-frequency edit ops into **semantic** events:

```
on edit ops for page P:
  raw ops               → stay in the firehose collab tier (§2), NEVER the durable bus
  debounce window (e.g. 2–5s inactivity, or block-boundary) → emit ONE knowledge.page.updated (semantic)
  the durable bus gets: knowledge.doc.updated (pointer, for live-embed invalidation) + knowledge.page.updated
```

- **Never per-keystroke / per-op on the durable bus** (ADR-04.5). Search/Refs/Notif/OLAP react to the semantic
  event.
- **Reference edges are NOT coalesced away**: a new `mention`/`artifact_ref`/`embed` node emits
  `refs.edge.created` immediately on persist ([03 §1.4](./03-events-contracts-and-glue.md)) — a discrete fact.
- **Emitted via the outbox only** (contract 2.2) in the same transaction as the block/row write; causality
  correct-by-construction (`emit(draft, cause)`).

---

## 8. The one editor render path (KN-4 / EI-05 §2 / contract 13.1 WASM target)

The editor obeys the day-one mandate: **read and edit run the same inline parser; controlled `contenteditable`
(not `<textarea>`); caret = char offset into serialized markdown.** The **`render(parse(md)) === md` round-trip
over a corpus is a hard CI gate** (the correctness bar for TE-15 whatever the concurrency engine).

### 8.1 The shared Rust core (one parser, client + server)

The parser/serializer/offset model is the **Rust `myelin-content` core compiled to WASM** and reused
server-side (contract 13.1). This eliminates the two-divergent-renderers trap *structurally* — one
`parseInline`/`serializeInline` implementation, not a server one and a client one that drift.

```
parse(md_string)        → inline AST (runs + the structured mention/artifact_ref/embed nodes, U+FFFC-anchored)
serialize(inline_AST)   → md_string
render(parse(md)) === md   -- the corpus gate (KD-2): every fixture round-trips losslessly
```

### 8.2 The primitives shipped + unit-tested standalone (before the integrated editor)

Three modules ship and are unit-tested **standalone** before the integrated editor (R3):

1. **The serializer** — inline AST ↔ markdown-subset string, with `mention`/`artifact_ref`/`embed` as
   **structured nodes** (U+FFFC placeholder + positional `inline_nodes`, [01 §2.2](./01-tech-and-data-model.md))
   so reference-extraction is a node-array walk.
2. **The offset model** — the caret is a **character offset into the serialized markdown**, bridged to/from DOM
   positions; a structured node is one caret position (one `U+FFFC` code point). Controlled `contenteditable`
   intercepts structural input, lets plain text through, normalizes on serialize.
3. **The DOM-surgery module** — **Enter-splits-a-block** and **caret-placement-after-split** (the #1 "this
   isn't a real editor" tells) are their own designed, unit-tested module. Browser variance (Enter/IME/paste)
   is the named top Knowledge risk.

### 8.3 Why a markdown-subset string, not inline-range JSON

(KN-2; EI-05 §2): it survives copy/paste, export, diff, and reference-extraction; needs no server-side
sanitisation pass; and survives an editor rewrite with zero schema migration. The structural nodes stay
structured so the reference grammar is reliable server-side. This is the frozen `myelin-content` inline
grammar (contract 13.1): **AST for block structure, markdown-subset string for inline runs**, the three
structured nodes preserved.

---

## 9. Agent edits flow through the same collab protocol (ADR-08, the four uniform sandbox guarantees)

A mock/real agent that edits a doc goes through `SEND_OP` (§2) exactly as a human — so attribution ("suggested
by agent"), undo, and history treat agent edits as first-class. The agent never mutates Knowledge's DB
directly: a side-effecting tool (`knowledge.page.append`, `knowledge.row.upsert`) goes through the Agent
Fabric's **`EffectApi::apply`** (plan-then-apply, contract 8.2), which enforces schema → capability →
delegation → tenant → budget → **HITL gate** → apply-via-public-endpoint → meter, then calls Knowledge's
**public endpoint** as the agent principal (same gateway, no carve-out), which applies the op through the
collab protocol. The **four uniform guarantees** (contract 8.4, X-6) apply by construction: the universal
**cost gate** (reserve/settle, contract 11.7), the **per-run attenuated token** attribution (contract 4.7),
the **HITL withhold** (a gated tool not in the approved set returns `Denied` and does not mutate, AG-8), and
the **isolation floor + escape drill** (any `compute` the tool runs is the CI runner's `kind=agent` job). The
*same mock code* runs deterministically in dev (`--use-mock`) and an `LlmAgentRuntime` later with zero platform
changes (ADR-08). See [03 §5](./03-events-contracts-and-glue.md) for the tool registrations and the AG-7 trace.

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md).
