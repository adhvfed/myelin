# Phase 2 — Subsystem Architecture: Issue Tracker (engineers + PMs + corporate)

> Phase: `02-holistic-architecture`. Altitude: **high-level**. Canonical brief:
> [`VISION.md`](../../../VISION.md) (never contradicted).
> Spine: [`../architecture-decisions.md`](../architecture-decisions.md) (the ADR register) and
> [`../system-overview.md`](../system-overview.md) (the holistic narrative). This doc aligns to
> both; any divergence is called out explicitly.
> Phase-1 foundation: [`../../01-research/subsystem-deep-dives/issue-tracker.md`](../../01-research/subsystem-deep-dives/issue-tracker.md)
> (the territory map) and [`../../01-research/technical-structuring.md`](../../01-research/technical-structuring.md).
>
> Phase 2 commits **direction + structure**; concrete schemas, the flexible-field query engine,
> the rollup algorithm, the SLA-calendar engine, and the sync protocol are **Phase-4** work. Each
> deferral is named with its open-question tag.

---

## 0. One-paragraph summary

The Issue tracker is Myelin's **planning and accountability spine** — the system of record for
units of intended/in-progress work and the structures (cycles, projects, epics, initiatives,
roadmaps, SLAs) that organise that work over time. It serves three audiences with different
mental models — **engineers** (Linear-fast, git-adjacent), **PMs** (roadmaps/forecasting), and
**corporate** (configurable workflows, SLAs, granular permissions, audit, reporting) — over
**one flexible issue object** reshaped by *layered, optional schemes* (workflow / field /
permission), not three bolted-together products (`issue-tracker.md §0, §4.4`). It **owns** the
issue lifecycle/workflow state machine, hierarchy/rollup semantics, cycles/roadmaps, the SLA
engine, and tracker analytics definitions; it **delegates** everything cross-cutting to the
shared layer — identity/authz (ADR-03), events (ADR-04), refs (ADR-13), search (ADR-03/10),
notifications, the agent/trigger fabric (ADR-08), durable timers (ADR-09), storage (ADR-10), and
GDPR/audit (ADR-12). Its differentiator is that **the agent fabric and event bus do the manual
labour Jira leaves to humans** — triage, status hygiene, rollup maintenance, SLA watching, dup
detection — while staying Linear-fast by default. Default language is **Rust** (ADR-02/14); the
flexible-collection definition/view layer and the query AST are **shared crates** (ADR-06/07),
not re-implemented here.

---

## 1. Role & responsibilities — what it OWNS vs delegates

### 1.1 The thesis (ratifies `issue-tracker.md §0`, pressure-tested)

**One issue object + layered optional schemes.** An org starts Linear-simple (sensible default
workflow, no required fields, no SLAs) and turns governance on **incrementally** — adding
workflow schemes, field schemes, permission schemes, SLA policies — *without data migration*.
Governance is **layered configuration over one object graph**, not baked-in schema. This is the
only structure that serves all three audiences as *views and policies over the same objects*
(`issue-tracker.md §4.4`). Phase 4 owns the open fork "how much is baked-in vs opt-in" (PR-3,
ADR-15).

### 1.2 What the Issue tracker OWNS (its core competency — does not delegate)

Per `system-overview.md §4` and `issue-tracker.md §9.3`:

- **The issue object and its flexible field model** — the typed-record instance, its property
  bag, soft-delete + full version/change history.
- **The workflow state machine** — states, transitions, guards/conditions, post-transition
  actions, and the mandated small **fixed set of state *categories*** (`unstarted / started /
  completed / cancelled`) over **unlimited named states**, so cross-project reporting and boards
  work over heterogeneous custom workflows (`issue-tracker.md §3.3`, §12.1).
- **Hierarchy & rollup semantics** — parent/child containment (`Initiative → Epic → Story →
  Sub-task`), and the **incremental, event-driven rollup compute** of progress/estimate/date
  aggregates (the local materialised tree, if §6.3 forces it — see §7).
- **Planning objects & their semantics** — Cycle (time axis) and Project/Epic/Initiative (scope
  axis) as **distinct axes** (an issue can be in cycle *N* and project *X* at once;
  `issue-tracker.md §3.6`); milestones/releases/versions; backlog ordering.
- **The SLA engine** — policies, business-calendar awareness, pause/resume conditions,
  escalation, breach semantics (the *timers* themselves are delegated to ADR-09; the *policy/
  calendar logic* is owned).
- **Tracker analytics definitions** — cycle time, lead time, throughput, CFD, burndown/burnup,
  velocity, SLA compliance, ageing (computed over the OLAP read store; ADR-10).
- **The workflow/field/permission *schemes*** — the configuration objects that reshape the one
  object per type/team/project.
- **Issue-tracker-specific compilers** — compiling the shared query AST (ADR-07) to its OLTP
  store and to the trigger `EventMatcher`; the SLA-timer registration; the import/export mappers.

### 1.3 What it DELEGATES to the shared layer

