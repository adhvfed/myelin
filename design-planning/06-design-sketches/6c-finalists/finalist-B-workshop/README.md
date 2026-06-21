# Finalist B — "Workshop" (deepened 6c mini-system)

> **Axis 3 = distinct-per-surface · warm · calm · contextual.** The dual-audience + approachability answer.
> Warm WITHOUT toylike — no emoji-as-UI, no whimsy, no sparkle on agents, no saturated traffic-light fills.
> Carrier **C-02** (roadmap + lens switch), grafting **C-19** (knowledge editor), **C-16** (live unfurl),
> **C-13** (HITL card), **C-09** (roadmap typography).
> **Honesty:** these are expert sketches — **`[DEFERRED-UNTIL-USERS]`**. The persona-adaptive vocabulary
> ("issue"↔"work item"↔"deliverable" via the lens) is carried as an **UNVALIDATED bet** (R-16 §6), not settled.

## Identity & persona
Hische/Hoefler editorial-warm but *disciplined*. A warm-grey/cream ramp (never `#fff`/`#000`), **one serif
(Fraunces) on reading headings only**, over a shared UI sans (Inter) for everything operational, mono
(JetBrains Mono) for code/refs. Generous measure (~66ch on the editor), humane copy, soft 10–14px radius,
borders-over-shadow with a single restrained soft-shadow token. Persona: a PM and an engineer share one
crafted workshop — neither is starved.

## The six axis positions
| Axis | Position |
|---|---|
| 1 Density | **calm** (compact only where earned — the diff) |
| 2 Nav | **contextual** (rail + sidebar adapts per surface; `⌘K` present) |
| 3 Unification | **distinct-per-surface** (the bet) — *but shell/chip/identity/palette/editor INVARIANT* |
| 4 Tone | **warm** (editorial, restrained) |
| 5 Agent | **ambient** (labelled, plain geometric mark, gated where consequential) |
| 6 Sovereignty | **on-demand** (residency cue in the scope bar; DSR receipt in states) |

## The distinct-per-surface bet (what this finalist must prove)
Surfaces feel **distinct** — the roadmap reads like a planning document, the diff like code, the knowledge
page like a finely-set book — while the **shell, the one reference chip, the identity badge, the agent
treatment, and the entire palette stay invariant** (one `tokens.css`, consumed by every screen). Distinctness
is **projection + measure + serif-emphasis (config on shared components)**, never a fork (R-16 §1.1). The diff
proves engineers aren't starved at the warm pole: compact mono rows, line-level keyboard hints
(`j/k`/`n/p`/`c`/`x`/`a`), self-assembling why-pane.

## Screens (`screens/`)
1. **`1-shell-roadmap-lens.html`** — shell + Q3 roadmap (now/next/later) over ISS-377/PR-412, with the
   **engineer/PM/exec lens switch** = the D5 dual-audience proof. German + Greek strings; locale SLA date.
2. **`2-engineer-pr-diff.html`** — dense engineer surface: **PR #412 context pane + diff** (flagship wedge),
   agent reviewer (advisory), keyboard model, the engineer **lens** of the same data.
3. **`3-fixagent-hitl.html`** — **FixAgent plan-then-apply approval card**: 3 effects, per-effect target chips,
   the **gated** open-PR-on-`main` effect, attribution + correlation + budget, Approve/**Edit**/Reject.
4. **`4-knowledge-unfurl.html`** — the **wedge**: a pasted `myelin://` link **unfurls live** with an inline
   action, inside the **ONE editor** tuned for prose (serif, ~66ch); restricted ref shows no title.
5. **`5-g2-rtl-i18n.html`** — **G2**: fully **mirrored RTL (Arabic)** shell + roadmap via logical properties,
   **Cyrillic** work item, locale Arabic-digit SLA date, bidi-isolated LTR refs.
6. **`6-states.html`** — unglamorous states: empty/loading-skeleton/error/permission-denied/erased-tombstone
   (Jürgen Vögel DSR)/agent-pending.

