# Frontend Foundation (E0.7) — thorough plan

Date: 2026-06-26. Status: PLAN (the settled E0.7 decision + the full stack + the build plan). This gates every
subsystem UI; Phase 2 sizes the UI prompts against it.

---

## 1. The decision (settled, 2026-06-26)

**Client SPA in SolidJS, wrapped by Tauri 2 for desktop + mobile (sharing a Rust core with Myelin), with
SolidStart providing optional SSR for the web target's auth/first-paint.**

Why (recap):
- **Multi-platform (web + mobile + desktop) requires a shippable *client* app**, which rules out git's
  server-rendered-Rust→HTML as the foundation (you can't wrap a server-render into a native app). Server
  rendering becomes an optional layer (SolidStart) for the web target's auth/first-paint, not the base.
- **Tauri 2 is the standout shell** because Myelin is Rust: the desktop/mobile apps share a **Rust core** with
  Myelin's own crates, and Tauri 2 covers desktop **and** iOS/Android from one web frontend — which also
  neutralizes "need React Native for mobile." Lightweight, on-thesis, agent-operable.
- **Solid over React** for this project: stability (fixes the founder's React/Next churn fatigue), fine-grained
  reactivity with fewer subtle bugs (the failure class this whole project fights), smaller/faster bundles (matters
  on mobile/desktop). The two cited React advantages evaporate: **TanStack Query has first-class Solid support**
  (`@tanstack/solid-query`), and **mobile is handled by Tauri**, not React Native. React's residual edge is
  agent training-data fluency — real but double-sided (more data, but contradictory/churned), and mitigable
  (§5). React remains the safe conventional fallback if Solid's ecosystem bites.

---

## 2. Design-manual reconciliation (reuse vs. re-target)

The design manual (`design-planning/08-design-system/`, direction **A "Instrument"**, user-confirmed) is the
binding visual + UX authority and is **almost entirely preserved.** The key fact: **only Tier 0 was built; Tiers
1–3 are specs; the React + React-Aria stack was decided, never coded.**

**REUSE AS-IS (framework-agnostic — the bulk of the manual's value):**
- Tier 0 foundations, *shipped*: the DTCG → Style Dictionary → `tokens.css`/`tokens.json` pipeline, the type
  scale, the strok icon sprite (`04-icons/dist/sprite.svg` + manifest), the live styleguide, the contrast/a11y CI
  gates.
- All **component specs** (`02-components/*.md`): anatomy, ALL states (empty/loading/error/permission-denied/
  erased-tombstone/agent-pending), keyboard + ARIA model, usage do/don'ts — these are behaviour/standard specs,
  not implementation.
- The **principles** (P1–P9), the **accessibility bar** (WCAG 2.2 AA / WAI-ARIA APG / EN 301 549 / AI Act — all
  tagged PROVEN, non-negotiable), the **patterns** (agent-native contract, dual-audience lenses, unglamorous
  states, perceived-performance, sovereignty-as-UX), direction **A "Instrument"**, and the **parameterization
  model** (a different direction = token-set swap + variant-flag flip, never a rewrite).
- The **`myelin-content` WASM render path** (`render(parse(md))===md` round-trip) — the shared render for the
  block editor, comments, chat, and humanised notifications ("one render path, four subsystems").

**RE-TARGET (an unbuilt decision, no code discarded):**
- **React → SolidJS.**
- **React-Aria → a Solid headless a11y-primitive library (the hard dependency).** Each spec's named "React Aria
  Components primitive" maps to its Solid equivalent. Recommended: **Kobalte** (`@kobalte/core`, WAI-ARIA-APG
  compliant — the Solid analog of Radix/React-Aria). Strong framework-agnostic alternative to evaluate: **Ark UI
  / Zag.js** (state-machine primitives shared across frameworks — the a11y logic lives in framework-agnostic
  machines). Fallback: **corvu**, or hand-built on WAI-ARIA APG for any missing primitive. **The a11y bar is
  PROVEN/binding; primitive coverage is the load-bearing validation of §9.**

---

## 3. The stack, layer by layer

| Layer | Choice | Notes |
|---|---|---|
| UI framework | **SolidJS** | fine-grained reactivity, no VDOM |
| Web meta-framework | **SolidStart** | file routing + server functions; SSR optional (auth/first-paint) |
| Headless a11y primitives | **Kobalte** (eval Ark/Zag) | the React-Aria replacement; hard dependency |
| Tokens | **reuse** DTCG → Style Dictionary → `tokens.css` + generated TS types | the only tier components touch; parameterization preserved |
| Icons | **reuse** the strok sprite + a Solid `<Icon>` wrapper | self-hosted, no CDN |
| Server-state / data | **TanStack `@tanstack/solid-query`** | the founder's preferred data layer, Solid adapter |
| Client-state | **Solid signals + stores** | no Redux/Zustand needed |
| Routing | **`@solidjs/router`** / SolidStart routing | |
| Forms | **Kobalte form primitives** + a Solid form lib (`@tanstack/solid-form` or `modular-forms`) | |
| Styling | **CSS driven by `tokens.css` vars** | direction-A is hairlines/near-zero-radius/compact; token-only, framework-agnostic |
| Editor | **ProseMirror core (framework-agnostic) + Solid view; `myelin-content` WASM for parse/serialize; Yjs/Yrs for CRDT** | the hardest piece — §9 |
| Desktop + mobile | **Tauri 2** | one web frontend wrapped; Rust core shares Myelin crates; validate mobile early |
| Auth | **SolidStart server session (httpOnly cookie) for web; Tauri secure storage (Stronghold/keychain) for native** | tokens from the hardened identity service (E0.5) |

---

## 4. Tauri + Rust-core sharing (the on-thesis advantage)

- **One Rust core, three shells.** The web app (SolidStart) is the shared frontend; Tauri 2 wraps the *same*
  Solid app for desktop + iOS/Android. The Tauri Rust side exposes commands that **reuse Myelin crates** —
  `myelin-content` (render/sanitise), `myelin-client` (the resilient API client), offline cache, identity token
  handling. The native apps are not a separate JS island; they have a Rust heart reusing the substrate.
- **Validate Tauri-mobile early** (it matured more recently than Tauri-desktop). Named fallback for mobile if it
  bites: **Capacitor** (wraps the same web app).

---

## 5. Agent-buildability tooling (the mitigation for Solid's lower fluency — on-brand for this project)

Solid's one real weakness with agents is fluency/footguns; this project is unusually good at fixing exactly that
with docs + lints. Three deliverables:
1. **"Solid patterns for agents" guide** (a frontend analog of the design manual + the doctrine docs the agents
   already read): the reactivity rules (never destructure `props`; read `props.x` at use-site; `createMemo`/
   `createEffect`/`createResource` correctly; `<Show>`/`<For>`/`<Switch>` not ternaries/`.map`; stores for
   nested state), Kobalte usage patterns, the **spec → Solid-component mapping convention**, token/icon usage, and
   the testing pattern. Every UI prompt reads it first.
2. **A frontend lint gate** (the clippy-equivalent): ESLint + **`eslint-plugin-solid`** (catches the
   prop-destructuring / reactivity foot-guns) + `eslint-plugin-jsx-a11y` + project conventions; plus **axe-core**
   in the e2e. Red-on-violation, in CI.
3. **The fixed spec→component convention:** each `02-components/*.md` spec → one Solid component on Kobalte, ALL
   states implemented, tokens-only, with its real-browser test. No subsystem ships its own primitive.

---

## 6. The build sequence (the manual's §3.1, re-targeted to Solid)

- **Tier 0 — reuse + wire in.** Tokens/type/icons/styleguide/a11y+contrast CI gates into the Solid design-system
  package. (Already built; just consumed.)
- **Tier 1 — overlay primitives FIRST** (the most expensive UX retrofit; built before any feature consumes them):
  **Dialog, ConfirmDialog (`alertdialog`, safe-action default focus — irreversible/GDPR/HITL), Popover,
  Dropdown/Menu, Tooltip, Toast.** Focus-trap + return-focus + scroll-lock + Escape/backdrop + portal-to-root +
  one z-index token scale (`chrome < popover < modal < toast`) + correct ARIA, **once**, on Kobalte.
- **Tier 2 — shared components** (each consumes the overlays): **nav shell · command palette (⌘K combobox modal) ·
  reference chip + unfurl (the connective tissue — specify early; renders identically in board cell / editor
  mention / inbox subject / dialog; permission-aware, no title leak, tombstone) · agent/HITL card (plan-then-apply,
  per-effect chips, Approve/Edit/Reject, audit) · comments/threads/mentions · views (table/board/calendar/list/
  gallery/timeline — the doubly-load-bearing dual-audience component: engineer board = PM roadmap, four config
  values apart; DO NOT fork it) · block editor (§9) · notifications inbox · identity/agent badge.**
- **Tier 3 — surfaces** (per subsystem, composed from Tiers 1–2; no surface introduces a new primitive — it is
  contributed *down* into Tier 1/2).

---

## 7. The app shell + the per-subsystem UI pattern

- **The SolidStart app shell** (a spine deliverable, before any subsystem UI): nav shell + routing + auth session
  + the layout grid + the command-palette trigger + global search + the inbox entry + the identity menu + the
  residency cue. The responsive drawer pattern + pin-to-viewport scroller rules from the manual.
- **Per-subsystem UI** = SolidStart routes + the shared component library + the subsystem's **product API** (E0.6)
  + the **reused Rust view-models**. Concretely: git's `web.rs` view-model becomes a **JSON data/view-model
  source** the Solid UI renders — *not* the render path. The same pattern for every subsystem: the Rust side
  serves view-models/data through the product API; the Solid client renders them.

---

## 8. Testing (real-browser, replacing the gated rehearsal)

- **Real Playwright e2e** against the running SolidStart app — replaces the gated headless-chromium-`--dump-dom`
  rehearsal. **The "switch test" becomes a real browser-driven Playwright test**, not a Rust harness (this closes
  the UI-layer structural floor identified in the roadmap).
- **Accessibility:** axe-core in the e2e + the manual's contrast/a11y CI gates (reused). WCAG 2.2 AA / EN 301 549.
- **The block-editor round-trip gate** (`render(parse(md))===md`) reused from `myelin-content`.
- Visual-regression against the styleguide is optional.

---

## 9. Risks & validations (honest — validate these FIRST)

- **Kobalte primitive coverage** vs. the specs (command-palette combobox, overlays, menus, forms). *Validate in
  Tier 1/2 before depending on it.* Fallback: Ark/Zag (framework-agnostic), corvu, or hand-built on APG.
- **The block editor is the single hardest component, and the one place React's ecosystem (TipTap/Lexical/Slate)
  is genuinely richer.** Mitigation: **ProseMirror core is framework-agnostic** — wrap it in a Solid view; reuse
  the **`myelin-content` WASM** for parse/serialize (the round-trip gate already exists) and **Yjs/Yrs** for CRDT
  (already in the Knowledge backend). This is where to validate the Solid choice hardest. **Named fallback:** if
  the Solid editor view proves intractable, isolate *only the editor* as a React micro-island (Tauri/web can host
  one component in a different runtime) — a contained fallback, not a stack reversal.
- **Tauri-mobile maturity** — newer; validate early; Capacitor fallback.
- **SolidStart SSR/auth maturity** — validate the session flow early.
- **Agent fluency** — expect a short ramp; the §5 guide + lint are the mitigation; budget a first "teach the
  agents Solid" pass.

---

## 10. E0.7 deliverables (→ Phase-2 prompts)

1. **The Solid design-system package:** Tier 0 wired (tokens/icons/styleguide/a11y gates) + the "Solid patterns
   for agents" guide + the frontend lint gate (eslint-plugin-solid + jsx-a11y + axe).
2. **Tier 1 overlay primitives** on Kobalte (with the Kobalte-coverage validation as the first sub-task).
3. **Tier 2 shared components** in sequence — reference-chip+unfurl, views, and the block editor flagged as the
   load-bearing/hard ones (the editor likely several prompts).
4. **The SolidStart app shell** + auth/session + the **Tauri 2 shell skeleton** sharing the Rust core (with the
   mobile-target validation).
5. **The real-browser (Playwright) + axe test harness**, and the switch-test re-platformed to a real browser test.

Each becomes one or more 400k–700k-execution prompts in Phase 2, every prompt opening with the anti-duplication
check (grep the ledger + crates + the design-manual spec; the spec tells you what to build, the manual's Tier 0
tells you what to reuse).
