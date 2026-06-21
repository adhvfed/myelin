# Git hosting — Design-system pass (visual/token level) + the X-1 affordances

> The **P4 design-system pass** (VISION §3 / OQ-12) over the preserved structural design sketch
> ([`information-architecture.md`](./information-architecture.md), [`user-flows.md`](./user-flows.md),
> [`wireframes.md`](./wireframes.md)). The structural sketch fixed *what* the screens are and their
> empty/loading/error/permission/erased/agent-pending states. This pass fixes the **visual/token layer**
> — which semantic tokens, type steps, spacing steps, glyphs, motion, and a11y treatments each surface
> consumes — and folds in the **X-1 affordances** (the fork-trust badge, the checks panel, the
> merge-queue affordances) that landed after the original sketch when the `CheckStatus` seam (recon X-1,
> contract 5.9) was frozen and the consumer module was declared (GIT-P6 / P-232).
>
> **This is a design sketch + sign-off, NOT frontend code.** No UI is built here. The frontend lands in
> **GIT-P31** (the Web UI), which builds to this pass. The fork-trust UX is **decision-shaped**
> (EI-01 §8): the sketch is produced here and **paused for human sign-off** — recorded, dated, in
> [`signoff.md`](./signoff.md). Date: 2026-06-21.
>
> **No new contract.** A design-system pass is not a contract (GIT-P7 states this explicitly). It names
> the tokens/treatments the eventual frontend consumes; it freezes nothing in the contract index.

---

## 0. What this pass is bound to (the inputs it must not contradict)

- **The token *system* (design-language §3):** three tiers (primitive → semantic → component); components
  consume **only semantic tokens** (`surface`, `surface-raised`, `surface-overlay`, `text-primary`,
  `text-muted`, `border`, `accent`, `success`, `warning`, `danger`, `info`, `agent`, `focus-ring`).
  The concrete *values* (palettes, type family, spacing numbers) are the design-system **package**
  deliverable and a live styleguide (design-language §9 OPEN→P4; EI-05 §3) — this pass fixes the
  **token-to-surface bindings**, the measured-contrast constraints, and the per-state treatments, not the
  hex values. Naming the value-table + styleguide build is a named follow-on (§7 floors).
- **The day-one primitives (design-language §8b / EI-05):** overlays portal-to-root on one z-index scale;
  the ONE editor render path; **measured-not-claimed** tokens (status never colour-alone; focus token ≠
  identity token; no inline colour on interactive elements; no sparkle/emoji-as-UI for agents); the
  layout-containment + mobile checklist; humanise-at-the-backend.
- **The live X-1 contract (recon X-1 / 5.9, `myelin-git::check_status`):** the visual states below are
  keyed **exactly** to the frozen enums so the GIT-P31 frontend renders the real projection, not a
  parallel vocabulary:
  - `CheckState = queued | in_progress | success | failure | error | neutral | cancelled`
  - `TrustTier = trusted | untrusted_fork`
  - `GateOutcome = AllRequiredGreen | Blocked { unmet: [CheckContext] }`
  - `fork_endorsed: bool` (set by the maintainer `check(subject, approve_untrusted_ci, repo)` flow)
  - `summary: HumanisedRef` (a `(template_key, args)` pair humanised by Notif — never a raw CI string).

---

## 1. The token map per primary screen (semantic bindings)

For each screen the pass fixes which semantic tokens carry which role. Every value below is a **semantic
token name**, not a literal — the value-table swap is what makes light/dark/high-contrast/tenant-theme a
token-table swap, not a component rewrite (design-language §3.1).

