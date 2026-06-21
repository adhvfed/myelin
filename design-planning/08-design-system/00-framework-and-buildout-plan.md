# Phase 8a — Design-System Framework & Buildout Plan

> **Phase:** `08-design-system` · deliverable **00** (framework + buildout plan). **File date: 2026-06-20.**
> **Status: the visual direction is RECOMMENDED, not chosen — the human decides.** This plan is built for
> the recommended finalist **A "Instrument"** *now*, but is **parameterised** so that picking **B / C / D /
> the §6 hybrid** is a **token-set swap + a flip of a small set of variant flags — never a component
> rewrite** (decision-brief §7). The parameterisation model (§2) is the load-bearing requirement of this
> file; everything else serves it.
>
> **Inputs:** [decision-brief](../07-judging/decision-brief.md) (recommend A 2.98, runner-up D 2.89,
> hybrid §6, cross-cutting gaps §7); [design-language](../../planning/02-holistic-architecture/design-language.md)
> §3 (token tiers), §5 (shared components), §8 (stack direction), §8b (day-one primitives + live styleguide);
> finalist A's [tokens.json](../06-design-sketches/6c-finalists/finalist-A-instrument/tokens.json) /
> [tokens.css](../06-design-sketches/6c-finalists/finalist-A-instrument/tokens.css) /
> [README](../06-design-sketches/6c-finalists/finalist-A-instrument/README.md); the interaction research
> [command-palette](../04-research/interaction/command-palette.md),
> [reference-unfurl](../04-research/interaction/reference-unfurl.md),
> [shared-patterns](../04-research/interaction/shared-patterns.md),
> [audit-method](../04-research/accessibility/audit-method.md),
> [motion-microinteractions](../04-research/visual/motion-microinteractions.md).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard / a measured artifact value / an
> existing architecture contract surfaced. **HOUSE STYLE** = our taste/synthesis.
> **`[DEFERRED-UNTIL-USERS]`** = a reception/comprehension hypothesis no expert pass can settle. Every
> "direction" choice carries the brief's `[DEFERRED-UNTIL-USERS]` flag: this panel is expert judgement, not
> user validation.
>
> **Scope guard:** this file decides the **stack** and writes the **buildout plan**. It does **NOT** build
> any component (that is **8b**). Nothing here is committed.

---

## 0. How to read this document

| § | What it gives you |
|---|---|
| §1 | **The framework / stack decision** — UI framework, accessible-primitive library, token pipeline, live styleguide, self-hosted assets. Confirms/refines design-language §8 with reasoning + current-tooling citations. |
| §2 | **The parameterisation model** — *the re-runnable structure.* The token set is the primary direction-parameter; a small named set of variant flags carries the rest. "Human picks D instead of A" = swap tokens + flip flags. **The key requirement of this phase.** |
| §3 | **The component inventory structure** — the catalogue 8b fills, in build SEQUENCE (overlay primitives → shared components → surfaces) and why. |
| §4 | **The cross-cutting gaps to implement** — the brief's §7 set, stated as required design-system capabilities. |
| §5 | **How it plugs into the build** — monorepo/ADR-01, the design-system package, generated types, design-before-code, what the implementation agent consumes from `08-design-system/`. |

---

## 1. The framework / stack decision

This **confirms design-language §8 with one refinement**: §8 said "a mature accessible component
ecosystem" without naming one; the panel flagged the keyboard/overlay layer as **aspirational** in 3 of 4
finalists (decision-brief §7.1), so this phase must **name the primitive library and treat it as a hard
dependency**, not a "we'll pick later." Each decision below is tagged and, where it makes a current-tooling
claim, cited.

### 1.1 UI framework — **TypeScript + React (function components + hooks)** — CONFIRM §8

- **Decision:** TS + React as the default for all web surfaces, with one shared component library + token
  package in the monorepo (ADR-01). *(CONFIRM design-language §8.1; PROVEN-as-contract — ADR-01/ADR-02 name
  TS/React-class as the expected baseline.)*
- **Refinement (HOUSE STYLE):** §8 wrote "React-class component framework," meaning "the React class of
  framework," **not** React class components. The build target is **function components + hooks** — every
  accessible-primitive library below ships hooks, not class APIs, and React class components are legacy.
  Read "React-class" everywhere in §8 as "React-family, function-component." *(This is a wording
  disambiguation, not a stack change.)*
