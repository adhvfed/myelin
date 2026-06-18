# Phase 1 — Research: Index & Executive Summary

> Phase: `01-research`. Canonical brief: [`VISION.md`](../../VISION.md) (read it first; it is
> the single source of truth and may not be contradicted).
>
> This README is the entry point to Phase 1. It frames what Myelin is, indexes every Phase-1
> document, summarises the key findings, names the **subsystems** and **shared systems** that
> later phases will build, and points at what Phase 2 should do next.

---

## 1. What Myelin is (one paragraph)

**Myelin is an EU-sovereign, GDPR-by-construction software delivery platform: one platform,
five subsystems — git hosting, continuous integration, an issue tracker (serving engineers,
product managers, *and* corporate governance alike), a Notion-class knowledge platform, and a
chat that references any artifact and puts humans and agents in the same channels.** Its
differentiator is not any single tool — every competitor has a competent git server or wiki —
but the **shared backend** that makes the five *one* product: a single identity/permission
model, one event bus, one cross-artifact reference graph, one agent fabric, one search,
notifications, and storage/GDPR machinery. Because of that shared layer, work flows across the
subsystems without seams, **autonomous agents are first-class principals rather than bolt-ons**,
and the whole thing runs on EU-controlled infrastructure with data-subject rights designed in.
Myelin occupies an empty market intersection — **unified ∧ EU-sovereign ∧ agent-native** — that
no incumbent (GitLab, Atlassian, Microsoft, Linear, Notion, Slack, or the EU-OSS point tools)
holds at once.

---

## 2. Index of Phase 1 documents

| Document | One-line description |
|---|---|
| [`personas.md`](./personas.md) | The 15 human personas (P1–P15: ICs, product/delivery, corporate/enterprise gatekeepers) + 5 agent personas (A1–A5), their pains with today's fragmented stack, and the four organisation archetypes — *who* Myelin serves. |
| [`use-cases.md`](./use-cases.md) | The testable use-case catalogue (`UC-*` ids) across per-subsystem, cross-subsystem ("the wedge"), agent-driven, corporate/GDPR, and scale/migration/offboarding/edge cases, with MVP-vs-later prioritisation. |
| [`competitive-landscape.md`](./competitive-landscape.md) | Per-subsystem and integrated-suite competitor analysis (steal / avoid / wedge), the EU-sovereignty market context, the crisp positioning statement, and a differentiation table. |
| [`gdpr-eu-sovereignty.md`](./gdpr-eu-sovereignty.md) | GDPR and EU digital-sovereignty obligations translated into hard *architectural constraints* (residency, key custody/crypto-shred, the `PersonalDataHolder` contract, DSR orchestrator, cell topology), with `[OPEN — LEGAL]` flags. |
| [`agent-native-design.md`](./agent-native-design.md) | The agent-native decomposition: agent fabric (agents as principals), event bus + propagation, triggers/automations, the strategy-pattern runtime (`AgentRuntime`/`Agent`/`ToolSurface`/`EffectApi`, plan-then-apply, mock→real), and the safety/governance machinery. |
| [`technical-structuring.md`](./technical-structuring.md) | The platform's structural thesis: subsystems own their state on top of shared systems; the three glue contracts (`ArtifactRef`, event envelope, one Principal); the seam matrix; candidate tech directions; biggest structural risks. |
| [`subsystem-deep-dives/git-hosting.md`](./subsystem-deep-dives/git-hosting.md) | Git hosting & code review: object model, repo storage/sharding/replication at scale, review/diff-anchoring UX, protocol, events, the immutability-vs-erasure tension, hardest problems. |
| [`subsystem-deep-dives/continuous-integration.md`](./subsystem-deep-dives/continuous-integration.md) | CI/CD: config-as-code, event-triggered pipelines (the agent-native execution arm), the distributed job scheduler, untrusted-code isolation, log firehose, secrets, EU-sovereign runners. |
| [`subsystem-deep-dives/issue-tracker.md`](./subsystem-deep-dives/issue-tracker.md) | Issue tracker: one flexible issue object serving engineers + PMs + corporate via layered schemes; workflows, hierarchy/rollups, custom fields, the query AST, analytics, and the make-or-break model duality. |
| [`subsystem-deep-dives/knowledge-platform.md`](./subsystem-deep-dives/knowledge-platform.md) | Knowledge platform (Notion-class): block model, databases/views, the CRDT-vs-OT collaboration problem, references/backlinks, the hardest GDPR surface (free-text PII), shared-DB-primitive boundary. |
| [`subsystem-deep-dives/chat.md`](./subsystem-deep-dives/chat.md) | Chat: unified human/agent actor model, mentions/references/permission-aware unfurls, real-time fan-out at millions of connections, write-vs-read fanout, agent loop/abuse safety, chat as the HITL surface. |
| [`open-questions-and-risks.md`](./open-questions-and-risks.md) | The consolidated, de-duplicated register of every open question, assumption, risk, and uncertainty across all Phase-1 docs, grouped by theme with resolver-phase and severity. |