Dark mode: every screen consumes `[data-theme="dark"]` from `tokens.css` (set on `<html>` to preview).

## Tokens
`tokens.json` (W3C DTCG, `$type`/`$value`, three tiers **primitive → semantic → component**, light + dark) and
`tokens.css` (the CSS-var projection the screens consume; **no hardcoded colours/space in the HTML**).
Neutral-led warm ramp + **one** restrained terracotta accent (identity only). The **focus-ring / primary-action
token is a DERIVED AA-safe blue — deliberately NOT the brand accent** (the focus ≠ identity rule, §8b.3).

## How it meets G1 (WCAG 2.1 AA / EN 301 549) — MEASURED
Contrast measured with the WCAG relative-luminance formula. Key token pairs (light theme):

| Pair | Ratio | Floor | Pass |
|---|---|---|---|
| `--text` #211E18 on `--surface` #FBF8F2 | **15.68:1** | 4.5 | ✅ |
| `--text-2` #5C5345 on `--surface` (secondary) | **7.13:1** | 4.5 | ✅ |
| `--text-3` #6E6456 on `--surface` (muted) | **5.47:1** | 4.5 | ✅ |
| `--accent` #9C4A21 (identity, text use) on `--surface` | **5.80:1** | 4.5 | ✅ |
| `--focus` #1C5DA8 ring/primary on `--surface` | **6.23:1** | 3.0 (UI) | ✅ |
| `--on-accent` #FFF on `--accent` (button) | **6.15:1** | 4.5 | ✅ |
| `--status-ok` #2E6B43 / `--status-warn` #8A5A12 / `--status-danger` #A63A28 on surface | **6.00 / 5.58 / 6.08:1** | 4.5 | ✅ |
| status text on its tinted pill bg (ok/warn/danger) | **5.46 / 5.12 / 5.37:1** | 4.5 | ✅ |
| `--agent` #4B4A8C on `--surface` / on `--agent-wash` | **7.46 / 6.81:1** | 4.5 | ✅ |
| `--text` on diff add/del bg | **14.41 / 14.03:1** | 4.5 | ✅ |

Dark theme key pairs: `--text` #ECE6D9 on #1A1813 **14.26:1**; `--focus` #6BA8E8 **7.09:1**; `--on-accent`
#16140F on `--accent` #E08A53 **6.70:1**; statuses 5.89–7.71:1. All ✅.
Borders (`--line` 1.31:1) are **decorative only** — status is never carried by colour or border alone (always
glyph + label + position), so sub-3:1 borders are not a violation.

Other G1: **visible focus on every interactive element** (`:focus-visible` derived AA-safe ring, 2px/2px
offset; the diff has a focused-line example). **Keyboard model** documented per hard component — diff
(`j/k`/`n/p`/`c`/`x`/`a`), HITL card (tab to Approve/Edit/Reject), palette (`⌘K`), roadmap lens (segmented
buttons). **Status never by colour alone.** **Reflow** at <1080px collapses sidebar + pane, content never clips.

## How it meets G2 (i18n / l10n / RTL) — R-18 demo set
- **D-G2.1 German** — screen 1: `Schwellenwertkonfiguration für Anfrageratenbegrenzung` (long compound),
  `Jürgen Vögel` (umlaut) in states; no truncation/clipping (containers grow, 2-line tolerance).
- **D-G2.2 Non-Latin** — Greek (screen 1: `Δοκιμή στα ελληνικά`) **and** Cyrillic (screen 5: Bulgarian
  `Експорт на счетоводен дневник`); diacritic-safe line-height, no tofu.
- **D-G2.3 Mirrored RTL** — screen 5: whole shell + roadmap in **Arabic** (`dir="rtl"`), mirrored via
  **logical properties** (`inline-start/inline-end`, no `[dir=rtl]` override sheet); **mixed-direction run**:
  LTR `myelin://`, `PR #412`, `@Mara Ø.` `<bdi>`-isolated inside Arabic prose; directional chevron mirrors,
  non-directional status glyphs do not.
