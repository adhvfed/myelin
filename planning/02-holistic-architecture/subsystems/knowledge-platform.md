# Phase 2 — Subsystem Architecture: Knowledge Platform (Notion-class)

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../../VISION.md)
> (single source of truth; never contradicted). Phase-2 spine this doc aligns to:
> [`architecture-decisions.md`](../architecture-decisions.md) (the ADR register) and
> [`system-overview.md`](../system-overview.md) (the holistic narrative). Phase-1 deep-dive this
> doc builds on: [`01-research/subsystem-deep-dives/knowledge-platform.md`](../../01-research/subsystem-deep-dives/knowledge-platform.md),
> with the structural foundation [`01-research/technical-structuring.md`](../../01-research/technical-structuring.md).
>
> **Altitude.** This is a *high-level* architecture (VISION §5.2): what the subsystem contains
> and how it interacts as a whole. Concrete schemas, the CRDT-vs-OT decision, the flexible-field
> query engine, and the formula/rollup dataflow internals are **Phase-4 deliverables** and are
> flagged here, not decided. Where this doc must diverge from the spine, it says so explicitly.

---

## 1. Role & responsibilities

The Knowledge Platform is Myelin's **Notion-class workspace**: the home for an organisation's
durable, human- and agent-authored knowledge — specs, RFCs, design docs, runbooks, onboarding
guides, decision logs, meeting notes — **and** for *structured data* (databases with
table/board/calendar/timeline views). It is subsystem #4 of five.

Its differentiated job inside Myelin (vs standalone Notion/Confluence) is to be **not a silo**
but a *rich, referenceable, agent-readable substrate* the rest of the platform points at and
writes into (Phase-1 deep-dive §1). A design doc embeds a live issue board; an incident runbook
references the exact CI run that failed; a PRD backlinks every issue that implements it. This is
the wedge (`technical-structuring.md §4.3`) realised in the knowledge domain.

### What it OWNS (core competency)

Per `system-overview.md §4` (ratified): the Knowledge subsystem owns

- **The block tree** — the ordered tree of typed blocks that is a page, with stable block ids
  that survive moves/edits/collaboration and act as reference targets (deep-dive §2.1).
- **Rich-text / collaborative editing** — the live multi-author (and multi-agent) editing
  engine, presence/awareness, and the **CRDT-vs-OT concurrency choice** (TE-15) *over* the
  shared content model (ADR-05). **This concurrency engine is Knowledge-owned, not shared.**
- **Databases + formula/rollup** — the structured-collection *instances*, the **formula/rollup
  incremental dataflow engine** (TE-18), and the physical storage/query execution for
  flexible user-defined fields (TE-17). The *field-definition + view + query-AST primitive* is
  **shared** (ADR-06); the *engines* are Knowledge-owned.
- **The page-permission tree** — hierarchical ACL inheritance with explicit overrides — as a
  *projection into* the shared ReBAC model (ADR-03), not a bespoke ACL system.
- **Version history / snapshots** — restore points, diffs, named versions, and the
  compaction/GC of the op-log (deep-dive §2.8).

### What it DELEGATES to the shared systems

Knowledge implements the three glue contracts (ADR-13) and delegates everything cross-cutting:

| Concern | Delegated to | Notes |
|---|---|---|
| Identity, page-tree ACL, agent delegation | **Identity & Access** (ADR-03) | Page-tree inheritance *compiles to* ReBAC tuples; no bespoke ACL |
| Event emission/consumption | **Event Bus** (ADR-04, ADR-13) | Transactional outbox; coalesced semantic events, **not raw keystrokes** |
| Mentions/relations/backlinks | **Reference Graph** (ADR-13) | Knowledge is the **heaviest producer & consumer** of refs |
| Full-text + structured + vector search | **Search** (ADR-03, ADR-10) | Permission-aware at query time; multilingual; vector for agent RAG |
| Mentions/comment/share/watch alerts | **Notifications** (ADR-12) | "What needs *me*" inbox |
| Agent authors/readers/triggers | **Agent Fabric** (ADR-08) | Mock now, real later by strategy-pattern swap |
| Block/row/CRDT-update durable storage; media | **Storage** (ADR-10) | OLTP for tree/rows; **object store** for media/snapshots; KMS/crypto-shred |
| DSR/erasure/audit/retention | **GDPR/Audit** (ADR-12) | Knowledge is the **hardest GDPR surface** in Myelin |
| Long-running/scheduled automations (daily-notes, living docs) | **Durable-workflow** (ADR-09) | Scheduled + event-driven page/row maintenance |

