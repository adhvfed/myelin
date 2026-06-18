# Phase 2 — Architecture Decision Register (ADR)

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth; never contradicted). Phase-1 foundation:
> [`01-research/README.md`](../01-research/README.md),
> [`01-research/technical-structuring.md`](../01-research/technical-structuring.md),
> [`01-research/open-questions-and-risks.md`](../01-research/open-questions-and-risks.md),
> [`01-research/gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md),
> [`01-research/agent-native-design.md`](../01-research/agent-native-design.md),
> [`01-research/use-cases.md`](../01-research/use-cases.md).
> Companion Phase-2 doc: [`system-overview.md`](./system-overview.md) (the holistic narrative
> + diagrams + end-to-end walkthroughs). Read that for *how the parts interact*; this doc is
> *what we decide and why*.

---

## 0. How to read this register

This is the cross-cutting decision register for Phase 2. Phase 2's job (VISION §5.2, Phase-1
README §5) is to **commit a high-level architecture** — take positions on the decisions that
span subsystems, so the Phase-4 subsystem agents and Phase-3 shared-systems agents build on a
coherent spine rather than re-litigating the same questions five times.

**Altitude.** Decisions here are *directional and structural*: which model, which class of
technology, which boundary. Concrete schemas, sharding internals, algorithms, wire formats,
and per-store engine choices are **deferred to Phase 3 (shared systems) and Phase 4
(subsystems)**, and each ADR names what it defers.

**Status vocabulary.**
- **DECIDED** — Phase 2 commits this; later phases implement, not re-debate (they may refine
  within the decision, or escalate a written deviation per VISION §3).
- **DECIDED (directional)** — the *direction* is committed (e.g. "durable-execution
  semantics"); the *instantiation* (build vs adopt vs which library) is a Phase-3 deliverable.
- **[OPEN → P3]/[OPEN → P4]/[OPEN — LEGAL]** — genuinely unresolved; carried forward with a
  named resolver phase. Phase 2 must not foreclose these.

**ADR index.**

| ADR | Title | Status |
|---|---|---|
| [ADR-01](#adr-01--repobuild-structure-monorepo--cargo-workspace--glue-crates) | Repo/build structure: monorepo + Cargo workspace + glue crates | DECIDED |
| [ADR-02](#adr-02--backend-language-strategy-rust-default-justified-divergence) | Backend language strategy (Rust default, justified divergence) | DECIDED |
| [ADR-03](#adr-03--the-permission-model-relationship-based-zanzibar-style) | The permission model (relationship-based / Zanzibar-style) | DECIDED (directional) |
| [ADR-04](#adr-04--event-bus-delivery-model--the-firehose-split) | Event-bus delivery model + the firehose split | DECIDED (directional) |
| [ADR-05](#adr-05--the-shared-rich-contentblock-model) | The shared rich-content/block model | DECIDED |
| [ADR-06](#adr-06--the-shared-databaseviews--field-definition-primitive) | The shared database/views + field-definition primitive | DECIDED |
| [ADR-07](#adr-07--the-single-query-ast) | The single query AST | DECIDED |
| [ADR-08](#adr-08--agent-fabric-strategy-pattern-boundary-plan-then-apply) | Agent Fabric strategy-pattern boundary (plan-then-apply) | DECIDED |
| [ADR-09](#adr-09--durable-workflow-engine-for-automationshitl) | Durable-workflow engine for automations/HITL | DECIDED (directional) |
| [ADR-10](#adr-10--datastore-strategy-per-tier) | Datastore strategy per tier | DECIDED (directional) |
| [ADR-11](#adr-11--multi-tenancy--the-cell-based-region-pinned-topology) | Multi-tenancy + the cell-based region-pinned topology | DECIDED |
| [ADR-12](#adr-12--gdpr-by-construction-the-personaldataholder-spine) | GDPR-by-construction: the PersonalDataHolder spine | DECIDED |
| [ADR-13](#adr-13--the-three-glue-contracts-as-binding-platform-law) | The three glue contracts as binding platform law | DECIDED |
| [ADR-14](#adr-14--primary-datastoretech-per-system-summary-table) | Primary datastore/tech per system (summary table) | DECIDED (directional) |

A running **open-questions carry-forward** table closes the document (§ADR-15).

---

## ADR-01 — Repo/build structure: monorepo + Cargo workspace + glue crates

**Status: DECIDED.** Resolves TE-3 / open-question §13.2 "monorepo vs polyrepo".

### Context
The single biggest structural risk in the whole platform is that the glue rots into
"integrated-by-API" — Myelin becoming the stitched-together suite it exists to beat
(`technical-structuring.md §12 risk #1`; `open-questions-and-risks.md` TE-1, §6.1). The
differentiator is the *shared layer*, and the shared layer is only real if the contracts
(event envelope, `ArtifactRef`, `Principal`, `PersonalDataHolder`) cannot silently drift
between subsystems.

### Options
1. **Polyrepo** — one repo per subsystem/shared-system; contracts shipped as versioned
   packages. Strong team isolation; but versioned-package coordination tax and contract drift
   are exactly the failure mode Myelin must avoid (`technical-structuring.md §9.1`).
2. **Monorepo + single Cargo workspace** — one source of truth for the glue; atomic
   cross-cutting refactors; one CI graph; Myelin can dogfood/host itself. Needs good build
   tooling as it grows.
3. **Polyglot monorepo** — monorepo spanning Rust + (where justified) other languages and the
   frontend, with per-language build tools under one repo and one CI graph.

### Decision
Adopt a **polyglot monorepo** whose Rust portion is a **single Cargo workspace** (option 3,
subsuming 2). The **glue lives in shared crates** that every subsystem depends on and whose
traits it implements. A subsystem *becomes part of Myelin by depending on these crates*; that
dependency is the mechanical embodiment of "speaking the three glue contracts" (ADR-13).

Canonical shared crates (from `technical-structuring.md §9.3`, ratified and extended here):

| Crate | Owns (contract surface) |
|---|---|
| `myelin-events` | Event envelope, `ArtifactRef`, event taxonomy types, transactional-outbox helper |
| `myelin-identity` | `Principal`, capability/permission types, the authz client (ADR-03) |
| `myelin-refs` | Reference-graph edge types + client (ADR-13) |
| `myelin-agent` | `AgentRuntime`/`Agent`/`ToolSurface`/`EventInbox`/`EffectApi` traits + `MockAgentRuntime` (ADR-08) |
| `myelin-gdpr` | `PersonalDataHolder` trait + DSR client + crypto-shred/KMS abstraction (ADR-12) |
| `myelin-content` | The shared rich-content/block model (ADR-05) |
| `myelin-query` | The shared query AST + view primitives (ADR-06, ADR-07) |
| `myelin-tenancy` | `TenantId`/`Region`, residency tags, cell-routing client types (ADR-11) |

Frontend code (stack chosen in the design-language deliverable / Phase 4 per VISION §4) lives
in the same monorepo so the **shared design language** and generated API/types stay in lockstep.

### Rationale
Directly mitigates the #1 risk: contracts that live in one workspace and are imported by every
subsystem cannot drift; a breaking change to a contract is a single PR that breaks every
consumer's build *now*, not silently in production months later (`technical-structuring.md
§9.1`, §9.3). Dogfooding ("Myelin hosts itself, one CI graph") is a credibility and quality
mechanism (`open-questions-and-risks.md` §6.1).

### Consequences
- **Build tooling is a first-class concern.** A large Cargo workspace needs caching, sparse/
  partial builds, and good incrementality (Phase-6 roadmap / Phase-4 CI). Cargo workspaces +
  the platform's own CI handle this; revisit if build times become the bottleneck.
- **Enforcement of "no subsystem-to-subsystem DB access"** is a *lint/architecture-test*
  obligation, not merely a convention — a subsystem crate must not depend on another
  subsystem's storage crate. [Deferred mechanism → P3/P6; the rule is DECIDED.]
- **Self-host parity (ADR-11)**: the same artifacts build a managed cell and an on-prem
  install. The monorepo makes "no hidden cloud deps" auditable.

### Deferred
Workspace layout details, build-cache strategy, the architecture-test/lint that forbids
cross-subsystem DB deps → P3/P6.

---

## ADR-02 — Backend language strategy (Rust default, justified divergence)

**Status: DECIDED.** Implements VISION §4. Informs TE-8, TE-21.

### Context
VISION §4: "Rust is a good default… Rust is not a requirement. Each subsystem's architecture
agent decides the languages and tools that best suit that subsystem's needs… justified in
writing." The `.gitignore` is pre-seeded for Cargo + `cargo-mutants`, signalling the quality
bar. Phase 1 surveyed candidates (`technical-structuring.md §8.1`).

### Options
Per-service: **Rust** (memory-per-connection, no GC pauses, strong types, `gix`/`Yrs`/
`Tantivy`; Mononoke proves a scalable Rust git server is feasible); **Go** (mature network
services, weaker types, GC); **Elixir/BEAM** (best-in-class soft-real-time fan-out — directly
relevant to chat's connection tier); **TypeScript/Node** (frontend alignment, BFF/tooling).

### Decision
**Rust is the default for all backend services and the shared-glue crates.** A subsystem may
diverge **only with a written justification** in its Phase-4 architecture doc, evaluated
against: (a) does the divergence buy a *material* capability Rust can't reasonably match for
that workload; (b) does it still implement the glue crates' traits cleanly across the language
boundary; (c) does it preserve EU-deployability/self-hostability (ADR-11). The **glue crates
themselves are Rust, non-negotiably** — they are the contract surface.

Phase-2 directional guidance (subsystem agents may refine):
- **Hot-path cores stay Rust**: event bus, CI scheduler, search core, the chat fan-out tier,
  the authz decision path, git serving core.
- **The one language most likely to earn a divergence is the chat real-time connection tier**
  (BEAM/Elixir's Phoenix Channels for millions of connections) — flagged, *not* pre-decided
  here; it is TE-21, owned by the Phase-4 Chat agent. If chat diverges, it still emits/consumes
  the Rust-defined envelope over the wire and implements `PersonalDataHolder`.
- **Frontend stack is open** and set in the design-language deliverable (VISION §4); TS/React-
  class is the expected baseline but not mandated here.

### Rationale
Honours VISION §4's "agents choose, Rust default, justify divergence" exactly. Concentrates the
non-negotiable coherence (the contracts) in Rust while leaving subsystem agents the latitude
the VISION explicitly grants. Cross-language services are *allowed* precisely because the glue
is a wire contract (envelope + `ArtifactRef` + authz API), not a shared binary.

### Consequences
- Cross-language services consume the glue via **generated bindings / a stable wire API**, not
  by linking the Rust crates. This makes the envelope/`ArtifactRef`/authz protocol-level
  definitions (Phase 3) load-bearing for polyglot support.
- Hiring/ops cost of Rust is accepted as the price of the quality bar (VISION §4,
  `technical-structuring.md §8.1`).

### Deferred
Each subsystem's final language choice → P4 (with written justification). The chat
connection-tier language → P4 (Chat), TE-21.

---

## ADR-03 — The permission model (relationship-based / Zanzibar-style)

**Status: DECIDED (directional).** Resolves the *model class* for TE-2 / SC-1 / §13.2
"permission model formalism" — the single most pervasive cross-cutting hazard. Detailed
algebra and store internals → P3.

### Context
Permission-aware reads at scale (search results, reference backlinks, chat unfurls, issue
lists) are *the* recurring correctness hazard, and a leak is simultaneously a security breach
and a GDPR breach (`open-questions-and-risks.md` TE-2, SC-1, §6 #2; `use-cases.md §8.4`). Every
subsystem deep-dive independently concluded simple RBAC is insufficient
(`technical-structuring.md §2.1`): issues need field/transition/confidential-issue visibility;
knowledge needs page-tree inheritance with overrides; chat needs per-viewer permission-aware
unfurls; search and refs need permission-filtered reads **at query time** without N+1 checks or
leaks.

### Options
1. **RBAC** (roles → permissions). Familiar, simple, but cannot express the relationship-/
   hierarchy-derived visibility above without an explosion of roles; doesn't answer "can P see
   object O" cheaply for arbitrary O across subsystems.
2. **ABAC** (attribute/policy rules, e.g. OPA/Rego). Very expressive, but per-object decisions
   over large result sets are expensive, and "list all objects P can see" (the search/refs
   need) is not naturally answerable.
3. **ReBAC / Zanzibar-style** (relationship tuples `object#relation@subject`, computed
   "check" + "list-objects"/"list-subjects" queries; SpiceDB/OpenFGA-class). Designed for
   exactly the "can this user see this object" and "which objects can this user see" questions
   at Google scale, with caching and consistency tokens.

