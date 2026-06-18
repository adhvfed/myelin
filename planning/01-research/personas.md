# Myelin — Personas & User Segments

> Phase 1 research deliverable. Canonical brief: [`VISION.md`](../../VISION.md).
> This document defines *who* Myelin serves so that later phases (use cases, architecture,
> roadmaps, prompts) can be grounded in concrete needs rather than abstractions.

## 0. How to read this document

Myelin is **one platform, five subsystems** — git hosting, CI, issue tracker, knowledge
platform, chat — unified by shared backend systems (identity/access, event bus, agent
fabric, storage, search, notifications, cross-artifact reference graph). The whole point
of the platform is that **work flows across subsystems** and that **agents are first-class
users, not bolt-ons**.

Personas here are therefore described against two axes:

1. **Which subsystems they touch most** — to inform where each subsystem must invest in UX
   and depth.
2. **What cross-subsystem flows they live in** — because the differentiator is the glue,
   not any single tool.

A note on terminology used throughout:

- **Subsystem touch map** uses these short codes: **Git**, **CI**, **Issues**,
  **Knowledge**, **Chat**, and the shared layer **Shared** (identity, search,
  notifications, reference graph, agent fabric).
- "Today's fragmented stack" is shorthand for the typical reality: GitHub/GitLab +
  Jenkins/Actions + Jira/Linear + Notion/Confluence + Slack/Teams, stitched together with
  webhooks, bots, and copy-paste. Much of Myelin's value proposition is *removing the
  seams* between these.

