# Chat — 04 · Views, CLI & API / Agent-Tool Surface

> See [`00-overview.md`](./00-overview.md) for framing. This doc indexes the **views** (the detailed UX lives in
> [`../design/`](../design/) — information-architecture, user-flows, wireframes — PRESERVED from Phase 4), the
> **CLI** commands, and the **API / agent-tool** surface. Every screen owes empty/loading/error states (carried
> in the wireframes); the composer reuses the **one editor render path** over the **frozen `myelin-content` Chat
> subset** (contract 13.1; X-2).

---

## 1. The views (full detail in [`../design/wireframes.md`](../design/wireframes.md))

Chat composes into the **one shell** (rail + secondary nav + main content + optional context pane;
design-language §5.1) and owns only its **conversation list**, **content area**, and **context pane**. Switching
to Chat must never feel like switching apps. The 13 primary screens:

| # | Screen | Role | Architecture tie |
|---|---|---|---|
| S1 | **Conversation list** (secondary nav) | navigate channels/DMs/threads/saved; unread/mention badges | `membership_by_principal` index ([01 §2.1](./01-tech-and-data-model.md)) |
| S2 | **Message timeline** | virtualised infinite scroll, grouped messages, date separators, "new messages" divider, inline unfurl cards, agent posts, HITL cards | the message log + `resume` ([02 §1–2](./02-internals-and-algorithms.md)); lazy-on-viewport unfurls ([02 §4](./02-internals-and-algorithms.md)) |
| S3 | **Composer** | rich-text over the **frozen `myelin-content` Chat subset**; `/` slash menu, `@`-mention + `#`-artifact autocomplete (Search-backed), paste-URL→unfurl, code blocks, draft persistence | the **one editor render path** (`render(parse(md)) === md`, WASM core); body = markdown-subset string + the three structured nodes ([01 §1.4](./01-tech-and-data-model.md)) |
| S4 | **Unfurl card** | live, per-viewer, permission-aware, **actionable** projection of an `ArtifactRef` | `refs.resolve` + the shared-per-ref cache over the 4-step ladder ([02 §4](./02-internals-and-algorithms.md)) |
| S5 | **Thread pane** | where agent detail + **streaming output** live | `thread_root_id` (the frozen `thread-<root>` `#sub` kind); firehose partials + durable final ([02 §7.3](./02-internals-and-algorithms.md)) |
| S6 | **Activity / Mentions** | a **VIEW** into the one Notif inbox (C-9) | `Notif.list_inbox(filter=chat∧…)` — never a 2nd store ([02 §5.3](./02-internals-and-algorithms.md)) |
| S7 | **Search view** | ACL-filtered messages + artifact-scoped | `query` always conjoined with the frozen `list_objects` `Filter` ([03 §7](./03-events-contracts-and-glue.md)) |
| S8 | **Member roster / presence** | per channel; **agent presence class** | firehose presence ([02 §7.2](./02-internals-and-algorithms.md)) |
| S9 | **Channel detail / settings** | topic, membership, linked artifacts, notif prefs, **retention** (GDPR), **agent rules** | `conversation.retention_days`, `linked_ref` ([01 §2](./01-tech-and-data-model.md)) |
| S10 | **Notification preferences** | per-channel/thread mute, keyword alerts, DND | `membership.notif_pref`; delivery is Notif's |
| S11 | **HITL approval card** | Chat is the primary home — renders in thread + inbox | the bridge with the per-effect `idem_key` ([02 §5](./02-internals-and-algorithms.md)) |
| S12 | **Agent provenance popover** | "why did this agent post?" — agent, on-behalf-of, trigger, audit link | `causation_id`/`correlation_id`/`on_behalf_of` ([02 §7.5](./02-internals-and-algorithms.md)) |
| S13 | **Canvas** | a pinned `knowledge/page` ref atop a channel — **embed, not Chat editor** | `conversation.pinned_canvas`; Knowledge authors ([05 §6](./05-hard-problems.md)) |

