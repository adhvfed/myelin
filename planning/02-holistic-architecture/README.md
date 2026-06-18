# Phase 2 — Holistic Architecture: Index & Executive Summary

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth; never contradicted). Consumes Phase 1
> ([`01-research/README.md`](../01-research/README.md), structural foundation
> [`01-research/technical-structuring.md`](../01-research/technical-structuring.md)).
> Seeds Phase 3 (shared systems) and Phase 4 (subsystems).

---

## 1. What Phase 2 is (one paragraph)

Phase 2 commits the **high-level architecture** of Myelin — how the systems are structured and
built, the **tech** chosen per system, the **views** and **CLI/API** every subsystem needs, the
**usage examples** that prove the design, and **how the subsystems interact as a whole** — plus it
establishes the **shared design language**. It does *not* yet specify schemas, algorithms, or
wire formats: those are Phase 3 (shared systems) and Phase 4 (subsystems). The spine is a set of
**14 architecture decisions (ADRs)** that take cross-cutting positions once, so the five subsystems
sit on a coherent foundation rather than re-litigating the same questions five times. The
load-bearing thesis (from Phase 1, ratified here): **five subsystems own their domain state on top
of eight shared systems, speak three glue contracts (`ArtifactRef`, one event envelope, one
`Principal`), never read each other's databases, and run inside region-pinned cells on commodity
EU primitives** — which is simultaneously the cross-artifact wedge, the agent-native substrate, and
the GDPR/EU-sovereignty mechanism.

---

## 2. Index of Phase 2 documents

| Document | One-line description |
|---|---|
| [`architecture-decisions.md`](./architecture-decisions.md) | **The spine.** The ADR register (ADR-01…ADR-14) — *what we decide and why* — plus the consolidated open-questions carry-forward (ADR-15). Every other Phase-2 doc cites it. |
| [`system-overview.md`](./system-overview.md) | **The map of the whole.** The layering, the cell/control-plane model, the request/event lifecycles, and three end-to-end walkthroughs (PR context pane, agent-native flagship, DSAR fan-out) that exercise every shared system at once. |
| [`shared-systems-overview.md`](./shared-systems-overview.md) | **The eight shared systems, one level down.** What each owns, its internal structure, tech direction, the contracts/APIs it exposes, its CLI/admin surface, how it scales in a cell, and the inter-shared-system glue (§10). The springboard for Phase 3. |
| [`design-language.md`](./design-language.md) | **The shared design language.** Design principles, the dual-audience resolution, token direction, the WCAG/i18n/RTL baseline, the shared component/interaction patterns, the agent-native UX contract, and the **consolidated catalogue of views** (§7) for Phase-4 sketching. |
| [`cli-and-api.md`](./cli-and-api.md) | **The unifying CLI & API conventions.** One surface, three consumers (humans, scripts, agents); the CLI grammar mirroring `ArtifactRef`; the REST/GraphQL/gRPC/git-wire/MCP/webhook strategy; the common response envelope; the cross-cutting verbs. |
| [`subsystems/git-hosting.md`](./subsystems/git-hosting.md) | Git hosting & code review — front-door/serving-tier/control-plane structure; `gix`-vs-shell-out and storage/replication flagged; the PR context pane; the hardest `PersonalDataHolder`. |
| [`subsystems/continuous-integration.md`](./subsystems/continuous-integration.md) | CI/CD — control plane + execution plane bridged by a lease queue; the firehose log pipeline; trust tiers + microVM isolation; the CI↔agent substrate convergence. |
| [`subsystems/issue-tracker.md`](./subsystems/issue-tracker.md) | Issue tracker — one issue object + layered optional schemes serving engineers/PMs/corporate; consumes the shared field/view/query primitive; event-driven rollups; the SLA engine. |
| [`subsystems/knowledge-platform.md`](./subsystems/knowledge-platform.md) | Knowledge (Notion-class) — block tree + collaborative editing (CRDT/OT, Knowledge-owned); databases + formula/rollup; the heaviest refs producer; the hardest free-text-PII GDPR surface. |
| [`subsystems/chat.md`](./subsystems/chat.md) | Chat — connection/fan-out/durable-log tiers by scaling profile; per-viewer permission-aware unfurls; the HITL approval-card surface; the one flagged language divergence (connection tier). |
| [`consistency-review.md`](./consistency-review.md) | The cross-document consistency pass: contradictions/misalignments found (and resolutions), plus the consolidated open-questions list carried into Phase 3/4. |