The shared **rich-content/block model** (`myelin-content`, ADR-05) is the canonical content
representation: Knowledge **leads** its block taxonomy (ADR-05 names Knowledge as the lead,
Chat/Issues as consumers) and owns the *collaboration mechanism over it*, but the AST itself —
including the `mention(Principal)`, `artifact_ref(ArtifactRef)`, and `embed(ArtifactRef)` inline
nodes — is shared and is the producer of `ref.created` events platform-wide.

---

## 2. High-level internal structure

The subsystem is a set of Rust services (ADR-02, ADR-14) inside a region-pinned cell (ADR-11),
each owning a slice of the domain and talking to the shared layer only via the glue contracts.
These are *architecture-altitude* components, not a module list.

```
┌───────────────────────────────────────────────────────────────────────────┐
│  KNOWLEDGE SUBSYSTEM (Rust services; one region-pinned cell)               │
│                                                                            │
│  ┌──────────────────┐   ┌───────────────────┐   ┌──────────────────────┐  │
│  │ Document Service │   │ Collaboration /   │   │ Database Service     │  │
│  │ (block tree,     │◄─►│ Sync Engine       │   │ (rows, schema, the   │  │
│  │  pages, hierarchy│   │ (CRDT|OT — TE-15; │   │  shared field/view   │  │
│  │  versions/history│   │  presence relay;  │   │  primitive ADR-06;   │  │
│  │  ADR-05 content) │   │  authoritative for│   │  formula/rollup      │  │
│  └────────┬─────────┘   │  perms/schema/    │   │  dataflow TE-18)     │  │
│           │             │  erasure on ops)  │   └──────────┬───────────┘  │
│           │             └─────────┬─────────┘              │              │
│           │                       │ firehose (ops/presence)│              │
│  ┌────────▼───────────────────────▼────────────────────────▼──────────┐  │
│  │  Projection / Render service  — resolves ArtifactRef → rendered      │  │
│  │  projection per viewer (ADR-13); unfurls; embedded live views        │  │
│  └────────┬─────────────────────────────────────────────────┬──────────┘  │
│           │                                                  │             │
│  ┌────────▼─────────┐   ┌──────────────────┐   ┌─────────────▼──────────┐ │
│  │ Permission       │   │ Indexing/Outbox  │   │ Automation/Trigger     │ │
│  │ projector        │   │ feeder (events →  │   │ adapter (tool defs,    │ │
│  │ (page-tree → authz│  │  Bus/Search/Refs) │   │  triggers, agent edits │ │
│  │  tuples, ADR-03) │   │  semantic+coalesced│  │  via collab layer)     │ │
│  └──────────────────┘   └──────────────────┘   └────────────────────────┘ │
│           │                       │                          │            │
│  ┌────────▼──────────┐  ┌─────────▼─────────┐  ┌─────────────▼──────────┐ │
│  │ GDPR holder        │ │ Storage adapter   │  │ Export/Import service   │ │
│  │ (locate/export/    │ │ (OLTP tree/rows;  │  │ (lossless JSON, MD,     │ │
│  │  rectify/erase;    │ │  object media;    │  │  HTML/PDF, CSV;         │ │
│  │  crypto-shred)     │ │  CRDT snapshots)  │  │  Art. 20 portability)   │ │
│  └────────────────────┘ └───────────────────┘  └─────────────────────────┘ │
└───────────────────────────────────────────────────────────────────────────┘
       │ authz          │ events/outbox     │ refs        │ search    │ gdpr
       ▼                ▼                   ▼             ▼           ▼
   Identity         Event Bus          Reference Graph  Search     GDPR/Audit
```

**Components, at altitude:**

1. **Document Service** — owns the block tree, pages, the page hierarchy (sub-pages = the
   folder-like nesting; deep-dive §2.3), version history and snapshots. Authority for *what a
   page is*; persists block tree + CRDT/OT updates + snapshots via the Storage adapter.
2. **Collaboration / Sync Engine** — the live editing path: applies/relays edit ops, broadcasts
   presence/awareness, and is the **authority for what the merge layer cannot enforce** —
   permission checks on incoming updates, schema validation for database rows, and erasure
   (deep-dive §6). Runs over a **firehose transport** (ADR-04), *not* the durable bus; emits
   only coalesced semantic events to the bus. The CRDT-vs-OT engine choice (TE-15) lives here.