### Decision
Adopt a **relationship-based (Zanzibar-style) authorization model** as the platform's single
permission model, owned by Identity & Access and **co-designed with Search and Refs from the
start** (not retrofitted). One policy engine evaluates **humans, agents, and services
identically** (ADR-08, ADR-13). The model must natively answer:
- `check(subject, permission, object)` — the per-action gate (every entrypoint: UI, CLI, git
  wire, API, event-trigger).
- `list-objects(subject, permission, type)` — the **permission-filtered read** primitive that
  Search and Refs use to *pre-filter* rather than post-filter (no leak, no N+1).
- relationship inheritance (org → team → project → repo/space/channel → artifact → sub-artifact)
  expressed as tuples, so issue-field/knowledge-page/chat-unfurl visibility all reduce to the
  same machinery.

RBAC remains the **authoring/UX surface** (admins assign roles); roles are *compiled into
relationship tuples*. ABAC-style attribute predicates are supported **at the edges** where
relationship tuples are a poor fit (e.g. "field visible only if issue.severity < X"), kept off
the hot list-objects path. This is "ReBAC core, RBAC face, ABAC at the edge."

### Rationale
ReBAC is the only one of the three that answers the **list-objects** question — the thing that
makes permission-aware search/refs/unfurls correct *and* fast (`technical-structuring.md §2.1`,
§2.3, §2.4). Zanzibar is battle-tested at world scale and has open, EU-self-hostable
implementations (SpiceDB-class, `technical-structuring.md §8.2`), satisfying ADR-11's
portability/sovereignty constraint. Co-design with Search/Refs is mandated because post-
filtering large result sets both leaks and is slow (SC-1).

### Consequences
- **Search and Refs must consume an authz "filter" primitive**, not call `check` per result.
  This couples their Phase-3/4 designs to the authz store's `list-objects` semantics and its
  consistency model (read-your-writes via Zanzibar "zookies"/consistency tokens). Load-bearing
  cross-system contract.
- The **authz tuple store is a first-class datastore** (ADR-10, ADR-14), residency-pinned and a
  `PersonalDataHolder` (tuples reference subjects).
- **Agent delegation** (`agent.policy ∩ delegation ∩ tenant.policy`, ADR-08) is expressed in
  the same tuple algebra; the exact intersection semantics are the P3 authz pass.

### Deferred / Open
- **[OPEN → P3]** The full tuple schema, the namespace/relation definitions per subsystem, the
  consistency-token strategy, caching, and the **delegation/on-behalf-of algebra** (AG-2).
- **[OPEN → P3]** `Service` vs `Agent` as one principal kind or two (AG-1) — affects how
  governance attaches; does not change the ReBAC choice.

---

## ADR-04 — Event-bus delivery model + the firehose split

**Status: DECIDED (directional).** Resolves the *semantics* for TE-9/TE-10/TE-11/TE-12 and
SC-4; concrete transport tech → P3.

