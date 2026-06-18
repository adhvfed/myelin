# UX & Design

Myelin must be top-of-the-line in UX and design, and design must come before implementation
for any frontend. The throughline of everything below: **user-friendliness is not a skin on
top of the architecture — it is the architecture, presented well.** Almost every principle
here is one that platforms re-learn the *hard way* when the primitives are built late, so the
secondary message is: **build these first.**

---

## 1. Build the overlay primitives on day one

The single most expensive UX retrofit is overlays. Build, before any feature uses them, a
small set of **primitives**: Dialog, ConfirmDialog, Popover, Dropdown, Tooltip, Toast. Each
must, from the start:

- **Always portal to the document root.** A modal or menu rendered inside a transformed or
  `overflow`-clipped ancestor will re-root or clip in ways that look like impossible bugs (a
  "create" dialog that renders inside a 240px sidebar column because the sidebar carries a
  `transform`). Portal-always kills this whole class.
- Centralise **focus-trap + return-focus, scroll-lock with scrollbar-width compensation,
  Escape/backdrop dismiss, and ARIA wiring** in the primitive, so every consumer inherits
  correct behaviour for free.
- Share **one documented z-index scale** (e.g. chrome < popover < modal < toast). Per-component
  magic z-index numbers collide unpredictably (a toast stacking under a modal by DOM order).
- **Failure mode if ignored:** the same overlay is hand-rolled five or six times; each copy
  re-implements focus/dismiss/positioning slightly differently; one forgets to portal and
  clips; one forgets ARIA; and you find them one at a time over months. Replacing native
  `window.confirm` and ad-hoc menus *after* the fact is pure rework.
- **Keep each primitive single-purpose.** "Nine anchored menus" are usually three different
  shapes (a viewport-pinned popover, an inline flow dropdown, a grid someone else positions).
  Forcing all three onto one component complects them — split by shape.

## 2. One editor render path

If you build a block/rich-text editor, the deepest trap is **two divergent renderers** — one
for read mode, one for edit mode — with no shared source of truth. They *will* drift (read
mode showing raw `**bold**`, edit mode showing styled text), and unifying them later is a
long, repeated fight.

- **Read and edit must run the exact same inline parser.** Build one `parseInline`-style
  pipeline and point both modes at it; make round-trip (`render(parse(md)) === md`) a
  correctness gate over a corpus.
- **A plain `<textarea>` fundamentally cannot show formatting as you type it** (you can't show
  bold while the `**` is still being typed). The honest path is a controlled `contenteditable`
  — intercept structural input events, let plain text through, normalise on serialise. Model
  the caret as a **character offset into the serialised markdown** and bridge it to/from DOM
  positions. Expect browser-variance (Enter, IME, paste) to be the top risk.
- **Store inline content as a markdown-subset string** (not inline-range JSON): it needs no
  server-side sanitisation, survives copy/paste/export/diff, keeps the reference grammar
  server-side, and survived an entire editor rewrite with zero schema migration.
- Even Enter-splits-a-block and caret-placement-after-split are non-trivial and deserve their
  own design — "Enter just inserts a newline" is the number-one *"this isn't a real editor"*
  tell. Slice the rewrite primitive-first (serializer, offset model, DOM-surgery module
  shipped and unit-tested standalone) so there's no regression window.

## 3. The token system — measured, single-source, and live

Make design tokens the **single source of truth**, and verify them against measurement, not
claims.

- **Measure contrast; never trust a stated ratio.** An identity/accent colour can quietly fail
  its accessibility target (a brand accent that's only ~2.8:1 on the background fails AA), so
  the canonical primary button and the focus ring may need a *different* token than the
  identity colour — **the focus token is not the identity token.**
- **Status is never conveyed by colour alone** (it must also carry a glyph/label/position),
  regardless of contrast. No saturated status fills — "the screen is not a traffic light."
- **Hierarchy comes from weight and colour before size**; very large/heavy type is the amateur
  tell. A boundary (a hairline, a common region) groups *more strongly* than whitespace — but
  reach for space first and a hairline only when space can't carry it. Keep spacing on a fixed
  ramp (off-scale 5/7/13px values are the most common amateur tell).
- **Borders carry separation; shadow is the rare exception** — keep essentially one shadow
  token for genuinely floating surfaces. Without a single token source, every value silently
  forks (dozens of ad-hoc shadow literals at several different blurs, decorative dividers
  everywhere, emoji used as UI).
- **Never set colour via inline style on an interactive element** — inline styles beat
  `hover:`/`focus:` utility specificity, so you ship a "hover me" control that doesn't.
- Keep a **live styleguide that renders from the product's real tokens** (and can run with the
  full stack down) so the design reference can never drift from the app.
