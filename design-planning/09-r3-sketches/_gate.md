# R3.0 sketch-pack gate log (orchestrator)

Per-surface verdict against `_brief.md` rails + `git.md` G-spec DoDs. A surface must be ACCEPTED
here before its build wave opens (VISION §3).

## 01-pr-list — ACCEPTED (2026-07-16)

Rails scan clean (no hex, no inline outline:none, no physical properties, no spinner/Loading-text;
accent limited to glyph tint / tab underline / brand dot). Full R-21 state table incl. specced-not-
sketched states honestly marked. Data contract correctly identifies the leak-free prefilter
(`compose_pr_list_query`) and the anti-oracle count rule. Keyboard j/k-as-alias argument accepted.

Open-question decisions (binding for the build wave):
- **Q1 topbar "PRs · N" counter: NO.** Attention is the inbox's job (P8); the four entries
  (repo header, Code landing, inbox links, ⌘K) are the front door. Revisit post-dogfood.
- **Q2 bare-key `o`/`x`: DEFER.** Arrows/j/k + Enter only; `x` waits for a real bulk action.
- **Q3 PR title/body store: CONFIRMED in-scope.** It is a shared backend prerequisite of R3.1
  and R3.3 — build it once in the first backend wave (title required at create time, body
  optional; store alongside pr record in myelin-git; migration via all_durable_migrations()).
- **Q4 checks rollup: in the list query.** No N+1; per-row degraded state "checks unavailable"
  (fail-static row) if the projection join fails.
- **Q5 cross-repo /prs residency: single-cell for R3.** Multi-cell fan-out deferred with the
  honest floor note; the cross-cell row state stays specced-not-built.
- **Q6 StatusPill + Skeleton: contribute DOWN into frontend/packages/design-system.** Note the
  R3.6 a11y wave is already adding Skeleton there — R3.1 consumes it, does not fork. StatusPill
  lands in design-system with `pr-state` + `check-verdict` variants.
- Icon gaps: accept `edit` as interim draft glyph; request `pr-draft` + `filter`/`sort` in the
  icon backlog (not R3-blocking).

## 04-repo-browsing — ACCEPTED with one gate fix (2026-07-16)

