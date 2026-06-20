# Myelin Design Tokens — the complete system (Phase 8b)

> **Direction:** finalist **A "Instrument"** (midnight command-deck: neutral-led, one rationed
> electric-blue accent, near-zero radius, hairlines-first, compact default, crisp/instant motion).
> **File date: 2026-06-20.** Artifacts in this folder: [`tokens.json`](./tokens.json) (DTCG source),
> [`tokens.css`](./tokens.css) (the CSS-custom-property projection components consume), this doc.
>
> **Pipeline:** `tokens.json` (W3C DTCG) → Style Dictionary v4+ → `tokens.css` (+ TS constants).
> See [`../00-framework-and-buildout-plan.md`](../00-framework-and-buildout-plan.md) §1.3.
> **Self-hosted assets, no CDN** (sovereignty/GDPR, §3.3). **Not committed.**
>
> **Tagging:** **PROVEN** = the contrast math (WCAG 2.1/2.2 relative-luminance formula) and the cited
> standards. **HOUSE STYLE** = the chosen values / the character of direction A. Every ratio in the
> tables below is **MEASURED** with the WCAG formula on the actual token hex (recomputed, not copied
> from the finalist README), per the measured-not-claimed gate (§8b.3; R-17 §3).

---

## 1. The tier model (PROVEN architecture — design-language §3.1)

| Tier | What it is | Who consumes it |
|---|---|---|
| **Primitive** | Raw values, no meaning: the neutral ramps, the brand ramp + its derived focus steps, functional ramps, agent ramp, type scale, spacing, radius, elevation, motion, z-index. | Nothing directly. Only aliased upward. |
| **Semantic** | Intent-named, **theme-aware**: `surface*`, `text-*`, `border*`, `accent`/`on-accent`, `focus-ring`, `success`/`warning`/`danger`/`info` (+ `on-`/`-subtle`), `agent`/`on-agent`/`agent-subtle`. | **Components consume ONLY this tier.** This is the swap surface (§7). |
| **Component** | Optional per-component overrides, each bound to a **semantic** token (never a primitive): `button-primary-bg` (→ `focus-ring`), `rail-active-accent` (→ `accent`), `chip-*`, `agent-mark`, focus width/offset. | The few components that need a named handle; everything else reads semantics. |

The three-tier indirection is what makes dark / light / high-contrast a **table swap, not a
component rewrite** — and it is the *same* mechanism the direction-parameter rides (§7).

---

## 2. Measured contrast tables (every semantic text/UI pair, all three themes)

**Floors (PROVEN — WCAG 1.4.3 / 1.4.11):** normal text **4.5:1**; large text (≥24px or ≥18.7px bold)
& UI components/graphical objects **3:1**. `on-*` rows are text on a coloured fill. `border` hairlines
are **decorative grouping**, not "essential UI", so they are exempt from 3:1 — the **focus-ring**
carries the 3:1 UI obligation on every interactive element (and meets it in every theme).

### 2.1 DARK (default) — base `#0c0d10`

| Pair | Token | Measured | Verdict |
|---|---|---|---|
| `text-primary` / surface | `#e7e9ee` | **16.00:1** | AAA |
| `text-muted` / surface | `#aeb3be` | **9.25:1** | AAA |
| `text-subtle` / surface | `#868d99` | **5.81:1** | AA |
| `text-primary` / raised | `#e7e9ee` | 15.52:1 | AAA |
| `text-muted` / raised | `#aeb3be` | 8.97:1 | AAA |
| `text-subtle` / raised | `#868d99` | 5.64:1 | AA |
| `accent` (identity) / surface | `#5b8cff` | 6.14:1 | AA |
| **`focus-ring` (DERIVED) / surface** | `#7ea6ff` | **8.13:1** | AAA |
| `on-accent` / focus fill | `#0c0d10` | 8.13:1 | AAA |
| `success` / surface | `#46b277` | 7.31:1 | AAA |
| `warning` / surface | `#d6a93f` | 8.89:1 | AAA |
| `danger` / surface | `#e0695c` | 5.87:1 | AA |
| `info` / surface | `#7ea6ff` | 8.13:1 | AAA |
| `agent` / surface | `#a99bf2` | 8.01:1 | AAA |
| `on-agent` / agent fill | `#0c0d10` | 8.01:1 | AAA |
| `border-strong` / surface | `#3a3f47` | 1.83:1 | decorative (exempt) |

