# Myelin — Use-Case Catalogue

> Phase 1 research deliverable. Canonical brief: [`VISION.md`](../../VISION.md).
> Derived from [`personas.md`](./personas.md). This document enumerates *what people and
> agents need to do* on Myelin, so later phases (architecture, roadmaps, prompts) can be
> grounded in concrete, testable flows rather than abstractions.

## 0. How to read this document

Myelin is **one platform, five subsystems** — git hosting (**Git**), CI (**CI**), issue
tracker (**Issues**), knowledge platform (**Knowledge**), chat (**Chat**) — unified by
**Shared** backend systems: identity & access (**Id**), event bus (**Bus**), agent fabric
(**Agents**), storage, search (**Search**), notifications (**Notif**), and the
cross-artifact reference graph (**Refs**). Where a use case touches the audit/compliance
surfaces specifically, we tag **Audit** and **GDPR** as facets of Shared.

The catalogue is organised so that each concern in scope is addressed:

1. **§1 Conventions** — id scheme, columns, autonomy notation.
2. **§2 Per-subsystem use cases** — the bread-and-butter, organised by subsystem (so each
   subsystem architecture agent has a checklist). Personas annotated per row.
3. **§3 Cross-subsystem use cases** — *the wedge*. Flows that only Myelin can do because
   the subsystems share identity, a bus, and a reference graph. These are the highest-value
   and most differentiating rows.
4. **§4 Agent-driven use cases** — agents as actors (A1–A5), triggered by events.
5. **§5 Corporate / compliance / GDPR use cases** — the gatekeeper flows (P11–P15) that
   decide adoption and lawfulness.
6. **§6 Scale, migration, offboarding, incident & "non-obvious" use cases** — the edge and
   operational cases that are easy to forget and expensive to retrofit.
7. **§7 Prioritisation view** — MVP vs. later, with rationale.
8. **§8 Implications for the shared systems** — what these use cases *require* of Id, Bus,
   Refs, Search, Notif, Audit/GDPR.
9. **§9 Open questions, assumptions, and deferrals** — honesty about uncertainty.

**Uncertainty flag (read first).** Use cases are derived from the personas (themselves
unvalidated hypotheses) plus strong domain knowledge of how these tools are used. The
*existence* of these flows is well-evidenced; the *relative priority* and several
behavioural details (especially GDPR-erasure-vs-integrity, agent autonomy boundaries, and
design-tool depth) are **not settled** and are flagged inline and in §9. Where I suspect a
use case exists but cannot fully specify it, I mark it **(under-specified)**.

---

## 1. Conventions

**ID scheme.** `UC-<area>-<n>` where `<area>` is one of `GIT`, `CI`, `ISS`, `KN`, `CHAT`
(subsystems), `X` (cross-subsystem), `AG` (agent-driven), `CORP` (corporate/compliance),
`EDGE` (scale/migration/offboarding/incident/non-obvious). Ids are stable handles for later
phases to cite.

**Table columns.**
- **ID / Title** — stable id and short name.
- **Personas** — primary actor(s); see `personas.md` (P1–P15 humans, A1–A5 agents).
- **Goal** — the user's intent, in one line.
- **Subsystems** — which subsystems + shared facets the case exercises. **Bold** = the
  primary surface; others are touched.
- **Pri** — priority: **M** (MVP must-have), **N** (near-term, post-MVP), **L** (later).
  Rationale aggregated in §7; per-row marks let architects see weight at a glance.

**Autonomy notation (for agent rows).** `propose` = agent drafts, human approves;
`act-low-risk` = agent acts autonomously within policy on reversible/low-stakes actions;
`gated` = agent may perform high-stakes action only under explicit per-scope policy;
`escalate` = agent hands to a human. Per the vision and `personas.md §5`, **human-in-the-
loop is the default**; autonomy is a per-action, per-scope policy owned by P12/P15.

**A note on "world-scale."** Many rows below are trivial for 5 users and hard for 5
million. Where scale changes the shape of the use case (search, notifications, event fan-
out, permission checks, the reference graph), it is called out in §6 and §8 rather than
repeated per row.

---

## 2. Per-subsystem use cases

### 2.1 Git hosting

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-GIT-1** Clone/push over HTTPS & SSH | P1–P5, A1 | Get code in/out with fast, authenticated git operations | **Git**, Id | M |
| **UC-GIT-2** Browse code, history, blame | P1–P5, P10 | Read code, trace why a line exists, navigate history | **Git**, Search, Refs | M |
| **UC-GIT-3** Open / review / merge a pull (merge) request | P1–P5, A1, A3 | Propose, review, and integrate a change | **Git**, CI, Chat, Refs | M |
| **UC-GIT-4** Inline + threaded review comments | P1–P5, P9, A3 | Discuss specific lines; resolve threads | **Git**, Chat, Notif | M |
| **UC-GIT-5** Branch protection & merge policy | P3, P12, P4 | Require reviews/checks/signatures before merge | **Git**, CI, Id, Audit | M |
| **UC-GIT-6** Required status checks gate merge | P1–P3, P12 | Block merge until CI/agents pass | **Git**, CI, Bus | M |
| **UC-GIT-7** Repo/org/team structure & permissions | P3, P11, P15 | Organise repos under orgs/teams with scoped access | **Git**, Id | M |
| **UC-GIT-8** Fork & contribute (incl. from a stranger) | P5, P1 | External contributor proposes a change safely | **Git**, CI, Id | N |
| **UC-GIT-9** Tags, releases, changelogs | P3, P10, P6 | Cut a release; publish notes tied to merged work | **Git**, Knowledge, Issues, Refs | N |
| **UC-GIT-10** Commit signing & verified authorship | P12, P1 | Cryptographically attest who authored/merged | **Git**, Id, Audit | N |
| **UC-GIT-11** Large files / monorepo ergonomics | P1, P3 | Handle big repos and binary assets without pain | **Git**, storage | N |
| **UC-GIT-12** Code search across repos | P1–P5 | Find a symbol/string across the org's code | **Git**, **Search** | N |
| **UC-GIT-13** Protected-branch CI for fork PRs (secret-safe) | P5, P3, P12 | Run CI on untrusted forks without leaking secrets | **Git**, CI, Id, Audit | N |
| **UC-GIT-14** Web-based edit / suggestion commit | P10, P6, P9 | Non-CLI users propose small doc/code edits | **Git**, Knowledge | N |
| **UC-GIT-15** Mirror / import a repo (with history) | P15, P3 | Bring an existing repo in, preserving history | **Git**, EDGE-migration | N |
| **UC-GIT-16** Repo archival / read-only / transfer | P15, P11 | Retire or move a repo without losing references | **Git**, Refs, Audit | N |
| **UC-GIT-17** Per-commit / per-PR reference resolution | P1–P4 | See the issue/doc/run a commit relates to inline | **Git**, **Refs**, Issues, Knowledge, CI | M |

