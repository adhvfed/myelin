# R-03 — JTBD Catalogue for the Three Audiences

> **Phase 4 research corpus** · deliverable of prompt **R-03** (workstream
> [`ws-b-jtbd-and-flows.md`](../../02-research-roadmap/ws-b-jtbd-and-flows.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0). This file is the **jobs
> layer** the rest of the corpus builds on: R-04 (cross-surface flows) realises these jobs
> as blueprints; R-05 pressure-tests the persona assumptions behind them; R-16 deepens the
> dual-audience same-data pairs. Foundational item — no prior `04-research` dependency.

## 0. How to read this file

A **Jobs-To-Be-Done (JTBD)** catalogue. We write **job stories** in Alan Klement's format —
*"When [situation], I want to [motivation], so I can [outcome]"* — rather than persona
feature lists, because the situation-led framing decouples the job from any one subsystem
and keeps it stable while solutions change. **PROVEN** *(JTBD job-story format; A. Klement /
Intercom; ["Jobs to Be Done", A List Apart](https://alistapart.com/article/jobs-to-be-done/);
[learningloop.io job-story glossary](https://learningloop.io/glossary/job-stories-jtbd)).*

The deeper **theory** — that customers "hire" products to make progress on a job, and that
unmet need is found by scoring jobs on **importance × current satisfaction**
(opportunity-driven) — is **PROVEN** *(Outcome-Driven Innovation / JTBD; A. Ulwick,
Strategyn, since 1991; [strategyn.com/jobs-to-be-done](https://strategyn.com/jobs-to-be-done/);
[Outcome-Driven Innovation, Wikipedia](https://en.wikipedia.org/wiki/Outcome-Driven_Innovation)).*
But every **instantiation** below — which jobs Myelin's specific personas hold, and how
important each is — is a **HYPOTHESIS**, because no Myelin user has been interviewed. We tag
accordingly:

- **PROVEN-theory** — the *method/structure* is grounded (citation given once, above).
- **HYPOTHESIS-instantiation** — the *specific job, persona mapping, and any priority claim*
  is our reasoned guess from `personas.md` + `use-cases.md`, **unvalidated**. **Every job
  story in §2–§4 carries this tag.**

The decisive ODI step — ranking jobs by **importance × satisfaction** to find the
"opportunities" — is **`[DEFERRED-UNTIL-USERS]`** and is recorded as an executable plan in
**§6**, **not faked here**. We deliberately do **not** assert relative priority between jobs
in the catalogue (no "this is a top job") because that *is* the deferred measurement.

**Column legend** for the job tables:
- **Job** — the situation→motivation→outcome story.
- **Persona(s)** — who holds it (`personas.md` ids).
- **§7 surface(s) that finish it** — the view(s) from design-language §7 catalogue where the
  job completes. (CLI per §7.7 noted where the job is keyboard-first.)
- **UC** — the `use-cases.md` rows the job draws on (traceability, not a re-listing).
- **Tag** — always **HYP** (HYPOTHESIS-instantiation) for the specific row; the method is
  PROVEN-theory per §0.

Jobs are written at the **functional-job** altitude with the **emotional/social** dimension
named where it changes the design (JTBD core: jobs have functional, emotional, and social
sides — PROVEN-theory, Ulwick/Klement). Where a job is the **same data seen by another
audience**, it carries a `↔ Dnn` pair-id resolved in **§5** (the dual-audience pairs — the
platform's central justification).

---

## 1. The three audiences (and why the split matters)

`personas.md` clusters into three audiences; this catalogue is grouped by them so
**corporate/governance is structurally impossible to skip** (acceptance criterion):

| Audience | Personas | What "progress" means to them | Where they live |
|---|---|---|---|
| **A. Engineers** (individual contributors) | P1 backend, P2 frontend, P3 platform/SRE, P4 staff, P5 OSS maintainer | Ship correct code fast, keep context, low friction, agents save not cost time | Git, CI, Issues, Refs (constantly), CLI |
| **B. PM / delivery** | P6 PM, P7 EM, P8 PgM/TPM, P9 designer, P10 tech-writer | Communicate & track outcomes, scale process without bureaucracy, keep intent linked to delivery | Issues (PM lens), Knowledge, Chat, reporting |
| **C. Corporate / governance** | P11 CTO/VP, P12 security, P13 DPO, P14 procurement, P15 IT-admin | Adopt lawfully & controllably; demonstrate compliance; reduce vendor/sovereignty risk | Shared/admin, audit, GDPR consoles, residency |

> **The reason for the split (PROVEN — Myelin's positioning):** the market's defining failure
> is the *engineering-tool-vs-management-tool* split (design-language §2;
> `competitive-landscape.md §3`); and for an EU-sovereign product the corporate/governance
> audience are *central buyers*, not edge cases (`personas.md §4`, §6). A JTBD catalogue that
> only covered engineers would re-create exactly the trap Myelin exists to dissolve.
> **HOUSE STYLE:** we elevate the agent-collaboration jobs into each audience (rather than a
> separate "agent" cluster) because agents are first-class *participants in human jobs*, not a
> feature — `personas.md §5`.

A note on the **CLI as a job surface** (design-language §7.7): for the engineer audience many
jobs *complete in the terminal*, not the web UI. Where a job is keyboard/CLI-first we name
the CLI verb alongside the §7 web surface — the design must finish the job in **either**
rendering of the one surface.

---

## 2. Audience A — Engineers (P1–P5)

### A-core: writing & shipping code

| # | Job | Persona(s) | §7 surface(s) that finish it | UC | Tag |
|---|---|---|---|---|---|
| E1 | **When** I pick up a unit of work, **I want to** see the *why* (issue, prior decision, design doc, the discussion) without leaving my flow, **so I can** make the right change the first time. | P1, P2, P4 | PR overview / **PR context pane** (§7.1); Issue detail (§7.3); Backlinks panel (§7.4); CLI `myelin pr view` (§7.7) | UC-X-3, UC-ISS-13, UC-GIT-17 | HYP |
| E2 | **When** I'm ready to propose a change, **I want to** open a PR that already shows its linked issue, CI run, and the relevant doc section inline, **so I can** get reviewed without making reviewers tab-hop. | P1, P2, P3 | PR overview (§7.1), Checks panel (§7.1), Diff view (§7.1) | UC-GIT-3, UC-X-3, UC-CI-4 | HYP |
| E3 | **When** CI goes red on my PR, **I want to** jump from the failing check → the failing step → the exact line of code, **so I can** fix it fast instead of digging through opaque logs. *(emotional: relief of not being blocked; this is the **wedge engineer flagship** R-04 realises.)* | P1, P2, P3 | Checks panel (§7.1) → Single-run view (§7.2) → Live log view (§7.2) → Diff/file view (§7.1); CLI `myelin run view`, log tail | UC-CI-3, UC-CI-4, UC-GIT-17 | HYP |
| E4 | **When** I'm reviewing someone's change, **I want to** review line-by-line *and* see only what changed since I last looked, with verdicts and threads that resolve, **so I can** give high-signal review without re-reading the whole diff. | P1, P4, P2 | Diff/files-changed view, Review surface (§7.1); inline+batched comments | UC-GIT-3, UC-GIT-4 | HYP |
| E5 | **When** I'm reading code I didn't write, **I want to** trace why a line exists (blame → commit → PR → issue → decision) live, **so I can** change it safely. | P1, P2, P4 | File view + blame, History/commit views (§7.1); Backlinks panel (§7.4) | UC-GIT-2, UC-GIT-17, UC-X-8 | HYP |
| E6 | **When** I need something across the org, **I want to** find the commit, the issue, the doc, and the chat thread from **one** search box, ranked together, **so I can** stop juggling five search boxes. | P1–P4 | Global/cross-artifact search (§7.6, §5.7); Code search (§7.1) | UC-X-2, UC-GIT-12 | HYP |
| E7 | **When** I'm deep in a task, **I want to** reach any object or action by keyboard via one palette, **so I can** move at muscle-memory speed without touching the mouse. *(emotional: flow/competence.)* | P1, P3, P4 | Command palette (§5.2) over the whole IA; CLI verbs (§7.7) | UC-X-6 (one identity to act as) | HYP |

### A-platform / reliability (P3, also P15 when self-hosting)

| # | Job | Persona(s) | §7 surface(s) | UC | Tag |
|---|---|---|---|---|---|
| E8 | **When** I want "when X happens, do Y" automation, **I want to** declare it as a first-class, observable trigger, **so I can** stop maintaining webhook glue that breaks silently. | P3, P15 | Event/trigger surfaces; CI definition editor (§7.2); agent governance triggers (§7.6) | UC-X-4, UC-CI-2 | HYP |
| E9 | **When** an incident fires, **I want to** run it as one linked timeline (incident issue + war-room channel + CI/deploy events + runbook doc), **so I can** coordinate without stitching tools mid-crisis. *(also a PM/governance job — see C-/B- below; ↔ shared with PM C-flow.)* | P3, P15 | Incident issue (§7.3), Incident channel/thread (§7.5), Single-run/deploy view (§7.2), runbook page (§7.4) | UC-X-11, UC-ISS-15, UC-EDGE-22 | HYP |
| E10 | **When** a runner/pipeline is unhealthy, **I want to** see run times, queue depth, flakiness and spend in one place on EU-controlled compute, **so I can** keep the paved road fast and in-region. | P3, P15 | Run dashboard (§7.2), Usage/quota view (§7.2), residency console (§7.6) | UC-CI-14, UC-CI-9, UC-CORP-14 | HYP |

### A-open-source (P5)

| # | Job | Persona(s) | §7 surface(s) | UC | Tag |
|---|---|---|---|---|---|
| E11 | **When** my issue queue is a noisy firehose, **I want to** face a curated queue where an agent has labelled/deduped/routed and I confirm, **so I can** triage without burning out. *(↔ shared queue concept with PM E1-class triage; emotional: control over overwhelm.)* | P5, (P6/P7) | Triage inbox (§7.3) with agent-assisted dedup/label (§6) | UC-ISS-12, UC-AG-5/6, UC-X-15 | HYP |
| E12 | **When** a stranger opens a fork PR, **I want to** run CI safely with scoped, auditable permissions and clean public/private separation, **so I can** accept outside contributions without leaking secrets. | P5, P3 | Checks panel (§7.1), Secrets mgmt (§7.2), public/private issue & channel scope (§7.3/§7.5) | UC-GIT-8/13, UC-ISS-17, UC-CHAT-10 | HYP |

> **Engineer-audience emotional throughline (HOUSE STYLE):** the felt job behind almost all
> of A is *"stay in flow; don't make me leave the page to find the why; don't make me babysit
> tooling."* This is the speed/calm/coherence triad (design-language P1/P2/P8). It tells the
> sketch funnel that engineer surfaces sit toward **Axis 1 = dense** and **Axis 2 =
> palette-led** (sketch-funnel §Part 1) — the keyboard path is load-bearing, not optional.

---

## 3. Audience B — PM / delivery (P6–P10)

| # | Job | Persona(s) | §7 surface(s) that finish it | UC | Tag |
|---|---|---|---|---|---|
| M1 | **When** stakeholders ask "what's shipping this quarter," **I want to** show a roadmap/now-next-later that reflects *real* delivery state pulled from the same data engineers produce, **so I can** report honestly without status-chasing. *(↔ **D1** with E-board jobs.)* | P6, P8, P11 | Roadmap/portfolio view (§7.3); Dashboards (§7.3) | UC-ISS-4, UC-X-5, UC-ISS-16 | HYP |
| M2 | **When** I write a spec/PRD, **I want to** have it *be* linked to the epics/issues/PRs delivering it (not a dead copy), **so I can** keep intent and delivery one living thread (spec-to-ship). | P6, P4, P10 | Block editor / PRD page (§7.4); Backlinks panel (§7.4); Issue hierarchy (§7.3) | UC-X-1, UC-KN-8/9, UC-ISS-5 | HYP |
| M3 | **When** I plan an iteration, **I want to** decompose and roll up work (epic→story→task→sub) and see progress roll up automatically, **so I can** keep a clear, trustworthy picture without manual aggregation. | P6, P7, P8 | Issue hierarchy panel (§7.3), List/board/timeline views (§7.3) | UC-ISS-5/16, UC-ISS-9 | HYP |
| M4 | **When** I run a team, **I want to** see trustworthy flow/health analytics (cycle time, WIP, review latency, CI health) drawn from one event stream — not surveillance, **so I can** unblock the team and report up honestly. *(emotional/social: fairness, not gaming.)* | P7, P11, P8 | Dashboards (§7.3), Delivery analytics (§7.3), Team page (§7.3) | UC-ISS-10, UC-X-14 | HYP |
| M5 | **When** I'm drowning in notifications, **I want to** one prioritised "what needs *me*" inbox across all subsystems with *why am I getting this*, **so I can** protect attention and miss nothing important. *(↔ **D3** — same inbox, engineer vs EM weighting.)* | P7, P6, (all) | Unified notifications inbox (§7.6, §5.8) | UC-X-7, UC-CHAT-12, UC-EDGE-4 | HYP |
| M6 | **When** a community/customer report comes in via chat, **I want to** promote it into clean tracked work and watch it flow triage→prioritise→deliver→back, **so I can** close the feedback loop without copy-paste. | P6, P5, P7 | Chat composer → convert-to-issue (§7.5); Triage inbox (§7.3); Roadmap (§7.3) | UC-X-15, UC-CHAT-8, UC-ISS-14 | HYP |
| M7 | **When** I'm coordinating cross-team dependencies, **I want to** see dependency graphs and portfolio timelines over the same shared data, **so I can** manage the critical path across many teams/repos. | P8, P4, P11 | Portfolio/timeline view (§7.3), dependency relations panel (§7.3) | UC-ISS-6/9, UC-X-13 | HYP |
| M8 | **When** the design changes, **I want to** trace which design decision a code change implements and who approved it, **so I can** keep design intent, code, and approval in one reference graph. | P9, P2, P6 | Backlinks panel (§7.4), PR review surface (§7.1), external-design reference embed (§7.4) | UC-X-12, UC-KN-16 | HYP |
| M9 | **When** code merges, **I want to** be told which docs it likely invalidated and draft updates from the change, **so I can** keep docs in sync with reality instead of letting them rot. | P10, P4 | Knowledge page + stale-doc/freshness signal (§7.4), agent-proposed edit card (§6) | UC-X-9, UC-KN-13, UC-AG-13/14 | HYP |
| M10 | **When** I cut a release, **I want to** assemble release notes/changelog from the linked merged PRs + issues + decisions, **so I can** publish notes tied to the real work, not hand-written. | P6, P10 | Release/changelog view (§7.1/§7.4), Backlinks panel (§7.4) | UC-X-10, UC-GIT-9, UC-AG-16 | HYP |

> **PM/delivery emotional throughline (HOUSE STYLE):** the felt job is *"let me communicate &
> trust the picture without maintaining a parallel reality."* Approachability is a **hard
> requirement** here, not a nice-to-have (design-language §2; the Sourcehut lesson). This puts
> PM surfaces toward **Axis 1 = calmer / more spacious** and **Axis 4 = warmer** — but over
> **the same components and the same data** as engineers (the §5 dual-audience pairs), never a
> second product.

---

## 4. Audience C — Corporate / governance (P11–P15)

*(Per acceptance criteria: this audience is present and first-class. These personas rarely use
Myelin daily but decide adoption and lawful operation — `personas.md §4`.)*

| # | Job | Persona(s) | §7 surface(s) that finish it | UC | Tag |
|---|---|---|---|---|---|
| G1 | **When** my board/customers ask about engineering health & spend, **I want to** see org-wide delivery health and cost from trustworthy shared data — one bill, one identity model, one audit trail, **so I can** justify the platform and the consolidation. | P11, P14 | Executive rollups / Dashboards (§7.3); Billing/usage view (§7.6) | UC-X-14, UC-CORP-26, UC-ISS-9 | HYP |
| G2 | **When** I evaluate a tool touching code & data, **I want to** see one identity & access model with fine-grained, *inspectable* RBAC across all five subsystems **and** the agent fabric, **so I can** reason about and enforce least privilege. | P12, P15 | Permission/role management — "who can see/do what" inspectable (§7.6); Org/team admin (§7.6) | UC-X-6, UC-CORP-3/4, UC-AG-22 | HYP |
| G3 | **When** an auditor asks for evidence, **I want to** query one tamper-evident audit trail of every human *and agent* action on every artifact, with provenance/correlation threading, **so I can** pass the audit without a manual scavenger hunt. *(emotional: defensibility under scrutiny.)* | P12, P13 | Audit log explorer (§7.6) | UC-X-16, UC-CORP-8/9, UC-AG-25 | HYP |
| G4 | **When** agents are first-class users, **I want to** see which agents exist, their identities/scopes/delegation/budgets and autonomy policy, with an org-level kill switch, **so I can** govern agents instead of fearing them. *(P12's single biggest fear of agent-native platforms — `personas.md` P12.)* | P12, P15 | Agent governance console + kill switch (§7.6, §6.4) | UC-AG-22/23/24/25, UC-EDGE-8 | HYP |
| G5 | **When** I must answer a data-subject access request, **I want to** locate/export every piece of a person's personal data across *all* subsystems in one inventory with deadline tracking and verifiable receipts, **so I can** honour the right within the GDPR clock. *(↔ **D4** — the DPO view; the data-subject sees their own side — R-04/R-19 realise both sides.)* | P13 | GDPR / data-rights console — DSR orchestrator (§7.6) | UC-X-17, UC-CORP-15/17, UC-EDGE-17 | HYP |
| G6 | **When** an erasure ("right to be forgotten") is requested, **I want to** erase a subject's personal data while preserving code-history & audit integrity, with the system telling me the consequence before it happens, **so I can** comply without breaking engineering integrity. *(the hard tension — `personas.md §7`, `use-cases.md §5.3`; mechanism deferred to architecture, but the *job* is in scope.)* | P13, P1 | GDPR console erase flow (§7.6); erased/tombstoned state on chip/unfurl/backlink (§5.3) | UC-CORP-18, UC-EDGE-17 | HYP |
| G7 | **When** sovereignty is the mandate, **I want to** see where this tenant's data lives, its lawful basis, sub-processors, and residency at a glance, **so I can** prove no personal data silently leaves EU control. | P13, P14, P15 | Data map / RoPA & residency console (§7.6); per-artifact residency/visibility cue (§5.3) | UC-CORP-14/15/16/22, UC-CORP-24 | HYP |
| G8 | **When** I run Myelin for my org, **I want to** one admin surface for identity, RBAC, residency, retention, agent policy and audit — SSO/SCIM/MFA included, **so I can** operate one platform instead of N tools' N admin models. | P15 | Org/team/project admin, SSO/SCIM (§7.6); single org-wide policy surface | UC-CORP-1/2/3/5/6, UC-CORP-25 | HYP |
| G9 | **When** I negotiate or exit a contract, **I want to** a clean DPA, transparent (ideally all-EU) sub-processor list, and strong data-portability/exit guarantees, **so I can** avoid lock-in and control TCO. | P14, P13, P15 | Billing / usage / **export & exit** surfaces (§7.6); Knowledge/KB export (§7.4) | UC-CORP-20/22/28, UC-KN-15 | HYP |
| G10 | **When** a user leaves, **I want to** revoke access and reassign their owned work/artifacts cleanly, then erase their personal data on request preserving authorship integrity, **so I can** offboard lawfully without orphaning work. | P15, P7, P13 | Org/team admin offboard flow (§7.6); ownership transfer; erased state (§5.3) | UC-EDGE-16/17/20, UC-CORP-18 | HYP |

> **Corporate/governance emotional throughline (HOUSE STYLE):** the felt job is *"let me trust
> & demonstrate control at a glance — make sovereignty legible, not buried in settings."* This
> is design-language **P9 (sovereignty as UX)** and maps to **sketch-funnel Axis 6
> (sovereignty visibility)** and **rubric D9**. **Honesty flag (carried to R-19):**
> sovereignty-*as-UX* is largely **HOUSE STYLE / under-evidenced** — there is no established
> external playbook for "a DPO trusts a console at a glance"; G3/G5/G6/G7 are exactly the jobs
> R-19's deferred regulated-buyer review must validate.

---

## 5. The dual-audience same-data pairs (the platform's central justification)

This is the make-or-break section: **the same underlying data, two (or three) jobs, one
component** — never a fork (design-language §2; ADR-06; rubric **D5**; sketch-funnel **Axis
3**). R-16 deepens each with a per-lens critique; here we *name the pairs* so they cannot be
quietly dropped. **PROVEN-theory** (one schema, many views is the stated wedge); each specific
pairing is **HYPOTHESIS-instantiation**.

| Pair | The shared data | Engineer job (lens 1) | PM/delivery job (lens 2) | Corp/exec job (lens 3, where it exists) | One component | Endangered if forked |
|---|---|---|---|---|---|---|
| **D1** | The **issue model** (one schema) | E-burn-down: *"...track this cycle on a keyboard-driven board, so I can ship the sprint"* (≈ E3/E4 plane; UC-ISS-3) | M1: *"...communicate a roadmap / now-next-later that reflects real delivery, so I can report honestly"* (UC-ISS-4) | G1 exec rollup: *"...see a portfolio rollup, so I can judge org throughput"* (UC-ISS-9) | **Views component** (§5.6): board ↔ roadmap ↔ portfolio over one record set | The §2 "serves neither" trap; PM keeps a parallel reality (Jira-vs-Productboard split) |
| **D2** | A **knowledge database** (records + views) | Engineer: *"...query a structured collection (e.g. service catalogue) as a table I can edit inline"* | PM/designer: *"...see the same records as a board/gallery/timeline for planning"* (UC-KN-4) | — | **Database views** (§7.4 / §5.6) | Two databases drift; the reference graph fractures |
| **D3** | The **notifications stream** (one read-state truth) | E: high-volume, terse, *"what's blocking my PR/build"* (E5-adjacent) | EM/PM M5: *"what needs my decision / who's blocked"* with *why-fired* provenance (UC-X-7) | P12: *"agent-volume governance"* lens (G4) | **Unified inbox** (§5.8) | Five firehoses return; the calm promise dies |
| **D4** | A **person's personal-data footprint** (the reference/event graph) | (Engineer is the *data subject* — sees their own data export side) | (PM rarely; EM may action offboarding M-adjacent) | DPO G5/G6: *"locate/erase across all holders"* — the **DPO view**; the **data-subject view** is the same data seen by the subject | **DSR console + per-artifact data lineage** (§7.6) | The "one data-subject view" (UC-X-17) becomes N manual searches |
| **D5** | A **dashboard / metric set** (one event stream) | Engineer: *"my flow / my CI health"* | EM/PgM M4/M7: *"team flow, dependencies, delivery health"* | Exec G1: *"org-wide health & cost"* | **Dashboards + charting language** (§3.7/§7.3) | Metrics get re-derived per audience and disagree (the "easy to game" trap, P7) |

> **Why these are load-bearing (HOUSE STYLE):** D1 is the issue tracker's central UX bet and
> the single most-cited open question (`personas.md §7`, `use-cases.md §2.3`, design-language
> §9). D4 is the hardest because the "two lenses" are a *data subject* and a *DPO* over the
> same personal-data graph — an unusually high-stakes same-data pair. **R-16 must critique each
> lens against its persona to prove neither is a degraded compromise; Phase 6 must sketch the
> dual-audience surfaces in *both* lenses** (sketch-funnel comparable-screen set).

---

## 6. `[DEFERRED-UNTIL-USERS]` — the importance × satisfaction ranking (the ODI core)

**This is the decisive measurement and it is NOT performed here.** Asserting which jobs above
matter most, or how unmet they are, without users would be the exact "taste-masquerading-as-
settled" failure the honesty rule forbids (VISION §3). The catalogue above therefore makes
**no relative-priority claims**. The ranking is recorded as a concrete, executable Phase-4
plan.

### 6.1 What to measure
For **every job story E1–E12, M1–M10, G1–G10** (and the §5 pairs as *paired* items), measure
two numbers per respondent on a **fixed scale** (use 1–5 or 1–10 consistently — PROVEN: ODI
permits either; [productplan.com/glossary/opportunity-scoring](https://www.productplan.com/glossary/opportunity-scoring/)):

- **Importance** — how important is making progress on this job?
- **Satisfaction** — how satisfied are you with your *current* way of getting it done (the
  fragmented stack)?

Compute the **opportunity score** per the ODI algorithm:

> **Opportunity = Importance + max(Importance − Satisfaction, 0)** — importance is weighted
> twice; jobs scoring **high-importance / low-satisfaction** are the underserved opportunities
> (PROVEN — Ulwick/Strategyn; [strategyn.com/jobs-to-be-done](https://strategyn.com/jobs-to-be-done/);
> [airfocus ODI guide](https://airfocus.com/blog/jobs-to-be-done-outcome-driven-innovation-ulwick/)).
> *Note: ODI thresholds are relative (rank order), not absolute cut-offs — interpret as a
> ranked opportunity landscape, not pass/fail.*

### 6.2 With whom (per audience — required, because the dual-audience bet hinges on it)
Recruit **separately by audience** so the same-data pairs in §5 can be compared *across*
audiences (the §5 D1–D5 pairs are the highest-value rows to rank):
- **Engineers:** ≥ 8–12 each spanning P1 backend, P2 frontend, P3 platform/SRE, P4 staff,
  P5 OSS maintainer — across the org archetypes (solo/startup, scale-up).
- **PM/delivery:** ≥ 8–12 spanning P6 PM, P7 EM, P8 PgM, P9 designer, P10 writer.
- **Corporate/governance:** ≥ 6–10 spanning P11 CTO/VP, P12 security, P13 DPO, P14
  procurement, P15 IT-admin — heavily weighted to **regulated-enterprise & public-sector**
  archetypes (where C-jobs are decisive; `personas.md §6`). *(This recruit overlaps R-19's
  regulated-buyer review and R-05's persona validation — run jointly to save fieldwork.)*

Method: a **survey** (n large enough to rank) for the importance/satisfaction numbers, paired
with **5–8 contextual interviews per audience** to validate that the job *stories themselves*
are real and well-worded before scoring (avoid scoring a mis-stated job — Klement: be specific,
not vague).

### 6.3 What would falsify a "this is a top job" hypothesis
A job we *implicitly* treat as central (e.g. **E3** the failing-CI→line wedge flow; **M1**
roadmap-reflects-delivery; **D1** the dual-audience issue model; **G5** the DSR job) is
**falsified as a top job** if, in the ranked results:
- its **opportunity score lands mid/low** (importance not high, or current satisfaction
  already high — i.e. the fragmented stack is "good enough"); **or**
- it ranks high for **only one audience** when we claimed it as a cross-audience anchor (this
  specifically falsifies a §5 dual-audience pair — if D1 scores high for PMs but engineers are
  already satisfied with their board, the "serves both equally" premise weakens); **or**
- interviews reveal the **situation never actually occurs** for the recruited segment (the job
  is theoretical).

Conversely, a job we under-weighted that scores **high-importance/low-satisfaction** is a
discovered opportunity the catalogue must promote. **Record the ranked table and the
falsification outcomes back into this file** when users exist; until then, §2–§5 stand as an
**unranked, HYPOTHESIS-tagged** inventory.

### 6.4 Why we can still proceed without the ranking (the no-user substitute, honestly bounded)
The downstream pipeline (R-04 flows, Phase 5 surface map, Phase 6 sketches) needs the *job
inventory* and the *dual-audience pairs* — both delivered here — to begin. It does **not**
need the ranking to *start*; the ranking sharpens *which finalists to favour* later and is
fed into the rubric's central-problem dimensions (D4/D5/D6). So deferring the ranking does not
block the corpus, and presenting the inventory as "complete but unranked" is the honest state.

---

## 7. Actionability toward the control artifacts

| Control artifact | What this catalogue equips | Where |
|---|---|---|
| **rubric.md D5** (dual-/tri-audience) | The named same-data pairs D1–D5 are the surfaces D5 scores; "neither lens a degraded compromise" has concrete targets. | §5 |
| **rubric.md D4** (one-product coherence) | The cross-surface jobs (E1/E3/M2/G3) are *why* coherence matters — each job spans subsystems and must feel like one product. | §2–§4 |
| **rubric.md D6 / D9** | Agent-collaboration jobs (E11, M9, G4) and sovereignty jobs (G3/G5/G6/G7) give D6/D9 their human-job grounding. | §4, §3 |
| **sketch-funnel Axis 1 (density)** | Engineer throughline → dense; PM/exec throughline → calmer — *over shared components*. | §2/§3 closers |
| **sketch-funnel Axis 3 (surface unification)** | The §5 pairs ARE the unification test the finalists must occupy materially different positions on. | §5 |
| **sketch-funnel Axis 6 (sovereignty visibility)** | The C-audience throughline (legible-not-buried) sets the always-on↔on-demand tension. | §4 closer |
| **sketch-funnel comparable screen set** | The dual-audience pairs (esp. D1) must be sketched in **both** lenses. | §5 note |
| **R-04** (next) | Every required R-04 flow is a job here: engineer flagship = **E3**; PM incident = **E9/M6 plane**; DPO DSR = **G5**; agent HITL = **E11/M9 plane**. | §2–§4 |

---

## 8. Completeness-critic (README §9) — which gloss-risks apply to R-03

R-03 is a *jobs* deliverable, not a *states/screens* deliverable, so most §9 unglamorous-UI-
state risks are **consciously deferred to their owning items** — but R-03 must ensure the
*jobs that surface those states exist*:

- **Permission-denied / erased-tombstoned (§9)** — owned by R-09/R-21 as *states*, but the
  **jobs that demand them are present here**: G6/G10 (erasure), G5 (DSR), E12/G2 (no-access).
  Covered as jobs; states deferred to R-09/R-21.
- **The DSR/erasure flow from the data-subject side AND the DPO side (§9)** — named explicitly
  as dual-audience pair **D4** (§5) and jobs **G5/G6**; the *flow* is realised in R-04/R-19.
  Covered.
- **Partial-failure agent branches (§9)** — owned by R-04/R-14 as flow branches; here the
  *agent-collaboration jobs* that lead into them are present (E11, M9, G4). Job-level covered;
  branch states deferred to R-04.
- **The CLI as a peer surface (§9, design-language §7.7)** — covered: CLI verbs named on
  engineer jobs E1/E3/E7 so "finish the job in either rendering" is explicit, not forgotten.
- **Storm / 30×-agent-surge inbox (§9)** — the *job* (M5 calm inbox, G4 agent-volume
  governance) is present; the *under-load state* is deferred to R-21. Named, deferred.
- **Touch/mobile, conflict-surfacing, optimistic-rollback (§9)** — out of scope for a jobs
  catalogue; consciously deferred to R-13/R-21 (states) — no job is hidden by omitting them.

---

## 9. Self-check against R-03 acceptance criteria

| Criterion (from prompt R-03 / ws-b) | Status | Evidence |
|---|---|---|
| **All three audiences have jobs (corporate/governance NOT skipped)** | ✅ Met | §2 (A engineers, 12 jobs), §3 (B PM/delivery, 10 jobs), §4 (C corporate/governance, 10 jobs) |
| **Every job maps to a §7 surface AND a persona** | ✅ Met | Every row in §2–§4 has Persona(s) + §7 surface column |
| **Every job is HYPOTHESIS-tagged** | ✅ Met | All rows tagged **HYP**; method tagged **PROVEN-theory** (§0); cited |
| **Dual-audience same-data pairs named explicitly** | ✅ Met | §5 names D1–D5 incl. the prompt's example (P1 burn-down-cycle ↔ P6 communicate-roadmap over one issue model = **D1**) |
| **Deferred importance×satisfaction ranking recorded as a plan, NOT faked** | ✅ Met | §6: what/with-whom/falsification + ODI formula cited; §2–§5 deliberately carry **no priority claims** |
| **Build ON canon, don't re-derive P1–P9 / §7 / §2** | ✅ Met | Applies §2 dual-audience & §7 catalogue; references not restates |
| **Date the file; tag claims; name uncertainties** | ✅ Met | Dated 2026-06-20; PROVEN/HOUSE-STYLE/HYP tags throughout; §4 sovereignty-as-UX flagged under-evidenced |
| **Web-grounded, cited URLs** | ✅ Met | JTBD/ODI/job-story sources cited in §0 and §6 |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §8 names which apply, covers job-level ones, consciously defers state-level ones to owning items |
| **Actionable toward rubric & sketch-funnel** | ✅ Met | §7 maps jobs → D4/D5/D6/D9, Axes 1/3/6, comparable-screen set, and the R-04 flows |

**Honest partials / top uncertainties.**
1. **No ranking** — by design (§6); the catalogue is an *unranked* inventory until users exist.
   The biggest risk is that we have *invented jobs no one holds*, or *missed a top job*; only
   §6's study resolves this.
2. **Persona priority is unvalidated** (`personas.md §0`): the assumption that scale-up &
   regulated/public-sector are the strongest fits is a strategy hypothesis, so the *weight* of
   the C-audience jobs is itself a HYPOTHESIS (feeds R-05).
3. **Sovereignty-as-UX jobs (G3/G5/G6/G7) are HOUSE-STYLE / under-evidenced** — no external
   playbook; R-19's deferred regulated-buyer review is the real test.
4. **Dual-audience D4** (data-subject ↔ DPO over one personal-data graph) is the least
   conventional same-data pair and the one most likely to need re-shaping after R-16/R-19.

---

*End of R-03 deliverable. Date: 2026-06-20. Method PROVEN; all specific instantiations
HYPOTHESIS (no users). Feeds R-04, R-05, R-16, Phase 5, Phase 6.*
