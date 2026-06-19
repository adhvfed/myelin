# CI/CD — Wireframes (primary screens)

> Phase 4 — CI design sketch (REQUIRED before architecture). ASCII wireframes of the **primary CI
> screens** from the design-language §7.2 catalogue, each showing **empty / loading / error** states
> (not just the happy path), per VISION §3 / design-language §5.10. The **day-one UX primitives**
> (design-language §8b) are applied throughout and called out where load-bearing.
>
> **Conventions used (from §8b):** loading shows **skeletons matching the final layout** (never a
> spinner on blank); error **blames the system in one quiet line + a path** (never the user); a degraded
> surface **fails static** ("temporarily unavailable" for that surface only); status is **never colour
> alone** — always glyph + label (`✓ Passed`, `✗ Failed`, `▸ Running`, `◴ Queued`, `⚠ Cancelled`); the
> **agent treatment** is a consistent badge `[◆ agent]` — **no sparkle/shimmer/magic-wand iconography,
> no emoji as UI** (§8b.3); overlays (the approval card, menus, the secret-reveal confirm) **portal to
> the document root** with one z-index scale + focus-trap (§8b.1); the shell is **pinned to the
> viewport**, each region its own scroller with `min-height:0` (§8b.4).

---

## Screen 1 — Run list / dashboard ("is main green?")

### Happy / live
```
┌ CI ─ acme/web ─────────────────────────────────────────────────────────[⌘K]─[⊘ filters]┐
│ Runs                                              Branch:[main ▾] Status:[all ▾] [↻ live]│
├──────────────────────────────────────────────────────────────────────────────────────────┤
│  ▸ Running   #991  build·test   feat/login   ◆ pushed by alia        2m ago   [cancel]    │
│  ✓ Passed    #990  build·test   main         pushed by ronan         14m      [↻ retry]   │
│  ✗ Failed    #989  test         main         ◆ TriageAgent proposing 31m      [view fix→] │
│  ◴ Queued    #992  deploy       release/2.1   waiting: runner gpu     —        [cancel]    │
│  ⚠ Cancelled #988  build        feat/api     superseded by #991       1h                  │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```
- Each row: **glyph + label** status (never colour alone), run id, pipeline, branch, **attributed
  actor** (`◆` = agent, §6.1 / §8b.3), relative time (humanised at backend — NOTIF-1), **row actions
  surfaced by default** (not hover-only — §8b.4 hover-isn't-touch).
- `#989` shows an **agent proposing a fix inline** → links to the agent-surfaced triage view (Screen 7).
- Live updates push subtly (`prefers-reduced-motion` honoured; "pages render, they don't animate in",
  §8b.6). `j/k` navigate, single-key retry/cancel on the focused row (P3 keyboard-first).

### Loading (skeleton matching the final layout — never a blank spinner)
```
│  ░░░░░░░  ░░░   ░░░░░░░░   ░░░░░░░    ░░░░░░░░░░          ░░░░    ░░░░░░░    │
│  ░░░░░░░  ░░░   ░░░░░░░░   ░░░░░░░    ░░░░░░░░░░          ░░░░    ░░░░░░░    │   (5 skeleton rows,
│  ░░░░░░░  ░░░   ░░░░░░░░   ░░░░░░░    ░░░░░░░░░░          ░░░░    ░░░░░░░    │    same column rhythm)
```

### Empty (onboarding-forward — guides the next action, P1)
```
│                         No runs yet for acme/web.                                │
│            Push to a branch, or run a pipeline to see it here.                   │
│                  [ Run a pipeline ]   [ View .myelin/ci.yml ]                    │
```

### Error (system-blamed, recoverable, fails static)
```
│   ⚠ Run history is temporarily unavailable. Retry, or check status.             │
│                              [ Retry ]                                           │
```
(Only the list region degrades; the shell + sidebar stay live — §8b.6 "fails static".)

---

## Screen 2 — Single-run view (DAG + jump-to-failure)

### Happy (partial-failure)
```
┌ Run #989 ─ main ─ test ─ ✗ Failed ──────────────────── triggered by: ronan ─[↻ retry failed]┐
│ Cause: git.push (a1b2c3d) · Definition: snapshot 7f3e… [view pinned config]                  │
├───────────────────────── DAG ──────────────────────────┬──── CONTEXT PANE (pre-fetched) ─────┤
│   ✓ build ──┬── ✓ lint                                  │  Pull request                       │
│             ├── ✗ test  (unit::login_test)  1m12s ◀─────│   #88 feat/login  ▸ checks failing  │
│             └── ◴ e2e   (skipped: needs test)           │  Issue                              │
│                                                         │   ENG-412  ◆ opened by TriageAgent  │
│  [✗ test] selected → jump to failing step ▾             │  Chat                               │
└─────────────────────────────────────────────────────────┴─────────────────────────────────────┘
```
- The DAG renders `needs` edges; per-job **glyph+label+timing**. Selecting the failed job **jumps to
  the failing step's log range** (the assembled-context path, §8b.6: failing check → step → line).
- **Context pane is pre-fetched, not a tab the user assembles** (§5.3 / §8b.6): the run's PR, the
  agent-opened issue, the chat post — the cross-artifact edges (refs), each a live permission-aware chip.
- The pinned **definition snapshot** is one click away (reproducibility/audit, sketch 02/05).

### Loading
```
│   ░░░░░░ ──┬── ░░░░░░                  ░░░░░░░░░░░░░░░░░    │   (DAG skeleton: nodes + edges
│            └── ░░░░░░                  ░░░░░░░░░░░░░░░░░    │    in the final graph shape)
```

### States: *queued* (DAG greyed, "waiting for a runner: label gpu"), *running* (live node pulses,
reduced-motion-safe), *dead-runner-reaped* (an honest banner: `⚠ A runner went away mid-job; #989 was
re-queued` — never silent), *timed-out*, *cancelled*.