> **Uncertainty (Git):** native protocol choice (smart HTTP/SSH, pack negotiation at
> scale), and monorepo strategy (e.g. partial clone, sparse) are architecture decisions, not
> use-case decisions — but UC-GIT-11/12 *imply* them and are flagged for the Git architecture
> agent.

### 2.2 Continuous Integration (CI)

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CI-1** Define a pipeline (as code) | P1–P3 | Declare build/test/deploy steps versioned with the repo | **CI**, Git | M |
| **UC-CI-2** Trigger on repo + platform events | P1–P3, P5 | Run pipelines on push/PR and on platform events | **CI**, **Bus**, Git | M |
| **UC-CI-3** View live + historical run logs | P1–P3 | Watch a run, debug a failure from logs | **CI**, Search | M |
| **UC-CI-4** Report status back to the PR | P1–P3 | Surface pass/fail as a merge gate on the PR | **CI**, Git, Bus | M |
| **UC-CI-5** Secrets & credentials management | P3, P12 | Inject secrets into runs with least privilege + audit | **CI**, Id, Audit | M |
| **UC-CI-6** Caching & artifacts between stages/runs | P1, P3 | Speed builds; pass build outputs downstream | **CI**, storage | N |
| **UC-CI-7** Matrix / parallel / fan-out builds | P1, P3 | Test across versions/platforms efficiently | **CI**, Bus | N |
| **UC-CI-8** Manual approval / deployment gates | P3, P7, P12 | Require human approval before a protected step | **CI**, Id, Notif, Audit | N |
| **UC-CI-9** Self-hosted / scalable runner pools | P3, P15 | Run on org-controlled (EU) compute; autoscale | **CI**, Id, GDPR | M |
| **UC-CI-10** Flaky-test detection & quarantine | P1, P3, A5 | Identify and isolate flaky tests automatically | **CI**, Issues, Bus | L |
| **UC-CI-11** Supply-chain controls (SBOM, provenance, pinned deps) | P12, P3 | Produce attestations; control dependency risk | **CI**, Audit, Git | N |
| **UC-CI-12** Preview / ephemeral environments per PR | P2, P9, P6 | Spin up a live preview for review | **CI**, Git, Chat | N |
| **UC-CI-13** Scheduled / cron pipelines | P3 | Run periodic jobs (nightly, cleanup, scans) | **CI**, Bus | N |
| **UC-CI-14** Pipeline observability & cost view | P3, P11, P15 | See run times, queue depth, spend by team | **CI**, Notif, reporting | N |
| **UC-CI-15** Deploy / rollback with audit trail | P3, A5, P12 | Ship and revert with who/what/when recorded | **CI**, Audit, Issues, Bus | N |

### 2.3 Issue tracker

The vision's hardest UX bet: **one underlying model serving engineers AND PMs AND corporate
needs**. Rows are split so the "engineer view" and "PM/portfolio view" over the *same data*
are both visible.

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-ISS-1** Create / edit / comment on an issue | P1–P10, A2 | Capture a unit of work or a bug with discussion | **Issues**, Chat, Notif, Refs | M |
| **UC-ISS-2** Assign, label, prioritise, set status | P1–P8, A2 | Make an issue actionable and findable | **Issues**, Id, Bus | M |
| **UC-ISS-3** Board / sprint (engineer) view | P1–P3, P7 | Plan and track iteration work on a board | **Issues**, reporting | M |
| **UC-ISS-4** Roadmap / now-next-later (PM) view | P6, P8, P11 | See the *same* work as outcomes over time | **Issues**, Knowledge, reporting | M |
| **UC-ISS-5** Hierarchies (epic → story → task → sub) | P6, P7, P8 | Decompose and roll up work | **Issues**, Refs | M |
| **UC-ISS-6** Cross-issue dependencies & blocking | P8, P4, P7 | Model and surface what blocks what | **Issues**, **Refs**, Notif | N |
| **UC-ISS-7** Custom fields / workflows per project | P6, P7, P15 | Adapt the tracker to a team's process | **Issues**, Id | N |
| **UC-ISS-8** Saved queries / filtered views | P4, P7, P8 | Slice the backlog by any dimension | **Issues**, **Search** | M |
| **UC-ISS-9** Portfolio / cross-project / cross-team view | P8, P11, P4 | One view spanning many projects/repos | **Issues**, reporting, Refs | N |
| **UC-ISS-10** Delivery analytics (cycle time, WIP, flow) | P7, P11, P8 | Understand flow/health from the event stream | **Issues**, **Bus**, reporting | N |
| **UC-ISS-11** SLAs / due dates / escalation | P7, P12, P5 | Track and escalate time-bound commitments | **Issues**, Notif, Bus | L |
| **UC-ISS-12** Triage queue / intake management | P5, P6, A2 | Process a noisy inbound stream into clean work | **Issues**, Chat, Search, Agents | N |
| **UC-ISS-13** Link issue ↔ PR ↔ doc ↔ chat ↔ CI run | P1–P8 | Tie intent to delivery to evidence | **Issues**, **Refs**, all | M |
| **UC-ISS-14** Convert chat/report/feedback → issue | P5, P6, P7, A2 | Promote a conversation into tracked work | **Issues**, Chat, Refs | N |
| **UC-ISS-15** Incident issues with timeline & roles | P3, A5, P12 | Run an incident as a structured, audited record | **Issues**, Chat, CI, Audit | N |
| **UC-ISS-16** Status roll-up / auto-status reports | P6, P7, P8, A2 | Generate trustworthy status without chasing | **Issues**, **Bus**, Knowledge | N |
| **UC-ISS-17** Public vs. private issue visibility | P5, P12 | Mix open community triage with private governance | **Issues**, Id | N |
| **UC-ISS-18** Customer/stakeholder feedback capture | P6, P5 | Funnel external feedback to prioritisation | **Issues**, Chat, GDPR | L |

> **Central UX risk (carried from `personas.md §7`):** UC-ISS-3 vs. UC-ISS-4 over one data
> model is the issue tracker's make-or-break. Flagged for the Issues architecture agent. The
> use-case requirement is: *the same issue must be a first-class citizen of both a sprint
> board and a roadmap without either view being a second-class projection.*

