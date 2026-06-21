# Phase 8 — Icon Library Requirements (Stage 1 of 5: requirements)

> **Phase:** `08-design-system` · deliverable **04** (icon library) · **Stage 1 — requirements only.**
> **File date: 2026-06-20.** Built with the **`strok`** vector CLI. **This document specifies; it does
> NOT build icons.** Stages 2–5 (first iteration + 3 review-refine passes) build against this spec.
>
> **Inputs:** [design-language §3.7](../../../planning/02-holistic-architecture/design-language.md)
> (one icon set, stable icon→meaning, the named glyphs merge/branch/run/doc/channel/agent), §3.2/§6/§8b.3
> (agent = plain geometric mark, **no** sparkle/magic-wand; **no** emoji as UI); [tokens.md](../01-tokens/tokens.md)
> (the `agent` token, hairline/stroke character, the semantic swap surface); [00-framework-and-buildout-plan §1.5](../00-framework-and-buildout-plan.md)
> (self-hosted, no-CDN; one icon set; **inline SVG inheriting `currentColor`**); the finalist-A skin
> [finalist-A-instrument](../../06-design-sketches/6c-finalists/finalist-A-instrument/) and its 6 screens;
> the surface map [05-user-facing-surfaces](../../05-user-facing-surfaces/); the concurrently-authored
> component specs [02-components](../02-components/) (read-only).
>
> **Tagging (VISION §3 honesty rule):** **PROVEN** = a cited standard, an inspectable artifact value, or
> an existing contract surfaced. **HOUSE STYLE** = our taste/synthesis. **`[VERIFY]`** = confirm at build.
>
> **The load-bearing finding (PROVEN — inspected the finalist screens):** the finalist-A HTML already
> ships its icons as **inline SVG** with `viewBox="0 0 24 24" fill="none" stroke="currentColor"
> stroke-width="2" aria-hidden="true"`. The 24×24 outline-on-currentColor grammar is therefore not a
> proposal — it is what the real screens render today; this library makes it a producible, consistent set.

---

## 0. How to read this document

| § | What it gives you |
|---|---|
| §1 | **The icon inventory** — every icon the product needs, grouped; the **CORE SET** (~40, what iteration 1 builds) vs the extended backlog. |
| §2 | **The visual spec** — 24×24 keyline grid, stroke/terminal/join, optical rules, the Instrument character, the plain agent mark. |
| §3 | **The theming/color contract** — monochrome + `currentColor` inheritance; the **verified** strok approach (a sentinel-token + a 1-line post-process). |
| §4 | **The strok workflow + file layout** — one `.strok` per icon, shared defaults, the render loop, naming, sprite/tokens emit. |
| §5 | **Acceptance criteria** — the checklist the 3 review-refine passes run against. |

---

## 1. The icon inventory

**Naming → meaning is a contract (PROVEN — design-language §3.7/P1):** one canonical glyph per meaning,
identical across every subsystem. The same `merge` glyph appears in git, CI annotations, chat unfurls, and
the palette. An `ArtifactRef` type icon (`page`, `run`, `pr`) is the **same** wherever that artifact is
chipped, unfurled, listed in the palette, or shown in a sidebar tree.

### 1.1 The CORE SET (iteration 1) — **42 icons**

These are the icons the finalist-A screens actually render or that the always-on shell/status/type grammar
requires. **Iteration 1 (Stage 2) builds exactly this set.** (Source: the 6 finalist screens + the
shell-and-nav / reference-chip / identity-badge / views / palette / notifications / agent-HITL component
specs. Icons confirmed *visible in a finalist screen* are tagged **[A]**; the rest are required by the
always-on grammar those screens assume.)