| Concern | Delegated to | Contract / note |
|---|---|---|
| AuthN, the `Principal`, all authz decisions | **Identity & Access** (ADR-03) | ReBAC `check` per action; `list-objects` for every list/board/search; field/transition/browse/confidential visibility expressed as relations (§6, §8.4 below) |
| Every meaningful state change → event | **Event Bus** (ADR-04, ADR-13) | Transactional-outbox; per-aggregate (per-issue) ordering; at-least-once + idempotent |
| issue↔PR↔commit↔doc↔chat↔run edges, and lateral links | **Reference Graph** (ADR-13) | Refs are *events*; backlinks permission-filtered on read; hierarchy/relations *may* live here or as a local tree (TE-7 — §7) |
| Full-text + custom-field + structured + semantic search | **Search** (ADR-03/10) | Indexed off the bus; **pre-filtered** by `list-objects`, never post-filtered |
| Assignment/mention/SLA/breach notifications | **Notifications** (ADR-12) | Consumes the bus; targeted fan-out for mentions |
| Triage/hygiene/forecast agents + automations | **Agent Fabric** (ADR-08) | One trigger engine; plan-then-apply; mock now, real later by swap |
| SLA timers, cycle-end, recurring issues, retention jobs | **Durable-workflow** (ADR-09) | Millions of durable timers; breach fires events even after restart (SC-11) |
| Attachments, large media, import blobs | **Storage** (ADR-10) | S3-compatible, residency-pinned, crypto-shred-capable |
| Analytics scans over years of history | **OLAP read store** (ADR-10) | CQRS read model fed by the bus; never scan OLTP |
| Erasure/export/rectify of personal data | **GDPR/Audit** (ADR-12) | Implements `PersonalDataHolder`; one tamper-evident audit log |
| The field-definition + view abstraction; the query AST | **`myelin-query`** shared crates (ADR-06/07) | Shared *primitive*, not the execution engine — see §1.4 |

### 1.4 The two shared primitives it consumes (NOT re-implements) — ADR-06/07

The single most important Phase-2 alignment for this subsystem. Per ADR-06 and ADR-07, the
tracker **does not own** the field-definition system, the view abstraction, or the query
language — those are shared crates so Issues and Knowledge author with the same UX and the
design language ships one views component:

- **Field definitions** (`myelin-query`): typed fields (text, number, select/multi, date,
  user/principal, relation, formula/rollup-typing, …) with per-field personal-data
  classification (ADR-12). The tracker *adds field instances and scoping* (global/org/team/type,
  transition-scoped required-ness) but the *type system* is shared.
- **Views** (`myelin-query`): table / board / calendar / timeline as a *query + grouping + sort +
  visible-fields* projection. The tracker supplies tracker-specific view types (Backlog,
  Roadmap, Cycle) **over** the shared abstraction.
- **The single query AST** (`myelin-query`, ADR-07): the canonical filter/selection form for
  **saved views, the CLI, the API, automations (`EventMatcher`), and agent triggers** — one form,
  one validator, permission-aware *by construction* (always composes with `list-objects`). This
  resolves `issue-tracker.md §5.4`'s "design the query language once" and open-question §12.3-#8.

**What stays tracker-owned (NOT shared):** the workflow/SLA/hierarchy/rollup engines, the
**physical storage + query execution** for flexible fields (the "JQL performance trap", TE-17),
and tracker analytics — exactly ADR-06's "share the schema language and the view model, not the
query planner" line. The Issues↔Knowledge boundary is a **named joint Phase-4 design** (ADR-06).

---

## 2. High-level internal structure

Architecture altitude: the major components/services and their responsibilities, not their
internals. The tracker is one subsystem service (a set of crates in the monorepo, ADR-01) that
owns its OLTP store and emits/consumes the envelope.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ISSUE TRACKER SUBSYSTEM (Rust; owns its PG OLTP store; talks only via glue)   │
│                                                                                │
│  ┌─────────────────────┐   ┌──────────────────────┐   ┌───────────────────┐   │
│  │ Issue Core           │   │ Workflow Engine       │   │ Scheme Registry    │  │
│  │ - issue object/CRUD  │   │ - state machine        │  │ - workflow schemes │  │
│  │ - flexible fields     │  │ - transitions+guards   │  │ - field schemes    │  │
│  │ - change-log/history │   │ - state categories     │  │ - permission schemes│ │
│  │ - human keys (TE-14) │   │ - post-txn actions     │  │ - type schemes      │ │
│  └─────────────────────┘   └──────────────────────┘   └───────────────────┘   │
│                                                                                │
│  ┌─────────────────────┐   ┌──────────────────────┐   ┌───────────────────┐   │
│  │ Planning & Hierarchy │   │ SLA / Governance      │   │ Query & Views      │  │
│  │ - cycles/projects/    │  │ - SLA policy+calendar  │  │ - AST compiler→OLTP│  │
│  │   epics/initiatives   │  │ - approvals/required   │  │   (ADR-06/07 consumer)││
│  │ - roadmap/milestones  │  │   fields-on-transition │  │ - saved views       │ │
│  │ - rollup compute      │  │ - registers durable    │  │ - permission-aware  │ │
│  │   (event-driven, §7)  │  │   timers (ADR-09)      │  │   list/board reads  │ │
│  │ - backlog ranking     │  └──────────────────────┘   └───────────────────┘   │
│  └─────────────────────┘                                                       │
│                                                                                │
│  ┌─────────────────────┐   ┌──────────────────────┐   ┌───────────────────┐   │
│  │ Tool/Trigger Surface │   │ Import/Export         │   │ Analytics Projector│  │
│  │ - registers ToolDefs │   │ - Jira/Linear/GH/CSV  │   │ - emits clean change││
│  │   (ADR-08)           │   │ - 2-pass ID remap     │   │   stream → OLAP     │ │
│  │ - default triggers   │   │ - dry-run/reconcile   │   │   (ADR-10, CQRS)    │ │
│  │ - mock agent handlers│   │ - canonical interchange│  │ - report queries    │ │
│  └─────────────────────┘   └──────────────────────┘   └───────────────────┘   │
│                                                                                │
│  ┌──────────────────────────────────────────────────────────────────────────┐ │
│  │ Glue adapters (the shared crates it depends on — ADR-01):                  │ │
│  │  myelin-events · myelin-identity · myelin-refs · myelin-agent ·            │ │
│  │  myelin-gdpr · myelin-content · myelin-query · myelin-tenancy              │ │
│  └──────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────┘
        │ OLTP (PG: issues + JSONB flexible fields + derived projections)  [ADR-10/14]
        │ outbox in same txn → BUS                                          [ADR-04]
        ▼
  shared layer (Id · Bus · Refs · Search · Notif · Agents · Storage · GDPR/Audit · WF · OLAP)
