# Competitive Landscape & Positioning

> Phase 1 research deliverable. Canonical brief: [`VISION.md`](../../VISION.md).
> Scope: map the alternatives Myelin can learn from and must position against — per
> subsystem (git, CI/CD, issues/PM, knowledge, chat) and as integrated suites — plus the
> EU-sovereignty market context. For each: **steal / avoid / wedge**. Ends with a crisp
> positioning statement and a differentiation table.

## 0. How to read this document

- **Steal** = a concrete idea, UX pattern, or architectural choice worth replicating.
- **Avoid** = a trap, anti-pattern, or strategic weakness we must not repeat.
- **Wedge** = where Myelin's three structural advantages — **agent-native**,
  **EU-sovereign**, **unified shared backend** — beat the incumbent.
- **Confidence flags**: facts I am reasonably sure of are stated plainly; anything
  uncertain, version-dependent, or time-sensitive is marked **[VERIFY]**. Funding,
  ownership, valuations, and exact feature availability move fast — treat all such numbers
  as indicative, not load-bearing.

A meta-point that shapes everything below: **no incumbent is simultaneously (a) a genuine
five-in-one unified suite, (b) agent-native by construction, and (c) credibly EU-sovereign.**
Each competitor is strong on at most one of these axes. That intersection is Myelin's wedge.

---

## 1. Git hosting & code review

### Landscape summary

| Product | Origin | Open source | Self-hostable | Notes |
|---|---|---|---|---|
| GitHub | US (Microsoft) | No (core) | Yes (GHE) | Market default; largest ecosystem |
| GitLab | US-HQ'd, originally NL/Ukraine roots | Open core | Yes (excellent) | Closest to "single app" suite |
| Gitea | Community | Yes (MIT) | Yes | Lightweight, fast |
| Forgejo | EU (Codeberg e.V., Germany) | Yes (copyleft) | Yes | Gitea hard-fork, EU-governed, sovereignty-friendly |
| Bitbucket | US (Atlassian) | No | Cloud + DC | Jira-coupled; Mercurial dropped |
| Azure DevOps Repos | US (Microsoft) | No | Server option | Enterprise, fading vs. GitHub |
| Sourcehut | EU (sr.ht, Drew DeVitt) | Yes | Yes | Minimalist, email-driven, no JS |

**[VERIFY]** GitLab is a US-listed public company (NASDAQ: GTLB); its cultural/founder
roots are partly European but it is **not** an EU-sovereign vendor today. Forgejo is
governed by Codeberg e.V., a German non-profit — the strongest EU/OSS-governance story in
this row.

### GitHub — what to steal
- **Pull request as the unit of collaboration**: diff + conversation + checks + reviewers
  + suggestions in one surface. This is the gold standard; Myelin's review UX must meet it.
- **Suggested changes** (reviewer proposes exact diff, author one-click applies).
- **CODEOWNERS** auto-review-routing.
- **Checks API** surfacing CI/external signals inline on the PR — clean separation that
  lets many producers post status to one consumer.
- **Code search** (the rebuilt semantic/symbol search) and the "press `t` to fuzzy-find a
  file" navigation.
- **GitHub-flavored Markdown** as a de-facto cross-artifact lingua franca, including
  `#123` issue/PR cross-references and `@mentions` — this is a primitive Myelin should
  generalize across *all five* subsystems via the reference graph.

### GitHub — what to avoid
- **Permissions sprawl**: org/team/repo permission model is historically confusing; later
  fine-grained PAT/app model is powerful but bolted on. Build one coherent model from day 1.
- **Notifications**: famously noisy and hard to tune. A known, solvable pain.
- **Closed core + US ownership** → disqualified for EU-sovereign buyers by construction.

### GitLab — what to steal
- **Single-app philosophy**: GitLab is the closest existing thing to Myelin's thesis (repo
  + CI + issues + wiki + registry in one). Study its information architecture and its
  failures (see §6).
- **Merge Request pipelines + "Merge when pipeline succeeds"** integration depth.
- **Built-in container/package registries** as first-class repo artifacts.

### GitLab — what to avoid
- **Bloat and performance**: monolithic Rails app, heavy to self-host at scale, UX can feel
  dense and slow. Myelin's "world-scale from day 1" + Rust steer is a direct answer.
- **Open-core feature-gating** friction: best features live behind paid tiers, which annoys
  self-hosters.