| # | name | meaning | where |
|---|---|---|---|
| **Subsystem / nav (8)** ||||
| 1 | `nav-code` | Code subsystem | rail, palette |
| 2 | `nav-ci` | CI subsystem | rail, palette |
| 3 | `nav-issues` | Issues subsystem | rail, palette |
| 4 | `nav-knowledge` | Knowledge subsystem | rail, palette |
| 5 | `nav-chat` | Chat subsystem | rail, palette |
| 6 | `inbox` **[A]** | Notifications ("what needs me") | top bar, rail |
| 7 | `search` **[A]** | Global/scoped search | top bar, palette |
| 8 | `settings` | Settings / admin | rail, menus |
| **Git / CI (10)** ||||
| 9 | `branch` **[A]** | Git branch / ref | PR head→base, file tree |
| 10 | `merge` | Merge (the named glyph) | PR context, history |
| 11 | `commit` | Commit | history, blame, chip |
| 12 | `pull-request` **[A]** | PR (open-state combine mark) | chip, palette, unfurl |
| 13 | `tag` | Git tag / release | history |
| 14 | `run` **[A]** | CI run / workflow | unfurl, palette, chip |
| 15 | `rerun` | Re-run a check/job | PR checks, run view |
| 16 | `check-pass` **[A]** | Check passed (✓) | diff, PR checks, CI rows |
| 17 | `check-fail` **[A]** | Check failed (✗) | diff, PR checks, CI rows |
| 18 | `check-pending` **[A]** | Check pending/in-progress (◐) | PR checks, CI rows |
| **Issue / work (5)** ||||
| 19 | `issue` **[A]** | Issue / work item | board card, table, chip |
| 20 | `sub-issue` **[A]** | Sub-issue / child | issue detail checklist |
| 21 | `priority` **[A]** | Priority marker | card, detail |
| 22 | `cycle` | Cycle / sprint | board, sprint view |
| 23 | `roadmap` **[A]** | Roadmap / timeline lens | views toggle |
| **Objects (8)** ||||
| 24 | `repo` | Repository | sidebar, chip |
| 25 | `file` | File | tree, code search |
| 26 | `folder` | Folder / space | tree |
| 27 | `doc` **[A]** | Doc / page (the named glyph) | context pane, tree, chip |
| 28 | `database` | Database view | knowledge db |
| 29 | `channel` **[A]** | Chat channel (the named glyph) | sidebar, unfurl |
| 30 | `message` **[A]** | Message / comment | timeline, diff comment, chip |
| 31 | `link` **[A]** | Link / reference / backlink | unfurl, backlinks, external-link reuse |
| **Principals (3)** ||||
| 32 | `human` **[A]** | Human principal / avatar fallback | identity, assignee, mention |
| 33 | `agent` **[A]** | **Agent mark — plain geometric, NO sparkle** | identity, HITL card, attribution |
| 34 | `team` | Team principal | team page, scope |
| **Agent / HITL (4)** ||||
| 35 | `approve` **[A]** | Approve a proposed effect | HITL card, unfurl action |
| 36 | `edit` **[A]** | Edit a proposed effect / edit affordance | HITL card, row actions |
| 37 | `reject` **[A]** | Reject a proposed effect | HITL card |
| 38 | `gate` **[A]** | HITL gate / awaiting approval | per-effect gate, inbox |
| **Chrome (4)** ||||
| 39 | `chevron` **[A]** | Disclosure / next-prev (one glyph, rotated) | trees, palette, accordions |
| 40 | `kebab` | Overflow / more actions | row actions everywhere |
| 41 | `close` **[A]** | Dismiss / remove (× / clear chip) | dialogs, chips, filters |
| 42 | `external-link` **[A]** | Open in full / navigate away (↗) | chips, unfurls |

> **Counts to honour:** CORE = **42** (in the 30–45 band; ~26 are **[A]** confirmed-in-finalist, the rest
> required by the shell/status/type grammar those screens assume). One `chevron` covers up/down/left/right
> by rotation — do **not** author four glyphs (§2.5). `external-link` and `link` are distinct
> (navigate-away arrow vs reference-chain).

### 1.2 Extended backlog (Stages 2–5 may add; not required for iteration 1) — **~50 icons**

Grouped by the same taxonomy. Built on-demand as surfaces beyond the finalist set are sketched.

- **Git/CI actions:** `deploy`, `check-skipped`, `signature-verified`, `branch-protection`, `environment`,
  `matrix`, `secret`, `log`, `step`, `compare`, `fork`, `clone`.
- **Issue/work:** `board`, `list-view`, `table-view`, `calendar-view`, `gallery-view`, `sla`, `portfolio`,
  `dashboard`, `bookmark/watch`, `estimate`.
