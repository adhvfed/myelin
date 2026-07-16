# R3 sketch — Repo browsing completeness (G-1 / G-2 / G-3)

> Surface: repo home · tree-at-path · blob · commit log · error trio · honest-rail/NotAvailable.
> Implementation-ready screen spec. Dissolves ux-git findings **4, 6, 7, 8, 10, 12, 13** and
> firstrun finding **1**. Direction A "Instrument", frozen 08 design system + REFINEMENTS R1.
> Sketches link `../tokens.css`; test all three `data-theme`s (dark default / light / high-contrast).

Files:
- `sketch-repo-home.html` — G-1: title + visibility chip, ref switcher (open popover shown), branches/tags counts, clone, latest-commit bar, tree with **clickable dirs** + per-row activity, **full README render**, activity panel; + empty & skeleton.
- `sketch-tree.html` — G-2 tree-at-path: ref+every-segment breadcrumb, parent-dir row, dir rows (deeper) vs file rows (blob); + empty-dir & skeleton.
- `sketch-blob.html` — G-2 blob: full path breadcrumb, Raw + Download + **Blame (deferred slot)**, code view; + **binary → download fallback**, large/truncated file, skeleton.
- `sketch-commit-log.html` — G-3: ref-carrying breadcrumb (no hardcoded `main`), **prev/newer + older pager with position feedback**; + first-page boundary, empty, skeleton.
- `sketch-error-states.html` — the shared **no-access / not-found / retryable-error** trio + status→state map (the R-21 pattern, spec'd once, reused by every route above).
- `sketch-not-available.html` — catch-all + unbuilt-subsystem NotAvailable **inside the shell** + the **honest rail** treatment.

---

## 1. IA + routes (SolidStart, under `frontend/apps/web/src/routes/`)

All under the shell group `(app)/`. `[...path]` = catch-all (nested segments); this is the core routing
change vs today (the current blob route matches a **single** segment — ux-git finding 4).

| Route | Purpose | Status |
|---|---|---|
| `(app)/git/repos/[repo]/index.tsx` | G-1 repo home | EXISTS — extend (switcher, full README, clickable dirs, activity) |
| `(app)/git/repos/[repo]/tree/[ref]/[...path].tsx` | G-2 tree-at-path (nested) | **NEW** |
| `(app)/git/repos/[repo]/tree/[ref]/index.tsx` (or `[...path]` matching empty) | tree root at a non-default ref | **NEW** (folds into the catch-all) |
| `(app)/git/repos/[repo]/blob/[ref]/[...path].tsx` | G-2 blob (nested path) | EXISTS as `blob/[ref]/[path]` — **widen `[path]` → `[...path]`** |
| `(app)/git/repos/[repo]/commits/[ref].tsx` | G-3 commit log | EXISTS — add prev cursor + position + ref-true breadcrumb |
| `(app)/git/repos/[repo]/commit/[oid].tsx` | G-3 commit detail (diff) | EXISTS — out of this surface's build scope; breadcrumb ref fix noted |
| `(app)/[...404].tsx` | catch-all → `<NotAvailable>` in shell | **NEW** (firstrun finding 1) |
| `(app)/issues/index.tsx`, `chat/…`, `ci/…`, `knowledge/…` | unbuilt subsystem indexes → `<NotAvailable>` | **NEW** (thin; render NotAvailable) |

**Ref carried through every route** as the `[ref]` segment (branch name, tag, or oid). The switcher on
home/tree/blob/commits rewrites the current route's `[ref]` and preserves `[...path]`. **Kill hardcoded
`'main'`**: repo-home links (`Commits`, tree/blob) and the commit breadcrumb use `default_branch` from the
VM (finding 6), and every deeper surface uses the ref it was reached with (breadcrumb never resets to main).

**Breadcrumb contract** (findings 13, 6): `Repositories / {repo} / {ref-pill} / seg / seg / …` — the repo,
the ref, and **every** path segment are individual links (ref → tree root at ref; each segment → tree at
that sub-path). Blob's last segment is the current file (not a link, `aria-current`). LTR/bidi-isolate the
mono path segments in RTL.

---

## 2. Full state set per surface (R-21 §5.3)

Sketched states are marked ✎. Six-plus states; where a state doesn't change layout it reuses a shared
component rather than a bespoke screen.

| State | Repo home | Tree | Blob | Commit log |
|---|---|---|---|---|
| populated | ✎ | ✎ | ✎ (text) | ✎ (page 2) |
| empty | ✎ (no commits, onboarding-forward) | ✎ (empty dir) | n/a (a file always has bytes; 0-byte → "empty file" line) | ✎ (empty ref) |
| loading/skeleton | ✎ | ✎ | ✎ | ✎ |
| no-access (403) | shared trio | shared trio | shared trio (+ "part restricted" is a diff/PR concern, not blob) | shared trio |
| not-found (404 / bad ref/path) | shared trio | ✎-described | shared trio | shared trio |
| error (5xx/network, retryable) | shared trio | shared trio | shared trio | shared trio |
| **binary / large-file** | — | — | ✎ **download fallback + truncated** | — |
| stale/reconnecting | activity panel only (live push) → out of scope, noted | — | — | live "new commits above" → out of scope, noted |
| moved/outdated | — | — | — | — |

- **no-access vs not-found vs error** are the DISTINCT dignified trio → `sketch-error-states.html`; the
  frontend maps status → `kind` and **never renders `err.message`** (findings 7). Anti-oracle: policy may
  make no-access indistinguishable from not-found (no existence leak).
- **empty** teaches the next action (R-20); repo-home empty keeps the existing clone/push block.
- **skeleton** sets `aria-busy` + is structure-matching (no spinner, no "Loading…" text — the current
  routes' `<p>Loading…</p>` fallbacks are replaced). One debounced polite live region announces settle.
- **binary/large-file** (blob, finding 10): never `contents.split('\n')` a binary → garbled dump. Detect
  server-side (`is_binary`), render the download fallback; large/truncated text shows a head + a
  "download full file" affordance.

---

## 3. Data contract — EXISTING vs NEW (the backend work order)

Tagged against `frontend/apps/web/src/lib/api.ts`. **EXISTING** = already on a VM. **NEW** = proposed
endpoint/field the build wave must add.

### Repo home — `RepoHomeVM` (extend)
- `EXISTING:RepoHomeVM.state` ("populated"|"empty"|"restricted"), `.slug`, `.clone_url`, `.entries[]` (`RepoEntry{path,is_dir}`)
- `EXISTING:RepoHomeVM.readme_excerpt` → **replace/augment** with `NEW:RepoHomeVM.readme` (full markdown string; rendered via the BlockEditor read-path, not `<pre>` — G-1 "README render, editor read-path")
- `NEW:RepoHomeVM.default_branch` (string) — **kills hardcoded 'main'** (finding 6). Drives Commits/tree/blob links + breadcrumb.
- `NEW:RepoHomeVM.latest_commit` `{short_oid, oid, summary, author, committed_at}` — the "latest commit" bar.
- `NEW:RepoEntry.latest_commit` `{short_oid, summary, committed_at}` + `NEW:RepoEntry.name` (basename; today `path` is used as both) — the per-row "Letzte Änderung / Zuletzt geändert" activity columns. *(If per-entry last-commit is too costly at v1, degrade: drop the two activity columns, keep name only — the tree still works. Flagged as an open question.)*
- `NEW:RepoHomeVM.counts` `{branches, tags}` — the "4 branches · 2 tags" affordance.
- `NEW:RepoHomeVM.visibility` ("private"|"internal"|"public") — the header visibility chip (R-19 §1.2).
- Activity panel: reuse `NEW:` a small `activity[]` feed OR reuse the commit-log head N — **open question** whether activity is commits-only (cheap) or multi-kind (branch/tag/CI events).

### Ref switcher — **NEW endpoint** `GET /v1/git/repos/{repo}/refs`
- `NEW:RefsVM { branches: RefRow[], tags: RefRow[], default_branch: string }`, `RefRow { name, oid, is_default? }`.
- Drives the Menu+filter combobox on home/tree/blob/commits. Permission-pre-filtered (only refs the viewer may see).

### Tree-at-path — **NEW endpoint** `GET /v1/git/repos/{repo}/tree/{ref}/{...path}`
- `NEW:TreeVM { path: string, ref: string, entries: TreeEntry[], readme?: string }`
- `NEW:TreeEntry { name, path, is_dir, size?, mode?, latest_commit? }` — dir rows link to `tree/…`, file rows to `blob/…`.
- Root call = empty `{...path}`; returns top-level (same shape the repo-home tree uses — share the projection).

### Blob — `BlobVM` (extend for binary/large-file)
- `EXISTING:BlobVM.path`, `.contents`, `.base_oid`, `.viewer_may_edit`
- `NEW:BlobVM.is_binary` (bool) — gate the download fallback (finding 10).
- `NEW:BlobVM.size_bytes` (number) + `NEW:BlobVM.is_truncated` (bool) + `NEW:BlobVM.shown_lines` — large-file head + "download full".
- `NEW:BlobVM.raw_url` (string) — the Raw affordance (server streams with a safe content-type; never inline-executed).
- `NEW:BlobVM.language`/`.mime` — language label + syntax hinting (hinting itself can be client-side; optional).
- `NEW:BlobVM.download_url` — Download button (may equal raw_url with `Content-Disposition: attachment`).
- **Blame** = named follow-on, **deferred, not R3 scope**. The slot is present (disabled "soon" button). Eventual `NEW: GET /v1/git/repos/{repo}/blame/{ref}/{...path}` → per-line `{oid, short_oid, author, committed_at, pr_number?}` powering the W5 backlink trail (blame→commit→PR→issue). Do not build now; keep the slot so the header layout is stable when it lands.

### Commit log — `CommitsPage` (extend for bidirectional paging)
- `EXISTING:CommitsPage.items[]` (`CommitRowVM{oid,short_oid,summary,author,committed_at,parents}`), `.page.next_cursor`, `.page.limit`
- `NEW:CommitsPage.page.prev_cursor` (string|null) — the "Neuere/Newer" link (finding 12). Alternatively drive prev from URL history, but an explicit prev_cursor is cleaner and back-button-independent.
- `NEW:CommitsPage.page.offset` (or `range:{from,to}`) — the "Commits 31–60 · Seite 2" position feedback. **No fabricated total** unless cheap; if total is expensive, show "Seite 2" + range only (honest, no fake N).
- `EXISTING:CommitRowVM.parents.length>1` → the merge badge (already used). Signature glyph "signed" is `NEW:CommitRowVM.signature` ("valid"|"none"|"invalid") — glyph+label, never colour-alone (G-3, G1).

### Error mapping — frontend, no new endpoint
- Shared `mapEdgeError(status|VM.state) → 'no-access' | 'not-found' | 'error'` + `<RepoErrorState kind={…}>`. `RepoHomeVM.state:"restricted"` already exists → maps to `no-access`. `401` stays the central `api.ts` redirect to `/login` (unchanged). **Stop rendering `String(err.message ?? err)`** at all five current fallbacks (findings 7).

---

## 4. Keyboard map + screen-reader behaviour

**Landmarks (every surface):** `header[banner]` (shell), `nav[aria-label=Primary]` rail, `main`,
`nav[aria-label=Breadcrumb]`, content `section`s with `aria-labelledby`. Skip-link to `main` (shell owns).

**Global:** `⌘K`/`Ctrl-K` command palette (shell). Focus ring is the token's `:focus-visible` on every
interactive element (tokens.css already ships it) — no `outline:none` anywhere in these sketches.

**Ref switcher (Menu + filter = combobox):**
- Trigger: `button aria-haspopup="listbox" aria-expanded`. `Enter`/`Space`/`Down` opens.
- Open: focus the filter `<input role="combobox" aria-controls=<listbox> aria-expanded="true">`; the two
  groups are `role="listbox"` (or one listbox with `role="group"` sections). Roving via
  `aria-activedescendant` on the highlighted `role="option"` — **no focus leaves the input** (P3 pattern,
  matches command-palette). `Up/Down` move active option across both groups; `Enter` selects & navigates;
  `Esc` closes and returns focus to the trigger; typing filters both lists live. Default branch marked with
  a persistent "default" chip + the selected option carries `aria-selected="true"` (a check glyph, not
  colour-alone). Announce "Branches, N results / Tags, M results" politely on filter.

**Tree:** rows are a plain list of links (dir vs file by leading icon + trailing `/`), natural tab order;
`Enter` follows. Parent-dir row (`..`) is `aria-label="Up to parent directory"`. SR reads
"folder, crates" / "file, web.rs" via `aria-label` on the icon-bearing link (icon has a text alternative,
not just decorative) so type is announced, not inferred from colour.

**Blob:** the `<pre>` is `aria-label="File contents"`; line-number gutter is `aria-hidden` (decorative) so
SR reads source lines only. Binary fallback is a `role="note"` region; the Download/Raw are ordinary links.
Blame slot: `aria-disabled="true"` + a `title`/visually-hidden "coming soon" so it's discoverable but
inert. Large-file notice is inline, not an alert.

**Commit log:** `<ol>` list; each row's oid link is the primary target. Pager is `nav[aria-label]`; the
position readout is `aria-live="polite"` so paging announces "Commits 31–60, page 2". First-page "Newer" is
`aria-disabled` (present, not removed) — the boundary is legible.

**Error trio:** `no-access`/`not-found` = `role="note"` (calm, not an error to fix); `error`
(retryable) = `role="alert" aria-live="assertive"` with a Retry button that keeps context. Never announce
raw error text.

**NotAvailable / rail:** unbuilt rail items are real links (keyboard-reachable, **never disabled** — P3),
`title` tooltip; the "soon" dot is decorative (the tooltip + destination copy carry the meaning, not the
dot's colour). NotAvailable is `role="note"` with `aria-labelledby` the heading; primary action "Go to Code".

**i18n/RTL:** logical properties throughout (`inset-inline-*`, `padding-inline`, `border-inline-*`); mono
runs (paths, oids, refs) bidi-isolated LTR inside RTL prose; German strings present per surface
(`Verzweigung wechseln`, `Zuletzt geändert`, `Vorschau nicht verfügbar — Binärdatei`, `Neuere/Ältere ·
Seite`, `Etwas ist schiefgelaufen`, `Bald verfügbar`) to prove the +30–40% expansion budget; dates via
`Intl`/CLDR in the viewer locale (sketch strings are illustrative).

---

## 5. Component reuse

**Existing design-system primitives (02-components):**
- **Dropdown/Menu overlay** (`overlays.md`) → the **branch/tag switcher**. It is a Menu with an embedded
  filter `<input>` — i.e. the combobox pattern (listbox + `aria-activedescendant`), built on the overlay
  substrate (focus-trap, Esc, portal, `shadow-popover`, the one z-scale). **Not** the ⌘K command palette
  (that is global nav/act/search) — the switcher is a scoped, in-context picker. **Which primitive:
  Dropdown/Menu (overlays.md), upgraded with a filter input → combobox semantics.**
- **Reference chip / identity badge** — commit author, latest-commit, activity rows render the `Principal`
  badge (pseudonymised author, R-04 §3.2); oids/paths that point at artifacts are `ArtifactRef` chips where
  applicable. Sketch approximates with mono links; the build uses the real chip.
- **Block editor read-path** (`block-editor.md`) — the **full README render** runs the one-render-path
  (`render(parse(md))`), NOT a `<pre>` dump (the current code's `readme_excerpt` in `<pre>` is replaced).
- **Overlay Toast** — clone-URL "copied" toast (already wired).
- **Icon** — by registry name only, from the 42-set: `nav-code, nav-issues, nav-chat, nav-ci,
  nav-knowledge, repo, folder, file, doc, branch, tag, commit, merge, chevron, gate, check-pass,
  check-fail, link, search, inbox, human, external-link`.

**Shared primitives to confirm/add (check REFINEMENTS/02-components first):**
- **Skeleton** — used on all four surfaces; expected shared primitive (brief's list). Sets `aria-busy` +
  the one polite live region.
- **Chip / StatusPill** — visibility chip, "default" ref chip, "signed" signature badge, "soon" tag,
  merge/pagination badges. Expected shared (brief's list).
- **Button** — Raw/Download/Retry/switcher-trigger; primary CTA rides `--c-btn-primary-bg` (Retry, Go to Code).
- **`<RepoErrorState kind>`** (**NEW shared component, named + justified**): the no-access/not-found/error
  trio. Justified because five routes need the identical mapping and copy; a shared component is the only way
  the R-21 distinction is enforced uniformly and `err.message` never leaks. Lives in the git surface but is a
  thin composition of Icon + Button + the state-card shell — contribute down if a second subsystem needs it.
- **`<NotAvailable>`** (EXISTS `components/NotAvailable.tsx`) — **upgrade** from a one-line note to the
  teaching state (heading + "soon" tag + Go-to-Code CTA), and mount it via the catch-all + subsystem
  indexes inside the shell.

**New icons to name (not in the 42-set; register per §3.6 before build, don't draw ad hoc):**
- `download` — the binary/large-file/blob Download affordance (no existing glyph; `external-link` is "open
  raw", semantically distinct). **Proposed NEW icon `download`.**
- (Reused, no new icon: Raw = `external-link`; Blame = `human`; parent-dir = `folder` variant / `chevron`.)

---

## 6. Rail honesty argument (within the manual's rails)

Unbuilt destinations (Issues/Chat/CI/Knowledge) are: **muted** (`--text-subtle`, one step below the
`--text-muted` of a built-inactive item) **+ a neutral "soon" dot + a `title` tooltip**, and **still real,
keyboard-reachable links** that land on the teaching `<NotAvailable>`. Argued against the manual:
- **Not disabled** — P3 (keyboard-first, never keyboard-only) and honesty: the destination is declared, so
  it must be reachable; a dead/disabled icon is less honest than a reachable "not yet" page.
- **Not accent** — accent is rationed (§3.1, R1); a "coming soon" is not important enough to spend it.
- **Not colour-alone** — the dot is a neutral `--text-subtle`, and the *meaning* is carried by the tooltip +
  the destination page copy ("Bald verfügbar"), satisfying WCAG 1.4.1.
- **Active state still R1** — even when an unbuilt item is the current route, active = `--surface-hover`
  fill (no colored side-bar), text stays muted to keep the "soon" read.

---

## 7. Open questions (for the orchestrator gate)

1. **Per-entry last-commit cost.** The tree's "Letzte Änderung / Zuletzt geändert" columns need a
   last-commit-per-entry walk, which is expensive on large trees. Ship it (like GitHub, with a bounded/async
   fill), or degrade v1 to name-only rows and add activity columns later? Sketch shows the richer version.
2. **Activity feed scope.** Is repo-home "Aktivität" commits-only (cheap, reuse the log) or multi-kind
   (branch/tag created, CI passed, PR opened)? Multi-kind needs a new cross-subsystem feed the other
   subsystems don't emit yet.
3. **README render path now vs later.** Full markdown render requires the BlockEditor read-path (WASM
   parser) to be wired on this surface. If that's not ready in the build wave, is a sanitized-HTML interim
   acceptable, or do we keep `readme_excerpt` in a styled block until the editor lands? (Sketch assumes full
   render.)
4. **Position feedback without a total.** "Commits 31–60 · Seite 2" implies an offset; a true total commit
   count may be costly. Acceptable to show page + range only (no "of N"), or is "of N" required? (Sketch
   avoids a fake total.)
5. **Ref switcher at scale.** For repos with thousands of branches, does `GET /refs` paginate/server-filter
   the combobox, or is client-filter over a capped list enough for v1? Anti-oracle pre-filter must hold
   either way.
6. **Raw/Download auth + residency.** `raw_url`/`download_url` must go through the same permission gate and
   stay in-region (no CDN of private bytes, P2/ADR-11). Confirm these are gateway-proxied, not signed public
   URLs.
7. **`tree/` vs `blob/` disambiguation.** With `[...path]` catch-alls, a path that is a file requested under
   `tree/` (or a dir under `blob/`) should redirect to the correct sibling rather than 404 — confirm the
   edge returns an `is_dir` hint so the route can redirect instead of showing not-found.
8. **Blame deferral line.** Confirmed **not R3 scope**; the slot is designed (disabled "soon"). Gate check:
   is the disabled affordance acceptable, or should the slot be fully absent until blame ships? (Sketch keeps
   the slot for layout stability + discoverability.)