3. **Database Service** — instances of the **shared structured-collection primitive** (ADR-06):
   typed field definitions, views (table/board/calendar/timeline) as query-AST projections
   (ADR-07), rows, two-way relations, and the **formula/rollup incremental dataflow engine**
   (TE-18) and **flexible-field query execution** (TE-17) — both Knowledge-owned.
4. **Projection / Render service** — implements the `ArtifactRef` resolution contract (ADR-13):
   given a ref and a viewer, returns the current rendered projection for unfurls/embeds, a
   permission decision, and update-event hooks for cache invalidation. This is how *other*
   subsystems embed a Knowledge page/block without touching its DB, and how Knowledge embeds
   *their* artifacts (a live issue board in a doc).
5. **Permission projector** — translates the page-tree (inheritance + overrides) into ReBAC
   relationship tuples (ADR-03) so Knowledge reads are answered by the platform `check` /
   `list-objects` primitives, never a bespoke ACL. Row-/field-level visibility (a scoped
   feature, deep-dive §2.7) compiles to tuples + ABAC-at-the-edge predicates (ADR-03).
6. **Indexing / Outbox feeder** — writes events to the transactional outbox in the *same* DB
   transaction as the state change (ADR-04), feeding Bus → Search/Refs/Notif/OLAP. Coalesces
   high-frequency edits into semantic `kb.page.updated` events; never emits per-keystroke.
7. **Automation / Trigger adapter** — registers Knowledge `ToolDef`s into the shared
   `ToolSurface` (ADR-08) and applies agent-authored edits *through the same collaboration
   protocol* as humans (so attribution/undo/history treat agent edits as first-class).
8. **GDPR holder** — implements `PersonalDataHolder` (ADR-12): locate/export/rectify/restrict/
   erase across blocks, rows, history, mentions, and authorship; crypto-shred per-subject/tenant
   keys for immutable history.
9. **Storage adapter** — OLTP (block tree + rows), object store (media + snapshots), residency-
   pinned, per-tenant envelope-encrypted (ADR-10, ADR-12).
10. **Export/Import service** — lossless JSON (the spine of both portability and Art. 20),
    Markdown/HTML/PDF, CSV for databases; scopeable to a data subject for DSAR (deep-dive §2.10).

---

## 3. Technology

Default **Rust** for all services (ADR-02, ADR-14), justified divergence only in writing. The
Knowledge-specific picks, consistent with the spine:

| Concern | Choice (Phase-2 direction) | Rationale / spine tie |
|---|---|---|
| Service language | **Rust** | ADR-02 default; no material reason to diverge. The CRDT story *favours* Rust (Yrs is Rust-native). |
| Collaboration engine | **CRDT, leading candidate Yrs (Rust Yjs)** for text/list/map, layered with a tree/move CRDT for block structure — **TE-15, to prototype in P4** | Deep-dive §6 leading candidate: offline-first aligns with UX; "dumb relay + persistence" servers scale horizontally; Rust-native. **Not finally decided** — OT remains viable if we accept online-only; P4 prototypes & benchmarks. |
| Block-tree storage | **OLTP (Postgres-class) per-block rows (adjacency list + fractional/ordering key)** as source of truth, with CRDT update-log + periodic compacted snapshots — **TE-16, P4** | ADR-10 OLTP tier. Per-block rows scale to huge docs and enable block-level refs/permissions; single-blob caps doc size. Exact model is a P4 trade-off. |
| Flexible-DB storage/query | **JSONB property-bag source-of-truth + derived indexable projection (generated columns / search index) maintained off the bus** — **TE-17, P4** | ADR-06/ADR-10. Avoids per-tenant DDL sprawl; the "JQL performance trap" is the dominant per-subsystem risk and is **not** solved by sharing — it is Knowledge's P4 problem. |
| Formula/rollup | **Async incremental dataflow off the bus** (spreadsheet-style dependency graph; eventual consistency stated) — **TE-18, P4** | Deep-dive §2.4: editing one cell cascades recompute; cycle detection + fan-out caps; mirrors ADR-08 loop governance. |
| Media/blobs | **S3-compatible object store** (MinIO/Ceph self-hostable), content-addressed, residency-pinned | ADR-10 object tier. |
| Search | **Shared Search** (Tantivy/OpenSearch/Meilisearch-class), block- or page-level docs (TE/§10 open), multilingual, vector for RAG | ADR-03/ADR-10; permission-aware via `list-objects`. |
| Editor / renderer (frontend) | **One shared editor over `myelin-content`** (ADR-05); frontend stack set by the design-language deliverable (TS/React-class baseline, not mandated) | ADR-05: "share the AST, not the editor" — but one editor *component* is built against the shared model. |
| Content model | **`myelin-content`** shared crate (ADR-05) | Knowledge leads the taxonomy; concurrency stays Knowledge-owned. |
| Structured-collection primitive | **`myelin-query`** shared crate (ADR-06, ADR-07) | Field defs + views + query AST shared; engines Knowledge-owned. |

