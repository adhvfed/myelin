# Knowledge Platform — 02 · Internals & Algorithms

> See [`00-overview.md`](./00-overview.md) for framing, [`01-tech-and-data-model.md`](./01-tech-and-data-model.md)
> for the schema. This doc is the algorithmic depth: the resume-cursor durable transport (KN-1, built first),
> the CAS floor → CRDT ladder, the block-tree storage algorithms, the flexible-DB query model, the read-time
> formula/rollup engine, and the one editor render path (KN-4). Each hard problem's full resolution + cited
> prior art is in [`05-hard-problems.md`](./05-hard-problems.md); this doc gives the mechanism.

---

## 1. The collaboration stack, layered (the doctrine ladder)

The collaboration architecture is **three layers, built bottom-up** (KN-1; EI-04 §2), each independently
correct and drilled:

```
   ┌─────────────────────────────────────────────────────────────────────────┐
   │ LAYER 3  MERGE         CAS floor (v1)  →  Yrs CRDT (promotion-triggered)  │  ← what merges concurrent edits
   ├─────────────────────────────────────────────────────────────────────────┤
   │ LAYER 2  AUTHORITY     server-side enforcement of what merge can't:       │  ← perms / schema / erasure
   │                        permission check on each op, schema validation,     │
   │                        erasure — the Collaboration/Sync Engine            │
   ├─────────────────────────────────────────────────────────────────────────┤
   │ LAYER 1  TRANSPORT     the RESUME-CURSOR DURABLE TRANSPORT (KN-1, FIRST)   │  ← reconnect loses zero ops
   │                        idempotent apply, durable op-log, op_seq cursor     │
   └─────────────────────────────────────────────────────────────────────────┘
```

The **transport (Layer 1) is built first** — before any merge engine — because a real-time relay *without*
resume cursors silently loses the gap on a reconnect, and that floor masquerading as done is the failure
(EI-04 §2.2). The CRDT (or the CAS floor) **slots into** this transport; swapping Layer 3 does not touch
Layers 1–2.

---

## 2. Layer 1 — the resume-cursor durable transport (KN-1, the FIRST thing built)

### 2.1 The protocol

A client maintains a **resume cursor** = the last `op_seq` it has durably applied (the per-doc monotonic
counter, [01 §3](./01-tech-and-data-model.md)). The transport rides the **bus firehose seam** (event-bus
§4.3/§5.5: `firehose::publish/tail`), with the durable bus carrying only the `knowledge.doc.updated` pointer
event — the collab op-stream never melts the durable control bus (ADR-04.5).

```
CONNECT(page_id, cursor):
  1. authorize: Id.check(viewer, edit|comment, page_ref, zookie)            -- Layer 2 (no op without authz)
  2. resume:    ops := SELECT * FROM doc_op WHERE page_id=? AND op_seq > cursor ORDER BY op_seq  -- the GAP
                send ops to the client (catch up); client applies idempotently (op_id dedup)
  3. subscribe: firehose.tail(stream=collab:<page_id>, range=from cursor)   -- live ops from here on
  4. presence:  join the awareness channel (ephemeral; NOT persisted)

SEND_OP(page_id, op):                                                       -- a client emits an edit
  1. authorize the op (Layer 2)
  2. assign op_seq (per-doc monotonic), op_id = (client_id, lamport)        -- deterministic id
  3. PERSIST: INSERT INTO doc_op ... ON CONFLICT (tenant,page_id,op_id) DO NOTHING  -- IDEMPOTENT APPLY
  4. apply to the live doc state (Layer 3 merge); bump block.version (CAS)
  5. firehose.publish(collab:<page_id>, op)                                 -- fan out to other clients
  6. coalesce → emit knowledge.doc.updated (pointer) + semantic event via OUTBOX (debounced, §7)

RECONNECT (dropped connection):
  client re-runs CONNECT(page_id, last_durably_applied_op_seq)
  → step 2 replays EXACTLY the missed ops; the UNIQUE(op_id) makes re-sends of in-flight ops no-ops
  → ZERO ops lost, ZERO duplicate effects   ← the KN-1 drill (T-5; [07])
```

### 2.2 Why this is correct (the cited mechanism)