### Error / permission-denied
```
│   ⚠ Couldn't load this run. Retry.   [ Retry ]                                  │
│   — or —                                                                        │
│   🔒 You don't have access to this run.   (graceful no-access card, never a leaked title — §5.3/P9)│
```

---

## Screen 3 — Live log view (firehose, secret-masked, collapsible per step)

### Streaming
```
┌ Run #989 ─ test ─ live log ───────────────── [search in log] [⤓ download] [follow ●]┐
│ ▾ build           ✓ 42s                                                              │
│ ▾ lint            ✓ 8s                                                               │
│ ▾ test            ✗ 1m12s                                                            │
│    41│ running 128 tests                                                             │
│    42│ test login::happy_path ... ok                                                 │
│ ▸▸ 73│ test login::expired_token ... FAILED        ◀── deep-linked (#step-test#L73)  │
│    74│   assertion failed: expected 401, got 500                                     │
│    88│ secret DEPLOY_TOKEN = ●●●●●●●●  (masked in-flight — defence in depth, not a boundary)│
│ ▸ e2e             ◴ skipped                                                          │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
- **Structured around the step graph** (collapsible per step, §6.6) — not a flat blob. The failing line
  is **deep-linked + range-highlighted** (`#step-test#L73`, the stable sub-anchor, IA §3).
- **Secrets masked in-flight** (`●●●●`) — labelled as best-effort defence-in-depth, **not** the security
  boundary (egress default-deny is — sketch 04 Part 4).
- The log region **owns its own scroller** (`min-height:0`, overscroll-contain) so it never pushes the
  header/controls off (§8b.4). `follow ●` pins to tail; scrolling up detaches follow (Linear-grade).

### States
```
connecting:  │ ░ connecting to the live stream…                                  │  (skeleton lines)
archived:    │ ◷ This run finished. Showing the archived log (cold).  [load range]│  (range-read from T2)
truncated:   │ … 12,402 lines hidden — [load earlier]                            │  (range pagination)
erased:      │ ⊘ Part of this log was erased on a data-subject request.          │  (§5.10 erased state)
error:       │ ⚠ The log stream dropped. Reconnect.   [ Reconnect ]              │  (fails static; archive still readable)
empty:       │ This step produced no output.                                     │
```

---

## Screen 4 — Environments & deployments + Approvals queue (a HITL surface)