### Gitea / Forgejo — what to steal
- **Lightweight, fast, low-resource self-hosting** — proof a forge need not be heavy.
- **Forgejo specifically**: EU non-profit governance, copyleft, and active work on
  **federation (ActivityPub-based forge federation)** — directly relevant to sovereignty
  and to a future federated/multi-instance story. Worth tracking as both inspiration and
  potential interop target. **[VERIFY]** federation maturity.

### Bitbucket — what to steal / avoid
- Steal: tight Jira linking (commit/branch/PR ↔ issue smart-links). Myelin gets this *for
  free* because issues live in the same platform.
- Avoid: being defined by the adjacent product (Bitbucket exists largely to feed Jira);
  declining standalone investment; Mercurial removal burned trust.

### Azure DevOps Repos — what to steal / avoid
- Steal: enterprise-grade branch policies, required reviewers, build validation gates.
- Avoid: dated UX; Microsoft itself is steering customers to GitHub — a fading platform.

### Sourcehut — what to steal / avoid
- Steal: **radical performance and simplicity**, no-JS-required pages, email/patch
  workflow for those who want it, and a clean separation of services. A good North Star for
  "fast and uncluttered."
- Avoid: developer-purist ergonomics that alienate PMs and non-engineers — Myelin must
  serve product managers and corporate users too, so the Sourcehut aesthetic is a *value*,
  not a *UX template*.

### Myelin's git wedge
- **Review is an agent-native surface**: a mock (later real) reviewer agent is a
  first-class participant on the PR — same comment threads, same suggested-change
  primitive, same permissions — not a bot bolted via webhook.
- **Cross-artifact references are native**: a PR can first-class reference an issue, a
  knowledge doc (design spec), a CI run, and a chat thread through the shared reference
  graph, with bidirectional backlinks everywhere.
- **EU-sovereign self-hostable forge** with a coherent identity/permission model shared
  across all subsystems — neither GitHub (closed/US) nor GitLab (US-listed, heavy) offers
  this combination.

---

## 2. CI/CD

### Landscape summary

| Product | Origin | OSS | Self-host | Model |
|---|---|---|---|---|
| GitHub Actions | US (MS) | Runner OSS, service no | Self-hosted runners | YAML, marketplace of actions |
| GitLab CI | US | Open core | Yes | YAML `.gitlab-ci.yml`, integrated |
| CircleCI | US | No | Server (enterprise) | YAML, orbs; strong caching/parallelism |
| Buildkite | AU | Agent OSS, control plane SaaS | **Hybrid: you run agents, they run UI** | Pipelines + your own infra |
| Jenkins | US (CDF/OSS) | Yes | Yes | Plugins, Groovy; ubiquitous, legacy |
| Drone / Woodpecker | Woodpecker = community fork | Yes | Yes | Container-native, lightweight |
| Argo (Workflows/CD) | CNCF (OSS) | Yes | Yes (k8s-native) | GitOps, k8s-native pipelines/CD |

### What to steal (per tool)
- **GitHub Actions**: the **reusable composable action/marketplace** model and
  `workflow_dispatch`/event-driven triggers. The matrix build syntax. **But** the trigger
  model is the key idea for Myelin: pipelines triggered by *platform* events, not just git.
  Myelin should generalize this — any artifact event (issue transition, doc publish, chat
  command) can trigger a pipeline.
- **GitLab CI**: deeply **integrated** config living next to code, MR pipelines, manual
  gates, environments/deployments as first-class objects, child/parent pipelines.
- **CircleCI**: best-in-class **caching, test splitting, parallelism**, and **orbs**
  (versioned reusable config). **[VERIFY]** CircleCI now markets autonomous CI agents
  ("Chunk") and an OpenAI Codex plugin that inspects failed steps and proposes patch
  commits (2026) — strong signal that the whole industry is racing toward agent-native CI,
  validating our thesis and raising the bar.
- **Buildkite**: the **hybrid architecture** — control plane SaaS, **runners on your own
  infrastructure**. This is *gold* for EU sovereignty: compute and secrets never leave
  EU-controlled infra while still getting a great UI. Strong model to emulate.
- **Jenkins**: the lesson is the **plugin ecosystem's reach** (and its maintenance pain).
- **Drone/Woodpecker**: clean **container-native, lightweight** execution; Woodpecker's
  community-governed OSS posture is sovereignty-friendly.
- **Argo**: **GitOps + Kubernetes-native** declarative pipelines and CD; DAG workflows.
  Relevant if Myelin's CI targets k8s-native execution at world scale.

### What to avoid
- **YAML sprawl / config-as-stringly-typed-DSL**: every system here suffers from
  unmaintainable YAML. Consider a typed, validated, programmable config (with a simple
  declarative surface) — a real differentiator if done well.