- **Why React over a Rust/WASM UI framework:** the design-intensive, fastest-moving layer needs the deepest
  talent pool and the most battle-tested **accessible primitives** — Rust UI is still immature there
  (design-language §8.2). WASM-Rust is retained **at the edges where it earns it** (the `myelin-content` AST
  + sanitiser, the `myelin-query` AST parser/validator, diff rendering) so the *exact* Rust logic is shared
  client↔server, killing a class of drift bugs (design-language §8.1; ADR-05/ADR-07). *(CONFIRM §8.)*

### 1.2 Accessible-primitive library — **React Aria Components (Adobe)** — NEW, the §8 gap filled

This is the decision §8 deferred and the panel said matters most. Comparison on the four axes the prompt
names — accessibility maturity, headless-ness, self-host / sovereignty fit, RTL:

| Axis | **React Aria Components** (Adobe) | Radix Primitives | Ark UI / Zag-js |
|---|---|---|---|
| **Accessibility maturity** | **Deepest available**; the choice when WCAG compliance is a contractual constraint and you need the strictest ARIA-pattern implementation *(PROVEN-as-reported — LogRocket / greatfrontend 2026)* | Widely adopted, "accessible enough for the vast majority"; teams often add React Aria for the patterns Radix lacks (date pickers, disclosure) | State-machine (Zag/XState) core; solid, but the newest of the three |
| **Headless-ness** | Headless: behaviour + ARIA + interactions, **you own all styling** (more code per component, full control) | Headless, broadest primitive coverage; the base shadcn/Radix-stack builds on | Headless, **framework-agnostic** (React/Vue/Solid) via Zag-js |
| **Self-host / sovereignty fit** | **Pure npm logic, zero runtime network calls, no CDN/telemetry** — installs into the monorepo and ships from our own bundle. Aligns with no-CDN (§1.5). *(PROVEN — it is a logic library, not a hosted service.)* | Same (npm logic) | Same (npm logic) |
| **RTL support** | **First-class** — Adobe builds for i18n/localization as a core concern (locale-aware, RTL-aware interactions, internationalised dates/numbers) — the strongest fit for our G2 RTL mandate (design-language §4) *(HOUSE STYLE read of Adobe's i18n posture; the web sources don't rank RTL head-to-head — `[VERIFY]` against React Aria's i18n docs at build time)* | RTL works but is styling-owner's responsibility | RTL supported |

- **Decision (HOUSE STYLE, decisive): React Aria Components.** Myelin's market is **EU public-sector
  procurement where EN 301 549 / WCAG 2.1 AA is a *legal eligibility requirement*, not polish**
  (audit-method §1; design-language §4). When accessibility is a contractual constraint, the rigorous
  primitive is the correct default — and the panel's central worry (decision-brief §7.1, §7.4–§7.6) is
  exactly the keyboard/overlay/focus layer React Aria implements most strictly. Its first-class i18n/RTL
  posture is the second deciding factor for our G2 mandate. The cost — "more code per component" — is
  acceptable because we are building **one shared library once**, not per-feature.
- **Where Radix would win instead (honest):** if we were optimising for shipping speed with the broadest
  off-the-shelf primitive set and a shadcn-style ecosystem, Radix is the pragmatic default *(PROVEN-as-
  reported)*. We are not — we are optimising for the strictest auditable a11y on a legal floor. **Recorded
  so the choice is overrulable.**
- **The primitive library does NOT decide the look.** React Aria is headless — it provides *behaviour +
  ARIA + focus management*, and **our token layer (§2) decides every pixel**. This is what keeps the
  primitive choice orthogonal to the direction choice: swapping finalist A→D changes tokens, not the
  primitive.

### 1.3 Token pipeline — **DTCG source → Style Dictionary → CSS custom properties (+ platform outputs)** — CONFIRM §3.1 + name the tool

- **Source of truth:** the finalist's **`tokens.json` in W3C DTCG format** (`$type`/`$value`, three tiers
  primitive→semantic→component, light+dark) — finalist A already ships exactly this *(PROVEN — inspect
  [tokens.json](../06-design-sketches/6c-finalists/finalist-A-instrument/tokens.json); the focus token is
  already DERIVED and DISTINCT from the identity accent per §8b.3)*.
