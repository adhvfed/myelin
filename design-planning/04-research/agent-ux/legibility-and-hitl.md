# R-14 — Agent Legibility & the Plan-then-Apply / HITL Trust Pattern Set

> **Phase 4 research corpus** · deliverable of prompt **R-14** (workstream
> [`ws-f-agent-ux.md`](../../02-research-roadmap/ws-f-agent-ux.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0). Trust-*calibration* testing
> is **R-15**'s deferred study; this file specs the **patterns + the HAX-18 audit** (the no-user
> substitute). Depends on **R-04** ([`cross-surface-flows.md`](../jtbd-flows/cross-surface-flows.md),
> esp. **F-AGT-1** — the agent flagship). Feeds **R-15**, **rubric D6** (agent legibility & trust),
> and **sketch-funnel Axis 5** (agent presence). Every finalist's agent/HITL moment is scored
> against this file.

This is the **visible half of the design-language §6 agent-native UX contract** made precise and
implementable. It does **not** re-derive §6, P7, or §8b.3 — it **applies** them and surfaces the
existing [`agent-fabric.md`](../../../planning/05-refined-shared-systems-architecture/agent-fabric.md)
mechanics (effects, gates, attribution, budgets) as UI. **It invents no new backend mechanism.**

## 0. How to read this file · method · tags