- **Jenkins**: plugin dependency hell, Groovy, security CVE history, snowflake controllers.
- **Vendored secrets in SaaS**: forcing secrets/compute into the vendor's US cloud is the
  exact sovereignty failure Myelin must avoid → adopt Buildkite-style hybrid by design.
- **Marketplace supply-chain risk**: GitHub Actions' third-party action ecosystem has had
  serious supply-chain incidents; Myelin must design action provenance/pinning from day 1.

### Myelin's CI/CD wedge
- **Event-bus-triggered pipelines across all subsystems** (not just git pushes): the same
  trigger fabric reacts to issue, doc, and chat events. This is the literal embodiment of
  the VISION's "first-class event propagation and triggers."
- **Agent-native pipelines**: a mock (later real) agent can be a pipeline step, can be
  triggered by a failed build, can open a PR with a fix — using the *same* agent fabric and
  strategy-pattern seams as every other subsystem, not a separate bot product.
- **EU-sovereign execution by construction**: hybrid runner model where compute + secrets
  stay on EU infra; control plane is EU-hosted. No incumbent SaaS offers this turnkey.

---

## 3. Issue tracking & project management

This subsystem is the hardest because it must serve **engineers, product managers, AND
corporate/governance needs** simultaneously — a span no single incumbent nails.

### Landscape summary

| Product | Origin | OSS | Self-host | Best at |
|---|---|---|---|---|
| Jira | US (Atlassian) | No | Cloud + Data Center | Corporate workflows, configurability, reporting |
| Linear | US | No | No (SaaS only) | Engineering speed + UX |
| GitHub Issues / Projects | US (MS) | No | With GHE | Dev-proximate, lightweight |
| Azure Boards | US (MS) | No | Server | Enterprise Agile, traceability |
| Shortcut | US | No | No | Mid-weight eng PM |
| Asana | US | No | No | Cross-functional PM / tasks |
| monday.com | IL/US | No | No | Flexible "work OS", non-eng teams |

### Linear — what to steal (the UX North Star)
- **Speed**: local-first, optimistic, keyboard-driven; everything is instant. This is *the*
  reason teams switch. Myelin's tracker UX must feel like Linear.
- **Opinionated, minimal-config defaults** with **command palette (`Cmd-K`)** everywhere.
- **Cycles** (sprints) and **a clean triage inbox**.
- **Git integration**: branch names auto-link, PR merge auto-closes issue.
- **[VERIFY]** Linear has shipped AI features (auto-triage, agents) — confirm specifics.

### Linear — what to avoid
- **Not self-hostable, not EU-sovereign, opinionated to the point of inflexibility** for
  corporate workflows (custom fields, hierarchies, SLAs, audit). Linear *deliberately*
  doesn't serve the corporate/PM-governance end — which is exactly half of Myelin's mandate.

### Jira — what to steal (the enterprise-depth North Star)
- **Configurable workflows, custom fields, schemes, hierarchies** (epic/story/sub-task and
  beyond), **JQL** query language, **roadmaps/Advanced Roadmaps**, SLAs, audit logs,
  permission schemes, and reporting. This breadth is *why* corporations buy it.
- **Marketplace** extensibility (also a liability — see avoid).

### Jira — what to avoid
- **It is slow, heavy, over-configurable, and widely disliked.** "Jira" is a byword for
  enterprise-tool pain. The opportunity: deliver Jira's *power* with Linear's *speed and
  UX*. That synthesis is a genuine market gap.
- Confusing project/board/issue-type config; admin complexity; US/closed.

### GitHub Issues/Projects — steal / avoid
- Steal: **dev-proximate simplicity**, issues living next to code, `#`-cross-refs, task
  lists, Projects' flexible table/board/roadmap views over issues.
- Avoid: weak for true PM/corporate (limited hierarchy, reporting, SLAs); Projects is
  capable but not a Jira replacement for governance-heavy orgs.

### Azure Boards — steal / avoid
- Steal: **end-to-end traceability** (work item ↔ commit ↔ build ↔ release) and enterprise
  Agile process templates (Scrum/CMMI). Myelin gets traceability natively via the reference
  graph.
- Avoid: dated UX, Microsoft-stack lock-in.

### Shortcut — steal / avoid
- Steal: a balanced **eng-PM middle ground** (Stories, Epics, Iterations) lighter than Jira.
- Avoid: limited differentiation; small ecosystem.

