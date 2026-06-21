# Icon Library — Iteration 1 build notes

> **Stage 2 of 5** (requirements → **ITERATION 1** → review-refine ×3). Built with `strok`.
> Date: 2026-06-20. Builds the **CORE 42** set from `00-requirements.md §1.1` against §2–§4.

## What was built

- **42 / 42 CORE icons** authored as `strok/<name>.strok`, rendered to `preview/<name>.png`,
  and exported + post-processed to `svg/<name>.svg` (`currentColor`). All §1.1 names exactly.
- **`build.sh`** — idempotent, name-agnostic. Loops over every `strok/*.strok`, renders a PNG,
  exports a raw SVG, and `sed`-swaps the `$ink` sentinel hex (`#ff00ff`) → `currentColor`.
  Skips `_*.strok` includes. Fails loudly if any sentinel hex survives. Re-running is a no-op diff.
- **`icons-index.html`** — self-contained contact sheet: all 42 inline SVGs on a neutral panel,
  `color:` set on the grid container to prove `currentColor` inheritance, plus a **size slider**,
  an **accent-color toggle** (recolors the whole set live), and a **dark-bg toggle**. No remote refs.

### Authoring model (verified, for the refine passes)
- Shared header in every file: `documentsize 24x24` + a `defaults` block (`fill none`,
  `stroke $ink`, `stroke-width 2`, `stroke-linecap/linejoin round`) + `palette { ink #ff00ff }`.
- `ellipse`/`rectangle`/`triangle` shapes are sized via `place … at=X,Y size=WxH` (they scale to the
  place box — e.g. a centered ⌀20 circle is `at=2,2 size=20x20`).
- `path` shapes carry **absolute 24-grid coordinates** in their `addpoint`s and are placed with
  `at=0,0` (this preserves coordinates exactly; placing a path `at=x,y size=…` rescales it — avoid).
- Curves use per-point `mode=catmull-rom` / `mode=arc` (NOT a standalone `mode` line — that errors).
- `round-corners N` on a rectangle scales with the place box (an 18px box with `round-corners 3`
  renders r≈2.25). Tune at the source size if exact radius matters.
- All geometry kept inside the 2,2 → 22,22 live area (2px padding).

## currentColor verification — **PASS**

- `grep -L 'stroke="currentColor"' svg/*.svg` → empty (every file has it).
- `grep -l '#' svg/*.svg` → empty (no hardcoded hex anywhere in shipped SVG).
- `fill="none"` preserved on every stroke shape; the only intentional `fill="currentColor"` is the
  centered status/identity **dot** in `agent`, `nav-issues`, `kebab`, `settings` knobs, `priority`
  (deliberate filled dots, allowed by §3.1 / criterion 5).

## Agent mark — compliant

`agent` is a **plain rounded-square outline with a single centered filled dot**. No sparkle, no
magic-wand, no star, no emoji (design-language §8b.3 / requirements §2.4). Recognisable in pure
monochrome; sits as a clear "different kind of principal" beside the `human` head-and-shoulders mark.

## Branch vs merge (the named glyphs) — differentiated

Both share the node vocabulary (⌀6 nodes, a left rail). `branch` shows the line **diverging out**
from the rail up to the branch node; `merge` shows the branch line **converging back into** the rail.
The curve direction is the distinguishing read. Refine pass should sanity-check they stay distinct at 16px.

## Icons I'm least happy with (priority targets for refine passes)

1. **`cycle`** — the arrowhead on the ring reads as a small flag rather than a clean directional cap.
   Redraw as a proper open-ring + chevron arrowhead (align with `rerun`'s grammar).
2. **`link`** — the two chain "C"s + diagonal bar read a touch like a single rounded bar at small size.
   Tighten the ring curvature / increase the gap so the chain-link metaphor is unmistakable at 16px.
3. **`pull-request`** — busy (left rail, 3 nodes, converging arc + arrowhead). Reads as PR but is the
   densest glyph in the set; consider simplifying to fewer nodes for optical weight parity.
4. **`inbox`** — the tray reads cleanly but is close to a "download/monitor" silhouette; the refine
   pass should confirm it's unambiguous against any future `download`/`export` backlog glyph.
5. **`database`** — the body is two stacked curve bands + sides; verify the lower curve depth is
   optically balanced with the top ellipse at 16px.
6. **Optical-weight pass (whole set):** filled dots (`agent`, `nav-issues`) vs all-outline glyphs —
   check none reads heavier at 16px. `channel` (4 strokes) and `priority` (3 bars) are the lightest;
   `run`/`nav-ci`/`agent` (enclosed) the heaviest. Tune in refine pass 1 per criterion 4.

## Deferred / not built

- **None of the CORE 42 are missing** — all 42 are present, rendering, and shipping `currentColor`.
- Extended backlog (§1.2, ~50 icons) intentionally **not** built — out of scope for iteration 1.
- HOUSE-STYLE taste calls left for the refine passes per §6: `linecap round` vs `butt`, and the exact
  agent silhouette. Current set commits to `round` throughout for consistency.
- No `strok emit react` / sprite sheet generated — the shipped contract is the per-file `svg/` set
  (framework plan §1.5). A sprite is a refine-pass decision (§4.5).

## Acceptance-criteria status (self-check against §5)

| # | Criterion | Status |
|---|-----------|--------|
| 1 | All 42 CORE exist (strok+svg+png, exact names) | PASS |
| 2 | Renders at 24px; legible at 16px | PASS (a few weak at 16 — see above) |
| 3 | Grid/geometry: 24×24, 20×20 live area, sw2, fill none, round caps/joins | PASS |
| 4 | Optical alignment / one weight family | MOSTLY — flagged outliers for refine |
| 5 | `stroke="currentColor"`, `fill="none"`, no hex | PASS (verified) |
| 6 | Agent: no sparkle, plain, mono-recognisable | PASS |
| 7 | No emoji / per-theme / per-direction / animated | PASS |
| 8 | Stable name→meaning (one chevron rotated, link≠external-link) | PASS |
| 9 | Self-contained / no-CDN | PASS |
| 10 | `build.sh` reproducible / no-op re-run | PASS |