**Two methods, per the prompt:**
1. **Microsoft HAX — 18 Guidelines for Human-AI Interaction** *(PROVEN, Amershi et al., CHI 2019;
   validated with 49 practitioners against 20 AI products;
   [Microsoft HAX Toolkit](https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/);
   [Guidelines list](https://medium.com/microsoft-design/guidelines-for-human-ai-interaction-9aa1535d72b9);
   [paper PDF](https://www.microsoft.com/en-us/research/wp-content/uploads/2019/01/Guidelines-for-Human-AI-Interaction-camera-ready.pdf)).*
   The HAX-18 numbering used throughout: **G1–G2 Initially**, **G3–G6 During interaction**, **G7–G11
   When wrong**, **G12–G18 Over time**.
2. **NN/g agentic patterns + the §6.1–§6.5 critique checklist** *(NN/g research agenda + current
   agentic-HITL doctrine — cited inline).*

**The doctrine-beats-HAX rule (the prompt's hard requirement).** Where design-language §6 is
**stricter** than HAX, **doctrine wins** and the conflict is named in **§9**. (HAX is a *floor*; the
§6 contract is *more* specific — ws-f preamble.) Example up front: HAX G3 "time services based on
context" tolerates *proactive interruption*; §6.5 + P8 forbid it by default (agents are calm,
explicit-first; CHAT-1 — a mention notifies, never auto-spawns a costed run). Doctrine wins.

**Tags.** **PROVEN** = grounded in a cited standard/source, an AI-Act/WCAG legal duty, or an existing
architecture mechanism we *surface* (not invent). **HOUSE STYLE** = our design synthesis/taste. Every
*specific visual choreography* below is HOUSE STYLE; the *mechanisms* (plan-then-apply, the durable
gate, per-effect idempotency, attribution, budgets) are PROVEN against `agent-fabric.md`. **Nothing
here is user-validated** (see §10 `[DEFERRED-UNTIL-USERS]`, owned in depth by R-15).

**The §6 critique checklist (applied per surface in §6).** (1) labelled as agent? (2) proposes before
acting? (3) plan legible (concrete effects + authority)? (4) gated on consequential actions? (5)
attributed + audit-linked? (6) volume calm?

---

## 1. The agent treatment — making "an agent did/proposes this" unmistakable (§3.2, §5.11, §8b.3)

The single recognisable visual signature for *any* `Principal` with `kind=agent`, applied wherever an
agent appears (reviewer/author on a PR, commenter on an issue, participant in chat, editor of a doc,
actor in the audit log). **Built once, inherited everywhere** (P1 coherence; one `agent` semantic
token family, §3.2).

### 1.1 The four-channel signature (never color-alone)

| Channel | Spec | Tag |
|---|---|---|
| **Label (text, always)** | The literal word badge — `Agent` — adjacent to the agent's name, e.g. `TriageAgent · Agent`. Text is the **primary** carrier; it survives color-blindness, grayscale, screen-reader, and high-contrast. | **PROVEN** (WCAG 2.2 SC 1.4.1 *Use of Color* — info "not by color alone"; [W3C Understanding 1.4.1](https://www.w3.org/WAI/WCAG21/Understanding/use-of-color.html); AI-Act transparency duty, ADR-08) |
| **Icon (shape)** | One stable agent glyph from the single icon set (§3.7), consistent stroke/weight — **a plain geometric mark, NOT a sparkle / shimmer / magic-wand / star** (§8b.3). The icon disambiguates by *shape* for color-blind users. | **HOUSE STYLE** (the *no-sparkle* rule is HOUSE STYLE; the legibility duty it serves is PROVEN, AI-Act §6) |
| **Color (the `agent` semantic token)** | The reserved `agent` token family (§3.2) — *distinct, consistent, non-alarming* (NOT a functional status color; never reads as success/warning/danger). Color is the **redundant** channel, never the only one. The token is measured-contrast-validated like every semantic pair (§8b.3). | **PROVEN** (color is supplementary per 1.4.1; contrast measured per WCAG AA) |
| **Attribution string** | On any agent-authored content: `on behalf of @<human> · triggered by <event>` resolved via the ONE humanisation surface (`humanise`, contract 7.3 / C9) — never a raw id, never an agent-authored raw string. | **PROVEN** (mechanism = agent-fabric C9 + §8b.5; AI-Act disclosure) |

**The hard prohibitions (carried verbatim from §8b.3, doctrine):**
- **No sparkle / shimmer / magic-wand / star "AI" iconography.** Agents look like *labelled
  principals*, not magic. *(HOUSE STYLE; the duty it serves — "agents are never disguised, never
  magic," §6.1 — is PROVEN.)*
- **No emoji as the agent marker** — an emoji can't inherit `currentColor` or be re-themed for
  dark/high-contrast/RTL. *(PROVEN — §8b.3 rendering constraint.)*
- **Agents are never disguised as humans.** Same identity-badge *component* as a human (§5.11), but the
  agent channels are always present. *(PROVEN — AI-Act, ADR-08.)*

### 1.2 Why the agent treatment is *its own* family, not a status color

A common trap (R-02 lineage): reusing success-green / warning-amber for "agent." That makes the screen
a traffic light (violates §8b.3 "no saturated status fills; the screen is not a traffic light") **and**
conflates "an agent touched this" with "this is good/bad." The `agent` token is a **fourth, neutral
semantic axis** orthogonal to functional status — an agent-authored comment on a *failing* check still
reads red-for-CI **and** agent-for-author, two independent channels. *(HOUSE STYLE, reasoned.)*

---

## 2. Plan-then-apply — the proposed-effects card *before* anything happens (§6.2; ADR-08.3)

The platform-law: **agents emit *proposed* effects; they never perform side effects directly**
(`agent-fabric.md` §5.2, `EffectApi`; ADR-08.3). The UI's job is to render the **plan** — concrete,
reviewable, attributed — *before* `EffectApi::apply` runs. This is the surface that turns
"plan-then-apply" from a backend invariant into *felt trust* (P7; HAX **G1** make-clear-what-it-can-do,
**G11** make-clear-why, **G16** convey-consequences).

### 2.1 The anatomy of a plan card (every field maps to a real `proposed_effect` / `hitl_gate` field)

```
┌──────────────────────────────────────────────────────────────────────┐
│ ⬠ FixAgent · Agent        on behalf of @dev · triggered by ci.failed   │  ← §1 treatment + attribution (C9 humanised)
│                                              correlation: incident-#9  │  ← the correlation_id thread (R-15 deepens)
├──────────────────────────────────────────────────────────────────────┤
│ Proposes 2 effects:                                                    │  ← PLAN = Vec<ProposedEffect> (§4.3)
│  1. Open PR  → repo/api  (fix #88)        authority: open_pr (no gate) │  ← effect · target ArtifactRef · requires_approval
│  2. Merge PR → repo/api  (after checks)   authority: git.merge ⚠ GATE  │  ← consequential → §6.3 frozen default = gated
│                                                                        │
│ Scope:  may act on  repo/api, issue ENG-412   (delegation ∩ tenant)   │  ← delegated authority, NOT the human's full rights
│ Budget: 3 / 12 effects · est. cost shown      (live estimate)         │  ← hitl_gate.cost_estimate, live (§5.3)
├──────────────────────────────────────────────────────────────────────┤
│   [ Approve ]   [ Edit… ]   [ Reject ]            Why? · Audit trail   │  ← §3 controls + §6.4 provenance/audit link
└──────────────────────────────────────────────────────────────────────┘
```

**Each line is grounded, not decorative:**

| Card element | Backs onto (PROVEN mechanism) | Conveys |
|---|---|---|
| **Effect list** ("Open PR", "Merge PR") | `run.proposed_effect` rows (§4.3) — every proposed effect recorded whether applied/gated/denied | *what will change* — concrete, not "FixAgent is working" |
| **Per-effect target** (`repo/api`, `#88`) | the effect's `object` `ArtifactRef` → rendered as the **§5.3 reference chip** (live, permission-aware) | *on which artifacts* |
| **Per-effect authority + GATE marker** | `tool_def.requires_approval` from the **frozen §6.3 defaults** (merge=yes, open_pr=no) | *which effects are consequential* — the gate is shown on the effect that needs it, not the whole card |
| **Scope line** | `agent.policy ∩ delegation ∩ tenant.policy` (§5.2 step 3; intersection, never up) | *under whose delegated authority* — and that the agent can do **nothing no human role can** (EI-02 §2) |
| **Budget line** | `run.budget` (integer minor-units) + `hitl_gate.cost_estimate`, live | *consequences/cost* (HAX G16); pre-empts budget-exceeded surprise |
| **Why? / Audit** | per-action provenance + tamper-evident audit link (§6.4; R-15 owns the depth) | *make clear why* (HAX G11) |

**Design rules (HOUSE STYLE unless noted):**
- **Concrete effects, never a vague "Apply changes."** "Open PR #88, link ENG-412, post to #incidents"
  — the plain-language projection of `Vec<Effect>` (§6.2 example). *Vague = the §6 trap.*
- **The gate marker sits on the effect, not the card.** A mixed card (one ungated `open_pr` + one gated
  `merge`) shows the gate per-effect, so partial approval is legible (ties to §3.3 / per-effect
  idempotency C4). *(HOUSE STYLE; mechanism PROVEN.)*
- **Targets are live reference chips** (§5.3): permission-aware per viewer (an approver who can't see
  `ENG-412` gets the graceful no-access chip, **never a leaked title** — ADR-03), live-not-snapshot,
  tombstones gracefully. *(PROVEN — §5.3 hard rules.)*
- **Cost/scope are always shown, never on a "details" toggle.** Consequences before action is the whole
  point (HAX G16; §6.3 "convey consequences"). *(HOUSE STYLE.)*
- **`--dry-run` parity.** The same card renders from `run --dry-run` (contract 8.7) — the plan is
  inspectable with zero apply. *(PROVEN mechanism.)*

---

## 3. The Approve / Edit / Reject behaviour — backed by a durable gate (§6.3; AG-8)

The controls resolve a **durable-workflow HITL gate** (`hitl_gate`, contract 9.4) that can wait
**minutes or days** holding no runtime (`agent-fabric.md` §5.3, §5.6). The gate is **never silently
lost**: it lives in chat *and* the inbox (§4), reminded until resolved. *(All PROVEN mechanism;
choreography HOUSE STYLE.)*

### 3.1 Approve

- **Effect:** the workflow signal re-runs the withheld step with the tool name added to the approved
  set; `EffectApi::apply` step 6 now passes; the effect applies via the subsystem's public endpoint as
  the agent principal (no carve-out, §5.2 step 7). Card resolves to `Approved by <human> · <when>`,
  attributed.
- **Idempotent by construction (C4):** double-click = one approval (`idem_key = card_id[:effect_idx]`).
  The UI may optimistically flip to "Approving…" then settle; a re-click sends the same key → no
  double-apply. *(PROVEN — agent-fabric C4 / contract 9.1.)*
- **HAX:** G9 efficient correction is *not* the path here — Approve is acceptance; Edit/Reject are the
  correction/dismissal paths (G8/G9).

### 3.2 Reject

- **Effect:** the gate settles `Halted::Rejected` with the reason in the trace + audit; the proposed
  effect is **discarded, not applied** (e.g. the PR is *not* opened — `git.pr.opened` never fires); any
  already-completed steps (the filed issue, the chat post) **stand** (saga semantics — see §5).
- **UI:** card resolves to `Rejected by <human> · <reason>`. The reason field is **required** (one quiet
  line) so "why did this stop?" is answerable later (G11). The agent **does not retry the same plan**
  (dedup key, R-04 §7.2). *(Mechanism PROVEN; required-reason is HOUSE STYLE.)*
- **HAX G8** efficient dismissal: Reject is one click + a short reason, never a multi-step ceremony.

### 3.3 Edit — the human amends the proposed effect (the load-bearing §6.3 differentiator)

This is the control that makes Myelin's HITL **more than a yes/no** — the human stays in control of the
*content* of the action, not just whether it happens (§6.3; HAX **G9** efficient correction). It is
also where most external "approval workflow" patterns stop (they offer approve/reject only — Google
Cloud's HITL pattern explicitly frames "approve, **reject, or modify**" as the full set:
[Google Cloud agentic design patterns](https://docs.cloud.google.com/architecture/choose-design-pattern-agentic-ai-system)).

**The Edit interaction (HOUSE STYLE; mechanism PROVEN):**

1. **Enter edit mode** on a single proposed effect — the effect's parameters become editable in place,
   typed against the effect's **`ToolDef` JSON Schema** (the same schema `EffectApi::apply` step 1
   validates against). The human edits *within* what the schema and the agent's `delegation ∩ tenant`
   scope allow — **the human cannot widen the agent's authority via Edit** (the edited effect re-runs
   the full `EffectApi` pipeline; capability/delegation/tenant are re-checked at apply, fail-closed).
2. **Show the diff.** The card renders the **diff between the agent's proposed effect and the
   human-amended effect** (e.g. proposed branch `fix/auto-88` → edited `fix/dev-88`; or scope narrowed
   from `repo/*` to `repo/api`). The diff is the legibility guarantee — the human sees exactly what
   they changed before applying (R-04 §7.2 "gate-edited" branch).
3. **Apply the human's version.** `EffectApi.apply(edited_effect)` runs; the result is attributed to
   **`human-edited-agent-proposal`** — a distinct attribution kind so the audit trail records that a
   human modified the agent's plan (not "the agent did this," not "the human did this from scratch").
4. **Validation failure is graceful.** If the human's edit fails schema or exceeds scope, the card
   shows the inline error and *does not apply* (the effect stays withheld) — never a partial mutation.

**Constraints that make Edit safe (PROVEN):**
- Edit re-enters the **full plan-then-apply pipeline** (schema → capability → delegation → tenant →
  budget → gate → apply). Editing does not bypass any guard (§5.2).
- Edit **cannot escalate**: the intersection `agent.policy ∩ delegation ∩ tenant.policy` still bounds
  the applied effect; a human can narrow but the *agent's* delegated authority is the ceiling for an
  agent-attributed apply. (A human wanting to do *more* than the agent may does it as themselves, a
  separate action.) *(PROVEN — EI-02 §2 intersection invariant.)*

### 3.4 Partial approval on a multi-effect card

A card may gate **several effects** ("approve these 3 merges"). The controls operate **per-effect**:
Approve effects 0 and 2, Reject 1 — three independently-idempotent signals, each → exactly one
`EffectApi::apply`; the rejected effect is withheld (`idem_key = card_id:effect_idx`, C4). The card
shows per-effect state so "a partial approval is well-defined" is *visible*, not just true in the
backend. *(PROVEN — agent-fabric C4; choreography HOUSE STYLE.)*

---

## 4. Surfaces — where the agent treatment and the card appear

Per §5.4 + §6.3 + system-overview §8.2, the HITL card is a **shared component** with three homes; the
agent treatment (§1) appears wherever an agent principal appears.

| Surface | Role | Why (PROVEN/HOUSE STYLE) |
|---|---|---|
| **Chat (primary)** | The plan card's primary home — approval happens *where the team already is*, not in a separate ops console (§6.3; system-overview §8.2; R-04 F-AGT-1 seam). | PROVEN (the approval-card surface is system-overview §8.2); HOUSE STYLE that it's *primary* |
| **Notifications inbox (second home)** | The card *also* lands in the unified inbox so a gate is **never missed** (§5.8); deduped, "why am I getting this," one-action triage. | PROVEN (§5.8 inbox is the durable second home; the durable gate is reminded via inbox, §5.3) |
| **Inline on the artifact** | The card can appear inline on the affected PR/issue/doc — e.g. an agent's proposed-fix marker on a diff line (R-04 F-ENG-1), an agent-suggested transition on an issue. | HOUSE STYLE (component is shared, §5.4); the agent-treatment on inline content is PROVEN (§6.1) |
| **CLI** | `myelin` surfaces a gate as a textual card; `run --dry-run` prints the plan; approve/edit/reject as verbs — same `ArtifactRef` scheme (§7.7). | HOUSE STYLE (CLI parity); `--dry-run` is PROVEN (contract 8.7) |

**Calm-by-default placement (P8 / §6.5; deepened by R-15):** the *card* is foregrounded (it needs the
human), but routine agent *chatter* (triage updates, review comments, status posts) is kept **out of
the main timeline** — threaded, collapsible, inbox-routed. A storm of gates **collapses** into a
grouped "7 approvals awaiting you" with per-item controls (R-04 §7.2 card-storm; R-21 owns the storm
state-craft). *(PROVEN intent §6.5; the grouping choreography HOUSE STYLE; surge bounded by the named
agent-lane shed budget C10.)*

**Sketch-funnel Axis 5 (agent presence) hook.** This surface table *is* the axis: a finalist at the
**ambient** pole keeps cards inbox-first and chatter fully threaded; a finalist at the **foregrounded**
pole shows the agent as a visible participant proposing inline in the PR/queue. Both must satisfy §1
(labelled) and §3 (gated where consequential) — Axis 5 varies *presence*, never *legibility*.

---

## 5. The agent state set (incl. partial-failure) — the durable-gate-aware states

The prompt's required set, each a **designed state** (not a 500), grounded in agent error-recovery
doctrine — *preserve context, acknowledge the limit, offer the next step, degrade gracefully*
*(PROVEN method;
[Redis HITL production oversight](https://redis.io/blog/ai-human-in-the-loop/);
[Google Cloud agentic patterns](https://docs.cloud.google.com/architecture/choose-design-pattern-agentic-ai-system)).*
The state machine matches `run.state` + `hitl_gate.state` in `agent-fabric.md` §4 — **we surface it,
we don't invent it.**

```
agent-pending ─▶ agent-working ─▶ gate-awaiting ─▶ { approved | gate-edited | gate-rejected }
                     │                  │
   (cut across, any time:)  agent-error · budget-exceeded · loop-guard-tripped · denied/cross-cell
```

| State | Backs onto (`run`/`gate` field) | Frontstage design (HOUSE STYLE) | HAX |
|---|---|---|---|
| **agent-pending** | run created, not yet stepping | Quiet agent-treatment badge: "TriageAgent will review this" — present, not noisy. The §5.10 `agent-pending` state. | G1 (what it can do), G2 (sets expectation) |
| **agent-working** | `run.state = running`; loop driving `step` | "TriageAgent is reviewing the failure…" with the agent badge; **no fake progress bar** — show the step it's on if known, else a calm indeterminate marker. | G2, G11 |
| **gate-awaiting** | `hitl_gate.state = pending` (durable wait) | The **plan card** (§2) live in chat + inbox; budget/scope shown; reminded, never lost. | G16 (consequences), G9/G8 |
| **approved** | gate signalled, effect applied | Card → `Approved by <human> · <when>`, attributed; the applied effect's chip updates live. | G11 |
| **gate-edited** | edited effect applied (§3.3) | Card → shows the proposed→amended diff; attributed `human-edited-agent-proposal`. | **G9 efficient correction** |
| **gate-rejected** | `Halted::Rejected` + reason | Card → `Rejected by <human> · <reason>`; proposed effect discarded; completed steps stand; agent won't retry same plan. | **G8 dismissal**, G11 |
| **agent-error (mid-chain)** | `agent.run.failed`; saga — completed steps not rolled back | Quiet card: "FixAgent couldn't propose a fix — the issue is filed; take it from here." **No half-open PR**; human inherits a *partial-but-coherent* state, never corrupt. `correlation_id` preserved for the audit thread. | **G7/G9/G11** (When-wrong cluster) |
| **budget-exceeded** | `agent.run.failed{reason: budget}` (platform-enforced, §5.2) | "Triage paused — budget reached. Resume · Increase budget (admin) · Take over." Work done so far stands; raising budget is a governance action (R-15). | G16, G11 |
| **loop-guard-tripped** | causal-depth ceiling / shared-root tripwire / circuit breaker (§5.5) | "Automation paused to prevent a loop." **Operator alarm, not a user-facing crash.** Per-tenant kill-switch visible in the governance console (R-15). | G11, G17 (global controls) |
| **denied / cross-cell** | `EffectApi → Denied`; missing grant or cross-cell no-grant | Card explains *which* grant is missing; **never leaks the target's content** (ADR-03). Nothing silently happens. | G10 (scope when in doubt), G11 |
| **stale approval** | durable wait re-checks on resume; base moved | "The base changed — re-propose?" rather than opening a broken PR / silent stale merge. | G11, G16 |

**The partial-failure design invariant (HOUSE STYLE, reasoned from saga semantics, PROVEN backend):**
on *any* mid-chain failure the human inherits a **coherent partial state** — completed effects stand,
the failed effect leaves no half-mutation (the routing split + plan-then-apply guarantee this), and the
`correlation_id` lets the human (and the audit log) read what happened end-to-end. *This is the
difference between "the agent broke" and "the agent did 2 of 3 things and told me clearly."*

---

## 6. Per-surface HAX-18 conformance notes (the §6.1–§6.5 critique applied)

For each agent-touching §7 surface: the §6-critique 6-check + the HAX guidelines most load-bearing for
that surface (the prompt's emphasis: **Initially G1/G2** + **When-wrong G7–G11**). **Doctrine-wins
conflicts are flagged ⚠ and resolved in §9.**

### 6.1 PR agent-reviewer / agent-author (§7.1)
- **§6 critique:** (1) labelled ✓ (§1 treatment on the reviewer row, dismiss/override available, §7.1).
  (2) proposes-before-acting ✓ (a review *comment* is advisory; a *merge* is gated, §6.3 git.merge=yes).
  (3) plan legible ✓ for any proposed effect (a suggested fix renders as a plan card). (4) gated ✓
  (merge). (5) attributed + audit ✓. (6) calm ✓ (review comments batched/threaded, §6.5).
- **HAX:** **G1/G2** — the reviewer's scope and reliability must be stated ("reviews for X; may miss Y")
  so the human knows *how far to trust the review* (links to R-15 calibration). **G11** — each comment
  links *why* (the rule/check it fired on). **G9** — dismiss/override an agent comment in one action.
- **⚠ Doctrine-wins:** an agent reviewer is **never** an *approving* reviewer that satisfies a required
  check on its own for a consequential merge — §6.3 + branch-protection keep a human gate. (HAX is
  silent on this; doctrine is strict.)

### 6.2 Issue triage inbox (§7.3)
- **§6 critique:** (1) labelled ✓ (agent-suggested labels/dedup marked). (2) proposes ✓ (triage/forecast
  are **suggest, not auto** — §6.3 Issues default = no-gate *because advisory*, the human accepts).
  (3) plan legible ✓. (4) gated where consequential ✓ (an SLA-bound `transition(→done)` with an approver
  edge is gated, ABAC caveat, §6.3). (5) attributed ✓. (6) calm ✓ (triage volume out of the main queue).
- **HAX:** **G3 time-services-by-context** ⚠ — HAX permits proactive triage; doctrine says *suggest,
  human accepts* (no auto-transition of consequential state). **G10 scope-when-in-doubt** — a low-
  confidence triage label is offered, not applied. **G15 granular feedback** — accept/reject a single
  suggested label feeds future quality (R-15).

### 6.3 CI triage view (§7.2)
- **§6 critique:** (1) labelled ✓ (the agent-surfaced triage view, §7.2). (2) proposes ✓ (a proposed
  fix is a **plan**, §6.2). (3) plan legible ✓ (failing-step → proposed effect). (4) gated ✓ (any
  `deploy`/`write_secret` = yes, §6.3). (5) attributed ✓ (correlation across CI→issue→chat). (6) calm ✓.
- **HAX:** **G1/G2** — state what the triage agent diagnoses vs. doesn't. **G16** — a proposed deploy
  shows the *consequence* (which env) before the gate. **G11** — the diagnosis links the failing
  step/line (R-04 F-ENG-1 / R-22 wedge).

### 6.4 Chat HITL card (§7.5, §5.4) — the flagship
- **§6 critique:** all six ✓ — this surface *is* the contract: labelled (§1), proposes (§2 plan card),
  plan legible (concrete effects + authority), gated (Approve/Edit/Reject on consequential effects),
  attributed + audit (§6.4 link), calm (card foregrounded, chatter threaded).
- **HAX:** **G16 convey-consequences** (cost/scope on the card), **G9 correction** (the Edit path),
  **G8 dismissal** (Reject), **G11 why** (provenance + audit). **G7 efficient invocation** — explicit
  `@agent` summons (explicit-first, CHAT-1).
- **⚠ Doctrine-wins:** **G3** (proactive timing) → **explicit-first** (a mention *notifies*, never
  auto-spawns a costed run; implicit auto-dispatch is L-3, counsel-gated). **G13 learn-from-behaviour**
  → no silent learning on tenant content (build-data-as-training **foreclosed by default**, AG-8). See §9.

### 6.5 Agent governance console (§7.6) — owned in depth by R-15, audited here
- **§6 critique:** (5) attribution/audit ✓ (which agents exist, scopes, delegation, budgets, kill
  switches — §6.4). (4) gated ✓ (autonomy policy is suggest-by-default; raising it is a governed admin
  action). (1) labelled ✓ (every agent principal listed with the treatment).
- **HAX:** **G17 global controls** — the kill-switch + per-tenant autonomy policy *is* HAX G17 (overall
  control of agent behaviour). **G18 notify-about-changes** — a policy/budget change is announced.
  **G14 update-cautiously** — autonomy is raised deliberately, never silently. **G2** — the console
  states each agent's reliability/scope so admins (P12/P15) calibrate.
- **⚠ Doctrine-wins:** suggest-by-default is **never** loosened to autonomous-by-default for a
  consequential action without a written deviation (§6.3 table rule); HAX has no such floor.

**Coverage check:** all five prompt-named agent-touching surfaces have a HAX-18 + §6-critique note;
every surface satisfies the 6-check; When-wrong (G7–G11) and Initially (G1/G2) are foregrounded per the
prompt; the Over-time cluster (G12–G18) lands mainly on the governance console (G14/G17/G18) and
feedback (G15) → R-15.

---

## 7. Actionability toward the control artifacts

| Control artifact | What this file equips | Where |
|---|---|---|
| **rubric.md D6** (agent legibility & trust, 12%) | The *checkable* definition of "every agent action legible, gated where consequential, attributable; trust calibrated not blind": §1 (always labelled, not magic) + §2 (plan-then-apply shows effects before they happen) + §3 (Approve/**Edit**/Reject) + §5 (full state set incl. partial-failure) + §6 (per-surface HAX-18). A finalist scores 4 only if it shows §1+§2+§3 on ≥1 surface with ≥1 partial-failure state. | §1–§6 |
| **sketch-funnel Axis 5** (agent presence) | §4 surface table defines the ambient↔foregrounded poles *with the legibility floor held constant* — finalists vary presence, never whether the agent is labelled/gated. | §4 |
| **rubric G1** (a11y floor) | §1 (never color-alone, WCAG 1.4.1; icon+label+attribution) + §5 (live-region-friendly state announcements without spam) make the agent treatment **AA-checkable**; the HITL card is a §4-named hard component (keyboard-operable) — R-17 audits it. | §1, §5 |
| **R-15** | Hands off: per-action provenance + "why did this happen?" + audit-trail link (named in §2/§3/§6, deepened by R-15); calm-volume patterns (§4); governance/kill-switch (§6.5); the deferred trust-calibration study. | §2, §4, §6.5 |

---

## 8. Completeness-critic (README §9) — which gloss-risks R-14 owns vs. defers

R-14 owns the **agent-legibility** gloss-risks:
- **Agent-pending state (§9)** — **OWNED & covered** (§5; ties §5.10).
- **Partial-failure agent branches (§9: gate-rejected, agent-error-mid-chain, budget-exceeded,
  loop-guard-tripped)** — **OWNED & covered as designed states** (§5; the *flow* branches are R-04
  §7.2, the *visual state spec* is here per R-04's named handoff).
- **Cross-cell / no-access effect → never leak (§9, ADR-03)** — **covered** (§5 denied/cross-cell row;
  §2 live-permission-aware target chips).
- **Screen-reader announcement of agent-proposal arrival without spamming (§9 a11y)** — **covered as a
  requirement** (§1 attribution string + §5 calm state announcements); the *audit method* is R-17.
- **No-color-alone / focus-token≠identity-token (§9 a11y)** — **covered** (§1.1 four-channel signature,
  WCAG 1.4.1); measured-contrast + focus-token QA is R-17.
- **Storm / 30×-agent-surge card grouping (§9)** — **touched** (§4 calm placement; R-04 §7.2 card-storm);
  the *state-craft catalogue* is **deferred to R-21** (named), the surge *budget* is C10 (named, not owned).
- **Calm agent volume / agent-out-of-main-timeline (§6.5)** — **patterns named** here (§4); **deepened
  by R-15** (named).

---

## 9. Doctrine-beats-HAX conflicts (resolved in doctrine's favour, per the prompt)

| HAX guideline | What HAX permits/encourages | §6 doctrine (stricter) — **wins** | Resolution |
|---|---|---|---|
| **G3 Time services based on context** | Proactive action/interruption when contextually useful | §6.5 + P8 + CHAT-1: **calm, explicit-first** — a mention *notifies*, never auto-spawns a costed run; no proactive interruption by default | **Explicit-first.** Implicit auto-dispatch is **L-3, counsel-gated** (GDPR Art. 22 / AI-Act human-oversight); not built (agent-fabric §3.4/§12). |
| **G13 Learn from user behavior** | The system improves by observing user actions | AG-8: **build-data-as-LLM-training foreclosed by default**; no tenant content feeds training without separately-ratified opt-in | **No silent learning.** Granular feedback (G15) is captured as *signals*, not training; runtime-agnostic (mock today). |
| **G14 Update and adapt cautiously / G17 Global controls** | Adapt over time; give users overall control | §6.3 suggest-by-default: autonomy is **never** loosened to autonomous-by-default for a consequential action without a written deviation; kill-switch + gates are mandatory | **Autonomy is opt-in per-action/per-scope by policy owners** (P12/P15); the floor is stricter than "cautious." |
| **(implicit) approve/reject sufficiency** | Many agentic patterns offer approve/reject only | §6.3: **Approve / Edit / Reject** — the human controls the *content*, not just yes/no | **Edit is mandatory** (§3.3), exceeding the common floor. |
| **(implicit) AI iconography** | "Magic"/sparkle AI affordances are common industry practice | §8b.3: **no sparkle/shimmer/magic-wand; no emoji-as-UI**; agents look like labelled principals | **Anti-magic by rule** (§1.1). |

**The meta-resolution:** HAX-18 is treated as a **floor/checklist** (a conformance audit per surface,
§6); the §6 contract is the **binding spec** where the two diverge. Every divergence above makes Myelin
*more* conservative/trustworthy, never less — consistent with P7/P12/P13 (the gatekeepers' deepest
fear is *ungoverned* automation).

---

## 10. `[DEFERRED-UNTIL-USERS]` — what these patterns assume that only users can confirm

These are **expert-authored patterns + a HAX-18 heuristic audit (the no-user substitute), NOT
validated UX.** The decisive validation — **PAIR-style trust calibration** — is **R-15's** deferred
study; recorded here as the assumptions R-14 makes:

- **What to test (R-15 owns the protocol):** do users (a) correctly read the agent treatment as
  "an agent, not a human/magic"; (b) understand from the plan card *what will change, on what, under
  whose authority* before approving; (c) use Edit correctly (amend rather than reject-and-redo); (d)
  recover from each partial-failure state knowing what state they're in; (e) **calibrate** trust —
  neither rubber-stamp (over-trust) nor reject-everything (under-trust).
- **With whom:** mixed engineers/PMs on the chat HITL card + the agent-reviewed PR; **P12 security +
  P15 admin** on the governance console; the regulated-buyer lens (P13 DPO) on attribution/audit
  legibility (jointly with R-19).
- **What would falsify the pattern hypotheses:** (1) users approve without reading the effects/scope
  (the plan card failed to convey consequences → HAX G16 unmet); (2) the agent treatment is mistaken
  for a status color or for a human (§1 failed); (3) a partial-failure state leaves users unsure what
  happened (§5 saga-legibility failed); (4) Edit is unused because it's not discoverable or feels
  unsafe (§3.3 failed); (5) the agent is *over*-trusted on a low-reliability task because G2 wasn't
  conveyed.
- **The runtime caveat (carried to R-15, PROVEN-by-architecture):** all of this is drawn against the
  **mock** runtime (`--use-mock`); **mock-agent trust may not predict real-LLM trust.** The mitigation
  is structural: the **contract** (plan-then-apply, the durable gate, intersection scope, budgets, one
  `correlation_id`, attribution) is designed to be trustworthy **regardless of runtime** — the
  strategy-pattern payoff means the *exact same UI* works for mock and real (§6 closing note). The
  thing to validate is the contract's legibility, not the mock's specific outputs.

---

## 11. Self-check against R-14 acceptance criteria

| Criterion (prompt R-14 / ws-f) | Status | Evidence |
|---|---|---|
| **Agent treatment unmistakable + color-blind-safe (never color-alone, never sparkle/emoji)** | ✅ Met | §1.1 four-channel signature (label+icon+color+attribution); WCAG 1.4.1 cited; §8b.3 prohibitions carried verbatim; §1.2 why-not-a-status-color |
| **Plan-then-apply shows concrete proposed effects per artifact + delegated authority before they happen** | ✅ Met | §2 card anatomy — effect list, per-effect target chip, per-effect authority+gate, scope (∩ intersection) line, budget; each mapped to a real `proposed_effect`/`hitl_gate` field; concrete-not-vague rule |
| **Approve / Edit / Reject behaviour incl. the Edit path (human amends the proposed effect)** | ✅ Met | §3 (durable gate); §3.3 Edit = edit-in-schema → proposed→amended diff → apply attributed `human-edited-agent-proposal`, re-runs full pipeline, cannot escalate; §3.4 partial approval |
| **Surfaces: chat primary, inbox, inline (+ CLI)** | ✅ Met | §4 surface table (chat primary / inbox second-home-never-missed / inline / CLI `--dry-run`) + Axis-5 hook |
| **Per-surface HAX-18 conformance note for each agent-touching §7 surface** | ✅ Met | §6.1 PR reviewer · §6.2 issue triage · §6.3 CI triage · §6.4 chat HITL · §6.5 governance console — each with §6-critique 6-check + load-bearing HAX (G1/G2 + G7–G11 foregrounded) |
| **Full agent state set incl. partial-failure (rejected/error/budget + loop-guard, denied/cross-cell, stale)** | ✅ Met | §5 state machine + table; each backs onto a real `run`/`gate` field; saga partial-coherence invariant |
| **Doctrine-beats-HAX conflicts resolved in doctrine's favour and noted** | ✅ Met | §9 table (G3→explicit-first; G13→no-silent-learning; G14/G17→opt-in autonomy; approve/reject→Edit-mandatory; AI-iconography→anti-magic) + flagged ⚠ inline in §6 |
| **Surfaces existing agent-fabric mechanics, not new ones** | ✅ Met | Every card field/state maps to `agent-fabric.md` (`EffectApi` §5.2, `hitl_gate`/per-effect idem C4, budgets §5.4, loop guards §5.5, frozen §6.3 defaults, `humanise` C9); "invents no new mechanism" stated in §0 |
| **Date; PROVEN/HOUSE-STYLE tags; grounded cited web research** | ✅ Met | Dated 2026-06-20; tags throughout; HAX CHI 2019 (3 URLs), WCAG 1.4.1 (W3C), Google Cloud + Redis HITL patterns, NN/g cited |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §8 (owns agent-pending, partial-failure visual states, cross-cell-no-leak, no-color-alone, SR-announcement requirement; defers storm-craft→R-21, calm-volume-depth→R-15) |
| **Actionable toward rubric D6 + Axis 5; feeds R-15 + Phase 6** | ✅ Met | §7 mapping (D6 scoreable, Axis-5 poles, G1 a11y, R-15 handoff) |
| **Trust-calibration recorded as a plan, not faked** | ✅ Met | §10 `[DEFERRED-UNTIL-USERS]` (R-15-owned protocol; falsification criteria; mock-vs-real runtime caveat) |

**Honest partials / top uncertainties.**
1. **All patterns are expert-authored, unvalidated** (§10) — the central risk: a "legible" plan card may
   still be rubber-stamped (over-trust) in practice; only R-15's calibration study resolves it.
2. **The Edit-path UX is the least-evidenced** (§3.3) — editing a typed effect within schema+scope is
   our design (HOUSE STYLE); whether humans reach for Edit vs. reject-and-redo is a HYPOTHESIS to test.
3. **HAX is a 2019 floor; agentic-specific guidance is still maturing** (NN/g's own research agenda flags
   agent UX patterns as open) — the per-surface HAX notes are `[VERIFY]`-worthy as the field's
   conventions settle; the §6 contract is the stable spine regardless.
4. **Mock-vs-real runtime** (§10 caveat) — real-LLM error rate/plan quality may shift which partial-
   failure states dominate; the contract holds, the *frequency* of each state is a HYPOTHESIS.

---

*End of R-14 deliverable. Date: 2026-06-20. HAX-18 method PROVEN (Amershi et al., CHI 2019; cited);
WCAG 1.4.1 / agentic-HITL patterns cited; all specific choreography HOUSE STYLE; surfaces existing
agent-fabric mechanics, invents none; nothing user-validated (R-15 owns trust calibration). Feeds
rubric D6, sketch-funnel Axis 5, R-15, Phase 6.*