### Asana / monday (PM side) — steal / avoid
- Steal: **non-engineer-friendly PM** — timelines/Gantt, portfolios, workload, custom
  views, automations, and **goal/OKR tracking** (the "corporate needs" surface). monday's
  flexible board-as-database and Asana's portfolio/goals are the patterns to learn for the
  PM persona.
- Avoid: these are *not* dev tools — no real code/CI integration; SaaS/US. Myelin's edge is
  putting the PM surface *on the same substrate* as code and CI.

### Myelin's issues/PM wedge
- **One tracker for engineers + PMs + corporate governance**: Linear-grade speed/UX,
  Jira-grade configurability (custom fields, hierarchies, SLAs, audit, reporting), and
  Asana/monday-grade roadmaps/OKRs — on a single schema. No incumbent spans all three.
- **Native traceability** (Azure Boards' big idea) is free: issue ↔ commit ↔ PR ↔ CI run ↔
  doc ↔ chat through the shared reference graph, fully bidirectional.
- **Agent-native PM**: mock (later real) agents triage, estimate, link duplicates, draft
  status reports, and react to event-bus transitions as first-class actors with auditable,
  permissioned actions.
- **EU-sovereign + self-hostable** — neither Jira Cloud, Linear, nor monday offers this.

---

## 4. Knowledge platform (Notion-class)

### Landscape summary

| Product | Origin | OSS | Self-host | Best at |
|---|---|---|---|---|
| Notion | US | No | No | Docs + databases, blocks, UX |
| Confluence | US (Atlassian) | No | Cloud + DC | Enterprise wiki, Jira-coupled |
| Coda | US | No | No | Docs-as-apps, formulas, packs |
| Obsidian | US, local-first | No (closed, free) | Local files | Markdown, graph, plugins |
| Outline | US, **OSS** | **Yes (BSL)** | **Yes** | Team wiki, clean, self-hostable |
| AFFiNE | **CN/community, OSS** | **Yes** | **Yes** | Notion+Miro hybrid, local-first |

(Brief lists "AffinE" — interpreting as **AFFiNE**, the open-source local-first
Notion/Miro alternative. **[VERIFY]** AFFiNE governance/origin.)

### Notion — what to steal (the North Star)
- **Block-based editor**: everything is a composable block; pages nest infinitely.
- **Databases as first-class** with multiple **views** (table, board, calendar, gallery,
  timeline) over the same data, plus relations/rollups. This database model is the single
  most important idea — and it directly parallels the **PM/issue views** in §3; Myelin
  should consider a **shared "database/views" primitive** spanning knowledge AND issues.
- **Slash commands**, drag-to-rearrange, `@`-mentions of people/pages/dates.
- **Folders/spaces/permissions** and beautiful, approachable UX.

### Notion — what to avoid
- **Performance at scale** (large workspaces get slow), **weak offline**, **SaaS-only/US**
  (GDPR concern), and **lock-in** (export is imperfect). Real-time collab is good but the
  data model is proprietary.

### Confluence — steal / avoid
- Steal: **enterprise wiki governance** — spaces, page hierarchies, permissions, page
  history/versioning, templates, and **deep Jira integration** (embed issues, requirement
  ↔ ticket links). The "knowledge ↔ work item" link is exactly what Myelin does natively.
- Avoid: clunky/dated editor, slowness, plugin reliance, US/closed.

### Coda — steal / avoid
- Steal: **docs-as-applications** — formulas, buttons, automations, and **packs**
  (integrations) that make a doc behave like a lightweight app. Powerful for "living"
  process docs.
- Avoid: steep learning curve; can over-engineer simple docs; SaaS/US.

### Obsidian — steal / avoid
- Steal: **local-first markdown**, **bidirectional links + graph view**, plugin
  extensibility, and **data ownership** (plain files). The backlink/graph paradigm maps
  perfectly onto Myelin's reference graph.
- Avoid: single-user-centric; weak native real-time multiplayer; closed (though free).

### Outline — steal / avoid (most relevant for sovereignty)
- Steal: **clean, fast, OSS, self-hostable team wiki** with good search and structure — the
  proof you can build a great knowledge tool that EU orgs can run themselves.
- Avoid: less powerful than Notion's database model; smaller ecosystem.

### AFFiNE — steal / avoid
- Steal: **open-source, local-first, Notion+whiteboard hybrid** — interesting for combining
  docs and canvas; sovereignty-friendly licensing.
- Avoid: younger/less mature; verify governance and EU-fit. **[VERIFY]**

### Myelin's knowledge wedge
- **Notion-class editor + database/views** that **shares the views primitive with the
  issue tracker** — one mental model for "structured data + multiple views" across PM and
  knowledge.
- **Knowledge is a first-class reference target**: a doc can be referenced by a PR, an
  issue, a CI run, or a chat message, with backlinks — turning the wiki into the
  organisation's living, linked source of truth instead of a stale silo.
- **Agent-native docs**: mock (later real) agents draft/update docs from events (e.g.
  auto-generate release notes from merged PRs + closed issues) via the event bus.
- **EU-sovereign, self-hostable Notion** — the single biggest unmet need for EU enterprises
  that love Notion's UX but cannot use it for GDPR/sovereignty reasons.

---

## 5. Chat

### Landscape summary

| Product | Origin | OSS | Self-host | Best at |
|---|---|---|---|---|
| Slack | US (Salesforce) | No | No (EKM partial) | Dev/work chat, apps, threads |
| Microsoft Teams | US (MS) | No | No | Enterprise, M365 bundle |
| Mattermost | US, **OSS** | **Yes** | **Yes** | Self-hosted Slack alt, gov/defense |
| Zulip | US, **OSS** | **Yes** | **Yes** | Threaded topics model |
| Discord | US | No | No | Communities, voice, presence |

### Slack — what to steal (the North Star)
- **Channels + threads + DMs**, **rich message composition**, **slash commands**, **app/bot
  framework**, **unfurling of links** into rich previews, **emoji reactions**, **Huddles**,
  and **search**. The **link-unfurl + slash-command** model is exactly how Myelin's chat
  should reference *any artifact* (commit/issue/doc/CI-run) — but native, not via per-app
  integrations.
- **Workflow Builder** (no-code automations) as an accessible automation surface.

### Slack — what to avoid
- **Channel sprawl + notification overload**; **search/retention behind paywalls**;
  **SaaS-only/US** (a hard GDPR blocker for many EU orgs); per-message-history limits
  historically frustrating.

### Microsoft Teams — steal / avoid
- Steal: **deep suite integration** (calls, meetings, files, M365) — the bundling lesson.
- Avoid: bloated, sluggish, confusing; wins on bundling not quality. A cautionary tale that
  *integration alone* doesn't guarantee good UX — Myelin must be integrated **and** fast.

### Mattermost — steal / avoid (most relevant for sovereignty)
- Steal: **OSS, fully self-hostable, EU-deployable**; trusted by **government/defense**;
  proves a Slack-class tool can be sovereign. Strong reference architecture for our chat.
- Avoid: UX historically trails Slack; plugin ecosystem smaller. Myelin must clear the UX
  bar Mattermost doesn't.

### Zulip — steal / avoid
- Steal: **topic-based threading model** — every message belongs to a *topic* within a
  stream, making async catch-up far better than Slack's flat channels. A genuinely better
  idea for high-volume, async, and **agent-heavy** conversations (agents generate volume).
- Avoid: learning curve; smaller ecosystem.

### Discord — steal / avoid
- Steal: **presence, low-friction voice, community feel, performance** at massive scale.
- Avoid: consumer/community framing, weak enterprise compliance/admin; US/closed.

### Myelin's chat wedge
- **Humans and agents in the same channels as equals** — the VISION's literal requirement.
  Mock (later real) agents post, are `@`-mentioned, respond to slash commands, and act on
  events using the same identity/permission model as humans. No incumbent treats agents as
  native channel members with real platform permissions.
- **Reference any artifact natively**: `#issue`, a commit, a doc, a CI run unfurl into rich,
  permission-aware, live previews via the shared reference graph — not brittle per-vendor
  integrations.
- **Zulip-style topics** considered to tame the volume that agent participation creates.
- **EU-sovereign, self-hostable Slack** with Mattermost's deployability and Slack's UX.

---

## 6. Integrated suites & the EU-sovereignty play

This is the crux. Myelin is not five tools; it is **one platform with shared backend
systems**. The relevant competition is the *suites*, and the strategic frame is *EU
digital sovereignty*.

### 6.1 The three integrated incumbents

**GitLab — "the single application for the entire DevOps lifecycle."**
- Closest existing thing to Myelin's thesis: repo + CI + issues + wiki + registry + (some)
  security, on shared identity.
- **Steal**: the integrated information architecture; the value of cross-stage linking;
  proof the market wants one tool. **[VERIFY]** GitLab now ships the **Duo Agent Platform**
  (planner agent, security agent, agentic chat) — a direct competitor moving toward
  agent-native, validating our direction and meaning we are *not* alone on that axis.
- **Avoid**: heavy monolith, dense UX, open-core gating, and — decisively — **US-listed,
  not EU-sovereign**, and **no knowledge-platform/chat** of Notion/Slack class. GitLab is
  DevOps-centric; it does not serve PMs/corporate or knowledge/chat the way Myelin must.
- **Myelin's edge over GitLab**: (1) two whole subsystems GitLab lacks (Notion-class
  knowledge, Slack-class chat); (2) EU-sovereign by construction; (3) agent-native from the
  ground up via one fabric/strategy pattern, not retrofitted; (4) world-scale Rust core vs.
  Rails monolith; (5) Linear/Notion-grade UX vs. dense enterprise UX.

