# Frontend Component Implementation Patterns (the hard parts, solved)

Date: 2026-06-26. Status: implementation guidance (the behavioural approach the UI prompts follow).

The design manual (`design-planning/08-design-system/`) owns the **look** (direction A "Instrument" tokens) and
the **a11y spec** (WCAG/APG). This doc owns the **behavioural approach** for the load-bearing components — the
patterns that are expensive and risky to get right, captured so each UI prompt starts from a known-good design.
These are a strong **starting point**, not finished code: implement each fresh against the tokens and **harden the
named edges** (especially §3) with real-input tests.

---

## 0. Dependency stance: minimal

`solid-js` + SolidStart (+ `@solidjs/router`, `@solidjs/meta`) + the `myelin-content` parse/serialize layer
(WASM, the canonical markdown AST with the `render(parse(md))===md` gate). **Hand-build the primitives.** Rationale:
stability, full control, no a11y-library coverage gaps, small bundles, no churn — and it is a proven, reachable
approach. A headless a11y library (Kobalte), an editor framework (ProseMirror), or a query library (TanStack) are
**options of last resort for a specific component that proves intractable**, not the default. The a11y *bar* is
non-negotiable; we meet it ourselves and gate it with axe + keyboard tests, not by importing it.

---

## 1. Overlay primitives (hand-built; the mechanics are non-negotiable)

Every modal-class surface is built on one `Dialog` primitive that owns, *correctly*:
- **Portal to `document.body`** — escapes transformed/clipped ancestors. (A `position:fixed` panel's containing
  block becomes *any* ancestor carrying a `transform`; the app shell's nav aside has one, so a non-portaled panel
  mis-anchors. This bug is subtle and load-bearing.)
