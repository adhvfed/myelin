# Finalist C — "Wayfinding"

> Stage 6c deepened finalist. **Axis-3 = distinct-per-surface (utilitarian) · Axis-5 = foregrounded agents.**
> Carrier **C-20** (chat topics + foregrounded HITL), grafting **C-13** (single-object HITL focal craft),
> **C-07** (data-annotating foregrounded agent), **C-04** (honest seam discipline, dialled back from EXTREME).
> Persona/visual lineage: **Erik Spiekermann** — information design as wayfinding; functional grotesk;
> address-grade labelling (channel ▸ topic ▸ message, each addressable like a platform number); density that
> stays navigable like a transit map; colour rationed to one wayfinding accent. Date 2026-06-20.
> **The bet: foregrounded agents WITHOUT drowning humans.** Agents look like agents — label + plain
> geometric mark, never sparkle/magic-wand/emoji.
> **[DEFERRED-UNTIL-USERS]:** these are expert sketches (R-14/R-15 HAX+PAIR heuristic, R-17/R-18 standards),
> not user-validated. Trust-calibration (R-15 Part 2) and real-RTL comprehension (R-18 §10) are deferred studies.

## The six axis positions
| Axis | Position | How it shows up |
|---|---|---|
| 1 density | **medium** (dense when earned) | chat = medium; CI run = dense-but-calm (DAG route-map + log time-series); roadmap = calm |
| 2 nav | **rail** (+ ⌘K spine) | icon rail · channel▸topic wayfinding column · content · context pane; palette is the wedge |
| 3 unification | **distinct-per-surface (utilitarian)** | each surface keeps its own working identity (chat / CI / roadmap differ) but shares ONE shell, chip, agent treatment, status grammar, button, palette, tokens — distinct that still feels like one product |
| 4 tone | **utilitarian** | signage clarity, no warmth-decoration; hierarchy from weight+colour before size |
| 5 agent | **FOREGROUNDED** | agents are visible inline collaborators (chat participant, log annotator, triage panel) — yet **contained by topic**, chained-collapsible, gated, one click from provenance |
| 6 sovereignty | **on-demand** | residency cue (`eu-west · Frankfurt`) present near data, expandable; no leak on restricted refs |