### 2.2 LIGHT — base `#ffffff`

| Pair | Token | Measured | Verdict |
|---|---|---|---|
| `text-primary` / surface | `#15171c` | **17.93:1** | AAA |
| `text-muted` / surface | `#4a505b` | **8.11:1** | AAA |
| `text-subtle` / surface | `#646b78` | **5.36:1** | AA |
| `text-primary` / raised | `#15171c` | 16.87:1 | AAA |
| `text-muted` / raised | `#4a505b` | 7.63:1 | AAA |
| `text-subtle` / raised | `#646b78` | **5.05:1** | AA ← lowest text pair in the system |
| `accent` (identity) / surface | `#2f6bff` | 4.50:1 | AA (identity only) |
| **`focus-ring` (DERIVED) / surface** | `#1452d6` | **6.55:1** | AA |
| `on-accent` / focus fill | `#ffffff` | 6.55:1 | AA |
| `success` / surface | `#15794a` | 5.43:1 | AA |
| `warning` / surface | `#8a5a07` | 5.92:1 | AA |
| `danger` / surface | `#c0392b` | 5.44:1 | AA |
| `info` / surface | `#1452d6` | 6.55:1 | AA |
| `agent` / surface | `#5b4ec9` | 6.22:1 | AA |
| `on-agent` / agent fill | `#ffffff` | 6.22:1 | AA |
| `border-strong` / surface | `#9aa1ac` | 2.60:1 | decorative (exempt) |

### 2.3 HIGH-CONTRAST — base `#000000` (authored theme; EN 301 549 / WCAG 1.4.11)

| Pair | Token | Measured | Verdict |
|---|---|---|---|
| `text-primary` / surface | `#ffffff` | **21.00:1** | AAA |
| `text-muted` / surface | `#d4d8e0` | 14.70:1 | AAA |
| `text-subtle` / surface | `#aab2c0` | 9.84:1 | AAA |
| `text-subtle` / raised | `#aab2c0` | 9.28:1 | AAA |
| `accent` (identity) / surface | `#93b4ff` | 10.21:1 | AAA |
| **`focus-ring` (DERIVED) / surface** | `#aec4ff` | **12.14:1** | AAA |
| `on-accent` / focus fill | `#000000` | 12.14:1 | AAA |
| `success` / surface | `#5ad08c` | 10.84:1 | AAA |
| `warning` / surface | `#f0c050` | 12.36:1 | AAA |
| `danger` / surface | `#ff8a7d` | 9.18:1 | AAA |
| `info` / surface | `#aec4ff` | 12.14:1 | AAA |
| `agent` / surface | `#c3b6ff` | 11.46:1 | AAA |
| `on-agent` / agent fill | `#000000` | 11.46:1 | AAA |
| **`border` / surface** | `#5a5f6b` | **3.28:1** | UI (≥3:1 — lifted: borders are essential UI here) |
| `border-strong` / surface | `#8c93a1` | 6.80:1 | AA |

> **Result: every semantic text and `on-*` pair passes AA in all three themes.** The **lowest text
> pair in the entire system is 5.05:1** (`text-subtle` on `surface-raised`, light), comfortably above
> the 4.5:1 floor. High-contrast is AAA across the board and lifts borders to the 3:1 UI floor.

### 2.4 The focus-ring derivation + the accent-fails-AA carve-out (PROVEN — §8b.3; R-17 §3.2)

The **identity accent** (`accent`) and the **focus/primary token** (`focus-ring`) are **distinct
tokens**. Rule: the accent may carry brand identity at its natural chroma even if it sits near the AA
floor, but **focus and the primary-action fill must ride a *derived*, higher-contrast token** — so
the affordance a keyboard user depends on never rests on a sub-AA colour.