- **Body scroll-lock with scrollbar-width compensation** (so the page doesn't jolt sideways on open).
- **A real focus trap** — focus moves in on open, Tab/Shift+Tab wrap inside the panel, focus returns to the
  trigger on close.
- **Escape + backdrop dismiss**, both individually disableable; the Escape handler `stopPropagation`s so a modal
  opened from inside a panel doesn't also collapse the panel.
- **`role="dialog"` + `aria-modal` + `aria-labelledby`/`aria-describedby`.**
- **A custom-header slot** — so the command palette is just `Dialog` + a search-input header, not a new overlay.

The six primitives, built once and inherited everywhere: **Dialog · ConfirmDialog** (`alertdialog`, default focus
on the *safe* action, reserved for irreversible/GDPR/HITL) **· Popover** (anchored, non-modal) **· Dropdown/Menu**
(roving) **· Tooltip** (never takes focus; hover *and* focus) **· Toast** (never steals focus; AT via live region;
hosts undo). One z-index token scale `chrome < popover < modal < toast`.

---

## 2. Popover positioning (one shared viewport clamp)

A single helper clamps every caret/anchor-positioned float (mention picker, slash menu, reaction picker,
block-handle menu): pull the **right** edge in to keep a small gutter; on a too-narrow viewport collapse `left` to
0 and trim `max-width` (the highlighted top-left row stays reachable on a phone/full-width composer). One source of
truth — the four floats anchor identically, so the clamp lives once, never copy-pasted (and drifting).

---

## 3. The block editor (per-block contenteditable — the key architectural decision)

**Each block is its own small `contenteditable` surface, not one document-level `contenteditable`.** This is the
decision that makes a custom block editor tractable: it contains the contenteditable minefield (IME, paste,
selection, undo) *per block*, and makes each block independently editable and independently persisted.

- **Edit lifecycle:** click to focus; debounced PATCH (~500 ms) on input pause; immediate PATCH on blur.
- **Inline formatting:** a `parseInline` grammar (bold/italic/code/links) renders inline runs; canonical
  parse/serialize is `myelin-content` (WASM), gated by `render(parse(md))===md`.
- **Kind conversion — ONE path:** markdown shortcuts (`# `, `## `, `- `, `> `, `[] `, `[x] `, ```` ``` ````, …)
  **and** the slash menu both route through a single `onSelectKind`. Never two conversion paths.
- **Mention / reference:** trigger chars (`@`, `#`, `~`, `$doc/`) open a floating picker keyed on the chars typed
  after; ↑↓ navigate, Enter/Tab insert the canonical short-form, Esc close. The items live in the **parent**
  component so keyboard intercepts can call `onSelect` directly with the current row.
- **Structural edits:** Enter splits a block (caret → start of the moved text); Backspace at offset 0 merges into
  the previous block (caret at the join point).
- **Ordering:** `sort_order` as an `f64`; insert = average of the two neighbours; a server `reorder_batch`
  renormalizes if float precision ever runs out (don't pre-optimize — averaging is fine for v1).
- **Paste:** a markdown-document → block-seeds parser (line-level: headings / bullets / todos / quotes / fenced
  code w/ language / images / dividers / GFM tables / paragraphs; blank lines separate; soft-wrapped lines join)
  so a pasted doc becomes real blocks, not one literal paragraph. It is the inverse of the markdown export
  serializer — keep them in lockstep.

**NAMED HARDENING (the "good start, not fully hardened" edges):** IME composition, rich/HTML paste, multi-block
selection, and undo/redo *across* blocks are where a per-block custom editor is hardest. Treat each as an explicit
hardening task with real-input tests. This is the one component where, if the edges prove intractable, wrapping a
ProseMirror instance *per block* is the named fallback — still per-block, not a document-wide rewrite.

---

## 4. Command palette (an action engine, not search-only)

- A `Dialog` with a search-input header; ⌘K from anywhere in the shell.
- Runs a **debounced search AND a command registry in parallel**; matched **actions** render above search results;
  the combined list is **one** ↑↓/Enter keyboard surface.
- Actions execute **in-place** (navigate, quick-create, set status/priority/assignee on the focused-or-selected
  items) without a full navigation.
- **Context contribution without coupling:** the on-screen surface publishes its actionable context (its selection
  + an imperative bulk `apply`) to a tiny global store; the palette reads it; when no relevant surface is mounted,
  those actions simply don't appear. This is how "context-aware commands" stay decoupled from the surfaces.

---

## 5. Data layer (SolidStart-native + a server-side cookie-auth gateway client)

- **One server-side gateway client** handles every backend call. It runs **only** server-side (server functions /
  route loaders / API routes / middleware), reads the session from an **httpOnly cookie**, adds the Bearer token,
  and on 401 does a **single refresh round-trip + one retry**, else throws an `Unauthorized` error the loader turns
  into a `/login` redirect. **Tokens never reach client JS** — this is the SSR auth simplification, and it is the
  reason SSR earns its keep even though the app is otherwise a client SPA.
- **Typed errors:** an `Unauthorized` error and a `GatewayError` that extracts the server's `{error:{message}}`
  envelope (so toasts read like the API author wrote them) while preserving status + raw body.
- **Data fetching:** SolidStart's native loaders/actions + `createResource`; the router's data layer handles
  cache/dedupe. No third-party query library required.

---

## 6. Real-time (SSE, server-proxied)

- The client subscribes via **`EventSource` (SSE)** — simpler than a client WebSocket, and proxy-friendly.
- A **SolidStart API route proxies the backend stream to the client as SSE** (an HTTP gateway can't proxy WS
  upgrades), so the backend can speak WS/stream while the browser sees SSE. Used for notification arrivals,
  presence, read-state, and agent-run traces.

---

## 7. The app shell (a slot-based layout frame)

Header (brand + global search + actor/identity menu + unread badge) + a fixed icon rail + a **secondary-nav slot**
+ a fluid **main slot**. Pages pass their `secondaryNav` and content as slots; the shell owns the chrome — the
palette trigger, global keyboard shortcuts, quick-create, the help launcher, the residency cue. Responsive: the
secondary nav becomes a drawer at the mobile breakpoint; the main slot is the `min-height:0` scroll container.

---

## How this folds into the plan

- **E0.7 stack** is revised by these patterns (see `08-frontend-foundation.md` §3): minimal deps, hand-built
  primitives, per-block editor, SolidStart-native data, SSE.
- The **"Solid patterns for agents" guide** (E0.7 deliverable 1) carries §0 + the Solid reactivity rules; **this
  doc** carries the per-component behavioural approach. Together they are what every UI prompt reads first.
- These approaches lower the risk on the components the spine/Git track flagged as hard (overlays, command
  palette, block editor) — they have a known-good design; the work is implementing it cleanly against the new
  tokens and hardening the §3 edges.
