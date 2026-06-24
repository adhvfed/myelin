# Issue Tracker — Governance admin views design pass (S13–S18; the pre-frontend sketch)

> The **ISS-P29 / P-396** design pass: a visual/token-level sketch over the **governance admin views**
> (S13 workflow/scheme editor · S14 SLA policy editor · S15 team/project settings + the permission
> inspector · S16 automation/trigger builder · S18 audit/change-history), **including the
> empty / loading / error / permission-denied states** for each. Per VISION §3 ("no frontend code
> without a reviewed design sketch behind it") this is a **design sketch, not frontend code** — no Rust
> UI ships under this prompt. The governance frontend lands with the M6 surface prompts (ISS-P33+).
>
> **Date: 2026-06-24.** Conforms to the frozen design system (`design-planning/08-design-system/`): the
> **Forms & Controls** spec
> ([`02-components/forms-and-controls.md`](../../../../design-planning/08-design-system/02-components/forms-and-controls.md))
> is the ONE control set these editors compose from (Button / Input / Select-Combobox / Checkbox-Radio-
> Switch / Field+Validation); the **Overlays** spec
> ([`02-components/overlays.md`](../../../../design-planning/08-design-system/02-components/overlays.md))
> governs the guard popover, the assign/scheme dropdowns, and the breach-simulation preview (portal-to-
> root, focus-trap, flip-when-off-screen); the semantic tokens are the finalist-A set; the glyphs are the
> 42-icon library.
>
> **Keyed to the live backend:** each governance screen's view-model is already declared in
> `myelin_issues::governance` (this prompt) — `GovernanceView::view_model()` names the **REAL engine** the
> editor writes through. The eventual frontend writes through the **same engine** (the same public API the
> UI / CLI / agents hit — ADR-08, no privileged back-channel), never a parallel calc.

---

## 0. The structural bet this pass makes visible (the falsifiable rule)

The governance bet (arch §1.1): **the schemes are the product surface, made editable through the SAME
engines the runtime enforces.** An admin does not configure a *shadow* of the workflow — the S13 editor
edits the very `Workflow` FSM the ISS-P12 interpreter runs; the S14 breach-simulation previews the very
`business_fire_at` the ISS-P26 SLA engine arms; the S15 inspector's "who can / why" IS Identity's
`list_subjects` / `explain` (contract 4.4), not an Issues recompute.

**The visual pass MUST NOT betray this structurally:** every editor's save writes through the named engine
(`GovernanceViewModel::backing_engine`); there is **no second workflow model, no parallel breach calc, no
private ReBAC evaluator** (EI-01 §7). A reviewer can falsify it: if the S15 inspector ever shows a member
Identity's `explain` does not, it forked. The CDC pair (`cdc_4_4_issues_inspector.rs`) pins
inspector-equals-explain in CI.

---

## 1. S13 — Workflow / scheme editor (the FSM graph + the frozen QueryAst guard builder)

```
┌ Workflow scheme: Support ─────────────────────────────────────────────────────────────────┐
│  ┌Todo┐ →[guard]→ ┌In Progress┐ →[guard: CI green]→ ┌In Review┐ → ┌Done┐   ┌Cancelled┐     │
│   unstarted        started                            started      completed  cancelled    │
│  (every state maps to a fixed CATEGORY — the mandatory invariant, sketch 02)               │
│  Selected transition → [ guard builder: query-AST predicate ]  [ post-action ]             │
│  ⚠ Validation: state "Blocked" is unreachable                                              │
│  Assign scheme to:  Type ▾  ×  Team/Project ▾                                               │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Backing engine:** `crate::workflow::Workflow` (the ISS-P12 FSM) + `crate::schemes` (ISS-P11). The graph
  IS the live `Workflow.states` / `Workflow.transitions`.
- **The guard builder is the frozen `QueryAst`** (`GuardLanguage::FrozenQueryAst`) — no free-form scripting
  (arch §1.1). The guard popover **portals to root + flips off-screen** (Overlays §8b.1/§8b.4).
- **Inline validation (glyph + label, before save):** `workflow_unreachable_states()` flags an orphaned
  state (a `Blocked` with no inbound path from the initial state) and a missing-category mapping — the `⚠`
  line above. Computed over the REAL FSM (no second model).
- **States:** **Empty** (new scheme) → the Linear-simple default (Todo→In Progress→Done + Cancelled), the
  no-config baseline, editable. **Loading** → a skeleton matching the graph layout, no blank flash. **Error**
  (save failure) → one quiet line, the editor holds local state (no lost edits). **Permission-denied** → the
  editor is read-only with a "you may view but not edit this scheme" banner (never a silent disabled save).

---

## 2. S14 — SLA policy editor + calendar editor + breach-simulation preview

```
┌ SLA policy: First-response (Support) ─────────────────────────────────────────────────────┐
│  Metric: time-to-first-response   Target: [ 4h ]   Calendar: [ Biz-hours (UTC) ▾ ]         │
│  Pause when: [ state:waiting-on-customer ]   Escalation: [ on-call → lead after 30m ]      │
│  ── Breach simulation preview ──                                                            │
│   Start: Thu 08:00  →  Breach fires:  Thu 12:00   (4h business time; weekend skipped)      │
│   ⚠ A 40h budget would exceed this calendar's reachable working windows                    │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Backing engine:** `crate::sla_calendar::{SlaEngine, business_fire_at, Calendar}` (ISS-P26). The
  breach-simulation preview's `fire_at` IS `simulate_breach()` → `business_fire_at()` — the **REAL**
  business-calendar arithmetic, **not a `start + budget` wall-clock shortcut** (which would silently
  disagree over weekends/holidays). `breach_simulation_uses_real_sla_engine` pins this.
- **Inline validation:** a budget exceeding the calendar's reachable working windows surfaces as a
  `CalendarError` (the `⚠` line), never a hang.
- **States:** **Empty** → a sensible default policy (4h first-response, business-hours calendar). **Loading**
  → the form skeleton. **Error** → the simulation preview shows the engine's error inline ("budget
  misconfigured"), the rest of the form stays editable. **Permission-denied** → read-only policy view.

---

## 3. S15 — Team / Project settings + the permission inspector (contract 4.4)

```
┌ Project settings: ENG ────────────────────────────────────────────────────────────────────┐
│  Members ▾    Prefix: [ ENG ]    Scheme assignments: Workflow ▾  SLA ▾                      │
│  ── Permission inspector ──  "Who can  [ approve ▾ ]  on  [ issue:ENG-1421 ] ?"             │
│   ✓ alice (approver)   ✓ bob (approver)   ✓ carol (lead → ∪ lead arm)                       │
│   Why?  → expand issue:ENG-1421#approve = approver ∪ lead → ALLOW (alice is an approver)    │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Backing engine:** `myelin_identity::IdentityService::{list_subjects, explain}` (contract 4.4),
  consumed through `crate::governance::PermissionResolver`. **The inspector reads `list_subjects` / `explain`
  — NEVER a private recompute.** `PermissionInspector::who_can` renders EXACTLY the `SubjectTree.members`;
  `PermissionInspector::why` renders EXACTLY the `RewriteTrace`. **0 private recompute** (the
  inspector-equals-explain gate, `cdc_4_4_issues_inspector.rs`).
- **Leak-free is visible by absence:** a subject Identity's `explain` excludes is **absent** from the
  inspector — never greyed, never counted (the same leak-free discipline the views hold). The membership
  list shows only the resolver's members; there is no "N hidden" leak.
- **States:** **Empty** ("no one can approve this yet — assign an approver or lead"). **Loading** → the
  membership skeleton while the Expand resolves. **Error** (Identity unreachable) → "permission inspector
  temporarily unavailable" (the inspector fails STATIC — it never falls back to an Issues-side guess).
  **Permission-denied** → the inspector itself is permission-gated (you must be able to administer the
  project to inspect its permissions); a non-admin sees the gate, not a partial answer.

---

## 4. S16 — Automation / trigger builder (the frozen QueryAst condition + the ToolDef picker)

```
┌ Automation: auto-assign triage ───────────────────────────────────────────────────────────┐
│  When: [ state:triage  ∧  type:bug ]   (the frozen query-AST condition — no scripting)      │
│  Do:   [ assign → on-call ]   [ agent handler: triage-forecast ▾ ]   HITL: [ requires ✓ ]   │
│  ── Stateful trigger ──  "Remind me when  [ unblocked ]  · stale-after  [ 30d ]"           │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Backing engine:** `crate::trigger::{IssueTriggerEngine, ArmableCondition}` (ISS-P25). The condition is
  the frozen `ArmableCondition` (= `QueryAst` / `EventMatcher`, contract 3.4) — `GuardLanguage::FrozenQueryAst`,
  no second condition language.
- **Inline validation:** the agent-handler picker rejects an undeclared `ToolDef`; a side-effecting handler
  without a HITL gate surfaces the `requires_approval` default (never a silent un-gated agent effect).
- **States:** **Empty** ("no automations yet — add one"). **Loading** → the condition-builder skeleton.
  **Error** (invalid condition) → the builder flags the offending clause inline. **Permission-denied** →
  read-only automation list.

---

## 5. S18 — Audit / change-history (Issues contributes attribution, not the log)

```
┌ Change history: ENG-1421 ─────────────────────────────────────────────────────────────────┐
│  ● alice moved Todo → In Progress           2026-06-24 09:12   [human]                      │
│  ● triage-forecast set priority = high      2026-06-24 09:10   [AI · agent]                 │
│  ● bob commented "repro on staging"         2026-06-23 17:40   [human]                      │
└────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Backing engine:** contract 10.6 (the tamper-evident audit log). **Issues CONTRIBUTES attribution**
  (the actor / agent badges, the humanised activity strings — NOTIF-1) — it does **not own the log**. The
  timeline reads the upstream append-only log; agent-vs-human is rendered distinctly (AI-Act labelling).
- **States:** **Empty** ("no changes yet"). **Loading** → the timeline skeleton. **Error** → "history
  temporarily unavailable" (the log read failed static). **Permission-denied** → only the entries the viewer
  may see are shown (the same `SetExpr` leak-free discipline — a confidential change is absent, not redacted
  in place).

---

## 6. Cross-cutting rules applied (the §8b checklist + Forms & Controls + Overlays)

- **Every field has a programmatic label** (placeholder is never the label); **targets ≥ 24×24 CSS px**;
  **compact default** (`--control-h:28px`), `density:comfortable` lifts via tokens (Forms §1).
- **Status never by colour alone** — validation/state carries **glyph + label + position** (the `⚠` / `✓`
  lines above; WCAG 1.4.1). **Never inline-style colour on an interactive element** (tokens only).
- **The one primary per surface rides `--focus-ring`** (the derived token, not raw `--accent`) — the save
  button on each editor.
- **Overlays** (guard popover, assign/scheme dropdowns, breach-simulation preview) **portal to root**, share
  one z-index scale, centralise focus-trap / Escape / ARIA, and **flip when off-screen** (Overlays §8b.1/4).
- **Skeletons match the final layout; error blames the system in one line; degraded panes fail static** —
  the inspector + the audit log fail STATIC (never an Issues-side guess) when their upstream is unreachable.
- **Humanised strings at the backend** (NOTIF-1): the change-history shows "alice moved In Progress", never
  `issue.state_changed`.

---

## 7. The named floors (the prompt: "none new")

No governance screen opens a new floor. The anti-parallel-engine contracts (named in
`crate::governance::GovernanceFloors`) are the load-bearing constraints:

- **S15 inspector reads `list_subjects` / `explain` (4.4) — 0 private recompute** (`INSPECTOR_READS_EXPLAIN`).
- **S14 breach-simulation reuses the ISS-P26 SLA engine** (`BREACH_SIM_USES_SLA_ENGINE`).
- **S13 guard builder + S16 trigger condition reuse the frozen `QueryAst`** (`GUARD_BUILDER_IS_FROZEN_QUERYAST`).
- The S17 import wizard's **engine** is ISS-P28 (this view drives it); the concrete token-value table + the
  live styleguide land with the frontend foundation (ISS-P33+), the SAME named floors the views' pass
  carries (`design-system-pass.md` §7).
