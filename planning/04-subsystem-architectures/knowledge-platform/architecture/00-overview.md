# Knowledge Platform — 00 · Overview, Role & Responsibilities

> Phase: `04-subsystem-architectures/knowledge-platform` — **Phase 5-B rewrite** (the subsystem architecture
> re-written from scratch against the RECONCILED shared layer). Canonical brief:
> [`VISION.md`](../../../../VISION.md) (never contradicted). Doctrine (binding):
> [`EI-04`](../../../../external-insights/04-hard-problems.md) §2 (real-time collab — the named hard
> problem), [`EI-05`](../../../../external-insights/05-ux-and-design.md) §2 (one editor render path).
>
> **The frozen build-to surface (what this rewrite conforms to, no drift):**
> [`05/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md) (the FROZEN
> reconciled contracts, supersedes Phase 3) + the rationale in
> [`05/00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L; read its **Part 4 per-system punch list** for Knowledge). The refined shared docs
> most load-bearing for Knowledge: [`event-bus.md`](../../../05-refined-shared-systems-architecture/event-bus.md)
> (the firehose resume-cursor protocol, OQ-J), [`reference-graph.md`](../../../05-refined-shared-systems-architecture/reference-graph.md)
> (the `#sub` grammar + tombstone ladder, X-4), [`search-and-indexing.md`](../../../05-refined-shared-systems-architecture/search-and-indexing.md)
> (the `Filter` conjoin), [`storage.md`](../../../05-refined-shared-systems-architecture/storage.md) (per-subject DEK).
>
> **Preserved design record (NOT rewritten):** the exploration sketches ([`../sketches/`](../sketches/) 01–07
> + [`00-findings.md`](../sketches/00-findings.md)) and the design sketches
> ([`../design/`](../design/): IA, user-flows, per-screen wireframes with empty/loading/error states). Those
> are the Stage-1 ground truth this architecture builds on.
>
> **Status convention** (VISION §3, name-your-floors): *DECIDED* = committed for build/test; *FLOOR* = a
> partial answer shipped with a named follow-on; *[OPEN — LEGAL]* = a defensible engineering posture flagged
> for counsel/DPO (the structural floor ships regardless). Every property that can fail names its **drill**
> ([`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md)). Dated 2026-06-19.

---

## 0. Changes vs the Phase-4 first pass (the reconciliation deltas absorbed)

This is the Phase-5-B rewrite: the Phase-4 design was **sound and largely ratified**, so this section lists
**exactly what changed and why** — every change is a conformance to a now-FROZEN contract shape, not a design
reversal. No ADR was reversed; the Phase-4 language/DB choice stands (Rust + Postgres + S3 + Yrs).

| # | Phase-4 first pass | Phase-5-B (reconciled) | Why / frozen contract |
|---|---|---|---|
| Δ1 | Collab transport rode `firehose::publish/tail` with a Knowledge-local `op_seq` resume cursor. | Conforms to the **frozen `firehose::subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)` resume-cursor protocol**, `scope = doc:<page_id>`, per-`(stream,scope)` monotonic `seq`, `resync_required` → `*.snapshot` fallback. Knowledge still **owns** the resume-cursor + idempotent-apply + CRDT (KN-1, built FIRST); the bus provides the seam. | Contract 3.5 / OQ-J (co-designed once for board/doc/channel). [02 §2](./02-internals-and-algorithms.md), [03 §1.8](./03-events-contracts-and-glue.md). |
| Δ2 | Block-tree `block_type` enum was a Knowledge-local list (`paragraph…database_view…equation`). | Conforms to the **frozen `myelin-content` v1 Block taxonomy** Knowledge now formally LEADS and FREEZES (X-2): paragraph/heading/bullet_list/ordered_list/task_list/blockquote/code_block/callout/table/divider/image/embed/db_view/toggle/sync_block. Chat/Issues consume strict subsets. The three inline ref nodes (mention/artifact_ref/embed) are identical across all three. | Contract 13.1 / X-2 / OQ-B. [01 §2](./01-tech-and-data-model.md), [03 §2](./03-events-contracts-and-glue.md). |
| Δ3 | Synced blocks/transclusion **deferred entirely** in favour of `embed`. | `sync_block { source: ArtifactRef }` is now a **node type IN the frozen taxonomy** (Knowledge-only transclusion). Knowledge still ships the **behaviour** as a FLOOR (a read-projection sync, not editable-in-place multi-home); editable-in-place stays the named CRDT-era follow-on. The node exists in the AST so the taxonomy is complete; the engine is floored. | X-2 (taxonomy completeness) + EI-04 §2 (floor). [05 §7](./05-hard-problems.md). |
| Δ4 | `#sub` anchors: `#b<id>`, `#h-<id>`, `#row-<id>`, `#comment-<id>`. | Conforms to the **frozen unified `#sub` vocabulary** (X-4): `b<opaqueid>`, `h<opaqueid>` (**hyphen dropped**), `row-<opaqueid>`, `comment-<opaqueid>`, plus **`field-<opaqueid>`** (a db field within a row) added. Refs stores the full sub-URN + the `#sub`-stripped root; resolution runs the one **4-step tombstone ladder**. | Contract 5.7 / X-4 / OQ-D. [03 §2.1](./03-events-contracts-and-glue.md). |
| Δ5 | `order_key` = "base-62 unbounded keys + jitter + lazy rebalance via a *measured* key-length threshold". | Conforms to the **frozen `order_key`/LexoRank encoding** byte-identical with Issues (X-3): base-62 alphabet `0-9 A-Z a-z` (ASCII-ordinal so byte-compare == rank), first item `"U"`, midpoint bisection, **2-char random jitter suffix**, **48-char rebalance trigger**, tiebreak `created_at` then ULID. | Contract 13.3 / X-3 / OQ-C. [01 §2](./01-tech-and-data-model.md), [02 §3.5](./02-internals-and-algorithms.md). |
| Δ6 | DB views: `query_ast jsonb`; field types a Knowledge-local list. | Conforms to the **frozen `myelin-query` shapes** byte-identical with Issues (X-3): the `FieldType` enum, the `ViewSpec` view-model, the `QueryAst` grammar (= the bus `EventMatcher` core, contract 3.4). Knowledge owns its **executor**; the definitions are identical. | Contract 13.3 / X-3 / OQ-C. [01 §4](./01-tech-and-data-model.md), [02 §4](./02-internals-and-algorithms.md). |
| Δ7 | `list_objects` returned a `Filter{set_expr}` (shape under-specified, "facet-expressible"). | Conforms to the **frozen `SetExpr` push-down** (OQ-E): `All/None/Ids/NotIds/InRelation{relation, via_column}/Union/Intersect/Difference/TupleSet{index}`, lowered to a SQL predicate / JOIN over Knowledge's own `db_row.id` column against the **per-tenant authz reverse index**. Row-level via `InRelation`; field/transition hiding via the **`CaveatContext`** at `check`-time, off the hot path. | Contract 4.3 + 4.2 / OQ-E. [02 §4.1](./02-internals-and-algorithms.md), [03 §3](./03-events-contracts-and-glue.md). |
| Δ8 | Free-text erasure residual was a Knowledge-local GD-6 write-up. | Instantiated **by reference** to the ONE platform-wide free-text/immutable-content erasure posture (X-7, contract 10.9): per-subject DEK crypto-shred (self-authored) + pseudonym-map shred (identity, grammar `<pseudonym>@<tenant>.noreply`) + `restrict` suppression; residual third-party free-text under the documented lawful-basis limit + best-effort `rectify`/tombstone. Not restated five times. | Contract 10.9 / X-7 / OQ-G `[OPEN — LEGAL]`. [05 §6](./05-hard-problems.md), [06 §8](./06-reconciliation-compliance.md). |
| Δ9 | Comments: KB-native data + shared thread render; consolidation flagged. | Confirmed under **OQ-L**: v1 ships **two threading stores, one scheme** — KB comments and Chat threads share the same `#thread-`/`#comment-` `#sub` grammar + `myelin-content` AST + `refs.edge.created`. Consolidation onto the Chat threading primitive + firehose transport is the **named follow-on** (promote when doc-anchored comments need real-time multi-party presence). | OQ-L. [05 §10](./05-hard-problems.md), [06 §9](./06-reconciliation-compliance.md). |
| Δ10 | Living-doc/daily-note templating + HITL approval cards described loosely. | Conforms: living-doc templates and SLA-style status strings register into the **ONE `humanise`/ICU-MessageFormat surface** (contract 7.3, OQ-L) — no second template engine. Scheduled living-doc automations + HITL approval cards use the **`SCHEDULE_AND_RUN_JOB` long-park + per-effect `idem_key`** idioms (contract 9.1/9.2/9.4, OQ-F). | Contract 7.3 + 9.x / OQ-L + OQ-F. [03 §5](./03-events-contracts-and-glue.md), [04 §3](./04-views-cli-and-api.md). |
| Δ11 | The nine Stage-1 open questions lived in a separate `08-committed-resolutions.md`. | `08` is **folded into 01–07** (the file split is now 00–07). Its commitments (CR-A fractional index → the frozen LexoRank; CR-C `U+FFFC` anchor; CR-D rows=tuples/fields=caveat; CR-E push-down; CR-F online CAS→CRDT migration; CR-I per-subject DEK) survive verbatim, now phrased against the frozen shapes. | Phase-5-B file consolidation. |

Everything else from Phase 4 carried forward unchanged: Rust+Postgres+S3+Yrs; the CAS-floor→CRDT ladder with
the resume-cursor transport built FIRST; the read-time formula/rollup engine (KN-3); the page-tree
inheritance-with-overrides ReBAC fragment; per-block rows (adjacency list); the one editor render path
(`render(parse(md)) === md`); the AG-7 content-addressed agent-trace holder; reindex-from-source via `replay`.

---

## 0a. Reading map (the document split)

| # | Doc | What it owns |
|---|---|---|
| 00 | **this** | Role, responsibilities, owns-vs-delegates, the floors named up front, the component map, the build-order law, the reconciliation deltas (§0). |
| 01 | [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | Language/DB choice + written justification; the full schema (block tree, op-log, snapshots, the flexible DB over the frozen `myelin-query` shapes, the page-tree ACL projection, the typed tables); the stateful-component register. |
| 02 | [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | The hard-problem algorithms: the resume-cursor transport over the frozen firehose protocol (KN-1), CAS floor → CRDT, block-tree storage + the frozen LexoRank, the flexible-DB query with the `SetExpr` push-down, the read-time formula/rollup engine, the one editor render path. |
| 03 | [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `knowledge.*` event taxonomy; how Knowledge IMPLEMENTS every frozen glue contract (`ArtifactRef`/`project`/`replay`, the outbox, `check`/`list_objects`+`SetExpr`+`CaveatContext`+the ReBAC fragment, `PersonalDataHolder`, `ToolDef`s); the TE-7 typed-edge mirror; the AG-7 holder. |
| 04 | [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | The views (ref `design/`), the CLI surface, the API/agent-tool surface. |
| 05 | [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem-specific hard problem resolved, cited prior art, the named floor. |
| 06 | [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md) | How Knowledge now IMPLEMENTS the frozen reconciled contracts (the firehose protocol, `myelin-content`, `myelin-query`+`order_key`, the `SetExpr` Filter, the `#sub` grammar, the erasure posture, threading), plus any residual request for Phase 6. |
| 07 | [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | The quantified drills owed + open questions for Phase 6. |

**Floors named up front** (the honest state of v1):

1. **Collaboration ships CAS-floor first, CRDT is the named next step** (KN-1; EI-04 §2). The resume-cursor
   durable transport is **item 0, built FIRST** over the frozen firehose protocol; per-block optimistic
   compare-and-swap + soft-locks + snapshot/restore is the **v1 floor that does not merge**; the Yrs CRDT is
   the scheduled promotion **triggered by the first true concurrent-edit conflict** (R5). [05 §1](./05-hard-problems.md).
2. **Read-time formula/rollup, never stored** (KN-3, contract 13.3 `rollup`/`formula` "computed at READ TIME,
   never stored"); materialise only when read-time recompute is *measured* too slow. [05 §4](./05-hard-problems.md).
3. **Synced-block transclusion is a read-projection floor** (Δ3): the `sync_block` node exists in the frozen
   taxonomy; v1 renders it as a live read-projection (like `embed`), **not** editable-in-place multi-home —
   that is the named CRDT-era follow-on. [05 §7](./05-hard-problems.md).
4. **Free-text PII erasure: structural floor is reliable; residual is the platform posture** (contract 10.9 /
   X-7 `[OPEN — LEGAL]`) — instantiated by reference, not over-promised. [05 §6](./05-hard-problems.md).
5. **Cross-cell collab for a multi-cell tenant is designed-not-built** (OQ-I): v1 pins a doc's authoritative
   collab session to one cell; the cross-cell PII-free pointer bridge is the named floor. [05 §9](./05-hard-problems.md).
6. **Offline depth = read + queued light-edit on the CAS floor; full offline-first arrives with the CRDT**
   (the CRDT is what makes deep offline correct). [05 §8](./05-hard-problems.md).
7. **KB-native comment threads (one scheme, two stores)** → consolidation onto the Chat threading primitive
   is the named follow-on (OQ-L). [05 §10](./05-hard-problems.md).

---

## 1. Role & responsibilities

The Knowledge Platform is Myelin's **Notion-class workspace** (VISION §2, subsystem #4 of five): the home
for an organisation's durable, human- and agent-authored knowledge — specs, RFCs, design docs, runbooks,
onboarding guides, decision logs, meeting notes — **and** for *structured data* (databases with
table/board/calendar/timeline views). Its differentiated job inside Myelin is to be **not a silo** but the
*rich, referenceable, agent-readable substrate* the rest of the platform points at and writes into
(Phase-2 §1): a design doc embeds a live issue board; an incident runbook references the exact CI run that
failed; a PRD backlinks every issue that implements it. The reference graph that cascades an action is the
graph a human traverses — the platform moat (EI-02 §7), and Knowledge is its **heaviest producer and
consumer**.

### 1.1 What Knowledge OWNS (its core competency + its frozen-contract obligations)

- **LEADS + FREEZES the canonical `myelin-content` node taxonomy** (X-2 / OQ-B, contract 13.1) — the block +
  inline AST that Chat and Issues consume as strict subsets. Knowledge owns the complete v1 set and the three
  platform-load-bearing inline reference nodes (`mention(Principal)`, `artifact_ref(ArtifactRef)`,
  `embed(ArtifactRef)`) that produce `refs.edge.created` uniformly across all three subsystems. **Owns the
  ADF→`myelin-content` lossy-map** (contract 13.2) that Issues import depends on.
- **Co-owns `myelin-query` + `order_key` parity** (X-3 / OQ-C, contract 13.3) — the `FieldType` enum,
  `ViewSpec`, `QueryAst`, and the LexoRank `order_key` encoding are **byte-identical** with Issues; Knowledge
  owns its own compiler/executor for flexible-DB execution. The `rollup`/`formula` field types are
  read-time-computed by contract.
- **The collab op-stream resume-cursor durable transport** (KN-1) — **built FIRST**, over the **frozen
  firehose `subscribe/resume/scope` protocol** (contract 3.5); the reconnect-loses-zero-ops drill is
  Knowledge's. The CRDT slots into this transport.
- **The block tree** — the ordered tree of typed blocks that is a page, with **stable opaque block ids** that
  survive moves/edits/collaboration and are the `#sub` ref targets (`b<opaqueid>`, X-4). The `#sub`
  block/heading/row anchor grammar is Knowledge's stability obligation under the Refs-owned grammar.
- **Rich-text / collaborative editing** — the live multi-author (and multi-agent) editing engine,
  presence/awareness, and the **CRDT-vs-OT concurrency choice (TE-15)** *over* the shared content model
  (Knowledge-owned, not shared — ADR-05: share the AST, not the editor).
- **Databases + read-time formula/rollup** — the structured-collection instances, the read-time formula/rollup
  engine, and the physical storage/query execution for flexible user-defined fields.
- **The page-permission tree** — hierarchical ReBAC inheritance with explicit overrides, declared as the
  Knowledge namespace fragment (contract 4.9), not a bespoke ACL.
- **The typed tables** `page_parent` + `db_relation` — Knowledge's half of the TE-7 typed-edge mirror
  (contract 5.5); Refs holds the rebuildable projection.
- **Version history / snapshots** — restore points, diffs, named versions, op-log compaction/GC.
- **The AG-7 content-addressed agent-trace holder** (contract 8.8) — accepts an agent execution trace as a
  Knowledge document (reusing the block model) and registers it as an erasable `PersonalDataHolder`.
- **The ONE editor render path** — `render(parse(md)) === md` over a corpus, one Rust `myelin-content` core
  compiled to WASM, reused client + server (contract 13.1 WASM target; EI-05 §2).

### 1.2 What Knowledge DELEGATES to the shared systems

Knowledge implements the three glue contracts (ADR-13) and delegates everything cross-cutting. It **reads no
other subsystem's store** (the `no-cross-db` lint, ADR-01); it interacts only through the frozen contracts.

| Concern | Delegated to | The frozen contract Knowledge calls / implements |
|---|---|---|
| Identity, page-tree ACL, row/field ABAC, agent delegation | **Identity** (`myelin-identity`) | `authenticate`/`check`(+`CaveatContext`)/`list_objects`(+`SetExpr`)/`delegation`; declares the **Knowledge ReBAC namespace fragment** (4.9, page-tree inherit-with-overrides + row + field caveat); `write_tuples`→zookie stamped on `page.acl_zookie` (4.6/4.10). |
| Event emission/consumption | **Event Bus** (`myelin-events`) | `OutboxTx::emit(draft, cause)` (the only emit path, 2.2), the `EventHandler` consumer template (2.4), `events::reindex`+Knowledge's `replay` (2.6). Coalesced semantic events, never raw keystrokes. |
| The collab op-stream + presence transport | **Bus firehose seam** (3.5) | `firehose::subscribe/resume(stream, scope=doc:<id>, cursor)` + the `knowledge.doc.updated` pointer event. **Knowledge owns the resume-cursor protocol + CRDT over it** (KN-1). |
| Mentions / relations / backlinks / embeds | **Reference Graph** (`myelin-refs`) | the three inline nodes emit `refs.edge.created` (5.4); calls `resolve`/`backlinks`/`traverse` (5.2/5.3); the `#sub` grammar + tombstone ladder (5.7); the TE-7 typed-edge mirror (5.5). |
| Full-text + structured + vector search | **Search** (`myelin-search`) | `declare_indexable(IndexSpec)` (6.3), `query`/`semantic` (6.1/6.2) **with the `list_objects` `Filter` conjoined**; `project` feeds the index. Knowledge is a primary feeder + the prime RAG corpus. |
| Mentions/comment/share/watch alerts + humanisation | **Notifications** (`myelin-notif`) | declares its `watcher` relation + `define_notif_rule` (7.6); `project` Display mode feeds the **sole `humanise` templating surface** (7.3). |
| Agent authors/readers/triggers | **Agent Fabric** (`myelin-agent`) | registers `ToolDef`s (8.1) with the frozen `requires_approval` defaults; agent edits flow **through the same collab protocol** as humans; the four uniform sandbox guarantees apply (8.4). |
| Block/row/CRDT-update durable storage; media; keys | **Storage** (`BlobStore` + KMS) | OLTP tree/rows; object store for media + CRDT snapshots; **per-subject DEK crypto-shred** for free-text/op-log (11.4). |
| DSR / erasure / audit / retention | **GDPR/Audit** (`myelin-gdpr`) | implements `PersonalDataHolder` (10.1); the residual is the platform erasure posture (10.9) **by reference**. Knowledge is the **hardest GDPR surface** in Myelin. |
| Scheduled automations (daily-notes, living docs); HITL waits | **Durable-workflow** (`myelin-flow`) | `DurableExecutor::start` + the timer wheel (9.3); HITL approval resumes via a durable signal with the **per-effect `idem_key`** rule (9.1/9.4); living-doc jobs use `SCHEDULE_AND_RUN_JOB` (9.2). |

### 1.3 The one-paragraph thesis

*Knowledge is a thin authority over a content tree and a set of structured collections, sitting on the shared
substrate. A page is an ordered tree of typed blocks (adjacency list + the frozen LexoRank `order_key`);
inline runs are a markdown-subset string with `mention`/`artifact_ref`/`embed` as structured nodes (KN-2,
contract 13.1), so reference-extraction is reliable and the one editor render path (`render(parse(md)) === md`)
is the correctness bar regardless of concurrency engine (KN-4). Live editing rides the resume-cursor durable
transport that is item 0 (KN-1), over the frozen `firehose::subscribe/resume` protocol — built before any
merge engine, so a dropped connection loses zero ops; the v1 merge floor is per-block optimistic
compare-and-swap (no silent overwrite, no blend), promoted to a Yrs CRDT on the first true concurrent-edit
conflict. Databases are a JSONB property bag per row queried over the frozen `myelin-query` shapes with
derived indexable projections; formulas and rollups are computed at READ TIME, never stored (KN-3).
Permissions are the page tree compiled to ReBAC tuples (inherit-with-overrides), row visibility pushed down
via the `SetExpr` `InRelation` filter and field hiding via a `CaveatContext` off the hot path. Every edge,
mention, and embed is a structured node that emits `refs.edge.created` through the outbox; every state change
is a coalesced semantic event in the canonical envelope; erasure is per-subject crypto-shred reaching into
the immutable op-log + history, with the residual handled per the platform posture (10.9). Knowledge invents
no auth, reads no other store, and is fully rebuildable from its own source via `replay` — which is what
makes it recoverable and erasure-correct.*

---

## 2. The internal component architecture (at altitude)

A set of Rust services (ADR-02; [01 §1](./01-tech-and-data-model.md) justifies the choice) inside a
region-pinned cell (ADR-11), each a thin shell over `myelin_substrate::serve(AppSpec)` (contract 1.1), each
owning a slice of the domain and talking to the shared layer only via the glue contracts.

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│  KNOWLEDGE SUBSYSTEM  (Rust services; one region-pinned cell; serve(AppSpec) each) │
│                                                                                    │
│  ┌────────────────────┐   ┌──────────────────────┐   ┌────────────────────────┐   │
│  │ Document Service   │   │ Collaboration / Sync │   │ Database Service       │   │
│  │ · block tree (adj  │◄─►│ Engine               │   │ · rows (JSONB bag)     │   │
│  │   list + LexoRank) │   │ · resume-cursor       │  │ · myelin-query exec    │   │
│  │ · pages, hierarchy │   │   transport over the  │  │   (FieldType/ViewSpec/ │   │
│  │ · op-log+snapshots │   │   frozen firehose     │   │   QueryAst, frozen)   │   │
│  │ · CAS floor → CRDT │   │   subscribe/resume    │   │ · read-time formula/  │   │
│  │   (TE-15) authority│   │   (OQ-J; built FIRST) │  │   rollup (KN-3)        │   │
│  │   for schema/erase │   │ · presence relay      │   │ · SetExpr push-down    │   │
│  └─────────┬──────────┘   └──────────┬───────────┘   └───────────┬────────────┘   │
│            │                         │ firehose       ───────────┘                 │
│            │      ┌──────────────────▼─────── (ops/presence) ─────┐                │
│  ┌─────────▼──────▼──────────────────────────────────────────────▼─────────────┐  │
│  │ Projection / Render service — project(ref,viewer)+resolve embeds (ADR-13)    │  │
│  │ + the one editor render path (render(parse(md))===md) shared client (WASM)   │  │
│  └─────────┬───────────────────────────────────────────────────┬─────────────┘    │
│            │                                                    │                  │
│  ┌─────────▼────────┐  ┌────────────────────┐  ┌────────────────▼──────────────┐  │
│  │ Permission       │  │ Indexing / Outbox  │  │ Automation / Trigger / Agent  │  │
│  │ projector        │  │ feeder (events →   │  │ adapter (ToolDefs; agent edits│  │
│  │ (page-tree →     │  │ Bus/Search/Refs;   │  │ via collab; AG-7 trace write; │  │
│  │ ReBAC fragment)  │  │ coalesced semantic)│  │ SCHEDULE_AND_RUN_JOB jobs)     │  │
│  └──────────────────┘  └────────────────────┘  └───────────────────────────────┘  │
│            │                      │                          │                     │
│  ┌─────────▼─────────┐ ┌──────────▼─────────┐  ┌─────────────▼────────────────┐   │
│  │ GDPR holder       │ │ Storage adapter    │  │ Export / Import service        │   │
│  │ (locate/export/   │ │ (OLTP tree/rows;   │  │ (lossless JSON, MD, HTML/PDF,  │   │
│  │ rectify/restrict/ │ │ object media +     │  │ CSV; Art. 20; the ADF→content  │   │
│  │ erase; per-subject│ │ snapshots; KMS)    │  │ lossy-map import, contract 13.2)│  │
│  │ crypto-shred)     │ │                    │  │                               │   │
│  └───────────────────┘ └────────────────────┘  └──────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────────┘
     │ authz          │ events/outbox    │ refs        │ search    │ gdpr   │ flow
     ▼                ▼                  ▼             ▼           ▼        ▼
  Identity         Event Bus        Reference Graph  Search    GDPR/Audit  Workflow
```

**The components, one line each** (detail in [01](./01-tech-and-data-model.md)/[02](./02-internals-and-algorithms.md)):

1. **Document Service** — authority for *what a page is*: the block tree (adjacency list + LexoRank `order_key`),
   the page hierarchy (sub-pages = the folder-like nesting), version history/snapshots, the op-log, and the
   CAS-floor→CRDT concurrency state. Authority for the non-merge concerns (schema validation, erasure).
2. **Collaboration / Sync Engine** — the live editing path over the **resume-cursor durable transport** (KN-1,
   the first thing built) on the frozen `firehose::subscribe/resume(scope=doc:<id>)` protocol: applies/relays
   edit ops idempotently, broadcasts presence/awareness on the ephemeral tier, emits only **coalesced
   semantic** events to the durable bus.
3. **Database Service** — instances of the shared structured-collection primitive over the **frozen
   `myelin-query` shapes**: typed field definitions, views as `ViewSpec` query projections, rows (JSONB bag),
   two-way relations, the read-time formula/rollup engine, the flexible-field query execution with the
   `SetExpr` push-down.
4. **Projection / Render service** — implements `project(ref, viewer)` and `resolve` for embeds (ADR-13);
   hosts the **one editor render path** as a Rust core compiled to WASM and reused server-side (KN-4).
5. **Permission projector** — compiles the page-tree (inheritance + overrides) into the Knowledge ReBAC
   namespace fragment tuples (contract 4.9).
6. **Indexing / Outbox feeder** — writes events to the transactional outbox in the same DB transaction as the
   state change; coalesces high-frequency edits into semantic `knowledge.page.updated`.
7. **Automation / Trigger / Agent adapter** — registers Knowledge `ToolDef`s; applies agent-authored edits
   through the same collab protocol; owns the AG-7 trace write path; runs scheduled living-doc automations as
   `SCHEDULE_AND_RUN_JOB` jobs.
8. **GDPR holder** — `locate/export/rectify/restrict/erase` across blocks, rows, history, mentions,
   authorship; **per-subject DEK crypto-shred** for the immutable op-log + version history; residual per 10.9.
9. **Storage adapter** — OLTP (block tree + rows), object store (media + CRDT snapshots), residency-pinned,
   per-tenant envelope-encrypted (with per-subject sub-keys for free-text columns, contract 11.4).
10. **Export/Import service** — lossless JSON (the spine of portability + Art. 20), Markdown/HTML/PDF, CSV;
    the ADF→`myelin-content` lossy-map import (contract 13.2).

---

## 3. The build-order law (R1 / R3 — what is sequenced first)

Per the roadmap sequencing law (R1: "order by what kills you first — silent data-loss floors before any
feature surface") and the doctrine floor for collaboration (KN-1: CAS → CRDT, but the **transport** is item 0):

1. **The resume-cursor durable transport (KN-1)** over the frozen `firehose::subscribe/resume` protocol.
   Before any merge engine, before any editor. A relay without resume cursors silently loses the gap on a
   reconnect (EI-04 §2.2) — so the transport, with idempotent apply and a durable resume cursor, is the *first*
   thing built and the **reconnect-loses-zero-ops drill** (KD-1) is its gate.
2. **The editor primitives standalone (KN-4).** The serializer, the offset model, and the DOM-surgery for
   Enter-splits-block / caret-after-split are shipped and unit-tested **before** the integrated editor, with
   `render(parse(md)) === md` as a hard corpus gate (KD-2).
3. **The CAS floor** (per-block optimistic compare-and-swap + soft-locks + snapshot/restore) — the v1 merge
   floor that does not blend but never silently overwrites.
4. **The structured-DB + read-time formula/rollup** over the frozen `myelin-query` shapes.
5. **References/backlinks/embeds + Search integration** (emit `refs.edge.*`; declare `IndexSpec` + `project`
   + `replay`; the `#sub` grammar + tombstone ladder).
6. **GDPR holder + the AG-7 agent-trace write.**
7. **The CRDT promotion** — Yrs slotted into the transport from step 1, triggered by the first true
   concurrent-edit conflict (R5).

This file is the map; the substance is in [01](./01-tech-and-data-model.md)–[07](./07-drills-and-open-questions.md).