- **`op_seq` is a durable, per-doc monotonic cursor.** A reconnecting client never asks "what did I miss?"
  guessing — it states its cursor and the transport returns exactly `op_seq > cursor`. This is the
  log-as-source-of-truth pattern (Kreps, *The Log*, 2013) applied to a single doc: the op-log is
  authoritative, the live state is a projection.
- **Idempotent apply on a deterministic `op_id`.** `op_id = (client_id, lamport_counter)` is collision-free
  per client and re-derivable; `UNIQUE(tenant, page_id, op_id)` makes a re-delivered op a no-op (Helland,
  *Idempotence Is Not a Medical Condition*, 2012). At-least-once delivery + idempotent apply ≈
  effectively-once — the same posture the bus consumer template uses (substrate §5), so **a flaky network or
  a relay restart cannot double-apply or drop**.
- **The firehose, not the durable bus.** Op frames ride `firehose::publish/tail` (a separate transport,
  event-bus §4.3); the durable bus gets only the coalesced `knowledge.doc.updated` pointer. An agent is
  **never woken per op** (EI-03 §6.1). The durable record is `doc_op`; the firehose is the live fan-out.
- **Agents use the identical protocol.** A mock or real agent editing a doc calls `SEND_OP` exactly as a
  human client does (deep-dive §6), so attribution, undo, and history treat agent edits as first-class — the
  "collaborator client" interface does not care whether the peer is human or agent.

### 2.3 Presence / awareness (ephemeral, never persisted)