### 2.4 Knowledge platform

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-KN-1** Create rich-text doc (Notion-class block editor) | P4, P6, P9, P10 | Author structured rich content | **Knowledge**, Search | M |
| **UC-KN-2** Folders / spaces / hierarchy & nesting | P10, P6, P15 | Organise knowledge into navigable structure | **Knowledge**, Id | M |
| **UC-KN-3** Tables, lists, embedded blocks | P6, P8, P10 | Express structured info inline | **Knowledge** | M |
| **UC-KN-4** Databases (structured, queryable collections) | P6, P8, P9 | Notion-style databases with views/filters | **Knowledge**, Search | N |
| **UC-KN-5** Real-time / concurrent collaborative editing | P4, P6, P9, P10 | Co-edit a doc live without conflicts | **Knowledge**, Bus, Notif | N |
| **UC-KN-6** Comments, mentions, suggestions on docs | P4, P6, P9, P10 | Discuss and propose edits in-context | **Knowledge**, Chat, Notif, Id | M |
| **UC-KN-7** Doc version history & restore | P10, P4, P12 | See and revert document changes | **Knowledge**, Audit | N |
| **UC-KN-8** Reference any artifact from a doc | P4, P6, P10 | Embed/link issues, PRs, runs, channels live | **Knowledge**, **Refs**, all | M |
| **UC-KN-9** PRD / RFC / ADR / runbook templates | P4, P6, P3 | Capture durable decisions in a standard shape | **Knowledge**, Issues, Refs | N |
| **UC-KN-10** Docs-as-code coupling to Git | P10, P1 | Keep docs in sync with the code they describe | **Knowledge**, Git, Bus | N |
| **UC-KN-11** Full-text + semantic doc search | P4, P10, all | Find the right doc/section fast | **Knowledge**, **Search** | M |
| **UC-KN-12** Public knowledge base / external docs | P5, P10 | Publish docs to the public web cleanly | **Knowledge**, Id, GDPR | L |
| **UC-KN-13** Stale-doc detection & freshness signal | P10, A4 | Flag docs likely outdated by recent changes | **Knowledge**, Bus, Agents | N |
| **UC-KN-14** Permissions per space/page (incl. inheritance) | P15, P12, P6 | Control who reads/edits which knowledge | **Knowledge**, Id | M |
| **UC-KN-15** Export / portability of knowledge | P14, P15 | Get knowledge out in open formats (anti-lock-in) | **Knowledge**, GDPR | N |
| **UC-KN-16** Reference / embed external design tools | P9, P2 | Link Figma etc. as first-class references | **Knowledge**, Refs | N |

> **Scope uncertainty (Knowledge, from `personas.md §7`):** native design *authoring* vs.
> *referencing* (P9) is undecided. UC-KN-16 reflects the conservative "reference" stance;
> deeper authoring (whiteboards, canvases) is **(under-specified)** and deferred to
> positioning + Knowledge architecture.

### 2.5 Chat

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CHAT-1** Channels (team/project/topic) + DMs | all, A1–A5 | Converse in organised spaces | **Chat**, Id, Notif | M |
| **UC-CHAT-2** Threads & replies | all | Keep focused sub-conversations | **Chat**, Notif | M |
| **UC-CHAT-3** Reference/embed any artifact in a message | all | Drop a live issue/PR/doc/run into chat | **Chat**, **Refs**, all | M |
| **UC-CHAT-4** Mentions, presence, read state | all | Get the right person's attention | **Chat**, Id, Notif | M |
| **UC-CHAT-5** Humans + agents in the same channel | all, A1–A5 | Talk to agents like teammates | **Chat**, **Agents**, Bus | M |
| **UC-CHAT-6** Search across chat history | all | Find a past decision/message | **Chat**, **Search** | M |
| **UC-CHAT-7** Event-driven channel posts (bot-style) | P3, A5 | Pipe platform events into a channel | **Chat**, **Bus**, Notif | M |
| **UC-CHAT-8** Convert message → issue / doc / decision | P5, P6, P7, A2 | Promote conversation into durable artifacts | **Chat**, Issues, Knowledge, Refs | N |
| **UC-CHAT-9** Incident / war-room channels | P3, A5, P12 | Coordinate an incident with linked timeline | **Chat**, Issues, CI, Audit | N |
| **UC-CHAT-10** Channel permissions & private/public scope | P15, P12, P5 | Control visibility of conversations | **Chat**, Id | M |
| **UC-CHAT-11** Slash-commands / agent invocation in chat | P1–P7, A1–A5 | Trigger an agent/action from chat | **Chat**, Agents, Bus | N |
| **UC-CHAT-12** Notification routing & quiet hours | P7, all | Control what pings whom and when | **Chat**, **Notif**, Id | N |
| **UC-CHAT-13** Retention / export / e-discovery of chat | P12, P13, P14 | Manage chat data lifecycle lawfully | **Chat**, GDPR, Audit | N |

---

## 3. Cross-subsystem use cases — the wedge