**Atlassian suite (Jira + Confluence + Bitbucket + Bamboo/Pipelines + ... ).**
- **Steal**: the **cross-product linking** (issue ↔ page ↔ commit), enterprise governance
  depth, and the marketplace ecosystem. Atlassian proves enterprises pay for an integrated,
  governable suite.
- **Avoid**: products feel **stitched together, not unified** (separate data models,
  permission models, UIs); heavy; **Atlassian is sunsetting Server, pushing Cloud (US)** —
  **[VERIFY]** Data Center remains but the strategic direction is Cloud, a sovereignty
  problem; no first-class chat (they killed Stride/HipChat and partnered Slack); no modern
  agent-native fabric.
- **Myelin's edge**: genuinely unified (one identity, one permission model, one event bus,
  one reference graph) instead of integrated-by-API; EU-sovereign; agent-native; modern UX.

**Microsoft (GitHub + Teams + Azure DevOps + Loop + Planner + Copilot).**
- **Steal**: the breadth, **Copilot's deep IDE/agent integration**, and the sheer gravity
  of an everything-bundle.
- **Avoid**: it's a **bundle of separate products**, not one platform (GitHub, Teams, Azure
  DevOps, Loop each have different models/UX); **US-owned, CLOUD-Act-exposed** — the
  archetypal sovereignty risk; Teams UX bloat.