```

**Component responsibilities (high-level):**

1. **Issue Core** — the issue object, flexible-field property bag, the **change-log/version
   history** (every field change recorded — the audit + GDPR basis, `issue-tracker.md §3.9, §8`),
   and **human-readable key allocation** (`ENG-1421`; per-team counter — TE-14, §6.2 deferred to
   P4; default direction: per-team batched/Hi-Lo allocation, gaps tolerated).
2. **Workflow Engine** — the state machine; transitions with guards that may reference
   permissions, field values, **linked-PR/CI status** (the CI gate, §9), and approvals; enforces
   the fixed state-*category* mapping for reporting.
3. **Scheme Registry** — the *layered governance* config: workflow / field / permission / type
   schemes assigned per type per team/project. This is what makes "Linear-simple by default,
   Jira-powerful on demand" one product, not a fork.
4. **Planning & Hierarchy** — cycles, projects, epics, initiatives, roadmaps, milestones; backlog
   fractional ranking (TE-19, §6.4 deferred to P4); **rollup compute driven by bus events**, not
   synchronous writes (§7, ADR-11 §5).
5. **SLA / Governance** — SLA policy + business-calendar logic; **registers durable timers** with
   the ADR-09 substrate; approvals and transition-scoped required fields.
6. **Query & Views** — compiles the shared AST (ADR-07) to the OLTP store; saved views as
   first-class permissioned objects; every list/board read is **permission-filtered via
   `list-objects`** (ADR-03), never post-filtered.
7. **Tool/Trigger Surface** — registers typed `ToolDef`s into the shared `ToolSurface` (ADR-08);
   ships default triggers and **mock agent handlers** (triage, dedup, hygiene) behind the strategy
   pattern.
8. **Import/Export** — Jira/Linear/GitHub/CSV importers; two-pass ID remap; dry-run + reconcile;
   the canonical interchange format that round-trips with portability export (`issue-tracker.md
   §10`).
9. **Analytics Projector** — emits a clean change-event stream and feeds the OLAP read store
   (CQRS, ADR-10) so report queries never touch OLTP (`issue-tracker.md §6.5`).

---

## 3. Technology

Aligned to ADR-02 (Rust default) and ADR-14 (the per-system map, which already lists this
subsystem). No divergence from the Rust default — nothing about the tracker argues for another
backend language (it is transactional OLTP + event processing + query compilation, squarely
Rust's strengths; no soft-real-time fan-out tier like chat's, TE-21).

| Layer | Choice (Phase-2 direction; P4 may refine) | Rationale / citation |
|---|---|---|
| **Backend language** | **Rust** | ADR-02/14 default; strong types for the workflow state machine and the scheme algebra; no divergence justification exists |
| **OLTP store** | **Postgres-class**: relational core + **JSONB (+ GIN) for the flexible-field long tail + generated/derived columns for hot fields** | ADR-10/14; the standard hybrid answer to the JQL performance trap (TE-17 / `issue-tracker.md §6.7`). Distributed-SQL only if a single shard outgrows PG |
| **Analytics / reporting** | **OLAP columnar (ClickHouse-class)** read model, **fed by the bus (CQRS)** | ADR-10; `issue-tracker.md §6.5` — aggregate scans over years/millions would kill OLTP. The state-change event log doubles as the time-in-state source |
| **Search** | Shared **Search** system (Tantivy/OpenSearch/Meilisearch-class) | ADR-10/14; custom-field + full-text + structured, ACL-aware via `list-objects`. The tracker *projects into* it; does not run its own index |
| **Durable timers (SLA/cycle/recurring/retention)** | Shared **durable-workflow** substrate | ADR-09; SC-11 — millions of timers, fire-after-restart |
| **Rich text (descriptions/comments)** | Shared **`myelin-content`** model (ADR-05) | One AST, `mention`/`artifact_ref` as first-class nodes; **single-author-at-a-time** concurrency for issue bodies (NOT the CRDT engine Knowledge uses — ADR-05 keeps concurrency subsystem-owned) |
| **Field defs / views / query AST** | Shared **`myelin-query`** (ADR-06/07) | The consumed primitive; tracker owns execution only |
| **Frontend stack** | Open; set in the design-language deliverable | ADR-02; expected TS/React-class baseline, not mandated here. Linear-class sync-engine UX is a **P4 frontend/sync item** (§6.6 of the deep-dive) |

**Key libraries (directional):** the shared glue crates (ADR-01); a state-machine/workflow
representation (likely a data-driven interpreter, not codegen, so schemes are config); a
fractional-indexing crate for backlog ranking (TE-19); PG access via the platform's standard
data layer.

---

## 4. Views / screens the UI requires

> Feeds the **shared design-language** work and the **Phase-4 design sketches** (VISION §3 — no
> frontend code without a design sketch, incl. empty/loading/error states). This enumerates the
> *required surfaces and their key states*; detailed wireframes are Phase 4. Several reuse the
> **shared views component** (table/board/calendar/timeline, ADR-06) and the **shared editor**
> (ADR-05).

### 4.1 Primary screens

| # | Screen | Purpose | Key states (beyond the standard empty / loading / error) |
|---|---|---|---|
| S1 | **Issue detail** | The canonical artifact view | Rich-text body (shared editor), properties sidebar (type/status/priority/assignee/labels/cycle/project/custom fields), relations & hierarchy panel, **linked PRs/commits/CI runs/docs** (resolved via Refs, per-viewer filtered), activity/comment timeline, sub-issue checklist+progress, SLA timers, **agent-suggested actions** card. States: *confidential/restricted* (redacted for unauthorised — §8.4), *transition-blocked* (guard failed), *SLA breaching/breached*, *agent proposal pending (HITL card)*, *soft-deleted/restorable* |
| S2 | **List view** | Grouped, sortable, filterable, **keyboard-navigable** issue list | *Bulk-select / bulk-edit*, *saved-view-applied*, *permission-filtered (some rows hidden — no leak)*, *optimistic-update-in-flight* |
| S3 | **Board / Kanban** | Columns by state-category or any field; WIP limits; swimlanes | *Live multi-user drag (presence)*, *agent-moved card*, *WIP-limit-exceeded*, *drag-reorder conflict resolving* |
| S4 | **Table / spreadsheet** | Inline-editable cells (the shared Knowledge-db UX) | *Cell-edit*, *validation error on field*, *required-field-missing* |
| S5 | **Timeline / Gantt / Roadmap** | Initiatives/epics/projects over time + dependency overlays | *Rollup progress*, *dependency-blocked highlight*, *date-at-risk (forecast)*, *cross-team initiative spanning teams* |
| S6 | **Backlog** | Ordered, drag-to-prioritise | *Rank-rebalancing*, *concurrent-reorder conflict*, *bulk-move-to-cycle* |
| S7 | **Calendar** | Due dates / cycle boundaries | *Overdue*, *cycle-start/end markers* |
| S8 | **Cycle / Sprint view** | Capacity vs committed; burndown; carry-over on close | *Planning (drag into cycle)*, *over-capacity*, *cycle-close (carryover handling)* |
| S9 | **Triage inbox** | Linear-style incoming bugs/requests with agent assist | *Agent-suggested labels/severity/dup/owning-team*, *duplicate-suspected cluster*, *bulk-triage* |
| S10 | **My Work hub** | Personal cross-subsystem inbox ("my issues / my cycle / my PRs needing review") | Cross-subsystem (depends on Notif + Refs); *nothing-assigned*, *blocked-items*, *overdue* |
| S11 | **Dashboards / Reports** | Configurable widgets: cycle-time, CFD, velocity, SLA gauges, counts | *Computing (OLAP)*, *no-data-yet*, *drill-down*, *export* |
| S12 | **Saved-view manager** | Create/share/permission saved views; choose layout/grouping/sort/columns | *Shared-with-team*, *private*, *layout switch* |
| S13 | **Workflow/scheme editor** | Author workflow / field / permission / type schemes (the governance surface) | *State-machine graph editor*, *guard/condition builder*, *scheme-assignment*, *validation (unreachable state)* |
| S14 | **SLA policy editor** | Define SLA policies, business calendars, pause/escalation rules | *Calendar editor*, *escalation chain*, *breach simulation/preview* |
| S15 | **Team / Project settings** | Membership, keys/prefix, default schemes, automations | *Permission-restricted admin* |
| S16 | **Automation / trigger builder** | Author triggers (EventMatcher + action), see agent vs rule actions | *Dry-run preview*, *agent-handler (mock) selected*, *HITL-gate configured*, *disabled/erroring trigger* |
| S17 | **Import wizard** | Jira/Linear/GH/CSV import with mapping + dry-run | *Mapping preview*, *dry-run reconciliation report (mapped/lossy/dropped)*, *running/resumable*, *partial-failure* |
| S18 | **Audit / change-history** | Per-issue and scoped immutable history | *Agent-vs-human actor distinction*, *export for compliance*, *redacted (post-erasure)* |
| S19 | **Command palette / quick-create** | Keyboard-first create-anywhere, query bar | *Query autocomplete (compiles to AST)*, *create-from-template* |

**Cross-cutting state requirements (all screens):** every list/board/search reflects
**permission-aware reads** (S2/S3/S5/S9 must never render an issue the viewer can't see — ADR-03,
SC-1); every mutating screen supports **optimistic updates** (Linear-class sync expectation,
`issue-tracker.md §5.5` — the sync engine is a P4 frontend item); every artifact reference renders
via the shared unfurl (ADR-05/13), **degrading gracefully** on erasure-tombstone (ADR-12).

---

## 5. CLI commands

> Aligned to `issue-tracker.md §11`. **Design law:** every command is expressible against the
> *same API the UI uses* (no privileged back-channel), respects permissions identically (ADR-03),
> shares the query AST (ADR-07), and supports `--json` for agent/script consumption.
> `myelin issue ...` is the namespace; bulk + scriptable output are first-class (power users +
> agents). The CLI is one of the clients in the layering (`system-overview.md §2`).

```bash
# ── Issues ────────────────────────────────────────────────────────────────────
myelin issue create --type bug --title "Login 500 on SSO" --team ENG \
                    --assignee me --priority high