### Context
The event bus is the agent-native backbone and simultaneously the feed for Refs, Search, Notif,
analytics, and external webhooks (`technical-structuring.md §2.2`; `agent-native-design.md §2`).
Its delivery semantics drive idempotency requirements on **every consumer in every subsystem**,
so they are load-bearing and must be committed early (`open-questions-and-risks.md` §6 #6).
CI logs, chat presence/typing/read-state, and collab op-streams must **not** traverse the
durable bus the same way (TE-11, SC-9, SC-10).

### Decision
Commit the following **structural semantics** (instantiation deferred to P3):

1. **At-least-once delivery + idempotent consumers.** Exactly-once *effect* is achieved via the
   `event_id` dedup key carried in the envelope. Every consumer must be idempotent on
   `event_id`. (`agent-native-design.md §2.1`; `technical-structuring.md §2.2`.)
2. **Per-aggregate ordering.** All events for one PR / one issue / one run / one doc are
   ordered (partition key = aggregate). **Global ordering is explicitly NOT required**
   (every deep-dive agrees, TE-12).
3. **Transactional-outbox emission by default.** An event is emitted **iff** the underlying
   state change committed — no ghost/lost events. **True event-sourcing is reserved** for a
   few high-audit aggregates (issue transitions, permission changes) at each subsystem's
   discretion; the *platform contract* ("a reliable ordered stream of canonical events") is
   identical either way, which is what lets each subsystem defer the internal choice (TE-10,
   `agent-native-design.md §2.1`).
4. **References-not-payloads + bounded retention + crypto-shred + tombstones.** Events carry
   IDs/`ArtifactRef`s; personal data lives in an erasable store the event points to. The bus
   has bounded retention and supports crypto-shred so the append-only log cannot defeat erasure
   (ADR-12; `gdpr-eu-sovereignty.md §6.2`). The envelope's `contains_personal_data` flag routes
   GDPR handling.
5. **Two transports — the firehose/control split.** **Durable control/domain events** (the
   canonical envelope) ride the durable, ordered, replayable bus. **High-volume ephemeral
   firehose streams** (CI log lines, chat presence/typing/read-state, collab op-streams) ride
   **dedicated transports**; the durable bus carries only "data available/updated" pointer
   events into them. This split is mandatory, not optional (TE-11; `technical-structuring.md
   §2.2`).

### Candidate technology (directional, P3 decides)
Durable bus: **Kafka/Redpanda** (durable, partitioned, replayable, per-partition ordering),
**NATS JetStream** (lighter, EU-deployable, good fan-out), or **Postgres logical-decoding/
outbox** (fewest moving parts at small scale). Firehose: a dedicated low-latency fan-out path
(NATS/Redis-class for ephemeral; an append-mostly log/object-store tier for CI logs). Selection
must satisfy **bounded retention + crypto-shred + tombstones on EU-deployable infra** (GD §10
Q9; ADR-11).

### Rationale
At-least-once + idempotent is the proven, simplest-correct default; exactly-once delivery is
expensive and usually illusory across heterogeneous consumers (`agent-native-design.md §2.1`,
§6). Per-aggregate (not global) ordering is what makes the bus horizontally scalable while
still giving agents/consumers causal order within a run/PR (TE-12). The outbox pattern is the
standard fix for the dual-write problem on a busy git server / CI scheduler
(`technical-structuring.md §2.2`). The firehose split prevents CI-log/chat-presence volume from
melting the durable bus or starving control-event ordering (SC-9, SC-10).

### Consequences
- **Idempotency is a platform-wide obligation** baked into the consumer template in
  `myelin-events`. Every trigger/automation/agent/indexer/notifier must dedupe on `event_id`.
- **The trigger/automation/agent engine sits on the durable bus** (ADR-08, ADR-09); firehose
  streams are observed via pointer events, never by waking an agent per log line.
- Agent-generated load governance (budgets, loop caps) is enforced on the durable bus path
  (ADR-08 §safety; AG-4/AG-5).

### Deferred / Open
- **[OPEN → P3]** Durable-bus and firehose transport selection; partitioning/sharding; the
  `EventMatcher` predicate language (CEL/JSONLogic/custom — AG-7) for triggers; replay/
  compaction strategy; exact retention windows.

---

## ADR-05 — The shared rich-content/block model

**Status: DECIDED.** Resolves TE-4 / §13.2 "shared rich-content/block model". Adopt, with a
clear boundary.

### Context
Chat messages, issue descriptions/comments, and knowledge blocks are all "structured rich
content with mentions and artifact references as first-class nodes" (`chat.md §2.3`;
`knowledge-platform.md §9.2`; `issue-tracker.md §3.1`; `technical-structuring.md §3.4`). Sharing
the representation buys consistent rendering, one editor, and **mentions/`ArtifactRef`s as
first-class content nodes everywhere** — but over-sharing risks coupling subsystems that have
genuinely different performance/collaboration profiles (risk #10, `open-questions-and-risks.md`
TE-4).

### Decision
**Adopt one shared rich-content/block model** (`myelin-content`) as the **document/content
representation and the canonical serialization**, with this boundary:

**Shared (in `myelin-content`):**
- The block/inline node taxonomy (paragraph, heading, list, table, code, callout, embed, …)
  and the **inline node types that are platform-load-bearing**: `mention(Principal)`,
  `artifact_ref(ArtifactRef)`, and `embed(ArtifactRef)` — so a reference in a chat message, an
  issue comment, and a doc block is the *same* node type, resolved the *same* way by Refs and
  rendered the *same* way by every unfurl (ADR-13).
- Serialization, sanitization, and a render-projection API (what `ArtifactRef` resolution
  returns for unfurls).

**NOT shared (subsystem-owned):**
- **The collaboration/concurrency mechanism.** Knowledge needs full collaborative editing
  (CRDT/OT, TE-15) over a large block tree; chat messages are small and mostly immutable-after-
  send; issue descriptions are single-author-at-a-time. Each subsystem chooses its own
  edit/concurrency strategy *over the same content model*. The content model is a **data/AST
  contract, not a shared editing engine**.
- **Storage layout** (per-block rows vs document blob vs hybrid — TE-16) stays subsystem-owned.

### Rationale
The high-value, low-risk core of the shared model is the *node taxonomy + the reference/mention
nodes*, because that is what makes "reference any artifact from anywhere" and "render any unfurl
consistently" true platform-wide (`technical-structuring.md §3.4`; UC-CHAT-3, UC-ISS-13,
UC-KN-8). The high-risk part is *collaboration/concurrency*, which differs radically per
subsystem; keeping that subsystem-owned avoids risk #10 (shared-primitive over-reach) while
still capturing the reuse win. This is the "share the AST, not the editor" line.

### Consequences
- One editor component and one renderer can be built against `myelin-content` for the shared
  design language (Phase-2 design deliverable), even though concurrency differs underneath.
- `mention`/`artifact_ref` nodes are the **producers of `ref.created` events** (ADR-13) — refs
  are emitted *from content*, uniformly.
- Free-text PII in content is the hardest GDPR surface (`knowledge-platform.md`,
  `gdpr-eu-sovereignty.md §6`); the content model carries the `contains_personal_data`/erasure
  hooks so each holder can crypto-shred/redact (ADR-12).

### Deferred / Open
- **[OPEN → P4]** CRDT-vs-OT for Knowledge collaborative editing (TE-15) and block-tree storage
  (TE-16) — owned by Phase-4 Knowledge, *over* this content model.
- **[OPEN → P4]** Exact block taxonomy completeness and extension mechanism → P4 (Knowledge
  leads, Chat/Issues consume).

---

## ADR-06 — The shared database/views + field-definition primitive

**Status: DECIDED.** Resolves TE-5 / §13.2 — the **highest-impact reuse boundary in the
platform** (`open-questions-and-risks.md` §6, TE-5; `technical-structuring.md §4.2`). Adopt the
*primitive*, keep the *engines* subsystem-specific.