These are the flows that justify Myelin existing as *one platform*. Each is hard or
impossible in today's fragmented stack precisely because it requires **shared identity, a
shared event bus, and a shared reference graph**. These are the highest-value rows.

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-X-1** Spec-to-ship traceability | P4, P6, P10 | From PRD (Knowledge) → epic/issues → PRs (Git) → CI evidence → release, all linked and live | **Refs**, Knowledge, Issues, Git, CI | M |
| **UC-X-2** Unified cross-artifact search | P1–P10 | One search returns commits, issues, docs, chat, runs ranked together | **Search**, all | M |
| **UC-X-3** PR pane shows full context | P1–P4, A3 | A PR surfaces its issue, the relevant doc section, CI run, and discussion inline — no tab-hopping | **Git**, Issues, Knowledge, CI, Chat, **Refs** | M |
| **UC-X-4** Event-driven automation ("when X do Y") | P3, P15, A5 | First-class, observable triggers replacing webhook glue | **Bus**, all | M |
| **UC-X-5** Roadmap reflects real delivery state | P6, P8, P11 | PM roadmap pulls live status from the same issue/PR/CI data engineers produce | **Issues**, Git, CI, **Bus**, reporting | M |
| **UC-X-6** One identity & permission model everywhere | all, P12, P15 | A user/team/agent has one identity and consistent scoped access across all five subsystems | **Id**, all | M |
| **UC-X-7** Unified, smart notification inbox | P7, all | One prioritised "what needs *me*" inbox across subsystems, not five firehoses | **Notif**, **Bus**, all | M |
| **UC-X-8** Reference graph never rots | P1, P4, P10 | A link from chat to an issue stays meaningful months later (resolves to current state) | **Refs**, all | M |
| **UC-X-9** Merged PR flags docs needing update | P10, A4 | A code change signals the docs (Knowledge) it likely invalidates | **Bus**, Git, Knowledge, Agents | N |
| **UC-X-10** Release notes assembled from linked work | P6, P10, A4 | Changelog generated from merged PRs + issues + decisions | **Refs**, Git, Issues, Knowledge | N |
| **UC-X-11** Incident command across subsystems | P3, A5, P12 | Incident issue + war-room chat + CI/deploy events + runbook doc, one linked timeline | Issues, Chat, CI, Knowledge, **Bus**, Audit | N |
| **UC-X-12** Design intent ↔ code ↔ approval thread | P2, P9, P6 | Trace which design decision a code change implements and who approved it | **Refs**, Knowledge, Git, Issues, Chat | N |
| **UC-X-13** Cross-cutting "everything touching subsystem Z" | P4, P8 | Answer "all work/code/docs/decisions touching Z" in seconds | **Search**, **Refs**, all | N |
| **UC-X-14** Org-wide delivery health from one event stream | P7, P11 | Trustworthy analytics drawn from the bus, not bolted-on integrations | **Bus**, Issues, Git, CI, reporting | N |
| **UC-X-15** Feedback → triage → roadmap → ship loop | P5, P6, A2 | A community/customer report flows to triage, prioritisation, delivery, and back | Chat, Issues, Knowledge, **Refs**, Agents | N |
| **UC-X-16** Single audit trail across all artifacts | P12, P13 | Every action (human + agent) on any artifact is in one queryable, tamper-evident log | **Audit**, **Id**, all | M |
| **UC-X-17** One data-subject view across subsystems | P13 | "Everywhere this person's personal data appears" in one inventory | **GDPR**, **Id**, **Refs**, all | N |

> **Why these are the wedge:** every UC-X row collapses a multi-tool, multi-login, copy-
> paste workflow into one. The dependency is always the same triad: **Id** (so access is
> consistent), **Bus** (so events propagate), **Refs** (so links are live and bidirectional).
> §8 turns these into concrete requirements on the shared systems.

---

## 4. Agent-driven use cases

Per the vision and `personas.md §5`: agents are **first-class identities with scoped, least-
privilege permissions and full audit trails**, activated by **triggers on the event bus**,
**human-in-the-loop by default**. During development they are **mock implementations behind
the strategy pattern** — what matters here is that the *interaction model* (trigger →
scoped action → audit → optional human gate) is designed correctly so real agents drop in
later. The "Autonomy" column uses §1 notation.

### 4.1 Coding agent (A1)

| ID / Title | Trigger (event) | Goal | Subsystems | Autonomy | Pri |
|---|---|---|---|---|---|
| **UC-AG-1** Draft PR from a ready issue | Issue marked ready/assigned to A1 | Turn a well-specified issue into a draft PR with tests | Git, CI, Issues, **Bus** | propose | N |
| **UC-AG-2** Respond to review feedback | Review comment / changes requested | Update the PR to address feedback | Git, CI, Chat | propose | N |
| **UC-AG-3** Ask a clarifying question | Ambiguity detected in spec | Unblock itself by asking in chat/issue | Chat, Issues | act-low-risk | N |
| **UC-AG-4** Keep a branch green | CI failed on agent's PR | Diagnose and push a fix attempt | Git, CI, **Bus** | propose | L |

### 4.2 Triage agent (A2)

| ID / Title | Trigger (event) | Goal | Subsystems | Autonomy | Pri |
|---|---|---|---|---|---|
| **UC-AG-5** Auto-label & route new issue | Issue created | Classify, label, assign to the right team | Issues, **Bus**, Id | act-low-risk | N |
| **UC-AG-6** Deduplicate against existing issues | Issue created/updated | Flag/merge likely duplicates | Issues, **Search**, **Refs** | propose | N |
| **UC-AG-7** Request missing info | Issue lacks repro/details | Comment asking for required fields | Issues, Chat | act-low-risk | N |
| **UC-AG-8** Summarise inbound report | Chat report / feedback intake | Produce a clean issue from a messy report | Issues, Chat, **Refs** | propose | N |
| **UC-AG-9** Propose stale-issue closes | Staleness timer | Suggest closing inactive issues (human confirms) | Issues, **Bus** | propose | L |

### 4.3 Review agent (A3)

| ID / Title | Trigger (event) | Goal | Subsystems | Autonomy | Pri |
|---|---|---|---|---|---|
| **UC-AG-10** Mechanical PR review | PR opened/updated | Flag style, obvious bugs, missing tests | Git, CI, Chat | propose | N |
| **UC-AG-11** Security / license / dependency gate | PR opened; CI complete | Block-or-warn on policy violations | Git, CI, **Audit** | gated (P12 policy may block) | N |
| **UC-AG-12** Summarise a large PR for humans | PR opened | Give reviewers a high-signal summary | Git, Chat | act-low-risk | L |

### 4.4 Knowledge-curation agent (A4)

| ID / Title | Trigger (event) | Goal | Subsystems | Autonomy | Pri |
|---|---|---|---|---|---|
| **UC-AG-13** Flag stale docs from merged changes | PR merged; staleness timer | Identify docs the change likely invalidates | Knowledge, **Bus**, **Refs** | propose | N |
| **UC-AG-14** Suggest doc edits from a change | PR merged | Draft a doc update reflecting the change | Knowledge, Git | propose | L |
| **UC-AG-15** Maintain reference graph links | Doc/issue/PR edited | Keep cross-references accurate and rich | **Refs**, Knowledge | act-low-risk (links only) | N |
| **UC-AG-16** Generate changelog/summary | Release tagged | Assemble release notes from linked work | Knowledge, Git, Issues, **Refs** | propose | L |

### 4.5 Ops / SRE agent (A5)

| ID / Title | Trigger (event) | Goal | Subsystems | Autonomy | Pri |
|---|---|---|---|---|---|
| **UC-AG-17** Detect & open an incident | CI failure / alert event | Create an incident issue + war-room channel | Issues, Chat, CI, **Bus** | act-low-risk | N |
| **UC-AG-18** Run runbook-safe remediation | Known failure signature | Execute pre-approved runbook step | CI, Knowledge (runbook), **Audit** | gated | L |
| **UC-AG-19** Post status & escalate | Incident open; threshold crossed | Update channel, page humans if outside runbook | Chat, **Notif** | escalate | N |
| **UC-AG-20** Quarantine a flaky test | Flakiness detected | Isolate the test, open a tracking issue | CI, Issues | propose | L |
| **UC-AG-21** Deploy / rollback under policy | Deploy event / rollback signal | Ship or revert only under explicit policy | CI, **Audit**, Issues | gated | L |