| Theme | `accent` (identity) | `focus-ring` (DERIVED) | derivation | focus measured |
|---|---|---|---|---|
| Dark | `#5b8cff` (6.14:1) | `#7ea6ff` | lightened toward base for headroom | **8.13:1** |
| Light | `#2f6bff` (4.50:1, *exactly* AA) | `#1452d6` | darkened for ≥AA margin + white-on-it ≥AA | **6.55:1** |
| High-contrast | `#93b4ff` (10.21:1) | `#aec4ff` | lightened to AAA | **12.14:1** |

In light, the identity accent lands *exactly* at 4.50:1 — the very case the carve-out exists for: it
is fine as a brand mark (a 2px rail marker, paired with weight, never colour-alone) but it would be a
fragile primary-button background. So `button-primary-bg` → `focus-ring` (`#1452d6`, 6.55:1; white
text on it 6.55:1), **not** → `accent`. One `focus-ring` token renders on every interactive element
(palette row, diff line, board card, editor block, HITL button, overlay control) via `:focus-visible`
in `tokens.css`, and it meets the **3:1 UI floor against every surface in every theme**.

---

## 3. The scales (HOUSE STYLE values; the rules they obey are PROVEN)

### 3.1 Neutral & accent ramp summary
- **Neutral (per theme):** an 8-step ramp `0→7` carrying ~90% of the UI, plus three on-base text steps
  (`txt-1/2/3`). Dark = cool grey on `#0c0d10`; light = cool grey on `#ffffff`; high-contrast = pure
  `#000`→white. (50→950 *sense*: `0` is the lightest-job surface in dark, the page in light.)
- **Accent:** **one** rationed blue family. `identity-*` (brand), `focus-*` (derived AA-safe primary/
  focus, §2.4), `weak-*` (selected/active tint). No second accent — restraint is the product.
- **Functional:** `success`/`warning`/`danger` ramps + `info` (aliased to the derived blue so info ≠
  identity). Muted, never saturated traffic-light fills; always glyph+label (§3.4).
- **Agent:** a reserved **violet** ramp — a *fourth neutral semantic axis*, never a status colour.

### 3.2 Type scale (~1.2 ratio, shared platform-wide)
`caption 12 · body-sm 13 · body 14 · h3 16 · h2 20 · h1 24 · display 30 · code 13`.
Line-heights `tight 1.3 / body 1.45 / reading 1.6` — **body ≥1.45 for diacritic headroom** (never
1.0; clips Greek tonos / Cyrillic breve / Latin-ext caron — R-18 §3.2). Families: **UI sans + load-
bearing mono**, self-hosted, Latin-ext + Greek + Cyrillic coverage a *selection gate*; Arabic/Hebrew
for RTL is a `[VERIFY]` before build. Hierarchy from **weight & colour before size** (§8b.3).

### 3.3 Spacing & radius
- **Spacing:** 4px base ramp `0,4,8,12,16,24,32,48,64`. Every margin/padding/gap is a step — no
  off-ramp magic numbers (5/7/13px is the amateur tell).
- **Radius:** near-zero — `0, 3 (default), 6, 10, pill`. Sharp = fast read (Instrument character).

### 3.4 Elevation (borders-first; shadow reserved)
Hairline (1px) borders do nearly all grouping. Shadow exists only for genuinely floating layers:
`shadow.popover` (menus/dropdowns/hovercards), `shadow.overlay` (palette/dialog). Status is **never
colour-alone** — glyph + label + position carry it (WCAG 1.4.1); no saturated fills.

### 3.5 Motion (R-12)
Durations `instant 0 · micro 90 · fast 140 · base 180 · deliberate 240`. Easings `standard / enter /
exit / emphasized (reserved agent) / linear`. **No spring/bounce primitive.** Functional band is
120–200ms; `deliberate` (240, the only token >200) is reserved for notice-without-interrupt
(live-update, agent-enter). **`prefers-reduced-motion` zeroes every duration** as a first-class path
(reduced-motion loses the *animation*, never the *information*; state still flips + announces).

