# Sketch 06 — The HITL approval-card surface + the "Activity/Mentions" inbox-as-view (C-9)

> Exploration note. Chat's two named platform obligations: it **IS the HITL approval-card surface** (the
> withhold→approve→resume bridge renders here; the `approval` signal posts to the durable workflow), and
> the Chat **"Activity/Mentions" inbox is a scoped VIEW into the one Notif inbox** (C-9). Both are
> *already-decided platform contracts* (Workflow §6.3, Notif §1.3) — this sketch makes Chat's side concrete.

---

## Part A — The HITL approval-card round-trip (Chat's side of the bridge)

The platform warns (EI-03 §5.1): *"it's easy to ship the withhold logic and the card but forget the
bridge between them."* The full bridge is **already designed end-to-end** in Workflow §6.3, with three
owners: Agent Fabric (withholds the gated tool), Durable Workflow (the durable wait/signal), **Chat (the
card + the approval signal post)**. This sketch pins **Chat's exact role** so the bridge is wired, not
forgotten.

### The round-trip, with Chat's seat marked

```
1. Agent workflow hits a gated tool → ctx.wait_for_signal("approval:<call>", timeout=window)        [Workflow]
   → emits agent.approval.requested via OUTBOX, payload: { tool, args (ArtifactRefs), RISK,          [Agent Fabric]
                                                            LIVE COST ESTIMATE }                       (EI-03 §5.1)
   → Bus Signal tier routes it (reason = approval_requested, priority_class = critical)               [Bus/Notif]
   ┌──────────────────────────────────────────────────────────────────────────────────────────────┐
   │ 2. CHAT renders the APPROVAL CARD in the thread/channel where the agent run is anchored,        │ ← Chat
   │    AND as a Notif inbox item (reason=approval_requested) so it's never missed (C-9; §B).        │
   │    The card is HUMANISED at the backend (Notif §3.3 humanise; NOTIF-1) — the agent does no      │
   │    string work; args resolve per-viewer via Refs (a restricted arg → tombstone, never leaks).   │
   │    The workflow is now state='waiting', holding NO runtime, for up to `window` (may be DAYS).    │
   └──────────────────────────────────────────────────────────────────────────────────────────────┘
   ┌──────────────────────────────────────────────────────────────────────────────────────────────┐
   │ 3. A human clicks Approve / Edit / Reject on the card (in Chat).                                 │ ← Chat
   │    Chat calls Id.check(human, approve, run)  — approval authority is checked (Workflow §6.3).    │
   │    Chat calls DurableExecutor::signal(run, "approval:<call>", {approved|denied|edited, by:ref},  │
   │                                       idem_key = card_id)   — THE BRIDGE.                         │
   └──────────────────────────────────────────────────────────────────────────────────────────────┘
4. Signal lands in wf_signal (idempotent on card_id — a double-click is ONE approval).               [Workflow]
   Waiting workflow flips 'running', re-leases, replays to the wait, consumes the signal:            [Workflow]
     approved → gated TOOL_EXEC runs (step re-runs with the tool now allowed — AG-8)                 [Agent Fabric]
     denied   → tool WITHHELD (ordinary error to the loop, no mutation — AG-8); agent continues
     edited   → human-amended effect applied (design-language §6.3 "Edit lets a human amend")
     timeout  → the durable timer fired first → auto-deny path + notify
   → outcome announced back in the SAME thread (an agent message, humanised).                        [Chat]
```

### What Chat owns vs. consumes here

- **Chat owns:** the **card UI** (the §5.4 design-language shared component, rendered in the agent
  treatment), the **Approve/Edit/Reject affordance**, the **`Id.check(human, approve, run)`** gate before
  posting, and the **`DurableExecutor::signal(...)` call** (the bridge) with `idem_key = card_id` (so a
  double-click / retried click is one approval — idempotency is non-negotiable here).
- **Chat consumes:** `DurableExecutor::signal` (Workflow §5.1), `humanise` (Notif §3.3, so the card's
  pending-action + risk + **live cost estimate** are human-readable with routable links), `resolve`
  (per-viewer arg rendering, no leak), the `agent.approval.requested` Signal (to know a card is due).