**Responsive cases Chat owns (SUB-X):** the **hover-action** case (message row actions are hover-on-desktop but
a default `⋯`/long-press affordance on touch — never hover-only), the **width-takeover** case (rail + secondary
nav collapse to drawers at the mobile breakpoint so timeline+composer fill the viewport), and the **flip-popover**
case (the `@`/`#`/slash pickers anchored to a bottom-pinned composer flip *above* with a max-height when there's
no room below — tested against the *real* anchor). The shell is pinned `100vh`/`overflow:hidden` with
`min-height:0` scrollers so the composer never drops below the fold (design-language §8b.4).

---

## 2. The CLI surface (a peer surface; design-language §7.7)

`myelin chat <verb>`, `--json` everywhere, the same `myelin://…` `ArtifactRef` scheme as the UI:

```
myelin chat send '#incidents' "main is red"            # post (optimistic; idempotency nonce; outbox-coherent)
myelin chat send '#incidents' --thread <root> --reply  # reply in a thread
myelin chat tail '#incidents' --follow                 # the SSE/WS stream over the frozen resume-cursor protocol
myelin chat list                                       # my conversations (membership_by_principal)
myelin chat history '#incidents' --since <cursor>      # ordered range read (paginate / scroll-back)
myelin chat read '#incidents'                          # mark-read (read-state)
myelin chat search 'deploy failed' --in '#incidents'   # ACL-filtered (frozen list_objects Filter conjoined)
myelin chat join|leave|create|archive '#x'             # lifecycle (create/archive → requires_approval)
myelin chat invite '#x' @alice                         # membership (requires_approval)
myelin chat dm @alice "ping"                           # open/post a DM
myelin chat ref '#incidents' myelin://…/git/pr/88       # attach an artifact_ref → unfurl + refs.edge
myelin chat react <msg> ✅                              # reaction (may be an explicit approve-action)
myelin chat pin|bookmark <msg>                         # saved/pins
```

- **Ops example (the firehose + delegation):** `tail -n 40 build.log | myelin chat send '#incidents' --thread
  "$T" --reply --as ci-bot` — the body is a structured `code` block; `--as ci-bot` is authorized because the CI
  job token is allowed to **act as** that agent (`delegation`, contract 4.5). Same `chat.message.created` event
  as a human send → an `@oncall` mention can page via Notif's escalation chain (contract 7.5, on the
  `myelin-flow` timer wheel).
- The CLI goes through the **resilient client** (contract 1.9) and the same public surface as the UI — no
  carve-out. `myelin chat tail` reconnects via `resume(stream, scope, last_seq)` (contract 3.5), losing zero ops.

---

## 3. The API / agent-tool surface

- **Public surface (gateway-fronted, identity-injected):** the WS/SSE connection endpoint (the frozen
  `subscribe/resume/scope` protocol) + the REST/RPC for send/edit/react/list/history/search/lifecycle. **Tenant
  from the verified token, never the path** (ID-3).
- **Agent-tool surface:** the `ToolDef` set ([03 §8](./03-events-contracts-and-glue.md)) — `chat.post`,
  `chat.reply_in_thread`, `chat.react`, `chat.start_dm`, `chat.create_channel`, `chat.invite`,
  `chat.archive_channel` — with the **frozen `requires_approval` defaults** (X-6); all side-effecting tools route
  through **`EffectApi`** (plan-then-apply, reserves; no carve-out). Agent dispatch is **explicit-first** (CHAT-1):
  a mention notifies an agent's inbox; it does not auto-spawn a costed run.
- **The projection API** `project(ref, viewer)` ([03 §3](./03-events-contracts-and-glue.md)) is the internal-RPC
  surface *other* subsystems call (via Refs) to unfurl a chat artifact — the only way to read about chat
  artifacts (no cross-DB).
- **`run --dry-run`** (contract 8.7) on `chat.*` tools returns the `ProposedEffect`s without applying —
  plan-then-apply testability.

Continue to [`05-hard-problems.md`](./05-hard-problems.md).