---

## 3. Executive summary of key findings

### 3.1 Personas — who Myelin serves, and the gatekeepers

Three clusters of human personas plus five agent personas, mapped against *which subsystems
they touch* and *which cross-subsystem flows they live in* (the differentiator is the glue):

- **Individual contributors (P1–P5):** backend/frontend/platform-SRE/staff engineers and the
  open-source maintainer. Highest-frequency users; judge Myelin on raw daily UX (git speed, CI
  latency, low context-switching) and on whether agents save time or add noise.
- **Product & delivery (P6–P10):** product manager, engineering manager, program/project
  manager, designer, technical writer. These are why the issue tracker must serve PMs as a
  *co-equal* audience and why Knowledge + Chat matter as much as Git — they are most failed by
  today's "engineering tool vs. management tool" split.
- **Corporate / enterprise gatekeepers (P11–P15):** CTO/VP, security/compliance officer, DPO,
  procurement, and the IT/platform team that *operates* Myelin. They rarely use it daily but
  **decide adoption and lawful operation.** For the EU-sovereign positioning, the DPO (P13),
  security officer (P12), and self-host operator (P15) are central, not edge.
- **Agents (A1–A5):** coding, triage, review, knowledge-curation, ops/SRE — **first-class
  identities** with scoped least-privilege permissions, event-bus triggers, human-in-the-loop
  by default, full audit. Mock in Phase-1 build (strategy pattern), real later by config swap.

The four organisation archetypes (solo/startup, scale-up, regulated enterprise, public-sector)
weight these personas differently; the **scale-up** and **regulated/public-sector** ends are
the hypothesised strongest fits (unvalidated — see PR-1 in the risk register).

### 3.2 Positioning — the wedge

No incumbent is simultaneously (a) a genuine five-in-one unified suite, (b) agent-native by
construction, and (c) credibly EU-sovereign. GitLab/Atlassian/Microsoft are *unified (to
varying degrees) but US-controlled*; Forgejo/Mattermost/Outline/Woodpecker are *sovereign but
separate point tools* (recreating the "stitched" problem if integrated by hand); none has *one
cross-subsystem agent fabric*. **Myelin's durable edge is the combination**, not any single
axis — the agent-native axis alone is narrowing fast (GitLab Duo, GitHub/MS Copilot, CircleCI
"Chunk"). The macro environment is a strong tailwind: the US CLOUD Act/FISA 702 means EU-region
hosting by US hyperscalers is **not** sovereignty; stated EU demand to reduce US-cloud
dependence vastly exceeds the available sovereign alternatives, and EU procurement is starting
to apply explicit sovereignty criteria.

### 3.3 The agent-native + EU-sovereign wedge, structurally

These two non-negotiables are realised by the *same* shared layer:

- **Agent-native** decomposes into four shared capabilities: the **agent fabric** (agents are
  principals in IAM), the **event bus + propagation** (every subsystem emits canonical events;
  agents are just another consumer), **triggers/automations** (one declarative engine —
  subscriptions, durable automations, agent triggers), and the **strategy-pattern runtime**
  (platform code depends only on traits; `MockAgentRuntime` now, `LlmAgentRuntime` later;
  **plan-then-apply** so agents *propose* effects the platform validates and applies). Safety
  is load-bearing: per-run least-privilege, budgets, loop/runaway caps, HITL gates, audit.
- **EU-sovereign by construction** is not a feature but a structural property: a **cell-based,
  region-pinned topology** (each cell a full stack on commodity EU primitives; scale = many
  cells; control plane holds no in-region personal data), per-tenant envelope encryption with
  **crypto-shredding** as a first-class deletion primitive, and a **`PersonalDataHolder`
  contract** every store implements so a data-subject request reaches *everything* (DBs,
  search, event-bus history, backups, agent memory, reference graph, audit). The same
  strategy-pattern mandate that swaps mock→real agents generalises to "every external
  personal-data-touching dependency is a swappable, region-aware, EU-preferring adapter" —
  including the future real LLM backend.

### 3.4 The shared-systems structure (why it's one platform, not five)

A subsystem *is* part of Myelin if it speaks **three glue contracts**, and is a bolt-on if it
doesn't: the **addressing contract** (`ArtifactRef = myelin://<tenant>/<subsystem>/<type>/<id>`,
resolvable to a rendered projection + a permission check + update events), the **event
contract** (the common versioned envelope via transactional outbox, with per-aggregate
ordering + idempotency + tenant/region/actor/correlation), and the **identity contract** (one
`Principal`, checked by one policy engine everywhere — UI, CLI, git wire, API, event-trigger).
Subsystems own their domain state and never reach into each other's databases; all
cross-subsystem interaction flows through the shared layer. The **PR context pane** — a PR
surfacing its issue, the relevant doc section, the CI run, and the discussion inline,
permission-filtered per viewer and kept live by the bus — is the single clearest integration
test of the whole thesis.

---

## 4. The subsystems and shared systems (the canonical list for later phases)

These are the units that Phase 4 (subsystems) and Phase 3/5 (shared systems) will architect.

### 4.1 Subsystems (one Phase-4 agent each)

1. **Git hosting** — repos, code browsing/blame/search, pull/merge requests & review, branch
   protection/merge policy, push-time hooks, the densest reference-graph centre.
2. **Continuous Integration (CI/CD)** — config-as-code pipelines triggered by *any* platform
   event, the distributed job scheduler, sandboxed/untrusted-code execution, EU-sovereign
   runners, logs/artifacts/caches, secrets, deploy mechanics.
3. **Issue tracker** — one flexible issue object serving engineers + PMs + corporate via
   layered workflow/permission/field schemes; hierarchy/rollups, cycles/roadmaps, SLAs,
   analytics, the query AST.
4. **Knowledge platform** — Notion-class block editor, databases with table/board/calendar/
   timeline views, real-time collaboration, references/backlinks, versioning, export.
5. **Chat** — channels/threads/DMs, unified human+agent actors, permission-aware artifact
   unfurls, real-time fan-out, the natural HITL/incident-coordination surface.

### 4.2 Shared backend systems (the glue — owned by Phase 3/5)

1. **Identity & Access (Id)** — one `Principal` (Human / Agent / Service), one policy engine,
   SSO/SCIM/MFA/tokens, the org/team/project hierarchy, agent delegation/on-behalf-of, and the
   likely **Zanzibar-style relationship-based authorization** for permission-filtered reads.
2. **Event Bus (Bus)** — the canonical versioned envelope + outbox emission + per-aggregate
   ordering + at-least-once/idempotent delivery, plus the **trigger/automation engine**
   (subscriptions, durable automations, agent triggers) and the firehose-vs-control split.
