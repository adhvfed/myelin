# R3.5 — First-run flow · sketch spec (NOTES)

> Surface: **first-run** — login → land → get code in → first CI run, as ONE continuous taught path.
> Direction A "Instrument". Reads the generated `../tokens.css` (semantic tokens only). This is the
> implementation-ready spec the HTML can't carry: IA/routes, full state set, data contract
> (EXISTING vs NEW), keyboard/SR map, component reuse, open questions.
>
> **Sketch files** (each links `../tokens.css`; each carries a sketch-only theme/RTL toolbar + ≥1 German string):
> - `sketch-login.html` — logged-out; real OIDC primary, dev seam gated, + SSO-unavailable / error / loading states.
> - `sketch-empty-tenant.html` — fresh empty tenant; onboarding-forward repos empty + honest chrome + loading skeleton.
> - `sketch-post-first-push.html` — repo appears (SSE/refresh) + the honest CI floor (checks pending + NotAvailable run).
> - `sketch-inbox-empty.html` — honest inbox: inbox-zero, loading skeleton, filtered no-results, de-DE expansion.
>
> **Review findings dissolved:** `ux-ux-firstrun.md` #1 (dead-end rail), #2 (empty teaches), #3 (fake inbox),
> #4 (force-landed on Code — recorded as accepted floor), #5 (palette client-side nav), #6 (login dev-only + tooltip reason);
> `ux-ux-a11y-visual.md` #2 (rail active = surface-hover not accent fill), #4 (skeleton + aria-busy), #6 (primary rides `--c-btn-primary-bg`), #7 (100dvh).

---

## 1. IA + routes (SolidStart, `frontend/apps/web/src/routes/`)

| Route | Exists? | First-run role | Change from today |
|---|---|---|---|
| `/login` | EXISTING (`routes/login.tsx`) | logged-out; real SSO primary + gated dev seam | Rework: OIDC button primary (rides `--c-btn-primary-bg`); SSO-unavailable reason as **visible text**; dev seam behind a dev-build guard; `100dvh`. |
| `/(app)/index.tsx` | EXISTING (`<Navigate href="/git/repos">`) | app-root landing | **Keep** the redirect to `/git/repos` as an **accepted floor** (see §6 / OQ-1). Not made role-aware in R3.5. |
| `/(app)/git/repos` | EXISTING (`git/repos/index.tsx`) | the de-facto first landing; empty→onboarding, then live-appears the pushed repo | Rework empty branch (`repos-empty`) from one sentence → onboarding-forward; add live/SSE affordance. |
| `/(app)/git/repos/[repo]` | EXISTING (`git/repos/[repo]/index.tsx`) | per-repo home; already teaches (`repo-empty` shows clone URL + `git clone/push`) | **Harmonize** copy/structure with the new tenant-level empty (same voice, same push-to-create framing). No behavior change required. |
| `/(app)/git/repos/[repo]/prs/[n]` (+ `/checks`) | **NEW route** (PR list GET + PR overview are censused-NEW in `_brief.md`) | first PR → Checks: the honest CI floor | Owned by the PR-overview R3 assignment; first-run references its **Checks** panel only. |
| `[...404]` catch-all → `NotAvailable` in shell | **NEW** | dignified fallback for the 4 unbuilt rail destinations | Fixes firstrun #1 (raw framework 404). |
| `/issues`, `/chat`, `/ci`, `/knowledge` index | **NEW (thin)** | each renders `NotAvailable` inside the shell | Rail links stay present but honestly marked (see §4). `/ci` is where the "view run" honest floor points. |

