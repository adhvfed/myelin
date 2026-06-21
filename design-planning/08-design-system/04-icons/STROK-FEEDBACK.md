# Strøk — feedback from building Myelin's icon library

> Captured 2026-06-20 after authoring 42 icons across 5 build/refine passes. For the Strøk
> maintainers (`~/Projects/strøk`). Tag: PROVEN (hit it firsthand) vs SUGGESTION (our opinion).

## Resolved by rebuilding (features existed in source, not in the installed binary)
The installed `~/.local/bin/strok` (built Jun 4) predated commit *"Add icon-authoring support:
currentColor, new --profile icon, batch render."* Rebuilding (`cargo install --path strok-cli
--force`, then sync the binary to `~/.local/bin`) gave us:
- **`currentColor`** as a real `fill`/`stroke` value, preserved verbatim in exported SVG — **this
  removed our whole `$ink`-sentinel-hex → `sed` post-process workaround.** (PROVEN: the first three
  build passes had to fake it.)
- **`new --profile icon`** — seeds the 24×24 grid + `stroke currentColor` defaults. Great.
- **`strok batch <dir>`** — themeable SVG + multi-size PNG in one shot; replaced our hand-rolled
  per-file `build.sh` loop. Great.
- **DX gap, not a bug:** the build/install path is `cargo install` → `~/.cargo/bin`, but our PATH
  used `~/.local/bin`. The `rsinstall` wrapper syncs them; a plain `cargo install` does not.
  *SUGGESTION:* mention this in CLAUDE.md / `--help` ("ensure ~/.cargo/bin or sync to ~/.local/bin").

## Remaining DX / capability findings
1. **`mode=arc` bulge depends on point order** (PROVEN — cost real iteration). To flip which side an
   arc bulges, authors must reverse the point order. We hit this on `database` (bottom + bands),
   `link` (hooks), and `rerun`. *SUGGESTION:* an explicit `sweep=cw|ccw` (or `bulge=left|right` /
   `large-arc`) modifier on `addpoint … mode=arc`, defaulting to current behavior so existing files
   are unaffected. Highest-value remaining DX win for icon authoring.
2. **The catmull-rom trap** (PROVEN — root cause of every "rough around the edges" icon). Agents
   reached for `mode=catmull-rom` to draw *geometric* shapes (rounded rects, circles, arches,
   cylinders); threaded through near-collinear points it produces wavy edges / faceting visible at
   large size. The fix was always to use the right primitive (`rectangle round-corners`, `ellipse`,
   `mode=arc`, `mode=sharp`). *SUGGESTIONS (low-risk, additive):* (a) help-text/DSL-doc steering —
   "use catmull-rom only for organic curves; for UI geometry use round-corners / arc / sharp"; (b) an
   **`audit` heuristic** that flags `catmull-rom` runs through near-collinear points (the exact trap).
   This prevents the whole roughness class for future authors.
3. **Rounded-rect-with-notch/tail is hand-composed** (SUGGESTION, minor). Folder tabs, speech-bubble
   tails, doc corner-folds were each a rounded-rect body + a separate `sharp` path, anchored by hand.
   A small helper (e.g. `round-corners` honoring a per-corner spec, or a `notch`/`tail` op) would cut
   boilerplate — but the manual composition works and reads fine, so this is low priority.
4. **No built-in contact sheet** (SUGGESTION, minor). For reviewing a whole icon set we used external
   ImageMagick `montage`. A `strok batch … --sheet contact.png` (grid of all icons) would be handy.

## Net
The big DX win (currentColor + batch) was latent and is now installed. The one genuinely useful
*new* capability is the **arc `sweep` control (#1)**; the highest-leverage *quality* safeguard is the
**catmull-rom guidance + audit heuristic (#2)**. Both are additive and shouldn't change existing
renders. Items 3–4 are nice-to-haves.