3. **Cross-Artifact Reference Graph (Refs)** — bidirectional, live, permission-aware edges
   between any artifacts; backlinks; live resolution with graceful tombstoning.
4. **Search & Indexing (Search)** — unified cross-artifact + per-subsystem, **permission-aware
   at query time**, full-text + structured + semantic/vector, multilingual, residency-pinned.
5. **Notifications (Notif)** — the one prioritised cross-subsystem "what needs *me*" inbox,
   delivery fabric, storm control/dedup, on-call/escalation routing.
6. **Agent Fabric (Agents)** — the strategy-pattern boundary, plan-then-apply execution, the
   permissioned tool registry (one catalogue, internal + MCP), and the safety machinery.
7. **Storage (Storage)** — three tiers (transactional / object-blob / log-firehose), per-tenant
   envelope encryption + crypto-shred, residency-pinned.
8. **GDPR / Audit machinery** — the `PersonalDataHolder` interface, DSR orchestrator,
   KMS/crypto-shred, tamper-evident audit log, data map/RoPA, retention/consent/sub-processor
   registries.

All of the above run inside a **region-pinned cell**, with a **global control plane** that
routes tenants→cells and holds no in-region personal data.

---

## 5. What Phase 2 (`02-holistic-architecture`) should do next

Phase 2 turns this research into a committed high-level architecture and establishes the shared
design language. Concretely, it should:

1. **Adopt the structural commitments** from `technical-structuring.md`: the layering
   (clients → subsystems-own-state → shared systems → cell substrate), the three glue contracts
   (`ArtifactRef`, event envelope, one Principal), and the **cell-based, region-pinned topology**
   as the reconciliation of world-scale and EU-sovereignty.
2. **Ratify or revise the load-bearing assumptions** in `open-questions-and-risks.md` §7 —
   especially monorepo + shared-glue crates (mitigating the #1 risk that the glue rots into
   "integrated-by-API"), the bus delivery model + firehose split, and "automations and agents
   are one trigger engine."
3. **Take a position on the top cross-cutting decisions** that span subsystems before Phase 4
   commits per-subsystem: the **permission model** (Zanzibar-style authz co-designed with Search
   and Refs), the **shared rich-content/block model**, and the **shared database/views +
   field-definition primitive** between Issues and Knowledge.
4. **Establish the shared design language** (principles, tokens, accessibility baseline, the
   catalogue of views) per VISION §5.2 — and enumerate the primary screens each subsystem needs,
   since *design sketches precede frontend implementation*.
5. **Hand the GDPR constraints to Phase 3 intact** — the `PersonalDataHolder` contract, DSR
   orchestrator, crypto-shred KMS, data map, and the erasure-aware bus/search/storage design are
   the spine Phase 3 owns; Phase 2 must not design anything that forecloses them.
6. **Carry the open-questions register forward** as the working backlog, tagging each item with
   its resolver phase, and flag the `[OPEN — LEGAL]` items for counsel/DPO before they bind.

The MVP thesis to test in Phase 2 (from `use-cases.md` §7): the smallest *genuinely
cross-linked* vertical through all five subsystems that makes the shared identity + event bus +
reference graph visibly better than five separate tools — plus the non-negotiable foundations
(one identity model, audit, EU residency, multi-tenancy, and mock agents firing on the real bus).

---

## 6. Definition-of-done checklist for Phase 1

Per VISION §6:

- [x] All deliverables are markdown under `planning/01-research/`.
- [x] Every concern in scope addressed (personas, use cases, competitive/positioning, GDPR/
      sovereignty, agent-native, technical structuring, five subsystem deep-dives) — breadth
      over depth where depth isn't yet due.
- [x] Open questions, assumptions, and risks listed explicitly and consolidated
      ([`open-questions-and-risks.md`](./open-questions-and-risks.md)).
- [x] Cross-references between docs and to `VISION.md` made throughout.
- [ ] Committed and pushed — **owned by the orchestrator**, not the research agents.