### 4.6 Cross-cutting agent governance use cases (non-obvious, but essential)

These exist *because* agents are first-class. They are mostly P12/P13/P15 concerns and are
easy to forget until an auditor or DPO asks.

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-AG-22** Register / configure an agent identity | P15, P12 | Create an agent as a scoped identity (not a shared token) | **Id**, **Agents**, Audit | M |
| **UC-AG-23** Define agent triggers & autonomy policy | P15, P12 | Bind agents to events and set per-scope autonomy | **Bus**, **Agents**, Id | M |
| **UC-AG-24** Org-level agent kill switch | P15, P12 | Instantly disable an agent or all agents | **Agents**, Id, Audit | M |
| **UC-AG-25** Audit an agent's full action history | P12, P13 | Review everything an agent did, like a human's log | **Audit**, **Agents** | M |
| **UC-AG-26** Agent acts on-behalf-of a human/team | A1–A5, P12 | Delegated scope so the agent inherits bounded rights | **Id**, **Agents**, Audit | N |
| **UC-AG-27** Mock↔real agent swap (dev/deploy) | P15, dev | Swap implementation behind the strategy pattern by config | **Agents** | M |
| **UC-AG-28** Agent processing under GDPR constraints | P13 | Agent processing personal data respects lawful basis + residency, appears in ROPA | **GDPR**, **Agents**, Audit | N |
| **UC-AG-29** Reverse / undo an agent action | P12, P3 | Roll back an agent's change where feasible | **Audit**, relevant subsystem | N |

> **Strategy-pattern emphasis (vision non-negotiable):** UC-AG-27 is a *platform*
> requirement, not an agent feature. Every agent integration point must be an interface with
> a mock implementation now and a real one later. This is testable in Phase-1 build with
> deterministic mock agents firing on real events. Flagged for shared-systems + every
> subsystem architecture.

---

## 5. Corporate / compliance / GDPR use cases

P11–P15 rarely use Myelin daily but **decide adoption and lawful operation**. For the EU-
sovereign positioning these are central, not edge. Many are **non-obvious** and expensive
to retrofit — they belong in the design from day one.

### 5.1 Identity, access & administration

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CORP-1** SSO via SAML/OIDC | P15, P12 | Authenticate via corporate IdP | **Id** | M |
| **UC-CORP-2** SCIM provisioning / deprovisioning | P15 | Auto-create/disable accounts from HR/IdP | **Id**, Audit | N |
| **UC-CORP-3** Org/team/project hierarchy & RBAC | P15, P11 | Map company structure to access | **Id**, all | M |
| **UC-CORP-4** Fine-grained, auditable permissions | P12, P15 | Least-privilege across all five subsystems + agents | **Id**, **Audit**, Agents | M |
| **UC-CORP-5** Single org-wide policy surface | P15 | One place for RBAC, residency, retention, agent policy | **Id**, GDPR, Agents | N |
| **UC-CORP-6** MFA / strong auth enforcement | P12, P15 | Require strong authentication | **Id** | M |
| **UC-CORP-7** Service accounts & API tokens (scoped) | P3, P12 | Programmatic access with bounded, audited scope | **Id**, Audit | N |

### 5.2 Audit, security & supply chain

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CORP-8** Tamper-evident audit log of all actions | P12, P13 | Complete who/what/when/where for every artifact | **Audit**, **Id**, all | M |
| **UC-CORP-9** Audit evidence export for certification | P12 | Produce SOC2/ISO27001 evidence on demand | **Audit**, reporting | N |
| **UC-CORP-10** Secrets governance across CI | P12, P3 | Central, least-privilege, rotated secrets | CI, **Id**, Audit | M |
| **UC-CORP-11** Supply-chain attestation (SBOM/provenance) | P12 | Prove what was built from what | CI, Audit, Git | N |
| **UC-CORP-12** Branch protection / signed-commit policy org-wide | P12, P15 | Enforce integrity controls on code | Git, Id, Audit | N |
| **UC-CORP-13** Anomaly / access-review reporting | P12 | Periodic access reviews; flag dormant/over-broad access | **Audit**, Id | L |

### 5.3 GDPR / EU sovereignty (first-class per vision)

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CORP-14** Data residency enforcement (EU) | P13, P14, P15 | Guarantee data never leaves EU control | **GDPR**, **Id**, storage, all | M |
| **UC-CORP-15** Unified personal-data inventory | P13 | Know what personal data exists where, on what basis | **GDPR**, **Refs**, all | N |
| **UC-CORP-16** Lawful-basis & purpose tracking | P13 | Record lawful basis/purpose per processing | **GDPR** | N |
| **UC-CORP-17** Data-subject access request (DSAR) | P13 | Produce all of a person's data across subsystems | **GDPR**, **Id**, all | N |
| **UC-CORP-18** Right-to-erasure / "right to be forgotten" | P13, P1 | Erase a subject's personal data while preserving history integrity | **GDPR**, **Id**, **Audit**, all | N |
| **UC-CORP-19** Rectification of personal data | P13 | Correct inaccurate personal data everywhere | **GDPR**, **Id** | L |
| **UC-CORP-20** Data portability / export (anti-lock-in) | P14, P13, P15 | Export org + personal data in open formats | **GDPR**, all | N |
| **UC-CORP-21** ROPA / DPIA supporting records | P13 | Generate records of processing & impact-assessment data | **GDPR**, Audit | L |
| **UC-CORP-22** Sub-processor transparency | P14, P13 | Clear, ideally all-EU sub-processor list | **GDPR** | L |
| **UC-CORP-23** Consent / restriction-of-processing handling | P13 | Honour restriction and consent-withdrawal requests | **GDPR**, Id | L |
| **UC-CORP-24** Self-host / sovereign-cloud operation | P15, P3, P14 | Run the whole platform on org-controlled EU infra | all, **Id**, **GDPR** | M |
| **UC-CORP-25** Retention policy per data class | P12, P13, P15 | Auto-expire data per class and jurisdiction | **GDPR**, all | N |

> **The hard one (carried from `personas.md §7`, vision §3):** **UC-CORP-18** (erasure vs.
> integrity of code history and audit logs) is a genuine, non-trivial tension. The use-case
> requirement is only that *the platform has a defined, defensible behaviour* (likely
> pseudonymisation / identity-indirection rather than literal deletion). The mechanism is
> **deferred to shared-systems architecture** — but the use case is in scope and must not be
> silently dropped.

