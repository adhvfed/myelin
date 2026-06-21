# Finalist A — "Instrument"

> Phase 6c deepening. The **highly-unified · dense · palette-led · utilitarian** direction
> (Product-Precision / Linear–Vercel lineage; Rauno-Freiberg "Instrument"). Carrier concept **C-05**
> (diff), absorbing **C-01** (shell/PR pane), **C-08** (board), **C-15** (palette), **C-14** (ambient
> agent inbox). Status date: **2026-06-20**. **`[DEFERRED-UNTIL-USERS]`** — these are expert sketches,
> not user-validated; correctness of i18n/RTL/contrast is PROVEN-by-construction, comprehension is
> HYPOTHESIS. Tags: **PROVEN** (inspectable in the artifact / a measured number / a cited rule) ·
> **HOUSE STYLE** (our taste).

## Identity & persona

A **midnight command-deck** for a power user who lives on the keyboard. Hairlines do *all* grouping
(borders over shadow); ONE rationed electric-blue accent, reserved for identity + (derived) focus;
monospace is load-bearing and native, not quarantined; near-zero radius; compact-by-default. The thesis
of this finalist is **maximum muscle-memory transfer, minimum per-surface personality** — the same skin,
density-tuned per surface, so a user never feels they "left one app." Persona: **Mara Ø.**, a platform
engineer reviewing PR #412; the same skin serves a PM via a calmer lens on the *same* component.

## The six axis positions (PROVEN — read from the artifacts)

| Axis | Position | Where it's visible |
|---|---|---|
| **1 Density** | **dense** (compact default) | diff, board — high cards/lines per screen, kept calm |
| **2 Nav** | **palette-led** | `⌘K` is the spine (screen 6); rail is thin/summon-first |
| **3 Unification** | **highly-unified (one-skin)** | identical chrome/chip/status grammar across all 6 screens |
| **4 Tone** | **utilitarian** | no decorative colour; hierarchy from weight+colour before size |
| **5 Agent** | **ambient** | FixAgent is one typed row in the inbox; rolled-up board hint |
| **6 Sovereignty** | **on-demand** | residency chip in top bar; DSR row in sidebar; erasure receipt |

## Screens (the comparable set)

1. **`1-shell-pr-context.html`** — the shell framing **PR #412** + the self-assembling context pane
   (Code + Issue + CI + Doc + Agent in one skeleton). *(shell + dense engineer overview)*
2. **`2-pr-diff.html`** — the **dense engineer surface**: PR #412 diff (flagship wedge W1) with the
   agent suggested-fix, line-level keyboard hints, and the **rebase-orphan** state.
3. **`4-board-roadmap.html`** — the **shared views component, two lenses**: dense engineer **board** ↔
   calm PM **Q3 now/next/later roadmap** over the *same* issue data (the D4 + D5 proof, one toggle).
4. **`5-hitl-inbox.html`** — the **agent / HITL moment**: FixAgent plan-then-apply card (open PR #412
   follow-up on protected `main` = the **gated** effect; transition ISS-377; post to
   `#payments-incidents`), with Approve / **Edit** / Reject, per-effect target chips, attribution, audit.
5. **`6-palette.html`** — the **wedge**: `⌘K` over the universal graph — four modes, token→chip query
   AST, permission-pre-filtered rows, consequential verbs routing **into** the gate.
6. **`7-states-and-rtl.html`** — the **unglamorous states** (empty/loading/error/permission-denied/
   erased-tombstone/agent-pending) for the board surface + the **mirrored RTL** state.

## Token approach (DTCG)

`tokens.json` is **W3C DTCG** (`$type`/`$value`), three tiers **primitive → semantic → component**,
**light + dark**. `tokens.css` is the CSS-var projection every screen consumes — **no hardcoded
colours/space in the HTML, vars only**. Neutral-led + one accent. RTL is **by construction**: components
use logical properties (`inline-start/end`, `border-inline`, `margin-inline`), so `[dir="rtl"]` mirrors
with **no override sheet**.

**The focus token ≠ the identity token (PROVEN — design-language §8b.3 / R-17 §3.2).** `--accent` is the
brand/identity blue; `--focus` (= `--c-focus-ring` = primary-action fill) is a **distinct, derived,
AA-safe** token. Even though the identity accent itself clears AA here, focus rides the derived token so
the affordance is never hostage to a brand choice.

## How it meets G1 (Accessibility — WCAG 2.1 AA / EN 301 549) — measured

**Contrast measured, not claimed** (sRGB WCAG formula). Key token pairs:

| Pair | Dark | Light | Floor |
|---|---|---|---|
| primary text `--t1` / base | **16.00:1** | **17.93:1** | 4.5 |
| secondary text `--t2` / base | **9.25:1** | **8.11:1** | 4.5 |
| tertiary text `--t3` / base | **5.81:1** | **5.36:1** | 4.5 |
| **focus ring `--focus` / base** | **8.13:1** | **6.55:1** | 3.0 (UI) |
| identity accent `--accent` / base | 6.14:1 | 4.50:1 | 3.0 |
| primary-button text on `--focus` | 8.13:1 (dark text) | **6.55:1** (white) | 4.5 |
| status ok / warn / bad / agent | 7.31 / 8.89 / 5.87 / 8.01 | 5.43 / 5.92 / 5.44 / 6.22 | 4.5 |
| diff add-text / add-bg | 9.03:1 | 5.80:1 | 4.5 |
| diff del-text / del-bg | 7.16:1 | 5.87:1 | 4.5 |