- **Build tool:** **Style Dictionary v4+** (DTCG-native). It compiles the DTCG source to the platform
  outputs: **CSS custom properties** (the primary runtime, exactly the `tokens.css` projection finalist A
  hand-wrote — see [tokens.css](../06-design-sketches/6c-finalists/finalist-A-instrument/tokens.css)), plus
  TS constants for the rare cases CSS vars can't reach, and (later) any native targets. *(PROVEN-as-reported
  — Style Dictionary v4 has first-class DTCG support; the DTCG spec reached its first stable version
  **2025.10**; full 2025.10 support in Style Dictionary is **in progress in v5**, so pin a working version
  and `[VERIFY]` the 2025.10 dimension-object handling at build time — sources §6.)*
- **Why DTCG + Style Dictionary, not hand-authored CSS:** §3.1's three-tier architecture is what makes
  dark/high-contrast/tenant-theming a **token-table swap, not a component rewrite** — and that mechanism is
  *the same mechanism* the direction-parameter (§2) rides. Generating CSS vars from one DTCG source means a
  token change is **one PR that updates every frontend** (design-language §8.3) and means the §2 direction
  swap is a **source-file swap**, not a sweep through components. *(CONFIRM §3.1; tool choice HOUSE STYLE.)*
- **Measured-not-claimed is a pipeline gate (PROVEN — §8b.3 / audit-method §3):** a CI step measures the
  contrast of every semantic text/bg and focus/UI pair against WCAG AA over the *generated* token table (a
  brand accent at ~2.8:1 fails AA). Finalist A's pairs are already measured and pass in both themes
  *(PROVEN — README contrast table)*; the gate keeps any future token edit / direction swap honest. The
  **focus token ≠ identity token** derivation rule is enforced here, not left to discipline.

### 1.4 Live styleguide — **a lightweight custom styleguide that renders from the REAL tokens, runnable with the stack down** — over Storybook, per §8b.6

- **Decision (HOUSE STYLE, §8b.6 is explicit):** build a **lightweight custom styleguide** that renders the
  actual components from the **same generated `tokens.css` the app ships**, and that **runs with the backend
  stack down** (static, no API). This is a §8b.6 day-one deliverable on the design-system package, whose
  whole point is *"the reference can't drift from the app."* *(PROVEN-as-mandate — §8b.6.)*
- **Why not Storybook (or: Storybook is allowed, but not as the source of truth):** Storybook is a fine
  *component workbench* and we may use it for that. But §8b.6's requirement is a styleguide that **renders
  from the product's real tokens and runs stack-down** — the failure mode it guards against is a styleguide
  that drifts because it mocks tokens or needs the app's runtime. A custom styleguide that imports the
  *generated* `tokens.css` and the *real* components guarantees zero drift by construction; Storybook adds a
  parallel config surface that can drift. **House call: custom styleguide is the canonical reference;
  Storybook optional as a dev workbench, never the source of truth.** *(HOUSE STYLE.)*
- **It is also the a11y proving ground:** the styleguide is where the seven hard components (§3) are
  exercised against the audit-method checklist (keyboard path + SR announcement per component) and where the
  **direction swap is demoed** — toggle the token set and the variant flags (§2) live and watch every
  component re-skin without a rebuild. That live toggle *is* the proof the parameterisation works.

### 1.5 Self-hosted assets / no-CDN — fonts + icons in-bundle — CONFIRM §3.3 / §8.1

- **Fonts:** self-hosted variable families, **no third-party font CDN** — a sovereignty/GDPR constraint
  (no personal data or request metadata leaving the cell to a font host; ADR-11/ADR-12; design-language
  §3.3). Finalist A's CDN fonts are explicitly **throwaway**; production self-hosts a variable family
  carrying **Latin-ext + Greek + Cyrillic + Arabic** (the EU-multilingual + RTL coverage gate, R-18) —
  coverage is a `[VERIFY]` selection gate at build *(PROVEN-as-constraint — README "Known gaps"; §3.3)*.
- **Icons:** **one self-hosted icon set**, consistent stroke/weight, stable icon→meaning mapping across
  subsystems (design-language §3.7), shipped as inline SVG (so glyphs **inherit `currentColor` and re-theme
  with the tokens** — §8b.3 bans emoji-as-UI for exactly this reason; agents get a **plain geometric mark,
  no sparkle/shimmer/magic-wand** iconography, §8b.3). No icon-font CDN. *(CONFIRM §3.7 / §8b.3.)*
