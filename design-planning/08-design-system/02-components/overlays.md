# Overlays — Dialog · ConfirmDialog · Popover · Dropdown/Menu · Tooltip · Toast

> **Tier 1 · BUILD FIRST** (00-plan §3.1; design-language §8b.1). The shared overlay substrate where
> focus-trap + return-focus + scroll-lock + Escape/backdrop + portal-to-root + one z-index scale + correct
> ARIA live **once** and are inherited free by every consumer. **File date: 2026-06-20. Direction A.**
>
> **Implements:** design-language §8b.1 (the four mandates, verbatim) + R-10 §5 (the six primitives, shapes,
> ARIA, dismissal, state set, traps). **Maps to React Aria Components** primitives named per row.
>
> **Tagging:** **PROVEN** = WAI-ARIA APG modal-dialog / WCAG 2.2 cited. **HOUSE STYLE** = direction-A choices.

---

## 0. The four substrate mandates (design-language §8b.1 — VERBATIM, binding) — PROVEN

1. **Portal-always to the document root.** Every overlay portals to root, never renders in the triggering
   subtree — the "create dialog renders inside the 240px sidebar / clipped by a `transform`ed ancestor" bug
   class is forbidden by construction. *(React Aria: every overlay primitive renders through an internal
   portal by default.)*
2. **One documented z-index scale** as a single token set: `--z-chrome:100 < --z-popover:200 < --z-modal:300
   < --z-toast:400` (tokens.css §3.6). Per-component magic z-index numbers are **banned**. A toast always
   clears a modal; a modal always clears a popover.
3. **Centralised behaviour lives in the substrate, inherited free:** focus-trap + return-focus, scroll-lock
   with scrollbar-width compensation, Escape + backdrop dismiss, correct ARIA, background `inert`. Consumers
   **never re-implement these.** *(PROVEN — WAI-ARIA APG dialog-modal; React Aria's `useOverlay`/`FocusScope`/
   `usePreventScroll`/`useModalOverlay` implement exactly this.)*
4. **Single-purpose by shape** (HOUSE STYLE): split overlays by shape — viewport-pinned modal / anchored
   non-modal popover / inline-flow menu — "nine menus are three shapes." Do not force one component to do all.

**One substrate atom:** the `<OverlayContainer>` + `<FocusScope contain restoreFocus autoFocus>` + a
scroll-lock hook is the shared atom every overlay below composes from. The focus/trap/return/scroll-lock/ARIA
logic is written **once here** and is the mechanical a11y guarantee for every transient/modal surface (R-17 §5.7).

---

## 1. The six primitives at a glance (R-10 §5.2) — PROVEN shapes

| Primitive | Shape | Modal? | Focus | Dismiss | React Aria primitive |
|---|---|---|---|---|---|
| **Dialog** | viewport-centred | yes | trap + return | Esc · backdrop · close button | `Modal` + `Dialog` (`DialogTrigger`) |
| **ConfirmDialog** | small modal | yes | trap + return; **default-focus the SAFE action** | Esc = cancel | `Modal role="alertdialog"` + `Dialog` |
| **Popover** | anchored, flips | no | moves in; returns; **no background trap** | Esc · click-outside | `Popover` + `Dialog` |
| **Dropdown / Menu** | inline-flow list | no | roving within; returns | Esc · click-outside · select | `MenuTrigger` + `Menu`/`MenuItem` |
| **Tooltip** | tiny label | no | **never takes focus**; hover **and** focus | blur · Esc · pointer-leave | `TooltipTrigger` + `Tooltip` |
| **Toast** | corner region | no | **never steals focus**; AT via live region | auto-timeout · manual · pause-on-hover | `ToastRegion` + `Toast` (`UNSTABLE_ToastQueue`) |

---

## 2. Dialog

**Purpose.** The viewport-centred modal for create/edit forms, settings, branch-protection editor — any
focused task that must own the screen while open. **Implements** R-10 §5.2 (Dialog row) + APG modal-dialog.

**Anatomy.** scrim (`--overlay-scrim`) → portalled panel (`--surface-overlay`, `--shadow-overlay`, hairline
`--border-strong`, `--radius-2`) → header (title `--text-primary` h2/h3 + close button) → body (own scroller,
`min-height:0`) → footer (action row; primary action right, on `--c-btn-primary-bg`). Background is `inert`.

**Variants.** size `sm | md | lg` (max-inline-size tokens; never fixed-width — German strings must not clip,
§8b.4). `dismissable` (default true) vs `non-dismissable` (a blocking step that must be resolved — rare).

**Parameterization variant flags.** `density` sets header/footer padding and control height (compact: 28px
controls; comfortable: 32px). `tone` affects only copy voice in empty/confirmation bodies, never chrome.
No `switch(direction)` — re-skins via tokens only.