- **Myelin's edge**: one coherent platform vs. a federation of acquisitions; EU-sovereign
  vs. the canonical US hyperscaler; unified agent fabric vs. product-by-product Copilots.

### 6.2 EU digital sovereignty — market context

This is Myelin's strategic moat, and the macro environment is strongly tailwind.

- **The core legal problem**: US-headquartered cloud/SaaS vendors are subject to the **US
  CLOUD Act / FISA 702**, meaning US authorities can compel data access **even when data is
  stored in EU regions**. Therefore "AWS Frankfurt" / "Azure Germany" / data-residency-in-EU
  is **not** sufficient for true sovereignty — control and ownership matter, not just
  location. This is well established and a recurring theme in EU procurement debates.
  **[VERIFY (well-supported)]** — multiple 2025–2026 analyses reiterate that EU-region
  hosting by US hyperscalers does not satisfy sovereignty for sensitive workloads.
- **Gaia-X**: an EU federated-data-infrastructure initiative defining trust/portability/
  interoperability standards. **Important caveat: Gaia-X membership is NOT a sovereignty
  guarantee** — AWS, Azure, and Google are members; Gaia-X certifies portability/
  interoperability, not EU ownership or CLOUD-Act immunity. **[VERIFY]** Treat Gaia-X as a
  *standards/compliance signal to support*, not a marketing shield.
- **EU sovereign cloud providers** Myelin can target as deployment substrate: **OVHcloud
  (FR)**, **Deutsche Telekom / T-Systems Open Telekom Cloud (DE)**, **Scaleway (FR)**,
  **IONOS (DE)**, **Hetzner (DE)**, **Exoscale (CH)**, plus initiatives like **Delos Cloud
  (DE, sovereign Azure-stack operated under German control — [VERIFY])**. Myelin should be
  deployable on these and avoid hard dependencies on US-hyperscaler-only services.
- **Regulatory tailwind**: GDPR (the baseline), plus **EUCS** (EU Cybersecurity
  Certification Scheme for cloud), **NIS2**, **DORA** (financial-sector resilience), and the
  **Data Act / Data Governance Act**. **[VERIFY]** In **April 2026 the European Commission
  awarded a sovereign cloud framework contract (reported up to ~€180M / 6 yrs) to European
  providers** — the first time EU institutions applied explicit sovereignty criteria to
  cloud procurement. Strong evidence of real, funded demand.
- **Demand reality gap**: surveys (e.g. German industry) show ~80% *want* to reduce US-cloud
  dependence while ~80% *remain* dependent in practice — i.e. **stated demand vastly exceeds
  available sovereign alternatives.** That gap is Myelin's market. **[VERIFY exact figures]**