- **No third-party scripts/analytics from a CDN** either — opt-in telemetry stays in-cell (ADR-12). *(CONFIRM §8.1.)*

### 1.6 The stack, in one block

```
DTCG tokens.json (per-direction; A now)                    ← the direction-parameter source (§2)
  └─ Style Dictionary v4+  ──►  tokens.css (CSS custom properties)  + TS constants
        consumed by ▼
  React + TypeScript (function components + hooks)
        behaviour/ARIA/focus from ▼
  React Aria Components (headless; styled entirely by our token layer)
        edges in ▼
  WASM-Rust (myelin-content AST+sanitiser · myelin-query AST · diff render)  [shared client↔server]
        proven in ▼
  Custom live styleguide (renders REAL tokens, runs stack-down) + measured-contrast & a11y CI gates
        self-hosted ▼
  Fonts (variable, Latin-ext/Greek/Cyrillic/Arabic) + icons (inline SVG, currentColor) — NO CDN
```

---

## 2. The parameterisation model (the re-runnable structure)

**This is the phase's key requirement.** The brief says picking B/C/D instead of A is "a *re-run* of the
same machine with a different input, not a rebuild" (decision-brief §7). This section defines exactly what
that **input** is, so the claim is mechanical, not aspirational.

### 2.1 The direction-parameter = the semantic token set (primary swap)

- **The primary direction-parameter is the DTCG semantic token set** — the Tier-2 alias table (surfaces,
  text, border, accent, focus, status, agent) and the typographic/spacing/radius/motion choices it points
  at. Components consume **only semantic tokens** (design-language §3.1; finalist A's components already
  bind only to `{semantic.*}` / CSS vars — PROVEN by `tokens.json` Tier-3 → Tier-2 references). **Therefore
  re-pointing the semantic table re-skins every component with no component edit.** *(PROVEN mechanism —
  §3.1 three-tier architecture; this is the same mechanism that already gives light/dark/high-contrast.)*