- **Agents look like agents, not magic** — no sparkle/shimmer/magic-wand "AI" iconography; no
  emoji as UI (it can't inherit `currentColor` or be re-themed).
- Tag each rule as **proven** (cite the research/standard) vs **house style** (taste), so the
  line between "accessibility requires this" and "we prefer this" stays honest.

## 4. Density made calm — the interaction philosophy

The product is **dense, calm, utilitarian**: it shows a lot without shouting. Specify these
once and apply them everywhere:

- **One shell everywhere** (a consistent rail + contextual secondary nav + header) so muscle
  memory transfers and the platform feels like *one* thing, not five bolted together.
- **A command palette over the universal graph** is the highest-leverage friendliness feature —
  one keystroke to jump to any entity or run any action. It can reach anything precisely
  *because the reference graph is universal* (substrate §7): the graph that powers automation
  powers navigation.
- **The system assembles context; the user never does.** Wherever the graph links two things,
  show the link and pre-fetch the context (a failing check → the failing step → the line of
  code; a notification → *why* it fired). The user is never sent to another tab to assemble
  what the system already knows is related.
- **Optimistic updates, honest rollback** ("optimism for latency, honesty on failure").
  **Reversibility over confirmation** — prefer an undo window and restorable history to
  "are you sure?" dialogs. **Real-time as the default** on any surface that can be live, over a
  reconnect-safe transport so liveness is *trustworthy*.
- **Empty / loading / error are first-class designed states**, not afterthoughts: empty
  explains and offers the create action; loading shows *structure* (skeletons that match the
  final layout), never a spinner on a blank page; error blames the system in one quiet line
  and offers a path; a degraded surface fails *static* ("temporarily unavailable" for that
  surface only). Latency budgets are hard, not stylistic: keyboard-driven interactions respond
  in under ~100ms; suppress flash-of-spinner under ~1s; "pages render, they don't animate in."

## 5. Layout containment and mobile — the specific bugs

- **Pin the shell to the viewport** (`height: 100vh; overflow: hidden`) and make each internal
  region its own scroller. A shell rooted on `min-height` lets the *whole page* grow, pushing
  the composer/input below the fold. The load-bearing detail: a flex child that should scroll
  needs `min-height: 0` (and `overscroll-contain`) or it won't shrink below its content and the
  overflow leaks up the tree.
- **`width: 100%` is not a takeover.** Making a mobile panel full-width leaves it an in-flow
  child laid out *beside* the still-present main column — clipped off-screen, its controls
  unreachable. Collapse the other column (e.g. hide it at the breakpoint) so the panel actually
  fills the viewport.
- **Hover is not touch-reachable.** Any action that only appears on hover is invisible on a
  phone — surface row actions by default or behind an explicit mobile affordance.
- **Flip popovers when they'd go off-screen.** A picker anchored strictly *below* a
  bottom-pinned composer renders off the bottom of the screen — generalise vertical placement
  to flip above with a max-height when there's no room below. (And test it against the *real*
  anchor; a picker that already flips in one place hides the bug everywhere else.)
- The mobile drawer pattern (rail/secondary-nav become toggled overlays with backdrop-click +
  Escape + route-change auto-close) is reliable — but name the fixed-width assumptions baked
  into the shell before you try to make it responsive.

## 6. Humanise machine strings at the source

Raw machine strings leaking into the UI (`"merge_request merged"`, raw ids, unrendered
markdown) are the most common "this feels unfinished" tell. **Humanise at the backend, paired
with a routable reference**, not with a frontend-only string map — so every consumer (and
every agent-authored message) gets the human-readable form for free.

## 7. Design-before-code, and the switch test

- For any surface, **design first**: information architecture, the key flows, and
  wireframes/mockups of the primary screens *including the empty/loading/error states* —
  reviewed before the UI is built. Where design-first actually happened, it paid off (reading
  the design doc is what reveals that "nine menus are three shapes"); where it didn't, every
  principle above was re-learned as a production bug.
- The done-bar is the **switch test, reached by driving the real UI in a browser**: a surface
  is finished only when a team could move to it without hitting a wall the old tool didn't
  have. A dedicated "does this feel finished?" pass routinely finds a dozen-plus issues a
  feature checklist misses. (See process doctrine §4 — this is the same "actually try it" rule
  applied to design.)