**States.**
- **default / open:** appears (does not animate-in §8b.6); focus moves to the first focusable or the panel.
- **opening / closing:** `--dur-fast` (140ms) opacity + 2px rise on enter, `--dur-micro` exit; `--ease-enter`/
  `--ease-exit`. **Reduced-motion:** instant show/hide (first-class path).
- **loading (async content):** body shows structure-skeleton + `aria-busy="true"`; never a centred spinner.
- **error (async submit):** inline one-line system-blaming error inside the dialog; **dialog stays open,
  user input intact** (§8b.6). Announced in the dialog's polite live region.
- **nested:** a ConfirmDialog over a Dialog — z-scale + focus-trap stack keep order; **Esc closes top-most only.**
- **permission-denied / erased / agent-pending:** not Dialog-intrinsic — these are content states the hosted
  component renders (e.g. a no-access card *inside* the dialog body).

**Keyboard + ARIA (PROVEN — APG dialog-modal).** `role="dialog"` + `aria-modal="true"` +
`aria-labelledby` (title) + `aria-describedby` (optional). Focus moves in on open; **trapped** (Tab/Shift-Tab
cycle, no background escape); **returned to the trigger** on close. Esc closes + returns. Background `inert`.
→ **React Aria `Modal`/`Dialog`** provides all of this.

**Tokens.** `--surface-overlay`, `--overlay-scrim`, `--shadow-overlay`, `--border-strong`, `--radius-2`,
`--text-primary`, `--focus-ring`; spacing/motion/z (`--z-modal`) from the scales.

**Motion.** enter 140ms / exit 90ms, token easings; reduced-motion → 0ms instant.

**Do / Don't.**
- **Do** portal to root; **do** return focus to the trigger; **do** keep input on async error.
- **Don't** set a magic z-index; **don't** render the dialog inside the sidebar subtree; **don't** animate a
  paragraph in — the page renders, it doesn't slide.

---

## 3. ConfirmDialog

**Purpose.** The small modal that gates **irreversible / consequential + GDPR + agent-HITL** actions —
the deliberate carve-out from reversibility-over-confirmation (§8b.6). Everything else prefers an undo-toast.
**Implements** R-10 §5.2 (Confirm row) + §8b.6 confirm carve-out; APG `alertdialog`.

**Anatomy.** Same modal shell as Dialog, smaller. Title (the consequence, plain language) → describedby body
(what will change, on what — for GDPR/HITL, the concrete effects) → action row: **Cancel (default-focused) +
the destructive/confirming action** (`--c-btn-danger-bg` for destructive; `--c-btn-primary-bg` otherwise).

**Variants.** `confirm` (neutral, primary action) vs `destructive` (danger action, danger token + glyph).
Optional `requireReason` (a required one-line input — used by HITL Reject, R-14 §3.2).

**Parameterization variant flags.** `tone` shapes the consequence copy; `agentPresence` does not change the
component (HITL routes *into* ConfirmDialog regardless of presence default).

**States.** default (Cancel focused), hover/focus/active on the two buttons, **loading** (the confirming
action shows in-button spinner + `aria-busy`; Cancel stays operable), **error** (inline, dialog stays open).
disabled confirm until `requireReason` is non-empty.

**Keyboard + ARIA (PROVEN).** `role="alertdialog"` + `aria-describedby` (the consequence text is announced).
**Default focus = the SAFE action (Cancel)** — never the destructive one. Esc = Cancel. → React Aria
`Modal role="alertdialog"`.

**Tokens.** as Dialog + `--danger`/`--on-danger` (destructive), `--c-btn-primary-bg` (confirm).

**Motion.** as Dialog.

**Do / Don't.**
- **Do** reserve Confirm for the consequential/GDPR/HITL set; **do** default-focus Cancel; **do** show concrete
  consequences, not "Are you sure?".
- **Don't** put a Confirm on every action (confirm-fatigue trains click-through — R-10 §5.6); **don't**
  default-focus the destructive action.

---

## 4. Popover

**Purpose.** Anchored, **non-modal** floating surface for the reference unfurl hovercard ([R]), the filter
builder, a date picker. **Implements** R-10 §5.2 (Popover row) + WCAG 2.2 1.4.13 (content on hover/focus).

**Anatomy.** anchor (the trigger) → portalled panel (`--surface-overlay`, `--shadow-popover`, hairline,
`--radius-1`) → optional arrow/beak. Positioned relative to anchor; **flips above + caps max-height** when it
would overflow the viewport — **tested against the REAL anchor** (a picker under a bottom-pinned composer
renders off-screen otherwise — §8b.4).

