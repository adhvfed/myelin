# Design Refinements — user feedback log (binding)

> Direct user feedback on the chosen direction, captured during Phase 8. These OVERRIDE the
> sketches/specs where they conflict. Apply them everywhere, not only at the cited instance.
> Status date: **2026-06-20**.

## Direction chosen
The user **selected Finalist A "Instrument"** as the direction to proceed with (highly-unified ·
dense · palette-led · utilitarian). The design system (`01-tokens`, `02-components`, `03-styleguide`,
`04-icons`) is built for A. (The pipeline remains parameterized — see
[`00-framework-and-buildout-plan.md`](./00-framework-and-buildout-plan.md) — but A is the pick.)

## R1 — No rounded colored side borders (active/selected indicators)  [2026-06-20]
**Feedback:** the user likes direction A but **dislikes rounded colored side borders** — the
active/selected indicator that renders as a colored vertical bar with rounded ends (an accent
side-marker inset on a rounded item, e.g. the active rail nav item).

**Rule (general, binding):** do **not** use a colored side-bar / inset accent edge marker as the
selected or active indicator. Express selected/active with a **`--surface-hover` fill + brighter
text (`--text-primary`)**, optionally an **accent-tinted glyph** — a non-colour difference, so it
still never relies on colour alone (status/selection legibility preserved). If an edge marker is
ever genuinely needed, it must be **square-cut (no border-radius)**, not a rounded pill, and is the
exception, not the default.

**Applied:**
- Finalist A rail active item across all screens — removed `box-shadow: inset 2px 0 0 --c-rail-active`
  (and the RTL `-2px` variant); active state is now `--surface-hover` + `--text-primary`.
- `02-components/shell-and-nav.md` §4 default-state spec — updated to the fill+brightness treatment,
  "no colored side-bar / inset accent marker."

**Also apply (downstream / implementation):**
- The command palette active option (`06-palette` sketch used a colored `border-inline-start-color`
  leading edge): prefer the `--accent-weak` fill for the active row; keep a real `--focus-ring`
  outline for `:focus-visible` (do not let the colored leading edge be the only/active indicator).
- Any future tab/list/menu "selected" affordance: fill + weight, not a rounded colored side bar.