### 5.4 Commercial / procurement / cost

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-CORP-26** Org-wide cost / usage reporting | P11, P15, P14 | See spend and usage by team/subsystem | reporting, CI | N |
| **UC-CORP-27** Billing & plan management | P14, P15 | Manage subscription/seats/plan (or self-host licensing) | **Id**, billing | N |
| **UC-CORP-28** Exit / decommission with full export | P14, P15 | Leave the platform with all data, no lock-in | **GDPR**, all | N |
| **UC-CORP-29** DPA / contract & SLA artifacts | P14, P13 | Surface data-processing terms and SLAs | **GDPR**, Knowledge | L |

---

## 6. Scale, migration, offboarding, incident & non-obvious use cases

The cases most often forgotten in v1 and most expensive to retrofit. The vision demands
**world-scale from day 1** and **GDPR by construction**, so these are not optional.

### 6.1 Scale-driven

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-EDGE-1** Multi-tenant isolation at world scale | P15, P12 | One architecture serves a 3-person and a 10,000-person org with hard isolation | **Id**, all | M |
| **UC-EDGE-2** Event fan-out at scale without loss | P3, A5 | Bus delivers to many consumers/triggers reliably under load | **Bus** | M |
| **UC-EDGE-3** Search over millions of artifacts | P4, all | Cross-artifact search stays fast and relevant at scale | **Search**, **Refs** | N |
| **UC-EDGE-4** Notification storm control / dedup | P7, all | Avoid drowning users when events spike | **Notif**, **Bus** | N |
| **UC-EDGE-5** Large monorepo / high-traffic git at scale | P1, P3 | Git operations stay fast on huge repos/teams | **Git** | N |
| **UC-EDGE-6** Reference graph at scale (hot artifacts) | P4 | Graph queries stay fast when an artifact has thousands of links | **Refs** | N |
| **UC-EDGE-7** Rate limiting / abuse / quota controls | P15, P12, P5 | Protect the platform from abusive or runaway (incl. agent) load | **Id**, Bus, Agents | N |
| **UC-EDGE-8** Agent-generated load governance | P15, P12 | Bound how much agents can drive the bus/CI (cost + safety) | **Agents**, **Bus**, CI | N |

> **(under-specified):** sharding/partitioning strategy, tenancy model (pooled vs. siloed
> per residency), and bus delivery guarantees (at-least-once vs. exactly-once) are
> architecture decisions these rows *imply*. Flagged for shared-systems + per-subsystem
> architecture, not resolved here.

### 6.2 Migration-driven (onboarding from the fragmented stack)

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-EDGE-9** Import repos with history | P15, P3 | Bring git repos in preserving history/refs | Git | N |
| **UC-EDGE-10** Import issues from Jira/Linear/GitHub | P15, P6 | Migrate tickets with links/hierarchy/history | Issues, **Refs** | N |
| **UC-EDGE-11** Import docs from Confluence/Notion | P15, P10 | Migrate knowledge preserving structure/links | Knowledge, **Refs** | N |
| **UC-EDGE-12** Import chat history (where lawful) | P15, P13 | Bring conversation history in, GDPR-aware | Chat, GDPR | L |
| **UC-EDGE-13** Identity mapping during migration | P15 | Map external user identities to Myelin identities | **Id**, GDPR | N |
| **UC-EDGE-14** Rebuild reference graph from imported links | P15, A4 | Reconstruct cross-artifact links post-import | **Refs**, Agents | L |
| **UC-EDGE-15** Phased / coexistence migration | P15, P3 | Run alongside incumbents during transition | **Bus**, Id, all | L |

### 6.3 Offboarding, erasure & lifecycle

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-EDGE-16** Offboard a user (revoke + reassign) | P15, P7 | Disable access, reassign owned work/artifacts | **Id**, all, Audit | M |
| **UC-EDGE-17** Erase a departed user's personal data | P13, P15 | Honour erasure while preserving authorship integrity | **GDPR**, **Id**, Audit, all | N |
| **UC-EDGE-18** Offboard / disable an agent | P15, P12 | Cleanly retire an agent identity + its triggers | **Agents**, **Id**, Audit | M |
| **UC-EDGE-19** Decommission a tenant/org | P15, P14 | Export then erase an entire org's data | **GDPR**, **Id**, all | N |
| **UC-EDGE-20** Ownership transfer of orphaned artifacts | P15, P7 | Reassign artifacts owned by departed users/teams | **Id**, Refs, all | N |
| **UC-EDGE-21** Data retention expiry / auto-purge | P13, P12 | Auto-delete data past retention per class | **GDPR**, all | N |

> **Non-obvious but central:** UC-EDGE-17 is UC-CORP-18 from the *operational* angle (a
> leaver, not just a request). The reference graph and event log both contain that person's
> identity; erasure must propagate across **Refs**, **Audit**, and every subsystem
> coherently. This is the single most architecturally demanding GDPR flow. Deferred mechanism,
> in-scope requirement.

### 6.4 Incident response & operations

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-EDGE-22** Declare & run an incident end-to-end | P3, A5, P12 | Incident issue + channel + timeline + comms in one flow | Issues, Chat, CI, **Bus**, Audit | N |
| **UC-EDGE-23** Postmortem doc linked to the incident | P3, P4, A4 | Capture learnings linked to incident + fixes | Knowledge, Issues, **Refs** | N |
| **UC-EDGE-24** Status page / external comms (under-specified) | P3, P11 | Communicate outages externally | Chat, Knowledge | L |
| **UC-EDGE-25** On-call / escalation routing | P3, P7 | Page the right human when agents escalate | **Notif**, Id | L |
| **UC-EDGE-26** Platform self-monitoring & health | P15 | Operate Myelin itself with good observability | **Bus**, reporting | N |

### 6.5 Other non-obvious cases (suspected; some under-specified)

