# `strok` Icon Library — usage & reference

> The Myelin product icon set. **42 CORE icons**, one monochrome outline style, built with the
> `strok` vector CLI. Authored on a 24×24 grid, shipped as inline SVG that inherits `currentColor`.
> This is the library's entry doc; the full spec is [`00-requirements.md`](./00-requirements.md),
> the acceptance sign-off is [`ACCEPTANCE.md`](./ACCEPTANCE.md).

---

## 1. What you get

| Path | What it is |
|---|---|
| `strok/<name>.strok` | **Source of truth** — one `.strok` per icon. Edit these. |
| `svg/<name>.svg` | **Shipped artifact** — post-processed inline SVG with `stroke="currentColor"`. The app imports these. |
| `preview/<name>.png` | PNG preview (render-loop output / styleguide swatch). Generated. |
| `build.sh` | The one build step: render previews + export SVG + swap the `$ink` sentinel → `currentColor`. |
| `icons-index.html` | Live contact sheet — renders every shipped SVG, proves `currentColor` inheritance and recolor. |

**SVGs and PNGs are generated. Never hand-edit them — edit the `.strok` and re-run `build.sh`.**

---

## 2. The icon registry (name → meaning is a contract)

One canonical glyph per meaning, identical across every subsystem (design-language §3.7). The filename
**is** the contract key — never rename by appearance or call-site.