**Variants.** `hover` (hovercard — opens on hover *and* keyboard focus; WCAG 1.4.13 dismissable/hoverable/
persistent) vs `click` (interactive popover — filter builder). placement `top|bottom|start|end` (auto-flip).

**Parameterization variant flags.** `density` sets internal padding. On mobile, `surfaceUnification` is
irrelevant; layout flag in §8b.4 turns the popover into a bottom-sheet drawer (see §9 below).

**States.** default, hover, focus, **loading** (async content → structure-skeleton + `aria-busy`, never
spinner), **error** (inline one-line), **anchored-overflow** (flip + max-height + own scroller),
**mobile** (→ bottom-sheet drawer: backdrop + Escape + route-change auto-close, §8b.4). permission-denied /
erased are content states (the unfurl renders its own no-access/tombstone — [R]).

**Keyboard + ARIA (PROVEN).** Non-modal: focus may move in (click popover) but **no background trap**; Esc
and click-outside dismiss; **focus returns** to the trigger. `role="dialog"` (non-modal) or a labelled region.
A **hovercard shows on focus as well as hover** and is **dismissable** (Esc), **hoverable** (pointer can move
onto it), and **persistent** (doesn't vanish on a brief pointer-leave) — WCAG 2.2 1.4.13. → React Aria
`Popover` (non-modal, auto-flip placement, `OverlayArrow`).

**Tokens.** `--surface-overlay`, `--shadow-popover`, `--border`, `--radius-1`, `--text-*`, `--focus-ring`,
`--z-popover`.

**Motion.** open `--dur-micro` (90ms) opacity; reduced-motion → instant.

**Do / Don't.**
- **Do** auto-flip and cap height against the real anchor; **do** make hovercards keyboard-reachable + 1.4.13-compliant.
- **Don't** trap focus (it's non-modal); **don't** let a hovercard vanish before the pointer can reach it.

---

## 5. Dropdown / Menu

**Purpose.** Inline-flow anchored list for **row actions, block-convert, palette overflow**, the identity menu.
**Implements** R-10 §5.2 (Dropdown row) + APG menu/menu-button.

**Anatomy.** trigger (button) → portalled menu panel (`--surface-overlay`, `--shadow-popover`, `--radius-1`)
→ menu items (the **shared row atom**: icon `--text-muted` + label `--text-primary` + optional kbd-hint
`--text-subtle` mono + optional submenu chevron) + separators (non-interactive).

**Variants.** `actions` (menuitems) vs `selection` (`listbox`/`option` with checkmarks — single/multi).
`withSubmenus`. `withSections` (labelled groups).

**Parameterization variant flags.** `density` sets item height (compact 28px / comfortable 32px row).

**States.** default, **hover** (`--surface-hover`), **focus/active** (roving — active item gets `--focus-ring`
border-inline-start marker), **disabled item** (`--text-subtle`, not focusable in actions mode; explained),
**loading** (async menu → skeleton items + `aria-busy`), **empty** (a quiet "No actions available" line —
never an empty panel). permission-denied: a verb the viewer can't run is **absent**, never greyed (no teasing).

**Keyboard + ARIA (PROVEN — APG menu).** `role="menu"`/`menuitem` (actions) or `listbox`/`option` (selection).
**Roving** within (↑/↓/Home/End, type-ahead); Enter/Space activate; Esc closes + returns focus; left/right for
submenus. → React Aria `MenuTrigger`/`Menu`/`MenuItem` (roving + type-ahead + submenu built-in).

**Tokens.** `--surface-overlay`, `--surface-hover`, `--shadow-popover`, `--border`, `--text-primary/muted/subtle`,
`--focus-ring`, `--z-popover`.

**Motion.** open `--dur-micro`; reduced-motion → instant.

**Do / Don't.**
- **Do** reuse the shared row atom (same shape as palette row / slash-menu item); **do** omit unpermitted verbs.
- **Don't** use a Menu for a modal task (that's a Dialog); **don't** grey-out an action you could instead explain.

---

## 6. Tooltip

**Purpose.** Tiny anchored label for icon-button names and truncated text. **Implements** R-10 §5.2 (Tooltip
row) + WCAG 2.2 1.4.13.

