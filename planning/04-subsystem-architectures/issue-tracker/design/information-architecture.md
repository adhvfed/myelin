# Design — Information Architecture (Issue Tracker)

> Phase 4 design sketch (required before architecture; VISION §3/§5.2). Fits the **ONE-SHELL** design language
> (design-language §5.1: rail + contextual secondary nav + header + optional right context pane) and the **§7
> view catalogue** (§7.3 is the issue-tracker screen list). Every screen inherits the shared components (§5),
> tokens (§3), accessibility (§4), agent surfaces (§6), and the empty/loading/error/permission-denied/erased
> state set (§5.10). This doc fixes *structure & navigation*; flows are in `user-flows.md`, screens in
> `wireframes.md`.

## 1. Where Issues sits in the one shell

The shell is platform-owned (design-language §5.1). Issues contributes **one rail entry** ("Issues") and, when
active, **owns the secondary nav (left contextual sidebar) and the main content area**; the header, command
palette, global search, notifications-inbox entry, and identity menu are the shell's. The optional **right-hand
context pane** is where cross-artifact references unfurl (§5.3) — for Issues, the linked PRs/commits/CI
runs/docs/chat on an issue.

```
┌───────────────────────────────────────────────────────────────────────────────────────────┐
│ [Myelin]   ⌘K palette · global search                         🔔 inbox   ◐ theme   ⬡ me ▾   │  ← shell header (platform)
├──────┬────────────────────────────────────────────────────────────────────┬─────────────────┤
│ RAIL │  SECONDARY NAV (Issues-owned)        │  MAIN CONTENT (Issues-owned)  │ CONTEXT PANE    │
│      │                                      │                               │ (refs unfurl,   │
│ Code │  ▾ Team: ENG          [team switch]  │   <the active view: list /    │  §5.3; optional)│
│ CI   │    • My Work                         │    board / table / roadmap /  │                 │
│►Issue│    • Triage          (3)             │    issue detail / cycle / …>  │  Linked PRs     │
│ Docs │    • Active cycle                    │                               │  Linked docs    │
│ Chat │    • Backlog                         │                               │  CI runs        │
│ ──── │    • Roadmap                         │                               │  Backlinks      │
│ Inbox│    ▾ Views                           │                               │                 │
│      │      ⌗ Open bugs                     │                               │                 │
│      │      ⌗ My cycle                      │                               │                 │
│      │    ▾ Projects / Initiatives          │                               │                 │
│      │    ⚙ Team settings                   │                               │                 │
└──────┴──────────────────────────────────────┴───────────────────────────────┴─────────────────┘
```

**Persona-adaptive default landing (design-language §2):** an engineer lands on **My Work / Active cycle
(board)**; a PM lands on **Roadmap**; anyone can switch. The default is a per-role preference, never a lock —
the lens is a view, not a fork (the sketch-01 commitment: board and roadmap are co-equal views over one `issue`
table).

## 2. The navigation hierarchy (what nests in what)

```
Tenant (org)
 └─ Team  ──────────────── the primary scope selector in the secondary nav (prefix owner, TE-14)
     ├─ My Work            (a scoped VIEW into the one Notif inbox — C-9 / Notif §1.3; NOT a second store)
     ├─ Triage             (incoming queue; agent-assisted)
     ├─ Active cycle       (the current sprint/time-box)
     ├─ Backlog            (ordered, drag-to-rank)
     ├─ Roadmap            (initiatives/epics on a time axis — PM lens)
     ├─ Views              (saved query-AST views: list/board/table/calendar/timeline)
     ├─ Projects / Initiatives  (scope-axis containers; ranked issue types — sketch 01)
     └─ Settings           (schemes, SLA policies, automations, members, prefix)
```

Three **independent axes** surface as distinct nav concepts (the sketch-01 commitment — containment / time /
org-scope are separate):
- **Scope/containment** — Projects/Initiatives/Epics (ranked `issue` types) → roadmap + hierarchy panel.
- **Time** — Cycles (separate object) → cycle view + calendar.
- **Org-scope** — Team/Project (the identity scope object) → the secondary-nav team switcher + authz boundary.