- **D-G2.4 Locale dates/numbers** — SLA `due 07.07.2026, 17:00 CEST` (German/EU format) on screen 1; Arabic
  digits `٧‏/٧‏/٢٠٢٦، ١٧:٠٠` on screen 5 (display locale vs. computed-on-policy-calendar distinction noted).
- **D-G2.5 No machine strings** — humanised statuses/labels throughout (no raw enum keys).
- **D-G2.6 Logical properties (inspectable)** — `tokens.css` shell/chip/sidebar use `inline-start/end`,
  `border-inline`, `margin-block`, etc., so RTL is by-construction.

Languages shown: **English, German, Greek, Cyrillic (Bulgarian), Arabic (RTL)**.

## Honest self-assessment vs rubric D1–D10 (0–4 each)
| Dim | Weight | Score | One-line rationale |
|---|---|---|---|
| **D1** Power-user efficiency | 12% | **2.5** | Diff has a real keyboard model + `⌘K`; but calm/warm pole sacrifices some raw density vs Finalist A — engineers served, not maximised. |
| **D2** First-run / approachability | 10% | **4** | The empty roadmap teaches the next step; warm-without-toylike; a PM lands and acts. This finalist's strongest dimension. |
| **D3** Visual craft & tone | 12% | **3.5** | Disciplined editorial-warm, one serif, token-clean, AA-measured; distinctive and intentional. Risk: warmth could read soft to some engineers. |
| **D4** One-product coherence | 14% | **3** | One shell/chip/identity/palette/editor across 6 screens — but distinct-per-surface is *inherently* the harder coherence case to defend; the bet itself costs a point. |
| **D5** Dual-/tri-audience | 10% | **3.5** | The lens switch over the SAME ISS-377 rows (engineer↔PM↔exec) is direct, switchable, neither lens starved — best D5 in the set; capped because vocabulary mapping is unvalidated. |
| **D6** Agent legibility & trust | 12% | **3.5** | Plan-then-apply, per-effect chips, the gated `main` effect, attribution/correlation/budget, reserved mark + label, advisory inline review. Strong; slightly less than Finalist C's foregrounded depth. |
| **D7** Density-made-calm | 8% | **3.5** | Calm is the native default here; agent volume kept ambient/out of timeline; quiet wins. |
| **D8** Perceived performance | 6% | **3** | Structure skeletons (not spinners), live-unfurl re-resolves, pulse interruptible + reduced-motion path; optimistic-rollback only sketched. |
| **D9** Sovereignty / GDPR-as-UX | 8% | **2.5** | Residency cue in scope bar, no-leak permission/restricted refs, DSR tombstone + verifiable receipt — but on-demand (not always-on); Finalist D owns this. |
| **D10** The switch test | 8% | **3** | Roadmap-is-delivery kills the Productboard copy; diff+CI+chat+knowledge cover the daily flow; some depth (search, bulk ops) only gestured. |

**Weighted total ≈ 3.21 / 4** (`12·2.5 + 10·4 + 12·3.5 + 14·3 + 10·3.5 + 12·3.5 + 8·3.5 + 6·3 + 8·2.5 + 8·3` / 100).

**Weakest dimension: D1 (power-user efficiency, 2.5)** and **D9 (sovereignty, 2.5)** — the honest cost of the
warm/calm/on-demand position. D1 is the deliberate trade against Finalist A; D9 is deliberately ceded to
Finalist D's always-on posture.

## Known gaps
- Persona-adaptive **vocabulary** (lens label map) is an **unvalidated bet** (R-16 §6.4 falsifier: PMs may
  reject coarse same-schema records and demand a separate narrative tool).
- Exec-lens rollup is named in the switch but not separately rendered (PM + engineer lenses are the proof shown).
- RTL **comprehension** of the mirrored timeline + LTR-code islands is `[DEFERRED-UNTIL-USERS]` (R-18 §10.2).
- Fonts via CDN here; production self-hosts a single family with EU-24 + Greek + Cyrillic + Arabic coverage
  (no font CDN — sovereignty).
- Optimistic-update rollback and the command palette itself are gestured, not fully realized.