- **Concretely:** "A Instrument" = midnight base, one electric-blue accent, near-zero radius, hairlines,
  compact default, crisp/instant motion. "B Workshop" = cream/ink ramp (never #fff/#000), a serif on
  reading headings, ~66ch measure, comfortable default, gentle motion. "D Civic" = sober institutional
  blue, rigid grid, always-on sovereignty cues. **Each is a different semantic token set over the *same*
  primitive→semantic→component plumbing and the *same* components.**
- **What is NOT a token (the honest boundary):** a few finalists differ in *behaviour/layout defaults*, not
  just values — a serif confined to reading surfaces, an always-on sovereignty band, the default navigation
  paradigm. Those don't fit cleanly in a color/space token. They are the **variant flags** (§2.2). Keeping
  this set *small and named* is what stops the token swap from quietly becoming a rewrite.

### 2.2 The variant flags (the small, named set where finalists genuinely differ)

Six component-level flags. Each is a **prop/config value with a token-backed default**, read by the shell
and the shared components — **never branched in component logic per direction**. (The first four map
directly to the views component's already-specced config-delta model — shared-patterns §2.1 — proving the
mechanism exists.)

| Flag | Values | What it changes | A | B | C | D | Hybrid (§6) |
|---|---|---|---|---|---|---|---|
| **`density`** | `comfortable` ↔ `compact` | default spacing/row-height token set (§3.4/P5); per-surface override stays | compact | comfortable | medium | medium | compact base, comfortable PM lens |
| **`nav`** | `rail` ↔ `palette-led` ↔ `contextual` | which navigation surface is primary (shell §5.1 vs palette §5.2 emphasis) — **all three exist always; this sets emphasis/default, not presence** | palette-led | contextual | rail (+⌘K) | rail | palette-led spine |
| **`surfaceUnification`** | `one-skin` ↔ `distinct-per-surface` | whether per-surface distinctness (projection + measure + serif-on-reading-headings) is on; **bounded — chrome/chip/identity/palette/editor stay invariant either way** | one-skin | distinct | distinct | one-skin | one-skin + serif on reading only |
| **`tone`** | `utilitarian` ↔ `warm` ↔ `sober` | reading-surface type config (serif/measure/line-height) + empty-state voice; **token-and-copy, not chrome** | utilitarian | warm | utilitarian | sober | warm where it reads |
| **`agentPresence`** | `ambient` ↔ `foregrounded` | default verbosity/placement of the agent/HITL surfaces (inbox-row vs inline rendered diff) — **the HITL component is the same; this sets default surfacing** | ambient | ambient | foregrounded | ambient | foregrounded contract treatments |
| **`sovereigntyVisibility`** | `on-demand` ↔ `always-on` | whether the residency/lawful-basis band is always rendered near data vs summoned; the DSR console is present either way | on-demand | on-demand | on-demand | always-on | always-on |

- **Rule (HOUSE STYLE, the falsifiable test):** a component **must not contain a `switch (direction)`**.
  It reads `density`/`tone`/etc. flags and semantic tokens. If a 8b component branches on the *finalist
  name*, it has fractured the parameterisation — that is the review failure to catch.

### 2.3 "Human picks D instead of A" — the worked swap (NO component rewrite)

1. **Swap the token source:** point the build at `finalist-D-civic/tokens.json` instead of
   `finalist-A-instrument/tokens.json`. Style Dictionary regenerates `tokens.css`. Every component re-skins
   (sober institutional blue, rigid grid). The measured-contrast gate (§1.3) re-validates D's pairs.
2. **Flip the flags:** `density: medium`, `nav: rail`, `surfaceUnification: one-skin`, `tone: sober`,
   `agentPresence: ambient`, `sovereigntyVisibility: always-on` (the D row of the §2.2 table).
3. **`always-on` lights up the sovereignty band** that was built once and gated behind the flag — D's
   defining D9=4 surface (decision-brief §5) becomes default-visible.
4. **Done.** No component code changed. The styleguide's live toggle (§1.4) demonstrates the before/after.
   *(This is the decision-brief §7 claim made mechanical: B/C/D = re-run, not rebuild.)*

> **The hybrid (decision-brief §6) is reachable the same way:** A's token base + `surfaceUnification:
> one-skin` + `tone: warm` (serif on reading surfaces only) + `agentPresence: foregrounded` +
> `sovereigntyVisibility: always-on`. It is **a flag combination over A's chassis**, not a fifth design —
> which is exactly why §6 calls it "the discipline, not a stitch-up." The parameterisation model is what
> makes the hybrid free.

### 2.4 Honesty on the parameterisation

- **`[DEFERRED-UNTIL-USERS]`:** the *direction itself* is recommended-not-chosen; this model exists so the
  choice stays cheap to defer and cheap to overrule. The flags encode the **axes the finalists actually
  differed on** (decision-brief §2), not a speculative theming framework — keep it that small.
- **The bounded-distinctness bet (HOUSE STYLE):** `surfaceUnification` and `tone` only touch
  *projection + measure + serif-on-reading-headings*; chip/identity/palette/editor/shell chrome are
  invariant across all directions (the visual-craft lens found "no fork in the set" — decision-brief §6).
  If a future direction needs to vary the *chrome*, that is a new flag to add deliberately, not a silent
  per-component fork.

---

## 3. The component inventory structure (the catalogue 8b fills)

8b specifies each component below with **ALL states** (empty / loading / error / permission-denied /
erased-tombstone / agent-pending — design-language §5.10) and **usage rules**. This file fixes the
**catalogue** (mapped to design-language §5 + the shared-patterns research) and the **build SEQUENCE**.

### 3.1 The buildout SEQUENCE (and why)

```
TIER 0 — Foundations        tokens (DTCG→SD→css) · type · icons · the live styleguide · a11y/contrast CI gates
TIER 1 — Overlay primitives  Dialog · Confirm · Popover · Dropdown/Menu · Tooltip · Toast      ← BUILD FIRST
TIER 2 — Shared components    nav shell · command palette · reference chip/unfurl · agent/HITL card ·
                              comments/mentions · views (table/board/…) · block editor ·
                              notifications inbox · identity/agent badge
TIER 3 — Surfaces            the design-language §7 view catalogue (per subsystem), composed from Tiers 1–2
```

- **Overlay primitives FIRST (§8b.1 — the explicit sequencing mandate).** They are *"the most expensive UX
  retrofit"* and must be *"built before any feature consumes them"* (§8b.1; shared-patterns §5). The
  focus-trap + return-focus + scroll-lock + Escape/backdrop + portal-to-root + one z-index scale + correct
  ARIA all live **once** in this substrate and are inherited free — this is where most of G1's overlay
  obligations discharge (shared-patterns §5.5; audit-method §5). Build a feature on a missing overlay
  substrate and you re-implement (and break) focus management per feature. *(PROVEN sequencing — §8b.1.)*
- **Then shared components**, because each consumes the overlays: the **command palette** is a modal overlay
  with a combobox (needs focus-trap + portal); the **unfurl hovercard** is a Popover; the **HITL card** can
  overlay; **row-action menus** are Dropdowns; **toasts** carry optimistic-settle/undo. Building these
  before the substrate forces the retrofit §8b.1 forbids.
- **Then surfaces**, which are compositions — no surface introduces a new primitive; if it needs one, it is
  contributed *down* into Tier 1/2 (design-language §8.3 rule 1: no subsystem ships its own design system).
- **The reference chip + unfurl is the connective tissue** (design-language §5.3 / §P6; reference-unfurl
  §1): it appears inside *all four* big organisms (board cell, editor mention, inbox subject, dialog) — so
  it is specified early in Tier 2 and the same chip must render identically everywhere (shared-patterns §1,
  the D4 reviewer test). *(PROVEN reuse invariant.)*

### 3.2 The Tier-1 overlay primitives (8b spec target)

Six, split **single-purpose by shape** (§8b.1 rule 4): **Dialog** (viewport-centred modal, trap+return),
**Confirm** (`alertdialog`, default-focus the *safe* action, reserved for irreversible/GDPR/HITL),
**Popover** (anchored, non-modal, flips off-screen), **Dropdown/Menu** (inline-flow, roving), **Tooltip**
(never takes focus, shows on hover *and* focus), **Toast** (never steals focus, AT via live region, hosts
undo). All portal-to-root; one z-index token scale `chrome < popover < modal < toast`. *(All PROVEN against
ARIA-APG modal-dialog; full spec + state set is shared-patterns §5 — 8b renders it.)*

### 3.3 The Tier-2 shared components (8b spec target — mapped to design-language §5)

| Component | design-language § | Research spec it renders | The states/rules 8b must fix |
|---|---|---|---|
| **Navigation shell** | §5.1 | platform-ia; shell owns layout grid, breakpoints, palette trigger, search, inbox entry, identity menu, residency cue | responsive drawer pattern (§8b.4), pin-to-viewport + `min-height:0` scroller rule |
| **Command palette** | §5.2 | [command-palette](../04-research/interaction/command-palette.md) | the §4 capability set: ⌘K→type→fuzzy→run, focus-trap/Esc, roving combobox via `aria-activedescendant`, all 9 states |
| **Reference chip + unfurl** | §5.3 (the wedge) | [reference-unfurl](../04-research/interaction/reference-unfurl.md) | live-not-snapshot, permission-aware (no title leak), tombstone, hovercard WCAG 1.4.13, inline actions |
| **Agent / HITL card** | §5.4 / §6 | legibility-and-hitl | plan-then-apply, per-effect target chips, Approve/Edit/Reject, attribution+audit; `agentPresence` flag sets default surfacing |
| **Comments / threads / mentions / reactions** | §5.5 | shared content model | one thread model, `@`/`#` as chips, review batching, anchored comments |
| **Views (table/board/calendar/list/gallery/timeline)** | §5.6 | [shared-patterns](../04-research/interaction/shared-patterns.md) §2 | six projections of one query AST, roving-tabindex grid + keyboard-drag equivalent, inline-edit, all states; the `density`/`tone`/vocab/fields config-deltas (§2.2) |
| **Block editor** | §5.9 | shared-patterns §3 | one render path (`render(parse(md))===md` gate), controlled `contenteditable`, slash menu, mention/ref structured nodes, IME |
| **Notifications inbox** | §5.8 | shared-patterns §4 | one store / filtered views, "why it fired" provenance, deterministic ranking, dedup, storm state, agent-volume-out-of-stream |
| **Identity / agent badge** | §5.11 | reference-unfurl (person/agent card) | one badge per `Principal`; the agent treatment (four-channel, never colour-alone, plain mark) |

- **The views component is doubly load-bearing:** it is both the **issues↔knowledge reuse boundary** and
  the **dual-audience mechanism** (shared-patterns §2). It is where the `density`/`tone` flags (§2.2) do
  their most visible work — the engineer board and PM roadmap are the *same component*, four config values
  apart. *(PROVEN mechanism; the "two components" fork is the trap to catch — shared-patterns §2.5.)*

---

## 4. The cross-cutting gaps to implement (required design-system capabilities)

From decision-brief §7 — set-wide gaps the panel found **aspirational in most finalists**. Stated here as
**capabilities the design system MUST ship**, so they stop being gestures. Each binds to a Tier-1/Tier-2
component above and to the audit-method checklist.

1. **The command palette as a REAL primitive (the top gap — non-functional in 3/4, un-typable in the 4th).**
   *Required capability:* `⌘K` opens → **real text input** (not a fake span — A's input was a span,
   decision-brief §7.1) → **fuzzy-filter live** → **run** the active row; **focus-trap while open + Esc to
   close with return-focus**; a **roving listbox via `aria-activedescendant`** (DOM focus stays on the
   input, APG combobox pattern); plus the advertised **j/k / ]/[ handlers** as system bindings.
   *(PROVEN-required — command-palette §3/§11; binds to Tier-2 palette on the Tier-1 modal-overlay
   substrate. This is the wedge; it is non-negotiable.)*
2. **Optimistic-update + honest rollback (designed by NOBODY in the finalists — write-time perf is
   unproven).** *Required capability:* a shared **optimistic-action primitive** — apply immediately, show a
   subtle pending affordance, **settle on server-ack or roll back honestly** with an undo-toast on success
   and a clear, non-blaming revert + retry on failure (the typed content / prior state is never lost).
   Used by board drag, inline-edit, palette Act verbs, editor save. *(PROVEN-required — decision-brief §7.2;
   shared-patterns §2.2/§3.3; motion-microinteractions owns the settle motion. HOUSE STYLE for the exact
   affordance.)*
3. **`forced-colors` / high-contrast support (missing in B, C, D; only A ships it — EN 301 549 expects
   it).** *Required capability:* a **`forced-colors` fallback is a token-layer default**, validated in the
   styleguide, inherited by every component — not a per-component afterthought. *(PROVEN-required —
   decision-brief §7.4; audit-method; finalist A already proves it — README G1 evidence.)*
4. **`aria-busy` + live-regions on skeletons (missing on D's and C's loading skeletons — SR users get no
   load signal).** *Required capability:* every **skeleton/loading state sets `aria-busy` and announces via
   one polite live region** (debounced, never per-keystroke spam) — a **system default on the loading
   state**, not opt-in. *(PROVEN-required — decision-brief §7.5; audit-method §6; command-palette §8
   loading row; finalist A already ships `aria-busy` skeletons.)*

> Plus the **focus-token-covers-the-autofocused-palette-control** rule (decision-brief §7.6 — C's
> `outline:none` on the palette input with no replacement was the closest-to-Blocker finding): the one
> derived `focus-ring` token covers **every** interactive element including the autofocused palette input.
> *(PROVEN — §8b.3; finalist A's derived focus token is already AA-safe and distinct from identity.)*

These four (+ the focus rule) are the **acceptance bar that turns the finalists' aspirational keyboard/
overlay layer into a real one** — they are why the primitive library (§1.2) was named as a hard dependency.

---

## 5. How it plugs into the build

### 5.1 Where it lives (monorepo / ADR-01)

- **One `design-system` package in the monorepo** (ADR-01) holds: the DTCG token source(s), the Style
  Dictionary config, the generated `tokens.css` + TS constants, the Tier-1 overlay primitives, the Tier-2
  shared components, the self-hosted fonts/icons, and the live styleguide. **No subsystem ships its own
  design system** (design-language §8.3 rule 1); new components are contributed *to* this package, reviewed
  against design-language §1–§6. *(PROVEN-as-contract — ADR-01 / §8.3.)*
- **Versioned in lockstep with the backend contracts** (ADR-01): a token change or component change is one
  PR that updates every frontend; the package and the wire contracts live in the same workspace so neither
  drifts. *(CONFIRM §8.1.)*

### 5.2 Generated types (the no-drift seam)

- The frontend consumes **type-safe, generated API/types** from the platform wire contracts — the envelope,
  `ArtifactRef`, the query AST, `ToolDef` (design-language §8.1; ADR-01/ADR-02). This is what lets the chip
  render any `ArtifactRef`, the palette compose the same query AST as saved views and agent triggers
  (command-palette §4), and the palette's Act verbs *be* the agent `ToolDef`s (command-palette §7) — **one
  catalogue, humans and agents**. The design-system components are typed against these generated types so a
  contract change is a **compile error, not a runtime drift**. *(PROVEN-as-contract — §8.1.)*
- **WASM-Rust shares the AST logic** client↔server (myelin-content / myelin-query / diff) so the editor's
  parse and the query parser behave identically on both sides (design-language §8.1; ADR-05/ADR-07).

### 5.3 The design-before-code rule (how 8b and the impl track consume this)

- **VISION §3 / §5.2: no frontend code without a design sketch behind it.** This phase (8a) fixes the stack
  + the parameterisation + the buildout sequence. **8b** fills the component catalogue (§3) — each
  component specified with all states + usage rules **before** it is implemented. The **implementation
  track** then builds Tier 0→1→2→3 in the §3.1 sequence, against 8b's specs, on this stack.
- **What the implementation agent gets from `design-planning/08-design-system/`:** (1) the **stack** to use
  (§1 — TS+React, React Aria, Style Dictionary, custom styleguide, self-host) without re-litigating it;
  (2) the **parameterisation contract** (§2 — consume semantic tokens + the six flags, never branch on
  direction name) so the build survives a direction change; (3) the **build order** (§3.1 — overlays first)
  so it doesn't hit the §8b.1 retrofit; (4) the **cross-cutting capabilities** (§4) as a non-negotiable
  acceptance bar; (5) from 8b, the **per-component state-complete specs** to implement against.

### 5.4 The done-bar (carried from §8b.7)

A surface is **done only when, by driving the REAL UI in a browser, a team could move to it without hitting
a wall the old tool didn't have** (§8b.7, the switch test — rubric D10, which every static finalist scored
2 on because none clears it without working software). The optimistic-rollback + real palette (§4) are
prerequisites to *ever* clearing it. *(PROVEN-as-mandate — §8b.7.)*

---

## 6. Sources (current-tooling claims, web-verified 2026-06-20)

- Headless UI comparison (React Aria deepest a11y / contractual-WCAG choice; Radix broad+pragmatic default;
  Ark UI state-machine + framework-agnostic): LogRocket "Headless UI alternatives", greatfrontend "Top
  Headless UI libraries for React in 2026", PkgPulse "React Aria vs Radix Primitives 2026".
  <https://blog.logrocket.com/headless-ui-alternatives/> ·
  <https://www.greatfrontend.com/blog/top-headless-ui-libraries-for-react-in-2026> ·
  <https://www.pkgpulse.com/guides/react-aria-vs-radix-primitives-2026>
- DTCG spec first stable version **2025.10** (2025-10-28); Style Dictionary v4 first-class DTCG support,
  full 2025.10 support in-progress in v5: Style Dictionary DTCG info, DTCG community-group, designtokens.org
  format drafts. <https://styledictionary.com/info/dtcg/> ·
  <https://www.designtokens.org/tr/drafts/format/> · <https://github.com/style-dictionary/style-dictionary/releases>
- (Internal, PROVEN-as-contract) design-language §3/§5/§8/§8b; decision-brief §6/§7; the R-08/R-09/R-10/
  R-12/R-17 interaction & a11y corpus; finalist-A tokens.json/tokens.css/README; ADR-01/02/03/05/07/08/
  11/12/13.

**`[VERIFY]` before build:** React Aria RTL head-to-head depth (against its i18n docs); Style Dictionary's
2025.10 dimension-object handling (pin a working version); the self-hosted variable font's
Latin-ext/Greek/Cyrillic/Arabic coverage (R-18 selection gate).

---

*End of Phase 8a. The visual direction is **recommended (A "Instrument"), not chosen** — the human decides;
B/C/D/the hybrid are a token-set swap + a flag flip (§2), not a rebuild. Stack: TS+React (function
components) · React Aria Components · DTCG→Style Dictionary→CSS custom properties · custom live styleguide ·
self-hosted fonts+icons, no CDN. Build order: foundations → overlay primitives → shared components →
surfaces. Not committed. Components are 8b's job, not this file's.*