| ID / Title | Personas | Goal | Subsystems | Pri |
|---|---|---|---|---|
| **UC-EDGE-27** Accessibility (a11y) across all surfaces | all, P2 | Meet accessibility baseline (vision: top-tier UX) | all | M |
| **UC-EDGE-28** Internationalisation / localisation | all (EU) | Serve multiple EU languages/locales | all | N |
| **UC-EDGE-29** Offline / poor-connectivity resilience (under-specified) | P1, P9 | Degrade gracefully when connectivity is poor | Git, Knowledge | L |
| **UC-EDGE-30** Bulk operations / admin batch actions | P15, P5 | Mass-edit/label/move artifacts safely | all, Audit | N |
| **UC-EDGE-31** Webhooks/API for *external* integrations | P3, P15 | Interop with outside tools without breaking sovereignty | **Bus**, Id | N |
| **UC-EDGE-32** Public/anonymous read access (OSS, public docs) | P5, P10 | Serve unauthenticated readers safely | Git, Knowledge, Issues, Id, GDPR | N |
| **UC-EDGE-33** Spam / abuse handling on public surfaces | P5, A2 | Defend public issue/PR queues from spam | Issues, Git, Agents | L |
| **UC-EDGE-34** Conflict resolution in concurrent editing | P6, P10 | Resolve simultaneous edits (docs, fields) sanely | Knowledge, Issues, **Bus** | N |
| **UC-EDGE-35** Time-travel / point-in-time view for audit | P12, P13 | Reconstruct state "as of" a date for an audit | **Audit**, all | L |
| **UC-EDGE-36** Cross-org collaboration (vendors/partners) (under-specified) | P11, P15 | Share specific artifacts across org boundaries | **Id**, Refs | L |
| **UC-EDGE-37** Emergency break-glass access | P12, P15 | Grant time-boxed, fully-audited emergency access | **Id**, **Audit** | L |

---

## 7. Prioritisation view (MVP vs. later)

> **Uncertainty:** priorities are **hypotheses** derived from the personas (also unvalidated)
> and from "what makes the wedge demonstrable." They are sequencing guidance for architecture,
> not commitments. Revisit against the positioning/competitive doc.

### 7.1 The MVP thesis

The MVP must **prove the wedge**, not match every incumbent feature. The differentiator is
*the glue*, so the MVP is the smallest slice where **shared identity + event bus + reference
graph** produce a visibly better experience than five separate tools. That means a thin but
*genuinely cross-linked* vertical through all five subsystems, plus the non-negotiable
foundations (one identity model, audit, EU residency, mock agents on real events).

### 7.2 MVP must-haves (Pri = M)

Grouped by what they prove:

- **Shared foundation (without which nothing else is differentiated):** UC-X-6 (one identity
  model), UC-X-2 (unified search), UC-X-7 (unified inbox), UC-X-8 (live reference graph),
  UC-X-16 / UC-CORP-8 (single audit trail), UC-CORP-1/3/4/6 (SSO, RBAC, least-privilege,
  MFA), UC-CORP-14 + UC-CORP-24 (EU residency + self-host), UC-EDGE-1 (multi-tenant
  isolation), UC-EDGE-2 (bus fan-out), UC-EDGE-16 (offboard a user), UC-EDGE-27 (a11y).
- **Per-subsystem core:** Git (UC-GIT-1–7, 17), CI (UC-CI-1–5, 9), Issues (UC-ISS-1–5, 8,
  13 + both UC-ISS-3 board and UC-ISS-4 roadmap views over one model), Knowledge (UC-KN-1–3,
  6, 8, 11, 14), Chat (UC-CHAT-1–7, 10).
- **The wedge made visible:** UC-X-1 (spec-to-ship traceability), UC-X-3 (PR context pane),
  UC-X-4 (event automation), UC-X-5 (roadmap reflects delivery).
- **Agents native from day one (mock):** UC-AG-22/23/24/25/27 (register, trigger/policy,
  kill switch, audit, mock↔real swap). At least one end-to-end mock agent flow (e.g. A2
  auto-label on issue-created, UC-AG-5) to prove the interaction model on the real bus.

### 7.3 Near-term (Pri = N) — depth once the wedge is proven

Most per-subsystem depth (matrix CI, databases in Knowledge, dependencies/portfolio in
Issues, real-time co-editing, preview environments), the richer agent behaviours (A1 draft
PRs, A3 review, A4 stale-doc, A5 incident), migration importers (UC-EDGE-9–13), DSAR/erasure
operational tooling (UC-CORP-17/18, UC-EDGE-17), and scale-hardening (UC-EDGE-3/4/6).

### 7.4 Later (Pri = L)

Specialised/long-tail: SLAs (UC-ISS-11), flaky-test automation (UC-CI-10), public knowledge
base (UC-KN-12), e-discovery/retention nuance, advanced GDPR (ROPA/DPIA generation,
restriction handling), cross-org collaboration, status pages, break-glass, i18n breadth,
offline resilience.

### 7.5 Sequencing caveat

The non-negotiables (identity, audit, EU residency, agent interaction model, multi-tenancy)
are **MVP even though they are "infrastructure"**, because retrofitting them is the classic
way platforms fail their gatekeeper personas (P12/P13/P15). Per the vision, **world-scale
and GDPR are day-1 architectural constraints**, so their use cases sit in the MVP column
even when the user-visible feature they support is later.

---

## 8. Implications for the shared systems

What the use cases *require* of each shared system. This is the bridge to Phase 3 (shared-
systems architecture); it is requirements-shaped, not design-shaped.

### 8.1 Identity & access (Id)

- One identity per human **and per agent**; agents are scoped identities, never shared
  tokens (UC-AG-22, UC-CORP-4). Delegated/on-behalf-of scopes (UC-AG-26).
- Consistent RBAC across **all five subsystems + the agent fabric** from one model
  (UC-X-6, UC-CORP-3/4). Org/team/project hierarchy with inheritance.
- SSO (SAML/OIDC), SCIM, MFA, scoped service tokens (UC-CORP-1/2/6/7).
- Multi-tenant isolation that scales from 3 to 10,000s of users on one architecture
  (UC-EDGE-1). Tenancy model interacts with residency (pooled vs. siloed per region) —
  **(open)**.
- Lifecycle: offboarding, ownership transfer, break-glass, tenant decommission
  (UC-EDGE-16/19/20/37). Identity is where erasure-vs-integrity is ultimately resolved
  (pseudonymisation/indirection) — **deferred mechanism, in-scope requirement**.

### 8.2 Event bus (Bus)

- First-class, observable triggers ("when X do Y") replacing webhook glue (UC-X-4), driving
  both automation and **agent activation** (all UC-AG rows).
- Reliable fan-out at scale without loss (UC-EDGE-2); delivery guarantee
  (at-least-once vs. exactly-once) is **(open)** and affects idempotency requirements on
  every consumer.
- The bus is also the **source of truth for delivery analytics** (UC-ISS-10, UC-X-14) — so
  events must be rich, durable, and queryable, not just transient.
- Carries personal data (actor identities) → residency/retention/erasure apply to the event
  log itself (UC-CORP-14/25, UC-EDGE-17) — a P13 concern explicitly flagged in `personas.md`.