| Surface role | Semantic token | Notes (PROVEN / HOUSE STYLE) |
|---|---|---|
| Page / shell background | `surface` | the pinned-viewport shell (§8b.4); each region owns its scroller |
| Sidebar / context pane background | `surface` (flush) + `border` hairline divider | borders carry separation; **space first, hairline only when space can't carry it** (EI-05 §3, HOUSE STYLE) |
| Cards (README, PR description, ruleset rows) | `surface-raised` + `border` | flat layered surfaces; **shadow reserved for genuinely floating layers only** (§3.5, HOUSE STYLE) |
| Floating layers (⌘K palette, menus, popovers, unfurl hovercard, toasts, the HITL card when it overlays) | `surface-overlay` + the ONE shadow token | the single elevation token; portal-to-root, one z-index scale (§8b.1, PROVEN) |
| Primary text (code, titles, body) | `text-primary` | hierarchy from **weight & colour before size** (§8b.3, HOUSE STYLE) — no display-size headings on dense surfaces |
| Secondary text (meta, blame age, counts, "2m ago") | `text-muted` | |
| Brand accent (active rail item, primary button) | `accent` for fill; **`focus-ring` is a separate token** | **the focus token is NOT the identity token** (§8b.3, PROVEN) — the accent may fail AA as a focus ring; derive focus from a measured token |
| Keyboard focus on every interactive element | `focus-ring` | always visible on keyboard focus, AA on light/dark/high-contrast (§4, PROVEN — WCAG 2.2) |
| Status: check passed / PR merged / approved | `success` **+ glyph + label** | **never colour alone**; no saturated fill (§8b.3, PROVEN) — see §3 glyph map |
| Status: check failed / merge blocked / request-changes | `danger` **+ glyph + label** | same rule |
| Status: running / queued / awaiting | `warning` or `info` **+ glyph + label** | |
| Agent-attributed content (agent review row, HITL card, agent comment) | `agent` family **+ `🤖`-class glyph + the word "agent"** | the reserved agent treatment (§3.2); **distinguishable without colour** (icon + label); **no sparkle/shimmer/magic-wand, no emoji-as-UI** (§8b.3, legibility duty PROVEN via AI-Act §6) |
| Destructive admin (erasure, force-push-allow, history-rewrite) | `danger` accents on a confirm surface | reversibility-over-confirmation **carve-out**: GDPR/irreversible **still confirm** (§6.3) |

**Inline-colour ban (PROVEN, lint-enforced in Phase-8).** No screen sets colour via inline style on an
interactive element — inline style beats `hover:`/`focus:` specificity and ships a control whose hover/
focus silently dies (§8b.3). All interactive colour is token/utility-class driven. This is a build-time
lint the GIT-P31 frontend inherits; named here so the design contract is explicit.

---

## 2. Type, spacing & density per surface (the ramp bindings)

The pass binds each surface to steps on the **one modular type scale** (`display / h1 / h2 / h3 / body /
body-sm / caption / code`, design-language §3.3) and the **one 4px spacing ramp** (`0,1,2,3,4,6,8,12,16,
24…`, §3.4). Off-ramp values (5/7/13px) are forbidden — the amateur tell (§8b.3, HOUSE STYLE).

- **Code / diff / blame / search-result rows (Screens B, D, E):** `code` (monospace, load-bearing) at the
  **`compact` density token set**; tight line-height from the same scale; gutters and blame columns on the
  4px ramp. Density is **earned** here (P5) — this is where the engineer lives.
- **PR overview, ruleset editor, settings (Screens C, F):** `body` / `body-sm` at the **`comfortable`**
  default; section labels at `h3` by **weight**, not size; generous ramp spacing for the
  progressive-disclosure expanders (P4).
- **Repo home (Screen A):** `h2` repo title by weight; README content gets the **reading-optimised
  measure** (the long-form variant of the same scale, §3.3); quick-action buttons on the ramp.
- **The HITL card (Screen G):** `body` for the plan lines; the agent label at `body-sm` weight-strong; the
  cost estimate and "why" at `caption` `text-muted`. No large/heavy type — trust comes from legibility,
  not size.

---

## 3. The icon → meaning map (one set, stable mapping)

The pass fixes the glyph for each load-bearing meaning so "merge means merge" reads identically across Git
/ CI / Issues / Chat unfurls (design-language §3.7 / P1). **Status carries glyph + label + position, never
colour alone** (§8b.3). The agent glyph is a **plain agent mark, never a sparkle/magic-wand** (§8b.3 / P7).
Glyphs below are role names from the ONE icon set (not literal emoji — emoji can't inherit `currentColor`
or be re-themed, §8b.3); the structural wireframes use ASCII stand-ins (`✔ ⚠ ⟳ 🤖`) that map to these
roles.

