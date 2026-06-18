# Phase 2 — Shared Systems Overview (the eight-system high-level architecture)

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth; never contradicted). Phase-2 spine this doc aligns to:
> [`architecture-decisions.md`](./architecture-decisions.md) (the ADR register — *what we
> decide and why*) and [`system-overview.md`](./system-overview.md) (the holistic narrative).
> Phase-1 foundation: [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) §2
> (the eight shared systems), [`01-research/gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md),
> [`01-research/agent-native-design.md`](../01-research/agent-native-design.md).

This document is the **high-level architecture of the eight shared backend systems** — the
"glue" that makes five subsystems *one product by construction* (`technical-structuring.md §0`).
The ADR register decides the cross-cutting questions; the system overview tells the holistic
story. **This doc zooms in one level on each shared system**: what it owns, its internal
structure, its tech direction, the **contracts/APIs it exposes to subsystems**, the **CLI/admin
surface** it needs, how it **scales in the cell topology**, and its **open questions**. It then
describes **how the shared systems interact with each other**. It is the **springboard for Phase
3** (detailed shared-systems architecture), which owns every `[OPEN → P3]` carried here.

**Altitude.** This is Phase 2: *structure and direction*, not Phase-3 schemas/algorithms.
Where this doc would have to invent a tuple schema, a wire format, a sharding scheme, or an
engine choice, it **names the decision and defers it** with the same `[OPEN → P3]` tags the spine
uses. It does not re-open any DECIDED ADR; where it sharpens one, it cites the ADR and stays
inside the decision.

---

## 0. The eight systems at a glance (and the two ratified substrates)

The eight shared systems (VISION §2; `technical-structuring.md §2`), plus the two cross-cutting
substrates Phase 2 ratified alongside them (durable-workflow engine, ADR-09; OLAP read store,
ADR-10):

| # | Shared system | One-line role | Owning crate(s) | Primary ADR |
|---|---|---|---|---|
| 1 | **Identity & Access (Id)** | One `Principal`; one ReBAC policy engine; the `list-objects` filter | `myelin-identity`, `myelin-tenancy` | ADR-03, ADR-11 |
| 2 | **Event Bus (+ trigger engine)** | Canonical envelope, outbox, per-aggregate order; triggers/automations/agents; firehose split | `myelin-events` | ADR-04, ADR-08 |
| 3 | **Reference Graph (Refs)** | Bidirectional, live, permission-aware edges between any artifacts | `myelin-refs` | ADR-13 |
| 4 | **Search & Indexing** | Cross-artifact + per-subsystem search; FT + structured + vector; ACL-aware at query time | (consumes `myelin-query`, `myelin-identity`) | ADR-03, ADR-07, ADR-10 |
| 5 | **Notifications (Notif)** | The one prioritised "what needs *me*" inbox; delivery fabric; storm control | (consumes `myelin-events`) | ADR-12 |
| 6 | **Agent Fabric** | Strategy-pattern boundary (`MockAgentRuntime` now); plan-then-apply; tool registry; safety | `myelin-agent` | ADR-08, ADR-09 |
| 7 | **Storage** | Three tiers (txn / object / log-firehose); per-tenant envelope encryption + crypto-shred | (per-tier clients) | ADR-10, ADR-12 |
| 8 | **GDPR / Audit** | `PersonalDataHolder` spine; DSR orchestrator; KMS/crypto-shred; tamper-evident audit; data map | `myelin-gdpr` | ADR-12 |
| + | **Durable-workflow engine** | Durable execution for automations/HITL/SLA timers | (under Agent Fabric / Bus) | ADR-09 |
| + | **OLAP read store** | CQRS analytics read model fed by the bus | — | ADR-10 |

**Three properties hold for every one of them** (they are not repeated per-system below):

1. **Residency-pinned + per-tenant envelope-encrypted + crypto-shred-capable + a
   `PersonalDataHolder`** (ADR-11, ADR-12). Every store inside every shared system inherits the
   cell's region and registers with the DSR orchestrator.
2. **Multi-tenant by partition key, not by convention** — `tenant` + `region` are part of the
   addressing of every record, topic, index, edge, tuple, and queue (ADR-11).
3. **Rust by default** for the shared-system cores and the glue crates non-negotiably (ADR-02);
   the glue crates *are* the contract surface, so they never diverge.

A reader's map of the rest of this doc: §1–§8 are the eight systems; §9 is the two ratified
substrates; §10 is **how the shared systems interact with each other** (the inter-shared-system
glue — the part Phase 1/2 only sketched); §11 is the consolidated CLI/admin surface; §12 is the
consolidated open-questions handoff to Phase 3.

---

## 1. Identity & Access (Id)

### 1.1 What it owns
The single `Principal` abstraction (`Human` / `Agent` / `Service`) and the **one ReBAC policy
engine** that authorizes *every* action in the platform — UI, CLI, git wire, API, and the
event-trigger path — for humans and agents identically (ADR-03, ADR-13.3; `technical-structuring.md
§2.1`). It owns authentication (SSO/SAML/OIDC, SCIM, MFA, passkeys, SSH keys, scoped service/API
tokens, short-lived CI job tokens, scoped agent tokens), the org→team→project hierarchy with
inheritance, **agent delegation/on-behalf-of**, principal lifecycle (offboarding, ownership
transfer, break-glass, tenant decommission), and the **legal-role classification** of principals
and records (`tenant-content` vs `platform-operational`, ADR-12.5). Identity is also where
**erasure-vs-integrity is resolved** via the pseudonym mapping (ADR-12.4; `gdpr-eu-sovereignty.md
§8.1`).

### 1.2 High-level internal structure
Two cooperating planes behind one façade:

- **The principal/auth plane** — Postgres-class store of principals, orgs/teams/projects,
  credentials, tokens, SSO/SCIM connections, agent-identity records (`runtime_ref`, owner,
  `on_behalf_of`, capabilities, policy, lifecycle — `agent-native-design.md §1.2`). This plane is
  the RBAC *authoring surface*: admins assign roles; roles are **compiled into relationship
  tuples**.
- **The authorization plane** — a **Zanzibar/SpiceDB-class tuple store** holding
  `object#relation@subject` tuples, evaluating `check` and `list-objects` with consistency
  tokens ("zookies") for read-your-writes (ADR-03, ADR-14). This is the hot path; it must be
  cacheable and horizontally scalable. The **pseudonym/identity-indirection table** (real
  identity ↔ per-tenant pseudonym) also lives in this system's custody because it is the lever
  for git/bus/audit erasure (ADR-12.4).

ABAC-style attribute predicates are supported **at the edges** (e.g. "field visible only if
`issue.severity < X`") but kept off the hot `list-objects` path — "ReBAC core, RBAC face, ABAC at
the edge" (ADR-03).

### 1.3 Tech direction
Rust core; Postgres for the principal plane; a **SpiceDB-class** self-hostable Zanzibar store for
tuples (ADR-14). Both EU-deployable and self-hostable (ADR-11). The exact tuple schema, relation
namespaces per subsystem, consistency-token strategy, caching, and the delegation algebra are
**[OPEN → P3]** (ADR-03).

### 1.4 Contracts / APIs exposed to subsystems (the glue)
- `authenticate(credential) -> Principal` — resolves any entrypoint's credential to a principal.
- `check(subject, permission, object) -> Decision` — the **per-action gate** every entrypoint calls
  (ADR-03). Synchronous, in the request hot path (`system-overview.md §7.1`).
- `list_objects(subject, permission, type) -> {visible ids | filter}` — the **permission-filtered
  read primitive** Search and Refs consume to *pre-filter* (no N+1, no leak) — the single most
  load-bearing inter-shared-system contract (ADR-03 §Consequences; §10.2 below).
- `list_subjects(object, permission) -> subjects` — the inverse, for "who can see this" admin views.
- `delegation(agent, trigger_actor) -> effective policy` — the `agent.policy ∩ delegation ∩
  tenant.policy` intersection the Agent Fabric's `EffectApi` calls on every effect (ADR-08.3).
- `resolve_pseudonym(subject, tenant)` / `revoke_pseudonym(...)` — the erasure lever (ADR-12.4).
- Authz tuples are mutated **as a consequence of subsystem events** (`iam.permission_granted`,
  membership changes) so authz state is itself event-sourced and auditable (`agent-native-design.md
  §2.2`).

`myelin-identity` carries the `Principal`, capability/permission types, and the authz **client**
that every subsystem links (ADR-01). A subsystem **never implements its own auth** (ADR-13.3).

### 1.5 CLI / admin surface
`myelin auth login`/`whoami`; `myelin org|team|project create|grant|revoke`; `myelin role
…`/`myelin policy show <principal> <object>` (explainability: *why* can/can't P see O — a
ReBAC "explain"); `myelin agent create|grant|suspend|retire`; `myelin token issue|list|revoke`
(service/agent/CI tokens with scopes + TTL); `myelin sso configure`, `myelin scim …`. Admin views
(Phase-2 design language): the **permission inspector** ("who can see this / why"), org hierarchy
editor, agent roster with delegation, token/credential management, break-glass with audit.

### 1.6 Scale in the cell topology
Authz is the highest-QPS shared system (every action checks). It scales **inside a cell** via
tuple-store caching + consistency tokens; the tuple store is tenant-partitioned. Cross-cell is
avoided by construction — a principal's authority is evaluated in the cell that holds the object
(no cross-region authz in personal-data hot paths, ADR-11 §Consequences). Multi-cell tenants make
"a principal spanning cells" a hard case → **[OPEN → P3]** (SC-2/SC-3).

### 1.7 Open questions
`[OPEN → P3]` tuple schema + relation namespaces; consistency-token/caching strategy; the
**delegation/on-behalf-of algebra** (AG-2); **`Service` vs `Agent` as one kind or two** (AG-1);
cross-tenant reference visibility gating (so public-OSS refs aren't a PII side-channel); multi-cell
principal authority.

---

## 2. Event Bus (and the trigger / automation engine)

### 2.1 What it owns
The **canonical, versioned, ordered, durable stream of domain events** — the agent-native backbone
and simultaneously the feed for Refs, Search, Notif, Agents, OLAP, and external webhooks (ADR-04;
`technical-structuring.md §2.2`). It owns the **common envelope** (ADR-13.2), **transactional-outbox
emission**, **per-aggregate ordering**, **at-least-once + idempotent** delivery semantics, the
**firehose/control split**, and the **trigger/automation/agent engine** that sits on top
(automations and agents are *one* engine with different action handlers — ADR-08.5).

### 2.2 High-level internal structure
Four parts:

- **The durable bus** — partitioned, replayable, per-partition (= per-aggregate) ordered log
  carrying the canonical envelope (ADR-04.2). Partition key = aggregate (one PR / issue / run /
  doc). Bounded retention + crypto-shred + tombstones (ADR-04.4). **References-not-payloads**:
  events carry `ArtifactRef`s + pseudonymous identities; personal data lives in an erasable store
  the event points to (ADR-12.4).
- **The outbox relay** — each subsystem writes the event to an **outbox table in the same DB
  transaction** as its state change (ADR-04.3); a relay drains the outbox to the bus. This is the
  dual-write fix and the `myelin-events` consumer/producer template carries idempotency-on-`event_id`
  (ADR-04.1).
- **The firehose transport** — a *separate* low-latency fan-out path for high-volume ephemeral
  streams (CI log lines, chat presence/typing/read-state, collab op-streams). The durable bus
  carries only "data available/updated" **pointer events** into it; an agent is never woken per log
  line (ADR-04.5; `technical-structuring.md §2.2`).
- **The trigger engine** — three increasing tiers over the durable bus (`agent-native-design.md
  §3`): **Subscriptions** (deliver matching events), **Automations** (durable multi-step workflows,
  on the ADR-09 substrate), **Agent triggers** (wake an agent). A `Trigger` binds an
  `EventMatcher` (expressed in the shared query AST, ADR-07) to a target under a `run_as` principal
  with a `RunBudget`, `DelegationPolicy`, and HITL `gates` (ADR-08.5). The same `EventMatcher`
  predicate core is shared with saved-view filters (ADR-07 §Consequences) — one safe-evaluation
  engine, one DoS-hardening surface (AG-7).

### 2.3 Tech direction
Rust core. Durable bus: **Kafka/Redpanda** or **NATS JetStream** or **Postgres logical-decoding/
outbox** — selected in P3 against the hard constraint *bounded retention + crypto-shred +
tombstones on EU-deployable infra* (ADR-04 §Candidate tech; GD §10 Q9). Firehose: a dedicated
low-latency fan-out (NATS/Redis-class for presence; an append-mostly log/object tier for CI logs).
Both **[OPEN → P3]**.

### 2.4 Contracts / APIs exposed to subsystems (the glue)
- **The envelope itself** is the contract (ADR-13.2): non-negotiable fields `event_id`, `type`,
  `schema_ver`, `tenant`, `region`, `actor`, `subject` (`ArtifactRef`), `causation_id`,
  `correlation_id`, `contains_personal_data`, `visibility`.
- `emit(event)` via the **outbox helper** in `myelin-events` (transactional; ADR-04.3).
- `subscribe(matcher, consumer)` — the subscription primitive (also serves Search indexing, Refs
  building, Notif fan-out, webhooks — `agent-native-design.md §3.3`).
- `register_trigger(Trigger)` — bind matcher → target (subscription | automation | agent).
- Firehose `publish(stream, frame)` / `tail(stream, range)` for ephemeral/log streams.
- The **canonical event taxonomy** (exact dotted names like `git.pr.opened`) is **[OPEN → P3]**;
  the *envelope and addressing shape* is the Phase-2 contract, not the names (ADR-13 §Consequences;
  `technical-structuring.md §0`).

### 2.5 CLI / admin surface
`myelin event tail [--type … --tenant …]` (filtered firehose for ops/debug); `myelin trigger
create|list|disable|test` (with `--dry-run` matcher evaluation); `myelin automation …`; `myelin
webhook add|list|rotate-secret`; `myelin bus replay <correlation_id>` (replay a workflow for
debugging); `myelin bus retention show|set`. Admin views: the **automation/trigger builder**
(Zapier-class authoring UX over the query AST — `agent-native-design.md §6 Zapier row`), the event
inspector, webhook management, and dead-letter/replay tooling.

### 2.6 Scale in the cell topology
Per-aggregate (not global) ordering is what makes the bus horizontally scalable (ADR-04.2). Heavy
cross-system work (refs build, search indexing, rollups, analytics) is **async off the bus**, never
synchronous in the write path (ADR-11.5). The firehose split prevents CI-log/chat-presence volume
from melting the durable bus (SC-9/SC-10). **Agent-generated load** is governed on the durable-bus
path via budgets, loop caps, and per-tenant circuit breakers (ADR-08.6; §6 below). The bus is
in-cell; cross-cell event propagation for multi-cell tenants is **[OPEN → P3]**.

### 2.7 Open questions
`[OPEN → P3]` durable-bus + firehose transport selection; partitioning/sharding internals;
**`EventMatcher` predicate language** (CEL/JSONLogic/custom, AG-7); replay/compaction strategy;
exact retention windows; the canonical event taxonomy/dotted names (TE-10); cross-cell propagation.

---

## 3. Reference Graph (Refs)

### 3.1 What it owns
The **bidirectional, live, permission-aware graph of edges** between any artifact and any other —
the mechanism by which a chat message references a commit/issue/doc/run and that link **stays
meaningful and resolves to current state per viewer** (ADR-13.1; `technical-structuring.md §2.3`).
Refs owns universal addressing (`ArtifactRef`), edge storage + the backlink inverse-index, live
resolution with graceful tombstoning on erasure, and hot-artifact scale (a popular PR with
thousands of inbound edges).

### 3.2 High-level internal structure
- **Edges are events** (ADR-13 §"The Reference Graph"). The graph is *built from*
  `ref.created`/`ref.removed` events on the bus; any subsystem or agent creating a reference is
  simply emitting that event. The producers are the `mention`/`artifact_ref`/`embed` nodes in the
  shared content model (ADR-05 §Consequences) — refs are emitted *from content*, uniformly.
- **The edge/back-index store** — an edge table `(source, rel, target)` plus the inverse index
  for backlinks ("what references this?"). Postgres or a graph-index, tenant-partitioned (ADR-14).
- **Permission-filtered read** — backlinks are filtered at read time via Id's `list-objects` (you
  only see backlinks from artifacts you can read; ADR-03 §Consequences; ADR-13). This is the same
  graph-×-access-control join that recurs across the platform.
- **Live resolution / projection** — Refs does *not* store artifact content; it resolves an
  `ArtifactRef` to the target subsystem's **projection API** (current title/state, per viewer),
  degrading to a tombstone placeholder when the target is deleted/erased.

### 3.3 Tech direction
Rust; built from the bus; edge/back-index in Postgres-class or a graph-index store (ADR-14);
permission-filtered via the authz `list-objects` client. Whether issue hierarchy/relations and
knowledge db-relations live as **edge types in Refs** or as **subsystem-local materialised
structures projected into Refs** is **[OPEN → P3/P4]** (TE-7; ADR-06 §Deferred, ADR-13 §Deferred) —
world-scale rollups may force a local tree; the *contract* (refs are events, backlinks are
permission-filtered) holds either way.

### 3.4 Contracts / APIs exposed to subsystems (the glue)
- `ArtifactRef` = `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` — the addressing contract
  (ADR-13.1). Every subsystem exposes **stable, resolvable IDs down to sub-artifact granularity**
  (a PR comment, a doc block, a CI step).
- Every subsystem implements an **`ArtifactRef` projection API**: resolve a ref to (a) its current
  rendered projection for unfurls/embeds, (b) a permission check, (c) update events for cache
  invalidation (ADR-13.1; `system-overview.md §8.1`).
- `edges(ref) -> outbound` and `backlinks(ref, viewer) -> permission-filtered inbound` (the
  read API the PR context pane uses).
- Edge creation is **just `ref.created` emission** — there is no separate write API; the bus is
  authoritative (ADR-13).

`myelin-refs` carries the edge types + client.

### 3.5 CLI / admin surface
`myelin refs of <artifact>` / `myelin backlinks <artifact>` (permission-filtered); `myelin refs
graph <artifact> --depth N` (explore the neighbourhood); admin: orphaned/dangling-edge reports,
tombstone audit. Mostly a *read* surface; writes flow through content + the bus.

### 3.6 Scale in the cell topology
Hot-artifact fan-in (UC-EDGE-6) is the scale concern — thousands of inbound edges on a popular
artifact must stay fast; backlink reads are permission-filtered, so they compose with `list-objects`
caching. Tenant-partitioned; **cross-tenant edges** (public-OSS repo referenced from another tenant)
are a special **visibility-gated** case that must not become a personal-data side-channel
**[OPEN → P3]** (ADR-13 §Deferred; `gdpr-eu-sovereignty.md §3.1`).

### 3.7 Open questions
`[OPEN → P3/P4]` Refs-owns-hierarchy vs subsystem-local-tree (TE-7); cross-tenant reference
visibility gating; hot-artifact backlink scale strategy.

---

## 4. Search & Indexing

### 4.1 What it owns
**Unified ranking across all artifact types in one query** *and* per-subsystem search (code, docs,
chat, issues, CI runs) — full-text + structured/field + **semantic/vector** — **permission-aware at
query time**, multilingual, residency-pinned (ADR-03, ADR-07, ADR-10; `technical-structuring.md
§2.4`). "A user must never find what they cannot access" is the single most pervasive correctness
hazard, and a leak is both a security and a GDPR breach (SC-1).

### 4.2 High-level internal structure
- **The index tier** — full-text (multilingual per-language analyzers), structured/field index (for
  issue custom fields, knowledge db properties), and a **vector index** for semantic search / agent
  RAG / triage dedup. Tenant-partitioned, residency-pinned (the index inherits the tenant's region).
- **The indexer** — near-real-time incremental indexing fed **off the bus** (subsystems emit;
  Search indexes; ADR-10). Idempotent on `event_id` (ADR-04.1).
- **The permission-aware query path** — every query **pre-filters via Id's `list-objects`** rather
  than post-filtering results (no leak, no N+1; ADR-03 §Consequences). Queries arrive as the shared
  **query AST** (ADR-07), which is **permission-aware by construction** — it always composes with
  the authz filter, so no search surface can return artifacts the subject can't see.

### 4.3 Tech direction
Rust-first: **Tantivy (Rust)** is the leading candidate, with OpenSearch / Meilisearch-class as
alternatives (ADR-10, ADR-14). Must support `list-objects` pre-filtering and vector search;
**embeddings are personal data and must be erasable** (ADR-12; `gdpr-eu-sovereignty.md §6.6`). Engine
selection, the ACL-filter integration mechanism, and the vector approach are **[OPEN → P4 (Search)]**.

### 4.4 Contracts / APIs exposed to subsystems (the glue)
- `query(ast, viewer) -> ranked results` — the AST (ADR-07) compiled to the search backend, always
  composed with `list-objects(viewer, read, type)` (ADR-03).
- Indexing is **implicit**: subsystems emit events; Search subscribes. A subsystem declares *what*
  is indexable (field definitions from `myelin-query`, ADR-06) and *how* it projects to an index
  document; it does not call a write API per change.
- Search is a `PersonalDataHolder`: `locate`/`erase` (purge + re-index, not just hide — ADR-12;
  `gdpr-eu-sovereignty.md §3.5`).

### 4.5 CLI / admin surface
`myelin search "<query>" [--type … --tenant …]` (AST-backed, permission-filtered as the calling
principal); `myelin search reindex <subsystem|tenant>`; `myelin search index status`. Admin: index
health, reindex/backfill jobs, relevance/analyzer config, vector-index management. End-user search
UX is a subsystem-frontend concern (Phase-2 design language ships the unified search surface).

### 4.6 Scale in the cell topology
Indices are per-tenant, in-cell, fed async off the bus (ADR-11.5). World-scale **code search** is a
multi-year effort and is **scoped down for v1** (ADR-14; `git-hosting.md §4.5`). The dominant risk is
keeping permission-filtered queries fast over large result sets — solved by `list-objects`
pre-filtering co-designed with Id, not post-filtering (ADR-03).

### 4.7 Open questions
`[OPEN → P4]` engine selection; the precise `list-objects`↔index integration (filter push-down vs
pre-fetch); vector/embedding approach + its erasure; multilingual analyzer set; code-search v1 scope.

---

## 5. Notifications (Notif)

### 5.1 What it owns
The **one prioritised, cross-subsystem "what needs *me*" inbox** (UC-X-7) and the delivery fabric
(email/push/web/mobile/desktop) — explicitly *not* owned by chat (`chat.md §1`;
`technical-structuring.md §2.5`). It owns per-user routing, preferences, quiet-hours/DND, **storm
control / dedup**, **on-call / escalation routing** (for agent escalations and SLA breaches), and
notification history (a `PersonalDataHolder`).

### 5.2 High-level internal structure
- **The router** — consumes the bus, applies per-user routing + preferences + quiet hours + **storm
  control/dedup** (UC-EDGE-4), and decides *channel* (in-app inbox, email, push, …).
- **The inbox store** — the prioritised per-user "what needs me" model (Postgres-class, ADR-14),
  with read/seen state. **Targeted write-fanout** for high-signal events (e.g. "you were mentioned"),
  while low-signal bodies stay read-fanout (`chat.md §5.2`).
- **The delivery adapters** — email/push/web/desktop, each a **region-aware, swappable,
  EU-preferring adapter** (email/SMS providers are sub-processors — ADR-12.8;
  `gdpr-eu-sovereignty.md §3.7`).
- **Escalation/on-call** — escalation policies for agent escalations and SLA breaches (UC-AG-19,
  UC-EDGE-25), riding the durable-workflow substrate for timed escalation chains (ADR-09).

### 5.3 Tech direction
Rust; Postgres for routing/prefs/history/inbox; consumes the bus; pluggable EU-preferring delivery
adapters (ADR-14). Storm-control/dedup algorithm and the priority model are **[OPEN → P3]**.

### 5.4 Contracts / APIs exposed to subsystems (the glue)
- Notif is **bus-driven**: subsystems do not call a "notify" API per change; they emit events and
  Notif decides what is notifiable. A subsystem may annotate events with notification hints via the
  envelope (`visibility`, mention nodes in content → "notify this principal").
- `mentions` are first-class: a `mention(Principal)` node in the shared content model (ADR-05) is
  the canonical "notify this principal" producer — uniform across chat, issues, docs.
- `preferences(principal)` read/write API for the settings UI.
- Notif is a `PersonalDataHolder` (history erasable).

### 5.5 CLI / admin surface
`myelin notify prefs show|set`; `myelin oncall show|page`; `myelin notify test <channel>`; admin:
escalation-policy editor, delivery-adapter config (per region/tenant), storm-control thresholds.
The **unified notification inbox** is a flagship Phase-2 design-language surface (the cross-subsystem
"what needs me").

### 5.6 Scale in the cell topology
In-cell, bus-driven; storm-control caps fan-out under event spikes (including agent-generated
volume — §6). Delivery adapters egress *out* of the cell (email/push) — a **sovereignty egress
review** point (`gdpr-eu-sovereignty.md §3.3`): prefer EU providers, keep PII out of notification
payloads where possible.

### 5.7 Open questions
`[OPEN → P3]` priority/ranking model; storm-control/dedup algorithm; on-call/escalation data model
(jointly with the durable-workflow engine); push/email provider sovereignty posture.

---

## 6. Agent Fabric

### 6.1 What it owns
The **strategy-pattern boundary** behind which a `MockAgentRuntime` lives today and an
`LlmAgentRuntime` lives later — a config/implementation swap, not a rewrite (VISION §3 non-negotiable;
ADR-08; `agent-native-design.md §4`). It owns agent identities (via Id), the
`AgentRuntime`/`Agent`/`ToolSurface`/`EventInbox`/`EffectApi` traits, the **plan-then-apply** execution
model, the permissioned **tool registry** (one catalogue, exposable over MCP later), and the
**safety machinery**. It is the integration point of half the shared systems (ADR-08 §Consequences).

### 6.2 High-level internal structure
- **The runtime boundary** (`myelin-agent`) — the small trait set. **No LLM SDK, prompt, or model
  name appears anywhere in platform code** (ADR-08.2); all of it lives behind `LlmAgentRuntime`,
  introduced later. `MockAgentRuntime` (deterministic, rule-driven) ships now and makes the entire
  event→trigger→effect→event loop integration-testable with golden tests + `cargo-mutants`
  (`agent-native-design.md §4.5`).
- **The plan-then-apply engine** — agents are a pure-ish function `(event, context) →
  AgentDecision { effects, … }`; they **never perform side effects directly** (ADR-08.3). The
  platform's **`EffectApi` validates each effect** against **permissions ∩ delegation ∩ tenant
  policy** (calls Id, §1.4), budget, and HITL gates, then applies it — emitting domain events that
  may wake more agents, governed by loop caps. **Identical platform code for mock and real.**
- **The tool registry / `ToolSurface`** — every subsystem registers typed `ToolDef`s (name +
  JSON-schema input + required caps + effect kind + side-effecting flag) into one shared catalogue
  (ADR-08.4). The same registry is consumed internally and **exposable over MCP** to external agents
  later — defined once, governed once.
- **The event inbox** — the platform *delivers* matched events to an agent (driven by triggers,
  §2.2); agents don't poll. The `InboxEvent` carries the envelope, the matched trigger, the
  delegation token, the remaining budget, and prior-turn history (`agent-native-design.md §4.3`).
- **The safety machinery** (structural, not an appendix — ADR-08.6): least-privilege per-run
  permissions; per-run/agent/tenant budgets; **loop/runaway protection** via `causation_id` depth
  caps + cycle detection + idempotent tools + per-tenant circuit breakers; **HITL gates** as durable
  workflow waits surfaced as **chat approval cards** (ADR-09; the Chat subsystem is the HITL
  surface); attribution + tamper-evident audit of every agent action; agents always labelled as
  agents (AI Act). **Suggest-by-default; human-confirm consequential actions** (GDPR Art. 22).

### 6.3 Tech direction
Rust; trait boundary in `myelin-agent` with `MockAgentRuntime` now; runs on the **durable-workflow
substrate** (ADR-09) for budgets/gates/long HITL waits; tool registry in Postgres (ADR-14). The
`Agent::handle` signature (single-call vs driven multi-turn loop), streaming, and context
management are **provisional** and may revise when `LlmAgentRuntime` is built — but the
plan-then-apply core must survive (AG-3).

### 6.4 Contracts / APIs exposed to subsystems (the glue)
- `register_tool(ToolDef)` — every subsystem contributes its actions to the shared `ToolSurface`
  (ADR-08.4). This is how "agents and humans in the same channel" stays coherent: an agent's *only*
  way to act is the same permissioned catalogue, checked by the same Id engine.
- `EffectApi.apply(run, effect) -> Applied | Gated | Denied` — the platform-owned write-back path;
  subsystems expose their mutations *as tools*, not as agent-callable back-doors.
- Agent triggers are registered via the bus trigger engine (§2.2); the Agent Fabric is the *target
  handler*, not a separate event system (ADR-08.5).
- Agent identities are `Principal`s in Id (§1) — attribution, audit, and delegation reuse the same
  machinery as humans (ADR-08.1).

### 6.5 CLI / admin surface
`myelin agent create|grant|suspend|retire` (shared with Id, §1.5); `myelin agent run <id>
--dry-run` (replay an event through a mock agent, see proposed effects without applying — the
plan-then-apply payoff for testability); `myelin tool list|describe`; `myelin agent budget
show|set`; `myelin agent runtime set <id> <runtime_ref>` (the mock→real swap is a config change).
Admin: the agent roster, the **HITL approval queue** (surfaced in chat), budget/circuit-breaker
dashboards, the audit trail of agent actions.

### 6.6 Scale in the cell topology
In-cell; agent processing stays in-region (no cross-region agent runs on personal data, ADR-11).
The novel scale+safety concern is **agent-generated load**: agents generate volume far beyond
humans and agents-waking-agents can cascade — bounded by `causation_id` depth caps, budgets,
idempotent tools, and per-tenant circuit breakers (ADR-08.6; `technical-structuring.md §5.3`,
§6.4). **Adversarial validation of loop/runaway protection is [OPEN → P3 + testing P5]** (AG-4/AG-5).

### 6.7 Open questions
`[OPEN → P3]` delegation algebra (AG-2); `Agent` vs `Service` kinds (AG-1); `Agent::handle`
signature / streaming / context management (AG-3); adversarial loop/runaway + load-governance
validation (AG-4/AG-5). `[OPEN → P4 (CI)+P3]` **CI↔agent execution-substrate unification** (a CI
job and an agent run have the same shape — event → sandboxed work → results+events; TE-31) — flagged,
not decided (ADR-08 §Consequences). `[OPEN — LEGAL]` AI-Act classification (GD-9); GDPR-vs-LLM
erasure of decision-influencing data (AG-8); EU-sovereign real-LLM sub-processor (AG-9).

---

## 7. Storage

### 7.1 What it owns
The **durable primitives every subsystem leans on**, with residency + per-tenant envelope
encryption + crypto-shred baked in (ADR-10, ADR-12; `technical-structuring.md §2.7`). It is *not* a
single database — it is the **tiering + the portability constraint** plus the cross-cutting
KMS/crypto-shred. **No subsystem reads another subsystem's store** (ADR-01, ADR-13).

### 7.2 High-level internal structure (the three tiers + specialized stores)
- **Transactional (OLTP)** — domain state: repos/PR metadata, issues, doc blocks/rows, message
  metadata, run state. **Postgres-class**; JSONB + GIN + generated columns for flexible fields;
  distributed-SQL (CockroachDB/Yugabyte-class) only where a single shard outgrows Postgres (ADR-10,
  TE-17).
- **Object/blob** — LFS blobs, CI artifacts/caches, doc media, attachments, avatars, clone bundles,
  backups. **S3-compatible** (MinIO/Ceph self-hostable; EU providers); content-addressed, dedup,
  residency-pinned.
- **Log/firehose** — CI logs, chat message log, collab op-streams. Append-mostly tail+archive+
  range-read; **wide-column (Cassandra/Scylla-class)** is a candidate for the chat message log
  (TE-13, SC-10).
- **Specialized stores owned by their systems but on the same constraint**: the **authz tuple
  store** (Id, §1), the **search index** (§4), the **OLAP read store** (§9), the **durable-workflow
  store** (§9) — each residency-pinned, crypto-shred-capable.
- **The KMS / crypto-shred layer** — per-tenant envelope-encryption key hierarchy threaded through
  *every* tier; **crypto-shredding is a first-class deletion primitive** (destroy the key →
  ciphertext unrecoverable in DBs/backups/immutable logs). This layer is co-owned with GDPR/Audit
  (§8) — Storage provides the mechanism, GDPR drives the policy (ADR-12.3).

### 7.3 Tech direction
Rust clients; Postgres-class OLTP + S3-compatible object + log/firehose tier; KMS/crypto-shred
abstraction (ADR-14). All commodity, portable, self-hostable, EU-deployable — proprietary global
managed services are forbidden (ADR-11; `gdpr-eu-sovereignty.md §2.2`). Concrete engines, sharding
internals, and per-subject-vs-per-tenant crypto-shred granularity are **[OPEN → P3/P4]** (GD-4).

### 7.4 Contracts / APIs exposed to subsystems (the glue)
Storage is consumed as **tier clients** (an OLTP connection within the subsystem's own schema, an
object-store client, a log-tier client), each wired through the KMS so encryption + crypto-shred
are automatic. The cross-cutting contract is the **KMS/crypto-shred abstraction in `myelin-gdpr`**
(ADR-01) and the rule that every tier is a `PersonalDataHolder`. There is no generic "storage API"
spanning subsystems — the boundary is enforced by the no-cross-DB lint (ADR-01 §Consequences).

### 7.5 CLI / admin surface
`myelin storage usage [--tenant … --tier …]`; `myelin kms key list|rotate|shred`; `myelin backup
list|restore` (with **post-restore re-erasure** so a restore doesn't resurrect erased data,
`gdpr-eu-sovereignty.md §6.5`); `myelin storage residency verify <tenant>` (prove region pinning).
Admin: per-tenant storage/quota dashboards, key-custody (BYOK/HYOK) management, backup/retention
policy, residency attestation.

### 7.6 Scale in the cell topology
CI is the **heaviest storage consumer** and drives shared-storage requirements (TE-13; ADR-14).
OLTP shards by tenant; the **OLAP read store is fed by the bus** (CQRS) so analytics scans don't kill
OLTP (ADR-10 §Consequences, SC-5). Object/log tiers are content-addressed + residency-pinned.
Self-host parity: the same Storage artifacts run a managed cell and an on-prem install (ADR-11).

### 7.7 Open questions
`[OPEN → P3/P4]` concrete engine selection per store; sharding/partitioning internals;
**per-subject vs per-tenant crypto-shred granularity** (GD-4); HYOK's limits on what search/agents
can do over data they can't decrypt (GD §10 Q10); backup-window-vs-erasure-SLA residual exposure.

---

## 8. GDPR / Audit

### 8.1 What it owns
The cross-cutting machinery that makes "GDPR by construction" a **structural property, not a
feature** (ADR-12; `gdpr-eu-sovereignty.md §5`). It owns the `PersonalDataHolder` contract (the
spine), the **DSR Orchestrator**, the **KMS/crypto-shred** policy (mechanism in Storage, §7), the
**one tamper-evident audit log**, the **data map / classification registry**, and the
retention/consent/sub-processor registries.

### 8.2 High-level internal structure
- **The `PersonalDataHolder` registry** — every store and subsystem registers as a holder
  implementing `locate / export / rectify / restrict / erase` for a subject. The holder list is
  **exhaustive**: all 5 subsystem DBs, object store, **search index**, **event-bus history**,
  caches/CDN, **backups**, **agent memory/embeddings**, **reference graph**, notification history,
  authz tuples, and audit (carve-out + expiry). "We forgot the search index" is a *structural
  failure* (ADR-12.1; GD-3).
- **The DSR Orchestrator** — fans a subject- or tenant-scoped request to *all* holders, tracks the
  statutory deadline (1 month, extendable to 3), produces verifiable receipts, and is operable by
  Myelin **and by/for tenants** (Art. 28 assistance). Idempotent, resumable, verifiable (ADR-12.2;
  `gdpr-eu-sovereignty.md §5.2`). Erasure uses **crypto-shred (Storage) + pseudonym-mapping deletion
  (Id) + tombstoning (Refs) + purge-and-reindex (Search)** — see the §10.6 fan-out.
- **The audit log** — one append-only, tamper-evident, minimised, retention-bounded log recording
  every **human *and* agent** action; itself a carved-out, retention-bounded holder (ADR-12.9).
- **The data map / classification registry** — **generated** from schema-level personal-data tags
  (not hand-curated, so it can't drift) → inventory → RoPA, DPIA inputs, breach scoping (ADR-12.6).
- **Registries** — retention engine (per-category TTL + automated expiry), consent registry
  (versioned, withdrawable, propagating), sub-processor/transfer registry (versioned, public +
  per-tenant, change-notify + objection; transfers off-by-default and gated). (`gdpr-eu-sovereignty.md
  §5.3–5.5`.)

### 8.3 Tech direction
Rust; `myelin-gdpr` carries the `PersonalDataHolder` trait + DSR client + crypto-shred/KMS
abstraction (ADR-01). DSR orchestrator design, KMS key hierarchy, crypto-shred granularity,
retention engine, and the registries are **[OPEN → P3]** (ADR-12 §Deferred).

### 8.4 Contracts / APIs exposed to subsystems (the glue)
- **`PersonalDataHolder`** (the binding GDPR contract — **implementing it is a condition of being a
  subsystem/store**, ADR-12 §Consequences, ADR-13): `locate / export / rectify / restrict / erase`.
- `DSR.submit(subjectRef | tenant, kind) -> receipt` — the orchestrator entrypoint (Myelin- and
  tenant-operable).
- `audit.record(action)` — every human/agent action (mostly emitted *via* the bus + a dedicated
  audit consumer, so audit can't be bypassed).
- `classify(field) -> {is_personal, category, basis, retention}` — schema-level tags feeding the
  generated data map (ADR-12.5/.6).
- `data_role(record) -> tenant-content | platform-operational` — drives DSAR routing + deletion
  authority (ADR-12.5).

### 8.5 CLI / admin surface
`myelin dsr submit --subject … --kind access|erasure|export|rectify|restrict`; `myelin dsr status
<id>` / `myelin dsr receipt <id>`; `myelin tenant offboard <tenant>` (export + verifiable complete
deletion — erasure at tenant granularity); `myelin audit query`/`myelin audit export --tenant`;
`myelin datamap show|export-ropa`; `myelin retention show|set`; `myelin subprocessor list`. Admin:
the **DSR console** (deadline tracking, receipts), the audit explorer, the generated RoPA/data-map
view, retention/consent/sub-processor management, breach-scoping tooling.

### 8.6 Scale in the cell topology
The cell topology delivers GDPR wins "for free": residency by construction, **breach blast-radius =
one cell**, clean per-tenant/per-cell erasure and offboarding (ADR-11). The DSR fan-out is in-cell
(a subject's data lives in their tenant's cell); a multi-cell tenant's DSR must fan across that
tenant's cells — **[OPEN → P3]** (SC-2). The audit log is itself a holder under a legitimate-interest
carve-out (ADR-12.9).

### 8.7 Open questions
`[OPEN → P3]` DSR orchestrator design; KMS key hierarchy; crypto-shred granularity (GD-4); retention
engine; consent + sub-processor registries; post-restore re-erasure (GD-14); multi-cell DSR fan-out.
`[OPEN — LEGAL]` Art. 17 scope into immutable git history (GD-1/GD-2); audit-log retention carve-out
(GD-5); free-text PII completeness (GD-6); Schrems-III posture (GD-7); AI-Act classification (GD-9).

---

## 9. The two ratified cross-cutting substrates

These were ratified in Phase 2 alongside the eight (ADR-09, ADR-10; `system-overview.md §3`). They
are not a ninth/tenth "system" so much as substrates the eight sit on.

### 9.1 Durable-workflow engine (ADR-09)
**Durable-execution semantics** (Temporal-style: deterministic workflow orchestration +
non-deterministic, retryable, sandboxed activities + durable timers + signals for HITL waits) are
the substrate for **automations and human-gated agent/automation workflows** (`agent-native-design.md
§3.2`). The mapping: the **workflow** is durable and owns budget/gates/state; the **agent reasoning
step + tool calls are activities**. It also backs **SLA timers** (millions of durable timers, SC-11)
and **escalation chains** (Notif, §5). **Build-vs-adopt is directionally "prefer a self-hostable,
EU-deployable durable-execution substrate"** (self-hosted Temporal, a Rust-native library, or
bespoke) — **[OPEN → P3]** (TE-20); Temporal-the-cloud-service is disallowed by ADR-11. Exposed to
the Agent Fabric (gates/budgets), the trigger engine (automations), the Issue tracker (SLA timers),
and Notif (escalation).

### 9.2 OLAP read store (ADR-10)
The **CQRS analytics read model fed by the bus** (ClickHouse-class columnar, ADR-14). Issue
analytics, delivery health, and roadmap delivery-state read from here, not from OLTP — the standard
answer to "analytics scans would kill the OLTP store" (SC-5; `issue-tracker.md §6.5`). It is a
holder (it derives from personal data) and is fed async off the durable event stream, reinforcing
that **the bus is the analytics source of truth** (ADR-10 §Consequences).

---

## 10. How the shared systems interact with each other (the inter-shared-system glue)

The eight systems are glued to the *subsystems* by the three contracts (ADR-13). But the harder,
less-charted wiring is how the eight glue to *each other*. This is the part Phase 3 must build, so
it is the heart of this springboard. The recurring shape: **the Bus is the spine; Id is the gate;
GDPR is the fan-out.**

### 10.1 The Bus feeds Refs, Search, Notif, Agents, OLAP, Audit (one stream, many consumers)
The durable bus is the single fan-out point. Every one of these consumers is **idempotent on
`event_id`** (ADR-04.1) and **registered as a subscription** (ADR-04, `agent-native-design.md §3.3`):
`ref.created`/`removed` → Refs builds edges; every domain event → Search incrementally indexes →
Notif routes/dedups → OLAP appends the analytics row → the trigger engine matches → Audit records.
This is why "automations vs events-for-agents" is *not* a split — one engine, many handlers (ADR-08.5;
`system-overview.md §7.2`).

### 10.2 Id gives Search and Refs the `list-objects` filter (permission-aware reads everywhere)
Search and Refs **must not call `check` per result** — they consume Id's `list_objects` to
*pre-filter* (ADR-03 §Consequences). This couples their Phase-3/4 designs to the authz store's
`list-objects` semantics and consistency model (read-your-writes via consistency tokens). This is
the **most load-bearing inter-shared-system contract**: a leak here is both a security and a GDPR
breach (SC-1; `system-overview.md §5.2`). The PR context pane (`system-overview.md §8.1`) is its
integration test: Refs asks Id `list-objects` to filter targets before resolving them.

### 10.3 Agent Fabric calls Id for plan-then-apply (effect validation)
Every proposed effect is validated by `EffectApi` against **Id** (`permissions ∩ delegation ∩
tenant policy`), budget, and HITL gates *before* it is applied (ADR-08.3; §6.2). Agents are
`Principal`s in Id (ADR-08.1), so attribution/audit/delegation reuse the same machinery as humans.
The Agent Fabric is the integration point of half the shared systems: it **reads the Bus** (inbox),
**validates via Id**, **runs on the durable-workflow engine** (gates/budgets/long waits), **acts via
subsystem tools** (the `ToolSurface`), and **writes to Audit** (ADR-08 §Consequences).

### 10.4 Agent Fabric + durable-workflow + Notif/Chat (the HITL loop)
A sensitive effect returns `Gated` → the Agent Fabric opens a **HITL gate as a durable workflow
wait** (ADR-09) → the wait is surfaced as a **chat approval card** (Notif/Chat are the HITL surface)
→ a human signal (minutes or days later) resumes the workflow → the effect applies (ADR-08.6;
`system-overview.md §8.2`). This single loop wires Agents ↔ durable-workflow ↔ Notif ↔ Chat ↔ Id ↔
Audit — the agent-native flagship.

### 10.5 Storage's KMS underpins GDPR's crypto-shred (mechanism vs policy)
Storage provides the **per-tenant envelope-encryption + crypto-shred mechanism** (§7.2); GDPR/Audit
drives the **policy** (when to shred, per the DSR orchestrator and retention engine) (ADR-12.3). The
KMS abstraction in `myelin-gdpr` is the shared seam. Every tier (OLTP, object, log, search index,
authz tuples, OLAP, workflow store) is crypto-shred-capable, which is what makes erasure tractable
against backups and immutable logs (`gdpr-eu-sovereignty.md §6.5`).

### 10.6 GDPR's DSR fans out to all of them (erasure reaches everything)
A DSR is the **inverse fan-out** of the bus: the DSR Orchestrator calls `locate/export/erase` on
*every* `PersonalDataHolder` — the five subsystems **and** Search, Refs, Bus history, Agent memory,
Notif history, authz tuples, OLAP, and Audit (carve-out) (ADR-12.1; `system-overview.md §8.3`).
Erasure composes the systems' primitives: **Id** deletes the pseudonym mapping (git/bus history then
holds only pseudonyms), **Storage/KMS** crypto-shreds keys (backups + immutable logs), **Search**
purges + re-indexes, **Refs** tombstones (unfurls degrade gracefully), **Agents** erase memory/
embeddings. "Erasure reaches everything" is the compliance twin of "permission-aware reads
everywhere" — both are properties of the *whole* structure (`system-overview.md §5.2`).

### 10.7 The query AST is the shared language across Bus-triggers, Search, and saved views (ADR-07)
One AST (`myelin-query`) is the canonical filter/selection representation for **UI saved views, CLI,
API, automations (`EventMatcher`), and agent triggers** — declarative, safe-to-evaluate,
machine-constructable, and **permission-aware by construction** (always composes with `list-objects`).
The trigger `EventMatcher` (Bus, §2.2) and saved-view filters (Search, §4) **share the AST's
predicate core** — one safe-evaluation engine, one DoS-hardening surface (ADR-07 §Consequences,
AG-7). The content model (`myelin-content`, ADR-05) and the field/view primitive (`myelin-query`,
ADR-06) are the other shared *data* contracts these systems speak.

### 10.8 Interaction matrix (who depends on whom)
"→" = "calls / consumes from". Read row → column.

| ↓ uses → | Id | Bus | Refs | Search | Notif | Agents | Storage | GDPR | Workflow | OLAP |
|---|---|---|---|---|---|---|---|---|---|---|
| **Id** | — | emits authz events | | | | governs agents | tuple+PII store | holder; pseudonym lever | | |
| **Bus** | actor authz | — | feeds | feeds | feeds | feeds (inbox) | outbox/firehose | holder (history) | drives automations | feeds |
| **Refs** | `list-objects` | built from events | — | | | | edge store | holder; tombstone | | |
| **Search** | `list-objects` | indexed off bus | | — | | serves RAG | index store | holder; purge+reindex | | |
| **Notif** | | consumes | | | — | agent escalations | history store | holder | escalation timers | |
| **Agents** | effect validation | inbox + emits | creates edges | RAG/dedup | approval cards | — | tool registry | holder (memory) | gates/budgets | |
| **Storage** | | | | | | | — | KMS↔crypto-shred | workflow store | feeds |
| **GDPR** | subject+pseudonym | history holder | tombstone holder | purge holder | history holder | memory holder | KMS/crypto-shred | — | | holder |
| **Workflow** | run authz | signals/timers | | | escalation | activities | durable store | holder | — | |
| **OLAP** | | fed by bus | | | | | columnar store | holder | | — |

The pattern the matrix makes visible: **Bus is the fan-out spine** (its row/column are densest),
**Id is the universal gate** (every read/write path checks it), **GDPR is the universal fan-in**
(every store is a holder), and **the Agent Fabric is the densest consumer** of the rest — exactly
the "integration point of half the shared systems" the spine names (ADR-08 §Consequences).

---

## 11. The consolidated CLI / admin surface

Per-system CLI is listed in §1–§8. At the platform level, the shared systems expose **one coherent
`myelin` CLI** (one of the clients in the layering, `system-overview.md §2`), authorized by **one
`Principal`** like every other entrypoint (ADR-13.3). Grouping:

- **Identity/tenancy:** `auth`, `org`/`team`/`project`, `role`/`policy` (with `explain`), `agent`,
  `token`, `sso`/`scim`, `tenant` (incl. `offboard`).
- **Events/automation:** `event tail`, `trigger`, `automation`, `webhook`, `bus replay`.
- **Refs/search:** `refs`/`backlinks`/`refs graph`, `search`/`search reindex`.
- **Agents:** `agent run --dry-run`, `tool`, `agent budget`, `agent runtime set` (the mock→real swap).
- **Notifications:** `notify prefs`, `oncall`, `notify test`.
- **Storage/KMS:** `storage usage`, `kms`, `backup`, `storage residency verify`.
- **GDPR/audit:** `dsr`, `audit`, `datamap`/`export-ropa`, `retention`, `subprocessor`.

Two CLI commands are *load-bearing demonstrations* of the architecture's non-negotiables and should
be treated as first-class: **`myelin agent run --dry-run`** (plan-then-apply / strategy-pattern
testability, ADR-08) and **`myelin dsr submit` + `myelin tenant offboard`** (GDPR-by-construction
fan-out, ADR-12). The admin **views** (permission inspector, automation builder, unified
notification inbox, HITL approval queue, DSR console, generated RoPA/data-map) are the shared
systems' contribution to the **Phase-2 shared design language** catalogue.

---

## 12. Consolidated open questions → Phase 3 (the springboard)

This doc resolves nothing the spine left open; it **sharpens the structure** so Phase 3 can resolve
the backlog. Carried forward, by system (all already in ADR-15; collected here for the P3 agents):

- **Id (ADR-03):** tuple schema + relation namespaces; consistency-token/caching strategy;
  delegation/on-behalf-of algebra (AG-2); `Service` vs `Agent` kinds (AG-1); cross-tenant ref
  visibility gating; multi-cell principal authority.
- **Bus (ADR-04):** durable-bus + firehose transport selection; partitioning/sharding;
  `EventMatcher` predicate language (AG-7); replay/compaction; retention windows; the canonical
  event taxonomy (TE-10); cross-cell propagation.
- **Refs (ADR-13):** Refs-owns-hierarchy vs subsystem-local-tree (TE-7); cross-tenant edge gating;
  hot-artifact backlink scale.
- **Search (ADR-03/07/10):** engine selection; `list-objects`↔index integration mechanism;
  vector/embedding approach + erasure; multilingual analyzers; code-search v1 scope `[→ P4]`.
- **Notif (ADR-12):** priority model; storm-control/dedup algorithm; on-call/escalation data model;
  delivery-provider sovereignty posture.
- **Agents (ADR-08/09):** `Agent::handle` signature/streaming/context (AG-3); adversarial loop +
  load-governance validation (AG-4/AG-5, with P5 testing); CI↔agent substrate unification (TE-31,
  `→ P4 (CI)+P3`).
- **Storage (ADR-10):** per-store engines; sharding; per-subject-vs-per-tenant crypto-shred (GD-4);
  HYOK limits on search/agents (GD §10 Q10); backup-window residual.
- **GDPR/Audit (ADR-12):** DSR orchestrator design; KMS hierarchy; retention engine; consent +
  sub-processor registries; post-restore re-erasure (GD-14); multi-cell DSR fan-out.
- **Substrates (ADR-09/10):** durable-workflow build-vs-adopt (TE-20); OLAP feed/schema.
- **`[OPEN — LEGAL]`** (counsel before binding): Art. 17 into immutable git history (GD-1/2);
  audit-log retention carve-out (GD-5); Schrems-III (GD-7); AI-Act classification (GD-9);
  GDPR-vs-LLM erasure (AG-8); EU-sovereign real-LLM sub-processor (AG-9).

**The two invariants Phase 3 must never break** (`system-overview.md §5.2`): permission-aware reads
everywhere (Id's `list-objects` pre-filter, §10.2) and erasure reaches everything (the DSR fan-out,
§10.6). Every shared-system design Phase 3 produces is validated against those two.

---

## 13. Cross-references
- [`architecture-decisions.md`](./architecture-decisions.md) — the ADRs every decision here cites
  (ADR-03 Id, ADR-04 Bus, ADR-05/06/07 shared content/views/AST, ADR-08 Agents, ADR-09 workflow,
  ADR-10/14 storage/tech, ADR-11 cells, ADR-12 GDPR, ADR-13 glue).
- [`system-overview.md`](./system-overview.md) — the holistic narrative, the §3 shared-systems
  table this doc expands, the §7 lifecycles, and the §8 walkthroughs that exercise the inter-system
  glue of §10.
- [`VISION.md`](../../VISION.md) — the non-negotiables (world-scale, top-tier UX, agent-native,
  GDPR/EU-sovereign, Rust-default) every system inherits.
- [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) §2 — the source
  description of the eight shared systems.
- [`01-research/gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md) §5 — the
  `PersonalDataHolder` + DSR + KMS contracts behind §8.
- [`01-research/agent-native-design.md`](../01-research/agent-native-design.md) §1–§6 — the agent
  fabric, event/trigger model, and safety machinery behind §2 and §6.
- **Seeds Phase 3:** §10 (inter-shared-system glue) and §12 (the per-system `[OPEN → P3]` backlog)
  are the Phase-3 shared-systems work program.
```
