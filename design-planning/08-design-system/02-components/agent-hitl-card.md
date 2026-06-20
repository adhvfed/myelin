# Component spec — Agent / HITL approval card (plan-then-apply)

> **Phase 8b · `02-components/` · Tier-2 shared component.** Direction = finalist **A "Instrument"**
> (consumes [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**. **Not committed.**
>
> **Implements:** design-language **§5.4** (the agent/HITL approval card) + **§6** (the agent-native UX
> contract — §6.1 labelled, §6.2 plan-then-apply, §6.3 Approve/Edit/Reject + durable gate, §6.4
> attribution/audit, §6.5 calm volume). Research it renders:
> [`legibility-and-hitl.md`](../../04-research/agent-ux/legibility-and-hitl.md) (R-14, the card + state set +
> HAX-18 audit) · [`attribution-and-calm.md`](../../04-research/agent-ux/attribution-and-calm.md) (R-15,
> provenance / audit walk / scope-budget inspector / governance) · [`state-craft.md`](../../04-research/craft/state-craft.md)
> (R-21 §1.6 — agent-pending + partial-failure placement).
>
> **Tagging:** **PROVEN** = a cited standard / legal duty (WCAG 1.4.1, AI-Act, GDPR Art. 22) or an existing
> mechanism this spec *surfaces* (`agent-fabric.md`: `EffectApi`, `hitl_gate`, per-effect idempotency C4,
> budgets, loop guards; the tamper-evident audit log). **HOUSE STYLE** = the visual choreography.
> `[DEFERRED-UNTIL-USERS]` = trust-calibration hypotheses (R-15 owns the study).
>
> **Reuse:** every per-effect target is a **[`<ReferenceChip>`](./reference-chip-and-unfurl.md)** (live,
> permission-aware, never-leaking). The card has three homes: chat (primary), the
> [notifications inbox](./notifications-inbox.md) (second home, never missed), and inline on the artifact.
> **It invents no new backend mechanism** — every field maps to a real `proposed_effect` / `hitl_gate` field.

---

## 1. Name + purpose

**`<AgentHitlCard>`** — the surface that turns the platform-law *"agents emit proposed effects; they never
perform side effects directly"* (`agent-fabric.md` §5.2, `EffectApi`; ADR-08.3) into **felt trust**. It
renders the **plan** — concrete, reviewable, attributed — *before* `EffectApi::apply` runs, and resolves a
**durable HITL gate** that can wait minutes or days. This is the trust-bearing surface of agent-native (P7);
the visible half of the §6 contract. *(Mechanisms PROVEN; choreography HOUSE STYLE.)*

---

## 2. Anatomy (every field maps to a real `proposed_effect` / `hitl_gate` field)

```
┌──────────────────────────────────────────────────────────────────────┐
│ ⬠ FixAgent · AGENT        on behalf of @dev · triggered by ci.failed  │  ← §3 agent treatment + attribution (humanised, C9)
│                                              correlation: incident-#9 │  ← the correlation_id thread (clickable)
├──────────────────────────────────────────────────────────────────────┤
│ Proposes 2 effects:                                                   │  ← PLAN = Vec<ProposedEffect>
│  1. Open PR  → [repo/api]  (fix #88)        authority: open_pr        │  ← effect · target ReferenceChip · authority (no gate)
│  2. Merge PR → [repo/api]  (after checks)   authority: git.merge ⚠GATE│  ← consequential → frozen §6.3 default = gated
│                                                                       │
│ Scope:  may act on  [repo/api], [issue ENG-412]  (delegation ∩ tenant)│  ← intersection — NOT the human's full rights
│ Budget: 3 / 12 effects · est. cost shown        (live)               │  ← run.budget + hitl_gate.cost_estimate, live
├──────────────────────────────────────────────────────────────────────┤
│   [ Approve ]   [ Edit… ]   [ Reject ]            Why? · Audit trail  │  ← §4 controls + §6.4 provenance/audit
└──────────────────────────────────────────────────────────────────────┘
```

| Element | Backs onto (PROVEN) | Conveys |
|---|---|---|
| **Effect list** | `run.proposed_effect` rows | *what will change* — concrete ("Open PR #88"), never "FixAgent is working" |
| **Per-effect target** | the effect's `object` `ArtifactRef` → a **`<ReferenceChip>`** (live, permission-aware) | *on which artifacts* |
| **Per-effect authority + GATE marker** | `tool_def.requires_approval` from the frozen §6.3 defaults (merge=yes, open_pr=no) | *which effects are consequential* — the gate is on the **effect**, not the whole card |
| **Scope line** | `agent.policy ∩ delegation ∩ tenant.policy` (intersection, never up) | *under whose delegated authority* — the agent can do **nothing no human role can** |
| **Budget line** | `run.budget` (integer minor-units) + `hitl_gate.cost_estimate`, **live, always-on** | *cost/consequences* (HAX G16); pre-empts budget surprise |
| **Why? / Audit** | per-action provenance + tamper-evident audit link (R-15 §1.2/§2) | *make clear why* (HAX G11) |

**Design rules (HOUSE STYLE unless noted):** concrete effects never a vague "Apply changes"; the **gate
marker sits on the effect, not the card** (so partial approval is legible); targets are **live reference
chips** (an approver who can't see `ENG-412` gets the graceful no-access chip — never a leaked title, ADR-03);
**cost/scope are always shown, never behind a "details" toggle** (consequences-before-action is the whole
point); **`--dry-run` parity** — the same card renders from `run --dry-run` (contract 8.7), inspectable with
zero apply.

---

## 3. The agent treatment — four-channel, never colour-alone (§1; §8b.3)

The single recognisable signature for any `Principal kind=agent`, **built once, inherited everywhere** (P1):

| Channel | Spec | Tag |
|---|---|---|
| **Label (text, always)** | the literal `AGENT` badge adjacent to the name (`FixAgent · AGENT`); text is the **primary** carrier (survives colour-blind/grayscale/SR/high-contrast) | PROVEN (WCAG 1.4.1; AI-Act disclosure) |
| **Icon (shape)** | one stable **plain geometric mark** from the icon set — **NOT a sparkle / shimmer / magic-wand / star** | HOUSE STYLE rule; the legibility duty is PROVEN |
| **Colour** | the reserved **`--agent`** token (violet, a *fourth neutral axis* — never a status colour, never reads as success/warning/danger); the **redundant** channel | PROVEN (colour supplementary; contrast measured AA) |
| **Attribution string** | `on behalf of @<human> · triggered by <event>`, resolved via the one humanisation surface — never a raw id | PROVEN (mechanism + AI-Act) |

**Hard prohibitions (verbatim from §8b.3):** no sparkle/shimmer/magic-wand/star iconography; **no emoji as the
agent marker** (an emoji can't inherit `currentColor` or re-theme); agents are **never disguised as humans**
(same identity-badge component, but the agent channels are always present). Agents look like *labelled
principals*, not magic. *(PROVEN — AI-Act, ADR-08.)*

---

## 4. Approve / Edit / Reject — backed by a durable gate (§6.3)

The controls resolve a **durable-workflow HITL gate** (`hitl_gate`, contract 9.4) that holds no runtime while
it waits and lives in chat *and* the inbox, reminded until resolved — **never silently lost**.

- **Approve** — the workflow signal re-runs the withheld step; `EffectApi::apply` now passes; the effect
  applies via the subsystem's public endpoint **as the agent principal** (no carve-out). Card → `Approved by
  <human> · <when>`, attributed; the applied effect's chip updates live. **Idempotent by construction (C4):**
  `idem_key = card_id:effect_idx`; double-click = one apply. UI may optimistically flip to "Approving…" then
  settle.
- **Reject** — the gate settles `Halted::Rejected` with the **required reason** (one quiet line, so "why did
  this stop?" is answerable later — HAX G11); the proposed effect is **discarded, not applied**; any
  already-completed steps **stand** (saga semantics); the agent **does not retry the same plan** (dedup key).
  Card → `Rejected by <human> · <reason>`. One click + a short reason, never a ceremony (HAX G8).
- **Edit — the load-bearing differentiator (§6.3; HAX G9).** The human amends the *content* of the action, not
  just yes/no — where most external patterns stop at approve/reject:
  1. **Edit-in-schema** — the effect's parameters become editable in place, typed against the effect's
     **`ToolDef` JSON Schema** (the same schema `EffectApi::apply` validates against). The human edits *within*
     what the schema and `delegation ∩ tenant` scope allow.
  2. **Show the proposed→amended diff** — the card renders the diff between the agent's proposed effect and the
     human-amended one (e.g. branch `fix/auto-88` → `fix/dev-88`; scope narrowed `repo/*` → `repo/api`). The
     diff is the legibility guarantee. *(The diff render is the shared WASM-Rust diff path — 00-plan §1.6.)*
  3. **Apply the human's version** — `EffectApi.apply(edited_effect)` runs the **full** pipeline (schema →
     capability → delegation → tenant → budget → gate → apply); attributed to a distinct
     **`human-edited-agent-proposal`** kind.
  4. **Validation failure is graceful** — inline error, **does not apply** (the effect stays withheld), never a
     partial mutation.
  - **Edit cannot escalate (PROVEN — EI-02 §2):** the intersection still bounds the applied effect; a human can
    *narrow* but the agent's delegated authority is the ceiling for an agent-attributed apply. (A human wanting
    *more* does it as themselves, a separate action.)
- **Partial approval (multi-effect card)** — controls operate **per-effect**: Approve effects 0 and 2, Reject
  1 — three independently-idempotent signals (`idem_key = card_id:effect_idx`), each → exactly one apply; the
  rejected effect is withheld. The card shows **per-effect state** so partial approval is *visible*.

---

## 5. ALL states (durable-gate-aware, incl. partial-failure)

```
agent-pending ─▶ agent-working ─▶ gate-awaiting ─▶ { approved | gate-edited | gate-rejected }
                     │                  │
   (cut across, any time:)  agent-error · budget-exceeded · loop-guard-tripped · denied/cross-cell · stale
```

| State | Backs onto | Render (HOUSE STYLE) |
|---|---|---|
| **agent-pending** | run created, not stepping | quiet agent badge: "TriageAgent will review this" — present, not noisy |
| **agent-working** | `run.state=running` | "TriageAgent is reviewing the failure…" + badge; **no fake progress bar** — show the step if known, else a calm indeterminate marker |
| **gate-awaiting** | `hitl_gate.state=pending` (durable) | the **plan card** (§2) live in chat + inbox; budget/scope shown; reminded, never lost |
| **approved** | gate signalled, applied | `Approved by <human> · <when>`; applied chip updates live |
| **gate-edited** | edited effect applied | shows the proposed→amended diff; attributed `human-edited-agent-proposal` |
| **gate-rejected** | `Halted::Rejected` + reason | `Rejected by <human> · <reason>`; effect discarded; completed steps stand; no retry |
| **agent-error (mid-chain)** | `agent.run.failed`; saga — completed steps not rolled back | "FixAgent couldn't propose a fix — the issue is filed; take it from here." **No half-open PR**; `correlation_id` preserved |
| **budget-exceeded** | `agent.run.failed{reason:budget}` | "Triage paused — budget reached. Resume · Increase budget (admin) · Take over." Work done stands |
| **loop-guard-tripped** | causal-depth ceiling / circuit breaker | "Automation paused to prevent a loop." Operator alarm, not a crash; kill-switch in the governance console |
| **denied / cross-cell** | `EffectApi → Denied` | explains *which* grant is missing; **never leaks the target's content** (ADR-03). Nothing silently happens |
| **stale approval** | durable wait re-checks on resume; base moved | "The base changed — re-propose?" rather than a broken PR / silent stale merge |

**Cross-cutting matrix states.** **Loading** — card chrome skeleton (header + effect-row placeholders),
`aria-busy`. **Error** — an *infra* failure to load the card is a system-blamed line + retry; an
agent/effect failure is the *designed* `agent-error` row above. **Permission-denied** — the `denied` row
(never a leaked target); an approver who can't see a target gets the no-access chip in the effect row.
**Erased** — a target erased between propose and view renders the tombstone chip in its effect row.
**Empty** — N/A (a card always represents a proposed run).

**The partial-failure invariant (HOUSE STYLE, reasoned from saga semantics; backend PROVEN):** on *any*
mid-chain failure the human inherits a **coherent partial state** — completed effects stand, the failed effect
leaves no half-mutation, and the `correlation_id` lets the human + the audit log read what happened
end-to-end. *This is the difference between "the agent broke" and "the agent did 2 of 3 things and told me."*

---

## 6. Provenance, audit & scope (what survives the action — R-15)

- **Why?** — a one-click, persistent affordance that expands a *partial* explanation (not a model dump):
  **what** acted + did → **trigger** → **authority** (`on behalf of @dev`, scope `repo/api`) → **chain**
  (`correlation: incident-#9`, a clickable thread) → **audit** (deep link). Detail scales with stakes (PAIR
  partial-explanation; HAX G11).
- **Audit trail** — one click from the act opens the tamper-evident audit-log explorer **filtered to that
  entry**; the `correlation_id` chain view walks the whole agent flow (CI fail → triage → issue → chat →
  proposed PR → approval), each hop attributed, in causal order; a quiet **"verified ✓"** the DPO can expand
  to the CT inclusion/consistency proof. **Minimised actors** (pseudonyms + `ArtifactRef`s, never payloads);
  erased entries degrade to "[erased subject] did X" (never rewritten — chain integrity). *(All PROVEN —
  gdpr-and-audit §6.)*
- **Scope / budget / delegation inspector** — reachable from the card (this run), the agent's identity chip
  (standing authority), and the governance console (all agents). Reads as "may act on [chip], [chip]", never
  policy syntax; **budget always-on on the card, expandable here**.
- **Confidence (PROVEN-by-architecture caveat):** show confidence only as a categorical/N-best *suggestion
  strength* where the runtime supplies it — **never a fabricated number**; the mock supplies none, so v1
  surfaces **capability/scope statements** (HAX G1/G2), not a score (PAIR warns a number invites blind
  acceptance).

---

## 7. Variants + parameterization variant flags

- **`agentPresence` flag (`ambient`↔`foregrounded`)** — the **primary** flag for this component. `ambient`
  (A's default): the card lands as **one collapsed inbox row**, expanded in place on demand; routine agent
  chatter threaded/out-of-timeline. `foregrounded`: the card is a visible inline participant on the affected
  PR/issue. **The card component is identical** at both poles — the flag sets default surfacing, never
  legibility/gating/attribution (those are floors held constant — Axis 5 invariant).
- **`density` flag** — effect-row height / padding via `--space-*`.
- **`sovereigntyVisibility` flag** — `always-on` keeps the residency tag visible on cross-cell effect targets.
- **NOT affected:** `nav`, `surfaceUnification`, `tone`. **No `switch(direction)`.**

---

## 8. Keyboard + ARIA model

- **The card is a named hard component** (G1). In its **chat-primary / inbox** home it is an in-flow region
  (not a modal); the **Confirm** on a consequential approve uses the Tier-1 **Confirm** (`alertdialog`,
  React Aria **`AlertDialog`**, default-focus the *safe* action). Edit-in-place uses inline form fields
  (React Aria **`TextField`** etc., typed against the `ToolDef` schema).
- **Buttons** = React Aria **`Button`**; Approve / Edit / Reject in a logical tab order; per-effect controls
  are individually reachable. Visible focus via `--focus-ring` every theme.
- **Live-region announcement** of card *arrival* (`reason=approval_requested`, critical) and of state
  transitions (Approved/Rejected/error) via a **polite** region — **without spamming** (announce the gate and
  resolutions, not background working ticks). Reject's required-reason field is labelled + error-associated.
- **The agent treatment is AA-checkable** — label + icon + attribution carry meaning with colour stripped
  (1.4.1); the agent token is measured-contrast-validated like every semantic pair.
- **Reflow / RTL** — the effect rows + scope/budget lines reflow at 200%/320px; logical properties mirror RTL.

---

## 9. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| **Agent treatment** (label/mark/tint) | **`--agent`**, `--on-agent`, `--agent-subtle`, `--c-agent-mark` — the reserved fourth axis |
| Card surface / border | `--surface-raised` / `--surface-overlay`, `--border`, `--border-strong` |
| Effect targets | the `<ReferenceChip>` tokens (`--c-chip-*`) |
| Gate marker / consequential | `--warning` (+ glyph + the word "GATE"/⚠ — never colour-alone) |
| Approve (primary action) | `--c-btn-primary-bg` (→ `--focus-ring`), `--c-btn-primary-text` |
| Reject (consequential) | neutral by default; the *Confirm* on irreversible uses `--c-btn-danger-bg` (→ `--danger`) |
| Edit diff | `--diff-add-bg`/`--diff-del-bg`/`--diff-add-txt`/`--diff-del-txt` |
| Budget meter | `--text-muted` track, `--accent` fill (identity, not status) |
| Focus | `--focus-ring` |
| Confirm overlay | `--shadow-overlay`, `--overlay-scrim`, `--z-modal` |

The agent token is the load-bearing reserved token (00-plan §2.1 names it explicitly). Binds only to semantics.

---

## 10. Motion (token-based, reduced-motion first-class)

- **Card / inbox-row expand** — `--dur-base` (180ms) `--ease-standard`; expands in place, no jump.
- **Approve settle** — optimistic flip to "Approving…" `--dur-micro`, settle on ack; the applied effect chip's
  live flip uses `--dur-deliberate` (240, the reserved agent/notice band).
- **Edit diff reveal** — `--dur-fast` cross-fade between proposed and amended.
- **Agent-enter** — the *only* place `--ease-emphasized` (the reserved agent signature easing) may be used,
  at `--dur-deliberate`; still calm, **never sparkle/bounce** (§8b.3 / R-12).
- **`prefers-reduced-motion`** → durations 0; state flips + announces. Information never lives in the animation.

---

## 11. Usage do / don't

**Do**
- Show **concrete effects** with live target chips and the **gate on the effect**, not the card.
- Keep **cost + scope always visible**; budget live so budget-exceeded is never a surprise.
- Offer **Edit** prominently — it is the differentiator; make the proposed→amended diff the legibility moment.
- Render every partial-failure as a *designed, attributed, audit-linked* state leaving a coherent partial state.
- Keep the kill-switch / governance one click from any agent chip (R-15 §4).

**Don't**
- Don't use sparkle/shimmer/magic-wand/star/emoji for the agent — plain mark + the word `AGENT` (§8b.3).
- Don't reuse a status colour for the agent (it turns the screen into a traffic light + conflates "an agent
  touched this" with "good/bad").
- Don't let Edit *widen* the agent's authority — the intersection is the ceiling (re-checked at apply).
- Don't apply any consequential effect without the gate; don't fire a GDPR-erase/agent-merge optimistically
  (these Confirm, never optimistic — the OPT-2 carve-out).
- Don't dump agent chatter into the main timeline; thread/collapse/inbox-route it (P8 / §6.5).
- Don't fabricate a confidence number the runtime can't produce.

---

## 12. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** plan-then-apply, the durable gate, per-effect idempotency (C4), the intersection scope ceiling,
  budgets, loop guards, attribution, the tamper-evident audit log, `--dry-run` parity, the four-channel
  legibility duty (WCAG 1.4.1 + AI-Act). Every card field maps to a real backend field; **invents nothing**.
- **HOUSE STYLE:** all visual choreography; the Edit-in-place interaction + proposed→amended diff render; the
  required-reason on Reject; the per-effect gate placement; the calm/ambient default surfacing.
- **`[DEFERRED-UNTIL-USERS]`** (R-15 owns the PAIR-style study): does the plan card get *read* or
  *rubber-stamped* (over-trust — the headline failure)? is **Edit** reached for, or do humans reject-and-redo
  (least-evidenced path)? do users recover from each partial-failure state knowing their state? **The
  mock-vs-real caveat:** all of this is drawn against the mock runtime; the *contract* is designed trustworthy
  regardless of runtime (the strategy-pattern payoff — the exact same card renders for mock and real), so what
  validates now is the *contract's legibility*; **B2 trust-calibration must be re-run against the real LLM
  runtime** (the over-trust-from-fluency effect is a property of the LLM's output, not the card).

*End. Component spec HOUSE STYLE over the PROVEN `agent-fabric.md` + `gdpr-and-audit.md` mechanics +
design-language §5.4/§6; renders R-14/R-15; targets are `<ReferenceChip>`s; second home is the inbox. Consumes
the finalist-A token set incl. the reserved `--agent` token. Not committed.*
