# Myelin Design System — Component Inventory (Phase 8b · deliverable 02)

> **Direction:** finalist **A "Instrument"** (midnight command-deck; one rationed electric-blue accent;
> hairlines-first; near-zero radius; compact default; crisp/instant motion). **File date: 2026-06-20.**
> **Stack (frozen in [00-framework-and-buildout-plan.md](../00-framework-and-buildout-plan.md) §1):**
> TS + React (function components + hooks) · **React Aria Components** (headless behaviour/ARIA/focus) ·
> DTCG → Style Dictionary → `tokens.css` (the only tier components touch — [01-tokens](../01-tokens/tokens.md)) ·
> custom live styleguide · self-hosted fonts + icons, no CDN. **Not committed.**
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard (WCAG 2.2 / WAI-ARIA APG / EN 301 549)
> or an existing architecture/contract surfaced. **HOUSE STYLE** = our taste/synthesis (direction-A character).
> **`[DEFERRED-UNTIL-USERS]`** = a reception hypothesis no expert pass can settle, carried forward.

---

## 0. How to read this directory

Each spec file follows the **same fixed template** (the prompt's per-component spec template):

> **Name + purpose · the §5/research spec it implements · anatomy · variants + parameterization variant flags ·
> ALL states (default/hover/focus/active/disabled/loading + empty/error/permission-denied/erased/agent-pending
> where applicable) · keyboard + ARIA model (naming the React Aria Components primitive) · semantic tokens
> consumed · motion (token-based + reduced-motion path) · usage do/don't.**

The **build SEQUENCE is fixed by 00-plan §3.1** and is the order these were specced:
**Tier 0 foundations (tokens — done) → Tier 1 overlay primitives (FIRST) → Tier 2 shared components → Tier 3 surfaces.**
Overlays come first because they are *"the most expensive UX retrofit"* and every other component consumes
them (00-plan §3.1; design-language §8b.1).

---

## 1. The full component inventory (ALL components; ownership marked)

**Ownership legend:** **[F]** = foundational set, specced in THIS deliverable (this agent).
**[R]** = rich-components set, owned by the **rich-components agent** (separate deliverable — do not duplicate here).
**[T0]** = Tier-0 foundation, already shipped in [01-tokens](../01-tokens/).

### Tier 0 — Foundations (shipped)
| Component | Owner | Spec |
|---|---|---|
| Tokens (DTCG → `tokens.css`), type scale, icon set, contrast/a11y CI gates, live styleguide | **[T0]** | [../01-tokens/tokens.md](../01-tokens/tokens.md) |

### Tier 1 — Overlay primitives (BUILD FIRST — §8b.1)
| Component | Owner | Spec |
|---|---|---|
| **Dialog** (viewport-centred modal; trap + return) | **[F]** | [overlays.md](./overlays.md) |
| **ConfirmDialog** (`alertdialog`; safe-action default focus; reserved for irreversible/GDPR/HITL) | **[F]** | [overlays.md](./overlays.md) |
| **Popover** (anchored, non-modal, flips off-screen) | **[F]** | [overlays.md](./overlays.md) |
| **Dropdown / Menu** (inline-flow anchored list; roving) | **[F]** | [overlays.md](./overlays.md) |
| **Tooltip** (never takes focus; hover **and** focus) | **[F]** | [overlays.md](./overlays.md) |
| **Toast** (never steals focus; live-region; hosts undo) | **[F]** | [overlays.md](./overlays.md) |

### Tier 2 — Shared components
| Component | design-language § | Owner | Spec |
|---|---|---|---|
| **Navigation shell** (primary nav + contextual sidebar + content + context pane; responsive drawers) | §5.1 | **[F]** | [shell-and-nav.md](./shell-and-nav.md) |
| **Command palette** (⌘K; 4 modes; query-AST; permission-pre-filter) | §5.2 | **[F]** | [command-palette.md](./command-palette.md) |
| **Identity / agent badge** (one badge per `Principal`; reserved agent treatment) | §5.11 | **[F]** | [identity-and-agent-badge.md](./identity-and-agent-badge.md) |
| **Forms & controls** (button, input, select/combobox, checkbox/radio/switch, field + validation) | §5 (chrome) | **[F]** | [forms-and-controls.md](./forms-and-controls.md) |
| **Reference chip + unfurl** (the wedge; live, permission-aware, tombstones) | §5.3 | **[R]** | [reference-chip-and-unfurl.md](./reference-chip-and-unfurl.md) |
| **Agent / HITL approval card** (plan-then-apply; Approve/Edit/Reject; per-effect chips) | §5.4 / §6 | **[R]** | [agent-hitl-card.md](./agent-hitl-card.md) |
| **Comments / threads / mentions / reactions** (one thread model; anchored comments; review batching) | §5.5 | **[R]** | [comments-mentions.md](./comments-mentions.md) |
| **Views (table/board/calendar/list/gallery/timeline)** (six projections of one query AST) | §5.6 | **[R]** | [views.md](./views.md) |
| **Block editor** (one render path; `render(parse(md))===md`; slash menu; structured mention/ref nodes) | §5.9 | **[R]** | *rich-components agent* |
| **Notifications inbox** (one store / filtered views; "why it fired"; dedup; storm state) | §5.8 | **[R]** | *rich-components agent* |
| **Diff / files-changed viewer** (the hardest a11y component — no prior R-owner) | §7.1 | **[R]** | *rich-components agent (audit method: R-17 §5.1)* |

### Tier 3 — Surfaces
The design-language §7 view catalogue (per subsystem) — compositions of Tiers 1–2; **no surface introduces a
new primitive** (00-plan §3.1; design-language §8.3 rule 1). Not specced as components; built against these.

---

## 2. Coordination boundary (so the two component agents don't overlap)

- **This (foundational) set owns:** the overlay substrate, the shell/layout chrome, the palette, the
  identity/agent **badge**, and the form **controls** (button/input/select/checkbox/radio/switch/field).
- **The rich-components set owns:** the **reference chip + unfurl**, the **agent/HITL approval card**,
  **comments/mentions**, the **views** organism, the **block editor**, the **notifications inbox** (and the
  **diff** viewer per R-17 §5.1).
- **The seam, made explicit (the reuse invariants — R-10 §1):**
  - The **identity/agent badge** ([F], here) is consumed *inside* the chip, the HITL card, comments, views
    cells, the editor's mention node, and inbox rows ([R]). The agent **treatment** (label + plain geometric
    mark, never sparkle/emoji) is specced here once ([identity-and-agent-badge.md](./identity-and-agent-badge.md))
    and the rich set inherits it — it must not redefine it.
  - The **overlay substrate** ([F]) is consumed by the palette (modal Dialog), the unfurl (Popover), row-action
    menus (Dropdown), the HITL card (can overlay), and toasts ([R] consumers). The rich set builds *on* it and
    must not re-implement focus-trap / z-index / portal / Escape.
  - The **form controls** ([F]) are the atoms inside the HITL card's Edit form, the views inline-edit cell,
    the comment composer, and field-definition UI ([R]). The cell-editor / field-editor *molecules* are
    [R]'s (views/editor), built from these [F] atoms.
  - The **palette row**, the **dropdown item**, the **slash-menu item** ([R]), and the **inbox row** ([R])
    share one row shape (icon + label + kbd-hint). The canonical row atom lives with the palette/dropdown here;
    [R] reuses it.

---

## 3. Cross-cutting capabilities every component inherits (00-plan §4 — the acceptance bar)

1. **The command palette is a REAL primitive** — real `<input>`, fuzzy-filter live, run the active row,
   focus-trap + Esc + return-focus, roving listbox via `aria-activedescendant` ([command-palette.md](./command-palette.md)).
2. **Optimistic-update + honest rollback** — a shared optimistic-action affordance (apply immediately →
   pending → settle on ack or roll back honestly + undo-toast). Surfaced here via the **Toast** (undo host)
   and the **button** loading state; the *settle motion* is R-12's; the *rollback craft* is R-21's.
3. **`forced-colors` / high-contrast** is a **token-layer default** ([../01-tokens/tokens.css](../01-tokens/tokens.css)
   ships the `@media (forced-colors:active)` fallback) — inherited, not per-component.
4. **`aria-busy` + one polite live region on skeletons** — every loading state sets `aria-busy` and announces
   via one debounced polite region (never per-keystroke).
5. **The one derived `focus-ring` token covers every interactive element** including the autofocused palette
   input (00-plan §4 footnote; tokens §2.4).
