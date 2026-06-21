# Re-authoring pass — strok upgraded primitives (notch / per-corner round-corners / arc bulge / audit / diff / batch sprite+manifest)

> Conservative pass: improve `.strok` source clarity + add design-system pipeline outputs
> **without visual regressions**. Every geometry change was confirmed with `strok diff`
> against the pre-change render (256px). Date: 2026-06-21. Branch: `design-planning`.

## What changed

### 1. Arc icons — explicit `bulge=right` (geometry identical, source clearer)
The four arc-heavy icons relied on **point order** to choose which side each arc bulges
(the exact DX pain in `STROK-FEEDBACK.md` #1). Every arc in them resolves to SVG sweep
flag `1`, which the new `bulge=right` modifier expresses explicitly. Re-authored with
`bulge=right` on each `mode=arc` point:

| icon | arc points annotated | `strok diff` vs before (256px) |
|---|---|---|
| `database` | 3 (cylinder bottom + 2 bands) | changed_fraction **0.0**, within_tolerance **true** |
| `link` | 2 (both hooks) | changed_fraction **0.0**, within_tolerance **true** |
| `rerun` | 4 (full ring) | changed_fraction **0.0**, within_tolerance **true** |
| `cycle` | 4 (full ring) | changed_fraction **0.0**, within_tolerance **true** |

Byte-identical renders — pure source-readability win (intent no longer hidden in point
order). Shipped `svg/` arc paths are unchanged (verified: `A… 0 0 1 …` preserved).

### 2. Manifest annotations — all 42 sources
Prepended `# @meaning …` + `# @tags …` header comments to **every** `.strok` (meanings
from the ICONS-README §2 registry; tags = searchable synonyms + group). Drives
`batch --manifest`: all 42 manifest entries now carry a non-empty meaning + tags.

## What was tried and DELIBERATELY left untouched (couldn't match within tolerance)

| icon | attempted | result | decision |
|---|---|---|---|
| `message` | rounded-rect body + `notch edge=bottom shape=triangle` tail | changed_fraction **0.062**, within_tolerance **false** (exit 1). The original tail is a *separate floating open stroke* below the bubble; a notch welds the tail into the closed outline AND `round-corners` then rounds the notch tip into a mangled nub. Visibly worse. | **Keep original** (hand-composed floating tail). |
| `nav-chat` | same notch-tail idea | same construction as `message` — same failure mode | **Keep original.** |
| `folder` | `rectangle` + `notch edge=top shape=square` tab | changed_fraction **0.070**, within_tolerance **false** (exit 1). The folder "tab" is a tall L-stepped raised section, not a small edge protrusion; the notch produces a short box with a tiny bump — a different glyph. | **Keep original** hand-stepped path (`round-corners 1.5`). |
| `doc`, `tag`, `inbox` | (per-corner round-corners / notch fold) | already clean hand-composed geometry; the new primitives wouldn't simplify without shifting the look | **Keep original.** |

Honesty note: the `notch` primitive welds onto a *closed* outline and is then subject to a
following `round-corners`, so it does not reproduce our "separate overlapping open tail"
construction (message/nav-chat) or a tall stepped silhouette (folder). The hand-composed
sources are correct; we kept them.

## 3. `strok audit` — clean
All 42 sources report **"no suggestions"** (no `RoughCatmull`/mirror/dead-feature flags).
Earlier passes had already migrated off `catmull-rom`, so nothing to fix. Re-confirmed
clean after the bulge + annotation edits.

## 4. Pipeline outputs (build.sh)
`build.sh` now additionally emits, alongside the unchanged `svg/` + `preview/` + the live
`icons-index.html` contact sheet:
- `dist/sprite.svg` — `<symbol>` sprite (currentColor preserved, 42 symbols) for
  `<use href="sprite.svg#name"/>`.
- `dist/manifest.json` — `{version, count:42, icons:[{name, meaning, tags, canvas, sizes}]}`.

Sprite/manifest are generated from the same parsed set as the shipped SVGs (one `strok
batch`), so they can't drift. Build is **idempotent** (two consecutive runs → identical
output hash). `svg/` precision reformatted slightly (the current strok binary emits more
decimal places, e.g. `5.1193`→`5.11929`) — same geometry, a binary serialization detail,
not a re-author change.

## Verification (all green)
- 42 `svg/*.svg`, all `currentColor`, **no baked hex** (build.sh fails loudly otherwise).
- `dist/sprite.svg`: 42 `<symbol>`s, no baked hex, currentColor inherited.
- `dist/manifest.json`: count 42, **0** empty meanings.
- `strok audit`: clean across all 42.
- `build.sh`: idempotent (hash-stable across runs).
