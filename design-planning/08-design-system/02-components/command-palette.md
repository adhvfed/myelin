# Command Palette — the ⌘K keyboard nerve-centre (a REAL primitive)

> **Tier 2.** The single keyboard surface to **Navigate · Act · Search · Build-query** over one IA, through
> one query AST, against one permission-pre-filter, using the same typed action catalogue agents use.
> **File date: 2026-06-20. Direction A "Instrument" (palette-led is A's nav default).**
>
> **Implements:** design-language §5.2 + **R-08 command-palette** (the full spec) + 00-plan §4.1 (the top
> cross-cutting gap: the palette must be a REAL primitive — real `<input>`, fuzzy-filter live, run the active
> row, focus-trap + Esc + return-focus, roving listbox via `aria-activedescendant`). Built on the [F] overlay
> modal substrate ([overlays.md](./overlays.md)). Audited against R-17 §5.6.
>
> **Tagging:** **PROVEN** = WAI-ARIA APG combobox / WCAG / architecture contract. **HOUSE STYLE** = direction-A.

---

## 1. Purpose — and the gap it closes (00-plan §4.1) — PROVEN-required

The panel flagged the palette as **non-functional in 3/4 finalists and un-typable in the 4th** (the input was
a fake `<span>` — decision-brief §7.1). This spec makes it real and non-negotiable:

- `⌘K` / `Ctrl-K` opens → **a real `<input>`** (not a span) → **fuzzy-filter live** → **run the active row**.
- **Focus-trap while open + Esc to close with return-focus** (inherits the [F] Dialog modal substrate).
- A **roving listbox via `aria-activedescendant`** — **DOM focus stays on the input**, the active row is
  pointed at; this is what lets typing and arrow-navigating coexist (APG combobox pattern).
- The advertised **j/k, ]/[ system bindings** are real handlers, and **every bare shortcut's action is also
  in the palette with its shortcut shown** (the anti-discoverability-cliff rule, R-08 §3.2).

---

## 2. The four modes (R-08 §2) + mode signalling

One input with mode *signalling*, not four palettes. A **mode pill** at the input's logical-start reads the
inferred mode; it is **a word, never colour-only** (WCAG 1.4.1).

| Mode | Trigger | Does | Pill |
|---|---|---|---|
| **Navigate** (default) | open + type a name fragment | jump to any `ArtifactRef` in the IA (repo/PR/issue/page/channel/run/view/person/**agent**/admin) | `Go to` |
| **Act** | type a verb, or `>` prefix | run a permitted action = a typed `ToolDef` (same catalogue agents use); targets the focused artifact or prompts | `Run` |
| **Search** | `Tab`/`Enter` on the "Search for '…'" row, or `?` | hand off to the cross-artifact engine; "Open in Search view" overflow | `Search` |
| **Build-query** | a field token (`status:`, `assignee:@me`, `#`, `is:`) or `⌘F` in a views surface | compose a **query AST** (the same AST that saves as a view or arms an agent trigger) | `Filter` |

- **No prefix is ever mandatory** — plain text always works; prefixes only sharpen (R-08 §2.2; the new-PM
  discoverability cull-check, §10).
- **Seamless promotion:** Navigate that matches content surfaces a "Search for '…'" row (Tab to promote); a
  known field key promotes to Build-query in place. **No modal re-entry** — one continuous keystroke stream.
- **Scope is orthogonal to mode:** a scope chip (logical-start, after the mode pill) seeds from the open
  artifact (`in:repo/payments`); `⌘⇧K` toggles global↔local; Backspace-on-empty clears it.

---

## 3. Anatomy

```
┌─ Command palette (modal, portalled, scrim, --shadow-overlay) ──────────────┐
│ [pill: Filter] [chip in:acme ×] [chip status:in review ×]  cache▏  modes →  │ ← input row
├────────────────────────────────────────────────────────────────────────────┤
│ GROUP: Filter · matching your scope                                         │ ← group header (separator)
│  ▸ [ic] Payments cache thrashes…   In review · PR #412    ISS-377           │ ← row (active = focus-ring marker)
│        why: matched "cache" in title · Q3 Payments reliability              │
│  GROUP: Act · (same ToolDefs agents use)                                    │
│    [ic] Open PR #412 against main (protected)   [confirms before apply] ⚠   │ ← consequential → routes to gate
│    [ic] Arm as agent trigger — FixAgent [Agent]                             │ ← agent badge (label, not colour)
├────────────────────────────────────────────────────────────────────────────┤
│ ↑↓ move · ↵ open · ⇥ complete · → peek · esc close   ✓ showing only what    │ ← footer cheat-sheet + perm cue
│                                                          you can see        │
└────────────────────────────────────────────────────────────────────────────┘
```

- **The row atom** (shared with Dropdown item / slash-menu item / inbox row): `[type icon] primary label
  (lens-aware) · status hint (glyph+label)  [kbd shortcut]` + a `why` line (match provenance). The icon
  disambiguates type (PR vs issue vs run) for colour-blind users — **not colour-alone.**
- **Query chips** = the AST rendered humanly (`status:in review` chip; the AST stores the canonical
  `status_in_progress`). Chips are logical-property-based → RTL mirrors for free.
- **Permission cue** in the footer: "showing only what you can see."

---

## 4. Variants + parameterization variant flags

**Variants.** mode (4, §2) · scope (global / local) · with/without active peek (→ opens a row's inline unfurl).

**Parameterization variant flags.**

| Flag | Effect |
|---|---|
| **`nav`** | A's default is `palette-led` → the palette is the spine, its trigger foregrounded in the shell. At `rail`/`contextual` the palette is **still present and identical** (one palette every screen) — only its emphasis in the shell changes. **Component never branches on this.** |
| **`density`** | row height (compact 32px / comfortable) and result count visible. |
| **`agentPresence`** | whether the "Arm as agent trigger" / agent rows surface inline by default (ambient) or are foregrounded — gated by authority either way (absent if unauthorised). |
| **`tone`** | empty/no-results copy voice. |

---

## 5. ALL states (R-08 §8 — the happy path is the smallest part) — PROVEN-required

| State | Behaviour |
|---|---|
| **idle / empty input** | **Recents + suggested actions for the focused artifact** (never a blank box); `?` cheat-sheet hint visible. |
| **default row** | resting row; icon `--text-muted`, label `--text-primary`. |
| **hover** | `--surface-hover`. |
| **focus / active** | DOM focus stays on the input; the **active row** gets the `--focus-ring` border-inline-start marker + a faint `--accent-weak`→transparent wash; `aria-activedescendant` points at it. |
| **loading** | network-dependent results show **structure-skeleton rows** in the same container (local-pool renders <100ms; only the overflow skeletons); **suppress flash <~1s**; input stays live; **`aria-busy` + one polite live region announces "loading results" once** (never per keystroke). |
| **no results** | one quiet line "No matches for '…'." + escape hatches ("Search everywhere", "Create … named '…'", "Clear filters"); never blames the user. |
| **no access (specific ref)** | the graceful no-access row: "You don't have access to this item" — **no title, no metadata, indistinguishable from not-found** (the anti-oracle rule; a targeted ref the viewer can't see). |
| **error** | one quiet system-blaming line ("Search is temporarily unavailable — local results shown") + retry; **degrades to the local pool** so Navigate/Act still work (fails useful, not blank). |
| **permission-changed mid-session** | a cached recent the user lost access to is silently dropped on resolve; if mid-open, the row tombstones to no-access. |
| **stale / offline** | a subtle "results may be out of date" cue on the local-pool fallback; refresh on reconnect. |
| **erased / tombstoned** | a recent pointing at an erased artifact renders as a tombstone ("This item was deleted"), never a dangling title. |
| **agent-armable** | Build-query mode + agent-trigger authority → an "Arm as agent trigger" row (with the [Agent] badge); **absent if unauthorised** (no teasing a verb you can't run). |
| **consequential verb** | an Act row that is consequential shows the gate marker ("confirms before apply") and **routes INTO the [F] ConfirmDialog / HITL gate — never a fast path around it.** |
| **disabled** | n/a as a palette state — unavailable rows are absent, not disabled. |

---

## 6. Keyboard + ARIA model (R-08 §3, §11; R-17 §5.6) — PROVEN

**The keymap (the binding contract):**

| Key | Behaviour |
|---|---|
| `⌘K` / `Ctrl-K` | open from anywhere; again or `Esc` closes |
| *(type)* | fuzzy-filter/parse live; updates listbox + mode pill |
| `↓` / `↑` | move active row (wraps); `aria-activedescendant` follows |
| `Home` / `End` | first / last row |
| `Enter` | execute active row (Navigate→open · Act→run · Search-row→promote · query-value→commit chip) |
| `⌘Enter` | open in background / secondary target |
| `Tab` | **complete-then-exit**: complete the active suggestion into the input (e.g. `status:` ready for a value; "Search for '…'" → Search mode); with no completion, Tab exits to next focusable. *(HOUSE STYLE overload of APG Tab-to-close — flagged for AT verification, §10.)* |
| `→` | open a row's inline peek/unfurl without leaving the palette |
| `←` / `Backspace`-on-empty | pop the last query chip / scope token; collapse a peek |
| `Esc` | close peek → else clear non-empty query → else close palette + **return focus to the prior element** |
| `?` (empty input) | show the keyboard cheat-sheet / mode legend |

**ARIA (PROVEN — APG editable combobox + list autocomplete).** The palette is an **editable combobox with a
listbox popup**: `role="combobox"` input (`aria-expanded`, `aria-controls`, `aria-activedescendant`) +
`role="listbox"` popup + `role="option"` rows. **DOM focus stays on the input;** `aria-activedescendant` tracks
the active row (this is *the* pattern — not a custom invention). The active option is **scrolled into view**
(200% zoom). Group headers are **non-option separators** arrow-nav skips. **Focus trapped in the modal,
returned on close** (inherits the [F] Dialog substrate); **no trap** beyond the modal contract.
**Result count / loading / error announced via ONE polite live region, debounced — not per keystroke.**
A no-access target announces "no access," never a leaked title.

**React Aria mapping.** The palette is a **`Dialog`/`Modal`** (the [F] overlay substrate: trap + return +
portal + Esc) hosting React Aria's **`ComboBox`** (or `Autocomplete` + `ListBox` in newer RAC) for the
combobox/listbox semantics + `aria-activedescendant` roving. The mode pill / scope chip / query chips are
non-interactive labels + small `Button`s (chip remove). The cheat-sheet is a `Popover`/inline region.

---

## 7. The query-AST surfacing + human↔agent symmetry (R-08 §4, §7) — PROVEN mechanism

- **token → chip → AST:** a recognised field key becomes a filter chip; the row of chips **is** the AST. Field
  autocomplete is **permission- and schema-aware** (only fields you may filter on, for the scoped type).
  Operators humanised (`updated:<7d` → "updated in the last 7 days"); the chip shows the human phrase, the AST
  stores canonical. Free text + structured coexist in one AST.
- **The three lives of one AST:** `⌘S` → saved view; `Enter` → live result set; "Arm as agent trigger" →
  agent trigger condition (where authorised). A user learns the grammar once; the chip row in the palette is
  visually the same grammar as a saved view's filter bar (a checkable D4 coherence property).
- **Palette actions ARE the typed `ToolDef`s agents use:** one catalogue, one ReBAC `list-objects` pre-filter,
  one typed-arg schema, one audit trail. Consequential verbs carry their consequence and **route into the HITL
  gate, not around it.** Persona-adaptive vocabulary resolves lens labels to the canonical field (PM "work
  item" and engineer "issue" land on the same type); the chip renders the viewer's lens label, the AST stores
  canonical.

---

## 8. Permission-pre-filter as UX behaviour (R-08 §6) — PROVEN (ADR-03)

Pre-filtered, **never** post-filtered: results the user can't access are **absent from the candidate set** —
never fetched-then-hidden, never greyed, never count-shown. **No title leak, ever** (a targeted no-access ref
→ the graceful no-access row). **Counts don't leak** ("12 results" = permitted only; no "(3 hidden)").
**No-access is indistinguishable from not-found** — the palette cannot be used to probe whether an artifact
exists (the anti-oracle rule; a trust affordance for P12/P13).

## 9. Semantic tokens consumed

`--surface-overlay` (panel), `--overlay-scrim`, `--shadow-overlay`, `--border`/`--border-strong`, `--radius-2`;
`--text-primary/muted/subtle`; `--focus-ring` (active-row marker + input focus — the autofocused input is
covered by the one derived focus token, 00-plan §4 footnote); `--accent-weak` (active-row wash); `--agent` +
"Agent" label (agent rows — via the [F] identity/agent badge); `--warning` (gate marker, with glyph+label);
`--success` (the permission cue check); spacing/motion/z (`--z-modal`).

## 10. Motion + reduced-motion (R-08 §3.3)

Open/close: opacity show via the [F] Dialog substrate at `--dur-fast`; the palette **never animates in** under
`prefers-reduced-motion` (instant show — first-class path). Active-row marker transitions at `--dur-micro`.

## 11. Usage do / don't

- **Do** use a real `<input>` with `aria-activedescendant`; **do** keep DOM focus on the input; **do** show the
  shortcut on each row (the palette teaches the muscle-memory path); **do** route consequential verbs into the gate.
- **Do** keep a **visible pointer affordance** for the palette in the shell (never keyboard-only entry) and
  **never require a prefix** (the two new-PM discoverability cull-checks, R-08 §10).
- **Don't** ship a fake-span input; **don't** post-filter results or leak a no-access title or count; **don't**
  branch on the finalist name for the `nav` flag; **don't** announce per keystroke.

## 12. Carried flags (honesty)

- **`Tab`-overload (complete-then-exit, §6)** is a HOUSE-STYLE deviation from APG plain Tab-to-close — must be
  verified not to confuse AT users in the R-17 audit; fallback is strict APG Tab-to-close + a separate
  completion key. **`[DEFERRED-UNTIL-USERS]`** for the AT-user confirmation.
- Whether real PMs form the `⌘K` habit (vs always clicking the visible affordance) is the one genuinely
  user-testable question (R-08 §11) — **`[DEFERRED-UNTIL-USERS]`.** Ranking weights are HOUSE STYLE, tunable.