- Must bound **agent-generated load** so agents can't runaway-drive the bus/CI (UC-EDGE-8).

### 8.3 Reference graph (Refs)

- Bidirectional, **live** links between any artifact and any other (issue↔PR↔doc↔chat↔run):
  UC-ISS-13, UC-KN-8, UC-CHAT-3, UC-X-1/3/8/12/13. Links must resolve to current state, not
  rot (UC-X-8).
- Powers traceability (spec-to-ship), the PR context pane, "everything touching Z," and
  release-note assembly (UC-X-1/3/10/13, UC-AG-15/16).
- Contains personal data and is part of the **DSAR/erasure surface** (UC-X-17, UC-CORP-15/17/18,
  UC-EDGE-17) — the graph must answer "everywhere this subject appears."
- Must stay fast on **hot artifacts** with thousands of links (UC-EDGE-6).

### 8.4 Search

- Unified ranking across **all artifact types** in one query (UC-X-2), plus per-subsystem
  search (code UC-GIT-12, docs UC-KN-11, chat UC-CHAT-6, issues UC-ISS-8).
- Used by triage agents for **deduplication** (UC-AG-6) — semantic, not just lexical, ideally.
- Must scale to millions of artifacts with good relevance (UC-EDGE-3) and respect
  permissions (a user must never find what they can't access — a hard, easy-to-get-wrong
  cross-cut with Id).

### 8.5 Notifications (Notif)

- One **prioritised cross-subsystem inbox** ("what needs *me*"), not five firehoses (UC-X-7,
  UC-CHAT-12). Routing, quiet hours, and storm/dedup control (UC-EDGE-4).
- On-call/escalation routing for agent escalations (UC-EDGE-25, UC-AG-19).

### 8.6 Audit & GDPR (facets of Shared, first-class per vision)

- **Single, tamper-evident audit trail** of every human *and agent* action on every artifact
  (UC-X-16, UC-CORP-8, UC-AG-25). Time-travel/point-in-time reconstruction for audits
  (UC-EDGE-35).
- EU data residency by construction across **every** subsystem and shared store, including
  the bus, search index, and reference graph (UC-CORP-14, UC-CORP-24 self-host).
- Data-subject rights tooling spanning all subsystems at once: inventory, access, erasure,
  rectification, portability, restriction (UC-CORP-15–23, UC-X-17, UC-EDGE-17/19/21).
- **The unresolved tension** (vision + `personas.md §7`): erasure vs. immutability of code
  history and audit logs. In-scope as a use case (UC-CORP-18 / UC-EDGE-17); the *mechanism*
  is deferred to shared-systems architecture and must be explicitly, defensibly designed
  (not silently dropped).

### 8.7 Agent fabric (Agents)

- A common agent **interface** (strategy pattern) with **mock now / real later** swappable by
  config (UC-AG-27) — a platform-wide requirement, present at every agent integration point.
- Trigger binding, per-scope autonomy policy, human-in-the-loop gates, kill switches
  (UC-AG-23/24), full auditability and reversibility where feasible (UC-AG-25/29).
- Agents bound by the same Id, residency, lawful-basis, and audit constraints as humans
  (UC-AG-26/28).

---

## 9. Open questions, assumptions, and deferrals

**Honesty about uncertainty (vision principle).**

**Assumptions made:**
- Priorities (§7) assume the MVP's job is to **prove the cross-subsystem wedge**, not to
  reach feature parity with any incumbent. If positioning concludes a single subsystem must
  win standalone first, the MVP set shifts.
- Assumed Myelin **references** specialist authoring tools (e.g. Figma) rather than replacing
  them (UC-KN-16, UC-X-12) — inherited from `personas.md`; the depth of native
  design/whiteboard authoring is undecided.
- Assumed agents are **mock** in Phase-1 build (vision), so all UC-AG rows specify the
  *interaction model*, not real agent intelligence.

**Open questions (deferred to later phases):**
- **Erasure vs. integrity** (UC-CORP-18, UC-EDGE-17): the single hardest GDPR design
  question. Mechanism deferred to shared-systems architecture; the use case stays in scope.
- **Tenancy vs. residency model** (UC-EDGE-1, §8.1): pooled vs. siloed per region, and how
  self-host (UC-CORP-24) and managed-cloud share an architecture. Deferred to shared-systems.
- **Bus delivery guarantees** (UC-EDGE-2, §8.2): at-least-once vs. exactly-once drives
  idempotency requirements everywhere. Deferred.
- **Issue-model duality** (UC-ISS-3 vs. UC-ISS-4): one model serving board *and* roadmap as
  co-equal views — the issue tracker's central UX risk. Deferred to Issues architecture.
- **Agent autonomy & on-behalf-of model** (UC-AG-23/26): exact delegated-scope and policy
  model. Deferred to shared-systems (Id + Agents).
- **Permission-aware search** (§8.4): preventing users from discovering inaccessible
  artifacts via search/refs is subtle at scale. Deferred to Search + Id architecture.

**Use cases I suspect exist but could not fully specify (marked (under-specified) inline):**
- Native design authoring depth (beyond referencing) — UC-KN-16 / P9 scope.
- Status pages / external incident comms — UC-EDGE-24.
- Offline / poor-connectivity behaviour — UC-EDGE-29.
- Cross-org / partner collaboration boundaries — UC-EDGE-36.
- Sharding/partitioning and scale internals implied by UC-EDGE-3/5/6 (architecture, not
  use-case, decisions).
- Localisation breadth across EU languages — UC-EDGE-28 (in scope, depth undecided).

**Explicitly deferred entirely (out of Phase-1 use-case scope):**
- Detailed permission matrices / role definitions → shared-systems architecture.
- Concrete UI flows, screens, and empty/loading/error states → design phase (vision §2/§3).
- Quantitative prioritisation / market sizing → positioning/competitive doc.
- Concrete agent capability specs and mock implementations → architecture + build phases.

**Cross-references:**
- [`personas.md`](./personas.md) — the actors these use cases serve.
- Competitive landscape & positioning (Phase 1) — should validate/refute the §7 MVP thesis
  and the wedge framing in §3.
- Shared-systems architecture (Phase 3) — owns the deferred mechanisms in §8 (identity,
  agent permissioning, audit, GDPR erasure, tenancy, bus guarantees).
- Subsystem architectures (Phase 4) — each §2 sub-table is a starting checklist for that
  subsystem's agent; the issue-model duality (§2.3) is flagged as that agent's central risk.