| name | meaning | group |
|---|---|---|
| `nav-code` | Code subsystem | Subsystem / nav |
| `nav-ci` | CI subsystem (circle + play) | Subsystem / nav |
| `nav-issues` | Issues subsystem | Subsystem / nav |
| `nav-knowledge` | Knowledge subsystem | Subsystem / nav |
| `nav-chat` | Chat subsystem (stacked bubbles) | Subsystem / nav |
| `inbox` | Notifications ("what needs me") | Subsystem / nav |
| `search` | Global / scoped search | Subsystem / nav |
| `settings` | Settings / admin (sliders) | Subsystem / nav |
| `branch` | Git branch / ref (high horizontal split) | Git / CI |
| `merge` | Merge — the named glyph (deep downward convergence) | Git / CI |
| `commit` | Commit (node on a rail) | Git / CI |
| `pull-request` | PR — open-state combine mark | Git / CI |
| `tag` | Git tag / release | Git / CI |
| `run` | CI run / workflow (rounded square + play) | Git / CI |
| `rerun` | Re-run a check / job (circular arrow) | Git / CI |
| `check-pass` | Check passed — ✓ in status ring | Git / CI |
| `check-fail` | Check failed — ✗ in status ring | Git / CI |
| `check-pending` | Check pending / in-progress — clock in status ring | Git / CI |
| `issue` | Issue / work item | Issue / work |
| `sub-issue` | Sub-issue / child | Issue / work |
| `priority` | Priority marker (bars) | Issue / work |
| `cycle` | Cycle / sprint (loop + arrowhead) | Issue / work |
| `roadmap` | Roadmap / timeline lens | Issue / work |
| `repo` | Repository | Objects |
| `file` | File | Objects |
| `folder` | Folder / space | Objects |
| `doc` | Doc / page — the named glyph | Objects |
| `database` | Database view (cylinder) | Objects |
| `channel` | Chat channel — the named glyph (#) | Objects |
| `message` | Message / comment (single bubble) | Objects |
| `link` | Link / reference / backlink (chain) | Objects |
| `human` | Human principal / avatar fallback | Principals |
| `agent` | **Agent mark — plain geometric: rounded square + centered dot** | Principals |
| `team` | Team principal (two people) | Principals |
| `approve` | Approve a proposed effect — **bare ✓** (HITL action) | Agent / HITL |
| `edit` | Edit a proposed effect / edit affordance (pencil) | Agent / HITL |
| `reject` | Reject a proposed effect — **bare ✗** (HITL action) | Agent / HITL |
| `gate` | HITL gate / awaiting approval (lock) | Agent / HITL |
| `chevron` | Disclosure / next-prev — one glyph, rotated by CSS | Chrome |
| `kebab` | Overflow / more actions (3 dots) | Chrome |
| `close` | Dismiss / remove — bare ✗ (chip/dialog corner) | Chrome |
| `external-link` | Open in full / navigate away (↗) | Chrome |

### Glyph-family conventions (read these before adding similar icons)

- **Ring = CI status verdict.** Only the `check-*` trio (`check-pass` ✓, `check-fail` ✗,
  `check-pending` clock) is ring-enclosed, so they read as one matched set in a CI rows column. A bare
  ✓/✗ is therefore *not* a CI verdict.
- **Bare ✓/✗ = action / chrome.** `approve` (bare ✓) and `reject` (bare ✗) are HITL card buttons;
  `close` (bare ✗) is a dismiss affordance. `reject` and `close` share the bare-✗ mark by design (dismiss
  and reject are the same gesture, the universal convention) and never co-occur — `reject` sits beside
  `approve` on a HITL card; `close` sits in a chip/dialog corner. This is a deliberate context-disambiguated
  unification, not an accidental duplicate. (See `ACCEPTANCE.md` for the full rationale.)
- **`chevron` is one glyph, rotated.** Up/down/left/right are CSS `transform: rotate(...)`, never four
  files. Same rule for any future directional pair — use `flip=x` / CSS, not a redraw.
- **`link` vs `external-link`** are distinct: chain (reference) vs navigate-away arrow.

---

## 3. The visual spec (at a glance)

| Property | Value |
|---|---|
| Canvas | `documentsize 24x24` → `viewBox="0 0 24 24"` |
| Live area | 20×20, **2px padding** — all geometry inside `2,2 → 22,22` |
| Stroke width | `2` (one nominal weight, never mixed) |
| Fill | `none` (outline style). Only intentional filled marks: the small solid dots in `agent`, `nav-issues`, `kebab`, `settings` |
| Terminals | `stroke-linecap round` — **locked** (the §6 taste call; round reads calm beside the near-zero-radius UI) |
| Joins | `stroke-linejoin round` |
| Paint | `stroke="currentColor"` in shipped SVG (see §4) |
| Smallest render | 16px — every glyph is verified legible at 16px |

Keyline shapes (⌀20 circle, 20×20 square, 16×20 / 20×16 rect) keep all glyphs at one optical weight
despite different silhouettes. Character is **Instrument**: precise, geometric, restrained — instruments,
not illustrations.

---

## 4. The build step — `$ink` → `currentColor`

`strok` cannot emit `currentColor` (it resolves color tokens to hex and rejects the literal
`currentColor`). So every icon authors its stroke as a single **sentinel palette token** `$ink` bound to a
reserved hex used nowhere else:

```
palette
  ink #ff00ff        # sentinel — swapped to currentColor at build
```

`build.sh` is the only non-`strok` step (~2 lines of real work). For every `strok/<name>.strok` it:

1. renders `preview/<name>.png`,
2. exports a raw SVG (sentinel hex still present),
3. `sed`-swaps `#ff00ff` → `currentColor` and writes `svg/<name>.svg` (the shipped file; `fill="none"`
   survives untouched),
4. fails loudly if any sentinel hex leaks into a shipped file.

**To (re)generate everything:**

```bash
cd design-planning/08-design-system/04-icons
bash build.sh
```

It loops over whatever `.strok` files exist (no hardcoded names), skips `_*` includes, and is
**idempotent** — re-running produces a no-op diff. Requires `strok` on `PATH` and `sed`.

---

## 5. How the set is consumed

- **Inline SVG inheriting `currentColor`** (framework plan §1.5; self-hosted, no CDN). The wrapping
  element's CSS `color` recolors the icon — one drawing serves dark / light / high-contrast and the
  `--agent` token. There are **no per-theme or per-direction icon files.**
- **Sizes 16 / 20 / 24** are set by CSS `width`/`height` on the consuming `<svg>` — *not* a second
  drawing. The 24-grid source is the only source; small rendering is scaling. (Shipped SVGs keep a default
  `width`/`height="24"` plus `viewBox`, so they render at 24 if unsized; CSS overrides it.)
- Recolor is proven by `icons-index.html`: its container sets `color:` and every inline SVG follows,
  including a non-ink accent row to show it tracks any color, not just text ink.
- The agent treatment is **never colour-alone** — the `agent` icon carries the *mark* channel of the
  four-channel contract (label "Agent" + mark + `--agent` token + attribution). It must read in pure
  monochrome, which it does.

---

## 6. Adding a new icon

1. `strok new strok/<name>.strok 24x24`, then paste the shared header (`documentsize 24x24` + the
   `defaults` block: `fill none`, `stroke $ink`, `stroke-width 2`, `stroke-linecap round`,
   `stroke-linejoin round`) + `palette { ink #ff00ff }`. Copy an existing icon as a template.
2. Use **kebab-case, meaning-named, subsystem-agnostic** filenames (`pull-request`, not `pr-header-icon`).
   The filename is the contract key — add the row to the §2 registry too.
3. Build geometry **inside the 2,2 → 22,22 live area**. Reuse the family vocabulary (⌀6 node, ⌀4 dot, the
   status ring for CI verdicts only, one arrowhead size). Snap to the pixel grid.
4. `strok -f strok/<name>.strok render --out preview/<name>.png` and **look at it** at 16px too — iterate.
5. `bash build.sh` to ship the `svg/`. Verify `grep -L currentColor svg/*.svg` is empty and no `#` remains.
6. Add a `<figure>` cell to `icons-index.html` (inline the shipped SVG, drop `xmlns`/`width`/`height`).
7. Backlog icons (see `00-requirements.md` §1.2) inherit this *identical* spec — they are more of the same
   set, never a second style.