**All pairs clear AA in both themes.** Other G1 evidence in the artifacts: **one `--focus` ring on every
interactive element, every theme** (incl. a `forced-colors` fallback); **status never by colour alone**
(every status = glyph + label + position — diff uses `+/−` text signs + glyph; CI/PR use SVG glyph +
word); **keyboard models shown** — diff `j/k` line + `]/[` change (live JS), board `j/k` card nav,
palette `↑↓` roving + `↵/⇥/→/esc`, HITL card reachable buttons; **24px min target** token; **reduced-motion**
zeroes all animation (media query in `tokens.css`); **skip link**, landmarks, `aria-busy` skeletons.

## How it meets G2 (i18n / l10n / RTL) — what's shown

Per R-18 §7.1 demonstration set D-G2.1–D-G2.6:

- **D-G2.1 German (long compound + umlaut):** `Konfigurierbarer Rate-Limiting-Schwellenwert`,
  `Vorhersagbares Backpressure-Schwellenwert-Verhalten`, **Jürgen Vögel** as reviewer/DSR subject,
  German inbox provenance lines — on dense surfaces (sidebar rows wrap to 2 lines, **no truncation**).
- **D-G2.2 Non-Latin (Greek + Cyrillic):** Greek review comment with **tonos diacritics** in a mono diff
  line (`Αλέξανδρος Παπαδόπουλος`, `εκκαθάρισης`), Greek + Cyrillic issue titles on the board/roadmap
  (`Μετρικές…`, `Очистка устаревших ключей…`) — no tofu, no diacritic clipping (line-height ≥ 1.45).
- **D-G2.3 Mirrored RTL:** screen 7 mirrors the **shell + content surface in real Arabic**, with a
  **mixed-direction run** — LTR `PR #412`, SHA `a94bcc7`, `myelin://…`, and `Mara Ø.` `<bdi>`-isolated
  inside RTL prose; directional chevrons mirror, status glyphs/brand do not.
- **D-G2.4 Locale-formatted dates on an SLA surface:** `Fällig 23.06.2026, 17:00 MESZ · in 2 Tg 4 Std`
  (de-DE) on the PR context pane and roadmap; Arabic deadline `23.06.2026`.
- **D-G2.5 No machine strings:** humanised, localized states everywhere ("In review"/"قيد المراجعة",
  "awaiting approval"); no raw enum keys / ids leaking.
- **D-G2.6 Logical properties (inspectable):** the CSS uses `inline-start/end`, `border-block`,
  `margin-inline` throughout — grep the screens.

## Honest self-assessment vs rubric D1–D10 (0–4 each)

| Dim | Score | One-line rationale |
|---|:--:|---|
| **D1 Power-user efficiency** | **4** | Palette spine + diff/board keyboard models + dense-but-legible; crosses the flow faster than Linear. |
| **D2 First-run delight / approachability** | **2** | The PM roadmap lens + onboarding-forward empty state help, but the midnight-utilitarian skin reads cold to a non-engineer — this finalist's deliberate weak spot. |
| **D3 Visual craft & tone** | **3** | Disciplined token system, hairline grouping, one accent; intentional but austere (by design, not exemplary-warm). |
| **D4 One-product coherence** | **4** | One skin, one chip, one status grammar, one palette across all 6 screens; board↔roadmap is visibly one component density-tuned. |
| **D5 Dual-/tri-audience** | **3** | Same views component, engineer board ↔ PM roadmap via one in-place lens toggle; neither lens starved — but PM lens is the less-loved half. |
| **D6 Agent legibility & trust** | **3** | FixAgent always labelled + plain mark, plan-then-apply with per-effect gate/attribution/audit, Edit present; ambient (calm) but not the foregrounded D6 showcase that Finalist C is. |
| **D7 Density-made-calm** | **4** | The whole bet: dense diff/board kept calm, agent volume rolled up, one prioritised inbox, no firehose. |
| **D8 Perceived performance** | **3** | Structure skeletons (context pane, board loading, `aria-busy`), agent-pending designed; optimistic-rollback only gestured. |
| **D9 Sovereignty / GDPR-as-UX** | **2** | Residency chip, DSR sidebar row, erased-tombstone + erasure receipt cue — present and legible, but on-demand by axis position; the deep DSR console is Finalist D's job. |
| **D10 Switch test** | **3** | A team could move off Jira/GitHub-review/Linear and gain on speed + cross-artifact context; Slack/Notion parity is thinner. |

**Weighted total ≈ 3.21 / 4 (≈ 80 / 100).**
Computed: .12·4 + .10·2 + .12·3 + .14·4 + .10·3 + .12·3 + .08·4 + .06·3 + .08·2 + .08·3 = **3.20**.

**Weakest dimension: D2 (first-run delight / approachability) = 2** — and **D9 = 2** tied. Both are the
honest cost of the highly-unified, dense, utilitarian, on-demand-sovereignty pole: this finalist optimises
for the power user's muscle memory, and a non-engineer's warmth (D2) plus an always-on sovereignty posture
(D9) are exactly what it trades away. Choosing *against* A means preferring those; choosing A means
prizing one-skin coherence + power-user speed.

## Known gaps

- No standalone **CI run #1894** screen (the dense-engineer slot is satisfied by the diff; CI is
  referenced/linked but not built — an "OR" option per the brief).
- Optimistic-update **rollback** is described, not animated.
- Fonts via CDN are **throwaway**; production self-hosts a variable family carrying Latin-ext + Greek +
  Cyrillic + Arabic (sovereignty, design-language §3.3) — coverage is a `[VERIFY]` selection gate.
- RTL/contrast correctness is PROVEN-by-construction; **real-RTL comprehension and copy quality are
  `[DEFERRED-UNTIL-USERS]`** (R-18 §10).