Rails scan: ONE violation found and fixed by the orchestrator in `sketch-repo-home.html` — the ref-
switcher filter input had bare `outline:none`; a `:focus-within` `--focus-ring` ring was added to
the wrapper (must-ship #5 — builders copy this pattern, not the violation). Note: the sketch's
`var(--accent-weak, var(--surface-hover))` — `--accent-weak` does NOT exist in tokens; builders use
`--surface-hover` directly. Everything else clean (0 hex, no physical props, no spinner). Route
table (catch-all `[...path]`/`[...404]`), breadcrumb contract, error-trio mapping, and the
EXISTING/NEW data contract are build-ready.

Open-question decisions (binding):
- **Per-entry last-commit columns: in scope via one bounded walk** (single history walk, capped;
  entries not resolved within the cap render name-only — graceful degrade, no N-walks).
- **Activity panel: commits-only for R3.** Multi-kind (branch/tag/CI events) is a named follow-on.
- **README full render: in scope.** Use the BlockEditor read-path if it renders markdown today;
  else a sanitized markdown renderer as the named floor (never raw HTML injection, never `<pre>`).
- **Commit-log position: range + page only ("31–60 · Seite 2"), no fabricated total.**
- **tree/blob kind mismatch: client redirect to the correct route.**
- **Raw/Download: gateway-proxied, in-region, `Content-Disposition: attachment` — BINDING
  (sovereignty rail; no public signed URLs).**
- **Blame slot: present-disabled "soon"** (consistent with NotAvailable honesty); blame itself
  stays a named non-R3 follow-on.
- **Icon: approve `download` for registration** via the 04-icons pipeline (manifest+sprite), not ad hoc.

## 03-pr-diff — ACCEPTED (2026-07-16)

Rails scan clean (hex hits are `#211`-style PR/run numbers, not colors). Route split
(`prs/[n]/index.tsx` + `prs/[n]/diff.tsx`), N1–N4 data contract, R-21 table, keyboard/SR map all
build-ready. `<DiffViewer>` contributed down as the shared R-17 §5.1 hard component consumed by
G-7/G-4/G-3 (commit/[oid] migrates onto it, retiring bespoke FileDiff/DiffRow) — approved.

Open-question decisions (binding):
- **Q1 word-level emphasis: DEFER** (the +/− text channel is intact without it).
- **Q2 syntax highlighting: R3 ships monochrome mono.** Syntax color is a token-tier follow-on
  (`--tok-*` ramp + render path), never a per-surface hack.
- **Q3 comment store: git-owned endpoint, subject-generic schema.** Edge stays
  `…/prs/{n}/threads`; the STORAGE schema keys threads by the canonical type-qualified
  `object_key` (the R2.2 grammar) so the one-conversation-primitive migration is a re-face,
  not a rewrite. This is the architecture call: build once, generalize by key.
- **Q4 merge-base: three-dot via libgit2 merge_base** (durable repos are libgit2-backed). If a
  real blocker appears, the two-dot fallback must label itself in the UI ("compared against
  main @ oid") — honest floor.
- **Q5 batching: batch + verdict ship with R3.7a (G-8 wave)** — R-BATCH-1 is the differentiator,
  batch-without-verdict is incoherent. If R3.2 lands first, composer ships single-comment-only
  and the "Start review" button appears with R3.7a (sequencing, not scope cut).
- **Q6 viewed-marks: client-local for R3** (localStorage keyed pr+head_oid); server-side
  per-reviewer store is a named follow-on.
- **Q7 tree-index filter: fast-follow** (adds its no-results state then).
- **Q8 restricted = count-only: CONFIRMED as policy intent.** Restricted → in totals + count row;
  Absent-classed → excluded from totals entirely. No paths/diffstat cross the wire — binding.
- **Q9 deep-link freshness: honest banner for R3** (as sketched); server re-anchor decided
  together with the G-9 check→line minting work.
- **Icon: approve `expand-lines` for registration**; `chevron` pair acceptable interim.

## 05-first-run — ACCEPTED (2026-07-16)

Rails scan clean; the two accent `background` uses (8px live-dot, checklist current-step square)
are the §3.1 small-non-text-affordance carve-out, both carrying visible labels — allowed. Honest
chrome throughout (no fake inbox, dignified CI floor, dev seam relegated). Dissolves firstrun #1–6.

Open-question decisions (binding):
- **OQ-1 landing floor: ACCEPTED.** Everyone lands on Code until Issues/Chat/Knowledge exist;
  a floor, not a masquerade. Revisit at R4 dogfood.
- **OQ-2 checklist: KEEP with dismiss**; reception is `[DEFERRED-UNTIL-USERS]` — founder dogfood
  (R4) is the observation window.
- **OQ-3 OIDC shape: builder verifies against the actual R2.5 edge code** (route names in the
  sketch are provisional); `GET /v1/auth/config` must be exposable unauthenticated — confirm at
  build, fail the wave loudly if not.
- **OQ-4 live channel: the unified SSE firehose, typed events.** No second channel. The repos
  screen subscribes to `repo.created`/`repo.pushed` event types; these do NOT become inbox items
  (your own push is not a notification) — transport unified, content policy separate.
- **OQ-5 CI floor: ACCEPTED** — copy blames the absence of a connected run surface, never the
  user; "View run" lights up later with no layout change.
- **OQ-6 dev-seam render gate: the server flag** (`auth/config.dev_login_enabled`), belt-and-
  braces with the existing build-time PROD kill switch. Server truth wins over import.meta.env.

## 02-pr-overview-context — ACCEPTED (2026-07-16) · pack COMPLETE 5/5

Rails scan clean (hex hits are `#4117`-style run numbers). The AppShell `contextPane` fourth-region
spec (§1b of its NOTES) is BINDING for the R3.3 builder — shell-owned frame/drawer/landmark, column
drops when absent. LinkedRefVM as viewer-scoped resolver projections (UI invents nothing) is the
right shape.

**Cross-pack reconciliation (BINDING — the one conflict between packs 02 and 03):** both designed
the comment API. Canonical model = **threads** (03's shape): a thread has an optional content
anchor; comments belong to threads; 02's review batching layers on via `review_id` on comments +
the `PrReviewVM` lifecycle (`POST …/reviews`, `…/reviews/{id}/comments`, `…/submit`, `DELETE`).
The overview's "discussion" = threads with `anchor: null`. Storage keyed by canonical type-
qualified `object_key` (per the 03-Q3 decision) so issues/docs mount the same store later.
Endpoints stay PR-scoped at the edge. Submit emits ONE batch event (R-BATCH-1), server-side.

Open-question decisions (binding):
- **Q1 store generic: YES** — folded into the reconciliation above.
- **Q2 discussion inline + tab-as-anchor: ACCEPTED** (the G-6 switch test wins; extension, not
  re-divergence).
- **Q3 gate + changes_requested: backend truth first.** Builder verifies what the R0.2/R2 ruleset
  actually ingests before copy freezes; blocked-reasons list may never imply a gate input that
  isn't real. If required-approvals isn't a server gate input yet, the copy adapts (honest floor).
- **Q4 visibility_label: include only if projectable in the same query** from existing authz
  metadata; else drop (sketch tolerates absence without layout change).
- **Q5 diff-stat badge: sequencing** — renders when the G-7 endpoint lands; skeleton spelling until.
- **Q6 merge-commit only: ACCEPTED** (matches backend; methods are a later ruleset concern).
- **Q7 chips-open-in-pane: DEFERRED** to a spike informed by R4 dogfood (navigate-on-click,
  peek-on-hover ships).
- **Q8 agent-slot absence: ACCEPTED** (absent when no permission; empty state only when
  permitted-and-none).
- **Q9 one shared `?` cheat-sheet primitive** in the shell; packs 02+03 keymaps merge into one
  registry at build — orchestrator holds both build waves to it.

**R3.0 exit: all five surfaces ACCEPTED. Build waves may open (VISION §3 satisfied).**