---

## 3. The committed architecture (the spine)

The 14 ADRs, with status. **DECIDED** = committed, later phases implement not re-debate;
**DECIDED (directional)** = direction committed, instantiation deferred to a named phase.

| ADR | Decision (committed) | Status |
|---|---|---|
| **ADR-01** | **Polyglot monorepo + single Cargo workspace + glue crates.** A subsystem *becomes* Myelin by depending on the shared crates (`myelin-events/identity/refs/agent/gdpr/content/query/tenancy`); contracts can't silently drift; a no-cross-subsystem-DB lint is an architecture obligation. | DECIDED |
| **ADR-02** | **Rust is the backend default; divergence allowed only with written justification** (per VISION §4). Glue crates are Rust non-negotiably. Hot-path cores stay Rust; the one flagged likely divergence is the chat connection tier. | DECIDED |
| **ADR-03** | **Relationship-based (Zanzibar-style) authorization** as the one permission model — "ReBAC core, RBAC face, ABAC at the edge." Natively answers `check` and the `list-objects` permission-filtered read; co-designed with Search and Refs. | DECIDED (directional) |
| **ADR-04** | **Event bus: at-least-once + idempotent-on-`event_id`; per-aggregate (not global) ordering; transactional outbox; references-not-payloads + crypto-shred; the firehose/control split** (durable control events vs ephemeral CI-log/presence/collab-op streams). | DECIDED (directional) |
| **ADR-05** | **One shared rich-content/block model** (`myelin-content`) — share the AST + the `mention`/`artifact_ref`/`embed` nodes; **concurrency/editing stays subsystem-owned**. "Share the AST, not the editor." | DECIDED |
| **ADR-06** | **One shared structured-collection primitive** (`myelin-query`): field definitions + the view abstraction (table/board/calendar/timeline). **Workflow/SLA/formula engines and physical query execution stay subsystem-owned.** "Share the schema language and view model, not the query planner." | DECIDED |
| **ADR-07** | **One query AST** for saved views, CLI, API, automations (`EventMatcher`), and agent triggers — declarative, safe-to-evaluate, machine-constructable, **permission-aware by construction**. | DECIDED |
| **ADR-08** | **Agent Fabric strategy-pattern boundary, plan-then-apply.** Agents are first-class `Principal`s; a small trait set (`AgentRuntime`/`Agent`/`ToolSurface`/`EventInbox`/`EffectApi`); `MockAgentRuntime` now, `LlmAgentRuntime` later by config swap; one tool catalogue (MCP-exposable); one trigger engine for subscriptions/automations/agents; structural safety (budgets, loop caps, HITL, audit, AI-Act labelling). | DECIDED |
| **ADR-09** | **Durable-execution semantics** (Temporal-style: workflows + retryable activities + durable timers + HITL signals) as the substrate for automations, human-gated agent flows, and SLA timers. Build-vs-adopt → P3 (must be EU-self-hostable). | DECIDED (directional) |
| **ADR-10** | **Datastore tiering + the portability constraint.** Postgres-class OLTP, S3-compatible object, log/firehose tier, ClickHouse-class OLAP (CQRS off the bus), Tantivy-class search, SpiceDB-class authz tuples — **all EU-deployable/self-hostable; no hyperscaler-locked managed services.** | DECIDED (directional) |
| **ADR-11** | **Cell-based, region-pinned topology.** A cell = a complete stack of all subsystems + shared systems; scale = add cells; residency = the cell's region; self-host = one cell on the same artifacts; every record carries immutable `tenant`+`region`; a global control plane holds **no in-region personal data**. | DECIDED |
| **ADR-12** | **GDPR by construction: the `PersonalDataHolder` spine.** Exhaustive holder list (incl. search index, bus history, agent memory, backups); the DSR orchestrator; crypto-shred as a first-class deletion primitive; PII kept out of immutable structures (pseudonyms + references-not-payloads); schema-level legal-role classification; generated data map; one tamper-evident audit log. | DECIDED |
| **ADR-13** | **The three glue contracts are binding platform law.** `ArtifactRef` addressing, the common event envelope, one `Principal` checked by one engine — every subsystem implements all three; **no subsystem reads another's DB, ever.** | DECIDED |
| **ADR-14** | **Primary datastore/tech per system** (the consolidated directional map, consolidating ADR-02 + ADR-10). | DECIDED (directional) |
| **ADR-15** | The open-questions carry-forward (the working backlog for P3/P4/legal). | — |