myelin issue list "state:open assignee:me cycle:current sort:priority"   # query AST
myelin issue show ENG-1421
myelin issue show ENG-1421 --json                                        # agent-friendly
myelin issue update ENG-1421 --state in-progress --priority urgent
myelin issue assign ENG-1421 @alice
myelin issue comment ENG-1421 "Repro on staging, attaching trace"
myelin issue link ENG-1421 blocks ENG-1490                               # lateral relation
myelin issue parent ENG-1421 --set ENG-1000                              # hierarchy
myelin issue transition ENG-1421 "In Review"                            # workflow-aware (guards run)
myelin issue close ENG-1421 --resolution fixed
myelin issue bulk-update "label:flaky state:open" --add-label needs-triage   # bulk by query

# ── Planning ──────────────────────────────────────────────────────────────────
myelin cycle current | list | plan
myelin cycle add ENG-1421 --to current
myelin project create | list | show
myelin epic create | initiative create
myelin roadmap show --team ENG

# ── Views / queries ───────────────────────────────────────────────────────────
myelin view list | show <name>
myelin view save "state:open label:flaky" --name "Flaky bugs" --board
myelin query "blocked-by:open AND cycle:current"

# ── Governance / reporting ────────────────────────────────────────────────────
myelin sla status ENG-1421
myelin report cycle-time --team ENG --since 90d        # runs against the OLAP read store
myelin audit ENG-1421                                  # change history (agent vs human actors)
myelin export --team ENG --format canonical            # portability / GDPR (round-trips import)