- **Objects:** `thread`, `comment` (if distinct from `message`), `mention`, `attachment`, `code-block`,
  `reaction`, `unread-dot`, `pin`, `image/media`.
- **Principals:** `service` (the named non-human/non-agent actor; gear-class mark).
- **Status:** `success`, `warning`, `danger`, `info` (standalone semantic glyphs distinct from the CI
  `check-*` set — these are the toast/badge status family).
- **Sovereignty / GDPR:** `residency/region`, `key`, `shield`, `audit`, `erased/tombstone`,
  `visibility-eye`, `lock/unlock`, `share`, `kill-switch`, `budget/meter`, `export`, `dsr`.
- **Chrome:** `drag-handle`, `plus`, `filter`, `sort`, `back`, `help`, `checkbox`, `radio`,
  `toggle`, `hamburger`.

> **Total inventory ≈ 92 icons** (42 CORE + ~50 backlog). Backlog icons inherit the *identical* §2 spec
> and §3/§4 workflow — they are "more of the same set," never a second style.

### 1.3 Out of scope — never build these (PROVEN — design-language §8b.3 / §6.1)

- **Sparkle / shimmer / magic-wand / star "AI" iconography.** The agent mark is plain and geometric.
- **Emoji as UI** — an emoji cannot inherit `currentColor`, re-theme, or mirror RTL.
- **Animated or decorative glyphs** (motion is the token system's job, not an icon's).
- **Per-direction/per-theme icon variants** — one monochrome set serves all themes via `currentColor`
  (§3) and all directions via the semantic swap; never a `switch(direction)` of glyphs.

---

## 2. The visual spec

> **Character (HOUSE STYLE, derived from finalist A "Instrument"):** precise, geometric, restrained.
> Hairline-grade strokes, near-zero radius, no flourish — icons read as **instruments**, not illustrations.
> They sit beside load-bearing monospace and a single rationed accent without competing.

### 2.1 The keyline grid (PROVEN convention; values HOUSE STYLE)

- **24×24 canvas** (`documentsize 24x24`). **PROVEN** as the finalist's real grid — the screens render
  `viewBox="0 0 24 24"`. This is also the dominant open-icon-set convention (Lucide/Feather/Material-24).
- **Live area 20×20, padding 2px** on every edge. All geometry stays inside the 2,2 → 22,22 box; the 2px
  trim keeps glyphs from kissing text and gives optical breathing room at small sizes.
- **Optical sizing:** authored at 24; the screens render it down to **16–13px** inline (`width="12/13"`,
  and a 16×16 inline-glyph variant at stroke-width ~1.6). The library ships **one 24-grid source**; small
  rendering is a CSS `width/height`, not a second drawing. **[VERIFY]** during refine passes that each
  CORE glyph stays legible at 16px (the smallest common render).
- **Keyline shapes (PROVEN icon-grid convention):** align primary forms to the canonical keylines — a
  centered circle ⌀20 (≈r10), a square 20×20, a portrait/landscape rect 16×20 / 20×16 — so all icons feel
  the same visual weight despite different silhouettes.

### 2.2 Stroke, terminals, joins (PROVEN — read from the finalist SVGs; numbers HOUSE STYLE)

- **Stroke width 2 at the 24 grid** (`stroke-width 2`). **PROVEN** — the screens' 24-grid icons render
  `stroke-width="2"`; the 16-grid inline variants use ~1.6–1.7 (a smaller-grid equivalent, *not* a second
  weight). One nominal weight; do not mix.
- **Fill: none.** Icons are **outline (stroke-only)**, not filled silhouettes (`fill none`). **PROVEN** —
  finalist SVGs are `fill="none"`. The lone exception is a deliberately-filled status dot if a glyph needs
  it; default is stroke-only.
