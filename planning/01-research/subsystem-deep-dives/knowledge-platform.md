# Subsystem Deep-Dive: Knowledge Platform (Notion-class)

> Phase: `01-research`. This document **maps the territory**; it does not commit to a final
> architecture. Architecture decisions belong to phases `02`–`05`. Where this doc states a
> mechanism (e.g. "use a CRDT"), read it as "here is the design space and the leading
> candidate, with trade-offs" — not as a decree.
>
> Canonical brief: [`VISION.md`](../../../VISION.md). Nothing here may contradict it.

---

## 1. Purpose & role in Myelin

The Knowledge Platform is Myelin's **Notion-class workspace**: where an organisation hosts
its durable, human-authored (and increasingly agent-authored) knowledge — specs, RFCs,
design docs, runbooks, onboarding guides, meeting notes, decision logs, and **structured
data** (databases with table/board/calendar views). It is subsystem #4 of five.

Its differentiated role inside Myelin (vs. standalone Notion/Confluence) is that it is **not
a silo**. A doc lives in the same reference graph as commits, issues, CI runs, and chat
messages. A design doc can embed a live issue board; an incident runbook can reference the
exact CI run that failed; a PRD can backlink every issue that implements it. The platform's
value is the *connective tissue*, so the knowledge platform's first-class job is to be a
**rich, referenceable, agent-readable substrate** that the rest of the platform points at and
writes into.

Key positioning consequences:

- **Agent-native.** Agents are first-class authors and readers. An agent should be able to
  draft a doc, fill a database row, summarise a thread into a knowledge page, or keep a
  "living document" in sync with reality — all via the same event/trigger fabric humans use.
  Per VISION, during development these are **mock agents behind the strategy pattern**.