**Palette navigation (firstrun #5):** the ⌘K palette must navigate via `useNavigate()` (client-side `<A>`),
**not** `window.location.assign` — no full reload, no re-running the auth guard. Entries for unbuilt subsystems
route through `NotAvailable`, they are not silently dropped (keeps the palette an honest map of the IA).

---

## 2. Full state set (R-21 enumeration)

Required first-run states and where each is sketched:

| State | Where | Notes |
|---|---|---|
| **logged-out** | login · primary state | real OIDC button primary; dev seam relegated + gated. |
| **login: SSO unavailable** | login · variant | JWKS/trust-anchors unset → button `aria-disabled` + **visible reason text** (not a `title`), with the dev seam as the dev-build path. |
| **login: error (failure)** | login · variant | `role="alert"`, system-blaming one line + retry path; **never a raw `err.message`**. |
| **login: loading (redirecting)** | login · variant | button `aria-busy`; polite `role="status"`; **no spinner glyph** (status-ring is CI-reserved). |
| **fresh empty tenant** | empty-tenant · main | `repos-empty` becomes onboarding-forward; teaches push-to-create; waiting-for-push live affordance; first-run checklist. |
| **loading — repos list** | empty-tenant · variant | structure-matching **skeleton + `aria-busy` + one debounced polite live region** (fixes a11y #4). |
| **first repo pushed** | post-first-push · Frame A | repo live-appears in place (SSE); `Live · updated just now`; "New" via glyph+label. |
| **stale / reconnecting** | post-first-push · variant | list **fails static**; `Reconnecting… last updated 12s ago` + manual Refresh; glyph+label, not colour-alone. |
| **first CI run (honest floor)** | post-first-push · Frame B | checks render pending from `PrChecksVM`; merge gate from `gate_admitted`; run-detail surface = `NotAvailable` (no fabricated pipeline). |
| **inbox-zero** | inbox-empty · main | calm/rewarding "You're all caught up"; **no confetti**; topbar badge **absent** at zero. |
| **inbox loading** | inbox-empty · variant | row skeletons + `aria-busy` + polite region. |
| **inbox filtered-to-nothing** | inbox-empty · variant | distinct from inbox-zero; permission-honest (never reveals hidden matches). |
| **i18n / German expansion** | inbox-empty (`Alles erledigt`) · empty-tenant (`Warte auf deinen ersten Push …`) · post-first-push (de-DE gate) · login (`Datenregion`) | one German string per surface; labels not fixed-width. |

**States deliberately NOT sketched here** (owned elsewhere / out of first-run scope):
permission-denied repo (owned by `[repo]` `restricted` + `NotAvailable`, already good); erased/tombstoned
(no erasure in first-run); **inbox storm / 30× agent surge** (owned by the notifications-inbox component
spec, needs populated data); populated inbox rows with `ReferenceChip` subjects (needs real items — only the
skeleton/filtered shells are shown); the diff/PR-overview populated surfaces (other R3 assignments).

---

## 3. Data contract — EXISTING vs NEW

Tag = what each rendered field binds to. `EXISTING` = already in `frontend/apps/web/src/lib/api.ts`
or `lib/auth.ts`. `NEW` = the backend/edge work order for the build wave.

### Login
- `EXISTING: Viewer.{displayName,tenant,region}` (`lib/auth.ts`) — post-login chrome (identity + residency cue).
- `EXISTING: loginDev` action + `import.meta.env.PROD` build-time kill + `devLoginAllowed()` runtime guard — the dev seam is already fail-closed; the sketch just makes its **rendering** dev-build-only too.
- `NEW: GET /v1/auth/config → { sso_configured: bool, providers: [{id,label}], dev_login_enabled: bool }` — drives (a) enabled vs SSO-unavailable primary button, (b) provider label(s), (c) whether to render the dev seam at all. Without this the frontend can't honestly show the SSO-unavailable reason.
- `NEW: POST /v1/auth/oidc/start` (→ 302 to IdP) and `GET /v1/auth/oidc/callback` — the real human login **landed at the edge in R2.5**; confirm the exact route shape and wire the primary button's `<form action>` to it. The error state renders on a failed callback (`?error=` round-trip), humanised server-side.

### Empty-tenant landing
- `EXISTING: getRepos() → ReposPage{items: RepoHomeVM[], page}` — empty when `items.length === 0`.
- `EXISTING: RepoHomeVM.clone_url` — the wire URL shown in the push instructions (host `git.eu.myelin.dev`); on an empty tenant there is no repo yet, so the instructions use a **template** URL (`…/acme/<name>.git`) built from `Viewer.tenant`, not from a VM.
- `NEW: repo-created / first-push event (SSE)` — `GET /v1/git/events` (or reuse the notifications firehose, OQ-4) streaming `{type:"repo.created"|"repo.pushed", slug, at}` so the empty list flips to populated **in place** (no reload). This is what makes the "Warte auf deinen ersten Push …" affordance real; the manual Refresh button is the always-available fallback.

### Post-first-push
- `EXISTING: RepoHomeVM.{slug,readme_excerpt,clone_url,entries}` — the appeared repo row.
- `EXISTING: getPr → PrVM.{number,pr_state,base_ref,head_ref}` and `getPrChecks → PrChecksVM.{required_contexts,green_contexts,gate_admitted,required_approvals}` — the Checks panel. A required context **not** in `green_contexts` renders "Waiting for a report"; **`gate_admitted` is authoritative** for the merge-blocked line (UI never recomputes policy).
- `NEW: PR list GET` + `NEW: check→run ref` (both censused-NEW in `_brief.md`) — needed for the PR list and a real "View run" link. Until `/ci` ships, the run link is `NotAvailable` (honest floor); no new endpoint is required to render the floor itself.

### Inbox
- `NEW: GET /v1/inbox?filter=<what-needs-me|mentions|review> → { items: InboxItemVM[], unread_count, page }` — drives the topbar badge (**absent when `unread_count===0`**) and the panel. `InboxItemVM = { id, kind, reason, subject: ArtifactRef, priority, when, state }` (per `notifications-inbox.md` §2). The **frontend owns no humanisation** — subject/why-line resolve server-side (permission-/erasure-safe).
- `NEW: GET /v1/inbox/events (SSE, resume cursor)` — live delivery (`last_seq` backfill-then-live; `resync_required` → full reload, named not silent). Replaces the current hardcoded Dialog list.

---

## 4. Honest chrome details

- **Inbox badge** — bound to `unread_count`. Zero → **no badge, no `2`**; `aria-label="Inbox, no unread notifications"`. Non-zero → glyph + count (post-first-push shows `1`), `aria-label="Inbox, 1 unread notification"`.
- **Rail** — active item = `--surface-hover` fill + `--text-primary` + **accent-tinted glyph only** (never an accent tile — fixes a11y #2 / R1). Unbuilt destinations (`/issues /chat /ci /knowledge`) stay **present** (the chrome is the real chrome) but are `data-unavailable`: muted colour + a **square-cut** 4px corner marker (R1: any edge marker must be square, never a rounded pill) + accessible name suffix `— not available yet`; they route to `NotAvailable`, not a raw 404.
- **Interactive colour is class-driven** (not inline) so `:hover`/`[aria-current]`/`:focus-visible` all work (fixes a11y #5). The shared `:focus-visible` ring from `tokens.css` is never overridden — no `outline:none` anywhere (fixes a11y #1).
- **Residency cue** — `EXISTING: ResidencyCue(region,tenant)`; ambient T0 (glyph + text, never colour-alone).
- **Non-engineer personas** — landing is **Code** for now (accepted floor, OQ-1).

---

## 5. Keyboard + SR map

**Global (shell, per `shell-and-nav.md` §5):** skip-link is the first focusable element → `#main`; landmarks
`banner` (topbar) / `nav[aria-label=Surfaces]` (rail) / `main#main` / (later) `aside` context pane; one global
polite `role="status"` live region; ⌘K opens the palette from anywhere; **no focus traps** outside overlays.

- **Login:** logical tab order = primary SSO → (dev seam if present) → provider help. Autofocus is **not** forced; if used it still shows the `--focus-ring`. SSO-unavailable reason is real text referenced by `aria-describedby` on the disabled button (not a `title`). Error banner is `role="alert"` (assertive). Redirecting state is `aria-busy` + `role="status"` (polite).
- **Empty-tenant:** headings `h1 Repositories → h2 Get your first repository in / First run`. Copy `<pre>` blocks pair with a labelled Copy button (`aria-label="Copy: git remote add"`). Waiting affordance announces via a polite `role="status"` ("updates on its own…"); the live dot is `aria-hidden` (the text carries meaning). Skeleton: container `aria-busy="true"` + a visually-hidden `role="status"` "Loading repositories…" (debounced).
- **Post-first-push:** the live/stale indicators are `role="status"` (polite) — glyph `aria-hidden`, text carries state. Checks list: `<ul>` labelled by the "Checks" heading; each row = glyph(`aria-hidden`) + mono context + **text status** ("Waiting for a report" / "Passed") so status is never colour-only. Merge gate is `role="status"`. The disabled "View run" is `aria-disabled` with visible "· not available".
- **Inbox:** `role="dialog" aria-label="Inbox"`; filter tabs = `role="tablist"`/`role="tab"` with `aria-selected` + roving tabindex; empty/filtered/loading bodies are `role="status"`. Item rows (when populated) = the shared row molecule (`j/k` move, single-key triage, `Enter` open) per `notifications-inbox.md` §7 — not exercised in the empty shells here.

---

## 6. Component reuse

**Reuse (existing):** `NotAvailable` (dignified not-available), `ResidencyCue`, the MR-017 overlays
(`Dialog`/`Menu`/`Toast`), `Icon` (42-icon registry — this sketch uses only registry glyphs:
`nav-code/issues/chat/ci/knowledge`, `inbox`, `search`, `human`, `chevron`, `database`, `repo`, `link`,
`gate`, `pull-request`, `branch`, `check-pending`, `check-pass`, `approve`, `settings`, `close`,
`external-link`). **No new icon needed.**

**Status-rail discipline (icon manual §3.6):** the CI verdict ring (`check-pass`/`check-fail`/`check-pending`)
is used **only** for real CI verdicts (post-first-push Checks). It is deliberately **not** used for the
login redirect, the "waiting-for-push" affordance, or the checklist — those use a neutral live dot / an
`approve` mark / plain markers, so the reserved ring keeps one meaning.

**Shared primitives to factor down (named + justified; check REFINEMENTS first):**
- **`Button`** — the app hand-rolls accent fills today (login used raw `--accent`; a11y #6). Factor a shared `Button` with `variant="primary"` riding `--c-btn-primary-bg`/`--c-btn-primary-text`. **Justify:** consistency + the derived-token contrast floor, used by login, copy buttons, checklist, refresh.
- **`Skeleton`** — no skeleton primitive exists (a11y #4 is systemic). A `Skeleton` that matches row/card structure, sets `aria-busy`, and drives one debounced polite live region. Used by repos-list loading + inbox loading. **Static, no shimmer** (ambient/looping motion is banned).
- **`StatusPill` / `Chip`** — the PR-state pill and check-status label. Glyph + label + position; reuse for the "New" repo badge and PR pills. (Confirm against REFINEMENTS/existing chip before adding.)
- **`NotificationsInbox`** — the honest inbox shells here are the empty/loading/filtered states of this Tier-2 component; when items exist, subjects are `ReferenceChip`s and the why-line renders via the BlockEditor render path (per its spec). First-run only needs the empty surfaces.
- **`FirstRunChecklist`** (candidate, small) — see the argument below; if kept, it is a first-run-only composition of existing atoms (markers + labels), **not** a new shared primitive.

### Does the first-run checklist earn its place? (dense-but-calm argument)
**Kept — conditionally.** First-run spans four cross-subsystem steps (push → PR → check → merge) that no
single screen shows end-to-end; a compact, ~14rem checklist orients a newcomer to the arc without a modal
tour or a firehose. It respects dense-but-calm: instruction-forward (no delight/confetti), weight/colour
hierarchy (current = filled accent marker + "In progress"; todo = subtle), and it **earns its pixels by
teaching the next action** (R-20). **Guards against becoming a nag:** it renders only on the empty tenant /
until the first PR is merged, is dismissable, and never blocks the primary push instructions (which stand on
their own). **Counter-argument acknowledged:** a persistent checklist can read as a to-do the product is
imposing (P4/P8 risk) → this is the OQ-2 `[DEFERRED-UNTIL-USERS]` reception question.

---

## 7. Open questions (for the orchestrator gate — honestly named)

- **OQ-1 — landing persona floor.** `/(app)/index` lands everyone on **Code** (engineer surface). Recorded as an **accepted floor**: push-to-create is the only real subsystem in R3.5, so a role-aware default-landing map (PM→roadmap, etc., per R-06 / firstrun #4) has nothing to land non-engineers *on* yet. Revisit when Issues/Chat/Knowledge ship. **Floor, not masquerade** — the non-engineer sees a real (if engineer-shaped) screen, not a fake.
- **OQ-2 — checklist reception `[DEFERRED-UNTIL-USERS]`.** Does the first-run checklist read as calm-helpful or as a nag? Dismiss/auto-hide rules are a hypothesis; needs first-use observation.
- **OQ-3 — OIDC route shape.** Confirm the R2.5 edge login route(s) the primary button posts to (`/v1/auth/oidc/start` + callback), and the exact `error` round-trip so the failure state humanises server-side. Confirm `GET /v1/auth/config` (sso_configured / providers / dev_login_enabled) can be exposed unauthenticated at the login page.
- **OQ-4 — the repo-created live channel.** Is the "waiting for first push" affordance driven by a dedicated `GET /v1/git/events` SSE, or by the unified notifications firehose (`/v1/inbox/events`)? One store is the platform thesis (P1) — leaning toward the notifications firehose emitting a `repo.created` item — but a first push before any PR may not warrant an *inbox* item. Decide the channel.
- **OQ-5 — CI honest floor longevity.** The floor shows checks pending + `NotAvailable` run until `/ci` ships. Confirm this doesn't read as "broken" to a first-timer (the copy blames the *absence of a connected provider*, not the user/product). When check→run refs + a run VM land, the "View run" link lights up with no layout change.
- **OQ-6 — dev-seam render gate.** The dev seam is fail-closed server-side already; the sketch also hides its **rendering** outside dev builds. Confirm the client signal (`import.meta.env.DEV` vs `GET /v1/auth/config.dev_login_enabled`) — prefer the server flag so a prod build never even renders it.