**What "EU-sovereign by construction" must mean for Myelin (design constraints):**
1. Deployable entirely on EU-controlled infrastructure (no hard US-hyperscaler dependency).
2. Data residency *plus* operational sovereignty (EU-controlled operators/keys; customer-
   held keys where possible — beyond mere region pinning).
3. GDPR data-subject rights as architecture: access, erasure, portability, lawful-basis
   tracking, full auditability — designed in, per VISION §3.
4. Open/self-hostable enough to be credibly independent of any single US vendor.
5. Alignable with EUCS/NIS2/DORA/Gaia-X to be procurement-ready for regulated buyers.

### 6.3 Where the integrated/sovereign field leaves a hole

- **Sovereign-but-not-unified**: Forgejo, Mattermost, Outline, Woodpecker are great
  EU/OSS-friendly *point* tools — but they're separate products with separate identity,
  permissions, and data models. Integrating them yourself recreates the Atlassian "stitched"
  problem.
- **Unified-but-not-sovereign**: GitLab, Atlassian, Microsoft are integrated (to varying
  degrees) but US-controlled.
- **Neither is agent-native by construction**: all are retrofitting agents onto
  product-specific surfaces (Duo, Copilot, Chunk) rather than a single cross-subsystem agent
  fabric with one identity/permission/event model.

**Myelin occupies the empty intersection: unified ∧ sovereign ∧ agent-native.**

---

## 7. Positioning statement

> **Myelin is the EU-sovereign software delivery platform where code, CI, issues,
> knowledge, and chat live on one identity model, one permission model, one event bus, and
> one agent fabric — so work flows between them without friction and autonomous agents are
> first-class citizens, not bolt-ons. It delivers Linear-and-Notion-grade UX with Jira-grade
> enterprise depth, runs entirely on EU-controlled infrastructure with GDPR compliance built
> in by construction, and is world-scalable from day one.**
>
> We are not "GitLab but European," nor "Notion plus Slack." We are the **connective
> tissue** — the one platform whose differentiator is the *integration itself*: the shared
> backend and the agent-native event fabric that no incumbent has, sold into the largest
> unmet demand in European software (digital sovereignty).

**One-liners by frame:**
- vs. GitLab: *"GitLab's integration, plus knowledge and chat, made EU-sovereign and
  agent-native — without the monolith."*
- vs. Atlassian: *"One unified platform, not a stitched-together suite — and your data never
  leaves EU control."*
- vs. Microsoft: *"Everything Microsoft bundles, as one coherent product you actually own —
  in Europe."*
- vs. best-of-breed point tools: *"Stop integrating five US SaaS tools; run one sovereign
  platform where they were one thing all along."*

---

## 8. Differentiation table

Legend: ●●● strong · ●●○ partial · ●○○ weak · ✕ absent.
"Unified" = single identity/permission/event/reference model across subsystems (not
integrated-by-API). "Agent-native" = one cross-subsystem agent fabric with native identity/
permissions/triggers (not per-product bots). "EU-sovereign" = deployable on EU-controlled
infra free of US-hyperscaler/CLOUD-Act dependency.

| Platform | Git | CI/CD | Issues/PM | Knowledge | Chat | **Unified** | **Agent-native** | **EU-sovereign** | OSS / self-host |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| **Myelin** | ●●● | ●●● | ●●● | ●●● | ●●● | **●●●** | **●●●** | **●●●** | Yes / Yes |
| GitHub (MS) | ●●● | ●●● | ●●○ | ●○○ | ✕ | ●●○ | ●●○ (Copilot) | ✕ | No / GHE |
| GitLab | ●●● | ●●● | ●●○ | ●○○ | ✕ | ●●● | ●●○ (Duo) | ✕ | Open-core / Yes |
| Atlassian suite | ●●○ | ●●○ | ●●● | ●●○ | ✕ | ●●○ | ●○○ | ●○○ (DC) | No / DC |
| Microsoft (GH+Teams+ADO) | ●●● | ●●● | ●●○ | ●●○ | ●●● | ●○○ | ●●○ (Copilot) | ✕ | No / partial |
| Linear | ✕ | ✕ | ●●● | ●○○ | ✕ | ✕ | ●●○ | ✕ | No / No |
| Notion | ✕ | ✕ | ●●○ | ●●● | ✕ | ✕ | ●●○ | ✕ | No / No |
| Slack (Salesforce) | ✕ | ✕ | ✕ | ●○○ | ●●● | ✕ | ●●○ | ✕ | No / No |
| Jira | ✕ | ●○○ | ●●● | ●●○ | ✕ | ●○○ | ●○○ | ●○○ (DC) | No / DC |
| **Forgejo** | ●●● | ●●○ | ●●○ | ●○○ | ✕ | ●○○ | ✕ | **●●●** | **Yes / Yes** |
| **Mattermost** | ✕ | ●○○ | ●○○ | ●○○ | ●●● | ✕ | ●○○ | **●●●** | **Yes / Yes** |
| **Outline** | ✕ | ✕ | ✕ | ●●○ | ✕ | ✕ | ✕ | **●●●** | **Yes / Yes** |
| **Woodpecker** | ✕ | ●●○ | ✕ | ✕ | ✕ | ✕ | ✕ | **●●●** | **Yes / Yes** |

