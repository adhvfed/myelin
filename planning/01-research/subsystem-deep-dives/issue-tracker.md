# Subsystem Deep-Dive: Issue Tracker

> Phase 1 research. This document maps the territory for the issue-tracker subsystem.
> It is **research, not architecture** — it identifies concepts, constraints, hard
> problems and open questions that the architecture phases (`02`–`05`) will resolve.
> Canonical brief: `../../../VISION.md`. Cross-subsystem glue is owned by the shared-systems
> research; here we flag the dependencies and defer the design.

## 0. TL;DR / orientation

The issue tracker is the **planning and accountability spine** of Myelin. It is the one
subsystem that simultaneously serves three audiences with genuinely different mental models:

- **Engineers** want a fast, keyboard-driven, git-adjacent task list tightly coupled to
  PRs, commits, CI and branches (Linear / GitHub Issues mental model).
- **Product managers** want roadmaps, initiatives, cycles/sprints, prioritisation,
  capacity and "are we going to hit the date" views (Linear roadmaps / Jira Advanced
  Roadmaps / Productboard mental model).
- **Corporations** want configurable workflows, SLAs, granular permissions, audit trails,
  portfolio rollups and reporting/analytics for governance (Jira Data Center / Jira Align
  / ServiceNow mental model).

The central design tension is that these three want *the same underlying objects to behave
differently*. The research conclusion is that Myelin should ship **one issue object with a
flexible, partly schema-on-read field/relation model**, and let "views" + "workflow
schemes" + "permission schemes" reshape it per audience — rather than three separate
products bolted together. The risk is becoming "Jira: infinitely configurable and therefore
slow and ugly". Myelin's differentiator must be that the **agent fabric and event bus do
the work that humans do manually in Jira** (triage, status hygiene, rollup maintenance,
SLA watching, duplicate detection), and that the default UX is Linear-fast even though the
governance machinery exists underneath.

Uncertainty is flagged inline with **[ASSUMPTION]**, **[UNCERTAIN]**, **[DEFERRED]**.

---

## 1. Purpose & role in Myelin

### 1.1 What it is for
The issue tracker is the system of record for **units of intended or in-progress work** and
the structures that organise that work over time. "Work" spans bug reports, feature
requests, tasks, sub-tasks, epics, initiatives, incidents, support/customer requests,
chores, spikes, and corporate-governance items (risks, compliance tasks, OKR key results
where the org chooses to model them here).

### 1.2 Its role relative to the other four subsystems
- **Git hosting** — issues link to/from branches, commits, PRs/MRs. "Fixes #123" auto-closes;
  a PR shows the issues it resolves; an issue shows the code that implemented it. This is the
  engineer's primary loop.