| Meaning | Role glyph | Carries token | Where |
|---|---|---|---|
| Check `success` | check-mark | `success` | checks panel, merge readiness, viewed-files |
| Check `failure` | x-mark | `danger` | checks panel, jump-to-failure |
| Check `error` / `cancelled` | alert-triangle / slash-circle (**distinct from failure**) | `warning` | checks panel — an `error` is not a `failure` (§2.2 architecture) |
| Check `queued` / `in_progress` | clock / spinner-glyph | `info` | checks panel "queued → testing" |
| Check `neutral` (incl. un-endorsed fork) | dash-circle | `text-muted` | recorded, **never gating** |
| Trust: `untrusted_fork` un-endorsed | shield-question | `warning` | the **fork-trust badge** (§4) |
| Trust: `untrusted_fork` endorsed | shield-check | `success` | post-endorsement |
| Agent actor / proposal | agent-mark (no sparkle) | `agent` | review row, HITL card, agent comment |
| Merge ready / merged | merge-glyph | `success` | merge UX |
| Branch / ref | branch-glyph | `text-muted` | |
| Residency / region pin | region-pin (🇪🇺-class) | `info` | header org switcher, repo header |

---

## 4. The X-1 affordances — the design-system treatment (the heart of this pass)

These three affordances were named in the architecture's view doc (§2.2, the X-1 consumer surface) after
the original structural sketch. This pass fixes their **visual/token treatment** and **state machine
keyed to the live 5.9 enums**. They are sketched, not built (GIT-P31).

### 4.1 The fork-trust badge (the security-critical, decision-shaped affordance)

The poisoned-pipeline-execution defence made visible (recon X-1 / 5.9; EI-02 §1 blast-radius). A check
whose `trust_tier = untrusted_fork` is **recorded but NEUTRAL for gating** until a maintainer endorses it
(`fork_endorsed = true` via `check(subject, approve_untrusted_ci, repo)`) or it is re-run trusted.

**Treatment (sketch — sign-off required, EI-01 §8):**