Cursors, selections, "who's here" ride the **ephemeral NATS-core firehose** (event-bus §4.3, at-most-once is
fine), throttled, **not persisted** (deep-dive §6). A coarse `knowledge.read_state`-style summary is the only
durable trace. Presence is residency-pinned (the session is in the doc's cell, [05 §9](./05-hard-problems.md)).

---

## 3. Layers 2–3 — the merge ladder: CAS floor → CRDT (TE-15, KN-1)

### 3.1 Layer 2 — the authority the merge layer cannot enforce

The Collaboration/Sync Engine is the **authority for what a CRDT/CAS cannot enforce** (deep-dive §6) and runs
on **every incoming op**, before merge:

- **Permission check** — `Id.check(actor, edit|comment, block/page_ref, zookie)`. A revoked editor's op is
  rejected (the zookie carries read-your-writes; a just-revoked grant cannot be read stale, Id §8.4).
- **Schema validation** — a db-row op must satisfy the field definitions (ADR-06); a malformed op is
  rejected, not merged.
- **Erasure** — an op touching erased content degrades (the `*.erased` tombstone consumer, [03 §6](./03-events-contracts-and-glue.md)).

CRDTs guarantee *convergence, not application-level invariants* (deep-dive §6) — so these live **above** the
merge layer, always.

### 3.2 Layer 3a — the CAS floor (v1, the named floor that does not merge)

Per-block **optimistic compare-and-swap** on `block.version` (the last-modified token, [01 §2](./01-tech-and-data-model.md)):

```
EDIT_BLOCK(block_id, expected_version, new_inline, new_props):
  UPDATE block SET inline=?, props=?, version=version+1, edited_by=?, edited_at=now()
    WHERE tenant=? AND block_id=? AND version = expected_version       -- the CAS guard
  if rows_affected == 0:                                               -- a concurrent writer won
     return Conflict { current: <the server's current block state> }   -- the loser RECONCILES, never silently overwritten
```

- **Guarantee: no *silent* overwrite** (EI-04 §2.1). On a precondition miss the loser is rejected and handed
  the current server state to reconcile. It **does not merge** — concurrent editors of the *same block* get a
  conflict, not a blend. This is named as a floor (VISION §3), layered with:
  - **Advisory soft-locks** — a "someone is editing this block" presence marker (over the awareness channel)
    that *reduces* conflicts without being a hard lock (a hard lock fails the liveness goal).
  - **Snapshot / restore** — named version snapshots ([01 §3](./01-tech-and-data-model.md)) so any conflict
    is recoverable.
- **Different blocks edit freely in parallel** — the CAS guard is per-block, so two people editing different
  paragraphs never conflict. Conflicts are confined to the same-block, same-instant case the floor honestly
  does not blend.

### 3.3 Layer 3b — the CRDT promotion (Yrs; triggered by the first true concurrent-edit conflict)

**Promotion trigger (R5, named — not vague "v2"): the first true concurrent-edit conflict** measured in
production (two editors of the same block colliding often enough that the CAS conflict rate crosses a
threshold). The CRDT is then slotted into the transport from §2:

- **Per-block content CRDT** (Yrs `Y.Text`/`Y.XmlFragment`) for inline runs + **a tree/move CRDT**
  (Kleppmann's move operation) for block structure — the **hybrid granularity** (deep-dive §6): a per-block
  content CRDT scales better than one doc-blob CRDT and enables block-level features; the move CRDT handles
  concurrent re-parenting without cycles (last-writer-wins-with-cycle-break).
- **Rich-text marks across concurrent edits** → **Peritext** is the reference research (deep-dive §6); marks
  reconcile without losing formatting under concurrency.
- **The op-log becomes Yrs updates.** `doc_op.payload` carries Yrs update bytes instead of CAS deltas; the
  transport (§2) is unchanged — the resume cursor, idempotent apply, and op-log are identical. This is *why*
  the transport is built first: the CRDT is a Layer-3 swap, not a rewrite (KN-1; [05 §1](./05-hard-problems.md)).
- **Server stays a "dumb relay + persistence + authority"** (deep-dive §6): it persists ops, relays them,
  and enforces Layer 2 — it does not transform (the OT burden it avoids). Yrs being Rust-native keeps this
  in-process.

### 3.4 The correctness bar regardless of engine (KN-4)

Whatever Layer 3 is, the **editor round-trip gate `render(parse(md)) === md` over a corpus** is the
correctness bar (EI-05 §2; D10) — see §8. The concurrency engine changes how ops merge; it never changes
that a serialized doc round-trips losslessly.

---

## 4. The flexible-DB query model + the read-time formula/rollup engine (TE-17/TE-18, KN-3)

### 4.1 Flexible-field query execution (TE-17)

The JSONB property bag is the source of truth ([01 §4](./01-tech-and-data-model.md)); query execution:

```
VIEW_QUERY(db_id, view, viewer):
  1. filter  := Id.list_objects(viewer, read, 'database_row', zookie)      -- the leak-free pre-filter (ADR-03)
  2. ast     := compile(view.query_ast + view_override[viewer])            -- shared myelin-query AST (ADR-07)
  3. sql     := lower(ast)                                                 -- predicates → JSONB ops over `props`
                  · hot facets → the generated/expression-column index (db_row hot-facet projection)
                  · cold facets → a GIN jsonb_path_ops scan, bounded + paginated
                  + acl_clause(filter)                                     -- CONJOINED (permission by construction)
  4. rows    := execute(sql) LIMIT page                                    -- paginated; row-capped; statement-timeout (X-3)
  5. compute read-time formulas/rollups for the page of rows (§4.2)        -- never stored
  6. return
```

- **Derived projection, not per-tenant DDL** (the TE-17 resolution, [05 §3](./05-hard-problems.md)): a field
  that is *measured* to be frequently filtered/sorted gets a generated/expression-column index **provisioned
  off the bus** (a `knowledge.database.schema.changed` event triggers the projection feeder to add the index
  via expand→backfill→contract, substrate §9). The bulk stays JSONB; the *hot* facets get columnar speed
  without DDL-per-database sprawl.
- **Always permission-pre-filtered** — `list_objects` conjoined before scoring (ADR-03; never post-filter,
  the security+GDPR breach SC-1).

### 4.2 The read-time formula/rollup engine (TE-18, KN-3 — never stored)

**Decision (KN-3): formulas and rollups are computed at READ TIME, never stored; materialise only when
read-time recompute is measured too slow.** This inverts the earlier materialised-first lean to the
doctrine's floor-first (decision-record §(c) D11/TE-18). The engine is a **bounded dependency-graph
evaluator** (the spreadsheet model, deep-dive §2.4) run per page of rows:

```
COMPUTE_ROW(row, formula_field):
  deps := static_dependency_set(formula_field.expr)        -- which props/relations this formula reads
  for each dep:
     if dep is a property → read row.props[dep]
     if dep is a rollup over a relation → aggregate over db_relation(src_row=row, rel='rollup_source'):
          targets := the related rows/artifacts (permission-filtered via list_objects)
          value   := agg_fn(targets.map(target.props[rollup_field]))      -- COUNT/SUM/MIN/MAX/etc. at read time
     if dep is itself a formula → recurse (depth-bounded; cycle-detected)
  evaluate the expression (the safe myelin-query expression core — no Turing-completeness, statically cost-bounded, AG-7)
  return the value (NOT written back to db_row)
```

- **Read-time, never stored** (KN-3): editing one cell does **not** cascade a stored recompute across many
  rows/databases (the Notion scaling pain, deep-dive §2.4). The cost is paid at read, bounded to the page of
  rows being rendered, permission-filtered.
- **Cycle detection + fan-out caps** (mirrors ADR-08 loop governance): the dependency graph is walked
  depth-bounded with a visited-set; a formula cycle (A references B references A) surfaces as a **diagnostic
  cell value** (`#CYCLE`), never an infinite loop. The expression language is the **safe myelin-query
  predicate/expression core** (ADR-07; AG-7) — no UDFs, no loops, statically cost-bounded, so a crafted
  formula cannot DoS a render (substrate §7.5).
- **The named materialised follow-on** (KN-3 promotion trigger): when a rollup over a *large* related set is
  *measured* too slow at read time, that specific rollup is promoted to an **incrementally-maintained
  materialised aggregate** fed off the bus (`knowledge.row.updated` deltas) — the OLAP read store (Storage
  §3.4). Until measured, read-time is the design. **This is a per-rollup measured promotion, not a wholesale
  switch** ([05 §4](./05-hard-problems.md)).
- **Eventual consistency, stated** (deep-dive §2.4): a rollup reflects the related rows as of the read; cross
  -database relation/rollup propagation is eventual (the inverse-edge projection in Refs lags the typed
  table, [01 §4](./01-tech-and-data-model.md)) — acceptable and explicit.

---

## 5. Permission-filtered reads everywhere (the cross-cutting invariant)

Backlinks, database views, search results, and embedded-view contents are **pre-filtered by Id's
`list_objects`, never post-filtered** (ADR-03; deep-dive §7.4). A leak is both a security and a GDPR breach
(SC-1). Concretely:

- **Database views / rows** — `list_objects(viewer, read, 'database_row')` conjoined into the SQL (§4.1).
- **Backlinks panel** — `Refs.backlinks(page_ref, viewer)` is leak-free by construction (REF-1): you see a
  backlink iff you can see the artifact that made the reference.
- **Embedded live views** (a doc embedding an issue board) — the embed resolves via the *owning* subsystem's
  `project(ref, viewer)` (ADR-13), which is per-viewer permission-checked; a confidential issue degrades to a
  tombstone, never leaks.
- **The page-tree ACL with overrides** compiles to tuples (Id §5); a `direct_block` exclusion makes a
  narrowed sub-page disappear from a reader's `list_objects` **by construction** ([01 §5](./01-tech-and-data-model.md)).

The hot path is the permission filter, not the tree walk — exactly the Refs lesson (REF-2 §2.4).

---

## 6. Search indexing granularity (deep-dive Q10 — DECIDED)

**Decision: index at BOTH page-level and block-level, with page as the default unit and block-level for
jump-to-block + heading anchors.** Knowledge declares two `IndexSpec`s (Search §5.3): a page doc (title +
concatenated body, language-tagged) and a per-significant-block doc (heading/callout/code blocks that are
useful jump targets). This balances index size against the "jump to the exact block" UX (deep-dive §2.9).

- **Semantic/vector in v1: YES** for agent RAG (deep-dive Q10) — Knowledge is the prime RAG corpus; the
  embedding adapter is the swappable EU-hostable adapter Search owns (Search §4.8). Embeddings of personal
  data are personal data → erased with their source ([03 §6](./03-events-contracts-and-glue.md)).
- **Multilingual** (EU): per-language analyzers (Search §4.7) — Knowledge tags each page's `lang`.
- **Permission-aware** via `list_objects` (Search §4.2) — no leak via search.

Knowledge **never indexes itself** — it `project`s text and Search consumes off the bus (Search §5.3;
no cross-DB).

---

## 7. Event coalescing (semantic events, never keystrokes)

The Indexing/Outbox feeder **coalesces** high-frequency edit ops into **semantic** events (deep-dive §7.1):

```
on edit ops for page P:
  raw keystrokes / ops  → stay in the firehose collab layer (§2), NEVER the durable bus
  debounce window (e.g. 2–5s of inactivity, or block-boundary) → emit ONE knowledge.page.updated (semantic)
  the durable bus gets: knowledge.doc.updated (pointer, for live-embed invalidation) + knowledge.page.updated (semantic)
```

- **Never per-keystroke / per-op on the durable bus** (deep-dive §7.1; ADR-04.5). The durable bus carries
  coalesced semantic events + the `doc.updated` pointer; Search/Refs/Notif/OLAP react to the semantic event.
- **Reference edges are NOT coalesced away**: a new `mention`/`artifact_ref`/`embed` node emits
  `refs.edge.created` immediately on persist ([03 §3](./03-events-contracts-and-glue.md)) — the edge is a
  discrete fact, not a debounced summary.
- **Emitted via the outbox only** (BUS-2) in the same transaction as the block/row write.

---

## 8. The one editor render path (KN-4 / EI-05 §2 / DL §8b.2)

The editor obeys the day-one mandate: **read and edit run the same inline parser; controlled
`contenteditable` (not `<textarea>`); caret = char offset into serialized markdown.** The
**`render(parse(md)) === md` round-trip over a corpus is a hard CI gate** (the correctness bar for TE-15
whatever the concurrency engine).

### 8.1 The shared Rust core (one parser, client + server)

The parser/serializer/offset model is a **Rust `myelin-content` core compiled to WASM** and reused
server-side (DL §8.1). This eliminates the two-divergent-renderers trap (EI-05 §2) *structurally* — there is
one `parseInline`/`serializeInline` implementation, not a server one and a client one that drift.

```
parse(md_string)        → inline AST (runs + the structured mention/artifact_ref/embed nodes from inline_nodes)
serialize(inline_AST)   → md_string
render(parse(md)) === md   -- the corpus gate (DL §8b.2): every fixture round-trips losslessly
```

### 8.2 The primitives shipped + unit-tested standalone (before the integrated editor)

Per DL §8b.2 / R3 (editor primitives before consumers), three modules ship and are unit-tested **standalone**
before the integrated editor:

1. **The serializer** — inline AST ↔ markdown-subset string, with `mention`/`artifact_ref`/`embed` as
   **structured nodes** (never collapsed into the string) so reference-extraction is reliable (KN-2).
2. **The offset model** — the caret is a **character offset into the serialized markdown**, bridged to/from
   DOM positions (EI-05 §2). Controlled `contenteditable` intercepts structural input, lets plain text
   through, normalizes on serialize.
3. **The DOM-surgery module** — **Enter-splits-a-block** and **caret-placement-after-split** (the #1 "this
   isn't a real editor" tells, EI-05 §2) are their own designed, unit-tested module. Browser variance
   (Enter/IME/paste) is the named top Knowledge-P4 risk (DL §8b.2).

### 8.3 Why a markdown-subset string, not inline-range JSON

(KN-2 / D10; EI-05 §2): it survives copy/paste, export, diff, and reference-extraction; it needs no
server-side sanitisation pass; and it survives an editor rewrite with zero schema migration. The structural
nodes stay structured so the reference grammar is reliable server-side. This is the ADR-05 reconciliation:
**AST for block structure, markdown-subset string for inline runs**, structured ref/mention/embed nodes
preserved (decision-record §(d) tension 3).

---

## 9. Agent edits flow through the same collab protocol (deep-dive §6, ADR-08)

A mock/real agent that edits a doc goes through `SEND_OP` (§2) exactly as a human — so attribution
("suggested by agent"), undo, and history treat agent edits as first-class. The agent never mutates
Knowledge's DB directly: a side-effecting tool (`knowledge.page.append`, `knowledge.row.upsert`) goes through
the Agent Fabric's `EffectApi` (plan-then-apply, agent-fabric §5.2), which calls Knowledge's **public
endpoint** as the agent principal (same gateway, no carve-out), which then applies the op through the collab
protocol. Consequential edits open a **HITL gate** surfaced as a Chat approval card (agent-fabric §5.3); the
*same mock code* runs deterministically in dev and an `LlmAgentRuntime` later with zero platform changes
(ADR-08). See [03 §5](./03-events-contracts-and-glue.md) for the tool registrations and the AG-7 trace.

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md).
