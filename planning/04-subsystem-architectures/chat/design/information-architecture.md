# Chat — Information Architecture

> Phase-4 design sketch (produced BEFORE the architecture stage; VISION §3/§5.4). Fits the **one-shell
> design language** (design-language §5.1 rail + contextual nav + header) and the **§7.5 view catalogue**.
> Companion sketches: [`user-flows.md`](./user-flows.md), [`wireframes.md`](./wireframes.md). Exploration:
> [`../sketches/`](../sketches/). Every screen here owes empty/loading/error states (the wireframes carry
> them).

---

## 1. Where Chat sits in the one shell

Myelin is **one shell**: a primary rail (subsystem switcher: Code · CI · Issues · Knowledge · **Chat** ·
Inbox · Search), a contextual secondary nav (the current subsystem's tree/list), the main content area,
and an optional right-hand **context pane** (design-language §5.1). Chat composes into this skeleton — it
owns only its **secondary nav (the conversation list)** and its **content + context pane**; the rail,
command palette, global search, the unified-inbox entry, and the identity menu are the shell's
(design-language §5.1). Switching to Chat must **never feel like switching apps** (P1).

```
┌──────┬────────────────────────────┬─────────────────────────────────────────┬──────────────────┐
│ RAIL │ SECONDARY NAV              │ MAIN CONTENT                              │ CONTEXT PANE     │
│      │ (Chat conversation list)   │ (message timeline / thread / search)      │ (optional)       │
│ Code │                            │                                           │                  │
│ CI   │  ▸ Channels                │  #incidents                          ⋯    │  Thread / agent  │
│ Issu │    # incidents      ● 3    │  ──────────────────────────────────────   │  detail, or      │
│ Know │    # release-2.4          │  [date separator]                         │  channel detail, │
│ ●Chat│    # general              │  grouped messages, unfurl cards, agent    │  or member       │
│ Inbox│  ▸ Direct Messages         │  posts (badge+provenance), HITL cards     │  roster, or      │
│ Srch │    @ alice                 │  ──────────────────────────────────────   │  unfurl "as-of"  │
│      │  ▸ Threads / Mentions      │  [ composer: rich text, /, @, #, ⏎ ]      │                  │
│ ⌘K   │  ▸ Saved                   │                                           │                  │
│ 🔔   │                            │                                           │                  │
│ 👤   │                            │                                           │                  │
└──────┴────────────────────────────┴─────────────────────────────────────────┴──────────────────┘
```

The shell is **pinned to the viewport** (`100vh` / `overflow:hidden`); each region is its own scroller; a
scrolling flex child carries `min-height:0` + overscroll-contain (design-language §8b.4) so the composer
never gets pushed below the fold — the single most common chat-shell layout bug.

---

## 2. The navigation hierarchy (Chat's secondary nav)

The conversation list (the secondary nav) is the spine. Sections (Phase-2 Chat §4.1; Phase-1 §3):

```
Chat (secondary nav)
├─ ▸ Channels                      (sortable; unread ● / mention badge ⊕; agent members marked)
│    ├─ # incidents          ● 3   public/private icon; artifact-linked channels show a 🔗 ref chip
│    ├─ # release-2.4              (artifact-linked → myelin://…/issue/REL-240)
│    └─ # general
├─ ▸ Direct Messages               (1:1 and group-DM, kind = dm | group_dm — Sketch 08)
│    ├─ @ alice                ⊕
│    └─ @ alice, bob (group)
├─ ▸ Threads / Mentions            ← a VIEW into the Notif inbox (C-9; Sketch 06), NOT a 2nd store
├─ ▸ Saved                         (per-user bookmarks/pins)
└─ + Create / Browse channels      (the empty-state create affordance, design-language §5.10)
```

- **Custom sections + drag-reorder** are per-user (Phase-2 Chat §4.1).
- **"Threads / Mentions"** is structurally a **scoped query into the one Notif inbox** (Sketch 06 / C-9):
  `Notif.list_inbox(me, filter = subsystem∈{chat} ∧ reason∈{mentioned, replied, thread_watched,
  approval_requested})`. It is **not** a Chat-owned inbox store — one read-state truth (Notif §1.3).
- **Agent members are visibly marked** in rosters and the list (the agent treatment; design-language
  §6.1) — agents are never disguised as humans.

---

## 3. The primary screens (from design-language §7.5)

Each is a §7.5 catalogue entry; the wireframes carry empty/loading/error/permission-denied/erased states.

| # | Screen | Role | Key context-pane use |
|---|---|---|---|
| S1 | **Conversation list (secondary nav)** | navigate channels/DMs/threads/saved; unread/mention badges | — |
| S2 | **Message timeline** | the channel/DM view: virtualised infinite scroll, grouped messages, date separators, "new messages" divider, inline unfurl cards, agent posts, HITL cards | opens thread / channel detail / unfurl as-of |
| S3 | **Composer** | rich-text over `myelin-content`; `/` slash menu, `@`-mention + `#`-artifact autocomplete (Search-backed), paste-URL→unfurl, code blocks, file upload, draft persistence | — |
| S4 | **Unfurl card** | live, per-viewer, permission-aware, **actionable** projection of an `ArtifactRef` (Sketch 04) | "as-of" snapshot on hover |
| S5 | **Thread pane** | side-by-side/overlay; where agent detail + **streaming agent output** live (Sketch 07) | the pane itself |
| S6 | **Activity / Mentions** | a VIEW into the one Notif inbox (C-9; Sketch 06) | — |
| S7 | **Search view** | messages + artifact-scoped; ACL-filtered (find only what you can see) | result preview |
| S8 | **Member roster / presence** | per channel; **agent presence class** (available/busy/rate-limited/offline) | the pane |
| S9 | **Channel detail / settings** | topic, membership, **linked artifacts**, notification prefs, **retention policy** (GDPR), **agent rules attached to the channel** | the pane |
| S10 | **Notification preferences** | per-channel/thread mute, keyword alerts, DND (prefs here; delivery is Notif) | — |
| S11 | **The HITL approval-card surface** | Chat is the primary home of agent approval cards (Sketch 06) — renders in the thread + the inbox | the card |
| S12 | **Agent provenance popover** | "why did this agent post?" — agent, on-behalf-of, trigger, audit link (Sketch 07) | popover (portal'd) |
| S13 | **Canvas (embedded Knowledge page)** | a pinned `knowledge/page` ref atop a channel — **embed, not Chat-native editor** (Sketch 08) | — |

The **CLI is a peer surface** (design-language §7.7; Phase-2 Chat §5): `myelin chat send|tail|list|history|
read|search|join|leave|create|archive|invite|dm|ref|react|pin|bookmark`, `--json` everywhere, the same
`myelin://…` `ArtifactRef` scheme as the UI. `chat tail --follow` is the CLI analogue of an open channel
(the SSE/WS stream — Sketch 01).

---

## 4. Navigation flows between screens (the "system assembles context" principle)

The platform principle: **the system assembles context; the user never does** (design-language §8b.6;
EI-05 §4). Chat's cross-screen links, all via the universal reference graph (P6):

- **Timeline → unfurl → artifact:** a `#issue`/PR/run reference renders an inline chip → hover peeks →
  click opens the **unfurl card** (S4) → click-through opens the artifact in *its* subsystem (Code/CI/
  Issues/Knowledge), pre-fetched. From a failing-CI unfurl, the card links the failing step → the line of
  code — the system pre-fetches the next hop (design-language §8b.6).
- **Timeline → thread:** click a reply-count / "open thread" → the **thread pane** (S5) opens beside the
  timeline (not a route change) — agent detail and streaming live here.
- **Mention → Activity → message:** a mention notifies (Notif item) → appears in **Activity** (S6) and the
  unified inbox → click jumps to the message in context, marking the inbox item read (the linked
  read-state, Sketch 06).
- **HITL card → run provenance:** an approval card (S11) links to the **provenance popover** (S12): which
  agent, on whose authority, triggered by which event, the live cost estimate, the audit link.
- **Command palette (⌘K, the shell's):** jump to any channel/DM/thread; run any Chat action; search —
  permission-pre-filtered via `list_objects` (you can only find what you may see; design-language §5.2/§5.7).

---

## 5. The right-hand context pane (Chat's use of the shell's pane)

The context pane (design-language §5.1) is **single-purpose by content, portal-free (it's chrome, not an
overlay)**, and shows one of: the **thread** (S5), the **channel detail/settings** (S9), the **member
roster/presence** (S8), or the **HITL card / agent provenance** (S11/S12). It is collapsible; on mobile it
becomes a **drawer overlay** (backdrop + Escape + route-change auto-close; design-language §8b.4) and the
timeline collapses (the `width:100%`-is-not-a-takeover rule — collapse the other column, don't lay the
pane beside it).

---

## 6. Responsive / mobile structure (Chat owns the width-takeover + hover-action cases — SUB-X)

The Phase-3 handoff assigns **Chat (with Issues) the hover-action and width-takeover responsive cases**
(SUB-X; EI-05 §1/§5). Concretely:

- **Hover is not touch-reachable** (design-language §8b.4): message **row actions** (react/reply/pin/
  bookmark/more) appear on hover on desktop but must be **surfaced by default or behind an explicit
  affordance** on touch (a long-press / a `⋯` button) — never hover-only.
- **`width:100%` is not a takeover:** the rail and secondary nav collapse to drawers at the mobile
  breakpoint so the timeline + composer actually fill the viewport (not laid beside a still-present
  column, clipped off-screen).
- **Flip popovers off-screen:** the composer's `@`/`#`/slash pickers are anchored to a **bottom-pinned
  composer** — they must **flip above with a max-height** when there's no room below (design-language
  §8b.4; tested against the *real* anchor, the composer, T-8).

---

## 7. What this IA commits

- Chat composes into the **one shell**; it owns only its conversation list + content + context pane.
- The secondary nav is **Channels / DMs / Threads-Mentions / Saved**; **"Threads/Mentions" is a VIEW into
  the one Notif inbox** (C-9 binding constraint), not a second store.
- **13 primary screens** (S1–S13) mapped to the design-language §7.5 catalogue; the CLI is a peer surface.
- Cross-screen navigation embodies **"the system assembles context"** via the reference graph.
- Chat owns the **hover-action + width-takeover + flip-popover** responsive cases (SUB-X), and pins the
  shell to the viewport with `min-height:0` scrollers so the composer never drops below the fold.