# ── Import ────────────────────────────────────────────────────────────────────
myelin import jira --url https://co.atlassian.net --project FOO --dry-run
myelin import linear --token ... --team ENG
myelin import status <job-id>

# ── Automation / agents (strategy-pattern handlers; mock now, real later) ──────
myelin trigger list | create --on issue.created --when "type:bug" --do triage-agent
myelin agent dry-run triage-agent --on ENG-1421        # deterministic mock during dev
```

---

## 6. Usage examples (end-to-end)

### 6.1 Engineer loop — branch-named-for-issue auto-transitions (UI + git + CI)

**UI/CLI flow:** Engineer runs `myelin issue create --type bug --title "..." --assignee me`,
gets `ENG-1421`. They create a branch `feature/ENG-1421-fix-sso` in Git hosting.

1. **Git** emits `git.branch.created` (envelope, ADR-04/13) referencing `ENG-1421`.
2. The tracker **consumes** it (idempotent on `event_id`), creates the **ref edge** issue↔branch
   (`ref.create` → Refs, ADR-13), and — workflow-permitting — auto-transitions `ENG-1421` to
   *In Progress* (`issue-tracker.md §4.1`).
3. The engineer opens a PR "Closes ENG-1421"; **Git** emits `git.pr.opened`; the tracker links
   PR↔issue. **CI** runs; the workflow guard "can't mark Done while CI red on the linked PR"
   reads CI status via the Git↔CI checks contract (the gate is opt-in per workflow).
4. PR merges → `git.pr.merged` → the tracker transitions `ENG-1421` to *Done* (guard satisfied).
5. Each transition emits `issue.state_changed` (with from/to + **category**), feeding the OLAP
   read store (cycle-time metric) and Notifications (assignee/watchers).

The PR view, conversely, surfaces `ENG-1421` inline via the **PR context pane** (`system-overview
§8.1`) — the tracker exposes a per-viewer **projection** of the issue via its `ArtifactRef`; Git
never reads the tracker's DB.

### 6.2 Agent-native triage (the differentiator) — mock now, real later

**Trigger:** a new bug lands in Triage. A registered trigger `T = (EventMatcher: type:bug ∧
state:triage) → triage-agent` (ADR-08, one trigger engine).

1. Tracker emits `issue.created`; the **bus** matches `T`, wakes `MockTriageAgent` (a
   strategy-pattern `Agent`) on-behalf-of the reporter under a `RunBudget`.
2. The agent runs `handle(event, ctx) → AgentDecision { effects: [issue.update(labels,
   severity), ref.create(duplicate-of?), issue.comment("suspected dup of ENG-1390")] }` — it
   **proposes**, performs no side effects (plan-then-apply, ADR-08).
3. The **`EffectApi`** validates each effect against *permissions ∩ delegation ∩ tenant policy*
   (ADR-03), then applies via **the same permissioned tools a human uses** (`issue-tracker.md
   §7.2, §8.7`). Label/comment are non-sensitive → applied; a *transition* on a governed workflow
   may be **HITL-gated** → a durable wait (ADR-09) surfaces a chat approval card.
4. Applied effects emit more events (audited, attributed to the agent, one `correlation_id`; loop
   depth capped). The triage screen (S9) shows the suggestions.

The **identical mock code** runs deterministically in tests (`cargo-mutants` over
event→trigger→effect→event) and an `LlmAgentRuntime` later with **zero tracker changes** — the
strategy-pattern payoff (ADR-08).

### 6.3 SLA breach → escalation (durable timers)

A support issue under an SLA policy: on creation the SLA engine **registers a durable timer**
(ADR-09) for time-to-first-response with the business calendar. At 80% the timer fires
`sla.at_risk` → a trigger drafts a holding response (agent) / notifies on-call. On breach,
`sla.breached` fires **even after a restart** (SC-11) → escalation chain via Notifications. Pause
conditions (e.g. *waiting on customer*) suspend the timer. All breach events feed OLAP for
SLA-compliance reporting.

### 6.4 PM forecast drift (cross-subsystem agent)

Rollup recompute (§7) updates an initiative's progress/date projection off child-issue events.
When the forecast crosses an at-risk threshold, `initiative.health_changed` fires → an agent
flags the at-risk initiative to the PM in chat with the contributing blocked issues
(`issue-tracker.md §7.3`). The roadmap screen (S5) shows the date-at-risk state.

---

## 7. How rollups & cross-cutting reads work at scale (the structural choices)

Two world-scale hot-spots the tracker owns (`issue-tracker.md §6.3, §6.1`), resolved here at
**direction** level (mechanism → P4):

- **Hierarchy rollups are event-driven and async** (ADR-11 §5): a child change emits an event;
  the rollup component **debounces and incrementally recomputes** the affected ancestors off the
  bus, *not* synchronously in the write path. Whether the hierarchy/relations live as edge types
  **in the Reference Graph** or as a **tracker-local materialised tree** projected into Refs is
  **[OPEN → P3/P4]** (TE-7, ADR-13) — world-scale rollups may force a local tree for compute
  locality; the *contract* (refs are events, backlinks permission-filtered) holds either way.
- **Cross-cutting reads (portfolio/roadmap/analytics) cut across tenant-sharded OLTP**
  (`issue-tracker.md §6.1`). Resolution direction: **OLTP sharded by tenant + the OLAP read
  store** (ADR-10/11) for the aggregate-hungry queries; the tracker emits a clean change stream
  to feed it.

---

## 8. Interactions — events, refs, authz, search, notifications, agents, GDPR

### 8.1 Events EMITTED (envelope per ADR-13; exact dotted names → P3)

Lifecycle (`issue.created/updated[field deltas]/deleted/restored/state_changed[from,to,
category]/assigned/priority_changed/type_changed`); hierarchy/relations (`issue.linked/unlinked`,
`issue.parent_changed`, `issue.rollup_recomputed`); comments/collab (`comment.*`,
`mention.created`, `subscription.changed`); planning (`cycle.started/completed`,
`issue.added_to_cycle`, `project.created`, `initiative.health_changed`, `milestone.released`);
governance (`sla.started/paused/breached/met`, `approval.*`, `workflow.transition_blocked`);
agent hooks (`issue.triaged/duplicate_suspected/labelled_by_agent`); and a **`ref.created`** for
every edge to a PR/commit/doc/chat/run so Refs stays consistent (`issue-tracker.md §7.1`). All via
**transactional outbox** in the same DB txn (ADR-04), per-issue ordering, references-not-payloads
(personal data lives in the erasable store, not the event — ADR-12).

### 8.2 Events CONSUMED

From **Git** (branch named-for-issue, commit/PR referencing issue, merge → auto-transition/link);
from **CI** (pipeline pass/fail/deploy → update/gate transition, auto-create incident on prod
failure); from **Chat** (create-issue-from-message, references → edge); from **Knowledge** (spec/
doc linked/updated → reflect; doc deleted → relation cleanup); from **Identity** (user
deactivated/erased → reassign/anonymise, §8.6 below); from **durable-workflow** (SLA fired,
cycle-end, recurring-issue due); from **Agent Fabric** (agent-proposed effects, applied via the
same permissioned API — ADR-08). Every consumer is **idempotent on `event_id`** (ADR-04).

### 8.3 Authz needs it expresses to Identity (ADR-03)

The tracker is one of the reasons the platform chose **ReBAC** (`issue-tracker.md §9.1`,
ADR-03 context). It expresses, as relationship tuples / RBAC-compiled-to-tuples:
- **Browse visibility** (some issues invisible to some users), **field-level** and
  **transition-level** permissions, and **confidential/security issues** visible only to a
  security team.
- **`list-objects`** is the load-bearing primitive: every list/board/search/backlog is
  **pre-filtered**, never post-filtered (ADR-03, SC-1) — and **confidential issues must not leak
  via a chat/doc backlink** (the Refs read filter, §8.4 of the deep-dive). Attribute predicates
  ("field visible only if severity < X") ride ABAC-at-the-edge (ADR-03), off the hot path.
- Agents are **principals** authorized by the same engine (ADR-08).

### 8.4 Search, Notifications, Agents, Refs

- **Search:** projects issues + custom fields into the shared index off the bus; queries compile
  the shared AST to the search backend and **always compose with `list-objects`** (ADR-03/07).
- **Notifications:** assignment/mention/SLA/breach → the one prioritised cross-subsystem inbox
  (ADR-12); mentions use targeted write-fanout.
- **Agent Fabric:** registers typed **`ToolDef`s** (create/update/transition/link/comment/
  estimate, each with required caps + side-effecting flag) into the shared `ToolSurface` (ADR-08),
  consumed by our runtimes now and exposable over **MCP** later — defined once, governed once.
  Ships default **triggers + mock agent handlers** (triage, dedup, hygiene, SLA-draft, rollup-
  drift). Automations and agents are **one trigger engine** (ADR-08).
- **Reference Graph:** the issue↔PR↔commit↔doc↔chat↔run edges + lateral links; refs are emitted
  *from* `mention`/`artifact_ref` content nodes (ADR-05) and explicit links; backlinks
  permission-filtered on read.

### 8.5 PersonalDataHolder duties (ADR-12)

The tracker is **dense with personal data** (creators, assignees, mentions, commenters,
watchers, worklog authors, audit actors, **free-text PII** in titles/descriptions/comments/custom
fields) and has the audit-immutability-vs-erasure tension (`issue-tracker.md §8`). It implements
`PersonalDataHolder` (`locate / export / rectify / restrict / erase`):
- **Erasure = anonymise/pseudonymise the actor** ("Former user 8a2f") rather than destroy issues
  others legitimately own; propagate to history, comments, mentions, and the ref graph. The
  erasable pseudonym mapping lives **outside** immutable structures (ADR-12 §4) so destroying it +
  crypto-shred satisfies erasure without rewriting history.
- **Free-text PII** is the hardest case (can't be found by FK) — direction: agent-assisted scan +
  redaction-with-tombstone + crypto-shred of attachments; **residual risk documented honestly**
  ([OPEN — LEGAL] GD-6).
- **Field-level sensitivity classification** for worklogs/productivity metrics (works-council/
  labour-law constraints, GD-13) and special-category incident data — the field-def primitive
  carries per-field personal-data classification (ADR-06/12).
- **Audit:** every human *and agent* action recorded in the one tamper-evident audit log (ADR-12);
  agent actor + strategy identified (`issue-tracker.md §8.7`). Audit retention carve-out vs erasure
  is [OPEN — LEGAL] (GD-5).
- **Retention/auto-archival** per org (e.g. delete resolved support issues after N years) via the
  durable scheduler (ADR-09), interacting carefully with audit retention.
- **Residency:** issues + attachments pinned to the cell's region (ADR-11); no cross-region hot
  paths.

---

## 9. The two tightest seams it participates in

- **Issues ↔ Knowledge (ADR-06)** — *the biggest reuse decision in the platform.* Both implement
  the **shared field-definition + view + query-AST primitive**; each owns its **storage +
  execution** (the tracker: JSONB + derived projection + workflow/SLA/rollup engines; Knowledge:
  the formula/rollup dataflow engine + collab). This is a **named joint Phase-4 design**. The
  shared editor (ADR-05) renders issue descriptions/comments; the shared views component renders
  table/board/calendar/timeline for both.
- **Issues ↔ Git/CI** — the engineer loop (§6.1): branch/PR/commit linkage, **CI status gating
  transitions** (opt-in per workflow), auto-incident on prod failure. The Git↔CI checks contract
  is the platform's most load-bearing seam (`system-overview.md §4`); the tracker consumes its
  signals to drive transitions.

---

## 10. Changes implied in the shared systems (flag for Phase 3)

These are needs the tracker imposes on the shared layer; none contradict the spine, but Phase 3
must size them:

1. **Identity/Authz (ADR-03):** the ReBAC tuple schema must natively express **field-level**,
   **transition-level**, **browse**, and **confidential-issue** visibility, and `list-objects`
   must be efficient enough for large permission-filtered boards/backlogs with custom-field
   predicates (TE-2, §6.8 of the deep-dive). Confirm ABAC-at-the-edge covers "field visible if
   severity < X" without hitting the hot path.
2. **Event taxonomy (ADR-13, TE-10):** the canonical dotted names for the §8.1 event set, incl.
   **field-level deltas** in `issue.updated` (consumers need per-field granularity for hygiene/
   metrics/audit).
3. **Durable-workflow / timers (ADR-09):** must scale to **millions of SLA timers** with
   business-calendar pause/resume and exact-time breach firing after restart (SC-11) — confirm the
   chosen substrate supports business-calendar-aware durable timers, or the tracker layers calendar
   logic over plain timers.
4. **Reference Graph (ADR-13, TE-7):** decide whether **hierarchy + lateral relations** are
   first-class edge types *in* Refs or a tracker-local materialised tree projected into Refs —
   driven by world-scale **rollup** compute locality (§6.3). Affects rollup performance directly.
5. **OLAP read store (ADR-10):** the CQRS analytics model must ingest the tracker's clean change
   stream and support the time-in-state / CFD / velocity / SLA-compliance query shapes.
6. **Query AST (ADR-07, TE-6):** the grammar must support **relation traversal** ("issues
   blocking anything in cycle N") and **date math**, and remain safe/declarative for the
   `EventMatcher` trigger path.
7. **EventMatcher predicate language (AG-7):** the trigger filter must express the §6.2 triage/
   hygiene conditions and share the AST's safe-evaluation core.

---

## 11. Open questions for Phase 4 (detailed architecture)

Carried forward from `issue-tracker.md §12.3` and the spine's [OPEN → P4] backlog (ADR-15),
ranked by impact:

1. **Is "Epic/Initiative" a *type* or a *hierarchy level*?** — pivotal modelling fork
   (`issue-tracker.md §3.2/§3.5`; PR-2). Affects schemes, rollups, and the roadmap model.
2. **How much governance is baked-in vs opt-in scheme?** — the "Linear-fast by default,
   Jira-powerful on demand without a fork" decision (§1.1; PR-3).
3. **Flexible-field storage + query-execution model** — the JQL performance trap (TE-17): JSONB +
   GIN + derived columns + search-index projection; the dominant per-subsystem risk, *not* solved
   by ADR-06's sharing.
4. **Does Refs own hierarchy/relations, or does the tracker keep a local materialised tree for
   rollup performance?** (TE-7, §6.3, §7 above).
5. **Permission granularity actually required** (field/transition/browse/confidential) and can
   Identity express it at scale (§6.8/§8.4 of the deep-dive; co-design with Phase-3 authz).
6. **Human-readable monotonic keys at scale** (TE-14) — per-team counter; gapless vs gap-tolerant
   (users *perceive* gaps as bugs but gaplessness is a distributed-contention hazard).
7. **Drag-to-reorder ranking at scale** (TE-19) — fractional indexing + concurrent-reorder
   conflict story (humans **and** agents reordering).
8. **Rollup/forecast engine** — incremental, debounced, cycle-detecting recompute; Monte-Carlo
   forecasting as an agent-powered differentiator (`issue-tracker.md §4.2`).
9. **SLA business-calendar engine** — build vs durable-scheduler-provides-primitives (§6.9; ties
   to ADR-09 shape).
10. **Real-time sync engine** for Linear-class optimistic UX (§6.6) — cross-cutting with the
    frontend + Knowledge collab; per-entity subscriptions over the bus + client cache protocol.
11. **Import fidelity** (Jira ADF → shared content; JQL → AST; permission schemes → ReBAC tuples;
    user matching/merging; history depth) and **round-trip with portability export** (PR-8, §10 of
    the deep-dive).
12. **Free-text PII erasure completeness** and the documented residual risk ([OPEN — LEGAL] GD-6);
    crypto-erasure vs anonymisation as the mechanism (`issue-tracker.md §8.2`).
13. **Offline / local-first** in scope for v1? (`issue-tracker.md §5.5/§12.3-#9`).

