# R-15 — Agent Attribution / Audit + Calm-Agent-Volume Patterns; the Trust-Calibration Plan

> **Phase 4 research corpus** · deliverable of prompt **R-15** (workstream
> [`ws-f-agent-ux.md`](../../02-research-roadmap/ws-f-agent-ux.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0). This file is the **second half of the
> agent-UX corpus**: it depends on and **extends** R-14
> ([`legibility-and-hitl.md`](./legibility-and-hitl.md) — the agent treatment, the plan-then-apply card,
> Approve/Edit/Reject, the state set, the per-surface HAX-18 audit). R-14 specified *legibility at the
> moment of action*; **R-15 specifies what survives the action**: per-action **provenance**, the
> **audit-trail walk**, **scope/budget/delegation legibility**, the **governance console + kill-switch**,
> and **calm agent volume** (keeping agent output OUT of the main timeline). It then records the
> **`[DEFERRED-UNTIL-USERS]` PAIR-style trust-calibration study**.
>
> Feeds **rubric D6** (agent legibility & trust) and **D9** (sovereignty / GDPR-as-UX legibility), with a
> direct hand to **D7** (density-made-calm) on the agent-volume patterns; feeds **sketch-funnel Axis 5**
> (agent presence) and touches **Axis 6** (sovereignty visibility, jointly owned by R-19).

This is the **visible half of design-language §6.4 (attribution/audit) + §6.5 (calm volume)** made precise.
It **does not re-derive** §6 or P7/P8/P9 — it *applies* them and **surfaces existing backend mechanics** as
UI: the `run` envelope and budgets/loop-guards in
[`agent-fabric.md`](../../../planning/05-refined-shared-systems-architecture/agent-fabric.md), the
attribution fields in [`agent-native-design.md`](../../../planning/01-research/agent-native-design.md) §5.5,
and **the tamper-evident audit log that *is* the "why did this happen" walk** in
[`gdpr-and-audit.md`](../../../planning/05-refined-shared-systems-architecture/gdpr-and-audit.md) §6.
**It invents no new backend mechanism.**

## 0. How to read this file · method · tags

**Three methods, per the prompt:**
1. **Google PAIR — People + AI Guidebook** *(PROVEN method; principles applied **now**, the
   trust-*calibration testing* is DEFERRED to §B). Chapters used: **Explainability + Trust**, **Feedback +
   Control**, **Mental Models**, **Errors + Graceful Failure**.
   [Explainability + Trust](https://pair.withgoogle.com/chapter/explainability-trust/);
   [Feedback + Control](https://pair.withgoogle.com/chapter/feedback-controls/);
   [Mental Models](https://pair.withgoogle.com/guidebook-v2/chapter/mental-models/);
   [Guidebook overview](https://medium.com/google-design/people-ai-guidebook-41ec2ee5ec3f);
   [gen-AI update](https://medium.com/people-ai-research/updating-the-people-ai-guidebook-in-the-age-of-generative-ai-cace6c846db4).*
2. **Microsoft HAX-18** — specifically **G11 "make clear why the system did what it did"**, **G16 "convey
   consequences"**, **G15 "encourage granular feedback"**, **G17 "global controls"**, **G18 "notify about
   changes"** *(PROVEN, Amershi et al., CHI 2019; cited in full in R-14 §0; not repeated here).*
3. **NN/g + current agentic-governance doctrine** on provenance/audit — a compliant agent audit trail is
   **operation-level, attribution-complete, tamper-evident, real-time**
   *([Galileo, AI agent compliance & audit trails, 2025](https://galileo.ai/blog/ai-agent-compliance-governance-audit-trails-risk-management);
   [Kiteworks, tamper-evident agent audit trails, 2025](https://www.kiteworks.com/regulatory-compliance/ai-agent-audit-trail-siem-integration/)).*
   Myelin already meets all four structurally (hash-chain + Merkle, §6 below) — these sources *validate the
   bar*, they do not set new mechanism.

**Tags.** **PROVEN** = grounded in a cited standard/source, an AI-Act/GDPR legal duty, or an existing
architecture mechanism we *surface* (not invent). **HOUSE STYLE** = our design synthesis/taste. Every
*specific visual choreography* below is HOUSE STYLE; the *mechanisms* (the `run` provenance envelope, the
tamper-evident log as the provenance walk, budgets, loop guards, the agent-lane shed budget, kill-switch via
autonomy policy) are PROVEN against the architecture. **Nothing here is user-validated** (see §B
`[DEFERRED-UNTIL-USERS]`).

**The one design thesis that governs everything below (HOUSE STYLE):** *trustworthiness is a property of the
**contract**, not the runtime.* Mock agents today and LLM agents tomorrow flow through the **same** provenance
envelope, the **same** audit log, the **same** budgets and kill-switch. So we design the **attribution and
volume contract** to be trustworthy regardless of what the agent *says*; the deferred study (§B) validates
the contract's *legibility*, not any specific model's output.

---

# PART 1 — PATTERNS

## 1. The per-action provenance affordance — who / what / on-behalf-of / trigger / correlation-id

Every agent action (and, symmetrically, every human action — same model, EI-02 §2) carries a **provenance
envelope**. R-14 §1 made "an agent did this" *legible at a glance*; this section makes **"exactly who,
under what authority, why, and as part of which chain"** *answerable on demand*. Each field is a **real
column of the `run` record / `audit_entry`** — we render it, we don't invent it.

### 1.1 The five provenance fields (each maps to a real backend field)

| Field (UI label) | Backs onto (PROVEN mechanism) | What it answers | Rendered as |
|---|---|---|---|
| **Who** (`actor`) | `run.agent_principal` (a `Principal kind=Agent`); `audit_entry.actor` = the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (gdpr-and-audit §6.3) | *which agent acted* | the §1-R-14 **agent treatment** (label+icon+color+attribution), name as a live identity chip |
| **What** (`effect` / `tool`) | the `ProposedEffect` / `ToolDef` name + target `ArtifactRef` (R-14 §2) | *what it did, on which artifact* | verb + live **reference chip** (§5.3 — permission-aware, never leaks) |
| **On behalf of** (`on_behalf_of`) | `run.on_behalf_of` (the delegating human/team, agent-native §1.3 — the **intersection** of authority, never the agent's own ceiling) | *under whose delegated authority* | a second identity chip: `on behalf of @dev` |
| **Trigger** (`trigger`) | `run.trigger_event` + `binding_id` (the subscription that woke the run, agent-fabric §8.6 — **explicit-first**: a mention notifies, never auto-spawns a costed run) | *why it happened — what event/mention set it off* | humanised event chip: `triggered by ci.failed on repo/api #88` |
| **Correlation** (`correlation_id`) | `run.correlation_id` / `causation_id` / `depth` (agent-fabric §5.5; the **same id the loop-guard caps**) | *which multi-step chain this belongs to* | a **thread token** (`correlation: incident-#9`) that links every step across all five surfaces (the R-22 wedge) |

**Design rules (HOUSE STYLE unless noted):**
- **The provenance envelope is the same object everywhere** an agent appears — PR reviewer row, issue
  comment, chat message, doc edit, audit row, the plan card header. Built once (P1 coherence), inherited.
  *(HOUSE STYLE; the single-model duty is PROVEN — EI-02 §2 "agents flow through the same path as humans.")*
- **Always humanised, never a raw id.** `on behalf of @dev`, not a UUID; `triggered by ci.failed`, not an
  event topic string. One humanisation surface (agent-fabric C9 / §8b.5). *(PROVEN mechanism.)*
- **`correlation_id` is shown as a clickable thread, not printed.** Clicking it filters every surface to that
  chain (the "why did this whole thing happen" walk). *(HOUSE STYLE; the id is PROVEN.)*
- **Minimised by design.** Provenance shows pseudonymous actors + `ArtifactRef`s, **never payloads** — this
  is how the same affordance is GDPR-safe and audit-grade at once (gdpr-and-audit §6.3). *(PROVEN.)*

### 1.2 The inline "why did this happen?" affordance (HAX G11; PAIR explain-tied-to-action)

A persistent, one-click **`Why?`** control on any agent-authored artifact or effect (it appears on the plan
card per R-14 §2; here it is generalised to *anything an agent touched*). It expands a small, partial
explanation — **not a model dump** — answering, in order:

1. **What** acted and **what** it did (the `what`/`who` fields).
2. **Trigger** — the event/mention that woke it (`triggered by …`), tying explanation to a concrete cause.
   *(PAIR: "tie explanations to user actions … establish clear cause-and-effect"; HAX G11.)*
3. **Authority** — `on behalf of @dev`, within scope `repo/api` (the delegation intersection — §3).
4. **Chain** — "part of `incident-#9`" → the `correlation_id` thread link.
5. **Audit** — a deep link to the full tamper-evident record (§2).

**PAIR "partial explanation" rule (PROVEN method):** show *decision-critical* provenance inline; push the
exhaustive record to the audit explorer behind one click. Detail scales with stakes — a routine triage
label needs one line; a gated `merge` or an `erase` shows scope+consequence prominently
*([PAIR Explainability + Trust](https://pair.withgoogle.com/chapter/explainability-trust/): "account for
situational stakes … detailed explanations in high-risk scenarios, reduce in low-stakes").* *(HOUSE STYLE
choreography; PAIR principle PROVEN.)*

> **Note on agent "confidence" (PROVEN-by-architecture caveat).** PAIR offers confidence displays
> (categorical High/Med/Low, N-best, numeric %) as a calibration aid. **Myelin shows confidence only as a
> categorical/N-best *suggestion strength* where the runtime supplies it, never a fabricated number** — and
> the *mock* runtime supplies none, so v1 surfaces **capability/scope statements** (HAX G1/G2) rather than a
> confidence score. PAIR itself warns: *"avoid showing confidence displays if they could mislead
> less-sophisticated users into blind acceptance."* The trustworthy signal is **what the agent may do and
> what it's for**, not a number it can't honestly produce yet. *(HOUSE STYLE decision; PAIR caveat PROVEN.)*

---

## 2. The audit-trail link — the "why did this happen" walk as a first-class surface

**The load-bearing mechanism (PROVEN):** gdpr-and-audit §6.3 — *"the audit log **is** the 'why did this
happen' walk — one mechanism for audit + provenance + the loop guard."* One **per-tenant hash-chain whose
entries are Merkle leaves**, CT-style inclusion + consistency proofs (RFC 6962; Trillian/CT model;
**deliberately not a blockchain** — residency-safe, self-hosted in-cell, anchored to an independent witness).
**Every human AND agent action** is one entry. This is exactly the **operation-level, attribution-complete,
tamper-evident, real-time** bar the 2025 governance literature demands
([Galileo](https://galileo.ai/blog/ai-agent-compliance-governance-audit-trails-risk-management);
[Kiteworks](https://www.kiteworks.com/regulatory-compliance/ai-agent-audit-trail-siem-integration/)) — met
structurally, not bolted on.

### 2.1 The audit-log explorer (§7.6) — what the UI surfaces

| Surface feature | Backs onto (PROVEN) | Design (HOUSE STYLE) |
|---|---|---|
| **One link from any action → its audit entry** | `audit_entry` is written via the outbox for every action (gdpr-and-audit §6.3) | the `Audit trail` link in §1.2's "Why?" panel and on the plan card (R-14 §2) opens the explorer **filtered to that entry** |
| **The `correlation_id` chain view** | `correlation_id`/`causation_id`/`depth` carried on every entry | the explorer's primary view: a **provenance thread** showing the whole agent chain end-to-end (CI fail → triage run → issue filed → chat post → proposed PR → approval), each hop attributed, in causal order |
| **Verifiable, not just visible** | CT inclusion proof ("this action is in the log") + consistency proof ("the log wasn't forked/rewritten") | a quiet **"verified ✓"** affordance a DPO/auditor (P12/P13) can expand to the proof — *tamper-evidence is felt, not claimed* (D9) |
| **Minimised actors** | `actor`/`on_behalf_of`/`subject` = pseudonyms + `ArtifactRef`s, never payloads | the explorer **never shows payloads**; it shows who-did-what-to-which-artifact, GDPR-safe by construction |
| **Erased entries degrade gracefully** | identity erasure → actor reads as the tombstone pseudonym; the entry/hash is **never rewritten** (chain integrity); content holders crypto-shred separately | an erased subject's actions still appear as *"[erased subject] did X"* — the **GDPR-aware tombstone state** (R-14 §5 / §5.3), never a broken row, never a leak |

**Design rules:**
- **The audit link is one click from the act, always** — never buried in a settings tab. "Show me
  everything about this action / this chain / this subject" is answerable in the UI (D9 bar). *(HOUSE STYLE;
  the data is PROVEN.)*
- **Two reading lenses, one log:** the **operator/security lens** (P12/P15 — filter by agent, scope, outcome,
  time) and the **provenance lens** (anyone — "why did *this artifact* end up like this", following the
  chain). Same `audit_entry` rows, two queries. *(HOUSE STYLE.)*
- **Agent trace ≠ audit log (PROVEN distinction, gdpr-and-audit §6.5 / holder H17 vs H16).** The audit log is
  the tamper-evident *who-did-what*; the **agent execution trace** (the run's reasoning record) is a separate
  crypto-shreddable Knowledge doc. The "Why?" panel links the **audit entry** (always) and, where the run
  retained one and the viewer is permitted, the **trace** ("see the agent's reasoning") — clearly labelled as
  the *reasoning record*, distinct from the audit fact. *(PROVEN.)*

---

## 3. The scope / budget / delegation inspector — legible authority and consumption

§6.4 demands *"an agent's current permissions/delegation and budget are inspectable."* This is the per-agent
(and per-run) inspector that makes **"what may this agent touch, and how much has it spent"** answerable
without reading policy code.

| Inspector panel | Backs onto (PROVEN) | Conveys |
|---|---|---|
| **Effective scope** | `agent.policy ∩ delegation ∩ tenant.policy` (agent-fabric §5.2; agent-native §1.3 intersection — **the agent can do nothing no human role can**, EI-02 §2) | *what it may touch right now* — shown as concrete `ArtifactRef` scopes + tool list, not raw tuples |
| **Delegation source** | `on_behalf_of` + the binding that granted it | *whose authority it borrows* and that authority is **bounded below**, never above, the delegator |
| **Budget** | `run.budget` (integer minor-units) + reserve/settle gate (agent-fabric §5.4) | *how much it may spend and how much remains* — a live `3 / 12 effects · est. cost` meter (R-14 §2), so **budget-exceeded is never a surprise** (HAX G16 consequences) |
| **Loop ceilings** | `max_steps`, causal-`depth` ceiling (default 12), shared-root tripwire → per-tenant circuit breaker (agent-fabric §5.5) | *the automation can't run away* — surfaced as a quiet "automation limits" line, foregrounded only when tripped (R-14 §5 `loop-guard-tripped`) |

**Design rules (HOUSE STYLE; mechanisms PROVEN):**
- **Scope reads as "may act on …", never as policy syntax.** The inspector projects the intersection into
  plain `ArtifactRef` scopes + verbs (the same projection the plan card's *Scope* line uses, R-14 §2).
- **Budget is always-on on the plan card, expandable in the inspector.** Consequences-before-action (HAX G16)
  is a default, not a toggle.
- **The inspector is reachable from three places:** the plan card (this run's authority), an agent's identity
  chip (this agent's standing authority), and the governance console (all agents — §4). One component, three
  entry points (P1 coherence).

---

## 4. The agent governance console + kill-switch (§7.6) — the admin's overall control (HAX G17)

The org-level surface where P12 (security) / P15 (admin) hold **overall control of agent behaviour** — HAX
**G17 global controls** made literal, and PAIR **Feedback + Control** ("meaningful control options are
essential for trust";
[PAIR Feedback + Control](https://pair.withgoogle.com/chapter/feedback-controls/)).

| Console capability | Backs onto (PROVEN) | Design (HOUSE STYLE) |
|---|---|---|
| **Roster of agents** | every agent is a `Principal` | list each with the §1 treatment, its scope, delegation, budget, and standing autonomy policy |
| **Autonomy policy = suggest-by-default** | §6.3 frozen defaults (merge/deploy/erase = gated); autonomy granted **per-action, per-scope** by policy owners, **never autonomous-by-default on consequential actions** | a per-action/per-scope policy editor; **raising autonomy is itself a gated, audited admin action** (HAX G14 update cautiously, G18 notify about changes) |
| **Kill-switch** | the per-tenant **circuit breaker** + the run-pausing durable gate (agent-fabric §5.5) | a prominent **"pause all agents" / "pause this agent"** control; pausing settles in-flight runs to a coherent partial state (saga semantics, R-14 §5), **never a corrupt half-mutation** |
| **Agent audit** | the same audit log (§2), filtered to agent actors | "what have the agents done" — the operator lens of §2 |
| **Storm/surge controls** | the **per-surface agent-lane shed budget** (agent-fabric C10/OQ-K: per-tenant agent-run in-flight cap; **humans never queue behind agents**; `429 + Retry-After`) | the console shows the lane's health and the shed budget; under a 30× surge the **human lane holds, the agent lane sheds** — visible, governed, not a mystery slowdown (D-6 demo) |

**Design rules:**
- **The kill-switch is always one action away, never confirmed-to-death.** A panic control that takes five
  dialogs is not a panic control. One click pauses; a second confirms *if* the scope is org-wide. *(HOUSE
  STYLE; the circuit breaker is PROVEN.)*
- **Every governance change is announced + audited (HAX G18).** Raising an agent's autonomy or budget posts
  to the audit log and notifies affected owners — *no silent loosening* (the §6.3 floor). *(PROVEN.)*
- **⚠ Doctrine-wins (carried from R-14 §9):** suggest-by-default is **never** loosened to
  autonomous-by-default for a consequential action without a written deviation. HAX permits adaptive autonomy;
  doctrine forbids it as a default. Doctrine wins. *(PROVEN — §6.3.)*

---

## 5. Calm agent volume — keeping the agent OUT of the main timeline (§6.5; P8; D7)

The central R-15 *experience* problem: agents generate volume (review comments, triage updates, status posts,
chain steps), and **an agent-native product that dumps that volume into the main human stream has lost P8 and
D7 before it starts.** §6.5 is the doctrine; this is the pattern set that realises it.

### 5.1 The four calm-volume patterns

| Pattern | What it does | Backs onto | Tag |
|---|---|---|---|
| **Threading (Zulip-style topics, considered)** | Agent participation belongs to a **named topic within a channel**, not the flat stream. A late joiner reads the topic, not the firehose. | §6.5 names Zulip-topic threading *specifically because agent participation raises volume* (competitive-landscape §5) | **HOUSE STYLE** (decision); the volume problem is **PROVEN** |
| **Collapsible summaries** | A multi-step agent chain collapses to **one line + count** in the timeline ("TriageAgent did 4 things · expand"), expandable to the full chain; the `correlation_id` ties them | §6.5; the chain *is* the `correlation_id` walk (§2) | **HOUSE STYLE**; chain is PROVEN |
| **Inbox routing** | What needs a **human decision** (a gate) routes to the **unified inbox** (R-10 / §5.8) with "why am I getting this"; what's merely *informational* (a triage note) stays **out of the inbox and out of the main stream**, available on the artifact | §6.5; the durable gate's second home is the inbox (R-14 §4) | **PROVEN** (inbox is the gate's second home); routing split is **HOUSE STYLE** |
| **Agent-out-of-main-timeline (default)** | The default placement of routine agent chatter is **threaded / collapsed / on-artifact**, never the main timeline. The main stream is for humans + **gates that need you**. | §6.5; P8; D7 | **HOUSE STYLE**; doctrine PROVEN |

**Why Zulip topics specifically (PROVEN observation, HOUSE STYLE adoption):** in Slack-style channels every
message is one continuous stream with threads hidden in a sidebar; in **Zulip every message belongs to a
named topic within a stream**, so a reader sees *topics*, not a flat list, and can join hours later with
context intact — strong for **asynchronous, high-volume** participation
*([Capterra Slack vs Zulip](https://www.capterra.com/compare/135003-197945/Slack-vs-Zulip);
[Why Zulip](https://zulip.com/why-zulip/)).* Agent chatter is exactly high-volume + async, so the
topic-per-concern model contains it where flat channels would drown the human. **We adopt the *topic
discipline* for agent volume** (a chain = a topic) without mandating Zulip's full IA — the IA decision is
R-06's; this is the *agent-volume* rationale feeding **sketch-funnel Axis 5**.

### 5.2 The storm / surge experience (the §9 gloss-risk this file co-owns)

The **notification storm / 30×-agent-surge** is the calm-volume pattern's stress test. Two distinct surges,
two designed responses:

- **The infra surge (PROVEN mechanism — agent-fabric C10/OQ-K):** too many agent *runs* in flight → the
  **agent-lane shed budget** sheds the agent lane (`429 + Retry-After`) so **humans never queue behind
  agents**; other tenants are unaffected (D-6). The governance console (§4) shows lane health. *This is
  fairness, enforced below the UI.*
- **The attention surge (HOUSE STYLE; this file's pattern):** many gates/notifications arriving at once →
  the inbox **groups** them ("7 approvals awaiting you" with per-item Approve/Edit/Reject, R-14 §4), and the
  main timeline shows **one collapsed line**, never 30 rows. The **one prioritised inbox** discipline (D7)
  means the human triages a *bounded, ranked* list, not a firehose.

**The completeness-critic §9 risks this file addresses:** **notification storm / surge** — **co-owned**
(infra shed budget surfaced; attention-grouping pattern specified; the full *state-craft* of the storm inbox
is **R-21**'s, named). **Calm-agent-volume / agent-out-of-main-timeline** — **OWNED & covered** here (the R-14
§8 handoff). **Cross-cell / no-leak in provenance** — **covered** (§1.1 / §2.1 minimised actors + tombstones,
ADR-03). **Stale/erased audit rows** — **covered** (§2.1 erased-degrades-gracefully).

### 5.3 Axis-5 hook (agent presence) and the Axis-6 touch (sovereignty visibility)

This part **is** the variable in sketch-funnel **Axis 5**:
- **Ambient pole:** agent chatter fully threaded/collapsed, gates inbox-first, the main stream almost
  agent-free — the calm extreme.
- **Foregrounded pole:** the agent is a visible inline collaborator (proposals on the diff, a participant in
  the topic) — more present, but **still threaded, still gated, provenance still one click away**.
- **The invariant across the axis (the R-14 carry-over):** Axis 5 varies *presence and placement*, **never
  legibility, attribution, gating, or auditability** — those are floors held constant at every point.

**Axis-6 touch (jointly R-19):** the §1 provenance envelope and §2 audit walk are *also* sovereignty cues —
"who/what processed this, under whose authority, where" is part of P9. R-19 owns the residency/DSR side; R-15
owns the **agent provenance/audit** thread that feeds it.

---

# PART 2 — `[DEFERRED-UNTIL-USERS]` THE PAIR-STYLE TRUST-CALIBRATION STUDY

> **This is a plan, not a result. Nothing in Part 1 is user-validated.** Part 1 is *expert-authored patterns
> + a PAIR/HAX heuristic application* (the no-user substitute, per the standing preamble §C). The decisive
> question — **do users correctly understand what the agent can and can't do, and trust it the right amount?**
> — can only be answered with real users. Below is the **executable protocol**, the participants, and the
> falsification criteria.

## A. The research question (PAIR "Mental Models" + "Explainability + Trust")

**Appropriate trust = calibrated trust** *([PAIR Explainability + Trust](https://pair.withgoogle.com/chapter/explainability-trust/)):*
not blind acceptance (over-trust → rubber-stamping a bad agent merge) and not blanket rejection (under-trust →
the agent provides no value). Concretely:

1. **Capability legibility (PAIR Mental Models; HAX G1/G2):** do users correctly state *what the agent can
   and can't do* before they act — from the scope/budget inspector (§3), the plan card (R-14 §2), and the
   capability statement (§1.2)?
2. **Provenance comprehension (HAX G11; §1):** after an agent acts, can users answer *who/what/on-behalf-of/
   why/which-chain* using the provenance envelope and the "Why?" affordance — without help?
3. **Audit findability (D6/D9; §2):** when something looks wrong, do users find the audit trail and reconstruct
   *what happened* — and does a DPO/auditor (P13) trust the tamper-evidence "at a glance"?
4. **Trust calibration (the decisive one):** do users **approve the good plans and catch the bad ones** — i.e.
   does the legibility design move behaviour toward the agent's *actual* reliability, not above or below it?
5. **Calm-volume efficacy (P8/D7; §5):** under agent volume, do users still find the gate that needs them, and
   do they report the surface as *calm* rather than *noisy*?

## B. Method, with whom, and what we'd run

| # | Study | Participants (personas → real segments) | Stimulus | Primary measure |
|---|---|---|---|---|
| **B1** | **Mental-model elicitation** (PAIR Mental Models) | mixed **engineers (P1–P5)** + **PMs (P6–P10)**, n≈8–12/segment | the chat HITL flow + the agent-reviewed PR (R-14 surfaces) | pre-task: "what do you think this agent can/can't do?" vs the *actual* scope — gap = miscalibration risk |
| **B2** | **Trust-calibration task** (the core) | same | **seeded plans of known quality**: some *correct* agent plans, some *subtly wrong* (wrong branch, over-broad scope, an inappropriate gated merge) | **decision accuracy**: % correct approvals **and** % bad plans caught/edited/rejected. Over-trust = approving the bad ones; under-trust = rejecting the good ones |
| **B3** | **Provenance / "why" walk-up** (HAX G11; §1–§2) | same + **P12 security / P15 admin** on governance + audit | a completed multi-step agent chain; ask "why did this happen / who authorised it / show me the record" | task success + time; did they use "Why?" and the `correlation_id` thread; did they reach the audit entry |
| **B4** | **DPO/auditor trust** (D9; joint with R-19) | **P13 DPO / P14 counsel** (regulated-buyer lens) | the audit-log explorer + tamper-evidence affordance | "would you trust this record in an Art. 28 audit?" + can they verify inclusion/consistency |
| **B5** | **Calm-volume / storm** (P8/D7; §5) | mixed engineers + PMs | the surface under a **simulated 30× agent surge** (grouped inbox + collapsed timeline) | can they find the gate that needs them; subjective calm rating; missed-gate rate |

**Instruments (PAIR's own calibration questions, verbatim — PROVEN):**
- *"On this scale, show me how trusting you are of this recommendation."*
- *"What questions do you have about how the system came to this?"*
- *"What, if anything, would increase your trust?"*
- *"How satisfied are you with the explanation provided?"*
*([PAIR Explainability + Trust](https://pair.withgoogle.com/chapter/explainability-trust/).)* Pair the
subjective trust rating with the **objective B2 decision accuracy** — calibration is the *match between the
two*, which is the whole point (a confident-but-wrong user is the failure mode).

## C. What would falsify our design hypotheses

Each Part-1 pattern carries a falsifiable claim. The design **fails** if:

1. **Over-trust:** in B2, users approve a meaningful fraction of the *subtly-wrong* plans → the plan card +
   provenance failed to convey consequences (HAX G16 unmet; §1/§2 insufficient). **This is the headline
   failure** — a legible-*looking* card that gets rubber-stamped.
2. **Under-trust:** users reject *correct* plans at a rate that makes the agent net-negative → legibility
   tipped into alarm; calibration overshot.
3. **Provenance opacity:** in B3, users can't answer who/why/under-whose-authority from the envelope without
   help → §1 failed.
4. **Audit unfindable / distrusted:** in B3/B4, users don't reach the audit trail from the act, or a DPO
   doesn't trust the tamper-evidence at a glance → §2 / D9 failed.
5. **Capability mismatch:** B1 mental models diverge sharply from actual scope → §3 inspector + capability
   statements failed to set expectations (PAIR Mental Models).
6. **Volume noise:** in B5, users miss the gate that needs them, or rate the surface "noisy" under surge →
   §5 calm-volume patterns failed (P8/D7).

## D. The mock-vs-real runtime caveat (PROVEN-by-architecture — the load-bearing honesty note)

**Mock-agent trust may not predict real-LLM trust.** Everything testable today runs against the **mock**
runtime; a mock produces deterministic, scripted plans, so a study against it measures *can users read and
calibrate to a plan*, **not** *can users calibrate to a real LLM's variable, sometimes-wrong, sometimes-
confidently-wrong output*. Real LLMs introduce: variable plan quality, plausible-but-wrong rationales, and
the over-trust pull of fluent natural language — none of which the mock exhibits.

**The mitigation is structural, and it is the design's whole bet (HOUSE STYLE thesis; mechanism PROVEN):**
the **contract** — plan-then-apply, the durable gate, the intersection scope, budgets, loop guards, one
`correlation_id`, the tamper-evident audit log, attribution on every action — is designed to be trustworthy
**regardless of runtime**. The strategy-pattern payoff (§6 closing note; ADR-08) means the **exact same
provenance/audit/calm UI** renders for mock and real. So:
- **What B1–B5 validate now (against mock):** the *legibility of the contract* — can a user read the plan,
  find the provenance, reach the audit, and triage volume calmly. This is real signal and worth running.
- **What must be re-run against the real runtime later (cannot be skipped):** **B2 trust calibration**
  specifically, because the over-trust-from-fluency effect is a *property of the LLM's output*, not the
  contract. The plan re-runs B2 with the `LlmAgentRuntime` the moment it exists; the design's claim is that
  the *contract* holds and only the *frequency* of bad-plan-encounters shifts.

---

## 6. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric.md D6** (agent legibility & trust, 12%) | Completes R-14's D6 equipment with the *attributable + audit-linked + trust-calibrated* half: per-action provenance (§1), the audit walk (§2), scope/budget inspector (§3), governance/kill-switch (§4), and the **calibration study** that operationalises "trust is *calibrated*, not blind" (Part 2). A finalist scores 4 on D6 only if an agent action is **traceable to its origin in one click** and the agent is **stoppable** (kill-switch present). | §1–§4, Part 2 |
| **rubric.md D7** (density-made-calm, 8%) | The calm-volume pattern set (§5): agent-out-of-main-timeline, threading/topics, collapsible summaries, inbox routing, one-prioritised-inbox, the storm-grouping. "Agent volume kept out of the main timeline" is the literal D7 sub-criterion. | §5 |
| **rubric.md D9** (sovereignty/GDPR-as-UX, 8%) | The audit-log explorer as a *first-class legible surface* (§2): "who processed this / show me everything / verify it" answerable in the UI; minimised actors + erased-degrades-gracefully make it GDPR-safe. Joint with R-19. | §2, §5.3 |
| **sketch-funnel Axis 5** (agent presence) | §5.3 defines the ambient↔foregrounded poles **with attribution/audit/gating held constant**; the Zulip-topic rationale feeds the presence/placement choice. | §5.3 |
| **sketch-funnel Axis 6** (sovereignty visibility) | The provenance envelope (§1) + audit walk (§2) are sovereignty cues; R-19 owns residency/DSR, R-15 owns the agent-provenance thread. | §1, §2, §5.3 |

---

## 7. Self-check against R-15 acceptance criteria

| Criterion (prompt R-15 / ws-f) | Status | Evidence |
|---|---|---|
| **Per-action provenance (who / what / on-behalf-of / trigger / `correlation_id`) specified** | ✅ Met | §1.1 five-field envelope, each mapped to a real `run`/`audit_entry` field; humanised, minimised, coherent-everywhere rules |
| **Inline "why did this happen?" + audit-trail link specified** | ✅ Met | §1.2 "Why?" affordance (PAIR partial-explanation, HAX G11); §2 audit-log explorer = the "why did this happen" walk, one click from any act, with the `correlation_id` chain view |
| **Scope / budget / delegation inspector specified** | ✅ Met | §3 inspector (effective scope = intersection; delegation source; live budget meter; loop ceilings), three entry points |
| **Agent governance console + kill-switch surface specced** | ✅ Met | §4 roster, autonomy policy (suggest-by-default, gated to raise), one-action kill-switch via circuit breaker, agent audit, storm/shed-budget; HAX G17/G18 |
| **Calm-volume patterns concrete (NOT "be calm")** | ✅ Met | §5.1 four named patterns (Zulip-style threading, collapsible summaries, inbox routing, agent-out-of-main-timeline); §5.2 storm/surge (infra shed budget + attention grouping) |
| **Zulip-style topics consideration recorded** | ✅ Met | §5.1 + §5.1 rationale (Capterra/Why-Zulip cited): topic-per-chain discipline adopted for agent volume; full-IA decision left to R-06 |
| **Deferred trust-calibration study executable-as-written + explicitly flagged** | ✅ Met | Part 2 `[DEFERRED-UNTIL-USERS]`: RQs (A), B1–B5 protocol with participants + measures + PAIR verbatim instruments (B), falsification criteria (C) |
| **"Design the contract trustworthy regardless of runtime" caveat recorded** | ✅ Met | §0 thesis + Part 2 §D: mock-vs-real caveat, structural mitigation, what's validated now vs re-run later (B2) |
| **Date; PROVEN/HOUSE-STYLE tags; grounded cited web research** | ✅ Met | Dated 2026-06-20; tags throughout; PAIR (3 chapters), HAX (CHI 2019, via R-14), Galileo + Kiteworks (audit bar), Capterra/Why-Zulip (topics) cited |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §5.2: notification storm/surge co-owned (infra + attention), calm-volume owned, cross-cell-no-leak covered, erased-audit-row covered; storm state-craft deferred to R-21 (named) |
| **Builds on R-14, does not duplicate** | ✅ Met | Explicitly extends R-14 (§0); references R-14 §1 treatment / §2 card / §4 surfaces / §5 states / §9 doctrine rather than re-deriving |
| **Surfaces existing mechanics, invents none** | ✅ Met | Every field/surface maps to `agent-fabric.md` (`run` envelope, budgets, loop guards, C10 shed budget, circuit breaker), `gdpr-and-audit.md` §6 (hash-chain+Merkle audit), `agent-native-design.md` §5.5 (attribution); stated in §0 |

**Honest partials / top uncertainties.**
1. **All patterns are expert-authored, unvalidated.** The headline risk is unchanged from R-14: a *legible-
   looking* provenance/audit design can still be **rubber-stamped** (over-trust). Only Part 2 B2 resolves it,
   and only meaningfully **against the real runtime** (§D).
2. **Confidence display is deliberately thin in v1** (§1.2 note). Mock supplies no honest confidence; we show
   capability/scope instead. Whether users *want* a confidence signal — and whether it helps or harms
   calibration — is a HYPOTHESIS for B2 (PAIR itself warns it can mislead).
3. **Zulip-topic adoption is HOUSE STYLE for agent volume, not a full IA ruling** — R-06 owns the IA; if R-06
   lands on a non-topic IA, the *agent-volume discipline* (chain-as-topic, collapse, route) must still hold
   by other means. Flagged as a cross-item dependency.
4. **Mock-vs-real runtime (§D)** — real-LLM fluency may pull users toward over-trust in ways the mock can't
   surface; the contract is the bet, B2-on-real is the unskippable check.
5. **Audit-explorer scale UX is unspecced here** — at millions of entries the *query/filter* ergonomics
   matter; this file specs the affordances and lenses, not the at-scale interaction (touches R-13 perceived-
   performance and R-21 state-craft; flagged, not owned).

---

*End of R-15 deliverable. Date: 2026-06-20. PAIR method PROVEN (People + AI Guidebook; Explainability+Trust,
Feedback+Control, Mental Models cited); HAX G11/G15/G16/G17/G18 (via R-14); audit bar grounded
(Galileo/Kiteworks 2025); Zulip-topic model cited (Capterra / Why-Zulip). All specific choreography HOUSE
STYLE; surfaces existing agent-fabric + gdpr-and-audit + agent-native mechanics, invents none. Trust
calibration recorded as a `[DEFERRED-UNTIL-USERS]` plan, not faked; the "contract trustworthy regardless of
runtime" caveat recorded. Extends R-14; feeds rubric D6/D7/D9, sketch-funnel Axis 5 (+ Axis 6 touch),
Phase 6.*