## Screens (the comparable screen set)
- `screens/01-shell-chat.html` — **the shell + chat** (carrier C-20). The one-product frame; foregrounded-but-contained agents; in-stream HITL card; collapsed agent-chain (foregrounded ≠ firehose).
- `screens/02-ci-run.html` — **dense engineer surface**: CI run #1894 (graft C-07). DAG as a route-map, log as time-series, a foregrounded agent that **annotates the data** at the failing line.
- `screens/03-roadmap.html` — **approachable PM/corporate surface**: Q3 roadmap now/next/later over the same issues, the lens-switch, the topic-summary canvas. Shares the same components as screen 2, tuned by role (D5).
- `screens/04-hitl-card.html` — **the standout**: FixAgent HITL approval card (graft C-13's focal craft). Proposed effects, per-effect target chips, the **gated** effect (open PR on protected `main`), the **Edit** proposed→amended diff, scope (∩ intersection), live budget, attribution + Why? + audit, correlation walk.
- `screens/05-palette.html` — **the wedge**: ⌘K palette (Navigate/Act/Search/Build-query, permission-filtered, consequential verbs route into the gate) + a live `myelin://` unfurl with an inline action.
- `screens/06-rtl-arabic.html` — **mirrored RTL state** (G2): the whole shell + chat in real Arabic, mixed-direction LTR runs `<bdi>`-isolated.
- `screens/states.html` — **unglamorous states**: empty · loading · error · permission-denied · erased/tombstone · agent-pending · full HITL gate states · **the agent-storm (calm-under-30×-surge)** · stream-drop/resume.

## Token approach
DTCG `tokens.json` (W3C `$type`/`$value`), three tiers **primitive → semantic → component**, projected to CSS
vars in `tokens.css`. Screens consume **only** vars (no hardcoded colours/space). Neutral-led cool-grey ramp +
**one rationed wayfinding orange** accent; reserved **agent indigo** (its own semantic axis, not a status colour);
**topic blue** for thread addresses. Light + dark (`[data-theme]` toggle on every screen). 4/8 spacing ramp,
near-zero radius. **The focus/primary-action token is DERIVED AA-safe and is NOT the brand accent** (blue
`--focus` ≠ orange `--accent`) — the §8b.3 focus-token-≠-identity rule. Logical properties throughout
(`inline-start/inline-end`, `margin-inline-*`, `border-inline-*`, inset-start box-shadow) so RTL is by-construction.

## How it meets G1 (WCAG 2.1 AA / EN 301 549) — MEASURED
Contrast measured with the WCAG relative-luminance formula. AA floor: **4.5:1** body text, **3:1** large/UI/focus.

| Pair | Dark | Light |
|---|---|---|
| primary text on bg | **17.48:1** | **15.55:1** |
| secondary text (ink-2) on bg | **11.36:1** | **9.26:1** |
| tertiary/meta (ink-3) on bg | **4.98:1** | **5.49:1** |
| accent on bg (UI/icon) | **5.99:1** | **4.58:1** |
| accent-ink on accent (button label) | **5.87:1** | **4.87:1** |
| **focus ring on bg** (≠ accent) | **8.19:1** | **4.92:1** |
| agent token on agent-bg | **6.88:1** | **5.94:1** |
| topic token on bg | **7.65:1** | **6.31:1** |
| ok / warn / crit status on bg | 7.01 / 8.93 / 6.36 | 5.04 / 5.16 / 5.62 |
| gate-marker (warn) on panel | **8.39:1** | — |

All text pairs ≥4.5:1; all focus/UI/status pairs ≥3:1 (lowest focus 4.92:1). **Visible focus** on every interactive
element (`:focus-visible` 2px derived-blue ring). **Status never by colour alone** — glyph (`✓ ✗ ◴ ○ ◷ ⚷`) + label +
position everywhere (`.status` grammar). **Keyboard model**: palette (↑↓ move · ↵ run · ⇥ drill · esc); HITL card
(per-effect Approve/Edit/Reject individually focusable, Edit re-runs the full pipeline); diff/log/DAG focusable rows;
agent treatment carried as **TEXT** (`Agent` badge), not colour/icon (WCAG 1.4.1). **Reflows** at narrow width
(shell collapses pane → nav → single column).

## How it meets G2 (i18n / l10n / RTL) — languages/RTL demonstrated
- **German** (D-G2.1) — `Jürgen Vögel`, `Neuberechnung bei der Verdrängung ist der Hot-Path`, compound labels
  (`Rate-Limiting-Schwellenwerte`-class), `Hot-Path`, `flake · letzte 50`, `SLA fällig … MESZ` — no truncation;
  containers grow, two-line tolerant.
- **Greek** (D-G2.2, non-Latin) — roadmap table column `Τίτλος` / `Κατάσταση` and real titles with diacritics
  (`Η προσωρινή μνήμη πληρωμών υπερφορτώνεται`, `Σε εξέλιξη`, `Έτοιμο`) — diacritic headroom via `--lh-tight: 1.35`.
- **Arabic RTL** (D-G2.3, mirrored) — `06-rtl-arabic.html`: `dir="rtl"`, whole shell + chat + context pane mirror
  via logical properties; **mixed-direction runs** (`CI run #1894`, `main@b5d94c0`, `@jurgen`, `weight()`, all
  `myelin://`/SHA/ref/handle) `<bdi>`-isolated so LTR code never reverses; breadcrumb chevron mirrors, the agent
  geometric mark + status glyphs do NOT (non-directional, R-18 §4.2).
- **Locale-aware** (D-G2.4) — SLA on the roadmap: `fällig 07.07.2026, 17:00 MESZ · in 3 Tagen` (de-DE date + decimal
  `2,1 %` + `0,04 €`); display locale ≠ calculation calendar.
- **No machine strings** (D-G2.5) — humanised states/enums throughout; **Logical CSS** (D-G2.6) inspectable in
  `tokens.css` + every screen (`start/end`, never physical `left/right`).

## Honest self-assessment vs rubric D1–D10 (0–4 each)
| Dim | Score | One-line rationale |
|---|---|---|
| **D1** density-made-legible | **3** | CI surface earns real density (DAG + aligned durations + log) and stays calm; chat/roadmap deliberately lighter — strong, not the densest in the set (that's A). |
| **D2** dual-audience approachability | **3** | roadmap + lens-switch + topic-summary canvas read for a PM; but the tone is utilitarian-signage, less inviting than B's warmth. |
| **D3** flow velocity | **3** | ⌘K spine, keyboard model, refs-as-navigation, consequential-verbs-route-to-gate; rail nav is a touch slower than A's palette-led. |
| **D4** one-product coherence | **3** | ONE shell/chip/agent-treatment/status-grammar/button/palette/tokens across 7 screens IS the evidence; but Axis-3 is *distinct-per-surface* by design, so coherence is deliberately looser than A/D's one-skin. |
| **D5** dual-lens over one data model | **3** | screens 2+3 are the same components + same issues (ISS-377, CI #1894) at engineer vs roadmap density; lens-switch present; not as deeply wired as B's three-lens-over-one-record. |
| **D6** agent legibility & trust | **4** | the bet and the strength: foregrounded-yet-contained agents, plan-then-apply card with per-effect gate + Edit-diff + scope∩ + budget + Why?/audit + correlation walk, the full 10-state set, and the agent-storm proving calm-under-volume. Best-in-set. |
| **D7** calm-under-load | **4** | the agent-storm state is the explicit proof: human lane holds, agent lane sheds, chain collapses, gates group; chatter out of the main timeline by default. |
| **D8** visual craft / restraint | **3** | rationed accent, hairline grouping, no sparkle, signage discipline; competent and consistent rather than singular — honest 3. |
| **D9** sovereignty-as-UX | **2** | residency cue near data, no-leak restricted refs, tombstone + audit-hash-never-rewritten, correlation/audit walk — present and correct, but on-demand (not the always-on DSR console; that's D's job). Weakest dimension. |
| **D10** accessibility & i18n floor | **3** | measured AA on all pairs, derived focus token, status-not-colour-alone, full RTL mirror with real Arabic + bidi isolation, German + Greek; honest 3 because it's standards-met-by-construction, not user-tested (R-17/R-18 deferred). |

**Total: 31 / 40.** **Weakest dimension: D9 sovereignty-as-UX (2)** — by design this finalist sits at Axis-6
*on-demand*; the always-on DSR/sovereignty console is Finalist D's flagship, so C shows the *cue* treatment, not
the console.

## Known gaps
- Sovereignty is the deliberate trade (D9=2) — C is the foregrounded-agent finalist, not the regulated-buyer one.
- One Arabic RTL screen (shell+chat) is mirrored; the dense CI surface RTL is specified (logical props) but not
  separately rendered — the hardest real-RTL comprehension case (LTR code islands in RTL chrome) is the named R-18 §10.4 bet.
- Static HTML: live unfurl, streaming log, storm-collapse animations are depicted, not interactive.
- Motion is functional + reduced-motion-pathed but minimal; all "pages render, they don't animate in."
- All trust/comprehension claims are `[DEFERRED-UNTIL-USERS]` (R-14 §10 / R-15 Part 2 / R-18 §10).