---

## 12. Cross-references

- [`VISION.md`](../../../VISION.md) — non-negotiables (world-scale, top-tier UX, agent-native,
  GDPR/EU-sovereign, Rust-default).
- [`../architecture-decisions.md`](../architecture-decisions.md) — ADR-02 (Rust), ADR-03 (ReBAC),
  ADR-04 (bus), ADR-05 (content), **ADR-06 (db/views primitive)**, **ADR-07 (query AST)**, ADR-08
  (agents), ADR-09 (durable workflow/timers), ADR-10 (datastores), ADR-11 (cells), ADR-12 (GDPR),
  ADR-13 (glue contracts), ADR-14 (per-system tech map — this subsystem's row).
- [`../system-overview.md`](../system-overview.md) — §4 (owns vs delegates), §8.1 (PR context
  pane), §8.2 (agent-native flagship), §8.3 (DSAR fan-out).
- [`../../01-research/subsystem-deep-dives/issue-tracker.md`](../../01-research/subsystem-deep-dives/issue-tracker.md)
  — the Phase-1 territory map this architecture builds on.
- [`../../01-research/technical-structuring.md`](../../01-research/technical-structuring.md) — §3
  (glue), §4.1/§4.2 (owns/delegates + Issues↔Knowledge seam), §5 (cells).
