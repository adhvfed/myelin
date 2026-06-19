# Chat — Wireframes (primary screens, with empty / loading / error states)

> Phase-4 design sketch (BEFORE architecture; VISION §3/§5.4). ASCII wireframes of the **primary screens**
> from the design-language §7.5 catalogue, each showing **empty / loading / error** (and where relevant
> permission-denied / erased / agent-pending) — not just the happy path (design-language §5.10/§8b). Applies
> the **day-one UX primitives** (design-language §8b): overlay/portal rules, one editor render path,
> measured tokens, layout-containment, humanised strings. Companion:
> [`information-architecture.md`](./information-architecture.md), [`user-flows.md`](./user-flows.md).

> **Conventions used below.** `●` unread dot, `⊕` mention badge, `[agent]` = the agent treatment
> (badge+label, **never colour alone**, **no sparkle/magic iconography** — §8b.3). Overlays (menus,
> popovers, pickers, the HITL card when floating) **portal to the document root** with the one z-index
> scale `chrome < popover < modal < toast` (§8b.1). Loading shows **skeleton structure**, never a spinner
> on blank (§8b.6). Errors **blame the system in one quiet line + a path** (§5.10).

---

## S2 — Message timeline (the channel view) — the primary screen

### Happy path
```
┌──────────────────────────────────────────────────────────────────────────┬─ context pane ─┐
│ # incidents   🔒private   3 members   🔗 myelin://…/issue/REL-240    ⋯  ⓘ │  (collapsed)    │
├──────────────────────────────────────────────────────────────────────────┤                │
│  ── Tuesday, 18 June ───────────────────────────────────────────────────  │                │
│                                                                            │                │
│  alice  10:14                                                              │                │
│  main is red — investigating                                              │                │
│                                                                            │                │
│  alice  10:14   (grouped)                                                  │                │
│  Looking at ┌───────────────────────────────────────────────┐            │                │
│             │ ⌥ PR #88  Fix flaky deploy step     ✓ checks ●3 │ ← unfurl   │                │
│             │ Git · open · reviewers: bob          [Approve]  │   (S4)     │                │
│             └───────────────────────────────────────────────┘            │                │
│                                                                            │                │
│  [agent] triage-bot  10:15   ⓘ why?                                        │                │
│  🔴 main red — opened ISSUE-412, triaging          ← humanised (NOTIF-1)   │                │
│                                                                            │                │
│  ── New messages ──────────────────────────────────────  [jump to latest] │                │
│                                                                            │                │
│  bob  10:16   ↳ 4 replies                            [ open thread → S5 ]  │                │
│                                                                            │                │
├──────────────────────────────────────────────────────────────────────────┤                │
│ [ Reply in #incidents…   /  @  #  ⧉code  📎 ]                          ⏎  │  ← composer S3 │
└──────────────────────────────────────────────────────────────────────────┴────────────────┘
```
- Hover on a message reveals **row actions** (react/reply/pin/⋯) on desktop; on touch they are a **default
  `⋯` affordance**, never hover-only (§8b.4; SUB-X hover-action case — Chat owns it).
- The composer is **bottom-pinned**; the shell is `100vh`/`overflow:hidden`; the timeline is the scroller
  with `min-height:0` so the composer never drops below the fold (§8b.4).

### Empty (onboarding-forward — §5.10)
```
│                          ╭───────────────────────────────╮                 │
│                          │   No messages yet in #incidents │                 │
│                          │   Start the conversation, or    │                 │
│                          │   link an artifact to this      │                 │
│                          │   channel.                      │                 │
│                          │   [ Send a message ]  [ Link… ] │ ← create action │
│                          ╰───────────────────────────────╯                 │
```

### Loading (skeleton matching final layout — §8b.6, never a blank spinner)
```
│  ░░░░░  ░░:░░                                                               │
│  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░                                             │
│  ░░░░░  ┌─────────────────────────────┐  ← unfurl skeleton                  │
│         │ ░░░░░░░░░░░░░░░  ░░░░░░       │                                    │
│         └─────────────────────────────┘                                    │
│  ░░░░░░░░░░░░░░░░░░░                                                        │
```

