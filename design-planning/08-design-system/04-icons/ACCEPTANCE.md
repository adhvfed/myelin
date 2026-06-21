# Icon Library — Acceptance Sign-off (Stage 5 of 5, review-refine pass 3 / final)

> Built with `strok`. Date: 2026-06-20. This is the honest sign-off against
> [`00-requirements.md` §5](./00-requirements.md). Verified after `bash build.sh` on the final sources.
> `.strok` sources are the source of truth; `svg/` + `preview/` are generated. Nothing committed.

## Result: **10 / 10 PASS** (with one consciously-deferred cosmetic nit, §A below)

| # | Criterion | Result | Evidence |
|---|---|---|---|
| 1 | All 42 CORE icons exist as `strok` + `svg` + `preview`, exact §1.1 names | **PASS** | 42/42/42 triplets; every §1.1 name matched (scripted name check, no extras, none missing). |
| 2 | Every icon renders cleanly at 24px AND legible at 16px | **PASS** | Full-set montage at 64px and **20px** (`/tmp/icons-p3.png`, `/tmp/icons-p3-small.png`) reviewed; all glyphs hold their read at 16px, incl. the dense `database` cylinder and airy `channel`/`priority`/`kebab`. |
| 3 | Grid + geometry consistency (24×24, 20×20 live area, sw2, fill none, round cap/join) | **PASS** | All 42 `viewBox="0 0 24 24"`; all strokes `stroke-width="2"`; 109/109 stroked elements `stroke-linecap="round"`; every source carries the identical `defaults` header (scripted `grep -L` found zero deviations); geometry inside `2,2→22,22`. |
| 4 | Optical alignment — one weight/size family at 16px, no outlier | **PASS** | Node circles ⌀6, status dots ⌀4, matched CI ring, parity arrowheads (normalized in pass 2). Final 16px even-weight sign-off on the small montage: reads as one family. `database` stays densest, `channel` airiest — within family tolerance (§A). |
| 5 | SVG inherits `currentColor`, no hardcoded hex; recolor verified dark/light/high-contrast | **PASS** | `grep -L currentColor svg/*.svg` → empty; `grep -l '#' svg/*.svg` → empty (no hex, no `#ff00ff` sentinel). Recolor proven: `icons-index.html` container sets `color:` + accent row; montages on `#202028` and `#f5f5f5` confirm every silhouette holds on dark and light surfaces. Only intentional fills are the solid dots in `agent`/`nav-issues`/`kebab`/`settings`. |
| 6 | Agent icon plain & geometric, no sparkle/star/wand/emoji, monochrome-recognisable | **PASS** | `agent` = rounded-square + centered ⌀4 filled dot. Inspected at 90px (`/tmp/spot.png`): clean geometric primitive, distinct from `human`/`team`, carries the mark channel in pure monochrome. |
| 7 | No emoji, no per-theme/per-direction variants, no animated/decorative glyphs | **PASS** | Single monochrome set; `chevron` is one glyph rotated by CSS (no L/R/U/D files); no emoji, no `switch(direction)`, no animation in any source. |
| 8 | Stable name→meaning map; one canonical glyph per meaning | **PASS** (see C-pass/approve resolution) | §1 registry honoured. The one near-duplicate flagged into pass 3 — `check-pass` ≈ `approve` — is now **resolved** (below). `reject`/`close` share the bare-✗ by *deliberate, documented* unification. |
| 9 | Self-contained / no-CDN | **PASS** | All sources + outputs under `04-icons/`. No remote asset refs: only URL anywhere is the W3C SVG namespace URI (a standard XML identifier, never fetched); no `xlink:href`/`<image>`/`<use>`/`@import`. |
| 10 | Reproducible — `build.sh` regenerates deterministically, re-run is a no-op diff | **PASS** | Two consecutive `build.sh` runs → `diff -r` empty (idempotent); after editing only `approve`/`reject` sources, `git diff` touched exactly those 2 SVGs. No stray `.raw.svg` intermediates. |

---

## The `check-pass` ≈ `approve` resolution (criterion 8, the pass-3 decision)

**Before (pass 2):** both were check-in-a-ring, differing only by a 2px ring radius (16 vs 18) —
imperceptible at 16px, effectively a duplicate glyph with two names.

**Decision — hardened split, per the brief's own guidance:**

- **The status ring is reserved exclusively for the CI verdict family.** `check-pass` (✓ in ring),
  `check-fail` (✗ in ring), `check-pending` (clock in ring) remain ring-enclosed so they read as one
  matched set down a CI rows column. The ring now *means* "this is a check verdict."
- **HITL actions lose the ring.** `approve` is now a **bare ✓**, `reject` a **bare ✗** — clean action
  marks for HITL card buttons, matching the `edit` (pencil) / `gate` (lock) action vocabulary.

This makes `check-pass` (ringed ✓) and `approve` (bare ✓) **genuinely, unmistakably distinct** at 16px,
and gives `approve`/`reject` a matched bare pair. Verified on the HITL+CI row montage (`/tmp/hitl-ci*.png`).

**Consequence — `reject` (bare ✗) == `close` (bare ✗):** kept as a *deliberate context-disambiguated
unification*, the same precedent the brief endorses for close/check-fail. Dismiss and reject are the same
gesture in every major UI (GitHub/Linear), and the two never co-occur — `reject` sits beside `approve` on
a HITL card; `close` sits in a chip/dialog corner. Documented in `ICONS-README.md` §2 so the contract is
explicit, not accidental. This is the one consciously-accepted shared mark; everything else is one glyph
per meaning.

---

## Taste calls locked this pass

- **`stroke-linecap` / `stroke-linejoin`: `round` — LOCKED.** The §6 open HOUSE-STYLE call. Round terminals
  read calm and consistent beside the near-zero-radius Instrument UI and hold at 16px; `butt` was the
  fallback and is not needed. All 42 sources + 109 stroked elements use round uniformly.
- **CI ring vs HITL bare mark — LOCKED** (above).
- **`check-pending` = clock** (not the spec's ◐ suggestion) — clearer; kept from pass 1.

---

## A. Residual nits consciously deferred (not blockers)

1. **`database` is the densest tile; `channel`/`priority` the airiest.** After pass-2 weight normalization
   they read as one family on the contact sheet and at 16px. Lightening the `database` cylinder risked
   losing the band read, so **no micro-nudge applied** — judged within family tolerance. Deferred as a
   purely cosmetic taste item; revisit only if a real CI/sidebar surface shows it dominating.
2. **Shipped SVGs keep `width="24" height="24"` alongside `viewBox`.** The spec lists stripping them as
   *optional*; with `viewBox` present, CSS `width`/`height` on the consuming `<svg>` still controls render
   size (the attributes are just a 24px default fallback). Left in deliberately — safer default, matches
   finalist behavior. A one-line `sed` in `build.sh` can strip them later if the React layer prefers it.

No criterion fails on either. **The library is shippable.**