### Happy
```
┌ CI ─ acme/web ─ Environments ──────────────────────────────────────────────────────┐
│  prod      ▸ deploying   RUN-992  v2.1.0   started 1m ago                [logs] [⊘]  │
│  staging   ✓ deployed    RUN-988  v2.0.9   3h ago               [rollback to v2.0.8] │
│ ── Approvals queue ─────────────────────────────────────────────────────────────────│
│  ◇ DEP-77  Deploy web → prod   requested by ◆ DeployAgent   waiting 4m   [Approve▾]  │
│            risk: prod · est. cost ~120 credits · on behalf of: ronan                 │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
- The approvals queue is one home of the **HITL approval card** (the other is chat + the inbox, §6.3).
  Each shows **action + risk + live cost estimate + on-behalf-of** (the AG-8 card contract). The agent
  requester is **labelled** (`◆`).

### The approval card (overlay — portals to root, focus-trapped, §8b.1)
```
        ┌──────────────────── Approve deployment? ─────────────────────┐
        │  ◆ DeployAgent proposes:  Deploy web → prod  (v2.1.0)         │
        │  Under delegated authority of: ronan                          │
        │  Risk: production environment · Est. cost: ~120 credits       │
        │  Affected: 3 services · rollback available                    │
        │                                                               │
        │           [ Reject ]   [ Edit… ]   [ Approve ]                │
        └───────────────────────────────────────────────────────────────┘
```
- **Approve / Edit / Reject** (Edit lets the human amend the effect before applying — §6.3). Backed by a
  **durable workflow gate** that can wait days (Workflow §6.3); the card persists, the human is reminded
  via the inbox; **never silently lost**. Resolving it `signal`s the workflow → resume.

### States
```
empty:    │ Nothing awaiting approval. Nothing currently deploying.        │
loading:  │ ░░░░░░░  ░░░░░░  ░░░░░       (env rows + queue skeleton)        │
deploying:│ ▸ deploying  (live progress; cancel available)                 │
failed:   │ ✗ Deploy failed — RUN-992.  [view logs]  [rollback]            │
error:    │ ⚠ Environments are temporarily unavailable.  [ Retry ]         │
```

---

## Screen 5 — Pipeline editor + validator (shift-left, no runner spend)

### Happy (valid + plan preview)
```
┌ CI ─ acme/web ─ Pipelines ─ .myelin/ci.yml ───────────── [Validate] [Plan] [Save]┐
│  1 on:                                          │  ✓ Valid · 0 lint warnings       │
│  2   pull_request: {branches: [main]}           │  ── Plan preview (no spend) ──    │
│  3 jobs:                                         │   build → ┬ lint                 │
│  4   test:                                       │           └ test ×(linux·1.79,   │
│  5     uses: …/cargo-test@sha256:abcd…           │                    linux·stable) │
│  6     matrix: {rust: ["1.79","stable"]}         │   deploy (needs test) · env:prod │
│                                                  │   secrets referenced: DEPLOY_TOKEN│
└──────────────────────────────────────────────────┴───────────────────────────────────┘
```
- **Schema-validated live**; `Plan` shows the **resolved DAG + matrix expansion + referenced secrets**
  with **no runner spend** (the cost-saving shift-left, sketch 05). One editor render path — the same
  validator backs the CLI `myelin ci validate/plan` and the UI (no divergence).

### Error states (validation is the whole point of this screen)
```
schema-error:        │  ✗ Line 5: `uses` must be digest-pinned — `@sha256:…` required.    │  ← supply-chain (sketch 05): floating tag rejected, fail-closed
lint-warning:        │  ⚠ Line 6: matrix has no `os` axis; defaulting to linux.           │
unknown-secret:      │  ✗ Plan: secret `DEPLOY_TOKEN` is not defined for env `prod`.      │  ← caught BEFORE a run wastes compute
loading:             │  ░ validating…  (skeleton over the plan pane, editor stays usable) │
empty (new file):    │  No pipeline yet. Start from a template:  [Rust] [Node] [Blank]    │
```

---

## Screen 6 — Runner fleet / self-hosted runner management

```
┌ CI ─ Runners ──────────────────────────────────────── pool:[eu-west ▾] [+ Register]┐
│  ✓ healthy    runner-12   labels: gpu,large    3/4 jobs   attested ✓   eu-west       │
│  ⚠ degraded   runner-09   labels: linux        1/4 jobs   attested ✓   eu-west       │
│  ◌ offline    runner-04   labels: arm64        —          attested ✓   eu-west       │
│  ◇ pending    runner-21   labels: linux        —          ⏳ attesting…  eu-west       │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
- **Attestation status is a first-class trust cue** (`attested ✓` / `⏳ attesting`) — the supply-chain /
  self-hosted trust surface (sketch 05; P12/P15). Health, capacity, job-assignment visibility.
- **States:** *no runners* (empty: "Register a self-hosted runner, or use hosted pools" + register
  CTA), *loading* (skeleton rows), *pending-attestation*, *degraded*, *offline*, *error* (fails static).