### Context
The issue tracker and Knowledge databases are both "typed records + multiple views
(table/board/calendar/timeline) + filters" (`competitive-landscape.md §3/§4`;
`issue-tracker.md §3.7`; `knowledge-platform.md §10 Q1`). Both deep-dives recommend **sharing
the field-definition and view-query primitives** while keeping the issue lifecycle/workflow/SLA
engine tracker-specific. Getting the boundary wrong is risk #10: too shared → can't get
subsystem-specific performance; too separate → drift.

### Decision
**Adopt a shared "structured collection" primitive** (in `myelin-query` alongside the query
AST, ADR-07) consisting of:

**Shared:**
- **Field definitions**: a typed field-definition system (text, number, select/multi-select,
  date, user/principal, relation, formula/rollup-typing, …) with per-field metadata
  (personal-data classification per ADR-12, residency-neutral).
- **Views**: the view abstraction (table / board / calendar / timeline) as a *query + grouping
  + sort + visible-fields* projection over a typed collection, expressed in the shared query
  AST (ADR-07).
- **The relation field type** rides the Reference Graph where cross-artifact, or a subsystem-
  local relation index where intra-collection (the choice is ADR-13's open item TE-7).

**NOT shared (subsystem-owned):**
- **The issue lifecycle/workflow state machine, transitions, SLA engine, hierarchy/rollup
  semantics, and tracker analytics** stay in the Issue tracker (`technical-structuring.md
  §4.1`).
- **The Knowledge formula/rollup dataflow engine, block-embedding, and real-time collab** stay
  in Knowledge (TE-18).
- **Physical storage / query execution** (JSONB property-bag + derived indexable projection vs
  per-database materialised tables — TE-17) stays subsystem-owned; the shared layer is the
  *definition + view + AST*, not the storage engine.

### Rationale
The reusable, low-risk core is *field definitions + the view abstraction + the query AST*; the
high-risk, performance-sensitive core is *query execution over flexible user-defined fields*
(the "JQL performance trap", TE-17) and the *workflow/formula engines*, which are genuinely
subsystem-specific. Sharing the definition/view/AST gives a consistent authoring UX and lets the
design language ship one "database/views" component, while leaving each subsystem free to make
its own storage/execution and engine choices (`issue-tracker.md §3.7`; `knowledge-platform.md
§10 Q1`). This is the "share the schema language and the view model, not the query planner" line.

### Consequences
- The Issues↔Knowledge boundary is now a **named joint design** for Phase 4 (the two tightest
  Issues/Knowledge seam, `technical-structuring.md §4.2`): both implement the shared field/view
  primitive; each owns its execution.
- The design language ships **one views component** (table/board/calendar/timeline) usable by
  both subsystems (Phase-2 design deliverable).
- Flexible-field query performance (TE-17) is the dominant per-subsystem risk and is explicitly
  *not* solved by sharing — it's a P4 problem for each.

### Deferred / Open
- **[OPEN → P4 (Issues+Knowledge, joint)]** The physical storage/query-execution model for
  flexible fields (TE-17); the formula/rollup engine (TE-18); drag-to-reorder ranking at scale
  (TE-19).
- **[OPEN → P3/P4]** Whether relations ride Refs or a local relation index (TE-7, ADR-13).

---

## ADR-07 — The single query AST

**Status: DECIDED.** Resolves TE-6 / §13.2 "single query AST".

### Context
Issues recommends a single query AST (`issue-tracker.md §5.4`); it is useful platform-wide for
saved views, search, automations, and agent triggers — anything that needs a
machine-constructable, safe, multi-surface query (`technical-structuring.md §3.4`; TE-6).
Without it, every surface grows its own query language and JQL-style parser footguns.

### Decision
**Adopt one query AST** (`myelin-query`) as the canonical representation of "a filter/selection
over a typed collection or over the search index," serving **UI (saved views), CLI, API,
automations (`EventMatcher` filters), and agent triggers**. The AST is:
- **Declarative and safe to evaluate** — no Turing-complete predicates on hot paths (shared
  constraint with the trigger `EventMatcher`, AG-7).
- **Machine-constructable** — agents and the UI emit the same AST; there is one parser/
  validator and one renderer back to human-readable form.
- **Compilable to multiple backends** — to the OLTP store (issue/knowledge field queries), to
  the search engine (full-text/structured/vector), and to the `EventMatcher` predicate path,
  each subsystem/shared-system providing its own compiler.

The AST is **permission-aware by construction**: it always composes with the authz
`list-objects` filter (ADR-03) so no query surface can return artifacts the subject can't see.

### Rationale
One AST avoids N parallel query languages and gives agents a single, validated, safe surface to
construct queries — directly serving the agent-native mandate (`issue-tracker.md §5.4`; AG-7).
Making it permission-aware *by construction* is how ADR-03's leak-free reads are guaranteed at
every query surface, not just search.

### Consequences
- The trigger `EventMatcher` (ADR-04/ADR-08) and saved-view filters share the AST's predicate
  core — one safe-evaluation engine, one place to harden against DoS (AG-7).
- Each compile target (OLTP, search, matcher) is a subsystem/shared-system deliverable; the AST
  shape is the contract.

### Deferred / Open
- **[OPEN → P3]** The concrete AST grammar, the predicate language for `EventMatcher`
  (CEL/JSONLogic/custom), and the per-backend compilers.

---

## ADR-08 — Agent Fabric strategy-pattern boundary (plan-then-apply)

**Status: DECIDED.** Ratifies the Phase-1 agent-native design (`agent-native-design.md §4–5`)
as binding architecture. This is a VISION non-negotiable (§3).

### Context
VISION §3: build agent-native *now* with **mock** implementations behind the **strategy
pattern**, so mock→real is a config/implementation swap, not a rewrite. Phase 1 fully designed
this (`agent-native-design.md`); Phase 2's job is to ratify it as the committed boundary so
every subsystem builds to it.

### Decision
Ratify the Phase-1 design as platform law:

1. **Agents are first-class `Principal`s** (kind `Agent`) in Identity & Access — not bot
   tokens. Authorized by the *same* policy engine as humans (ADR-03), attributed and audited
   like humans, with extra governance (budgets, loop protection, AI-Act duties, runtime
   binding) (`agent-native-design.md §1`).
2. **The strategy-pattern boundary is a small trait set** in `myelin-agent`: `AgentRuntime`,
   `Agent`, `ToolSurface`, `EventInbox`, `EffectApi`. **No LLM SDK, prompt, or model name
   appears anywhere in platform code** — all of it lives behind `LlmAgentRuntime`, introduced
   later. `MockAgentRuntime` (deterministic, rule-driven) ships during development. Swapping =
   pointing an agent identity's `runtime_ref` at a different runtime (`agent-native-design.md
   §4.1, §4.5`).
3. **Plan-then-apply is the core safety+testability choice.** Agents are a pure-ish function
   `(event, context) → AgentDecision { effects, … }`. They **never perform side effects
   directly**; they emit *proposed effects*. The platform's `EffectApi` validates each effect
   against **permissions ∩ delegation ∩ tenant policy** (ADR-03), budget, and HITL gates, then
   applies it — emitting domain events (which may wake more agents, governed by loop caps)
   (`agent-native-design.md §4.4`). Identical platform code for mock and real.
4. **Tools = one permissioned catalogue, two front-ends.** Every subsystem registers typed
   `ToolDef`s (name + JSON-schema input + required caps + effect kind + side-effecting flag)
   into the shared `ToolSurface`. The same registry is consumed internally by our runtimes
   **and exposable over MCP** to external agents later — defined once, governed once
   (`agent-native-design.md §4.2`).
5. **Automations and agents are ONE trigger engine** with different action handlers
   (subscriptions / durable automations / agent triggers), ratifying the Phase-1 recommendation
   that every relevant deep-dive reached independently (`agent-native-design.md §3`;
   `technical-structuring.md §2.2`). A `Trigger` binds an `EventMatcher` (ADR-07) to a target
   under a `run_as` principal with a `RunBudget`, a `DelegationPolicy`, and HITL `gates`.
6. **Safety is structural, not an appendix** (`agent-native-design.md §5`): least-privilege
   per-run permissions; per-run/agent/tenant budgets; **loop/runaway protection** via
   `causation_id` depth caps + cycle detection + idempotent tools + per-tenant circuit
   breakers; **HITL gates** as durable workflow waits surfaced as chat approval cards (ADR-09);
   attribution + tamper-evident audit of every agent action; agents always labelled as agents
   (AI Act). **Suggest-by-default; human-confirm consequential actions** (GDPR Art. 22 + AI Act).

### Rationale
This is the platform's defining structural seam and a VISION non-negotiable; Phase 1's design
is sound and the deep-dives corroborate it. Plan-then-apply is what makes mock agents
deterministically testable (golden tests + `cargo-mutants` over the event→trigger→effect→event
loop) *and* real agents safely sandboxed with identical platform code — the whole point of the
strategy pattern (`agent-native-design.md §4.4–4.5`). One trigger engine avoids a parallel
"events-for-agents vs events-for-everything" split (§3.3).

### Consequences
- The Agent Fabric depends on: Identity (delegation), the durable bus (ADR-04, inbox delivery),
  the durable-workflow engine (ADR-09, for budgets/gates/long HITL waits), and the authz engine
  (ADR-03, effect validation). It is the integration point of half the shared systems.
- **CI ↔ agent execution-substrate unification** is promising (a CI job and an agent run have
  the same shape: event → sandboxed work → results+events) but is **[OPEN → P4 (CI)+P3
  (Agents)]** (TE-31); Phase 2 flags it, does not decide it.

### Deferred / Open
- **[OPEN → P3]** Delegation/on-behalf-of algebra (AG-2); `Agent` vs `Service` kinds (AG-1);
  the exact `Agent::handle` signature (single-call vs driven multi-turn loop), streaming,
  context management — the trait surface is provisional and may revise when `LlmAgentRuntime`
  is built (AG-3), though the plan-then-apply core must survive.
- **[OPEN → P3 + testing P5]** Adversarial validation of loop/runaway protection (AG-4) and
  agent-generated load governance (AG-5).
- **[OPEN — LEGAL]** EU AI Act final classification (GD-9); GDPR-vs-LLM erasure of data that
  fed a decision (AG-8). Design-safe minimums (labelling, HITL, logging) are built now.

---

## ADR-09 — Durable-workflow engine for automations/HITL

**Status: DECIDED (directional).** Resolves the *semantics* for TE-20 / §13.2; build-vs-adopt →
P3.

### Context
Multi-subsystem workflows (CI fail → triage → open issue → link → post chat → propose PR) are
**durable, multi-step, long-running, partially human-gated** — an agent run may pause for *days*
on a HITL gate (`agent-native-design.md §3.2`). HITL gates surfaced as chat approval cards are a
GDPR Art. 22 / AI-Act safety mechanism (ADR-08; AG-10). SLA timers (millions of durable timers,
SC-11) need similar durable-scheduling substrate.

### Decision
**Adopt durable-execution semantics** (Temporal-style: deterministic workflow orchestration +
non-deterministic, retryable activities + durable timers + signals for HITL waits) as the
substrate for automations and human-gated agent/automation workflows. The mapping: the
**workflow** is durable and owns budget/gates/state; the **agent reasoning step and tool calls
are activities** (non-deterministic, retryable, sandboxed) (`agent-native-design.md §3.2, §6`).

**Build-vs-adopt is directionally "prefer a self-hostable, EU-deployable durable-execution
substrate"** — whether that is self-hosted Temporal, a Rust-native durable-execution library, or
bespoke is a **P3 decision** with explicit sovereignty weighting (Temporal-the-cloud-service is
disallowed; self-hosted Temporal or a Rust library satisfies ADR-11).

### Rationale
Single-shot webhook/Zapier-lite is insufficient for the vision's multi-subsystem, human-gated
workflows; durable execution is the right pattern for "wait days for a human signal without
holding resources" (`agent-native-design.md §3.2`, §6). Deferring build-vs-adopt keeps the
large-effort/sovereignty-sensitive choice (TE-20, risk #9) at the phase that can weigh it
properly, while committing the *semantics* now so ADR-08's gates/budgets/timers have a defined
substrate to target.

### Consequences
- The Agent Fabric (ADR-08), SLA timers (SC-11), and the automation engine all sit on this
  substrate. It is a shared system in its own right (or a capability of the bus/agent layer —
  P3 places it).
- Whichever instantiation, it must be **EU-deployable and self-hostable** (ADR-11), ruling out
  a US-hosted managed workflow service.

### Deferred / Open
- **[OPEN → P3]** Build vs adopt vs self-hosted Temporal vs Rust-native library (TE-20); the
  HITL-gate UX/data model (with Chat as the surface).

---

## ADR-10 — Datastore strategy per tier

**Status: DECIDED (directional).** Resolves the *tiering + portability constraint* for
TE-13/§13.2; concrete engines per store → P3/P4 (summarized in ADR-14).

### Context
Data profiles differ radically across the platform; the deep-dives converge on three storage
tiers plus specialized stores (`technical-structuring.md §2.7, §8.2`). The hard constraint is
**EU-deployable, self-hostable, portable primitives — no hyperscaler-locked managed services**
(`gdpr-eu-sovereignty.md §2.2, §4`; ADR-11). CI is the heaviest storage consumer (TE-13).

### Decision
Commit the **tiering and the portability constraint**; the per-store engine is directional:

| Tier / store | Role | Default direction (P3/P4 may refine) |
|---|---|---|
| **Transactional (OLTP)** | Domain state: repos/PR metadata, issues, doc blocks/rows, message metadata, run state | **Postgres-class** (portable, EU-deployable, self-hostable). JSONB + GIN + generated columns for flexible fields. Distributed-SQL (CockroachDB/Yugabyte-class) only where a single shard outgrows Postgres (TE-17). |
| **Object/blob** | LFS blobs, CI artifacts/caches, doc media, attachments, avatars, clone bundles, backups | **S3-compatible** (MinIO/Ceph self-hostable; EU providers). Content-addressed, dedup, residency-pinned. |
| **Log/firehose** | CI logs, chat message log, collab op-streams | Append-mostly tail+archive+range-read tier; **wide-column (Cassandra/Scylla-class)** is a candidate for the chat message log (TE-13, SC-10). |
| **OLAP/read store** | Issue analytics, delivery health (CQRS read model fed by the bus) | **Columnar (ClickHouse-class)** fed by the event stream (SC-5). |
| **Search** | Full-text + structured + semantic/vector, ACL-aware, multilingual | **Tantivy (Rust)** / OpenSearch / Meilisearch-class; must support `list-objects` pre-filtering (ADR-03) and vector (ADR-14, P4 Search). |
| **Authz tuple store** | Relationship tuples for ReBAC (ADR-03) | **SpiceDB-class** Zanzibar store (self-hostable, EU-deployable). |
| **Durable-workflow store** | Workflow/timer/signal state (ADR-09) | Backed by the durable-execution substrate's store (P3). |

**Every tier carries per-tenant envelope encryption + crypto-shred and is residency-pinned**
(ADR-11, ADR-12). **No subsystem reads another subsystem's store** (ADR-01, ADR-13).

### Rationale
Postgres/S3-compatible/OCI are the portable, self-hostable commodity primitives that reconcile
world-scale with sovereignty (`gdpr-eu-sovereignty.md §2.2`); proprietary global managed
services are forbidden by ADR-11. The CQRS OLAP split off the event stream is the standard
answer to "analytics scans would kill the OLTP store" (SC-5). The specialized stores (search,
authz, wide-column chat log) are where each subsystem earns its scale.

### Consequences
- The KMS/crypto-shred abstraction (ADR-12) is a cross-cutting dependency of *every* tier.
- The OLAP read store is **fed by the bus** (ADR-04), reinforcing that the durable event stream
  is the analytics source of truth (SC-5).

### Deferred / Open
- **[OPEN → P3/P4]** Concrete engine selection per store; sharding/partitioning internals;
  per-subject vs per-tenant crypto-shred granularity (GD-4); HYOK's limits on search/agents
  (GD §10 Q10).

---

## ADR-11 — Multi-tenancy + the cell-based region-pinned topology

**Status: DECIDED.** Ratifies the Phase-1 reconciliation of world-scale ∧ EU-sovereignty
(`gdpr-eu-sovereignty.md §4`; `technical-structuring.md §5`; SC-2/SC-3). The deepest unknowns
(multi-cell tenants, latency-vs-residency) are carried forward.

### Context
Two VISION non-negotiables pull opposite ways: **world-scale from day 1** wants global managed
services; **EU-sovereign by construction** forbids leaning on them (`gdpr-eu-sovereignty.md
§2.2`). Phase 1's reconciliation is a cell-based, region-pinned topology; Phase 2 must ratify it
as a structural commitment (Phase-1 README §5.1; SC-2).

### Decision
Ratify and commit:

1. **The cell is the unit of sovereignty and scale.** A **cell** = a complete, region-pinned
   stack of *all* subsystems + *all* shared systems on commodity, EU-deployable, self-hostable
   primitives. Tenants are assigned to a cell. **Scale = add cells** (not bigger global
   services). Residency = the cell's region. Breach blast-radius = one cell. **Self-host = one
   cell** on the customer's infra, running the *same artifacts* as a managed cell
   (`technical-structuring.md §5.1`; `gdpr-eu-sovereignty.md §4`).
2. **Everything is multi-tenant; every record carries `tenant` + `region`** as part of the
   partitioning key and routing address — not an afterthought column. Region binding is
   **immutable-by-default and enforced at the data layer** so misrouting a tenant's personal
   data is *impossible*, not merely discouraged (`technical-structuring.md §5.1`;
   `gdpr-eu-sovereignty.md §3.2`).
3. **Tenant isolation is a spectrum by tier** (`gdpr-eu-sovereignty.md §3.1`): **logical**
   (shared infra, `tenant_id` + row-level security) → **schema/DB-per-tenant** →
   **cell/stack-per-tenant** (dedicated; strongest isolation + cleanest residency; for
   public-sector/high-assurance). Isolation must hold across **all** shared systems — bus
   topics, search indices, caches, blob prefixes, agent context, reference-graph partitions,
   authz tuples (ADR-03).
4. **A global control plane** routes tenants→cells and orchestrates but **holds no in-region
   personal data** and is itself EU-sovereign (`gdpr-eu-sovereignty.md §4`).
5. **World-scale mechanisms inside a cell** (`technical-structuring.md §5.3`): stateless-ish
   front doors; heavy cross-system work (reference-graph build, search indexing, rollup
   recompute, analytics) is **event-driven and async** off the bus, not synchronous in the
   write path; OLTP sharded by tenant; a separate OLAP read store fed by the event stream
   (ADR-10); subsystem-specific scale hot-spots owned by their Phase-4 agents.

### Rationale
The cell topology is the only reconciliation of the two hardest non-negotiables that does not
lean on a US hyperscaler's proprietary global plane (`gdpr-eu-sovereignty.md §2.2, §4`; SC-2).
It also delivers GDPR wins "for free": residency by construction, bounded breach scope, clean
per-tenant/per-cell erasure and offboarding, and a natural unit for dedicated/sovereign
deployments. Self-host parity (same artifacts) forces clean packaging and no hidden cloud deps —
a sovereignty and quality mechanism (`technical-structuring.md §5.1`).

### Consequences
- **Cross-region operations in hot paths handling personal data are prohibited by
  construction** (`gdpr-eu-sovereignty.md §3.2`). This is a hard constraint every subsystem
  inherits.
- **Latency-vs-residency is a deliberate, accepted trade-off**: residency forecloses non-EU
  replicas/CDN for personal data, conflicting with global latency (SC-8). Accepted; mitigations
  (in-EU multi-region, clone-bundle caching) are P4.
- The control plane is a separate, global, personal-data-free deployable (ADR-01 §9.2).

### Deferred / Open
- **[OPEN → P3]** Cell sizing; tenant→cell assignment; **multi-cell tenants** (a 10,000-person
  org spanning cells) and cross-cell collaboration/latency — the deepest unknown (SC-2/SC-3,
  GD §10 Q8); the control-plane design holding zero in-region personal data.
- **[OPEN → P4]** Per-subsystem scale hot-spots (git ref-update consistency + monorepo; CI
  scheduler + runner elasticity on EU infra; chat millions-of-connections tier; knowledge
  collab; issue rollups) — each owned by its Phase-4 agent.

---

## ADR-12 — GDPR-by-construction: the PersonalDataHolder spine

**Status: DECIDED.** Ratifies and carries the GDPR constraints to Phase 3 intact (Phase-1
README §5.5; `gdpr-eu-sovereignty.md §5–§9`). Phase 2 must *not foreclose* these; Phase 3 owns
the mechanism detail.

### Context
"Right to erasure reaching every subsystem, the event bus, and the search index" is a property
of the *whole structure*, not a settings page (`gdpr-eu-sovereignty.md §0`). It cannot be
retrofitted. Phase 2's obligation is to commit the contracts and constraints so nothing
designed elsewhere forecloses them.

### Decision
Commit the following as binding platform constraints (mechanism detail → P3):

1. **The `PersonalDataHolder` contract is the spine.** Every store and subsystem registers as a
   holder implementing `locate / export / rectify / restrict / erase` for a subject. The holder
   list is **exhaustive**: all 5 subsystem DBs, object store, **search index**, **event-bus
   history**, caches/CDN, **backups**, **agent memory/embeddings**, **reference graph**,
   notification history, authz tuples, and audit (carve-out + expiry). "We forgot the search
   index" is a *structural failure* (`gdpr-eu-sovereignty.md §5.1`; GD-3).
2. **The DSR Orchestrator** fans a subject- or tenant-scoped request to *all* holders, tracks
   the statutory deadline, produces verifiable receipts, and is operable by Myelin **and by/for
   tenants** (Art. 28 assistance) (`gdpr-eu-sovereignty.md §5.2`).
3. **Crypto-shredding is a first-class deletion primitive.** Per-tenant envelope encryption
   (optionally per-subject where feasible); destroying a key renders ciphertext in DBs/backups/
   immutable logs unrecoverable — the answer for backups, append-only logs, CI logs, chat
   bodies, knowledge history (`gdpr-eu-sovereignty.md §3.3, §6.5`).
4. **Keep personal data out of immutable structures.** Git history and event payloads carry
   **references + pseudonymous identities**, with the erasable mapping *outside* the immutable
   store (`gdpr-eu-sovereignty.md §6.1–6.2`; ADR-04 "references-not-payloads"; ADR-05). This
   minimises the erasure-vs-immutability tension so it "rarely bites."
5. **Data classified by legal role** (`tenant-content` = processor vs `platform-operational` =
   controller) at the **schema level** so it can't drift, driving DSAR routing, lawful basis,
   retention, and deletion authority (`gdpr-eu-sovereignty.md §1.1`; GD-11).
6. **A system-generated data map / classification registry** (schema-level personal-data tags →
   generated inventory → RoPA, DPIA inputs, breach scoping) — *generated*, not hand-curated, so
   it can't drift (`gdpr-eu-sovereignty.md §3.8`; GD-12).
7. **Privacy-by-default** across all subsystems: private visibility, telemetry opt-in, minimal
   retention, agents least-privilege (`gdpr-eu-sovereignty.md §1.10, §8`).
8. **Every external personal-data-touching dependency is a swappable, region-aware,
   EU-preferring adapter** — the same strategy-pattern mandate that swaps mock→real agents,
   generalized; the future real LLM/agent backend is one such adapter and must be EU-hostable
   (`gdpr-eu-sovereignty.md §3.7, §8.9`; AG-9).
9. **One tamper-evident audit log** records every human *and agent* action; it is itself a
   carved-out, retention-bounded holder (`gdpr-eu-sovereignty.md §3.4`; UC-X-16).

### Rationale
These are hard architectural constraints from VISION §3 ("GDPR-safe & EU-sovereign by
construction… not bolted on later") and the Phase-1 GDPR doc. Phase 2's role is explicitly to
"hand the GDPR constraints to Phase 3 intact" and "not design anything that forecloses them"
(Phase-1 README §5.5). Crypto-shred + references-not-payloads + pseudonymous identities is the
combined technique that makes erasure tractable against immutable git/bus/audit (`gdpr-eu-
sovereignty.md §6`).

### Consequences
- `myelin-gdpr` (ADR-01) carries the `PersonalDataHolder` trait + DSR client + KMS abstraction;
  **implementing it is a condition of being a subsystem** (ADR-13).
- The bus (ADR-04), search (ADR-10), refs (ADR-13), and agent memory (ADR-08) each inherit
  erasure obligations; their Phase-3/4 designs must be erasure-aware from the start.

### Deferred / Open
- **[OPEN → P3]** DSR orchestrator design; KMS key hierarchy; crypto-shred granularity
  (per-subject vs per-tenant, GD-4); retention engine; consent + sub-processor registries;
  post-restore re-erasure (GD-14).
- **[OPEN — LEGAL]** Art. 17 erasure scope into immutable git history (GD-1/GD-2); audit-log
  retention carve-out (GD-5); Schrems-III posture (GD-7); EUCS/NIS2/DORA/Gaia-X applicability
  (GD-10); EU AI Act classification (GD-9). Flagged for counsel/DPO before they bind.

---

## ADR-13 — The three glue contracts as binding platform law

**Status: DECIDED.** Ratifies the Phase-1 "wedge" triad as the definition of platform
membership (`technical-structuring.md §3`; Phase-1 README §3.4, §5.1).

### Context
"A subsystem *is* part of Myelin if it speaks the three glue contracts, and is a bolt-on if it
doesn't" (`technical-structuring.md §3`). This is the operational definition that keeps the
platform unified-by-construction rather than integrated-by-API (risk #1, ADR-01).

### Decision
The **three glue contracts are binding platform law** — every subsystem implements all three,
checked structurally (ADR-01's lint), with **no subsystem reaching into another subsystem's
database, ever**:

1. **Addressing — `ArtifactRef`.** Every artifact is addressable as
   `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`, resolvable to: (a) its current rendered
   projection (for unfurls/embeds), (b) a permission check (ADR-03), (c) update events for
   cache invalidation. Every subsystem exposes stable, resolvable IDs down to sub-artifact
   granularity (a PR comment, a doc block, a CI step) (`technical-structuring.md §3.1`).
2. **Event — the common envelope.** Every meaningful state change is emitted in the common
   versioned envelope via transactional outbox (ADR-04). Non-negotiable envelope fields:
   `event_id` (idempotency), `type`, `schema_ver`, `tenant`, `region`, `actor`
   (human/agent/service incl. on-behalf-of), `subject` (`ArtifactRef`), `causation_id` +
   `correlation_id` (provenance + loop capping), `contains_personal_data`, `visibility`
   (`technical-structuring.md §3.2`; `agent-native-design.md §2.3`).
3. **Identity — one `Principal`, checked everywhere.** Every action — human, agent, or service;
   via UI, CLI, git wire, API, or event-trigger — resolves to a `Principal` and is authorized
   by the one policy engine (ADR-03). No subsystem implements its own auth
   (`technical-structuring.md §3.3`).

**Secondary contracts built on the triad** (also binding where adopted): `PersonalDataHolder`
(ADR-12), `ToolDef`/`ToolSurface` (ADR-08), the shared content model (ADR-05), the shared
database/views primitive (ADR-06), and the query AST (ADR-07).

**The Reference Graph** is built from `ref.created`/`ref.removed` events (events are
authoritative for edges); backlinks are the inverse index, **permission-filtered at read time**
via ADR-03's `list-objects`; refs render by *current* state per viewer, tombstoning gracefully
on erasure (`technical-structuring.md §2.3`).

### Rationale
This triad is *the wedge* (`use-cases.md §3`, Phase-1 README §3.4) and the answer to risk #1.
Making it binding law — enforced by the monorepo's shared crates (ADR-01) and a no-cross-DB
lint — is what mechanically prevents the glue from rotting into integrated-by-API.

### Consequences
- The **PR context pane** (UC-X-3) is the integration test of the whole thesis: Git resolves
  the PR's `ArtifactRef`s via Refs, renders each via the target subsystem's projection API,
  permission-filtered per viewer via Id, kept live by Bus update events — *every* shared system
  participates, *no* subsystem touches another's DB (`technical-structuring.md §4.3`). Walked
  through in [`system-overview.md`](./system-overview.md).
- The exact dotted event names are a **Phase-3 deliverable**; the *envelope and addressing
  shape* is the contract here, not the names (`technical-structuring.md §0`).

### Deferred / Open
- **[OPEN → P3]** The canonical event taxonomy (exact dotted names); `visibility` semantics.
- **[OPEN → P3/P4]** Whether issue hierarchy/relations and knowledge db-relations live as edge
  types *in* Refs or as subsystem-local materialised structures projected into Refs (TE-7) —
  world-scale rollups may force a local tree; the *contract* (refs are events, backlinks are
  permission-filtered) holds either way.
- **[OPEN → P3]** Cross-tenant references (public OSS repo referenced from another tenant) as a
  special visibility-gated case that must not become a personal-data side-channel
  (`gdpr-eu-sovereignty.md §3.1`).

---

## ADR-14 — Primary datastore/tech per system (summary table)

**Status: DECIDED (directional).** A single high-level map; per-store/per-subsystem refinement →
P3/P4. Consolidates ADR-02 and ADR-10.

> All choices are the **Phase-2 default direction**; a subsystem/shared-system agent may diverge
> with written justification (VISION §4, ADR-02). All stores are **residency-pinned + per-tenant
> envelope-encrypted + crypto-shred-capable + `PersonalDataHolder`s** (ADR-11, ADR-12), running
> inside a cell on EU-deployable, self-hostable primitives (ADR-11).

### Shared systems

| Shared system | Language (default) | Primary datastore/tech (directional) |
|---|---|---|
| **Identity & Access** | Rust | Postgres (principals/orgs/tokens) + **Zanzibar/SpiceDB-class tuple store** (ReBAC, ADR-03) |
| **Event Bus** | Rust | Durable log (**Kafka/Redpanda or NATS JetStream or PG-outbox** — ADR-04) + outbox in each subsystem's PG; **separate firehose transport** |
| **Reference Graph** | Rust | Built from `ref.created` events; edge/back-index store (PG or graph-index); permission-filtered via authz `list-objects` |
| **Search & Indexing** | Rust | **Tantivy / OpenSearch / Meilisearch-class**, ACL-aware (`list-objects` pre-filter), multilingual, vector; fed by the bus |
| **Notifications** | Rust | PG (routing/prefs/history) + push/email/web delivery; consumes the bus; storm-control/dedup |
| **Agent Fabric** | Rust | Trait boundary (`myelin-agent`); `MockAgentRuntime` now; runs on durable-workflow substrate (ADR-09); tool registry in PG |
| **Storage** | Rust | **Postgres-class (OLTP)** + **S3-compatible (object)** + **log/firehose tier**; KMS/crypto-shred (ADR-12) |
| **GDPR/Audit** | Rust | DSR orchestrator + KMS + tamper-evident audit log + data-map registry (ADR-12) |
| **Durable-workflow** (ADR-09) | Rust (or self-hosted Temporal) | Durable-execution store; powers automations/HITL/SLA timers |
| **OLAP/read store** | — | **ClickHouse-class** columnar, fed by the bus (CQRS; SC-5) |

### Subsystems

| Subsystem | Language (default) | Primary datastore/tech (directional) | Notable divergence flag |
|---|---|---|---|
| **Git hosting** | Rust | PG (PR/review/repo metadata) + object store (packs/LFS/bundles); git core via `gix`/libgit2/shell-out (TE-8) | git serving maturity (TE-8) |
| **CI/CD** | Rust | PG (run state) + object/log tiers (artifacts/caches/logs — heaviest, SC-9); microVM isolation (TE-28) | runner elasticity on EU infra (TE-29) |
| **Issue tracker** | Rust | PG (issues + flexible fields via JSONB/derived projection, TE-17) + OLAP read store (analytics) | flexible-field query perf (TE-17) |
| **Knowledge** | Rust | PG (block tree/db rows) + object store (media) + CRDT snapshots; collab engine (TE-15) | **CRDT vs OT (TE-15)** — prototype |
| **Chat** | Rust **(BEAM/Elixir candidate for connection tier only — TE-21)** | Wide-column chat log (TE-13) + PG (channel/membership metadata) + firehose for presence/typing | **connection tier language (TE-21)** |

### Rationale
Provides every Phase-4 agent a single consistent starting map so choices don't silently
conflict, while honouring VISION §4 (Rust default, justify divergence) and ADR-11 (portable/
sovereign primitives). The two flagged likely divergences (chat connection tier, git core) are
the ones Phase 1 surfaced with real justification; both are P4 decisions, not foreclosed here.

### Deferred / Open
All concrete engine selections, sharding internals, and the flagged divergences → P3/P4 as noted
per row.

---

## ADR-15 — Open-questions carry-forward (the working backlog)

Phase 2 inherits the Phase-1 register (`open-questions-and-risks.md`) and carries it forward,
tagging each item with its resolver phase. Items **Phase 2 has now resolved (the model/direction)**
vs **still open** vs **legal**:

### Resolved by Phase 2 (direction committed; instantiation may defer)
| Item | Phase-1 ref | Resolved in |
|---|---|---|
| Monorepo vs polyrepo | TE-3 | ADR-01 (monorepo + Cargo workspace) |
| Permission model formalism | TE-2/SC-1 | ADR-03 (ReBAC/Zanzibar, directional) |
| Bus delivery semantics + firehose split | TE-9/11/12, SC-4 | ADR-04 (at-least-once + idempotent; 2 transports; directional tech) |
| Shared rich-content/block model | TE-4 | ADR-05 (adopt AST, not the editor) |
| Shared database/views + field-def primitive | TE-5 | ADR-06 (adopt definition/view/AST, not the engine) |
| Single query AST | TE-6 | ADR-07 (adopt, permission-aware) |
| Agent fabric strategy-pattern boundary | (VISION §3) | ADR-08 (ratified) |
| Automations = agents = one trigger engine | (Phase-1 assumption) | ADR-08 (ratified) |
| Durable-workflow semantics | TE-20 | ADR-09 (durable-execution; build-vs-adopt → P3) |
| Storage tiering + portability constraint | TE-13 | ADR-10/ADR-14 (directional) |
| Cell topology + tenancy/residency commitment | SC-2/SC-3 | ADR-11 (ratified) |
| GDPR `PersonalDataHolder` spine carried intact | GD-3 et al. | ADR-12 |
| Three glue contracts as binding law | TE-1 | ADR-01 + ADR-13 |

### Still open → Phase 3 (shared systems)
Delegation/on-behalf-of algebra (AG-2); `Agent` vs `Service` kinds (AG-1); concrete authz tuple
schema + consistency tokens + caching (TE-2); durable bus + firehose transport selection (TE-9/
11); `EventMatcher` predicate language (AG-7); event taxonomy/dotted names (TE-10); durable-
workflow build-vs-adopt (TE-20); KMS key hierarchy + crypto-shred granularity (GD-4); DSR
orchestrator design (GD-3); retention/consent/sub-processor registries; cell sizing + tenant→
cell assignment + multi-cell tenants + control-plane-with-zero-personal-data (SC-2/SC-3); the
query AST grammar (TE-6); whether Refs owns hierarchy/relations or subsystems do (TE-7);
cross-tenant reference visibility gating; agent loop/runaway adversarial validation (AG-4) and
agent load governance (AG-5).

### Still open → Phase 4 (subsystems)
Issue-model duality board↔roadmap (PR-2); governance baked-in-vs-opt-in schemes (PR-3);
flexible-field query model (TE-17); formula/rollup engine (TE-18); drag-reorder ranking (TE-19);
CRDT-vs-OT + block-tree storage (TE-15/16); chat connection-tier transport + language (TE-21);
diff-anchoring across rewrites (TE-22); SHA-1-vs-256 (TE-23); git storage/replication backend
(TE-24); monorepo ambition (TE-25); forks/merge-queue/web-edit scope (TE-26); world-scale code
search scope (TE-27); CI isolation model (TE-28); runner ownership/EU infra (TE-29); component
registry supply-chain (TE-30); CI↔agent substrate unification depth (TE-31); CI metering unit
(TE-32); human-readable monotonic keys (TE-14); git core build-vs-embed (TE-8); migration/import
fidelity (PR-8).

### [OPEN — LEGAL] → counsel/DPO before binding
Art. 17 erasure scope into immutable git history (GD-1/GD-2); audit-log retention carve-out
(GD-5); free-text PII erasure completeness (GD-6); Schrems-III / EU–US DPF stability (GD-7);
CLOUD-Act exposure of hyperscaler "EU sovereign" partnerships (GD-8); EU AI Act final
classification (GD-9); Gaia-X/EUCS/NIS2/DORA/eIDAS-2/Data-Act applicability (GD-10); controller/
processor classification per category (GD-11); worklog/productivity special-category data
(GD-13); GDPR-vs-LLM erasure of decision-influencing data (AG-8); EU-sovereign real-LLM
sub-processor (AG-9).

### Commercial (outside engineering phases, feeds positioning)
Segment priority/WTP (PR-1); CD scope ambiguity (PR-5); CI config format (PR-6); pricing/
packaging/GTM/certification roadmap (PR-9); the narrowing agent-native gap (PR-10).

---

## Cross-references
- [`VISION.md`](../../VISION.md) — non-negotiables this register implements.
- [`system-overview.md`](./system-overview.md) — the holistic narrative, diagrams, and
  end-to-end walkthroughs that show these decisions interacting.
- [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) — the
  structural foundation (§2 shared systems, §3 glue contracts, §5 cells, §13 open questions)
  these ADRs commit.
- [`01-research/open-questions-and-risks.md`](../01-research/open-questions-and-risks.md) — the
  register ADR-15 carries forward.
- [`01-research/gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md) — owns the
  constraints ADR-11/ADR-12 ratify.
- [`01-research/agent-native-design.md`](../01-research/agent-native-design.md) — the design
  ADR-08/ADR-09 ratify.
- **Seeds Phase 3:** the [OPEN → P3] items in ADR-15 are the shared-systems backlog.
- **Seeds Phase 4:** the [OPEN → P4] items in ADR-15 are each subsystem agent's starting
  decisions, on the premises these ADRs set.
