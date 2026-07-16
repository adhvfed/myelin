# R3 sketch — PR list + navigation front door (NOTES)

> Surface: **PR list + navigation front door** — dissolves `reviews/2026-07-06/ux-ux-git.md`
> **critical #1** ("Pull requests are undiscoverable — no PR list and no link from anywhere").
> Implements the missing **front door** to G-6 (the wedge flagship, `git.md`). Direction A
> "Instrument", FROZEN (08 design system + 6c finalist-A). Not committed — the orchestrator commits.
>
> Sketches in this dir (link `../tokens.css`, the generated semantic tokens):
> - `sketch-repo-prs-populated.html` — per-repo index `/git/repos/{repo}/prs`, populated.
> - `sketch-cross-repo-prs.html` — cross-repo front door `/prs`, "needs your review" + "your PRs".
> - `sketch-empty-and-skeleton.html` — empty(teaching) · skeleton · no-access · error · filtered-no-results.

---

## 1. IA + routes (SolidStart, under `frontend/apps/web/src/routes/(app)/`)

| Route | File (NEW) | Screen | Purpose |
|---|---|---|---|
| `/git/repos/[repo]/prs` | `git/repos/[repo]/prs/index.tsx` **NEW** | repo PR index | the per-repo list; the sibling of the existing `commits/[ref].tsx`. |
| `/git/repos/[repo]/prs/[n]` | `git/repos/[repo]/prs/[n].tsx` (EXISTS) | PR overview | already built (G-6, partial); this sketch links **into** it. |
| `/prs` | `(app)/prs/index.tsx` **NEW** | cross-repo front door | "needs your review" + "your PRs" buckets across repos. |

- Filter/sort/bucket state lives in the **query string** (`?state=open&sort=updated&author=…&cursor=…`)
  so a view is deep-linkable and shareable (shell §5 "deep-linkable URLs"). The tab is `state`;
  the bucket on `/prs` is `?bucket=needs-review|yours|mentioned`.
- The per-repo index is a **contextual-sidebar** surface (no context pane — a list needs none; the
  pane belongs to G-6 the PR *overview*). The cross-repo `/prs` reuses the same shell with a
  bucket/scope sidebar.

### Nav change — explicit, mirrors the existing Commits link (the review's prescribed fix)

The review says: *"link to it from the repo home header and app nav, mirroring the Commits link at
`index.tsx:66`."* Concretely:

1. **Repo home header (primary per-repo entry).** In `git/repos/[repo]/index.tsx`, next to the
   existing `<A href=".../commits/main">Commits</A>` (line 66), add
   `<A href="/git/repos/{repo}/prs">Pull requests</A>`. Same visual treatment (`--text-primary`
   inline-flex link). This is the one-line change that makes PRs reachable at all.
2. **Repos-list / Code landing header (cross-repo entry).** On `/git/repos` add a header link
   **"Your pull requests"** → `/prs`. This is the front door for the "what needs me" job without
   forcing a repo choice first.
3. **Inbox rows become real links.** `AppShell.tsx:280-282` renders "A pull request needs your
   review" as a static `<span>`. Make it an `<A>` to the specific PR (`/git/repos/{repo}/prs/{n}`);
   the inbox's "PR" section header links to `/prs?bucket=needs-review`.
4. **Command palette.** Add `Go to pull requests` (repo-scoped, when inside a repo) and
   `Pull requests needing my review` / `My pull requests` (→ `/prs?bucket=…`) to `commands()`.

**Argued placement — do NOT add a 6th rail item.** The rail is the *subsystem* switcher
(Code · Issues · Chat · CI · Knowledge — one shell invariant, shell-and-nav §1). PRs are **part of
Code**, not a sixth subsystem; a rail glyph for a cross-cutting saved filter is the anti-pattern
(P1 "one product, not five tools", and it would crowd the 48px rail). Instead:
- per-repo PRs live **in the Code subsystem sidebar** (this sketch's left sidebar: Open / Your PRs /
  Needs review / Merged / Closed), the subsystem-owned contextual nav;
- the cross-repo `/prs` is reached as a **Code-landing destination + inbox + ⌘K** (an attention
  surface belongs to the inbox by P8, with `/prs` as its browsable full view).
- **Open question (Q1):** whether `/prs` also deserves a topbar affordance (a small "PRs · 3"
  counter beside the inbox) is a taste call left for the gate — floor is the four entries above.

---

## 2. Full R-21 state enumeration