---

## Screen 7 — Agent-surfaced triage view (failure-structured + proposed fix)

```
┌ Run #989 ─ ✗ Failed ─ Triage ──────────────────────────────────── ◆ TriageAgent ───┐
│  Structured failure:  step `test` · `login::expired_token` · expected 401, got 500   │
│  Log excerpt:  [view #step-test#L73]                                                  │
│ ── ◆ TriageAgent proposes (plan-then-apply) ───────────────────────────────────────  │
│   • Open issue ENG-412 "login returns 500 on expired token"        [✓ applied]        │
│   • Link RUN-991 ↔ ENG-412 ↔ PR #88                                [✓ applied]        │
│   • Post summary to #incidents                                     [✓ applied]        │
│ ── ◆ FixAgent proposes ──────────────────────────────────────────────────────────── │
│   • Open PR #88 with a fix              [◇ awaiting approval — protected repo]  [card→]│
│  Provenance: correlation #c-7741 · depth 2/12 · on behalf of: alia · [audit trail]    │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
- The **plan is shown before the effect** (§6.2): proposed effects as concrete reviewable items, each
  with its outcome (`✓ applied` / `◇ awaiting approval`). The gated `open PR` links to its approval card.
- **Attribution + provenance legible** (§6.4): `correlation_id`, **causal depth (`2/12` — the loop
  guard's ceiling visible)**, on-behalf-of, a link to the **tamper-evident audit trail**. "Why did this
  happen?" is answerable inline.
- The **agent is labelled** everywhere (`◆`), **no magic iconography** (§8b.3). The same UI serves mock
  and real runtimes (the strategy-pattern payoff).
- **States:** *failure-structured* (above), *agent-proposing* (effects pending), *awaiting-approval*,
  *approved/rejected*, *no-agent-attached* (a plain failure with a "Run triage agent" explicit action —
  CHAT-1 explicit-first, never auto-magic), *erased* (an actor in the trace was erased → pseudonym).

---

## Screen 8 — Usage / quota / billing (resource-seconds → credits)

```
┌ CI ─ Usage ───────────────────────────────────────────── period:[this month ▾]────┐
│  Used: 8,420 credits / 10,000     ████████████████░░░░  84% · ⚠ near limit          │
│  By repo:   acme/web 6,100 · acme/api 2,000 · acme/infra 320                         │
│  By class:  cpu 5,200 · gpu 2,800 · storage 300 · egress 120  (metered: resource-sec)│
│  ⚠ Near limit — runs will be refused at 10,000 (reserve gate). [Manage plan]         │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
- The meter is **resource-seconds** (wholesale), shown to the user as **credits** (markup) — the two
  kept separate (sketch 06; D8). **Reserve-gate honesty:** "runs will be **refused** at the cap" (no
  balance → no start; never a surprise infra bill — EI-03 §5.2), surfaced *before* exhaustion.
- **States:** *within-quota*, *near-limit* (above), *exceeded* ("New runs are paused — top up to
  resume; in-flight runs finish"), *throttled*, *loading* (chart skeleton), *error* (fails static).

---

## Cross-cutting checklist applied (design-language §5.10 / §8b)

| Requirement | Where honoured |
|---|---|
| Empty / loading / error / permission-denied / erased on every view | Screens 1–8 each enumerate them |
| Status never colour-alone (glyph + label) | every status token (`✓ Passed`, `✗ Failed`, `▸ Running`…) |
| Agents look like agents; no sparkle/emoji-as-UI | `◆ agent` badge; Screens 1,2,4,7 |
| Overlays portal to root + focus-trap + one z-index scale | approval card (4), secret-reveal confirm, menus |
| Loading = skeleton matching layout, not a spinner | Screens 1,2,3,5,6,8 |
| Error blames the system in one quiet line + a path; fails static | every error state |
| Shell pinned; each region own scroller (`min-height:0`) | live-log (3), run list (1), matrix |
| Flip popovers off-screen; test against real anchor; row-actions surfaced (touch) | run-list row actions (1); approval card placement |
| Humanised strings at the backend (NOTIF-1) + "why it fired" | relative times, notification reasons, attribution |
| Hard latency budgets (keyboard <100ms, no spinner-flash <1s) | the switch-test gate (T-7/T-8) — Phase 5 |
| Reversibility over confirmation, with the consequential carve-out | rollback is one action; deploy-to-prod + erase confirm/HITL |