**Two cross-cutting invariants** the whole architecture is judged against (system-overview §5.2):
**(1) Permission-aware reads everywhere** — search results, backlinks, unfurls, and lists are
*pre-filtered* by Id's `list-objects`, never post-filtered (a leak is both a security and a GDPR
breach). **(2) Erasure reaches everything** — every store, including bus history, the search
index, the reference graph, agent memory, and backups, is a `PersonalDataHolder` the DSR
orchestrator fans out to.

**Two cross-cutting substrates** ratified alongside the eight shared systems: the
**durable-workflow engine** (ADR-09) and the **OLAP read store** (ADR-10, CQRS fed by the bus).

---

## 4. Tech per system (directional — ADR-14; subsystems may diverge with written justification)

| System | Language | Primary datastore/tech (directional) | Flagged divergence |
|---|---|---|---|
| Identity & Access | Rust | Postgres (principals/orgs/tokens) + **Zanzibar/SpiceDB-class** tuple store | — |
| Event Bus | Rust | Durable log (**Kafka/Redpanda \| NATS JetStream \| PG-outbox**) + per-subsystem outbox + **separate firehose transport** | transport selection → P3 |
| Reference Graph | Rust | Edge/back-index store (PG or graph-index), built from `ref.created` events | refs-vs-local-tree (TE-7) |
| Search & Indexing | Rust | **Tantivy / OpenSearch / Meilisearch-class**, ACL-aware (`list-objects`), multilingual, vector | engine + vector → P4 |
| Notifications | Rust | Postgres (routing/prefs/history/inbox) + EU-preferring delivery adapters | provider sovereignty |
| Agent Fabric | Rust | `myelin-agent` trait boundary; `MockAgentRuntime`; on the durable-workflow substrate; tool registry in PG | `Agent::handle` shape (AG-3) |
| Storage | Rust | **Postgres-class OLTP + S3-compatible object + log/firehose tier** + KMS/crypto-shred | engines + crypto-shred granularity |
| GDPR/Audit | Rust | DSR orchestrator + KMS + tamper-evident audit + generated data map | DSR/KMS design → P3 |
| Durable-workflow | Rust (or self-hosted Temporal) | Durable-execution store | build-vs-adopt (TE-20) |
| OLAP read store | — | **ClickHouse-class** columnar, fed by the bus | — |
| Git hosting | Rust | PG (PR/repo metadata) + object store; git core via `gix`/libgit2/shell-out | git core (TE-8), storage/replication (TE-24) |
| CI/CD | Rust | PG (run state) + object/log tiers; microVM isolation | runner elasticity on EU infra (TE-29) |
| Issue tracker | Rust | PG (issues + JSONB flexible fields + derived projection) + OLAP | flexible-field query perf (TE-17) |
| Knowledge | Rust | PG (block tree/rows) + object store + CRDT snapshots; **Yrs** candidate | CRDT-vs-OT (TE-15) — prototype |
| Chat | Rust **(BEAM/Elixir candidate for connection tier only)** | Wide-column message log + PG (channel metadata) + firehose | connection-tier language (TE-21) |

**Frontend stack (design-language §8, open per VISION §4):** TypeScript + a React-class framework
as the default, with **one shared component library + design-token package** in the monorepo, a
**type-safe generated API/types** layer, and **WASM-Rust at the edges** where a canonical Rust
implementation already exists (the content AST + sanitiser, the query-AST parser, diff rendering).
Self-hosted assets, no third-party CDNs (sovereignty). Per-subsystem stack divergence allowed with
written justification, but the token package + glue-contract rendering stay shared.

---

## 5. The design language + the catalogue of views