**Uncertainty flag (read first).** These personas are synthesised from strong domain
knowledge of how engineering organisations actually work, plus knowledge of competitor
products. They are **hypotheses, not validated research**. No user interviews have been
conducted. The "pains" are well-evidenced industry patterns; the **relative priority**
between personas, and the willingness-to-pay and adoption assumptions, are **not yet
validated** and must be revisited in positioning and architecture phases. See
[Section 7](#7-open-questions-assumptions-and-deferrals).

---

## 1. Persona taxonomy at a glance

| # | Persona | Cluster | Primary subsystems | Agent-heavy? |
|---|---------|---------|--------------------|--------------|
| P1 | Backend engineer | Individual contributor | Git, CI, Issues | High |
| P2 | Frontend engineer | Individual contributor | Git, CI, Issues, Knowledge | High |
| P3 | Platform / SRE engineer | Individual contributor | CI, Git, Shared, Issues | High |
| P4 | Staff / principal engineer | Individual contributor (leverage) | All five | Medium-High |
| P5 | Open-source maintainer | Individual contributor (public) | Git, CI, Issues | Medium |
| P6 | Product manager | Product & delivery | Issues, Knowledge, Chat | Medium |
| P7 | Engineering manager | Product & delivery | Issues, Chat, Git (read), Knowledge | Medium |
| P8 | Program / project manager | Product & delivery | Issues, Knowledge, Chat | Low-Medium |
| P9 | Product designer | Product & delivery | Knowledge, Issues, Chat | Low-Medium |
| P10 | Technical writer | Product & delivery | Knowledge, Git, Issues | Medium |
| P11 | CTO / VP Engineering | Corporate / enterprise | Issues (reporting), Knowledge, Shared | Low |
| P12 | Security / compliance officer | Corporate / enterprise | Shared, Git, CI, audit surfaces | Medium |
| P13 | DPO / data protection officer | Corporate / enterprise | Shared (identity, audit, data rights) | Low |
| P14 | Procurement / vendor mgmt | Corporate / enterprise | Shared (contracts, billing, audit) | Low |
| P15 | IT admin / platform team (runs Myelin) | Corporate / enterprise | Shared (admin), all subsystems (ops) | Medium |
| A1 | Coding agent | Agent | Git, CI, Issues, Chat | — |
| A2 | Triage agent | Agent | Issues, Chat, Shared | — |
| A3 | Review agent | Agent | Git, CI, Chat | — |
| A4 | Knowledge-curation agent | Agent | Knowledge, Search, Reference graph | — |
| A5 | Ops / SRE agent | Agent | CI, Shared, Chat, Notifications | — |

"Agent-heavy?" indicates how much a human persona's daily work is expected to be
*mediated by or shared with* agents — a signal for where the agent fabric must be most
mature and where human-in-the-loop controls matter most.

---

## 2. Individual engineer personas

These are the highest-frequency users. They will judge Myelin on raw daily UX: speed of
git operations, CI feedback latency, how little context-switching the platform forces, and
whether agents save them time or create noise.

### P1 — Backend engineer

**Who they are.** Builds services, APIs, data models, and business logic. Lives in the
code, the test suite, and the deployment pipeline. Comfortable on the CLI; values keyboard
speed and reproducibility. Often owns on-call for their services.

**Primary goals.**
- Ship correct, well-tested changes quickly with high confidence.
- Get fast, trustworthy CI signal (and not be blocked by flaky or slow pipelines).
- Keep mental context: understand *why* code is the way it is (link to issue, doc, prior
  discussion) without leaving their flow.
- Reproduce and fix production issues quickly.

**Pains with today's fragmented stack.**
- Context is scattered: the *what* is in Jira, the *why* is in a Confluence doc or a Slack
  thread, the *how* is in the PR — three logins, three search boxes, no shared link graph.
- CI is a separate mental model (YAML in one place, results in another, secrets in a
  third). Debugging a red pipeline means digging through opaque logs.
- Agents/bots are bolted on per-tool (a Slack bot here, a GitHub Action there) with
  inconsistent permissions and no shared memory of the work.
- Cross-references rot: a Slack link to a closed Jira ticket tells you nothing months
  later.

**What success looks like.**
- One unified search finds the commit, the issue, the design doc, and the chat thread.
- A pull request shows its linked issue, the CI run, the relevant doc section, and the
  discussion inline — no tab-hopping.
- A coding agent can take a well-specified issue and open a draft PR; the engineer reviews
  rather than scaffolds.
- CI failures explain themselves; an ops/review agent can pre-triage flakiness.

**Subsystems touched most.** Git (heavy), CI (heavy), Issues (medium), Chat (medium),
Knowledge (light read). Lives in the **cross-artifact reference graph** constantly even if
they never name it.

### P2 — Frontend engineer

**Who they are.** Builds user-facing UI; cares about design fidelity, component reuse,
accessibility, and visual review. Bridges engineering and design.

**Primary goals.**
- Implement designs faithfully and quickly; catch visual regressions.
- Collaborate tightly with designers (P9) on intent and edge cases.
- Reuse a shared component system; keep the UI consistent.

**Pains with today's stack.**
- Design lives in Figma (separate tool, separate comments), specs in tickets, the
  implementation in the repo — three sources of truth that drift.
- Visual/UX review happens in screenshots pasted into PRs or Slack; feedback is lossy and
  detached from the code.
- Hard to trace "which design decision does this code implement, and who approved it."

**What success looks like.**
- A design artifact (or at least the *reference* to one) lives in the knowledge platform
  and is linked from the issue and the PR.
- Review comments, design intent, and code sit in one reference graph.
- CI can attach preview builds / visual diffs to the PR automatically.

**Subsystems touched most.** Git (heavy), CI (medium, preview builds & visual tests),
Issues (medium), Knowledge (medium — design/spec docs), Chat (medium).

### P3 — Platform / SRE engineer

**Who they are.** Owns the reliability, deployment infrastructure, CI runners, and
internal developer platform. Frequently the person who would *operate Myelin itself* in a
self-hosted deployment (overlaps with P15). Thinks in systems, automation, and blast
radius.

**Primary goals.**
- Keep pipelines, environments, and shared services healthy and fast.
- Provide paved-road workflows so product engineers move fast safely.
- Automate toil; codify runbooks; respond to incidents with good signal.

**Pains with today's stack.**
- CI is the constant pain center: maintaining runners, secrets, caching, matrix builds,
  and flaky-test quarantine across disconnected tools.
- Incident response spans pager, chat, dashboards, and a runbook wiki that's always stale.
- Event plumbing between tools (deploy → notify → update ticket → post to channel) is
  hand-built webhook glue that breaks silently.
- Audit and access control differ per tool, multiplying compliance work.

**What success looks like.**
- A single **event bus** makes "when X happens, do Y" first-class and observable instead
  of webhook spaghetti.
- Runbooks live in Knowledge, are referenced from incidents (Issues) and Chat, and can be
  executed/assisted by an **ops agent (A5)**.
- One identity/permission model across all subsystems to reason about and audit.

**Subsystems touched most.** CI (heavy), Shared/event bus (heavy), Git (medium), Issues
(medium — incidents), Chat (medium — incident channels), Knowledge (medium — runbooks).
The **strongest internal stakeholder for the shared backend**.

### P4 — Staff / principal engineer

**Who they are.** Senior technical leader who works through influence and leverage, not
just direct commits. Sets technical direction, reviews high-stakes changes, writes design
docs (RFCs), and unblocks teams across boundaries.

**Primary goals.**
- Drive architectural coherence across many teams and repos.
- Make decisions visible and durable (design docs, ADRs) so they're not re-litigated.
- Spot risk early; mentor; review the changes that matter most.

**Pains with today's stack.**
- The org's reasoning is fragmented: RFCs in one wiki, decisions in chat, consequences in
  tickets and code, with no durable thread connecting them.
- Hard to get a cross-cutting view of "all work touching subsystem Z" across repos,
  tickets, and docs.
- Their leverage tooling (search, linking, dashboards) is the weakest part of every
  existing toolchain.

**What success looks like.**
- A design doc in Knowledge links to the issues it spawns, the PRs that implement it, and
  the CI evidence that it works — and stays linked.
- Cross-artifact search and the reference graph let them answer "why" and "where" questions
  in seconds.
- Agents handle mechanical review (style, obvious bugs, missing tests) so human review
  focuses on architecture.

**Subsystems touched most.** All five, but distinctively **Knowledge + reference graph +
search** as their leverage surface, plus Git (review) and Issues (portfolio view).

### P5 — Open-source maintainer

**Who they are.** Maintains a public project; may be solo or community-led. Manages
contributions from strangers, triages a large noisy issue queue, and cares deeply about
project reputation, contributor experience, and governance.

**Primary goals.**
- Keep the contribution funnel healthy: easy first PRs, fast CI on forks, clear review.
- Triage and label a high-volume issue stream without burning out.
- Maintain public docs and a welcoming community.

**Pains with today's stack.**
- Issue triage is overwhelming and manual; spam and duplicates drown signal.
- CI on external contributions is a security and cost headache (secrets exposure, fork
  permissions).
- Public visibility + private maintainer coordination is awkward to mix.

**What success looks like.**
- A **triage agent (A2)** auto-labels, deduplicates, and routes issues; humans confirm.
- Safe CI for fork PRs with scoped, auditable permissions.
- Public read access with clean separation from private governance discussion in Chat.

**Subsystems touched most.** Issues (heavy — triage), Git (heavy — public review), CI
(medium — fork safety), Chat (light), Knowledge (medium — public docs).

> **EU-sovereignty angle for P5:** an EU-hosted, GDPR-clean public forge is a genuine
> differentiator for European public-sector and research open source (cf. demand behind
> the EU's own code-hosting efforts). Flagged for the positioning doc.

---

## 3. Product & delivery personas

These users are why the issue tracker must serve *both* engineers and non-engineers, and
why Knowledge and Chat matter as much as Git. They are the people most failed by the
"engineering tool vs. management tool" split in today's market.

### P6 — Product manager (PM)

**Who they are.** Owns the *what* and *why* of the product: discovery, prioritisation,
roadmap, requirements, stakeholder alignment. Usually non-technical-by-trade but technical-
adjacent. **Critically, the vision names PMs as a co-equal audience of the issue tracker.**

**Primary goals.**
- Maintain a clear, prioritised roadmap and communicate it.
- Write and evolve specs/PRDs; tie them to the work delivering them.
- Track progress and report status to stakeholders honestly.
- Gather and synthesise user feedback into prioritised work.

**Pains with today's stack.**
- The classic split: Jira/Linear is for engineers and feels hostile; Productboard/Notion
  is for PMs; the two never truly sync, so PMs manage a parallel reality and reconcile by
  hand.
- Roadmaps drift from execution; "what's actually shipping this quarter" requires manual
  status-chasing.
- PRDs in one tool, tickets in another, discussion in a third — no living link between
  intent and delivery.

**What success looks like.**
- A PRD in Knowledge *is* linked to its epics/issues, and roadmap views reflect real
  delivery state pulled from the same data.
- The issue tracker presents a PM-friendly view (roadmaps, now/next/later, outcomes) over
  the *same* data engineers see as sprints/boards — no second system.
- Agents summarise progress and surface risks across the portfolio.

**Subsystems touched most.** Issues (heavy — roadmap & reporting views), Knowledge (heavy —
PRDs, research), Chat (medium — stakeholder + agent updates). Light on Git/CI directly but
depends entirely on the **reference graph** tying delivery back to intent.

### P7 — Engineering manager (EM)

**Who they are.** Owns a team's delivery, health, and growth. Balances people, process,
and technical outcomes. Lives between ICs and leadership.

**Primary goals.**
- Keep the team unblocked, focused, and sustainably paced.
- Understand delivery flow and bottlenecks; report up honestly.
- Support performance/growth conversations with real signal (not surveillance).

**Pains with today's stack.**
- Metrics are scattered and easy to game; assembling a true picture of flow/health means
  manual spreadsheet work across tools.
- One-on-one notes, team docs, and delivery data live apart.
- Notifications are overwhelming and undifferentiated — no good "what needs *me*" view.

**What success looks like.**
- Trustworthy delivery analytics (cycle time, WIP, review latency, CI health) drawn from
  one event stream, not bolted-on integrations.
- A smart, prioritised notification/inbox model that respects attention.
- Team knowledge and process docs linked to the work they govern.

**Subsystems touched most.** Issues (heavy — boards, analytics), Chat (heavy),
Knowledge (medium), Git (medium read — review oversight). Strong stakeholder for
**notifications** and **reporting**.

### P8 — Program / project manager (TPM / PgM)

**Who they are.** Coordinates large, cross-team initiatives and dependencies. Cares about
timelines, milestones, risks, and cross-functional alignment more than individual tickets.

**Primary goals.**
- Make cross-team dependencies and critical paths visible and managed.
- Track milestones and risks across many teams/repos.
- Produce reliable status and forecasts for leadership.

**Pains with today's stack.**
- Dependency tracking across multiple Jira projects/instances is painful and brittle.
- Status reporting is a manual aggregation tax every week.
- No single timeline view spanning code, delivery, and decisions.

**What success looks like.**
- Cross-project dependency graphs and portfolio timelines over the same shared data.
- Automated, trustworthy status roll-ups; risks surfaced by agents.
- Decisions, plans, and execution linked in one reference graph.

**Subsystems touched most.** Issues (heavy — portfolio/dependencies), Knowledge (medium —
plans, status docs), Chat (medium). Depends heavily on **reporting + reference graph**.

### P9 — Product designer

**Who they are.** Owns user experience and interface design; collaborates with PMs and
frontend engineers. Works visually; cares about design systems and research.

**Primary goals.**
- Translate problems into well-crafted designs; keep a coherent design system.
- Collaborate on intent and feasibility with PM and engineering.
- Ensure what ships matches the design and incorporates research.

**Pains with today's stack.**
- Design tooling (Figma) is an island; comments and decisions don't reach the engineering
  reference graph.
- Research findings get lost; hard to connect "this design decision" to "this user
  insight" to "this shipped change."
- Handoff is lossy and detached from delivery.

**What success looks like.**
- Design artifacts and research live in (or are first-class references within) Knowledge,
  linked to issues and PRs.
- Design feedback and approvals are part of the same flow as code review.
- (Realistic scope) Myelin references and links to external design tools cleanly even if it
  doesn't replace them.

**Subsystems touched most.** Knowledge (heavy), Issues (medium), Chat (medium), Git (light —
visual review references). **Uncertainty:** how deep Myelin goes into design *authoring*
vs. *referencing* is an open product question, deferred to positioning/architecture.

### P10 — Technical writer

**Who they are.** Owns documentation quality: user docs, API references, internal
knowledge. Bridges engineering and end users/readers.

**Primary goals.**
- Keep docs accurate, discoverable, and in sync with the product/code.
- Establish information architecture and style; reduce doc rot.
- Work close to the source of truth (code, issues, releases).

**Pains with today's stack.**
- Docs drift from code because they live in a separate wiki with no link to what changed.
- "Docs-as-code" (Markdown in the repo) vs. "wiki" (Confluence/Notion) is an eternal,
  unhappy tradeoff.
- No reliable signal of *what changed that needs documenting*.

**What success looks like.**
- Knowledge platform supports both structured rich docs *and* close coupling to Git
  (a merged PR can flag docs that need updating; a doc references the code it describes).
- A **knowledge-curation agent (A4)** flags stale docs, suggests updates from merged
  changes, and improves discoverability.
- Release/changelog flows tie code changes to doc updates automatically.

**Subsystems touched most.** Knowledge (heavy), Git (medium — docs-as-code, change
signal), Issues (medium — doc tasks), Chat (light). Strong consumer of **A4** and the
**reference graph**.

---

## 4. Corporate / enterprise personas

These personas rarely use Myelin daily, but they **decide whether an organisation adopts
it** and **whether it can be operated lawfully**. For Myelin's EU-sovereign positioning,
P12–P14 are not edge cases — they are central buyers and gatekeepers.

### P11 — CTO / VP of Engineering

**Who they are.** Executive owner of engineering. Cares about velocity, cost, risk,
talent, and strategic alignment. Makes or sponsors platform-buying decisions.

**Primary goals.**
- Maximise org-wide engineering throughput and quality at sane cost.
- Reduce tool sprawl, integration cost, and vendor risk.
- Ensure the org can demonstrate control, security, and compliance.
- Bet on a strategically safe platform (here: EU sovereignty / reduced US-vendor
  dependency).

**Pains with today's stack.**
- Death by a thousand SaaS subscriptions; integration tax; data scattered across vendors.
- No single source of truth for "how is engineering actually doing."
- Geopolitical/regulatory exposure from US-controlled hyperscalers and SaaS.
- Agent strategy is fragmented across tools with no governance.

**What success looks like.**
- One platform, one bill, one identity model, one audit trail — measurable consolidation.
- Org-wide visibility into delivery health from trustworthy shared data.
- A coherent, governed approach to agents across the whole SDLC.
- Defensible digital-sovereignty story to their own board and customers.

**Subsystems touched most.** Issues/Knowledge (executive reporting & strategy views),
Shared (governance, audit, cost). Mostly a **consumer of roll-ups and a decision-maker**,
not a daily user. Primary **economic buyer** alongside P12/P14.

### P12 — Security / compliance officer (CISO-side)

**Who they are.** Owns information security and compliance posture (e.g. ISO 27001, SOC 2,
sector regimes). Gatekeeper for any tool touching code and data.

**Primary goals.**
- Enforce least-privilege access, strong authentication, and full auditability.
- Control secrets, supply-chain security, and CI/CD attack surface.
- Pass audits and respond to incidents with complete evidence.

**Pains with today's stack.**
- Per-tool, inconsistent RBAC and audit logs make a unified security picture impossible.
- CI/CD is a top supply-chain attack surface; secret sprawl across tools is dangerous.
- Bots/integrations hold broad, poorly-audited permissions.
- Evidence-gathering for audits is a manual, painful scavenger hunt.

**What success looks like.**
- **One identity & access model** with fine-grained, auditable RBAC across all five
  subsystems and the agent fabric.
- Comprehensive, tamper-evident audit logging of every action (human *and* agent) on every
  artifact.
- First-class secrets management and supply-chain controls in CI.
- Agents are *identities with scoped, least-privilege permissions and full audit trails* —
  not shared bot tokens.

**Subsystems touched most.** Shared (heavy — identity, access, audit), CI (heavy —
supply chain, secrets), Git (medium — branch protection, signing). Strong stakeholder for
**agent permissioning** — a security officer's biggest fear about agent-native platforms.

### P13 — DPO / GDPR data protection officer

**Who they are.** Accountable for GDPR compliance and data-subject rights. May be a legal/
governance professional, not technical. **For Myelin's core promise, this persona is a
first-class design constraint, not an afterthought.**

**Primary goals.**
- Ensure lawful processing: lawful basis tracking, data minimisation, purpose limitation.
- Guarantee data-subject rights: access, rectification, erasure ("right to be forgotten"),
  portability, restriction.
- Ensure EU data residency and control; manage records of processing (ROPA) and DPIAs.
- Be able to demonstrate compliance to regulators.

**Pains with today's stack.**
- Personal data (names, emails, commit authorship, chat content, issue mentions) is
  smeared across many US-controlled SaaS tools with unclear residency.
- "Right to erasure" is nearly impossible to honour across a fragmented stack — and erasing
  authorship/commit data conflicts with engineering integrity needs.
- No unified record of *what personal data exists where, on what lawful basis*.
- Sub-processor sprawl and cross-border transfer risk (post-Schrems II) are hard to manage.

**What success looks like.**
- **By construction**: EU data residency; a unified data inventory; lawful-basis tracking;
  built-in tooling for access/erasure/portability requests across all subsystems at once.
- Clear handling of the hard cases (e.g. anonymising authorship while preserving history
  integrity) with documented, defensible behaviour.
- Auditable consent/lawful-basis records and ROPA-supporting data.
- Confidence that no personal data silently leaves EU control.

**Subsystems touched most.** Shared (heavy — identity, data inventory, audit, data-rights
tooling). Touches *all* subsystems' data indirectly, because personal data lives
everywhere. **The single most important persona for validating the GDPR-by-construction
promise.** See [Section 7](#7-open-questions-assumptions-and-deferrals) for the hard open
questions this persona raises (erasure vs. integrity, pseudonymisation in the reference
graph).

### P14 — Procurement / vendor management

**Who they are.** Owns vendor selection, contracts, and ongoing vendor governance.
Gatekeeper alongside security and the DPO; cares about TCO, terms, lock-in, and risk.

**Primary goals.**
- Negotiate favourable, low-risk contracts; control total cost of ownership.
- Ensure data-processing agreements, SLAs, sub-processor lists, and exit/portability terms
  are sound.
- Avoid lock-in; ensure data export and an exit path.

**Pains with today's stack.**
- Managing many vendors, contracts, renewals, and DPAs is heavy overhead.
- Lock-in fears: getting data *out* of incumbents is hard.
- Unclear sub-processor chains and cross-border transfer exposure.

**What success looks like.**
- Consolidation reduces vendor count and contract overhead dramatically.
- Clean DPA, transparent sub-processor list (ideally all EU), and strong **data
  portability / exit** guarantees that reduce lock-in fear.
- Predictable, transparent pricing.

**Subsystems touched most.** Shared (contracts, billing, audit, export). Not a daily user;
a **gatekeeper and economic stakeholder**. Myelin's anti-lock-in, EU-sovereign story is
aimed squarely here.

### P15 — IT admin / platform team running Myelin

**Who they are.** The team that *operates Myelin* for their organisation — provisioning,
configuring, integrating with corporate identity, monitoring, and supporting users.
In self-hosted/sovereign deployments this overlaps heavily with P3 (Platform/SRE) and is a
*primary* persona, since EU-sovereign often implies self-managed or sovereign-cloud
operation.

**Primary goals.**
- Stand up and run Myelin reliably and securely (self-hosted or sovereign cloud).
- Integrate with corporate SSO/identity (SAML/OIDC/SCIM), provisioning, and policy.
- Configure org-wide settings: RBAC, retention, data residency, agent policy.
- Monitor health, manage upgrades, support users, control cost.

**Pains with today's stack.**
- Operating *N* separate tools means *N* upgrade cycles, *N* identity integrations, *N*
  monitoring setups, *N* support burdens.
- Inconsistent admin models; no single org-wide policy surface.
- Self-hosting options for the full stack are weak or absent in many incumbents.

**What success looks like.**
- One admin surface for identity, RBAC, residency, retention, agent policy, and audit
  across all subsystems.
- Clean SSO/SCIM integration; org/team/project hierarchy mapping.
- First-class self-host / sovereign-cloud operability (this is core to the EU promise) with
  good observability and upgrade ergonomics.
- Granular control over the **agent fabric**: which agents exist, what they may touch, and
  org-level kill switches.

**Subsystems touched most.** Shared (heavy — admin, identity, policy, audit), plus the
operational side of all subsystems. The persona that **makes or breaks the self-hosted
sovereignty story**; closely allied with P3, P12, P13.

---

## 5. Agent personas (first-class users)

Per the vision, **agents are first-class citizens, not bolt-ons**. They are *users* of the
platform with identities, scoped permissions, and full audit trails — they participate in
the same event bus, reference graph, and chat channels as humans. During development they
are **mock implementations behind the strategy pattern**, so the platform is built around
their interaction model and swapping in real agents is a config/implementation change, not
a rewrite.

Cross-cutting design principles for *all* agent personas:

- **Identity & least privilege.** Each agent is a distinct identity (or acts on behalf of a
  human/team with delegated scope), with explicit, auditable, least-privilege permissions —
  never a shared god-token. This is the answer to P12's deepest fear.
- **Event-driven.** Agents are activated by **triggers on the event bus** (e.g. "issue
  labelled `bug`", "PR opened", "CI failed", "doc edited"). The platform's first-class
  event propagation is what makes them native.
- **Human-in-the-loop by default.** Agent actions on high-stakes surfaces (merging,
  closing, deleting, deploying) default to *proposing* with human approval; autonomy is a
  per-action, per-scope policy decision owned by P12/P15.
- **Auditable & reversible.** Every agent action is logged like a human's and, where
  feasible, reversible. Agents appear in history transparently as agents.
- **GDPR-aware.** Agents processing personal data are bound by the same lawful-basis and
  residency constraints; their processing is part of the ROPA picture (a P13 concern).
- **Strategy-pattern pluggability.** A common agent interface lets mock and real
  implementations be swapped per deployment/config.

| Agent | Triggered by (events) | Acts on | Typical autonomy | Key human owner |
|-------|----------------------|---------|------------------|-----------------|
| A1 Coding | Issue ready/assigned; review feedback | Git (branches, PRs), CI | Propose (draft PR); human reviews/merges | P1–P4 |
| A2 Triage | Issue created/updated; chat report | Issues (labels, routing, dedup), Chat | Act on low-risk (label/route); propose closes | P5, P6, P7 |
| A3 Review | PR opened/updated; CI complete | Git (review comments), Chat | Advisory; never auto-merge by default | P1–P4, P12 |
| A4 Knowledge-curation | Doc edited; PR merged; staleness timer | Knowledge, Search index, Reference graph | Propose edits/links; human approves publish | P10, P4 |
| A5 Ops / SRE | CI failure; alert; deploy event | CI, Notifications, Chat, Issues (incidents) | Act on runbook-safe steps; escalate else | P3, P15 |

### A1 — Coding agent

**Goal.** Turn well-specified issues into working code changes; respond to review feedback;
keep changes tested and explained.
**Touches.** Git (heavy), CI (heavy), Issues (reads spec, updates status), Chat (asks
clarifying questions).
**Permissions.** Create branches/PRs, run CI; **cannot** merge or alter protected branches
without human approval; scoped to assigned repos/issues.
**Success.** Engineers review and refine rather than scaffold; throughput rises without
sacrificing review quality or auditability.

### A2 — Triage agent

**Goal.** Keep the issue stream clean: classify, label, deduplicate, route, and request
missing info; summarise inbound reports.
**Touches.** Issues (heavy), Chat (intake), Shared/search & reference graph (dedup).
**Permissions.** Label/route/comment; flag duplicates; **propose** (not force) closes;
scoped per project.
**Success.** Maintainers (P5) and PMs/EMs (P6/P7) face a curated queue, not a firehose;
nothing important is buried.

### A3 — Review agent

**Goal.** Mechanical and policy review of PRs (style, obvious bugs, missing tests, security
lint, license/dependency checks) so humans focus on architecture.
**Touches.** Git (review comments), CI (consumes results), Chat (summaries).
**Permissions.** Comment and request changes; **never auto-approves/merges** by default;
security checks may *block* per P12 policy.
**Success.** Faster, higher-signal human review; consistent enforcement of standards;
security/compliance gains an automated, auditable gate.

### A4 — Knowledge-curation agent

**Goal.** Keep the knowledge base accurate, linked, and discoverable: flag stale docs,
suggest updates from merged changes, improve structure, maintain the reference graph,
generate summaries/changelogs.
**Touches.** Knowledge (heavy), Search index, Reference graph.
**Permissions.** Propose edits, links, and reorganisation; **human approves publishing** of
substantive changes; may auto-maintain low-risk metadata/links per policy.
**Success.** Doc rot drops; the reference graph stays rich and trustworthy; P10 and P4 spend
time on substance, not janitorial work.

### A5 — Ops / SRE agent

**Goal.** Assist operations and incident response: detect issues from events, run
runbook-safe remediation, open/annotate incidents, post status, and escalate.
**Touches.** CI (heavy), Notifications, Chat (incident channels), Issues (incidents),
event bus.
**Permissions.** Execute pre-approved runbook actions in defined scopes; **escalate to
humans** for anything outside the runbook; deploy/rollback only under explicit policy.
**Success.** Faster MTTR, less toil and pager fatigue for P3/P15, with every action
auditable and within guardrails.

> **Note on agent realism (uncertainty):** these capability descriptions are *target*
> behaviours. In Phase 1 build, agents are **mock** (deterministic/stubbed) behind the
> strategy pattern. What matters for this phase is that personas, permissions, triggers,
> and human-in-the-loop boundaries are designed correctly so real agents drop in later.

---

## 6. Organisation archetypes

The same personas exist across organisations, but their *weighting, constraints, and
buying logic* differ sharply. This matters for both "world-scale from day 1" (the platform
must serve the smallest and the largest at once) and the GDPR/EU-sovereign positioning
(which lands hardest at the regulated and public-sector end).

| Dimension | Solo / startup | Scale-up | Regulated enterprise | Public-sector / EU institution |
|-----------|----------------|----------|----------------------|--------------------------------|
| Size | 1–15 | ~15–300 | 300–10,000s | Varies; often large, fragmented |
| Dominant personas | P1–P5, occasionally P6 | + P6–P10, P11 | + P11–P15 fully | P12–P15 dominate gatekeeping |
| Top priority | Speed, low friction, low cost | Scaling process without bureaucracy | Control, compliance, auditability | Sovereignty, transparency, accountability |
| GDPR posture | Aware, light-touch | Maturing; first DPO appears | Central, heavy, audited | Mandatory and strict; sovereignty is the point |
| Agent appetite | High (force multiplier for tiny teams) | High but needs governance | Cautious; needs strict permissioning/audit | Cautious; transparency + control essential |
| Deployment | Managed/cloud, EU region | Managed EU; some self-host interest | Self-host or sovereign cloud; strict residency | Self-host / sovereign cloud strongly preferred |
| Decisive buyer | The founder/engineer | CTO/VP + EM | CTO + CISO + DPO + procurement | Procurement + DPO + IT, under public rules |
| Biggest churn risk | Friction/cost vs. incumbents | Outgrowing the tool; process pain | Failed audit/compliance gap | Sovereignty/transparency failure; lock-in |

**Solo / startup.** Wants to *move fast and own less plumbing*. Values consolidation
(one tool instead of five subscriptions) and agents as a force multiplier. GDPR matters but
is light-touch; EU hosting is a nice trust signal. Must be near-zero-friction to onboard.
A weak onboarding/UX story loses this segment instantly.

**Scale-up.** The painful "growing pains" phase: introducing PMs, EMs, designers, and
process without becoming bureaucratic; the PM-vs-engineering tool split (P6) bites hard
here; the first DPO and security hire appear. Wants Myelin's unified data model to *scale
process without splitting tools*. Strong candidate for the core wedge.

**Regulated enterprise** (finance, health, etc.). Compliance, security, and audit are
existential. P12/P13/P14/P15 are full gatekeepers; nothing adopts without passing them.
Self-host or sovereign cloud, strict residency, complete audit, and tight agent
permissioning are entry tickets, not extras. Long sales cycles, high value, high stickiness.
This is where Myelin's GDPR-by-construction and unified-audit story is worth the most.

**Public-sector / EU institution.** Digital sovereignty is the *explicit mandate*, not a
preference — this is the archetype the vision is most directly aimed at. Transparency,
accountability, EU control of data and infrastructure, open standards, and avoidance of
US-hyperscaler/SaaS lock-in are decisive. Procurement is rule-bound and slow; portability
and exit guarantees matter enormously. Agent transparency and auditability are
non-negotiable. **Strategically, the segment most aligned with Myelin's core differentiator
— but with the hardest procurement and the highest assurance bar.**

**Cross-archetype implications for the platform:**
- **Multi-tenancy & scale** must serve a 3-person startup and a 10,000-person enterprise on
  the same architecture (vision: world-scale from day 1).
- **Deployment flexibility** (managed EU cloud *and* self-host/sovereign cloud) is required,
  not optional, because the most sovereignty-sensitive buyers demand self-operation (P15).
- **Configurable governance** (RBAC depth, retention, residency, agent autonomy) must scale
  from "sensible defaults, invisible" (startup) to "fully controlled and audited"
  (enterprise/public-sector) without forcing the startup through enterprise complexity.

---

## 7. Open questions, assumptions, and deferrals

**Honesty about uncertainty (vision principle). Explicitly flagged:**

**Assumptions made (unvalidated):**
- No user research has been conducted; personas are domain-knowledge hypotheses. Relative
  priority, willingness-to-pay, and adoption order are **not validated**.
- Assumed the scale-up and regulated/public-sector segments are the strongest fits; this is
  a strategy hypothesis for the positioning doc to test, not a fact.
- Assumed Myelin *references* rather than *replaces* deep specialist authoring tools (e.g.
  design canvases like Figma); the depth of native design/whiteboard support is undecided.

**Hard open questions raised by these personas (for later phases):**
- **GDPR erasure vs. integrity (P13 + P1):** how to honour right-to-erasure of personal
  data (e.g. commit authorship, chat mentions) while preserving the integrity and
  immutability of code history and audit logs. This is a genuine, non-trivial tension —
  likely resolved via pseudonymisation/identity-indirection rather than literal deletion,
  but it needs an explicit, defensible design. **Deferred to shared-systems architecture.**
- **Agent identity & permission model (A1–A5 + P12):** the exact model for agent
  identities, delegated/on-behalf-of scopes, autonomy policy, and kill switches. Deferred
  to shared-systems architecture (identity/access + agent fabric).
- **Personal data in the reference/event graph (P13):** the cross-artifact graph and event
  bus will contain personal data; their residency, retention, and erasure behaviour need
  design. Deferred.
- **PM/engineering unified data model (P6 vs. P1):** how one underlying issue model
  presents as both a PM roadmap surface and an engineering board without satisfying neither.
  This is the issue tracker's central UX risk. Deferred to issue-tracker architecture.
- **Designer persona depth (P9):** native authoring vs. referencing — a product-scope
  decision deferred to positioning + knowledge-platform architecture.

**Deferred entirely (out of scope for Phase 1 personas):**
- Quantitative market sizing and segment prioritisation → positioning/competitive doc.
- Concrete pricing/packaging per archetype → later commercial/positioning work.
- Detailed permission matrices and role definitions → shared-systems architecture.
- Specific agent capability specs and mock implementations → architecture + build phases.

**Cross-references:**
- Competitive landscape & positioning (Phase 1) — will validate/refute segment priority
  and the EU-sovereign wedge.
- Use-case catalogue (Phase 1) — should derive concrete cross-subsystem flows from these
  personas (especially the cross-subsystem "glue" moments and agent triggers).
- Shared-systems architecture (Phase 3) — owns the identity/access, agent permissioning,
  audit, and GDPR-erasure designs these personas demand.
