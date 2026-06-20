# Myelin Design System — index (direction A "Instrument")

> Phase 8 of the design/UX effort. The implementable design-system resources for the **chosen
> direction, Finalist A "Instrument"** (highly-unified · dense · palette-led · utilitarian). Built
> on the Phase-4 research corpus, the Phase-5 surface map, and the Phase-7 decision. Status date:
> **2026-06-20**. The whole system is **parameterized**: swapping to another direction is a
> token-set + variant-flag re-run (see `00-framework-and-buildout-plan.md` §2), not a rebuild.

## What's here
| Path | What it is |
|---|---|
| [`00-framework-and-buildout-plan.md`](./00-framework-and-buildout-plan.md) | The stack decision (TS + React + **React Aria Components** + Style Dictionary + a custom live styleguide + self-hosted assets), the **parameterization model** (token set + 6 variant flags; no component may `switch(direction)`), the component buildout sequence, and the cross-cutting capabilities to implement. |
| [`REFINEMENTS.md`](./REFINEMENTS.md) | **Binding user feedback** on the chosen direction. R1: no rounded colored side borders (active/selected = fill + brightness, never a colored side-bar). |
| [`01-tokens/`](./01-tokens/) | The complete **DTCG token system** — primitive→semantic→component, **light / dark / high-contrast**, measured-AA contrast (lowest 5.05:1), focus-ring derived ≠ identity accent. `tokens.json` (source) + `tokens.css` (the CSS-var projection components consume) + `tokens.md` (docs + contrast tables). |
| [`02-components/`](./02-components/) | The **component inventory** — each spec'd with anatomy, variants + the 6 parameterization flags, ALL states (incl. empty/loading/error/permission-denied/erased/agent-pending), keyboard + ARIA (mapped to React Aria), tokens consumed, motion, do/don't. Overlays · shell & nav · command palette · identity/agent badge · forms · reference chip/unfurl · agent/HITL card · comments/mentions · views · block editor · notifications inbox. See its `README.md` for the full index + the unwritten-spec flags. |
| [`03-styleguide/`](./03-styleguide/) | A **live styleguide** (`index.html`) rendering the REAL tokens — light/dark/high-contrast + RTL toggles, token galleries with live contrast labels, and a component showcase. Runs stack-down, no build, no CDN. |
| [`04-icons/`](./04-icons/) | The **icon library** (42 core icons) authored with `strok`. `strok/*.strok` (source) → `build.sh` → `svg/*.svg` (currentColor, theme-inheriting) + `preview/*.png`. `ICONS-README.md` (name→meaning registry + spec + build/consume/add-new), `ACCEPTANCE.md` (10/10), `icons-index.html` (contact sheet). |

## How the build track consumes this
1. Tokens are the contract — generate CSS vars + TS constants from `01-tokens/tokens.json` via Style Dictionary; the measured-contrast + focus≠identity CI gate guards them.
2. Components consume **only** semantic tokens; build the overlay primitives FIRST (§8b.1), then the shared components, per the buildout sequence; map each to its React Aria primitive; honor REFINEMENTS.
3. Icons ship from `04-icons/svg/` (run `build.sh` to regenerate); they inherit `currentColor` from the token-driven text/icon color.
4. The live styleguide is the reference that must not drift from the app (render it from the real generated tokens).

## Provenance & honesty
Built for direction **A** (Phase-7 recommendation, **confirmed by the user**); the runner-up was D "Civic" (sovereignty-first). All `[DEFERRED-UNTIL-USERS]` and `[UNDER-EVIDENCED]` flags from the corpus are carried — this is expert design work, **not user-validated**; the deferred-until-users research track (roadmap §6) is the validation plan. The Phase-7 panel flagged cross-cutting build gaps now specified as required capabilities in `00-framework-and-buildout-plan.md` §4 (real command-palette primitive, optimistic-update+rollback, forced-colors, aria-busy/live-regions).