- **The structured-data half matters as much as the prose half.** Databases (Notion's
  killer feature) overlap heavily with the Issue Tracker (subsystem #3). A central
  cross-cutting question (see §10) is *how much* of "structured records with views" is shared
  infrastructure vs. duplicated. This is the single most important boundary decision for
  this subsystem.
- **GDPR substrate.** Free-text knowledge is where personal data leaks in uncontrolled ways
  (names in meeting notes, performance reviews, customer emails pasted into a page). Erasure
  and access requests are *harder* here than in structured subsystems. This is a defining
  constraint, not an afterthought.

---

## 2. Core domain concepts & data-model considerations

### 2.1 The block — the atomic unit

The dominant modern model (Notion, Craft, Coda, AnyType, and the open-source `BlockNote`,
`Editor.js`, and ProseMirror-node ecosystems) is **block-based**: a document is an ordered
tree of typed blocks rather than a single HTML/markdown blob.

A block typically has:

- a stable **id** (must survive moves, edits, collaboration — used as a reference target);
- a **type** (`paragraph`, `heading`, `bulleted_list_item`, `to_do`, `toggle`, `quote`,
  `code`, `callout`, `image`, `table`, `table_row`, `database_view`, `embed`,
  `synced_block`, `column_list`/`column`, `divider`, `equation`, …);
- **type-specific properties** (e.g. heading level, code language, checkbox state, caption);
- **rich-text content** (an array of styled spans — see §2.2);
- **children** (ordered list of child block ids) → the document is a tree;
- **metadata**: created/edited by, created/edited at, and collaboration state.

Design tensions to flag for later phases:

- **Tree storage model.** Three main options:
  (a) **adjacency list** (each block stores `parent_id` + an order key);
  (b) **materialised path / closure table** (fast subtree reads, costlier moves);
  (c) **document-as-single-aggregate** (the whole page is one JSON/CRDT blob).
  Notion historically used (a) with each block as a row and a `content: [ids]` array for
  ordering. Single-blob (c) is simplest for collaboration (one CRDT per doc) but caps
  document size and makes cross-block queries hard. Per-block rows (a) scale to huge docs and
  enable block-level permissions/references but make whole-doc atomicity and collaboration
  harder. **This is a core unresolved trade-off.**
- **Ordering.** Sibling order needs a representation that supports concurrent insertion
  without renumbering: **fractional indexing** (LexoRank / fractional keys), or a sequence
  CRDT (RGA / Fugue / Yjs's list type). Fractional indexing is simple but has
  interleaving/precision pitfalls under heavy concurrency; CRDT sequences handle concurrency
  natively at higher complexity. (See §6 for the collaboration discussion.)
- **Block reuse / transclusion.** "Synced blocks" and embeds mean a block can appear in
  multiple places. This breaks the pure-tree assumption (a block has *one* canonical home but
  *many* render sites) and complicates permissions, erasure, and reference counting.

### 2.2 Rich text — the inline model

Inline content is itself a small ordered model: a run of **styled spans** with marks
(bold, italic, code, strikethrough, color), **inline links**, **inline references/mentions**
(`@user`, `@issue`, `@doc`, `@page`, date mentions), and **inline equations**. ProseMirror's
schema model (nodes + marks) and Yjs's `Y.XmlFragment`/`Y.Text` are the reference
implementations. The key requirement: inline references must be **structured tokens** (typed,
with a target id), not just hyperlinks, so they participate in the reference graph (§ 2.6).

### 2.3 Pages, folders & hierarchy

- A **page** is a document (a root block subtree) that is independently addressable,
  permissioned, and referenceable. Pages nest (a page can contain sub-pages), giving the
  folder-like hierarchy — Notion deliberately unifies "folder" and "page".
- **Spaces / workspaces / teamspaces**: a tenancy and grouping layer above pages, carrying
  default permissions and membership. Myelin needs a clear mapping between knowledge-platform
  spaces and the platform-wide org/project/team model from the shared identity system (don't
  invent a parallel hierarchy).
- **Open question:** do we expose an explicit "folder" concept distinct from "page", or the
  Notion-style "everything is a page"? Distinct folders are more familiar to Confluence/Drive
  users and corporate buyers; pure pages are more flexible. (See §10.)

### 2.4 Databases — structured data with views

This is the hard, differentiating half. A **database** is a collection of **records (rows)**
conforming to a **schema of typed properties (columns)**, rendered through one or more
**views**.

Property/column types to support (Notion-parity baseline):

- primitives: `title`, `text`, `number`, `checkbox`, `date`/`date-range`, `url`, `email`,
  `phone`;
- choice: `select`, `multi-select`, `status` (grouped select with workflow semantics);
- people & files: `person`, `files/media`;
- computed: `formula` (expression language over other properties), `rollup` (aggregate over a
  relation), `created/edited time`, `created/edited by`, auto-increment id;
- **relation**: a link to rows in another database (one-/two-way; this is what turns flat
  tables into a graph and overlaps with the platform reference graph);
- Myelin-specific: an **artifact-reference** property type (a typed pointer to any platform
  artifact — issue, commit, CI run, chat thread, doc), so a database row can be "about" a
  real platform object.

Views (each is a saved query + presentation over the same underlying rows):

- **Table** (grid; the default), **Board/Kanban** (group-by a select/status property),
  **Calendar** (place rows by a date property), **List**, **Gallery** (card grid),
  **Timeline/Gantt** (range over dates), **Calendar/Timeline** combos.
- A view carries: filter predicate, sort order, group-by, visible/hidden properties,
  per-view property ordering & width, and (for board/calendar) the grouping/date property.
- Views must be **per-user-overridable vs. shared**: a shared view definition, with optional
  personal tweaks layered on top. This personal-vs-shared split recurs across the platform.

Data-model considerations for flexible databases (the genuinely hard part):

1. **Schema-flexibility vs. queryability.** Two archetypes:
   - **EAV / JSONB property bag** — each row stores a `properties` map keyed by property id.
     Trivially flexible (add/remove columns cheaply), but querying/filtering/sorting/grouping
     at scale is hard: you need expression indexes, generated columns, or a secondary query
     store. Postgres `jsonb` + GIN indexes + generated columns is a common pragmatic answer.
   - **Per-database materialised table** (a real SQL table per database, columns added via
     DDL) — fast native queries and constraints, but DDL-per-tenant-database at world scale
     is operationally heavy and fights multi-tenancy.
   A likely middle path: JSONB-of-record as source of truth + a **derived, indexable
   projection** (generated columns or a separate columnar/search index) maintained via the
   event bus. Flag as a top architectural decision.
2. **Formulas & rollups = a dependency graph.** Computed properties depend on other
   properties (possibly across relations). Editing one cell can cascade recomputation across
   many rows in other databases. This is effectively an **incremental computation / dataflow**
   problem (think a spreadsheet engine). Must decide: synchronous compute vs. async via the
   event bus; cycle detection; recompute fan-out limits; and consistency guarantees (eventual
   is probably acceptable, but must be stated). This is a known scaling pain point in Notion.
3. **Relations & rollups at scale.** Two-way relations require maintaining inverse links
   transactionally or eventually-consistently. Rollups aggregate over potentially large
   related sets — needs incremental aggregation, not recompute-on-read at scale.
4. **Relation to the reference graph.** A database "relation" and an "artifact reference" are
   both edges. Decide whether databases ride on the **shared cross-artifact reference graph**
   or maintain a private relation index that feeds it. Strong reuse argument for the former.

### 2.5 Templates

- **Page templates** (pre-filled block subtrees) and **database templates** (a row template
  with default property values + a body block subtree). Notion also has "template buttons"
  (a block that instantiates blocks inline) and recurring/repeating templates (e.g. daily
  notes).
- Templates intersect with the Issue Tracker (issue templates) and CI (pipeline templates) —
  candidate for a **shared templating capability** rather than per-subsystem reinvention.
- Consider org-level template galleries and the GDPR-relevant question of templates that
  pre-seed personal-data fields.

### 2.6 References, backlinks & mentions

- Any structured reference (`@`-mention, inline link to a page, relation, embedded view,
  artifact-reference property) creates an **edge in the cross-artifact reference graph** (a
  shared system). The knowledge platform is one of the heaviest *producers* and *consumers* of
  this graph.
- **Backlinks** ("linked references", "mentioned in") are the inverse-index read over that
  graph: "what references this page/block/artifact?" Must be efficient and permission-filtered
  (you only see backlinks from things you can read).
- References must be **stable under rename/move** (target by id, render by current title) and
  must **degrade gracefully** on deletion/erasure (tombstone → "deleted item" placeholder,
  not a dangling crash).

### 2.7 Permissions

- **Inheritance down the page tree** with explicit overrides at any node (the Notion/Drive
  model): a child inherits parent ACL unless it sets its own. Sub-pages, databases, and
  potentially individual blocks (synced blocks complicate this) are permission boundaries.
- Roles/capabilities likely: read / comment / edit / full-access(manage), plus
  share-with-link, guest access, and public-publish (a page published to the web — with
  serious GDPR implications).
- **Database-level vs. row-level vs. property-level** permissions: Notion is mostly
  page/database-level; corporate buyers often want row-level (e.g. "see only your team's
  rows") and field-level (hide salary column). This is a real complexity axis to scope.
- Permissions must be expressed in the **shared identity/access model**, not a bespoke ACL
  system. Cross-subsystem consistency (an embedded issue board respects issue-tracker
  permissions) is essential.

### 2.8 Versioning & history

- **Page history / version snapshots** (restore to a previous version, see diffs) — corporate
  must-have, and the substrate for audit.
- Granularities: per-edit operation log (from the CRDT/OT layer) vs. periodic snapshots vs.
  named versions. CRDT update logs give *infinite* fine-grained history but grow unbounded
  (compaction/GC needed); snapshots are bounded but coarse. Likely a hybrid: op-log for live
  collab + periodic compacted snapshots for history/restore.
- **Tension with GDPR erasure**: history retains old content, including personal data that was
  later removed. Erasure must reach into history (see §8).

### 2.9 Search & indexing

- Full-text search over rich text (blocks), plus **structured query** over database
  properties (filter/sort), plus reference-graph navigation. Three different query shapes over
  the same content.
- Must be **permission-aware** (no leaking content via search), **multilingual** (EU = many
  languages; stemming/tokenisation per language), and ideally support semantic/vector search
  for agent retrieval (RAG over the knowledge base is an obvious agent use case).
- Should ride the **shared search system**; the knowledge platform feeds it documents/rows via
  events. Decide indexing model: block-level vs. page-level documents (block-level enables
  jump-to-block but multiplies index size).

### 2.10 Export & portability

- Per-page, per-space, and whole-workspace export to open formats: **Markdown/CommonMark** (+
  front-matter), **HTML**, **PDF**, and **structured JSON** (lossless block tree). Databases
  export to **CSV** and JSON.
- Portability is both a **product feature** (avoid lock-in — aligns with the EU-sovereignty
  pitch: "your data is yours, leave any time") and a **GDPR Article 20 right** (data
  portability). The lossless-JSON export is the spine of both.

---

## 3. Key UX / views required

> UX is a first-class requirement per VISION. This section lists *what must exist*; visual
> design is owned by later phases.

- **The editor.** Block-based WYSIWYG: slash-command menu (`/` to insert any block),
  drag-handles to move/reorder blocks, nested/indentable lists, inline `@`-mention and
  reference autocomplete, markdown shortcuts (`#`, `-`, `>`, ` ``` `), copy-paste fidelity
  (paste from web/Word/markdown → blocks), image/file upload & embed, code blocks with syntax
  highlighting, tables, and live presence (cursors/avatars of collaborators).
- **Database views.** Table (inline-edit cells, resize/reorder columns, add property), Board
  (drag cards between columns), Calendar (drag to reschedule), List, Gallery, Timeline.
  Per-view filter/sort/group UI. Row "peek"/open-as-page (a row is itself a page with a body).
- **Navigation.** Sidebar tree of spaces → pages → sub-pages; favorites/pins; recent;
  full-text search palette (quick-switcher); breadcrumb trail.
- **Backlinks & references panel.** "Linked references" / "mentioned in" on every page;
  hover-preview of referenced artifacts (peek a doc/issue/commit without leaving).
- **Comments & discussion.** Inline comments anchored to a text range or block; comment
  threads; resolve; @-mention to notify. (Overlaps with Chat subsystem — see §9.)
- **History UI.** Version timeline, diff view, restore.
- **Sharing/permissions UI.** Share dialog, member/guest management, link sharing, publish.
- **Templates UI.** Insert-from-template, "new from template", template gallery.
- **Mobile/responsive & offline.** At least read + light edit offline (offline is a strong
  driver toward CRDTs). Flag offline depth as a scoping decision.
- **Agent affordances in UX.** Surface agent-authored edits/suggestions distinctly
  (attribution, "suggested by agent", accept/reject), agent presence in the doc, and a way to
  trigger an agent on a doc ("summarise", "turn this into issues"). Must work with **mock
  agents** during development.

---

## 4. CLI commands expected

> Myelin requires CLI coverage per the planning process. These are *expected shapes*, not a
> final spec; naming/namespacing is for the architecture phase. Assume `myelin kb …` (or
> `myelin doc …` / `myelin db …`) namespace.

Pages/docs:

- `myelin kb page list [--space <s>] [--parent <id>]`
- `myelin kb page get <id> [--format md|json|html]` — fetch/render a page
- `myelin kb page create [--space <s>] [--parent <id>] [--title ...] [--from-file <md>] [--template <t>]`
- `myelin kb page edit <id> [--from-file <md>]` / `myelin kb page append <id> --from-file <md>`
- `myelin kb page move <id> --to <parent>` / `myelin kb page delete <id>`
- `myelin kb page history <id>` / `myelin kb page restore <id> --version <v>`
- `myelin kb page export <id|--space|--all> --format md|json|pdf --out <dir>`
- `myelin kb search "<query>" [--space <s>] [--type page|db|row]`

Databases:

- `myelin kb db create --space <s> --name ... [--schema <file>]`
- `myelin kb db schema get <db>` / `myelin kb db schema add-property <db> --type <t> --name ...`
- `myelin kb db row add <db> --set key=value …` (incl. setting an artifact-reference)
- `myelin kb db row list <db> [--view <v>] [--filter ...] [--sort ...]`
- `myelin kb db row get|update|delete <db> <row>`
- `myelin kb db view create <db> --type table|board|calendar … --group-by <prop>`
- `myelin kb db import <db> --csv <file>` / `myelin kb db export <db> --csv|--json`

Permissions / references / GDPR:

- `myelin kb share <page-or-db> --grant <principal>=<role>` / `... --revoke ...`
- `myelin kb backlinks <id>` — what references this
- `myelin kb refs <id>` — what this references
- `myelin kb export-subject --principal <user>` — all KB content authored/about a subject (DSAR)
- `myelin kb erase-subject --principal <user> [--dry-run]` — erasure workflow (see §8)

Agent-/automation-oriented (for the agent fabric):

- `myelin kb watch <id>` — stream events for a page/db (powers triggers)
- Templating: `myelin kb template list|apply`.

Design steer: CLI should be **scriptable and machine-output-friendly** (`--format json`
everywhere) because agents and CI will call it.

---

## 5. Hardest technical problems for WORLD-SCALE

1. **Real-time collaborative editing at scale** (expanded in §6). Concurrent multi-user (and
   multi-agent) editing with low latency, correctness, offline support, and history — across
   potentially millions of documents and many connections per popular doc. The defining
   engineering challenge of this subsystem.
2. **The flexible-database query problem** (§2.4). Schemaless flexibility *and* fast
   filter/sort/group/aggregate at scale, multi-tenant, without per-tenant DDL sprawl. Plus the
   formula/rollup **incremental dataflow** engine and its fan-out under load.
3. **The reference graph as a hot path.** Backlinks, "mentioned in", and relation rollups
   require an inverse index that stays consistent and is **permission-filtered at read time**
   without N+1 permission checks. At scale this is a graph + access-control join problem.
4. **Permission-filtered fan-out everywhere.** Search results, backlinks, database views, and
   notifications must all be filtered by the viewer's access. Doing this cheaply (precomputed
   ACL materialisation, e.g. a Zanzibar-style relation tuple store) vs. correctly (no stale
   leaks) is a core tension shared with the whole platform.
5. **History/version storage growth & compaction.** CRDT op-logs and snapshots grow without
   bound; compaction/GC must preserve restore points and audit needs while respecting erasure.
6. **Storage of large/binary content.** Images, files, embeds → object storage (shared
   system), CDN delivery, EU residency, virus scanning, and dedup. Plus huge documents
   (thousands of blocks) needing partial/lazy load.
7. **Multi-region & EU data residency vs. global collaboration.** A collaboration session is
   latency-sensitive and stateful; pinning data/sessions to an EU region while still being
   "world-scalable" forces decisions about where the authoritative collab server for a doc
   lives and how presence/state shard. Residency constraints can conflict with "nearest edge".
8. **Consistency model.** Choosing where strong consistency is required (permission changes,
   schema changes, erasure) vs. where eventual is fine (backlinks, search index, rollups), and
   making that legible. Mixed-consistency systems are where correctness bugs hide.
9. **Hot-document & thundering-herd.** A company all-hands doc or a heavily-watched database
   gets thousands of concurrent readers/editors; needs connection multiplexing, awareness
   throttling, and read-replica/caching strategy.

---

## 6. The collaboration problem (CRDT vs. OT) — focused analysis

This deserves its own section because it dominates the subsystem's architecture and is the
area of highest uncertainty.

**The two families:**

- **OT (Operational Transformation)** — clients send operations; a central server transforms
  them against concurrent ops to converge. Powers Google Docs. Pros: compact, server-mediated,
  mature for text. Cons: transformation functions are notoriously hard to get correct for rich
  trees; effectively *requires* a central authoritative server (weaker offline/P2P story);
  custom block types each need transform logic.
- **CRDTs (Conflict-free Replicated Data Types)** — data types that merge deterministically
  without a central coordinator. Modern sequence CRDTs (RGA, **Yjs/Yrs**, Automerge, **Fugue**
  to fix interleaving) handle text/lists; map CRDTs handle block properties. Pros: offline-
  first, P2P-capable, server can be "dumb relay + persistence", strong ecosystem
  (**Yjs** and its Rust port **Yrs** — relevant given Myelin's Rust steer). Cons: metadata
  overhead (tombstones, ids per char historically — improved by modern encodings), garbage
  collection/compaction needed, and **CRDTs guarantee convergence, not application-level
  invariants** (e.g. they won't enforce permissions or schema constraints — those live above).

**Why this is hard for a *block tree* (not just text):**

- Moving a subtree concurrently with edits, concurrent re-parenting (can create cycles —
  needs a "move" CRDT à la Kleppmann's move operation or last-writer-wins-with-cycle-break),
  and concurrent insert into the same position (interleaving anomalies → Fugue/peritext-style
  solutions). Rich-text marks across concurrent edits → **Peritext** is the reference research
  for CRDT rich text.
- **Block-granular vs. document-granular CRDT.** One CRDT per document is simplest and is what
  most Yjs deployments do; but it bounds doc size and couples permissions to whole docs. A
  CRDT-per-block or hybrid (tree structure CRDT + per-block content CRDT) scales better and
  enables block-level features but is far more complex and less battle-tested.

**Leading candidate (to be confirmed in architecture):** a **CRDT-based** approach using
**Yrs (Rust Yjs)** for text/list/map content, layered with a **tree/move CRDT** for block
structure, with a server acting as **persistence + relay + the authority for non-CRDT
concerns** (permissions, schema validation, erasure, snapshots, fan-out to the event bus).
Rationale: offline-first aligns with UX goals, Rust-native (Yrs) aligns with the tech steer,
and "dumb relay" servers scale horizontally more easily than OT's transform-everything
servers. **Uncertainty is high** — OT (à la what ProseMirror's `prosemirror-collab` or a
server-authoritative model offers) remains viable, especially if we accept "online-only"
editing and want simpler server-side invariant enforcement. This must be prototyped and
benchmarked in the architecture phase, not decided here.

**Cross-cutting hard parts regardless of family:**

- **Awareness/presence** (cursors, selections, who's here) — ephemeral, high-frequency, needs
  throttling and a pub/sub channel; usually *not* persisted.
- **Agents as collaborators.** A mock/real agent editing a doc must go through the *same*
  collab protocol (apply ops/updates), so attribution, undo, and history treat agent edits as
  first-class. Strategy-pattern boundary: the "collaborator client" interface should not care
  whether the peer is human or agent.
- **Server-side enforcement of what CRDTs can't enforce.** Permission checks on incoming
  updates, schema validation for database rows, and erasure must be enforced by an
  authoritative layer above the merge layer.
- **Persistence & snapshotting.** Periodically compact the op stream into a snapshot; store
  updates durably (the shared storage system) with EU residency; bound history growth.

---

## 7. Events this subsystem must EMIT and CONSUME

> Events flow through the **shared event bus**; triggers feed the **agent fabric**. Per
> VISION, agent integration is first-class and event-driven. Exact schemas/envelope are owned
> by the shared-systems architecture; below are the *semantic* events.

### 7.1 Emits (produces)

- **Page/doc lifecycle:** `kb.page.created`, `kb.page.updated` (coalesced/debounced; raw
  keystrokes stay in the collab layer — emit semantic change events, not every op),
  `kb.page.moved`, `kb.page.archived`, `kb.page.deleted`, `kb.page.restored`,
  `kb.page.published` / `kb.page.unpublished`.
- **Block-level (optional, higher volume):** `kb.block.inserted/updated/deleted` — likely
  internal/opt-in due to volume; agents may subscribe to coarser page events.
- **Database/schema:** `kb.database.created`, `kb.database.schema.changed`,
  `kb.database.view.created/updated`.
- **Rows:** `kb.row.created`, `kb.row.updated` (with changed-property delta),
  `kb.row.deleted`, `kb.row.moved` (board column change → mirrors issue "status change").
- **References:** `kb.reference.added` / `kb.reference.removed` (feeds the reference graph &
  backlinks; e.g. doc now references issue #42).
- **Comments:** `kb.comment.added`, `kb.comment.resolved`, `kb.mention.created` (→ notifications).
- **Permissions:** `kb.access.granted/revoked`, `kb.page.published` (security-relevant; also
  audit).
- **Collaboration (mostly internal):** presence/awareness on a side channel; not durable bus
  events.
- **GDPR/audit:** `kb.subject.export.requested/completed`, `kb.subject.erasure.requested/completed`,
  and audit entries for every access/permission change.

### 7.2 Consumes (subscribes)

- **Identity/access events:** user deprovisioned/anonymised (trigger erasure/ownership
  reassignment); team membership changes (recompute view membership/permissions); org/project
  lifecycle.
- **Cross-artifact lifecycle (for live references):** `issue.updated`, `issue.closed`,
  `ci.run.completed`, `git.commit.pushed`, `chat.message.posted` — to (a) refresh embedded
  views / mention previews, (b) update artifact-reference properties, (c) trigger
  agent-maintained "living documents".
- **Reference-graph events:** notification that a referenced artifact was deleted/erased →
  tombstone the reference and degrade rendering gracefully.
- **Agent fabric / trigger events:** "agent produced a draft/edit/suggestion for page X" →
  apply via the collab layer with agent attribution. (Mock agents during dev.)
- **Search/index events:** acknowledgements that content was (re)indexed (for consistency
  signalling).
- **Templating / automation triggers:** scheduled events (e.g. "create today's daily-note
  from template"), or platform events that should spawn a page (e.g. "incident opened → create
  incident doc from runbook template").

### 7.3 Trigger/automation surface (agent-native)

The subsystem must let rules say: *when* `<kb or platform event>` *and* `<condition>` *then*
`<action>` (notify, run agent, create/update page or row, move a row). This is the agent
fabric's hook into knowledge work. Design so the action executor is a **strategy interface**
(mock executor in dev, real agents later).

---

## 8. GDPR / erasure considerations (subsystem-specific)

The knowledge platform is the **hardest GDPR surface** in Myelin because personal data is
embedded in free text in unpredictable places, and because of history, references, and
exports. Specific concerns:

- **Free-text personal data is unstructured.** Unlike an issue's "assignee" field, a person's
  name/email/health info can be anywhere in prose. Right-to-erasure (Art. 17) may require
  locating personal data inside documents authored by others. **Honest limitation:** full
  automated detection is not solvable perfectly; the realistic design is (a) erase/anonymise
  *structured* personal references (person properties, mentions, author attribution) reliably,
  and (b) provide *tooling* (search, DSAR export, flagged-content review) plus a documented
  process for free-text. State this limitation explicitly rather than over-promise.
- **Authorship & attribution.** "Created by / edited by / commented by" are personal data.
  Erasure typically means **anonymisation** (reassign to "Deleted user" / pseudonymous id),
  not deletion of the content, to preserve the document's integrity and others' work.
- **History & versions retain old content.** Erasure must reach into version snapshots and the
  CRDT op-log — but CRDT logs are append-only and merge-dependent, so you cannot simply delete
  an op. Options: **rewrite/compact history** to a snapshot that excludes the data, or
  **crypto-shredding** (encrypt personal segments with per-subject keys, destroy the key on
  erasure). Crypto-shredding is the leading technique for "delete from immutable logs" and
  should be evaluated. **This is a genuinely hard, partially-open problem.**
- **Backlinks/references to erased subjects** must tombstone gracefully (mention of an erased
  user → neutral placeholder), without leaving the original personal data in any index.
- **Search index** must be purged in lockstep with erasure (no leaking via search). Same for
  any vector/embedding index — embeddings derived from personal data are themselves personal
  data and must be re-derivable/erasable.
- **Published/public pages** are a high-risk export of personal data outside access controls;
  publishing personal data needs lawful-basis tracking and easy unpublish + cache/CDN purge.
- **Data portability (Art. 20):** the lossless JSON/Markdown export (§2.10) is the mechanism;
  must be scopeable to a data subject ("everything about/by user X").
- **Data residency:** all primary storage, search indexes, object storage, *and collaboration
  session state* must be pinnable to EU regions. Collab-session locality (§5.7) is a residency
  concern, not just a latency one.
- **Lawful basis & retention:** per-space/per-database retention policies; ability to record
  lawful basis for databases holding personal data (e.g. a "customers" database). Audit log of
  access to sensitive pages.
- **Tension to resolve:** **versioning/audit (keep everything) vs. erasure (delete some
  things)**. The reconciling design (crypto-shredding + anonymise-attribution +
  history-compaction) must be chosen with legal input; flag as needing the shared
  GDPR/compliance design.

---

## 9. Dependencies

### 9.1 On shared systems

- **Identity & access:** users, teams, orgs, roles; the ACL/permission primitives. The KB
  permission tree must be expressed in this model (likely a Zanzibar-style relation store
  shared platform-wide). Hard dependency.
- **Event bus:** for all emit/consume (§7); the backbone of agent-native behaviour.
- **Agent fabric:** the strategy-pattern surface for agent authors/readers/triggers; mock
  implementations during development.
- **Storage:** durable store for blocks/rows/CRDT updates/snapshots (primary DB) + **object
  storage** for media/files, with EU residency and CDN. Likely the heaviest storage consumer.
- **Search:** full-text + structured + (likely) vector/semantic indexing; permission-aware.
  KB is a primary feeder.
- **Cross-artifact reference graph:** mentions, links, relations, artifact-reference
  properties, and backlinks all live here. KB is the heaviest producer/consumer.
- **Notifications:** mentions, comment replies, shares, changes to watched pages/databases.
- **GDPR/compliance services:** DSAR export, erasure orchestration, lawful-basis & retention,
  audit log, residency controls.

### 9.2 On / with other subsystems

- **Issue tracker (#3):** *the* deepest entanglement. Both need "structured records + views
  (table/board/calendar) + custom fields + relations". Strong case that **databases are
  shared infrastructure** with issues as a specialised, workflow-rich database, OR that they
  share a common structured-records engine. KB databases must embed issue views and relate
  rows to issues. **Resolve this boundary early** (§10).
- **Chat (#5):** comments-on-docs vs. chat threads overlap; chat references docs;
  thread→doc summarisation (agent). Decide whether doc comments reuse the chat/threading
  primitive.
- **Git (#1) & CI (#2):** docs reference commits/PRs/CI runs (artifact references, live
  status); "docs-as-code"/README rendering and wiki-from-repo are possible bridges; incident
  docs reference failing CI runs. Mostly via the reference graph, not deep coupling.
- **Templating, presence/collab, and rich-text editing** may themselves become **shared
  capabilities** reused by issues (issue descriptions/comments) and chat (message
  composition). Worth flagging as candidate shared frontend/editor infrastructure.

---

## 10. Open questions & explicit uncertainty

> Per VISION §3 (honesty about uncertainty). These are deferred to architecture phases; listed
> with my current lean where I have one.

1. **Databases: shared with the Issue Tracker or separate?** (Highest-impact question.) Lean:
   a **shared structured-records engine** with issues as a specialised database. Needs
   issue-tracker deep-dive reconciliation.
2. **Block tree storage:** per-block rows (adjacency list) vs. document-as-single-CRDT-blob vs.
   hybrid. Trade-off: doc size & block-level features vs. collaboration simplicity. *Unresolved.*
3. **CRDT vs. OT, and granularity** (per-doc vs. per-block CRDT). Lean: CRDT (Yrs) for
   Rust-alignment & offline; needs prototyping/benchmarking. *High uncertainty.*
4. **Flexible-DB query model:** JSONB-property-bag + derived projection vs. per-database
   materialised tables vs. external query store. Lean: JSONB source-of-truth + indexable
   projection. *Unresolved, needs scale prototyping.*
5. **Formula/rollup engine:** sync vs. async incremental dataflow; consistency guarantees;
   fan-out limits; cycle handling. *Open.*
6. **Permission granularity:** page/database-level only, or row-level and field-level? Corporate
   buyers likely want the latter; cost is significant. *Scope decision.*
7. **Folders vs. pure-pages hierarchy.** Lean: pure pages (Notion model) for flexibility, but
   corporate familiarity may demand an explicit folder concept. *Open.*
8. **Erasure from immutable history:** crypto-shredding vs. history-rewrite/compaction vs.
   anonymise-only. Needs legal + shared-GDPR-system input. *Genuinely hard; partially open.*
9. **Offline depth:** read-only, light-edit, or full offline-first? Drives the CRDT decision
   and sync complexity. *Scope decision.*
10. **Search granularity:** block-level vs. page-level index documents; and whether to commit to
    vector/semantic search in v1 (needed for agent RAG). *Open.*
11. **Synced blocks / transclusion in v1?** They break the clean tree and complicate
    permissions/erasure/reference-counting. Possibly defer. *Scope decision.*
12. **Comments:** reuse chat/threading primitive, or KB-native? *Open, cross-subsystem.*
13. **How "live" are embedded views & artifact references** (real-time via event bus vs. on-load
    fetch vs. cached)? Cost/consistency trade-off. *Open.*
14. **Multi-region collab + EU residency:** where does the authoritative collab server for a doc
    live, and how is session state sharded under residency constraints? *Hard, open.*

### Things I deferred / did not investigate deeply

- Exact event envelope/schema, transport, and delivery semantics (owned by shared event-bus
  design).
- Concrete database/storage technology choices (Postgres vs. distributed SQL vs. specialised
  stores) — architecture phase.
- The precise permission model formalism (Zanzibar-style vs. other) — a platform-wide decision.
- Detailed competitor feature-by-feature parity matrix (Notion/Coda/Confluence/AnyType/
  Outline/AFFiNE) — I relied on domain knowledge of their models rather than re-verifying each
  feature live; specific edge features should be confirmed during competitive-landscape work.
- Pricing/packaging, and the visual/interaction design language (later phases).

### Notable open-source / prior art worth evaluating later (not endorsements)

- **Editors/CRDT:** ProseMirror + `prosemirror-collab` (OT-flavoured), **Yjs/Yrs** (Rust),
  Automerge, **BlockNote** (block editor on ProseMirror+Yjs), TipTap, Slate, Lexical.
- **Research:** **Peritext** (CRDT rich text), **Fugue** (interleaving-free list CRDT),
  Kleppmann's **move operation** for trees.
- **Notion-like OSS:** **AFFiNE**, **Outline**, **AnyType**, **Docmost**, **Appflowy** — for
  data-model and UX reference and licensing posture (relevant to EU-sovereign self-hosting).

*(The above tools/papers are from my own knowledge; verify currency and licensing before any
architectural commitment.)*
