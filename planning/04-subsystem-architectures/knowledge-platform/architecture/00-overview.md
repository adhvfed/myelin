# Knowledge Platform — 00 · Overview, Role & Responsibilities

> Phase: `04-subsystem-architectures/knowledge-platform`. Canonical brief:
> [`VISION.md`](../../../../VISION.md) (never contradicted). Doctrine (binding):
> [`EI-04`](../../../../external-insights/04-hard-problems.md) §2 (real-time collab — the named hard
> problem), [`EI-05`](../../../../external-insights/05-ux-and-design.md) §2 (one editor render path).
> Binding directives: [`integration-directives.md`](../../../02b-doctrine-integration/integration-directives.md)
> Phase-4 **KN-1…KN-4**, plus SUB-X, X-1…X-5; decision-record
> [`§(c) D10/D11`](../../../02b-doctrine-integration/decision-record.md). Phase-3 build-to surface:
> [`contract-index.md`](../../../03-shared-systems-architecture/contract-index.md) and the foundational
> docs ([`00-platform-substrate`](../../../03-shared-systems-architecture/00-platform-substrate.md),
> [`identity-and-access`](../../../03-shared-systems-architecture/identity-and-access.md),
> [`event-bus`](../../../03-shared-systems-architecture/event-bus.md)). Phase-2:
> [`subsystems/knowledge-platform.md`](../../../02-holistic-architecture/subsystems/knowledge-platform.md)
> + [`design-language.md`](../../../02-holistic-architecture/design-language.md) §7.4/§8b. Phase-1:
> [`subsystem-deep-dives/knowledge-platform.md`](../../../01-research/subsystem-deep-dives/knowledge-platform.md).
>
> **Status convention** (VISION §3, name-your-floors): *DECIDED* = committed for build/test; *FLOOR* = a
> partial answer shipped with a named follow-on; *[OPEN → P5]* = handed forward. Every property that can
> fail names its **drill** ([`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md)).
> Dated 2026-06-19; this date stamps the claims herein (E-2).

---

## 0. Reading map (the document split)

| # | Doc | What it owns |
|---|---|---|
| 00 | **this** | Role, responsibilities, owns-vs-delegates, the floors named up front, the component map. |
| 01 | [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | The language/DB choice + written justification; the full schema (block tree, op-log, snapshots, databases, views, page-tree ACL projection). |
| 02 | [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | The hard-problem algorithms in depth: the resume-cursor transport (KN-1), CAS floor → CRDT, block-tree storage, the flexible-DB query model, the read-time formula/rollup engine, the one editor render path. |
| 03 | [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `knowledge.*` event taxonomy; every glue contract (`ArtifactRef`/`project`/`replay`, the envelope via the outbox, Id `check`/`list_objects` + the ReBAC namespace fragment, `PersonalDataHolder`, `ToolDef`s); the agent-trace holder (AG-7). |
| 04 | [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | The views (the editor, database views, backlinks, history, sharing), the CLI surface, the API/agent-tool surface. |
| 05 | [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem-specific hard problem resolved, with cited prior art and the named floor. |
| 06 | [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md) | The itemized list of shared-system changes Knowledge needs from Phase 5 reconciliation. |
| 07 | [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | The quantified drills owed + open questions for Phase 5. |
| 08 | [`08-committed-resolutions.md`](./08-committed-resolutions.md) | The **nine Stage-1 open questions, now committed** (fractional-index rebalancing, snapshot cadence/format, inline-node placeholder encoding, row-vs-field permission mechanism, `list_objects` push-down, CAS→CRDT online migration, comments component, cross-cell bridge, per-subject DEK granularity). |

> **Stage-1 inputs** (the committed direction this architecture builds on): the exploration sketches
> ([`../sketches/`](../sketches/) 01–07 + [`00-findings.md`](../sketches/00-findings.md)) and the design
> sketches ([`../design/information-architecture.md`](../design/information-architecture.md),
> [`../design/user-flows.md`](../design/user-flows.md), [`../design/wireframes.md`](../design/wireframes.md)
> — IA + flows + per-screen wireframes with empty/loading/error states, VISION §3 "design before UI code").
> [`08`](./08-committed-resolutions.md) closes the open questions `00-findings.md` §6 handed forward.

**Floors named up front** (the honest state of v1):

1. **Collaboration ships CAS-floor first, CRDT is the named next step** (KN-1; EI-04 §2). The
   resume-cursor durable transport is **item 0, built FIRST**; per-block optimistic compare-and-swap +
   soft-locks + snapshot/restore is the **v1 floor that does not merge**; the Yrs CRDT is the scheduled
   promotion **triggered by the first true concurrent-edit conflict** (R5). See [05 §1](./05-hard-problems.md).
2. **Read-time formula/rollup, never stored** (KN-3); materialise only when read-time recompute is
   *measured* too slow. The materialised projection tier is a named, measured-trigger follow-on. [05 §4](./05-hard-problems.md).
3. **Synced blocks / transclusion is deferred** (deep-dive Q11) — it breaks the clean tree and complicates
   permissions/erasure/reference-counting. v1 ships embeds (live `ArtifactRef`) but **not** a block that
   has one home and many edit sites. Named follow-on. [05 §7](./05-hard-problems.md).
4. **Free-text PII erasure is structurally reliable, free-text is best-effort + tooling** (deep-dive §8;
   GD-6 `[OPEN → LEGAL]`) — an honest, stated limitation, not an over-promise. [05 §6](./05-hard-problems.md).
5. **Cross-cell collab for a multi-cell tenant is designed-not-built** (inherits the bus cross-cell floor,
   event-bus §7.4). v1 pins a doc's authoritative collab session to one cell. [05 §9](./05-hard-problems.md).
6. **Offline depth = read + queued light-edit on the CAS floor; full offline-first arrives with the CRDT**
   (deep-dive Q9). The CRDT is what makes deep offline correct; the floor offers optimistic queued edits
   that reconcile via CAS. [05 §8](./05-hard-problems.md).

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

### 1.1 What Knowledge OWNS (its core competency + its Phase-3 handoff obligations)

From the Phase-3 handoff ([README §5](../../../03-shared-systems-architecture/README.md)) and the Phase-2
ownership table, Knowledge owns:

- **The `knowledge.*` taxonomy + the page-tree-inheritance-with-overrides ReBAC namespace fragment**
  (declared to Id, never a bespoke ACL — [03 §4](./03-events-contracts-and-glue.md), Id §5 Knowledge clause).
- **The collab op-stream resume-cursor durable transport** (KN-1) — **built FIRST**; the
  reconnect-loses-zero-ops drill is Knowledge's ([02 §2](./02-internals-and-algorithms.md), [07](./07-drills-and-open-questions.md)).
  The bus provides the *pointer-event seam* + the firehose `tail`/`publish`; Knowledge owns the protocol.
- **The block tree** — the ordered tree of typed blocks that is a page, with stable block ids that survive
  moves/edits/collaboration and act as reference targets (deep-dive §2.1). The `db_relation` / `page_parent`
  typed tables ([01 §4](./01-tech-and-data-model.md)) — Knowledge's half of the TE-7 typed-edge resolution
  (REF-1; Refs holds the rebuildable projection).
- **Rich-text / collaborative editing** — the live multi-author (and multi-agent) editing engine,
  presence/awareness, and the **CRDT-vs-OT concurrency choice (TE-15)** *over* the shared content model.
  This concurrency engine is **Knowledge-owned, not shared** (ADR-05: share the AST, not the editor).
- **Lead of the shared content/block-model taxonomy** (ADR-05; Chat/Issues consume) — Knowledge leads the
  block/inline node taxonomy in `myelin-content`, including the platform-load-bearing `mention(Principal)`,
  `artifact_ref(ArtifactRef)`, `embed(ArtifactRef)` inline nodes that produce `ref.created` events
  platform-wide.
- **Co-owner of the field-definition/view primitive** (ADR-06; with Issues) — the shared
  field-defs + view abstraction + query AST in `myelin-query`; Knowledge owns its *execution* (the flexible
  -DB query model TE-17 and the formula/rollup dataflow TE-18).
- **Databases + formula/rollup** — the structured-collection *instances*, the **read-time** formula/rollup
  engine (TE-18, KN-3), and the physical storage/query execution for flexible user-defined fields (TE-17).
- **The page-permission tree** — hierarchical ACL inheritance with explicit overrides — as a *projection
  into* the shared ReBAC model (ADR-03), not a bespoke ACL system.
- **Version history / snapshots** — restore points, diffs, named versions, op-log compaction/GC.
- **Accept a content-addressed write of an agent execution trace (AG-7)** and register it as an erasable
  holder ([03 §5](./03-events-contracts-and-glue.md)).

### 1.2 What Knowledge DELEGATES to the shared systems

Knowledge implements the three glue contracts (ADR-13) and delegates everything cross-cutting. It **reads
no other subsystem's store** (the `no-cross-db` lint, ADR-01); it interacts only through the contracts.

| Concern | Delegated to | The contract Knowledge calls / implements |
|---|---|---|
| Identity, page-tree ACL, agent delegation | **Identity** (`myelin-identity`) | `authenticate` / `check` / `list_objects` / `delegation`; Knowledge **declares its ReBAC namespace fragment** (Id §5) and **compiles the page tree to tuples** — no bespoke ACL. |
| Event emission/consumption | **Event Bus** (`myelin-events`) | `OutboxTx::emit(draft, cause)` (the only emit path), the `EventHandler` consumer template, `events::reindex` + Knowledge's `replay`. **Coalesced semantic events, never raw keystrokes.** |
| The collab op-stream + presence transport | **Bus firehose seam** | `firehose::publish/tail` + the `knowledge.doc.updated` pointer event. **Knowledge owns the resume-cursor protocol over it** (KN-1). |
| Mentions / relations / backlinks | **Reference Graph** (`myelin-refs`) | `mention`/`artifact_ref`/`embed` nodes emit `refs.edge.created`; Knowledge calls `resolve`/`backlinks`/`traverse`. Heaviest producer & consumer of refs. |
| Full-text + structured + vector search | **Search** (`myelin-search`) | `declare_indexable(IndexSpec)`, the `project` API; queries via `query`/`semantic`; permission-aware via `list_objects`. Knowledge is a primary feeder. |
| Mentions/comment/share/watch alerts | **Notifications** (`myelin-notif`) | declares its `watcher` relation + `define_notif_rule`; mentions feed the one inbox. |
| Agent authors/readers/triggers | **Agent Fabric** (`myelin-agent`) | registers `ToolDef`s; agent edits flow **through the same collab protocol** as humans (attribution/undo/history first-class). |
| Block/row/CRDT-update durable storage; media | **Storage** (`BlobStore` + KMS) | OLTP tree/rows; object store for media + CRDT snapshots; **per-subject DEK crypto-shred** for free-text/op-log erasure (GD-4). |
| DSR / erasure / audit / retention | **GDPR/Audit** (`myelin-gdpr`) | implements `PersonalDataHolder`; Knowledge is the **hardest GDPR surface** in Myelin (deep-dive §8). |
| Long-running / scheduled automations (daily-notes, living docs); HITL waits | **Durable-workflow** (`myelin-flow`) | `DurableExecutor::start`; the HITL approval card resumes via a durable signal. |

### 1.3 The one-paragraph thesis

*Knowledge is a thin authority over a content tree and a set of structured collections, sitting on the
shared substrate. A page is an ordered tree of typed blocks (adjacency list + fractional ordering key);
inline runs are a markdown-subset string with `mention`/`artifact_ref`/`embed` as structured nodes (KN-2),
so reference-extraction is reliable and the one editor render path (`render(parse(md)) === md`) is the
correctness bar regardless of concurrency engine (KN-4). Live editing rides the resume-cursor durable
transport that is item 0 (KN-1) — built before any merge engine, so a dropped connection loses zero ops;
the v1 merge floor is per-block optimistic compare-and-swap (no silent overwrite, no blend), promoted to a
Yrs CRDT on the first true concurrent-edit conflict. Databases are a JSONB property bag per row with
derived indexable projections; formulas and rollups are computed at READ TIME, never stored (KN-3).
Permissions are the page tree compiled to ReBAC tuples (page-tree inheritance with overrides). Every edge,
mention, and embed is a structured node that emits `refs.edge.created` through the outbox; every state
change is a coalesced semantic event in the canonical envelope; erasure is per-subject crypto-shred reaching
into the immutable op-log + history. Knowledge invents no auth, reads no other store, and is fully
rebuildable from its own source via `replay` — which is what makes it recoverable and erasure-correct.*

---

## 2. The internal component architecture (at altitude)

A set of Rust services (ADR-02; [01 §1](./01-tech-and-data-model.md) justifies the choice) inside a
region-pinned cell (ADR-11), each a thin shell over `myelin_substrate::serve(AppSpec)` (substrate §3), each
owning a slice of the domain and talking to the shared layer only via the glue contracts.

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│  KNOWLEDGE SUBSYSTEM  (Rust services; one region-pinned cell; serve(AppSpec) each) │
│                                                                                    │
│  ┌────────────────────┐   ┌──────────────────────┐   ┌────────────────────────┐   │
│  │ Document Service   │   │ Collaboration / Sync │   │ Database Service       │   │
│  │ · block tree (adj  │◄─►│ Engine               │   │ · rows (JSONB bag)     │   │
│  │   list + frac key) │   │ · resume-cursor       │  │ · shared field/view    │   │
│  │ · pages, hierarchy │   │   durable transport   │  │   primitive (ADR-06)   │   │
│  │ · op-log + snapshots│  │   (KN-1, built FIRST) │  │ · read-time formula/   │   │
│  │ · CAS floor → CRDT │   │ · presence relay      │   │   rollup dataflow      │   │
│  │   (TE-15) authority│   │ · authority for perms/│   │   (TE-18, KN-3)        │   │
│  │   for schema/erase │   │   schema/erasure on op│   │ · flexible-field query │   │
│  └─────────┬──────────┘   └──────────┬───────────┘   │   (TE-17)              │   │
│            │                         │ firehose       └───────────┬────────────┘   │
│            │      ┌──────────────────▼─────── (ops/presence) ─────┘                │
│  ┌─────────▼──────▼──────────────────────────────────────────────────────────┐    │
│  │ Projection / Render service — project(ref,viewer) + resolve embeds (ADR-13)│    │
│  │ + the one editor render path (render(parse(md))===md) shared client (WASM) │    │
│  └─────────┬───────────────────────────────────────────────────┬─────────────┘    │
│            │                                                    │                  │
│  ┌─────────▼────────┐  ┌────────────────────┐  ┌────────────────▼──────────────┐  │
│  │ Permission       │  │ Indexing / Outbox  │  │ Automation / Trigger / Agent  │  │
│  │ projector        │  │ feeder (events →   │  │ adapter (ToolDefs; agent edits│  │
│  │ (page-tree →     │  │ Bus/Search/Refs;   │  │ via collab; the AG-7 trace    │  │
│  │ ReBAC tuples)    │  │ coalesced semantic)│  │ write path)                   │  │
│  └──────────────────┘  └────────────────────┘  └───────────────────────────────┘  │
│            │                      │                          │                     │
│  ┌─────────▼─────────┐ ┌──────────▼─────────┐  ┌─────────────▼────────────────┐   │
│  │ GDPR holder       │ │ Storage adapter    │  │ Export / Import service       │   │
│  │ (locate/export/   │ │ (OLTP tree/rows;   │  │ (lossless JSON, MD, HTML/PDF, │   │
│  │ rectify/restrict/ │ │ object media +     │  │ CSV; Art. 20 portability)     │   │
│  │ erase; per-subject│ │ snapshots; KMS)    │  │                               │   │
│  │ crypto-shred)     │ │                    │  │                               │   │
│  └───────────────────┘ └────────────────────┘  └──────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────────┘
     │ authz          │ events/outbox    │ refs        │ search    │ gdpr   │ flow
     ▼                ▼                  ▼             ▼           ▼        ▼
  Identity         Event Bus        Reference Graph  Search    GDPR/Audit  Workflow
```

**The components, one line each** (detail in [01](./01-tech-and-data-model.md)/[02](./02-internals-and-algorithms.md)):

1. **Document Service** — authority for *what a page is*: the block tree (adjacency list + fractional
   ordering key), the page hierarchy (sub-pages = the folder-like nesting), version history/snapshots, the
   op-log, and the CAS-floor→CRDT concurrency state. Authority for the non-merge concerns (schema validation,
   erasure) the merge layer can't enforce.
2. **Collaboration / Sync Engine** — the live editing path over the **resume-cursor durable transport**
   (KN-1, the first thing built): applies/relays edit ops idempotently, broadcasts presence/awareness on the
   ephemeral firehose, and emits only **coalesced semantic** events to the durable bus.
3. **Database Service** — instances of the shared structured-collection primitive (ADR-06): typed field
   definitions, views as query-AST projections (ADR-07), rows (JSONB bag), two-way relations, the read-time
   formula/rollup dataflow (TE-18) and the flexible-field query execution (TE-17).
4. **Projection / Render service** — implements `project(ref, viewer)` and `resolve` for embeds (ADR-13);
   hosts the **one editor render path** as a Rust core compiled to WASM and reused server-side (the
   round-trip gate, KN-4).
5. **Permission projector** — compiles the page-tree (inheritance + overrides) into ReBAC tuples (Id §5).
6. **Indexing / Outbox feeder** — writes events to the transactional outbox in the same DB transaction as
   the state change; coalesces high-frequency edits into semantic `knowledge.page.updated`.
7. **Automation / Trigger / Agent adapter** — registers Knowledge `ToolDef`s; applies agent-authored edits
   through the same collab protocol; owns the AG-7 trace write path.
8. **GDPR holder** — `locate/export/rectify/restrict/erase` across blocks, rows, history, mentions,
   authorship; **per-subject DEK crypto-shred** for the immutable op-log + version history.
9. **Storage adapter** — OLTP (block tree + rows), object store (media + CRDT snapshots), residency-pinned,
   per-tenant envelope-encrypted (with per-subject sub-keys for free-text columns, GD-4).
10. **Export/Import service** — lossless JSON (the spine of portability + Art. 20), Markdown/HTML/PDF, CSV.

---

## 3. The build-order law (R1 / R3 — what is sequenced first)

Per the roadmap sequencing law (R1: "order by what kills you first — silent data-loss floors before any
feature surface") and the doctrine floor for collaboration (KN-1: CAS → CRDT, but the **transport** is
item 0):

1. **The resume-cursor durable transport (KN-1).** Before any merge engine, before any editor. A relay
   without resume cursors silently loses the gap on a reconnect (EI-04 §2.2) — so the transport, with
   idempotent apply and a durable resume cursor, is the *first* thing built and the
   **reconnect-loses-zero-ops drill** is its gate.
2. **The editor primitives standalone (KN-4).** The serializer, the offset model, and the DOM-surgery for
   Enter-splits-block / caret-after-split are shipped and unit-tested **before** the integrated editor, with
   `render(parse(md)) === md` as a hard corpus gate (EI-05 §2; DL §8b.2).
3. **The CAS floor** (per-block optimistic compare-and-swap + soft-locks + snapshot/restore) — the v1 merge
   floor that does not blend but never silently overwrites.
4. **The structured-DB + read-time formula/rollup** (TE-17/TE-18) on the shared primitive.
5. **The CRDT promotion** — Yrs slotted into the transport from step 1, triggered by the first true
   concurrent-edit conflict (R5).

This file is the map; the substance is in [01](./01-tech-and-data-model.md)–[07](./07-drills-and-open-questions.md).