| State | Sketched? | Behaviour |
|---|---|---|
| **Populated** | ✅ screens 1 & 2 | list-row molecule per PR; buckets on `/prs`. |
| **Empty (first-use)** | ✅ screen 3 | teaching: *"No open pull requests / Create one by pushing a branch, then opening a pull request from it into `main`."* + the exact `git switch -c … / git push -u origin …` snippet + ⌘K/`gh` paths. Distinct from filtered-no-results. |
| **Loading** | ✅ screen 3 | structure-matching **skeleton rows** (pill + title + subline + right-meta ghosts), `aria-busy="true"`, ONE debounced polite live region ("Loading pull requests…"); NO spinner, NO "Loading…" body text (manual §5.3, must-ship #4). Suppress flash <~1s. |
| **Error** | ✅ screen 3 | system-blaming one line ("We couldn't load pull requests"), **scoped to the list**, filters kept, retry; distinct from no-access; never a raw `err.message` (fixes ux-git #7). |
| **No-access (403-analogue)** | ✅ screen 3 | dignified "Pull requests are not available to you" — never leaks the count/existence of PRs. Policy's choice (Restricted) vs Absent (a 404 not-found repo → the repo route's own not-found). |
| **No-results (filtered)** | ✅ screen 3 | distinct from empty; permission-honest — "no results" never reveals hidden matches exist; offers Clear filters. |
| **Pagination** | ✅ screen 1 | cursor **prev/next** (Newer disabled at head) + position hint "Showing 1–6 of 6 open" (fixes ux-git #12's one-directional log). |
| **Erased / tombstoned author** | ⬜ not sketched | an author erased under GDPR renders the identity badge as **"[erased user]"** (badge spec owns it); the row still lists. No PII. |
| **Agent-pending** | ⬜ not sketched | a PR an agent is acting on (e.g. an agent-opened PR awaiting a HITL gate) shows the agent-pending marker on the row; the badge already carries the agent treatment (screens 1 & 2 show an **agent author**). Full agent-pending row is G-6/inbox territory. |
| **Stale / live-update** | ⬜ not sketched (specced) | a bus-pushed change (a PR going green, a new review, a merge) updates the row **in place** via `--dur-deliberate` `motion.liveUpdate`, no scroll-jump, no loss of keyboard selection; announced politely only if watched. |
| **Degraded** | ⬜ not sketched (specced) | if the checks projection is unreachable, the **checks-summary glyph** shows a neutral "checks unavailable" (a `check-pending`-style dash), the rest of the row stays live — the row **fails static**, never blanks. (This is the row-level analogue of ux-git #5: a checks failure must NOT masquerade as no-PR.) |
| **Cross-cell** | ⬜ not sketched (specced) | on `/prs`, a PR in another residency cell carries a T3 provenance tag on its repo chip, else it is **absent** (no leak). |

---

## 3. Data contract — EXISTING vs NEW (the backend work order)

**The query logic EXISTS; the edge GET does not.** `crates/myelin-git/src/list_filter.rs` already
ships `compose_pr_list_query(set_expr, viewer, tenant, region)` and the frozen
`PR_LIST_PERMISSION = "view"` (§5.3 push-down; lowered over `pr.id`). The list is **leak-free by
construction**: the ACL predicate from `list_objects(viewer, view, pull_request)` is conjoined into
the `WHERE` **before** `ORDER BY`/`LIMIT` — a PR the viewer can't `view` never survives the scan
(never a post-filter). **NEW work is only: (a) an edge GET that calls this + projects a row VM, and
(b) the per-row fields the VM needs.**

### NEW endpoint

```
GET /v1/git/repos/{repo}/prs?state=open|merged|closed|all&sort=updated|created&cursor={c}&limit={n}
  → { items: PrListRowVM[], page: { next_cursor: string|null, prev_cursor: string|null, limit: number } }
```
- MUST drive the list through `repo_authz` / `compose_pr_list_query` (the leak-free prefilter) — the
  edge returns only rows that survive `list_objects(viewer, "view", pull_request)`; **counts, cursors
  and the tab badges are computed over the prefiltered set** (a forbidden PR never contributes to a
  count — the anti-oracle rule).
- Cross-repo front door NEW endpoint: `GET /v1/git/prs?bucket=needs-review|yours|mentioned&…` — the
  same prefilter, plus a bucket predicate (`reviewer = viewer` / `author = viewer`). Same envelope,
  each row additionally carries `repo` (the repo chip).

### `PrListRowVM` (proposed) — field provenance

| VM field | Source | Notes |
|---|---|---|
| `number` | EXISTING `PrVM.number` | monospace `#48`. |
| `title` | **NEW** `PrListRowVM.title` | **no title field exists anywhere yet** (ux-git #3 confirms `PrVM` has no `title`/`body`). Needs a PR title store — R3 backend census item. |
| `pr_state` | EXISTING `PrVM.pr_state` (`draft`/`open`/`merged`/`closed`) | drives the state pill glyph+label. |
| `base_ref` / `head_ref` | EXISTING `PrVM.base_ref` / `PrVM.head_ref` | the `head → base` refs (monospace, bidi-isolated). |
| `author` | EXISTING `PrVM.author` (string) | **NEW**: upgrade to a `PrincipalRef` so the row renders the shared **identity/agent badge** (human vs agent four-channel). Today `author` is a bare string; an agent author needs `kind: "agent"` + attribution. See `crates/myelin-git/src/agent_author.rs` (agent-authorship exists server-side). |
| `reviews` (count) | EXISTING `PrVM.reviews` (number) | **NEW** on the row: a `review_state` enum (`requested`/`approved`/`changes`/`none`) + `you_are_requested: bool` so the "needs your review" bucket + the quiet "review requested" marker render. |
| `checks_summary` | **NEW** `PrListRowVM.checks_summary` | `{ verdict: "pass"|"fail"|"running"|"none", failing: n, total: n }`. Derivable from the EXISTING `PrChecksVM` projection (`green_contexts`/`required_contexts`/`gate_admitted`) but MUST be **rolled up per-PR in the list query**, not N+1 per-row `getPrChecks` calls. `gate_admitted` stays authoritative for merge readiness (UI never recomputes). |
| `updated_at` | **NEW** `PrListRowVM.updated_at` (unix secs) | formatted client-side via `Intl` (never hand-formatted — manual §6). |
| `repo` (cross-repo only) | **NEW** `PrListRowVM.repo` | the repo slug for the repo chip on `/prs`. |
| `page.prev_cursor` | **NEW** | the envelope today (`CommitsPage`/`ReposPage`) exposes only `next_cursor`; add `prev_cursor` for the bidirectional pager (fixes ux-git #12). |

### `frontend/apps/web/src/lib/api.ts` additions (NEW)

- `interface PrListRowVM { … }` (above), `interface PrListPage { items: PrListRowVM[]; page: {…} }`.
- `export const getRepoPrs = query(async (input:{repo, state, sort?, cursor?}) => …, "git-prs")`.
- `export const getMyPrs = query(async (input:{bucket, cursor?}) => …, "git-prs-cross")`.
- All through the existing `authed()` wrapper (401 → `/login` unchanged).

---

## 4. Keyboard map + SR behaviour

### Keyboard — argue `j/k`

- The list is a **`<Views>` list projection**; its composite-focus model is the manual's PROVEN one
  (roving `tabindex`, one Tab stop for the whole list, arrows re-rove). **Recommend `j`/`k` as
  aliases for `ArrowDown`/`ArrowUp`** (P3 keyboard-first; the board/list already spec `j/k` in
  `views.md` §3). Rationale: rows are homogeneous and reviewers page through many PRs — muscle-memory
  transfer from the board is the whole "Instrument" thesis. **But `j/k` is an *alias*, never the only
  path** — arrows + Tab + click all work (keyboard-first ≠ keyboard-only). `Enter`/`o` opens the
  focused PR; `x` (later) toggles multi-select for a future bulk action. **Open question (Q2):**
  whether a bare-key `o`/`x` is worth it before multi-select exists — floor is arrows/`j`/`k` + Enter.
- The **filter tabs** are an ARIA `tablist` (`role=tab`, `aria-selected`, roving arrow-key movement,
  the tab controls the list below). Sort is a `Menu` (⏎/Space opens, arrows move, Esc closes).
- Pager prev/next are ordinary links/buttons; `Newer` is `aria-disabled` at the head (not removed).
- **No trap** anywhere — Tab always exits the list to the next shell region.

### Screen-reader / landmarks

- Landmarks from the shell: `banner` (topbar), `nav[aria-label="Surfaces"]` (rail),
  `aside[aria-label="Pull requests in {repo}"]` (sidebar), `main#main`. The list is
  `ul[role=list][aria-label="Open pull requests, 6 items"]`.
- **Status announced as TEXT, never colour** (WCAG 1.4.1): every state pill and checks-summary glyph
  carries a `title` + visible label ("Open", "1 failing", "all passing", "no checks"). The agent
  author announces "Agent" (the four-channel label channel).
- **One debounced polite `role="status"` live region** (shell-hosted) announces: the result summary
  on load ("6 open pull requests, sorted by recently updated"), tab changes ("Now showing merged, 128
  items"), and background live-updates ("#48 checks passed") — debounced, never per-tick.
- Skeleton: `aria-busy="true"` on the list + the live region says "Loading pull requests…" once.

---

## 5. Component reuse (`design-planning/08-design-system/02-components/*`)

| Component / primitive | How this surface uses it | New? |
|---|---|---|
| **Navigation shell** (`shell-and-nav.md`) | the whole frame (topbar/rail/sidebar/content). **Active rail = `--surface-hover` fill + brighter text + accent-tinted glyph — NOT an accent fill** (R1 binding). ⚠️ `AppShell.tsx:226-228` currently sets the active rail to `background: var(--accent)` / `color: var(--on-accent)` — that is the **R1 violation** this sketch corrects; wiring should fix it. Context pane absent (a list needs none). | reuse (+1 fix) |
| **Views** — LIST projection (`views.md`) | the PR list IS the list projection of one query AST; rows are list-row molecules; empty vs filtered-no-results are its distinct states; permission-denied rows **absent by pre-filter**. The buckets on `/prs` are saved views (query + grouping). | reuse |
| **Identity / agent badge** (`identity-and-agent-badge.md`) | the author cell — human badge (avatar initials + name) and the **agent four-channel treatment** (label "Agent" + plain geometric mark + `--agent` + attribution) for agent authors (PR #46). Erased author → "[erased user]". | reuse |
| **Reference chip** (`reference-chip-and-unfurl.md`) | the **repo chip** on cross-repo rows (compact density); the `head → base` refs are ref-styled monospace. | reuse |
| **Overlays** — Menu (`overlays.md`) | the Sort dropdown (`Menu`), the ⌘K palette (existing `CommandPalette`). | reuse |
| **StatusPill** | the **state pill** (open/merged/closed/draft — glyph+label) and the **checks-summary** chip. **Candidate NEW shared primitive** — `views.md` status cells + G-9 checks + this surface all need one glyph+label pill; it is not yet in `02-components/`. Propose `StatusPill` (variants: `pr-state`, `check-verdict`) so the ring stays reserved for the CI verdict trio and PR-state uses non-ring glyphs. **Check REFINEMENTS first** — none conflicts. | **NEW (propose)** |
| **Skeleton** | the loading rows. Also not yet a named `02-components/` primitive though every surface needs it (manual §5.3). Propose `Skeleton` (row/card/bar variants, sets `aria-busy` + wires the polite live region by default). | **NEW (propose)** |
| **Pagination** | cursor prev/next + position. Small enough to be a local molecule; note it so the commit log (ux-git #12) and this list share one. | candidate |

### Icons (42-icon library only — `frontend/packages/design-system/src/icon-names.ts`)

Used (all exist): `pull-request` (open state), `merge` (merged), `close` (closed), `edit` (draft —
interim; see gap), `check-pass`/`check-fail`/`check-pending` (checks-summary — **ring reserved for the
CI verdict trio**, honoured), `commit` (Commits link), `repo` (repo chip), `human`/`agent` (badges),
`message` (review count), `chevron` (sort/pager), `search`, `inbox`, `gate` (no-access lock),
`external-link`.

**Gaps (name them; don't draw ad hoc):**
- **`filter`** and **`sort`** — no core glyph (both are in the USAGE-MAP §C backlog). Interim: text
  label "Sort:" + `chevron`; the filter is a `tablist` (needs no glyph). If a filter glyph is wanted,
  request `filter` from the backlog.
- **Draft PR state** — reusing `edit` reads as "editable" more than "draft". A dashed-circle draft
  glyph (as drawn inline in the sketch) has no registry name; **propose `pr-draft`** (or accept `edit`
  as interim). Named here per the manual's "need a new icon → name it in NOTES" rule.

---

## 6. Open questions for the orchestrator gate

- **Q1 — `/prs` topbar affordance?** Floor: the four entries (repo header, Code landing, inbox, ⌘K).
  A persistent topbar "PRs · N" counter is a taste call — does it earn a slot beside the inbox, or is
  that the inbox's job (P8)? Left open.
- **Q2 — bare-key row actions (`o`/`x`) before multi-select exists?** Floor is arrows/`j`/`k` + Enter.
  `x` (select) has nothing to act on until a bulk action ships; recommend deferring.
- **Q3 — PR `title` store.** No title field exists end-to-end today (`PrVM` has none). The list is
  hollow without it — this is a hard **backend prerequisite** (a title/body store, also needed by
  G-6). Confirm it lands in the same wave, else the list shows only `#number + refs`.
- **Q4 — checks-summary rollup source.** The clean design rolls `checks_summary` into the list query
  (no N+1). If that rollup is expensive, the fallback is a lazy per-visible-row fetch — but that risks
  the ux-git #5 failure mode (a checks error must degrade to "checks unavailable" on the row, never
  blank the row or the list). Confirm the rollup is in scope.
- **Q5 — cross-repo `/prs` residency.** For a multi-region tenant, does `/prs` fan out across cells
  (each cell's own leak-free scan, merged client-side with T3 tags) or stay single-cell? Affects the
  cross-cell row state. `[UNDER-EVIDENCED]` — sovereignty area.
- **Q6 — StatusPill / Skeleton contribution.** Both are proposed as NEW shared primitives here; they
  should be contributed **down** into `02-components/` (not built per-surface) since G-9/views/commit
  log all need them. Confirm ownership so this surface consumes rather than forks.