An issue is reachable from *all three* (it's in cycle N, under epic E, owned by team ENG) and from cross-artifact
context (a PR, a chat message). It is never "filed" in one tree — it's an addressable node (`ArtifactRef`)
projected into many views. This is the §6 "reference everything" wedge made navigational.

## 3. The screen inventory (mapped to design-language §7.3 + Phase-2 §4)

| # | Screen | Nav location | Shared components used | Primary persona |
|---|---|---|---|---|
| S1 | **Issue detail** | open from any view/ref | editor §5.9, refs §5.3, comments §5.5, agent card §5.4, identity §5.11 | all |
| S2 | **List view** | secondary nav / Views | views component §5.6 (list) | engineer |
| S3 | **Board / Kanban** | secondary nav / Views | views §5.6 (board), presence §5.11 | engineer |
| S4 | **Table / spreadsheet** | Views | views §5.6 (table, inline-edit) | PM/corporate |
| S5 | **Timeline / Roadmap** | secondary nav: Roadmap | views §5.6 (timeline), refs §5.3 | PM/exec |
| S6 | **Backlog** | secondary nav: Backlog | views §5.6 (list + drag-rank) | engineer/PM |
| S7 | **Calendar** | Views | views §5.6 (calendar) | PM |
| S8 | **Cycle / Sprint** | secondary nav: Active cycle | views §5.6 + charts §3.7 | engineer/PM |
| S9 | **Triage inbox** | secondary nav: Triage | list §5.6 + agent card §5.4 | engineer (P5) |
| S10 | **My Work hub** | secondary nav: My Work | **scoped view of Notif inbox §5.8** | all |
| S11 | **Dashboards / Reports** | Team page / Reports | charts §3.7 (OLAP-fed) | PM/exec |
| S12 | **Saved-view manager** | Views ▸ manage | views §5.6 | all |
| S13 | **Workflow / scheme editor** | Settings | state-graph editor + AST guard builder (overlay primitives §8b.1) | admin (P15) |
| S14 | **SLA policy editor** | Settings | calendar editor + escalation builder | admin |
| S15 | **Team / Project settings** | Settings | forms + permission inspector | admin |
| S16 | **Automation / trigger builder** | Settings | AST builder + agent-handler picker + HITL config | admin |
| S17 | **Import wizard** | Settings ▸ Import | stepper + mapping preview + reconciliation report | admin |
| S18 | **Audit / change-history** | Issue detail ▸ History; Settings ▸ Audit | timeline + actor badges §5.11 | corporate/DPO |
| S19 | **Command palette / quick-create** | shell-global ⌘K | palette §5.2 (query-AST autocomplete) | all |

S10 and S18 are **shared-surface participations**: My Work is a *filter* over the one Notif inbox (never a
second inbox — C-9); Audit reads the tamper-evident audit log (ADR-12) — Issues contributes the actor/agent
attribution, not the log.

## 4. Right-context-pane policy (the wedge surface)

The context pane is where the cross-artifact graph becomes tangible (design-language §6). On **issue detail**
(S1) it shows, per-viewer-filtered (`list_objects`, never leaking — §5.3/ADR-03): linked PRs (with CI status),
linked commits, linked docs, linked chat threads, linked CI runs, and **backlinks** ("referenced by"). Each is a
live, permission-aware unfurl via Refs `resolve` (reference-graph §4.2) that **tombstones gracefully** on
erasure. A confidential issue's references never leak its title to an unauthorised viewer (the
reference-graph §6.4 / deep-dive §8.4 guarantee surfaced as UX).

## 5. Density & dual-audience adaptation (design-language §2/§P5)

The *same* views component (§5.6) renders engineer surfaces **dense + keyboard-forward** (`compact` density,
`j/k` nav, single-key actions) and PM/exec surfaces **spacious + chart-forward + pointer-friendly** — tuned by
**density tokens + default layout per role**, not different code. The board, the roadmap, the table are one
component (sketch 01: one `issue` table underneath). Vocabulary translates per space ("issue" ↔ "work item" ↔
"deliverable") as a presentation choice over one model (design-language §2), never a schema fork.

## 6. Mobile / responsive (SUB-X + design-language §8b.4)

Issues co-owns the **hover-action and width-takeover responsive cases** (SUB-X). The IA consequences:
- **Row actions** (assign/transition/snooze on a list row) are hover-revealed on desktop → must be **surfaced by
  default or behind an explicit affordance on touch** (§8b.4 "hover is not touch-reachable") — a `⋯` row-action
  menu on mobile.
- The **secondary nav (team views tree)** collapses to a **toggled drawer** (backdrop + Escape + route-change
  auto-close — §8b.4) at the breakpoint; the main view goes full-viewport (collapse the other column, never
  `width:100%`-beside — §8b.4).
- The **context pane** becomes a bottom-sheet / tab on mobile, not a clipped side column.
- The shell is pinned to the viewport (`100vh`/`overflow:hidden`); the active view is its own scroller with
  `min-height:0` (§8b.4) so a long board/list doesn't push the header off-screen.

## 7. CLI as a peer surface (design-language §7.7)

Every primary capability has a `myelin issue …` / `myelin cycle …` / `myelin view …` / `myelin report …` verb
(Phase-2 §5) against the *same* API the UI uses (no privileged back-channel), sharing the query AST and the
canonical `ArtifactRef` (`myelin://…/issue/issue/ENG-1421`). The chip you see and the handle you paste are the
same identity. `--json` everywhere for agents/scripts.