[`design-language.md`](./design-language.md) is the coherence backbone for every frontend. Its
spine: **nine design principles** (one product not five tools; speed; keyboard-first/mouse-complete;
progressive disclosure; earned density; reference-everything; visible/labeled/trustworthy agents;
calm-by-default; sovereignty-as-UX); the **dual-audience resolution** (persona-adaptive views over
shared primitives, never a product fork); a **three-tier token system** with first-class dark mode,
a reserved `agent` treatment, and shared functional-status colors; a **WCAG 2.2 AA + EU-multilingual
+ RTL** baseline; the **shared components** (navigation shell, command palette, the reference
chip/unfurl, the agent/HITL approval card, comments/mentions, the tables/boards/views component, the
block editor, the notifications inbox, the cross-cutting empty/loading/error/permission/erased
states); and the **agent-native UX contract** (labeled, plan-then-apply, HITL approval cards,
attribution/audit, calm volume).

**The catalogue of views** ([`design-language.md` §7](./design-language.md)) is the single Phase-4
sketching checklist: every primary screen across Git hosting (§7.1), CI/CD (§7.2), Issues (§7.3),
Knowledge (§7.4), Chat (§7.5), and the shared/identity/admin/GDPR surfaces (§7.6) — plus the CLI as
a first-class peer surface (§7.7). Each view inherits the shared components, tokens, accessibility,
and agent surfaces, and **each requires empty/loading/error/permission-denied/erased states sketched
before any UI code** (VISION §3/§5.2).

---

## 6. Handoff — what Phase 3 and Phase 4 do next

**Phase 3 — shared-systems architecture.** Owns the detailed design of the eight shared systems and
the two substrates, grounded in the literature (consensus, log-structured storage, pub/sub,
indexing, Zanzibar). It resolves the `[OPEN → P3]` backlog (ADR-15; shared-systems-overview §12):
the **ReBAC tuple schema + consistency tokens + delegation algebra**; the **bus + firehose transport
selection + event taxonomy + `EventMatcher` predicate language**; the **durable-workflow
build-vs-adopt**; the **KMS hierarchy + crypto-shred granularity + DSR orchestrator**; the **cell
sizing + tenant→cell assignment + multi-cell-tenant** story; and Refs-owns-hierarchy-vs-local-tree
(TE-7). It must never break the two invariants (§3) and must keep the GDPR constraints (ADR-12)
intact. The §10 inter-shared-system glue is the heart of its work program.

**Phase 4 — subsystem architectures.** Builds each subsystem *on these premises*, resolving the
`[OPEN → P4]` backlog: git storage/replication + git-core build-vs-embed + diff-anchoring + SHA
choice; CI isolation model + runner infra + config grammar + CI↔agent substrate depth; issue-model
duality + flexible-field query engine + rollup/forecast engine; CRDT-vs-OT + block-tree storage +
formula engine; chat connection-tier transport/language + message-store substrate + unfurl
caching. The two **named joint designs** are **Git↔CI** (the commit-status/checks merge gate, the
most load-bearing seam) and **Issues↔Knowledge** (the shared field/view primitive, the biggest
reuse decision). For any frontend, Phase 4 produces **design sketches against the design-language
catalogue before writing UI code** (VISION §3).

**Legal/DPO (before binding).** The `[OPEN — LEGAL]` items — Art. 17 erasure scope into immutable
git history; audit-log retention carve-out; free-text PII completeness; Schrems-III posture; EU AI
Act classification; EU-sovereign real-LLM sub-processor — are flagged for counsel; design-safe
minimums (labelling, HITL, crypto-shred, pseudonyms) are built now.

---

## 7. Cross-references
- [`VISION.md`](../../VISION.md) — the non-negotiables every decision implements.
- [`01-research/README.md`](../01-research/README.md) — the research Phase 2 consumes.
- [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) — the structural
  thesis (§2 shared systems, §3 glue, §4 seams, §5 cells) these ADRs commit.
- **Seeds Phase 3:** the `[OPEN → P3]` items (ADR-15) + shared-systems-overview §10/§12.
- **Seeds Phase 4:** the `[OPEN → P4]` items (ADR-15) + the per-subsystem docs + the view catalogue.