**Anatomy.** anchor → portalled label (`--surface-overlay`, hairline, `--radius-1`, `--text-primary`, caption
size). Never interactive content (that's a Popover).

**Variants.** placement (auto-flip). `instant` vs default open-delay.

**States.** hidden / shown. No focus state of its own. Shows on **hover AND keyboard focus**; hides on blur /
pointer-leave / Esc. **Dismissable** (Esc) and **persistent** (1.4.13).

**Keyboard + ARIA (PROVEN).** `role="tooltip"` + the trigger's `aria-describedby` points at it. **Tooltip
never takes focus** and never steals it. → React Aria `TooltipTrigger`/`Tooltip` (shows on focus, Esc-dismiss,
1.4.13 by construction).

**Tokens.** `--surface-overlay`, `--border`, `--radius-1`, `--text-primary`, `--z-popover`.

**Motion.** `--dur-micro` fade; reduced-motion → instant.

**Do / Don't.**
- **Do** give every icon-only button a tooltip *and* an `aria-label`; **do** show on focus, not hover-only.
- **Don't** put actions or links in a tooltip (use a Popover); **don't** rely on a tooltip to convey essential
  info that has no other surface.

---

## 7. Toast

**Purpose.** Transient corner notice for **optimistic-settle confirms, async results, and undo** — the host of
the cross-cutting optimistic-rollback affordance (00-plan §4.2). **Implements** R-10 §5.2 (Toast row) +
§8b.6 reversibility-over-confirmation + WCAG 4.1.3.

**Anatomy.** a dedicated portalled corner region (`--z-toast`) → toast (`--surface-overlay`, hairline,
`--shadow-overlay`, `--radius-1`) → status glyph + label (glyph+text, never colour-alone) + optional
**Undo** action button + dismiss.

**Variants.** `info` (default), `success`, `warning`, `danger` (each = token + glyph + label, never colour
alone). `withUndo` (the optimistic-settle / honest-rollback case — apply immediately, toast "Moved to In
Progress · Undo"). `persistent` (no auto-timeout; for an error needing acknowledgement).

**Parameterization variant flags.** none change behaviour; `tone` affects message voice only.

**States.** enter / **shown** (auto-timeout, **pause-on-hover** and **pause-on-focus**) / exit. **Undo
hovered/focused/activated** (rolls back honestly; the typed content / prior state is never lost). danger toast
= `persistent` by default. There is no permission/erased/agent-pending state on the Toast itself.

**Keyboard + ARIA (PROVEN — WCAG 4.1.3).** A toast **never steals focus.** AT is informed via a live region:
**`role="status"` (polite)** for the vast majority; **`role="alert"` (assertive)** only for genuinely
time-critical/blocking events. Toasts with actions are reachable via a documented hotkey (React Aria's
`ToastRegion` places focus into the region on `F6`/the configured key, so keyboard users can reach Undo).
→ React Aria **`UNSTABLE_ToastQueue` + `ToastRegion`/`Toast`** (focus-management + live-region built in).

**Tokens.** `--surface-overlay`, `--shadow-overlay`, `--success`/`--warning`/`--danger`/`--info` (+ `-subtle`),
`--text-primary`, `--focus-ring`, `--z-toast`.

**Motion.** enter `--dur-fast` slide+fade from edge; exit `--dur-micro`; reduced-motion → instant show/hide
(the *information* — the message and the Undo — is never lost, only the animation).

**Do / Don't.**
- **Do** prefer an undo-toast over a Confirm for reversible actions; **do** use polite by default, assertive
  only for critical; **do** keep Undo keyboard-reachable.
- **Don't** auto-dismiss a toast carrying the only Undo affordance too fast; **don't** steal focus; **don't**
  spam — coalesce rapid settles.

---

## 8. Nested overlays (the foot-gun — R-10 §5.3 / R-17 §5.7) — PROVEN

A Confirm over a Dialog, a Dropdown inside a Popover: the **z-index scale** (§0.2) plus a **focus-trap stack**
keep order correct. **Esc closes the top-most only.** Background of each modal layer is `inert`. The substrate
must be tested against the deep stack (Confirm-over-Dialog-over-Popover). → React Aria's overlay system stacks
`FocusScope`s and manages this; the test is ours to write.

## 9. Responsive / mobile (§8b.4) — PROVEN bug classes

- A Dropdown/Popover **may become a bottom-sheet drawer** on small viewports: backdrop + Escape + **route-change
  auto-close**.
- The contextual sidebar / context pane become drawers (shell spec). All drawers reuse this substrate (portal,
  focus-trap, scroll-lock).
- **Flip + max-height + real-anchor test** is mandatory (a picker under a bottom-pinned composer is the classic
  off-screen bug).

## 10. The traps (R-10 §5.6) — what review catches

(a) **Per-feature overlays** re-implementing (and breaking) focus-trap/Escape/ARIA → the single substrate is
the only defence. (b) **Magic z-index** arms race → the single scale token bans it. (c) **Clipped overlay** in
a `transform`ed/`overflow:hidden` ancestor → portal-always. (d) **Confirm fatigue** → reversibility-over-confirmation.
(e) **Off-screen popover** under a bottom-pinned anchor → flip + real-anchor test. (f) **Focus-not-returned** →
the substrate's return-focus is non-negotiable. **Falsifiable review rule:** no component reads `direction`; no
component sets its own z-index; no component re-implements the trap.