### 3.6 Z-index (one scale)
`base 0 < chrome 100 < popover 200 < modal 300 < toast 400`. All overlays portal to the document root;
per-component magic z-index numbers are banned (§8b.1).

---

## 4. Accessibility properties baked into the tokens (PROVEN)
- **Measured-not-claimed:** every ratio above recomputed from hex; CI re-runs this over the *generated*
  table (a brand accent at ~2.8:1 fails AA — none here does).
- **focus-ring ≠ identity token**, AA-safe, one token everywhere, ≥3:1 in all three themes (§2.4).
- **Status not by colour alone**, no saturated fills; **agent treatment is a fourth neutral axis**,
  always label "Agent" + plain geometric mark + attribution (never sparkle/emoji — §8b.3).
- **High-contrast is an authored theme** *and* `tokens.css` ships a `@media (forced-colors: active)`
  fallback so focus + status survive the UA stripping author colours (the §4 gap-#3 capability).
- **Logical-property-friendly:** focus offset/width are logical-safe; components must use
  inline-start/end so `[dir="rtl"]` mirrors with no override sheet (R-18 §4.1).

---

## 5. Themes — how they are expressed
DTCG **theme sets** under `$themes` in `tokens.json`: `dark` (default = the resolved semantic tier),
`light`, `high-contrast` — each an **override map** re-pointing the semantic aliases at a different
primitive ramp. Style Dictionary emits one `:root[data-theme="…"]` block per theme into `tokens.css`.
Switching theme = setting `data-theme` on the root; no component change.

---

## 6. Consumption rules (for 8b components & the styleguide)
1. Read **semantic** vars only (`--surface`, `--text-primary`, `--focus-ring`, `--success`, …) or the
   component handles (`--c-btn-primary-bg`). Never a primitive hex, never an inline colour on an
   interactive element (inline style beats `:hover`/`:focus` specificity — §8b.3).
2. Spacing/radius/motion/z-index from the scale vars only.
3. Status = token **+** glyph **+** label. Agent = `--agent` **+** "Agent" label **+** plain mark.
4. Use logical properties throughout (RTL is free).

---

## 7. Parameterization note — the direction swap (the load-bearing requirement)

**The direction-parameter is the SEMANTIC tier** (00-plan §2.1). A different finalist (B/C/D/hybrid)
is **a new `tokens.json` whose semantic aliases re-point at that direction's primitives** — Style
Dictionary regenerates `tokens.css`, the measured-contrast gate re-validates, and **every component
re-skins with no component edit** (components bind only to semantics; Tier-3 binds only to Tier-2).

| The swap touches… | The swap does NOT touch… |
|---|---|
| Primitive ramps (neutral temperature, accent character, radius, type personality, motion magnitudes) | Any component code |
| Semantic alias targets (what `surface`/`accent`/`focus-ring`/… resolve to) | The semantic **token names** (the contract) |
| The 3 theme override maps for the new direction | The tier architecture, the focus≠identity rule, the a11y floor |

Worked example — **A → D "Civic"**: point the build at `finalist-D-civic/tokens.json` (sober
institutional blue, warm-neutral ramp); the contrast gate re-validates D's pairs; flip the six variant
flags to D's row (`density: medium`, `nav: rail`, `surfaceUnification: one-skin`, `tone: sober`,
`agentPresence: ambient`, `sovereigntyVisibility: always-on`). Done — no component rewrite (00-plan
§2.3). Behaviour/layout differences that aren't a colour/space value live in the **six variant flags**
(00-plan §2.2), never in a `switch(direction)` inside a component.

> Whatever the chosen direction, the **invariants hold**: three tiers, components consume only
> semantics, `focus-ring` is a derived AA-safe token distinct from `accent`, and every text/UI pair is
> measured against AA in light/dark/high-contrast before it ships.

---

*End. Tokens: PROVEN contrast math & standards; HOUSE STYLE values & direction-A character. Three
themes' semantic text pairs all pass AA (lowest 5.05:1). Self-hosted, no CDN. Not committed.*