**No divergence from the Rust default is requested.** The only "specialised tech" (Yrs/CRDT) is
itself Rust-native, reinforcing the default rather than diverging from it.

---

## 4. Views / screens the UI requires

> UX is a first-class requirement (VISION §3). This enumerates the **primary screens** with key
> states; visual design + wireframes (including empty/loading/error) are produced in the design-
> language work and the Phase-4 design sketches *before* UI code (VISION §3, §5.2). One editor
> over `myelin-content` and one views component over the shared db/views primitive (ADR-05/06)
> are reused here.

1. **The block editor (page view)** — the core surface. Block-based WYSIWYG: slash-command (`/`)
   insert menu, drag-handles to move/reorder, nested/indentable lists, inline `@`-mention &
   `ArtifactRef` autocomplete, markdown shortcuts, paste fidelity (web/Word/MD → blocks),
   image/file upload & embed, code blocks with highlighting, tables, **live presence**
   (cursors/avatars). *States:* empty (new page), loading (lazy block load for huge docs),
   read-only (no edit permission), offline/syncing, conflict-resolving, **agent-suggesting**
   (agent-authored edits shown distinctly with accept/reject), tombstoned-reference placeholder.
2. **Database views** — Table (inline-edit cells, resize/reorder columns, add property), Board
   (drag cards between status columns), Calendar (drag to reschedule), List, Gallery, Timeline.
   Per-view filter/sort/group UI; row "peek" / open-row-as-page. *States:* empty database,
   schema-editing, filtered-empty, loading large result set, permission-filtered (rows the
   viewer can't see simply absent — never post-filtered), formula-recomputing.
3. **Navigation sidebar** — tree of spaces → pages → sub-pages; favorites/pins; recent;
   quick-switcher / command palette (full-text search). *States:* empty workspace, search-no-
   results, deep-tree virtualised loading.
4. **Backlinks / references panel** — "Linked references" / "mentioned in" on every page;
   hover-preview (peek a doc/issue/commit/run without leaving). *States:* no backlinks,
   permission-filtered (only refs from things you can read), referenced-artifact-erased
   (graceful tombstone), live update on referenced-artifact change.
5. **Comments & discussion** — inline comments anchored to a text range or block; threads;
   resolve; @-mention to notify. (Overlaps Chat — §6; whether it reuses the chat threading
   primitive is **[OPEN → P4]**.)
6. **History UI** — version timeline, diff view, restore-to-version. *States:* no history,
   crypto-shredded/erased segment shown as redacted, restore-confirm.
7. **Sharing / permissions dialog** — member/guest management, link sharing, **publish-to-web**
   (with explicit personal-data warning + lawful-basis prompt, deep-dive §8). *States:* inherited
   vs overridden ACL, public-published warning, guest/link-only.
8. **Templates UI** — insert-from-template, new-from-template, org template gallery. *States:*
   empty gallery, template that pre-seeds personal-data fields (GDPR-flagged).
9. **Search / quick-switcher palette** — cross-type (page/db/row) results, permission-filtered,
   multilingual. *States:* loading, no-results, semantic-vs-keyword toggle.
10. **Agent affordances** (woven through, not a standalone screen) — agent presence in a doc,
    "suggested by agent" attribution, accept/reject of agent edits, "ask an agent" on a doc
    ("summarise", "turn this into issues"), and **HITL approval cards surfaced via Chat**
    (ADR-08/ADR-09) for consequential agent actions. Must work with **mock agents** in dev.
11. **Mobile/responsive read + light edit** — at least read + light edit offline (offline depth
    is a P4 scoping decision that interacts with the CRDT choice, deep-dive §10 Q9).

---

## 5. CLI commands

> Per VISION the CLI must cover the subsystem and be **scriptable / machine-output-friendly**
> (`--format json` everywhere) because agents and CI call it. Namespace: `myelin kb …`. These
> are *expected shapes*; final naming is P4. Every command authorizes via one `Principal`
> (ADR-13) and is auditable (ADR-12).

**Pages / docs**
```
myelin kb page list   [--space <s>] [--parent <id>] [--format json]
myelin kb page get    <id> [--format md|json|html]
myelin kb page create [--space <s>] [--parent <id>] [--title ...] [--from-file <md>] [--template <t>]
myelin kb page edit   <id> [--from-file <md>]      # replace body
myelin kb page append <id> --from-file <md>
myelin kb page move   <id> --to <parent>
myelin kb page archive|delete <id>
myelin kb page history <id> [--format json]
myelin kb page restore <id> --version <v>
myelin kb page export  <id|--space <s>|--all> --format md|json|pdf --out <dir>
myelin kb page publish|unpublish <id>
```

**Databases**
```
myelin kb db create --space <s> --name ... [--schema <file>]
myelin kb db schema get <db>
myelin kb db schema add-property <db> --type <t> --name ...
myelin kb db row add    <db> --set key=value …            # incl. --set ref=myelin://...
myelin kb db row list   <db> [--view <v>] [--filter <ast>] [--sort <prop>] [--format json]
myelin kb db row get|update|delete <db> <row>
myelin kb db view create <db> --type table|board|calendar|timeline --group-by <prop>
myelin kb db import <db> --csv <file>
myelin kb db export <db> --csv|--json
```

**Permissions / references / GDPR / agents**
```
myelin kb share <page-or-db> --grant <principal>=<role>   # read|comment|edit|manage
myelin kb share <page-or-db> --revoke <principal>
myelin kb backlinks <id> [--format json]                  # what references this
myelin kb refs      <id> [--format json]                  # what this references
myelin kb export-subject --principal <user> --out <dir>   # DSAR: all KB content by/about subject
myelin kb erase-subject  --principal <user> [--dry-run]   # erasure workflow (anonymise + crypto-shred)
myelin kb watch <id> [--format json]                      # stream events (powers triggers/agents)
myelin kb template list|apply
```

---

## 6. Usage examples (end-to-end)

### 6.1 Incident runbook references a live, failing CI run (the wedge)

**UI flow.** An on-call engineer opens the *Incident: API 5xx spike* page. They type `/embed`
and paste the CI run URL; the editor inserts an `embed(ArtifactRef)` node. The
Projection/Render service resolves `myelin://acme/ci/run/RUN-991` *for this viewer* (ADR-13),
returning the run's current status projection — rendered inline, kept **live** by `ci.run.*`
bus events (deep-dive §7.2). They `@`-mention the responsible issue; a `mention`/`artifact_ref`
node emits `ref.created`, so the issue's backlinks panel now shows "mentioned in Incident
runbook" (permission-filtered).

**CLI / API equivalent.**
```
$ myelin kb page create --space ops --title "Incident: API 5xx spike" \
    --from-file incident.md
# => created myelin://acme/kb/page/PAGE-7c2

$ myelin kb db row add incidents \
    --set title="API 5xx spike" \
    --set severity=high \
    --set ref=myelin://acme/ci/run/RUN-991
# => row created; artifact-reference property emits kb.reference.added → Reference Graph

$ myelin kb backlinks myelin://acme/ci/run/RUN-991 --format json
# => [{ "from": "myelin://acme/kb/page/PAGE-7c2", "type": "embed" }, ...]
```

What happened across the platform: Knowledge mutated its own state + wrote the outbox event in
one transaction (ADR-04 §7.1); the bus fanned `kb.reference.added` to **Refs** (edge created),
**Search** (indexed), and any **agent trigger** watching the incidents database. No subsystem
touched another's DB; the embed renders via CI's projection API (ADR-13).

### 6.2 Agent turns a meeting-notes page into issues (agent-native, mock now)

**UI flow.** In a meeting-notes page, the author clicks *Ask agent → "turn action items into
issues"*. A **mock** `TriageAgent` (ADR-08, strategy pattern) wakes on the trigger, reads the
page projection, and returns an `AgentDecision { effects: [issue.create×3, ref.create×3,
kb.page.update (link the created issues back)] }`. It performs **no side effects** itself
(plan-then-apply, ADR-08). The `EffectApi` validates each effect against
permissions ∩ delegation ∩ tenant policy (ADR-03); `issue.create` on a public project is
non-sensitive and applies; the page edit is applied **through the collaboration protocol** with
"suggested by agent" attribution, which the author can accept/reject. Every action is recorded in
the tamper-evident audit log (ADR-12).

If any effect were consequential on a protected resource, the Agent Fabric would open a **HITL
gate** on the durable-workflow substrate (ADR-09) surfaced as a **Chat approval card** —
the same machinery as `system-overview.md §8.2`. The *same mock code* runs deterministically in
dev and an `LlmAgentRuntime` later with **zero platform changes** (ADR-08).

```
$ myelin kb watch myelin://acme/kb/page/PAGE-meeting --format json
# stream shows: kb.page.updated, then the agent's proposed effects awaiting accept/reject
```

### 6.3 DSAR / erasure for a departed contributor (compliance on the same structure)

A DPO runs `myelin kb export-subject --principal alice` then `myelin kb erase-subject
--principal alice`. The Knowledge GDPR holder participates in the platform DSR fan-out
(`system-overview.md §8.3`): it locates structured personal data reliably (person properties,
mentions, author/edit attribution), exports it, then on erasure **anonymises authorship**
(reassign to a pseudonymous "Deleted user" to preserve others' work, deep-dive §8), tombstones
mentions so backlinks degrade gracefully, purges the **search index** in lockstep (no leak), and
**crypto-shreds** per-subject keys to reach immutable history/op-logs (ADR-12). Free-text PII in
prose is handled by tooling + a documented process — an **honest, stated limitation**, not an
over-promise (deep-dive §8; GD-6 `[OPEN — LEGAL]`).

---

## 7. Interactions with other subsystems & shared systems

### 7.1 Events emitted (semantic; via transactional outbox, ADR-04/ADR-13)

Coalesced/debounced — **raw keystrokes stay in the firehose collab layer**; the durable bus
gets semantic events (deep-dive §7.1):

- **Pages:** `kb.page.created/updated/moved/archived/deleted/restored/published/unpublished`.
- **Databases/rows:** `kb.database.created`, `kb.database.schema.changed`,
  `kb.database.view.created/updated`, `kb.row.created/updated(+delta)/deleted/moved`.
- **References:** `kb.reference.added/removed` → **Reference Graph** (Knowledge is the heaviest
  producer; deep-dive §2.6).
- **Comments/mentions:** `kb.comment.added/resolved`, `kb.mention.created` → **Notifications**.
- **Permissions:** `kb.access.granted/revoked`, `kb.page.published` (security-relevant + audit).
- **GDPR/audit:** `kb.subject.export.*`, `kb.subject.erasure.*`, plus audit entries for every
  access/permission change.

Every envelope carries the non-negotiable fields (ADR-13): `event_id` (idempotency), `tenant`,
`region`, `actor` (human/agent/service incl. on-behalf-of), `subject` (`ArtifactRef`),
`causation_id`/`correlation_id`, `contains_personal_data`, `visibility`.

### 7.2 Events consumed

- **Identity:** user deprovisioned/anonymised → erasure/ownership reassignment; team/membership
  changes → recompute view membership & ACL projections; org/project lifecycle.
- **Cross-artifact lifecycle (for live refs/embeds):** `issue.updated/closed`,
  `ci.run.completed`, `git.commit.pushed`, `chat.message.posted` → refresh embedded views &
  mention previews; update artifact-reference properties; drive agent-maintained living docs.
- **Reference-graph:** referenced artifact deleted/erased → tombstone the reference, degrade
  rendering gracefully (no dangling crash; deep-dive §2.6).
- **Agent fabric:** "agent produced a draft/edit/suggestion for page X" → apply via the collab
  layer with agent attribution (mock in dev).
- **Search:** (re)index acknowledgements for consistency signalling.
- **Durable-workflow / scheduled:** "create today's daily-note from template"; "incident opened
  → create incident doc from runbook template".

### 7.3 The two tightest seams

- **Issues ↔ Knowledge (ADR-06)** — *the biggest reuse decision in the platform*. Both implement
  the **shared structured-collection primitive** (field defs + views + query AST in
  `myelin-query`); each owns its execution and engines. Knowledge databases embed issue views
  and relate rows to issues via the relation field type (which rides Refs where cross-artifact —
  TE-7, ADR-13). **Named joint P4 design.** The boundary line is "share the schema language and
  view model, not the query planner or the workflow/formula engine."
- **Chat ↔ Knowledge** — doc comments vs chat threads overlap; thread→doc summarisation is an
  agent use case; HITL approval cards for Knowledge agents surface in Chat (ADR-09). Whether doc
  comments reuse the Chat threading primitive is **[OPEN → P4]** (deep-dive §10 Q12).

### 7.4 Authz, search, refs (the cross-cutting invariants)

- **Permission-aware reads everywhere (ADR-03, `system-overview.md §5.2`).** Backlinks, database
  views, search results, and embedded-view contents are **pre-filtered** by Id's `list-objects`,
  never post-filtered. A leak is both a security and a GDPR breach (SC-1). The page-tree
  inheritance compiles to ReBAC tuples; row-/field-level visibility uses ABAC-at-the-edge
  predicates kept off the hot list-objects path (ADR-03).
- **Refs render by current state, tombstone on erasure** (ADR-13): references target by id, render
  by current title, degrade to a neutral placeholder on deletion/erasure.

### 7.5 Agent tools & triggers registered (ADR-08)

Knowledge registers typed `ToolDef`s into the shared `ToolSurface` (one catalogue, internal +
MCP-exposable later), e.g.: `kb.page.create`, `kb.page.append`, `kb.row.upsert`,
`kb.page.summarise` (read-only), `kb.search`. Each declares input JSON-schema, required caps,
effect kind, and a side-effecting flag. Triggers bind an `EventMatcher` (query AST, ADR-07) to a
target under a `run_as` principal with a `RunBudget`, `DelegationPolicy`, and HITL `gates`.
**Agent edits flow through the same collab protocol as humans** so undo/history/attribution treat
them as first-class (deep-dive §6).

### 7.6 PersonalDataHolder duties (ADR-12)

Knowledge is an exhaustive-list holder implementing `locate / export / rectify / restrict /
erase`. Specific duties (deep-dive §8): anonymise authorship rather than delete content; reach
into **history/version snapshots and the CRDT op-log** via crypto-shred (op-logs are append-only
and merge-dependent — you cannot simply delete an op); tombstone backlinks to erased subjects;
purge the search **and any vector/embedding** index in lockstep (embeddings of personal data are
personal data); lawful-basis + easy unpublish + CDN purge for **published/public pages** (a
high-risk export); per-space/per-database retention and lawful-basis recording. Content carries
`contains_personal_data`/erasure hooks (ADR-05). **Honest limitation:** full automated free-text
PII detection is not perfectly solvable — reliable for *structured* personal references, tooling
+ documented process for free-text (deep-dive §8; GD-6 `[OPEN — LEGAL]`).

---

## 8. Implications for the shared systems (flag for Phase 3)

These are needs Knowledge places on shared systems — Phase 3 owns the mechanism:

1. **Reference Graph must carry block-granular sub-artifact refs** (`#block-id`) and tombstone
   gracefully, with backlinks permission-filtered at read time via `list-objects` (ADR-03/ADR-13).
   Knowledge is the heaviest producer/consumer; refs must stay consistent under rename/move
   (target-by-id). **Whether two-way db relations live *in* Refs or as a Knowledge-local
   materialised index projected into Refs is TE-7 — flag for P3/P4.**
2. **Search must support both block-level and page-level index documents, multilingual analysis,
   structured-field queries over flexible db fields, and vector/semantic search** for agent RAG —
   all ACL-aware via `list-objects` (ADR-03/ADR-10). Block-vs-page granularity is open (deep-dive
   §10 Q10) and affects index size; P3 Search must accommodate either.
3. **Event Bus / firehose split must accommodate the collab op-stream + presence** as a dedicated
   firehose transport (ADR-04), with the durable bus carrying only coalesced semantic
   `kb.page.updated` pointer events — never per-op/per-keystroke. P3 must size the firehose for
   collab.
4. **Storage / KMS must support crypto-shred granular enough to erase a subject from append-only
   CRDT op-logs and version history** (ADR-12). Per-subject vs per-tenant crypto-shred granularity
   (GD-4) directly bounds how cleanly Knowledge history erasure works.
5. **`myelin-content` (ADR-05) and `myelin-query` (ADR-06/07) crates** must expose the block
   taxonomy + the field-definition/view/AST primitive that Knowledge **leads**. P3/P4 ratify the
   taxonomy completeness and extension mechanism (ADR-05 defers this to P4 with Knowledge leading).
6. **Durable-workflow substrate (ADR-09)** must support scheduled + event-driven page/row
   maintenance (daily-notes, living docs) and the HITL gates for Knowledge agents (surfaced via
   Chat). P3's build-vs-adopt decision must keep these affordances.
7. **Identity must express page-tree inheritance + overrides, plus optional row-/field-level
   visibility**, in the ReBAC tuple algebra without an N+1 check on the hot read path (ADR-03).
   The exact tuple/namespace schema for the page tree is a P3 deliverable.

---

## 9. Open questions for Phase-4 detailed architecture

Carrying forward the deep-dive §10 register, tagged with the spine's `[OPEN → P4]` items:

1. **CRDT vs OT, and granularity** (per-doc vs per-block vs hybrid CRDT) — **TE-15**. Lean: CRDT
   (Yrs) + tree/move CRDT; **must be prototyped & benchmarked in P4** (deep-dive §6; ADR-05/14).
   *High uncertainty.*
2. **Block-tree storage model** — per-block rows (adjacency list) vs single-CRDT-blob vs hybrid —
   **TE-16**. Trade-off: doc size & block-level features vs collaboration simplicity.
3. **Flexible-DB query model** — JSONB property-bag + derived projection vs per-database
   materialised tables vs external query store — **TE-17**. The dominant per-subsystem risk; not
   solved by sharing (ADR-06). *Needs scale prototyping.*
4. **Formula/rollup engine** — sync vs async incremental dataflow; consistency guarantees;
   fan-out limits; cycle handling — **TE-18**. Mirrors ADR-08 loop governance.
5. **Permission granularity** — page/database-level only, or row-level and field-level? Corporate
   buyers likely want the latter; cost is significant (deep-dive §2.7). *Scope decision.*
6. **Folders vs pure-pages hierarchy** — explicit folder concept vs Notion-style "everything is a
   page" (deep-dive §2.3). *Open; corporate familiarity vs flexibility.*
7. **Erasure from immutable history** — crypto-shred vs history-rewrite/compaction vs
   anonymise-only — depends on KMS granularity (GD-4) + legal input (GD-6). *Genuinely hard,
   partially open.*
8. **Offline depth** — read-only, light-edit, or full offline-first? Drives the CRDT decision and
   sync complexity (deep-dive §10 Q9). *Scope.*
9. **Search granularity & vector-in-v1** — block-level vs page-level index docs; commit to
   semantic/vector search in v1 (needed for agent RAG)? (deep-dive §10 Q10.)
10. **Synced blocks / transclusion in v1?** — breaks the clean tree; complicates permissions,
    erasure, reference-counting. Possibly defer (deep-dive §10 Q11). *Scope.*
11. **Comments: reuse Chat threading primitive or KB-native?** — cross-subsystem (deep-dive §10
    Q12). *Open.*
12. **How "live" are embedded views & artifact refs** — real-time via bus vs on-load vs cached?
    Cost/consistency trade-off (deep-dive §10 Q13).
13. **Multi-region collab + EU residency** — where does the authoritative collab server for a doc
    live, and how is session state sharded under residency constraints (ADR-11; deep-dive §10
    Q14)? *Hard; interacts with the SC-2/SC-3 multi-cell-tenant question carried to P3.*
14. **Templating as a shared capability** — page/db templates intersect issue + CI templates;
    candidate for a shared templating capability rather than per-subsystem reinvention (deep-dive
    §2.5). *Cross-subsystem; flag.*

---

## 10. Coherence note

This document does **not diverge** from the Phase-2 spine. It ratifies and applies ADR-02 (Rust),
ADR-03 (ReBAC page-tree projection + permission-filtered reads), ADR-04 (outbox + firehose split
for collab), ADR-05 (shared content model, Knowledge-led taxonomy, Knowledge-owned concurrency),
ADR-06/07 (shared field/view/query primitive, Knowledge-owned engines), ADR-08 (plan-then-apply
agents, agent edits via the collab protocol), ADR-09 (durable workflows for living docs + HITL),
ADR-10/14 (storage tiers; Rust + Yrs), ADR-11 (cell residency incl. collab-session locality),
ADR-12 (PersonalDataHolder; hardest GDPR surface), and ADR-13 (three glue contracts;
block-granular `ArtifactRef`s; projection API). The only Knowledge-specific "specialised" tech
(Yrs/CRDT) is Rust-native and therefore *reinforces* the Rust default rather than diverging from
it.