```
┌ Checks (2) ─────────────────────────────────────────────────────────────────────────┐
│ ✔ ci/build   passed   required                                                        │
│ ⚠ ci/test    passed on a FORK run — neutral until trusted   [shield-question]         │
│   └ This run executed code from an untrusted fork. It does NOT satisfy the gate by     │
│     itself. A maintainer must review and trust it.        [ Trust this run ][ Re-run ] │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

- Token: **`warning` + shield-question glyph + the explicit words "untrusted fork" + "neutral until
  trusted"** — never colour alone, never a green check that lies (§8b.3 PROVEN; the whole point is that a
  fork's own green must NOT read as gating-green).
- The action `[ Trust this run ]` is gated on `approve_untrusted_ci` (Identity ABAC); it is the **only**
  path to `fork_endorsed = true`. A viewer without the permission sees the badge **read-only** (the state
  is honest; the action is absent) — no leaked affordance.
- After endorsement: the badge flips to **shield-check + `success`** and the row counts toward the gate.
  After a trusted re-run: a new `run_attempt` supersedes (the monotonic supersession rule, 5.9) and the
  fork badge clears.
- **Why decision-shaped:** this affordance is the human gate on attacker-controlled CI config. Whether the
  copy, the placement (inline vs a dedicated "untrusted runs" tray), and the exact friction (one-click
  trust vs typed-confirm) are *right* is a **security + abuse call a human must make** (EI-01 §8) — hence
  the sign-off below is explicitly required on THIS affordance.

### 4.2 The checks panel (the X-1 consumer surface)

The always-visible per-context status, fed by the `check_status` projection (`ci.check.updated` →
`CheckStatusRow`). One row per `(commit_oid, context)`.

**Treatment (sketch):**

```
┌ Checks ──────────────────────────────────────────────────────────────────────────────┐
│ ✔ ci/build    passed     required    "Built in 1m12s"            [logs ↗ #step-2]      │
│ ✗ ci/test     failed     required    "3 tests failed"            [jump to failure ↗]   │
│ ⟳ ci/lint     running…   optional    "Queued → running"                                │
│ ⚠ ci/e2e      error      required    "Runner cancelled"          [retry]               │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

- Each row: **glyph (from §3) + humanised `summary`** (the `HumanisedRef` resolved by Notif — **never a
  raw CI string**, §8b.5 PROVEN-leverage) + `required?` (Git's branch-protection policy decides, not CI) +
  a **jump-to-failure deep-link** that resolves `details_ref` `#step-<n>` into CI's run view (the
  pre-fetch-the-next-hop rule, EI-05 §4 — failing check → failing step → the line).
- **`error`/`cancelled` are visually distinct from `failure`** (§2.2 architecture; §3 glyph map) — an
  infra error is not a test failure and must not read as one.
- **State coverage** keyed to `CheckState`: `queued`/`in_progress` → skeleton-then-spinner-glyph (no blank
  spinner, §8b.6); `success`/`failure`/`error`/`cancelled` → terminal rows; `neutral` → recorded,
  greyed, explicitly "not gating". **Empty:** "No checks configured for this branch." **Loading:**
  skeleton rows matching the final layout. **Error (panel itself down):** "Checks unavailable — retry"
  **fail-static for this surface only** (§8b.6) while the rest of the PR renders.

### 4.3 The merge-queue / merge-readiness affordances

The merge UX driven by the durable `ci.result` wait (recon X-1; the merge-queue workflow holds **no
runtime** while CI runs — it wakes on the durable signal, possibly hours/days later).

**Treatment (sketch):**

```
┌ Merge readiness ─────────────────────────────────────────────────────────────────────┐
│ ⚠ Blocked: ci/test failing · ci/e2e error · @sec must still approve                    │
│   (the gate names WHICH context is unmet — humanised, with the next action)            │
│                                                                                        │
│ When green:   ✔ All required checks green · 2/2 approvals · threads resolved            │
│               [ Merge ▾ ]  ·  [ Enable auto-merge when green ]                          │
│                                                                                        │
│ In a queue:   ⟳ Queued (position 2) → testing → merged                                 │
│               Multi-day HITL hold: "Awaiting approval from @maintainer (held 1d)"       │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

- **Merge readiness names which gate is unmet** in humanised text with the next action (mirrors
  `GateOutcome::Blocked { unmet }` — the `unmet` contexts become the human-readable list). Never a bare
  "blocked".
- **Queue position + the "queued → testing → merged" lifecycle** is the visible face of the durable
  workflow; on a multi-day HITL hold it shows the pending-approval state (the workflow holds no runtime
  while it waits — the card just reflects the durable state). This is the **agent-pending / waiting**
  state (§5.10) applied to the merge queue.
- **Optimism for latency, honesty on failure** (EI-05 §4): enabling auto-merge updates optimistically;
  if the durable signal later reports `failure`, the PR dequeues with the humanised reason — honest
  rollback, no silent stall.

---

## 5. Cross-cutting treatments fixed by this pass (apply to every screen)

- **Overlays** (⌘K palette, menus, popovers, unfurl hovercard, the HITL card, confirm dialogs, toasts):
  `surface-overlay` + the ONE shadow token, **portal-to-root**, **one z-index scale** (chrome < popover <
  modal < toast), focus-trap + return-focus, scroll-lock, Escape/backdrop dismiss (§8b.1, PROVEN). The
  GIT-P31 frontend consumes the **shared** primitives — it does not re-implement these.
- **Loading is structure, never a blank spinner** (§8b.6, HOUSE STYLE measured in Phase-5): every loading
  state is a skeleton matching the final layout (file-tree rows, README block, checks rows, diff hunks).
  Latency budgets are hard: keyboard < ~100ms; suppress flash-of-spinner under ~1s; **pages render, they
  don't animate in**.
- **Error blames the system in one quiet line + a path + retry**, never a dead end; degraded surfaces
  **fail static** for that surface only (§8b.6).
- **Permission-denied → the §5.3 "no access" card / context-pane permission-stub**, never a leaked title
  (P9 / ADR-03). The context pane resolves each `ArtifactRef` per-viewer; an unseeable target degrades to
  a stub.
- **Erased/tombstoned → the §5.10 erased state** (GDPR-aware), never a dangling render.
- **Motion** is functional, fast (≈120–200ms), interruptible, and **`prefers-reduced-motion` is a
  first-class path** (§3.6 / §4). A PR going green or a queue advancing gets a **subtle** live transition;
  agent proposals get the consistent agent motion.
- **Humanisation is at the backend** (§8b.5): every machine string the frontend shows (`merge_request
  merged`, raw ids, the check `summary`) arrives **already humanised** via Notif + Refs, paired with a
  routable `ArtifactRef`. The frontend owns **no** humanisation map. This is why the checks-panel
  `summary` is a `HumanisedRef`, not a string.
- **Mobile / layout-containment** (§8b.4): the shell is pinned (`100vh`/`overflow:hidden`); each region
  owns its scroller (`min-height:0` on scrolling flex children); sidebar + context pane collapse to
  toggled overlays (backdrop + Escape + route-change auto-close); diff degrades to unified-only on narrow
  widths; hover-only row actions get an explicit mobile affordance.

---

## 6. Accessibility constraints the value-table must clear (named, not yet measured)

The token *values* (the design-system package, §7 floor) MUST be validated against these — this pass fixes
the **constraints**, the measurement is the Phase-5 gate (EI-05 §3 / §8b.3):

- **WCAG 2.2 AA** on every semantic text/background pair, in light, dark, and high-contrast — measured,
  never claimed. AAA pursued on the primary reading + code surfaces.
- **The `focus-ring` token meets AA contrast on every surface** and is **derived independently of the
  brand accent** (the accent may fail as a focus ring).
- **Status + agent + trust treatments pass for colour-blind users** (icon + text label always present —
  the §3 glyph map and the §4 fork-badge copy exist precisely to satisfy this).
- **Full keyboard operability** of the diff, the checks panel, the ruleset editor, the HITL card; logical
  tab order; the `?` shortcut cheat-sheet.
- **Screen-reader correctness** inherited from the shared components; live regions announce a check going
  green / a queue advancing / an agent proposal without spamming.
- **EU-multilingual + RTL**: externalised strings, logical (start/end) layout properties throughout, a
  type stack with broad Latin-extended / Greek / Cyrillic coverage (EU-sovereign procurement bar, EN
  301 549).

---

## 7. Floors (named, with their follow-on prompt)

- **The concrete token VALUE table** (palettes light/dark/high-contrast, the type family selections with
  EU-multilingual coverage validated, the spacing/radius numbers) and the **live styleguide rendered from
  the real tokens** (runnable with the stack down) are the design-system **package** deliverable
  (design-language §9 OPEN→P4; EI-05 §3 / §8b.6). This pass fixes the **bindings and constraints**; the
  values + the measured-contrast gate land with the frontend foundation in **GIT-P31** (and the
  platform-wide design-system package it builds on). Named, not silently skipped.
- **The frontend itself** (the Web UI for all eight screens + the X-1 affordances) lands in **GIT-P31**.
  This pass is the build-to. No frontend code is written here (VISION §3).
- **The measured-contrast / inline-colour-ban / round-trip-editor lints** are Phase-5/Phase-8 CI gates the
  GIT-P31 frontend inherits — named here as the design contract they enforce, built there.

---

## 8. The sign-off (decision-shaped — EI-01 §8)

The **fork-trust UX (§4.1)** is decision-shaped: it is the human gate on attacker-controlled CI config
(security + abuse scope). Per EI-01 §8, the sketch is produced and **paused for human sign-off** rather
than built autonomously. The dated sign-off is the green artifact for GIT-P7 — recorded in
[`signoff.md`](./signoff.md). The rest of the pass (the token map, type/spacing bindings, glyph map, the
checks-panel and merge-queue affordances, the cross-cutting treatments, the a11y constraints) is the
reviewed design sketch the frontend builds to.

## Cross-references
- design-language §3 (tokens), §4 (a11y/i18n), §5.x (components), §6 (agent UX), §7.1 (git catalogue),
  §8b (day-one primitives + measured tokens), §9 (the OPEN→P4 value-table follow-on).
- EI-05 (UX & design bar), EI-01 §8 (the human sign-off bottleneck), VISION §3 (design-before-code).
- recon X-1 / contract 5.9 (the `CheckStatus` seam — the live enums the §4 affordances key to);
  `myelin-git::check_status` (the declared consumer module, GIT-P6 / P-232).
- architecture `04-views-cli-and-api.md` §2.2 (the X-1 consumer surface this pass dresses).
- The structural sketch: `information-architecture.md`, `user-flows.md`, `wireframes.md`.