- **CI** — issues can be created/updated from CI events (flaky test → issue; failed deploy →
  incident). CI status can gate issue transitions ("can't mark Done while CI red on the
  linked PR"). [ASSUMPTION] gating is opt-in per-workflow.
- **Knowledge platform** — specs/PRDs/runbooks live as knowledge docs; an issue references its
  spec; an epic references its initiative brief; post-incident reviews are docs linked from
  the incident issue. The Notion-class "database" feature overlaps conceptually with issue
  custom fields — see §3.7 (boundary question).
- **Chat** — issues are referenced in channels; a chat message can spawn an issue; an issue's
  activity can post to a channel; agents and humans discuss an issue in-thread. Notifications
  (mentions, assignments) flow through the shared notification system into chat.

### 1.3 Differentiators we are explicitly pursuing
1. **Agent-native triage and hygiene** — first-class triggers, not bots scraping a REST API.
2. **One graph** — issue ↔ code ↔ doc ↔ CI ↔ chat references are first-class edges in the
   shared cross-artifact reference graph, queryable in any direction.
3. **Linear-class speed with Jira-class governance** — fast by default, powerful when
   configured, without two separate products.
4. **GDPR/EU-sovereign by construction** — see §8.

---

## 2. Competitive landscape (for positioning; verify before quoting externally)

Primarily from domain knowledge; **[UNCERTAIN]** on exact current feature flags and pricing —
flagged where it matters. Architecture phases should re-verify if a decision hinges on it.

| Product | Strengths to learn from | Weaknesses / gaps to beat |
|---|---|---|
| **Linear** | Speed, keyboard-first UX, opinionated cycles, clean issue model, good git integration, "triage" inbox, Projects/Initiatives, sub-issues, built-in SLA-ish "SLAs" recently. | Opinionated → limited custom workflows; weaker enterprise governance, portfolio depth, fine-grained permissions; closed-source SaaS (US). |
| **Jira (Cloud / Data Center)** | Custom workflows, fields, screens, permission schemes, JQL, Advanced Roadmaps, automation rules, enormous ecosystem, audit. | Slow, heavy UX; configuration sprawl; cross-project reporting historically weak; US/Atlassian-hosted cloud (sovereignty concern). |
| **Jira Align / Productboard / Aha!** | Portfolio/SAFe rollups, strategy→delivery linkage, capacity planning. | Heavyweight, separate product, expensive, disconnected from the dev loop. |
| **GitHub Issues / Projects** | Git-native, Markdown, simple, Projects v2 has custom fields + views + basic automation, good API/GraphQL. | Weak hierarchy/portfolio, limited SLAs/workflow config, US-hosted. |
| **GitLab Issues/Epics** | Integrated DevOps (closest structural analogue to Myelin), epics, iterations, scoped labels, boards. | UX heaviness; epics historically limited depth; analytics maturity. |
| **YouTrack (JetBrains)** | Powerful query language, agile boards, workflow scripting, time tracking; EU vendor option. | Smaller ecosystem; niche. |
| **Plane / OpenProject / Redmine / Taiga** | Open-source, self-hostable (sovereignty story). | Scale, polish, agent-nativeness all weaker. |
| **ServiceNow / Zendesk** | SLA engine, ITSM workflows, CSAT, queues. | Not dev-centric; the "corporate" muscle we must partially replicate for support/SLA cases. |

**Positioning takeaway:** Myelin must beat Linear on governance/portfolio and beat Jira on
speed/UX/agent-nativeness, while being the only one of these that is EU-sovereign *and*
unifies tracker + git + CI + docs + chat under one identity/event model.

---

## 3. Core domain concepts & data-model considerations

> This is *modelling research*, not the final schema. It enumerates what must be
> representable and flags the tension points. Storage tech is **[DEFERRED]** to architecture.

### 3.1 The Issue (core object)
Minimum first-class attributes (the "spine"):
- Stable identifier: opaque internal ID **plus** a human-facing key (`ENG-1421`). Per-team
  prefix + monotonic counter is the expected pattern. **[UNCERTAIN]/hard**: monotonic
  per-team counters at world scale — see §6.2.
- Title, description (rich text / Markdown; reuse knowledge-platform rich-text model or a
  subset — boundary in §3.7).
- Type (see §3.2), Status/State (see §3.3), Priority, assignee(s), reporter/creator,
  subscribers/watchers, labels/tags, team/project ownership, timestamps (created, updated,
  state-changed, due, started, completed), estimate(s), and a flexible custom-field bag.
- Soft-delete + full audit/version history (corporate + GDPR requirements, §8).

### 3.2 Issue types
- **Built-in / default**: Bug, Feature, Task, Sub-task, Epic, Initiative, Incident,
  Support request, Chore/Spike. **[ASSUMPTION]** ship a sensible default set; allow
  org-defined custom types.
- **Custom types** per org/team with their own field set, workflow, icon, and hierarchy
  rules. This implies **types are configuration, not enum** — a "type scheme" object.
- Open question: is "Epic" / "Initiative" a *type* or a *level in a hierarchy*? Different
  tools choose differently (Jira = type; Linear = separate Project/Initiative objects).
  See §3.5. **[DEFERRED]** but called out as a pivotal modelling fork.

### 3.3 States / workflow
- An issue's status belongs to a **workflow** (state machine): states + allowed transitions
  + optional guards/conditions + optional post-transition actions.
- States map to **categories** (e.g. `unstarted / started / completed / cancelled`) so that
  boards, metrics, and "is this done?" logic work across heterogeneous custom workflows.
  This category abstraction is what makes cross-project reporting tractable (Jira's lack of
  it historically hurt; Linear's fixed categories help). **Strong recommendation to research:
  mandate a small fixed set of state *categories* while allowing unlimited named states.**
- Workflows are assigned per type per team/project (a "workflow scheme").
- Transition guards may reference: permissions, custom-field values, linked-PR/CI status,
  approvals. Guards/actions are where the **agent fabric and event triggers** plug in (§7).

### 3.4 Custom fields
- Field types: text, number, select (single/multi), date, datetime, user, url, checkbox,
  duration/estimate, currency, relation-to-issue, relation-to-other-artifact (doc, repo,
  PR), formula/rollup (computed), and possibly "agent-maintained" fields (value written by
  an agent strategy). 
- **Schema-on-read vs schema-on-write** tension: Jira-style rigid screens/schemes give
  governance but are heavy; Notion/Linear-style flexible fields are fast but harder to
  report on and validate. Research recommendation: **typed field definitions at the
  project/org level (so reporting and validation work) + permissive UX**. The knowledge
  platform's "database" columns are the same problem — **share the field-definition
  primitive across both subsystems if feasible** (see §3.7, §9).
- Field scoping: global, per-org, per-team, per-type. Required-ness can be transition-scoped
  ("Resolution required to close").
- **Custom fields are the #1 source of write-amplification, index bloat, and migration pain
  at scale** — flagged for §6.

### 3.5 Relations & hierarchies
Two distinct mechanisms that are often conflated:

1. **Hierarchy (containment / rollup)** — parent/child forming a tree (or constrained DAG):
   `Initiative → Epic → Story → Sub-task`. Used for rollups (progress, estimates, dates).
   Hard questions: fixed levels vs arbitrary depth; single-parent (tree) vs multi-parent
   (DAG); cross-team hierarchies; rollup recomputation at scale (§6.3).
2. **Relations / links (lateral)** — `blocks / blocked-by`, `duplicates / duplicated-by`,
   `relates-to`, `causes`, `clones`, `depends-on`. These form a general graph and feed
   dependency analysis, critical-path, and "what's blocking the release" views.

Both should be edges in the **shared reference graph** rather than tracker-private tables, so
that chat/docs/CI can participate and so cross-subsystem queries work. **[DEFERRED]**: whether
hierarchy is a specialised edge type in the shared graph or a tracker-local optimisation with
a projection into the graph. World-scale rollups (§6.3) may force a local materialised tree.

### 3.6 Epics / Initiatives / Roadmap objects (PM layer)
- **Project / Cycle (Sprint) / Iteration**: a time-boxed or scope-boxed container of issues.
  Cycles have start/end, capacity, and a clear "what's in this cycle" membership.
- **Epic**: a larger body of work spanning cycles; rolls up child issues.
- **Initiative / Theme / Objective**: strategic grouping of epics/projects; the portfolio
  rung; often tied to OKRs and target quarters/dates.
- **Milestone / Release / Version**: a target that issues are "fixed in / targeted for";
  bridges tracker and git-hosting (tags/releases) and CI (deploys).
- Research note: Linear separates Project and Cycle cleanly; Jira overloads "Epic". Myelin
  should keep **Cycle (time) and Project/Epic (scope)** as distinct axes — an issue can be
  in cycle *N* and project *X* simultaneously.

### 3.7 Boundary with the Knowledge platform "database"
The knowledge platform offers Notion-class **databases** (rows + typed columns + views). An
issue tracker is, abstractly, a specialised database with a workflow engine, hierarchy,
git/CI links, and SLAs. **Key research question for architecture:** do we
- (a) build the tracker on top of the shared database/field primitive (DRY, but risk the
  tracker can't get the specialised performance/semantics it needs), or
- (b) keep them separate domains that *share the field-definition and view primitives* only?

Recommendation to investigate (a)-leaning-(b): **share the field-type and view query
primitives; keep the issue lifecycle/workflow/hierarchy/SLA engine tracker-specific.** This
is a cross-subsystem decision — coordinate with the knowledge-platform deep-dive. **[DEFERRED]**.

### 3.8 Multi-tenancy & ownership model
- Org → Team/Project → Issue is the likely ownership spine, but corporations need
  cross-team portfolio structures that don't fit a strict tree. Modelling "an issue belongs
  to one team but rolls up to an initiative spanning many teams" is core (§3.5).
- Tenant isolation is a shared-systems concern but **the tracker's query patterns
  (cross-project boards, portfolio rollups) cut across the natural isolation boundary** —
  this is a notable tension for the sharding/isolation strategy (§6).

### 3.9 Other modelled concepts
- **Comments / activity / change-log** (every field change recorded; basis of audit + GDPR).
- **Attachments** (delegate to shared storage; GDPR-relevant blobs).
- **@mentions & subscriptions** (drive notifications).
- **Estimates / story points / time-tracking** (worklogs — relevant to corporate billing &
  capacity; also GDPR-sensitive as they reveal individual productivity — §8.5).
- **SLAs / SLOs** (timers, pause conditions, breach events — §4.3).
- **Saved views / queries / boards** (§5.4).
- **Templates** (issue templates, project templates, workflow templates).
- **Automations/rules** (§7).

---

## 4. The three workflows (engineer / PM / corporate)

### 4.1 Engineer workflow (git/CI-coupled)
- Create issue fast (CLI, keyboard, from chat, from a failing CI run, from a code comment).
- Self-assign, set estimate, move to "In Progress" — ideally auto-triggered by **branch
  creation** named for the issue (`feature/ENG-1421-...`) or by opening a PR.
- PR/commit linkage: `ENG-1421` in branch/commit/PR title or body creates the edge; merging
  a PR with "Closes ENG-1421" transitions the issue (workflow-permitting). CI status surfaces
  on the issue.
- "My issues", "my cycle", "my PRs needing review" unified inbox (cross-subsystem; depends on
  shared notifications + reference graph).
- Triage inbox for incoming bugs/requests (Linear-style) with agent-assisted dedup/labelling.
- Keyboard-driven everything; offline-tolerant create [UNCERTAIN — whether offline is in scope].

### 4.2 PM workflow (planning/prioritisation)
- **Roadmap views**: timeline/Gantt of initiatives/epics/projects with target dates and
  rollup progress; dependency overlays.
- **Cycle/sprint planning**: capacity vs committed estimate; backlog grooming; drag issues
  into cycles; carry-over handling on cycle close.
- **Prioritisation**: ordered backlog (lexorank-style ordering — §6.4), priority fields,
  scoring frameworks (RICE/WSJF/value-effort) ideally as computed fields.
- **Forecasting**: "will this initiative land by Q3?" from velocity + remaining scope
  (Monte-Carlo-ish). **[DEFERRED]** but a known PM differentiator; agents could power it.
- **Portfolio rollup**: progress/health/risk across teams (overlaps corporate §4.3).
- Triage → backlog → cycle → done lifecycle ownership.

### 4.3 Corporate / governance workflow
- **Custom workflows** per process (change management, incident, compliance review,
  procurement). State machines with approvals and required fields.
- **SLAs**: time-to-first-response, time-to-resolution, with business-calendar awareness,
  pause/resume conditions, escalation on breach. Needs a reliable **timer/scheduler**
  (shared system) emitting breach events. ServiceNow/Jira-SM are the reference.
- **Permissions**: field-level, transition-level, project-level, and "browse" visibility
  (some issues invisible to some users) — delegated to the shared identity/access system but
  the tracker must express its permission *needs* (see §9). Confidential/security issues are
  a hard sub-case (visible only to a security team) and interact with the reference graph
  (a confidential issue must not leak via a chat/doc backlink — §8.4).
- **Audit**: immutable, queryable history of who changed what, when, why; export for
  compliance. Overlaps GDPR §8.6.
- **Reporting & analytics**: cycle time, lead time, throughput, CFD, burndown/burnup,
  velocity, SLA compliance, ageing, created-vs-resolved, custom pivot reports. These are
  **heavy aggregate queries over large datasets** → likely a separate analytics/read store
  (§6.5).
- **Portfolio views**: executive rollups across the whole org; OKR linkage; risk registers.

### 4.4 The unifying insight
All three are *views and policies over one object graph*. The architecture must let an org
turn governance on **incrementally** (start Linear-simple, add workflow/SLA/permission
schemes as they grow) without data migration. This argues for governance as **layered
optional schemes** rather than baked-in schema.

---

## 5. Key UX / views required

> UX is a first-class requirement (VISION §3). Detailed design is for later phases; here we
> enumerate the *required surfaces*.

### 5.1 Issue detail view
Rich-text body, properties sidebar (type/status/priority/assignee/labels/cycle/project/custom
fields), relations & hierarchy panel, linked PRs/commits/CI runs/docs, activity/comment
timeline, sub-issue checklist with progress, SLA timers, agent-suggested actions.

### 5.2 List / board / table views
- **List** (grouped, sortable, filterable, keyboard-navigable).
- **Board / Kanban** (columns by state-category or any field; WIP limits; swimlanes).
- **Table / spreadsheet** (inline-editable cells; shared with knowledge-platform db UX).
- **Timeline / Gantt / roadmap** (initiatives/epics over time, dependencies).
- **Backlog** (ordered, drag-to-prioritise).
- **Calendar** (due dates / cycles).
- All driven by the same saved-query engine (§5.4).

### 5.3 Aggregate / planning surfaces
Cycle view (capacity, burndown), roadmap/portfolio view, dashboards (configurable widgets:
charts, counts, SLA gauges), triage inbox, personal "My Work" hub, team page.

### 5.4 Saved views & queries
- A **query language** (Linear-filter / JQL / GitHub-search analogue) is required: filter by
  any field, relation traversal ("issues blocking anything in cycle N"), full-text, date math,
  "is empty", user/me, scoped labels.
- Saved views are first-class objects: shareable, permissioned, per-user or team-shared,
  with chosen layout (list/board/table/timeline), grouping, sorting, and column set.
- Research note: design the query language **once** to serve UI, CLI, API, automations,
  and agent triggers (§7). It must be both human-writable (a search bar) and
  machine-constructable (a structured AST). Strong recommendation: **structured filter AST as
  the canonical form, with a human text syntax compiling to it** — avoids JQL's
  parser-driven footguns while staying expressive. **[UNCERTAIN]** how much full-text vs
  structured to expose; depends on the shared search system's capabilities.

### 5.5 Performance UX expectations
Linear set the bar: optimistic local updates, instant filtering, sync engine, offline-ish
feel. This has deep architectural implications (§6.6) and should be flagged to the
frontend/sync architecture phase early.

---

## 6. Hardest technical problems for WORLD-SCALE

> "World-scale means world-scale" (VISION §3). The tracker is read-heavy, query-diverse, and
> aggregate-hungry. The hard problems:

### 6.1 Multi-tenancy + cross-cutting queries
Natural isolation is per-org (and per-team within), which favours sharding by tenant. But
portfolio/roadmap/analytics queries and cross-team initiatives **cut across** shards. Tension
between strict tenant isolation (good for GDPR/residency/perf) and cross-cutting reads. Likely
resolution direction: **OLTP sharded by tenant + a separate read/analytics store** (§6.5).
**[DEFERRED]** to shared-systems + tracker architecture.

### 6.2 Human-readable monotonic keys at scale
`ENG-1421` requires a gap-tolerant-or-gapless monotonic counter per team. Gapless +
distributed + high-throughput is a classic contention point (a single sequence row is a
hotspot). Options to evaluate: per-team sequence with batched allocation, allow gaps,
Hi/Lo allocators, or per-team single-writer. **[UNCERTAIN]** whether gaplessness is a real
requirement (it usually isn't, but users *perceive* gaps as bugs).

### 6.3 Hierarchy rollups & dependency computation
Rolling up progress/estimates/dates from millions of leaf issues to initiatives, and keeping
it fresh on every child change, is expensive. Deep/wide trees + cross-team parents + cycles in
the link graph (blocked-by) make naive recomputation O(bad). Need incremental/materialised
rollups, debounced recompute, and cycle detection. Critical-path/dependency analysis across a
large link graph is its own graph-compute problem. **A prime candidate for event-driven async
recomputation via the event bus** rather than synchronous writes.

### 6.4 Ordering / ranking at scale
Drag-to-reorder backlogs need stable fractional ranking (LexoRank / fractional indexing).
Known failure modes: rank exhaustion/rebalancing, and **concurrent reorders by multiple users
or agents** producing conflicts. Needs a concurrency story (CRDT-ish or server-arbitrated).

### 6.5 Reporting/analytics over huge datasets
Cycle-time, CFD, velocity, SLA-compliance over years of history and millions of issues are
aggregate scans. Doing them on the OLTP store will kill it. Implies a **separate columnar/OLAP
read model fed by the event stream** (CQRS-style), with the tracker emitting a clean change
event stream. The state-change event log doubles as the source for time-in-state metrics.

### 6.6 Real-time sync & collaborative editing
Linear-class UX implies a sync engine: optimistic updates, live multi-user changes on
boards/issues, presence, and conflict resolution. Collaborative rich-text on descriptions
overlaps the knowledge-platform's editor (share it). Live board reordering + agents mutating
issues concurrently raises the concurrency bar. Likely needs per-entity event subscriptions
over the event bus and a client cache/sync protocol. **Major cross-cutting architecture item.**

### 6.7 Custom-field indexing & query performance
Arbitrary user-defined fields + a flexible query language is the JQL performance trap.
EAV-style storage is flexible but query-hostile; per-field columns don't scale to thousands of
definitions across tenants. Hybrid (typed columns for common fields + JSON/EAV for the long
tail + a search-index projection) is the usual answer — **[DEFERRED]**, but flagged as the
single biggest query-perf risk.

### 6.8 Permission-filtered queries at scale
Browse/field-level permissions mean every list/search must be permission-filtered for the
viewing user — expensive when combined with full-text + custom fields. Must co-design with the
shared identity/access system; post-filtering large result sets is a perf and correctness
(leak) hazard, especially for confidential issues (§8.4).

### 6.9 SLA timers & scheduling at scale
Millions of running SLA timers with business calendars, pauses, and breach-at-exact-time
firing require a reliable distributed scheduler/timer service (shared system). Breaches must
fire events even under load and after restarts (durability).

### 6.10 Import at scale (correctness, not just throughput)
See §10. Large Jira/Linear imports stress ID remapping, link integrity, attachment volume,
and history fidelity. More a correctness/migration-engineering problem than raw scale, but it
*is* a scale problem for big customers (hundreds of thousands of issues + attachments).

---

## 7. Events — emit & consume (event bus / agent fabric)

> The tracker must be a **first-class event emitter and consumer**. Events are the agent
> integration surface and the analytics feed. Use the **strategy pattern** for agent handlers
> (mock during dev, real later — VISION §3). Below is a *research catalogue of event needs*,
> not a final schema. Naming/transport is **[DEFERRED]** to shared-systems.

### 7.1 Events the tracker EMITS (illustrative)
- Lifecycle: `issue.created`, `issue.updated` (field-level deltas), `issue.deleted`,
  `issue.restored`, `issue.state_changed` (with from/to + category — feeds metrics),
  `issue.assigned`, `issue.priority_changed`, `issue.type_changed`.
- Hierarchy/relations: `issue.linked` / `issue.unlinked` (with link type),
  `issue.parent_changed`, `issue.rollup_recomputed`.
- Comments/collab: `comment.created/updated/deleted`, `mention.created`,
  `subscription.changed`.
- Planning: `cycle.started`, `cycle.completed` (with carryover), `issue.added_to_cycle`,
  `project.created`, `initiative.health_changed`, `milestone.released`.
- Governance: `sla.started`, `sla.paused`, `sla.breached`, `sla.met`,
  `approval.requested/granted/rejected`, `workflow.transition_blocked`.
- Triage/agent hooks: `issue.triaged`, `issue.duplicate_suspected`, `issue.labelled_by_agent`.
- Reference-graph: every link to a PR/commit/doc/chat/CI run should emit a graph-edge event
  so the shared reference graph stays consistent.

### 7.2 Events the tracker CONSUMES (illustrative)
- From **git hosting**: branch created/named-for-issue, commit referencing issue, PR
  opened/merged/closed referencing issue, review approved → drive auto-transitions/links.
- From **CI**: pipeline started/passed/failed/deploy succeeded → update linked issue, gate
  transitions, auto-create incidents on prod failure.
- From **chat**: "create issue from message", message references issue (graph edge), command
  invocations.
- From **knowledge platform**: spec/doc linked or updated → reflect on issue; doc deleted →
  relation cleanup.
- From **identity/access**: user deactivated/erased → reassign/anonymise (GDPR §8).
- From **scheduler/timer**: SLA timer fired, cycle-end reached, recurring-issue due.
- From **agent fabric**: agent-proposed actions (label, dedup, transition, comment, estimate),
  applied via the same permissioned API a human uses (agents are principals — §8.7).

### 7.3 Agent-native triggers (the differentiator)
Triggers = (event filter via the query AST §5.4) + (condition) + (action via strategy-pattern
handler). Examples that should be *native*, not bolted on:
- New bug in triage → agent suggests labels, severity, duplicates, owning team.
- Issue stale N days in a state → agent nudges assignee / proposes transition.
- SLA at 80% → agent escalates / drafts a holding response.
- PR merged → verify acceptance criteria, request QA, transition.
- Rollup/forecast drift → agent flags at-risk initiative to PM.

**Research recommendation:** the trigger/automation engine and the agent fabric should be the
*same* mechanism with different action handlers (rule action vs agent action), so "automation"
and "agent" aren't two systems. Mock agent strategies during development satisfy VISION §3.

---

## 8. GDPR / erasure considerations specific to this subsystem

The tracker is **dense with personal data** (creators, assignees, mentions, commenters,
watchers, worklogs, audit actors) embedded across many objects and the immutable history.
This is one of the harder GDPR subsystems because **audit-immutability conflicts with
erasure**.

### 8.1 Personal data inventory (what to map for DSARs)
- Direct: user references (reporter, assignee, commenter, mention, watcher, audit actor,
  worklog author, @mentions in free text).
- Indirect/free-text: names/emails typed into titles, descriptions, comments, custom text
  fields, attachments. **Free-text PII is the hardest erasure case** — can't be found by FK.
- Behavioural: worklogs/time-tracking and activity reveal individual productivity → sensitive
  in some jurisdictions / works-council contexts (§8.5).

### 8.2 Erasure (right to be forgotten) strategy options
- **Anonymise/pseudonymise the actor** ("Former user 8a2f") rather than hard-deleting issues
  (deleting the issue would destroy other users' legitimate records). This is the expected
  approach; must propagate to history, comments, mentions, and the reference graph.
- Free-text PII: needs detection/redaction (agent-assisted scan?) — **[UNCERTAIN]** how
  complete this can be; document the residual risk honestly. Possibly require crypto-shredding
  of attachments and a tombstone for redacted text.
- **Crypto-erasure** option: per-subject encryption keys so "erase" = destroy key. Powerful
  but complicates search/indexing of that data. **[DEFERRED]** — shared-systems decision.

### 8.3 Audit vs erasure conflict
Corporate audit wants immutable history; GDPR wants erasure. The reconciliation (anonymise
actor identity while preserving the *fact* and *content* of the change, unless the content
itself is the PII) must be designed deliberately and documented. Legal-basis matters: audit
logs may have a legitimate-interest/legal-obligation basis that limits erasure scope —
**[UNCERTAIN]**, flag for legal review in architecture phase.

### 8.4 Confidential issues & leak prevention
Security/confidential issues must not leak personal or sensitive data via backlinks shown to
unauthorised users (a chat message or doc that references a confidential issue must not reveal
its title/contents). The reference graph must be **permission-aware on read**. This is both a
GDPR and a security requirement.

### 8.5 Special-category / sensitive data
Worklogs and productivity metrics can be subject to works-council / labour-law constraints in
EU states. Health-related incident reports might contain special-category data. The tracker
should support **field-level sensitivity classification** and access restriction. **[UNCERTAIN]
/ legal-dependent** — flag, don't over-design.

### 8.6 Data residency & export/portability
- Residency: issue data + attachments must be pin-able to an EU region/tenant (shared-systems).
- Portability (DSAR access + Art. 20): export a subject's data and an org's data in a
  structured, re-importable format (ties to §10 import format symmetry).

### 8.7 Agents as data processors / actors
Agents act on issues and read personal data. The audit trail must distinguish agent vs human
actors, record the agent identity/strategy, and respect the same permission and lawful-basis
constraints. Agent-driven processing of personal data (e.g. triage reading reporter PII) must
be covered by the org's lawful basis. Flag for the agent-fabric + identity research.

### 8.8 Retention
Configurable retention/auto-archival/auto-delete policies per org (e.g. delete resolved
support issues after N years) — both a corporate-governance feature and a GDPR
data-minimisation aid. Needs the scheduler and careful interaction with audit retention.

---

## 9. Dependencies on shared systems & other subsystems

### 9.1 Shared systems the tracker REQUIRES
- **Identity & access**: users, teams, orgs, roles; the tracker expresses needs for
  project/field/transition/browse-level permissions, confidential-issue visibility, and
  agents-as-principals. The tracker likely needs a permission model richer than a simple RBAC
  — flag to identity research that **ABAC/relationship-based (e.g. Zanzibar-style) may be
  required** for field/issue-level and "blocking-relation"-aware visibility. **[UNCERTAIN]**.
- **Event bus**: durable, ordered-enough, replayable (for analytics rebuild), with
  per-entity subscription for real-time sync (§6.6). Tracker is a heavy producer/consumer.
- **Agent fabric**: strategy-pattern handler registration, mock implementations now; the
  trigger engine (§7.3) is co-owned with this.
- **Storage**: attachments/blobs (GDPR-tagged, residency-pinned, crypto-shred-capable).
- **Search**: full-text + custom-field indexing feeding the query language (§5.4, §6.7);
  must be permission-aware (§6.8).
- **Notifications**: assignment/mention/SLA/breach notifications routed to chat/email/push.
- **Cross-artifact reference graph**: the canonical home for issue↔code↔doc↔CI↔chat edges
  and possibly hierarchy/relations (§3.5); must be permission-aware on read (§8.4).
- **Scheduler/timer**: SLAs, cycle ends, recurring issues, retention jobs (§6.9, §8.8).
- **Analytics/read store**: for reporting at scale (§6.5) — may be a shared platform service.

### 9.2 Other subsystems
- **Git hosting**: bidirectional linking, auto-transition on merge, branch/PR awareness
  (§4.1, §7.2). Tightest coupling of the five.
- **CI**: status surfacing, transition gating, auto-incident creation (§4.1).
- **Knowledge platform**: shared rich-text editor + shared field/view/database primitives
  (§3.7); spec/runbook linking.
- **Chat**: create-from-message, reference rendering (permission-aware), activity posting,
  human+agent discussion in-thread.

### 9.3 What the tracker OWNS (does not delegate)
The issue lifecycle/workflow state machine, hierarchy/rollup semantics, cycles/roadmaps/SLA
engine, the tracker query language semantics, and tracker-specific analytics definitions
(cycle time, etc.). These are the tracker's core competency.

---

## 10. Importing from Jira / Linear (and others)

### 10.1 Why it matters
Migration is the adoption gate. A company won't move off Jira unless import is high-fidelity.
This is both a feature and a credibility signal for the EU-sovereignty pitch ("leave Atlassian
cloud cleanly").

### 10.2 What must map
- **Issues**: fields, types, statuses (→ Myelin workflows + state categories), priorities,
  labels, components, resolutions, descriptions (Jira wiki-markup/ADF → Myelin rich text;
  lossy — flag), estimates/story points.
- **Hierarchy & relations**: epics/sub-tasks/initiatives → Myelin hierarchy; issue links →
  Myelin link types (mapping table needed; semantics differ).
- **Planning**: sprints/cycles, versions/releases/fix-versions, boards.
- **People**: reporters, assignees, watchers, comment authors → identity mapping (the hard
  part: matching/merging users; handling deactivated/erased source users).
- **History**: change-logs/activity (fidelity vs effort tradeoff — full history is expensive
  and PII-laden; offer configurable depth).
- **Attachments**: bulk transfer to shared storage (volume + residency considerations).
- **Custom fields**: map to Myelin field defs; the messiest part (type coercion, unmapped
  fields → JSON bag).
- **Comments/mentions**, **worklogs**, **components/teams**, **automation rules** (Jira
  automation → Myelin triggers — likely manual/assisted, not automatic).

### 10.3 Source specifics
- **Jira**: REST API (Cloud) / Data Center; or Jira's JSON/XML backup export. ADF rich-text
  is non-trivial to convert. JQL → Myelin query mapping for saved filters. Jira's permission
  schemes → Myelin permissions (lossy). **[UNCERTAIN]** on exact current API shapes — verify
  in architecture phase.
- **Linear**: GraphQL API; cleaner model (closer to Myelin's likely shape) → easier; Linear
  has its own export. Mapping Linear Projects/Cycles/Initiatives is fairly direct.
- **GitHub/GitLab Issues**: API-based; simpler model, weaker hierarchy → straightforward.
- **CSV/generic**: a lowest-common-denominator importer for everything else.

### 10.4 Import engineering concerns
- **ID remapping & link integrity**: two-pass import (create all, then wire links/parents) to
  avoid forward-reference problems; keep a source-ID↔Myelin-ID map (also enables incremental
  re-sync and rollback).
- **Idempotency / resumability**: large imports must resume after failure and not duplicate.
- **Rate limits** on source APIs; backfill throughput.
- **Dry-run + mapping preview + reconciliation report** (what mapped, what was lossy, what was
  dropped) — essential for trust.
- **Incremental / coexistence mode**: optionally keep syncing from Jira during a transition
  period (bidirectional is much harder — **[DEFERRED]**, probably out of scope for v1).
- **Symmetry with export**: the same canonical interchange format used for export/portability
  (§8.6) should ideally round-trip.

---

## 11. CLI commands expected

> Myelin is agent- and engineer-native; a first-class CLI is expected. Illustrative surface,
> not a spec. Should share the query language (§5.4) and emit/consume the same events.

```
# Issues
myelin issue create [--type bug] [--title ...] [--team ENG] [--assignee me] [--priority high]
myelin issue list "state:open assignee:me cycle:current sort:priority"   # query language
myelin issue show ENG-1421
myelin issue update ENG-1421 --state in-progress --priority urgent
myelin issue assign ENG-1421 @alice
myelin issue comment ENG-1421 "…"
myelin issue link ENG-1421 blocks ENG-1490
myelin issue parent ENG-1421 --set ENG-1000        # hierarchy
myelin issue close ENG-1421 --resolution fixed
myelin issue transition ENG-1421 "In Review"       # workflow-aware

# Planning
myelin cycle current|list|plan
myelin cycle add ENG-1421 --to current
myelin project|epic|initiative create|list|show
myelin roadmap show --team ENG

# Views / queries
myelin view list|show <name>
myelin view save "state:open label:flaky" --name "Flaky bugs" --board
myelin query "blocked-by:open AND cycle:current"

# Governance / reporting
myelin sla status ENG-1421
myelin report cycle-time --team ENG --since 90d
myelin audit ENG-1421                              # change history
myelin export --team ENG --format <canonical>      # portability/GDPR

# Import
myelin import jira --url … --project FOO --dry-run
myelin import linear --token … --team …
myelin import status <job-id>

# Automation / agents (strategy-pattern handlers)
myelin trigger list|create --on issue.created --when "type:bug" --do <handler>
myelin agent dry-run <handler> --on ENG-1421       # mock agent during dev
```

Design notes: every command must be expressible against the API the UI uses (no privileged
back-channel), must respect permissions identically, and should support `--json` for agent
consumption. Bulk operations (`issue bulk-update <query>`) and scriptable output are needed
for power users and agents.

---

## 12. Assumptions, deferrals & open questions

### 12.1 Key assumptions made here
- [ASSUMPTION] One flexible issue object + layered schemes (workflow/permission/field) beats
  three separate products. (Core thesis; architecture should pressure-test it.)
- [ASSUMPTION] A small **fixed set of state categories** with unlimited named states is
  mandated (enables cross-project reporting).
- [ASSUMPTION] Automations and agents are the *same* trigger engine with different action
  handlers.
- [ASSUMPTION] Hierarchy & relations are (or project into) the shared reference graph.
- [ASSUMPTION] Reporting/analytics runs on a separate read model fed by the event stream.

### 12.2 Deferred to architecture phases
- Storage tech, sharding/tenant-isolation strategy, OLTP vs OLAP split (§6.1, §6.5).
- Whether the tracker is built *on* the knowledge-platform database primitive or merely
  *shares* field/view primitives (§3.7).
- Custom-field storage & indexing scheme (§6.7).
- Sync-engine / collaborative-editing protocol (§6.6) — cross-cutting with frontend + knowledge.
- Crypto-erasure vs anonymisation as the erasure mechanism (§8.2).
- Permission model shape (RBAC vs ABAC vs Zanzibar-style) — flagged to identity research (§9.1).
- Bidirectional/coexistence import sync (§10.4).
- Forecasting/Monte-Carlo planning (§4.2).

### 12.3 Open questions (ranked by impact)
1. **Is "Epic/Initiative" a type or a hierarchy level?** Pivotal modelling fork (§3.2, §3.5).
2. **How much governance is baked-in vs opt-in scheme?** Determines whether we can be
   Linear-fast by default and Jira-powerful on demand without a fork in the product (§4.4).
3. **Tracker-on-shared-database, or separate-but-sharing-primitives?** (§3.7) — needs joint
   resolution with the knowledge-platform deep-dive.
4. **What permission granularity is truly required** (field/transition/browse/confidential)
   and can the shared identity system express it at scale (§6.8, §8.4, §9.1)?
5. **Does the reference graph own hierarchy/relations, or does the tracker keep a local
   materialised tree for rollup performance** (§3.5, §6.3)?
6. **Gapless vs gap-tolerant human keys** — is gaplessness a real requirement (§6.2)?
7. **How complete can free-text PII erasure be**, and what residual risk do we accept and
   document (§8.1, §8.2)?
8. **Single canonical query AST** for UI/CLI/API/automation/agents — feasible and how
   expressive (relation traversal, full-text mix) (§5.4)?
9. **Offline / local-first** in scope for v1 or not (§4.1, §5.5)?
10. **SLA/business-calendar engine** — build vs shared-scheduler-provides-primitives (§6.9).

### 12.4 Honest uncertainty notes
- Competitor feature details (§2) and Jira/Linear API specifics (§10.3) are from domain
  knowledge as of training; **re-verify before any decision hinges on them**.
- GDPR specifics around audit-log retention basis and works-council/worklog constraints
  (§8.3, §8.5) need **legal review** — this doc raises them, it does not resolve them.
- Scale numbers and "this will kill the OLTP store" claims (§6) are directional engineering
  judgement, not benchmarked; treat as hypotheses for the architecture phase to test.
```