Ratings are judgement calls for positioning, not benchmark scores. **[VERIFY]** the
agent-native column especially — vendors are shipping agent features fast (GitLab Duo,
GitHub/Microsoft Copilot, CircleCI Chunk), so the *gap is narrowing on that axis*; Myelin's
durable edge is the **combination** with unified + sovereign, which none of them hold.

---

## 9. Assumptions, uncertainties & deferred items

**Assumptions**
- "AffinE" in the brief = **AFFiNE** (OSS Notion/Miro hybrid).
- Drone is included via its actively-maintained community fork **Woodpecker**.
- EU-sovereign demand persists/strengthens through Myelin's build horizon (well-supported by
  current policy direction, but a macro/political assumption).

**Flagged uncertainties (do not treat as load-bearing without checking)**
- All **funding/ownership/valuation/employee numbers** and **specific 2025–2026 AI-agent
  feature names** (GitLab Duo Agent Platform, CircleCI "Chunk"/Codex plugin, Linear AI) —
  fast-moving; verify before external use.
- **Atlassian Server/Data-Center sunset specifics** and current self-host options.
- **Forgejo federation** maturity; **AFFiNE** governance/EU-fit.
- **EU sovereign-cloud facts**: the ~€180M April-2026 EC framework contract, the ~80/80
  German-industry demand-gap figures, **Delos Cloud** operating model, and exact
  EUCS/NIS2/DORA scope — directionally solid, exact figures **[VERIFY]**.
- The differentiation-table ratings are positioning judgements, not measured benchmarks.

**Deferred (out of scope for this doc; belongs to later phases)**
- Pricing/packaging and go-to-market strategy.
- Detailed feature-by-feature parity matrices per subsystem.
- Migration/import tooling from incumbents (a likely adoption wedge — flag for roadmap).
- Concrete reference deployment architectures on named EU clouds (architecture phases).
- Legal/compliance certification roadmap (EUCS/NIS2/DORA/Gaia-X labelling).

---

## 10. Cross-references
- [`VISION.md`](../../VISION.md) — canonical brief (subsystems, non-negotiables).
- Companion Phase-1 docs (personas, use-case catalogue, technical structuring) should align
  the **shared backend** (identity, event bus, agent fabric, reference graph, search,
  notifications) called out as Myelin's wedge throughout this document.
- The **shared "database/views" primitive** noted in §3 and §4 (issues and knowledge sharing
  one model) is a concrete cross-subsystem design hypothesis to carry into architecture.

---

### Sources (web-verified context)
- European Commission — Cloud Sovereignty Framework: https://data-en-maatschappij.ai/en/publications/europese-commissie-een-kader-voor-cloudsoevereiniteit-bij-strategische-aanbesteding
- Gaia-X: https://gaia-x.eu/
- Digital sovereignty 2026 (Gaia-X / Delos): https://www.digital-chiefs.de/en/digital-sovereignty-2026-gaia-x-delos-cloud-and-europes-response-to-the-cloud-ac/
- European sovereign cloud provider landscape 2026: https://www.softwareseni.com/the-european-sovereign-cloud-provider-landscape-in-2026-who-exists-and-what-they-offer/
- Cloud sovereignty crisis / CLOUD Act (Akave): https://akave.com/blog/europes-digital-sovereignty-dilemma-can-the-continent-break-free-from-us-cloud-dominance
- Gaia-X warns US hyperscalers on selling sovereignty: https://www.channeldive.com/news/gaia-x-warns-us-hyperscalers-about-selling-sovereignty/807362/
- GitLab Duo Agent Platform docs: https://docs.gitlab.com/user/duo_agent_platform/
- CircleCI AI/agents: https://circleci.com/solutions/ai-agents/
- Company profiles (funding/ownership, indicative): Tracxn / PitchBook (GitLab, CircleCI, Notion)