- **Chat does NOT own:** the durable wait, the timer, the budget/cost, the withhold/resume logic — those
  are Workflow + Agent Fabric (Sketch's whole point is Chat is the *surface*, not the engine).

### Open product question (flag, with Workflow §6.3)

- **Batch approval:** can one card approve a *batch* of gated calls (e.g. "open PR + link issue + post")?
  Workflow §6.3 flags this `[OPEN → P4 joint]`. Lean: **yes, a card can present a multi-effect plan**
  (design-language §6.2 "agents propose: open PR #88, link issue ENG-412, post to #incidents" — the plan
  is shown as concrete items), with **per-effect Approve/Reject** plus an "approve all" — but each effect
  still resolves its own gate signal (idem per effect), so a partial approval is well-defined. → architecture.
- **Where the card anchors:** the agent run is anchored to the thread/channel that triggered it
  (causality: `correlation_id` threads the run to the originating message — design-language §6.4). The
  card renders *there* + in the inbox. → architecture (the anchoring rule).

---

## Part B — "Activity/Mentions" is a scoped VIEW into the one Notif inbox (C-9)

**Decided platform contract (Notif §1.3):** there is exactly **one** cross-subsystem inbox, owned by
Notif. Chat's "Activity/Mentions" is **a filtered query into it — not a second store**:

```
Chat "Activity / Mentions"  =  Notif.list_inbox(me,
    filter = subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied, thread_watched, approval_requested})
    ranked by priority DESC
```

(verbatim from Notif §1.3's C-9 table). The load-bearing consequences for Chat:

- **Chat does NOT build a mentions inbox.** It renders a *view* over `Notif.list_inbox` with a chat-scoped
  filter. One store → **one read-state truth**: marking a mention read in Chat's Activity view is the
  *same row* as the unified inbox (Notif §1.3) — read it in Chat, it's read everywhere.
- **The rule that keeps it true:** every inbox item carries a structured `reason` + a `subject`
  `ArtifactRef` (Notif §2.1); Chat's view is a `filter` over those, served by the same `list_inbox`
  contract. Chat **adds a filtered view, never a second store** — this is the C-9 design rule made
  structural (Notif §1.3), and it is a *binding constraint on Chat's Phase-4 design* (the prompt's C-9
  obligation).
- **The link to Chat read-state (Sketch 03):** opening a channel and scrolling past a mentioned message
  marks the corresponding Notif inbox item `read` (Chat calls `Notif.mark(item, read)`), and conversely an
  inbox item snoozed in the unified inbox doesn't re-badge in Chat. The two read-states (Chat's per-channel
  scroll position, Notif's per-item state) are *linked at the mention*, not duplicated.
- **HITL cards land here too:** `reason = approval_requested` is a chat-scoped Activity item *and* a
  high-priority unified-inbox item (Part A step 2; design-language §5.8 "HITL approvals appear in the inbox
  too, so a human gate is never missed"). The card is a second home of the §5.4 component.

### Why this matters (the platform thesis)

The platform exists partly to fix **three inbox-like surfaces fragmenting attention** (Notif §1.3 "Why it
matters"; P8). If Chat built its own mentions store, we'd recreate the exact disease. The C-9 contract
forbids it; Chat honours it by being a *view*, not a store. Design-language one-liner (carried to UX):
*there is one inbox; everything else is a saved filter on it.*

---

## What this sketch hands forward

- **Chat is the HITL approval-card *surface*; the bridge is `Id.check(approve)` → `DurableExecutor::signal(
  run, name, payload, idem_key=card_id)`** — wired, idempotent, humanised, per-viewer-safe. The wait,
  timer, budget, and resume are Workflow + Agent Fabric.
- **Chat "Activity/Mentions" is `Notif.list_inbox` with a chat filter** — never a second inbox store (C-9
  binding constraint). One read-state truth, linked to Chat's per-channel scroll position at the mention.
- **HITL cards also appear in the unified inbox** so a gate is never missed.
- Open (flag to architecture, joint with Workflow): batch/multi-effect approval cards; card anchoring rule.