- **Terminals: `stroke-linecap round`** (HOUSE STYLE — soft terminals read calmer at small sizes and match
  Instrument's restraint without going playful). **[VERIFY]** against final taste in refine pass 1; `butt`
  is the fallback if round reads too soft beside the near-zero-radius UI.
- **Joins: `stroke-linejoin round`** (HOUSE STYLE; finalist's `6-palette` glyph uses `round`). Consistent
  joins across the whole set.

### 2.3 Optical-consistency rules (HOUSE STYLE; standard icon-grid practice)

- **Optical weight over geometric size:** a circle at ⌀20 and a square at 20×20 look equal-sized; tune so
  no glyph reads heavier/lighter than its neighbors at 16px.
- **One geometric vocabulary:** same corner radius family (near-zero / tight), same angle increments
  (prefer 45°/90°; diagonals consistent), same dot size for status/notification dots.
- **Snap to the pixel grid at 24** so down-scaling to 16 stays crisp; avoid half-pixel stroke centers.
- **Consistent metaphors:** the arrow that means "navigate away" (`external-link`) is the same arrowhead
  everywhere; `chevron` is one shape rotated, never redrawn per direction.

### 2.4 The agent mark (PROVEN constraint; form HOUSE STYLE)

- **A plain geometric mark — NO sparkle, NO magic-wand, NO star** (PROVEN — design-language §3.2/§6.1/§8b.3).
- Recommended form (HOUSE STYLE): a clean geometric primitive that reads as "a distinct kind of principal"
  beside the `human` avatar mark — e.g. a rounded square / hex outline with a single centered dot, echoing
  the finalist's "square + dot" agent glyph. It must be **recognisable in monochrome** because the agent
  treatment is **never colour-alone** (always label "Agent" + mark + `--agent` token + attribution; §3,
  tokens §6). The icon carries the *mark* channel of that four-channel contract.

### 2.5 Tag summary

- **PROVEN:** 24×24 grid, `fill=none`, `stroke=currentColor`, `stroke-width 2`, outline style — all read
  directly from the shipped finalist screens. The agent-has-no-sparkle and no-emoji rules. The icon-grid
  keyline/optical-weight conventions are standard practice.
- **HOUSE STYLE:** live-area 20 / 2px padding, `linecap/linejoin round`, the specific agent silhouette, the
  near-zero-radius geometric character. These are the taste calls the refine passes may tune.

---

## 3. The theming / color contract (CRITICAL)

### 3.1 The requirement

**UI icons are monochrome and inherit color.** The app's design tokens recolor them — the icon never bakes
a theme. In rendered SVG the painted attribute must be **`stroke="currentColor"`** (and `fill="none"`), so
the icon takes the CSS `color` of whatever element wraps it. This is exactly how the finalist screens work
(`stroke="currentColor"`, PROVEN) and what framework plan §1.5 mandates (inline SVG, `currentColor`,
forced-colors fallback for free). One drawing → every theme (dark/light/high-contrast) and the `--agent`
token → no per-theme icon files.

### 3.2 What strok does — VERIFIED (the key investigation)

I ran throwaway strok experiments in `/tmp`. Findings (PROVEN by running the CLI):

1. **strok SVG export resolves color tokens to hex; it does NOT emit `currentColor`.** A `$ink` palette
   token exports as `stroke="#1a1a1a"`. `strok inspect --svg` / `export svg` only ever write concrete hex
   (or `none`).
2. **strok rejects the literal string `currentColor` everywhere** — as a `stroke`/`fill` value and as a
   palette/scheme token value it errors: *"'currentColor' is not a valid color — use #rrggbb or none."*
   So you cannot author `currentColor` directly, and no scheme trick produces it.
3. **Therefore a tiny post-process is required** to ship `currentColor` SVGs. The fix is trivial and
   verified working:
   - Author every icon's stroke as a **single sentinel palette token `$ink`** bound to one reserved
     sentinel hex (e.g. `palette { ink #ff00ff }` — a hex used **nowhere else** in the icon).
   - `strok -f <icon>.strok export svg --out <icon>.raw.svg`
   - Post-process: replace the sentinel hex with `currentColor` and write the shipped file:
     `sed 's/#ff00ff/currentColor/g' <icon>.raw.svg > svg/<icon>.svg`
     (verified: produces `stroke="currentColor"`, `fill="none"` intact). A trivial build script does this
     for the whole `strok/` directory. **Optionally also** strip the fixed `width`/`height` and keep only
     `viewBox` so the consuming `<svg>` sizes by CSS — a second `sed`/script step.

   > **Recommendation (HOUSE STYLE):** use a memorable, never-otherwise-used sentinel hex (`#ff00ff`
   > magenta) so the replacement can never collide with a real value. `fill none` survives untouched.
   > Keep this script in the icon folder (e.g. `build.sh`) — it is the only non-strok step, ~2 lines.

### 3.3 Per-theme rendering is for the styleguide ONLY

- **Shipped icons are theme-agnostic** (`currentColor`) — never per-scheme files.
- strok's per-scheme render (`render --scheme dark`, or render-all) is used **only** to generate the
  **styleguide preview swatches** — e.g. an icon shown on dark/light/high-contrast surfaces in the live
  styleguide to prove it reads in all three. Those PNGs are previews, not artifacts the app imports.
- A define-once `scheme` block in a shared file can set `$ink` to each theme's text color purely so the
  preview PNGs look right; the shipped SVG always goes through the §3.2 sentinel→currentColor path.

---

## 4. The strok workflow + file layout

### 4.1 Directory layout (under `04-icons/`)

```
04-icons/
  00-requirements.md          ← this file (Stage 1)
  strok/<name>.strok          ← ONE .strok source per icon (the source of truth)
  svg/<name>.svg              ← shipped, post-processed, currentColor SVG (what the app imports)
  preview/<name>.png          ← PNG previews (the render-loop output; dark + light for styleguide)
  build.sh                    ← the ~2-line export + sentinel→currentColor post-process (§3.2)
  _defaults.strok             ← (optional) the shared header copied into each icon (see §4.2)
```

### 4.2 Shared defaults (every icon starts identical)

Each `<name>.strok` begins with the same header so the set is mechanically consistent:

```
documentsize 24x24
defaults
  fill none
  stroke $ink
  stroke-width 2
  stroke-linecap round
  stroke-linejoin round
palette
  ink #ff00ff          # sentinel — replaced by currentColor at build (§3.2)
```

- `documentsize 24x24` — the grid (§2.1).
- `defaults` block — **PROVEN working**: applies `fill/stroke/stroke-width/linecap/linejoin` to every shape
  so individual shapes only carry geometry. One change to defaults re-styles every icon.
- `$ink` is the **only** color token any icon references. (`stroke $ink` is in defaults; shapes never name
  a color.)

### 4.3 The render loop (what the Stage 2–5 iteration agents follow)

1. `strok new strok/<name>.strok 24x24` (then paste the §4.2 header), or hand-edit.
2. Build geometry with `strok shape … --template line|path|ellipse|rectangle|…` + `place`, or `addpoint`
   ops with `mode=arc|catmull-rom|controls|sharp`, `round-corners`, `smooth/sharpen` (per `strok --help`
   authoring guidance). Keep all geometry inside the 2,2→22,22 live area.
3. `strok -f strok/<name>.strok render --out preview/<name>.png` — **look at it**, adjust, repeat. Render
   both base and a dark preview for the styleguide (`render` with no `--scheme` emits base + every scheme).
4. `strok -f strok/<name>.strok inspect --svg` — sanity-check the resolved SVG (`fill="none"`,
   `stroke="#ff00ff"`, correct paths).
5. `strok -f strok/<name>.strok export svg --out svg/<name>.raw.svg` then run `build.sh` →
   `svg/<name>.svg` with `currentColor`.
6. `strok -f strok/<name>.strok audit` — optional; catches mirrored-pair / structural simplifications
   (e.g. a left/right glyph that should be one shape with `flip=x`).

### 4.4 Naming convention

- **`kebab-case`, meaning-named, subsystem-agnostic** (`pull-request`, `check-pass`, `external-link`,
  `sub-issue`). The filename IS the contract key in §1 (name→meaning). Never name by appearance
  (`green-check`) or by one call-site (`pr-header-icon`).
- Status/check family uses a consistent stem: `check-pass` / `check-fail` / `check-pending`
  (/ `check-skipped` backlog).
- One glyph that rotates (chevron) ships once as `chevron`; rotation is the consumer's CSS, not new files.

### 4.5 `strok emit` — use it for a tokens/sprite convenience output (HOUSE STYLE)

- The **shipped artifacts are the `svg/` files** (clean inline SVG the React components import — matches
  framework plan §1.5). That is the contract.
- **Optionally** use `strok emit react` per icon (or a wrapper script) to generate typed React icon
  components — convenient given the stack is TS+React, but **only after** the currentColor post-process is
  applied to the emitted output (emit, like export, resolves the sentinel hex, so the same §3.2 replacement
  must run on emitted code). **[VERIFY]** that `emit react` output keeps `stroke` overridable / inherits
  `currentColor` after post-process before relying on it; the plain `svg/` set is the guaranteed path.
- `strok emit tailwind` emits a `@theme` block from the palette — **not relevant** here (the icon palette
  is a single sentinel token; the real token system is the Style-Dictionary pipeline in `01-tokens/`).
- A **sprite sheet** (single `icons.svg` with `<symbol id="…">` per glyph) is a reasonable optional
  ship-format; it is produced by concatenating the post-processed `svg/` files, not by strok. Decide in a
  refine pass whether the app prefers per-file inline SVG (default) or a sprite.

---

## 5. Acceptance criteria (the review-refine checklist)

These become the checklist for the 3 review-refine passes (Stages 3–5). A pass fails if any item fails.

1. **All 42 CORE icons exist** as `strok/<name>.strok` + `svg/<name>.svg` + `preview/<name>.png`, with the
   §1.1 names exactly.
2. **Every icon renders cleanly** at 24px AND is legible at 16px (the smallest common inline size) — checked
   in `preview/`.
3. **Grid + geometry consistency:** every glyph on the 24×24 grid, inside the 20×20 live area (2px padding),
   `stroke-width 2`, `fill none`, consistent linecap/linejoin (§2.2).
4. **Optical alignment:** glyphs read as one weight/size family at 16px; no outlier looks heavier/lighter or
   mis-centered (§2.3). Apply the keyline shapes.
5. **SVG inherits `currentColor`:** every `svg/<name>.svg` has `stroke="currentColor"` and `fill="none"`
   (or an intentional filled status dot), and **no hardcoded hex remains** (grep for `#` finds nothing in
   shipped SVG). Verified by recoloring a sample in dark/light/high-contrast (styleguide).
6. **The agent icon has no sparkle/shimmer/magic-wand/star/emoji**, is plain and geometric, and is
   recognisable in pure monochrome (carries the mark channel of the four-channel agent treatment).
7. **No emoji, no per-theme/per-direction variants, no animated/decorative glyphs** anywhere in the set.
8. **Stable name→meaning map:** the §1 table is the registry; one canonical glyph per meaning; the same
   `ArtifactRef` type icon is identical wherever that type is chipped/unfurled/listed.
9. **Self-contained / no-CDN:** all sources and outputs live under `04-icons/`; nothing fetches a remote
   asset (sovereignty constraint, §1.5).
10. **Reproducible:** `build.sh` regenerates every `svg/` from `strok/` deterministically (export →
    sentinel→currentColor); re-running it is a no-op diff.

---

## 6. Flags for the human

- **The currentColor step is a (tiny, verified) post-process, not native strok.** strok cannot emit
  `currentColor` and rejects it as input; the library depends on a ~2-line `build.sh` (sentinel hex → sed).
  This is robust but worth knowing: the `.strok` files are the source of truth, the *shipped* `svg/` is one
  mechanical step downstream. If a future strok version learns `currentColor`, the post-process drops out.
- **CORE = 42, total inventory ≈ 92.** If 42 is too large for one iteration, the natural sub-cut is the
  ~26 **[A]** confirmed-in-finalist icons first, then the remaining 16 grammar icons in refine pass 1.
- **`stroke-linecap round` vs `butt` is a HOUSE STYLE taste call** to confirm in refine pass 1 against the
  near-zero-radius Instrument UI; everything else in §2.2 is PROVEN from the finalist artifacts.

---

*End of Stage 1. Requirements only — no icons built, nothing committed. Stages 2–5 build the §1.1 CORE set
against §2–§4 and gate on §5.*