### Error (system-blamed, one line, with a path; degraded fails static — §5.10/§8b.6)
```
│  ⚠ Couldn't load history — temporarily unavailable.   [ Retry ]            │
│    (live updates paused · reconnecting…)            ← resync indicator      │
```
On reconnect, the **resync** streams the gap from the durable log (Sketch 01) before live resumes —
**zero messages lost across the reconnect** (the owed drill).

---

## S3 — Composer (one editor render path; §8b.2)

### Happy path + the `@`/`#`/slash pickers (portal'd, flip-above a bottom-pinned composer)
```
┌──────────────────────────────────────────────────────────────────────────┐
│  ┌──────────────────────────────────────────────────────────────┐ flip ↑ │  ← picker FLIPS
│  │ @ali|                                                          │        │     ABOVE (no room
│  │   ┌────────────────────────────────────────────────────────┐  │        │     below the pinned
│  │   │ @ alice        Alice Ng                                 │  │        │     composer) — §8b.4
│  │   │ @ alice-bot   [agent]  triage agent                     │  │        │     tested vs the
│  │   │ @ ali-team     Aliasing team                            │  │        │     REAL anchor (T-8)
│  │   └────────────────────────────────────────────────────────┘  │        │
│  └──────────────────────────────────────────────────────────────┘        │
│ [ Message #incidents…   /=actions  @=people/agents  #=artifacts  ⧉  📎 ] ⏎ │
└──────────────────────────────────────────────────────────────────────────┘
```
- **One editor render path** (§8b.2): read and edit run the **same inline parser**; the body is a
  **markdown-subset string** with `mention`/`artifact_ref`/`embed` as **structured nodes** (ADR-05/KN-2);
  `render(parse(md)) === md` is the round-trip gate (T-5). **Controlled `contenteditable`, not
  `<textarea>`** (can't show formatting as you type); caret = char-offset into the serialised markdown.
- `#` autocomplete is **Search-backed** (`#ABC-` → issue suggestions; paste a PR URL → offer unfurl) — the
  differentiating surface (Phase-1 §2.4). Slash `/` opens the **server-side slash-command** menu.

### Error (send failed — optimistic, honest rollback, idempotency-safe)
```
│  alice  now   ⚠ Not sent — couldn't reach the server.   [ Retry ]  [ ✕ ]  │
```

### Empty (draft restored)
```
│ [ Message #incidents…  ·  draft restored ]                              ⏎  │
```

---

## S4 — Unfurl card (live, per-viewer, permission-aware; Sketch 04)

### Happy (viewer CAN see) — actionable
```
┌───────────────────────────────────────────────────────────┐
│ ⌥ PR #88   Fix flaky deploy step                  ✓ 3 checks│
│ Git · open · base main ← feat/fix · reviewers: bob          │
│ ──────────────────────────────────────────────────────────│
│ [ Approve ]  [ Open PR → ]                  as of 10:14 ⓘ  │ ← live; "as-of" on hover (audit)
└───────────────────────────────────────────────────────────┘
```

### Loading (skeleton)
```
┌───────────────────────────────────────────────────────────┐
│ ⌥ ░░░░  ░░░░░░░░░░░░░░░░░░░░░░                              │
│ ░░░ · ░░░░░ · ░░░░░░░░░░░░░░░                               │
└───────────────────────────────────────────────────────────┘
```

### Permission-denied (viewer CANNOT see — the no-leak card; §5.10/ADR-03; Sketch 04)
```
┌───────────────────────────────────────────────────────────┐
│ 🔒 A restricted pull request                               │ ← title NEVER shown; tombstone
│    You don't have access to this artifact.                 │   from refs.resolve → Tombstone
└───────────────────────────────────────────────────────────┘
```

### Erased / deleted (graceful tombstone — §5.10/ADR-12; Sketch 05)
```
┌───────────────────────────────────────────────────────────┐
│ ⊘ This artifact was deleted.                               │
└───────────────────────────────────────────────────────────┘
```

### Error (owning subsystem unreachable — resilient-client degraded; fails static, §8b.6)
```
┌───────────────────────────────────────────────────────────┐
│ ⚠ Couldn't load preview — temporarily unavailable.  [Retry]│
└───────────────────────────────────────────────────────────┘
```

---

## S11 — The HITL approval card (Chat is the surface; Sketch 06 / Flow 3)

### Agent-pending (the trust-bearing state — design-language §5.4/§6.3)
```
┌──────────────────────────────────────────────────────────────────────┐
│ [agent] FixAgent proposes an action — awaiting your approval     ⓘ why?│ ← agent treatment,
│ ─────────────────────────────────────────────────────────────────────│   no sparkle (§8b.3)
│ Plan (plan-then-apply):                                               │
│   • Open PR #88  →  Fix flaky deploy step   (on myelin://…/git/repo/x) │ ← proposed effect,
│ Scope: on-behalf-of @alice · delegation: repo:write (attenuated)      │   per-viewer args
│ Risk: protected repo · Est. cost: 0.12 credits     ← live cost (EI-03) │   via refs.resolve
│ ─────────────────────────────────────────────────────────────────────│
│ [ Approve ]   [ Edit… ]   [ Reject ]               waits up to 24h ⏲   │ ← durable wait;
└──────────────────────────────────────────────────────────────────────┘   never blocks
```
- Clicking **Approve** → `Id.check(human, approve, run)` → `DurableExecutor::signal(run, …, idem_key =
  card_id)` (idempotent; a double-click is one approval — Sketch 06). **Edit** amends the effect first
  (design-language §6.3 "human stays in control of the *content*"). **Reject** withholds (ordinary error,
  no mutation — AG-8).
- The card also lands in the **unified inbox** (C-9) so the gate is never missed (§5.8).

### Resolved (after approval)
```
│ ✓ Approved by @alice · PR #88 opened → [ open ]            10:21        │
```

### Timed out (the durable timer fired first — Flow 3)
```
│ ⏲ Expired — auto-denied after 24h. FixAgent continued without this step.│
```

### Error (signal post failed)
```
│ ⚠ Couldn't record your decision — please retry.  [ Approve ] [ Reject ]  │
```

---

## S5 — Thread pane (agent detail + streaming; Sketch 07)

### Agent-pending / streaming
```
┌─ Thread · bob 10:16 ───────────────────────────── ✕ ┐
│ root: "deploy step is flaky again"                   │
│ ──────────────────────────────────────────────────  │
│ [agent] triage-bot   working…  ▍                     │ ← streaming partials (firehose);
│ Investigating the failing step and recent commits…   │   "working…" affordance, no magic
│ ──────────────────────────────────────────────────  │   iconography (§8b.3)
│ [ Reply in thread…                              ] ⏎  │
└──────────────────────────────────────────────────────┘
```
On `Submit` the streamed text resolves into a normal **agent-attributed** message (provenance popover
available). A reconnect mid-stream re-fetches the final/in-progress from the durable log — never a
half-message (Sketch 01/07).

### Empty
```
│  No replies yet — start the thread.   [ Reply ]      │
```

---

## S6 — Activity / Mentions (a VIEW into the one Notif inbox; C-9 / Sketch 06)

```
┌─ Activity / Mentions ─────────────────────────────────────────────┐
│ (= Notif.list_inbox(me, filter: chat ∧ {mentioned,replied,approval})│ ← NOT a 2nd store
│ ─────────────────────────────────────────────────────────────────  │
│ ⊕ @you mentioned in #incidents · alice · 2m   why? ⓘ   [ go ]      │
│ ↳ reply in your thread · #release-2.4 · 5m            [ go ]        │
│ [agent] approval awaiting you · FixAgent · 8m  (critical) [ review ]│ ← HITL also here (§5.8)
└───────────────────────────────────────────────────────────────────┘
```
- Each item carries **"why it fired"** provenance (NOTIF-2) and a routable link; reading here marks the
  **same** read-state as the unified inbox (one truth — C-9).

### Empty
```
│  You're all caught up. Nothing needs you right now.                 │
```

---

## S8 — Member roster / presence (agent presence class; Sketch 07)

```
┌─ Members · #incidents ──────────────────┐
│ ● alice          online                  │
│ ◐ bob            away                     │
│ [agent] triage-bot   busy     ← fabric-  │ ← agent class: available/busy/
│ [agent] fix-bot      rate-limited  health│   rate-limited/offline; glyph+label,
│ ○ carol          offline                 │   NEVER colour alone (§8b.3/§4)
└──────────────────────────────────────────┘
```

---

## S9 — Channel detail / settings (GDPR-relevant: retention; agent rules)

```
┌─ #incidents · settings ───────────────────────────────────────────┐
│ Topic        [ incident coordination …                          ]  │
│ Linked       🔗 myelin://…/issue/REL-240            [ + link ]     │
│ Members      3   [ manage ]                                        │
│ Notifications  per-channel mute / keyword alerts / DND  [ edit ]   │
│ Retention    ⏲ auto-delete after [ 90 ] days   (GDPR-relevant)     │ ← purges ALL derived
│ Agent rules  which agents/triggers act here   [ manage ]          │   stores (Sketch 05)
│ ───────────────────────────────────────────────────────────────  │
│ [ Archive channel ]                          (requires approval)   │
└────────────────────────────────────────────────────────────────────┘
```

---

## S7 — Search view (ACL-filtered; §5.7)

### Happy
```
┌─ Search ──────────────────────────────────────────────────────────┐
│ [ deploy failed              ]  in:#incidents  refs:issue,ci  7d ▾ │
│ ─────────────────────────────────────────────────────────────────  │
│ #incidents · alice · 18 Jun   "…deploy failed on step 3…"   [ go ] │ ← only channels
│ ⌥ ci/run/4412  failed  step-3                              [ go ] │   you're in
└───────────────────────────────────────────────────────────────────┘
```
Permission-pre-filtered via `list_objects` (you can only find what you may see — §5.7; the
`search-requires-acl-filter` lint). **Empty** distinguishes *no-query* ("Search messages and artifacts…")
from *zero-results* ("No matches in channels you can see."). **Error:** "Search is temporarily
unavailable. [Retry]".

---

## S13 — Canvas (an embedded Knowledge page, NOT a Chat editor; Sketch 08)

```
┌─ #incidents ─ pinned canvas ──────────────────────────────────────┐
│ 📄 Incident #REL-240 runbook   (Knowledge page · embedded)  [open] │ ← myelin://…/knowledge/page/…
│ ─────────────────────────────────────────────────────────────────  │   Chat PINS; Knowledge
│ (rendered read-view of the Knowledge page via the ONE editor       │   AUTHORS (one editor
│  render path — §8b.2; edits happen in Knowledge, not here)         │   render path, no dup)
└───────────────────────────────────────────────────────────────────┘
```
Empty: "Pin a Knowledge page as this channel's canvas. [ Choose page… ]".

---

## Cross-cutting state checklist (applied to every screen — §5.10)

| State | Treatment |
|---|---|
| **Empty** | onboarding-forward, offers the create action (S2/S5/S6/S13 above) |
| **Loading** | skeleton matching final layout, never a blank spinner; suppress flash < ~1s (§8b.6) |
| **Error** | system-blamed, one quiet line + a path (retry/reconnect); degraded **fails static** ("temporarily unavailable" for *that* surface only) |
| **Permission-denied** | the no-leak "restricted" card/row (S4) — title never shown (ADR-03) |
| **Erased/tombstoned** | graceful "deleted/erased" placeholder (S4) — never a dangling leak (ADR-12) |
| **Agent-pending** | the "working…/awaiting approval" state (S5/S11), agent treatment, no magic iconography |

## Day-one UX primitives applied (design-language §8b — the testable mandates)

- **Overlays portal to root, one z-index scale, focus-trap/return-focus/scroll-lock/Escape/ARIA in the
  primitive** (§8b.1): the `@`/`#`/slash pickers, the provenance popover (S12), the HITL card when
  floating, menus.
- **One editor render path** (§8b.2): composer read==edit parser; markdown-subset string + structured
  nodes; controlled `contenteditable`; `render(parse(md)) === md` gate.
- **Measured tokens** (§8b.3): status by glyph+label+position (presence, unread, agent class) never colour
  alone; the agent treatment has **no sparkle/magic-wand iconography, no emoji as UI**; never set colour via
  inline style on an interactive element.
- **Layout containment** (§8b.4): `100vh`/`overflow:hidden` shell, `min-height:0` scrollers (composer never
  drops below the fold), `width:100%`-is-not-a-takeover (mobile collapses columns), **flip-popovers-above**
  a bottom-pinned composer tested against the real anchor, hover-actions touch-reachable.
- **Humanised strings** (§8b.5): all machine strings (`"merge_request merged"`, agent posts, notif reasons)
  humanised **at the backend** (Notif `humanise` + Refs display-resolution), never a frontend string map —
  every agent-authored message inherits it (NOTIF-1).
