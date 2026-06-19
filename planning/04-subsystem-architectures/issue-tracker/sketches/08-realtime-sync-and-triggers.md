# Sketch 08 — Real-time sync engine + the stateful Trigger ("unblock me when…") UX

> Exploration note. Weighs Phase-2 §11 Q10 / deep-dive §6.6: the Linear-class real-time sync engine; and the
> ISS-1 stateful-Trigger UX I own. Leans; commit in `00-findings.md`.

## Part A — Real-time sync (Linear-class optimistic UX)

Linear set the bar (deep-dive §5.5, §6.6): optimistic local updates, instant filter, live multi-user board/issue
changes, presence, a sync engine + client cache. This is a **cross-cutting frontend item** (deep-dive §6.6;
design-language P2) with deep architecture implications. The question: what's the sync substrate?

### What's already given
- The **event bus** delivers every state change (`issue.*` events, per-issue ordered, references-not-payloads).
- **Presence/typing/cursor** ride the **ephemeral firehose**, never the durable bus (event-bus §4.3; Chat/Knowledge
  own collab transport; design-language §5.11 "presence rides the firehose").
- **Optimistic update + honest rollback** is the platform default (design-language §8b.6 / P2): the client
  applies a mutation locally, the server confirms or returns authoritative state to reconcile.

### Candidate A — Per-entity subscriptions over the bus + a client cache/sync protocol (the Phase-2 direction)
The client holds a normalised cache of the issues in view; it **subscribes** (via the firehose/a sync gateway)
to the `issue.*` events for that scope (a board, a view, an issue); on an event it patches the cache; its own
mutations are optimistic with a server-confirm/rollback. (deep-dive §6.6; this is the Linear sync-engine shape.)

- **For:** reuses the bus as the change-source (one source of truth for "what changed"); presence on the
  firehose; no second real-time engine. Mutations go through the same permissioned API (CLI/UI/agent parity).
- **For:** **board live-drag + agent-moved-card** (S3 states) fall out: an agent's `issue.update` emits an event
  the subscribed boards patch — the agent-moved card appears live, labelled (design-language §6.1).
- **Cost / risk:** the subscription-fanout-per-viewer + the cache-coherence protocol (what to refetch on
  reconnect, how to bound a huge board's event stream) is real work — and it overlaps Knowledge's resume-cursor
  transport (KN-1) and Chat's connection tier (TE-21). **Lean: reuse the shared firehose + a resume-cursor
  subscription** (the KN-1 reconnect-loses-zero-ops substrate is exactly what a board sync needs on reconnect),
  rather than invent an Issues-specific socket protocol. Where the issue *description* needs concurrent editing,
  it is **single-author-at-a-time** (ADR-05 / Phase-2 §3 table — NOT the Knowledge CRDT), so issue-body
  concurrency is the CAS floor (sketch 06's arbitration family), not collab editing.

### Candidate B — Poll / refetch on interval
- **Against:** not Linear-class; wastes the bus; rejected (design-language P2).

### Floor vs follow-on
**Floor:** optimistic UI + bus-driven cache invalidation over the firehose with a resume-cursor on reconnect
(no silent gap). **Follow-on (named):** a richer offline/local-first mode (deep-dive §4.1 "[UNCERTAIN — offline
scope]"; design-language §9 open) — *out of scope for v1 unless promoted*; the optimistic+resume floor is the
v1 bar. Real-time board collab does **not** need a CRDT (coarse-grained mutations, server-arbitrated — sketch 06).

## Part B — The stateful Trigger ("unblock/remind me when…") — ISS-1, the UX I own

This is the **flagship agent-adjacent UX** the Phase-3 handoff explicitly hands me (ISS-1; event-bus §3.6/§4.6).
A **Trigger** is a *stateful per-person promise*: "remind me / re-surface this when ENG-1421 becomes unblocked,"
"ping me when this leaves triage," "notify me when this initiative goes at-risk." Distinct from an *automation*
(stateless per-event reflex) — a Trigger is **armed → {resolved | stale | disarmed}**, fires **once per
arming**, and its `stale_after` is a `myelin-flow` durable timer (event-bus §3.6).

### What I own vs consume
- **Consume:** `arm_trigger / disarm_trigger(Trigger{ owner, condition, arms_subject, on_resolve, stale_after })`
  — the bus primitive (event-bus §3.6). The `condition` is a safe query-AST `EventMatcher`. The `stale_after`
  durable timer is `myelin-flow`'s (durable-workflow §4.2). The `on_resolve` is a Notif inbox item (the resolve
  fires a notification into the **one** inbox — Notif §1.3).
- **Own:** the **Issues-side UX and semantics** — *what conditions are armable on an issue* and *how it's
  surfaced*. The armable conditions read Issues state: "becomes unblocked" = all `blocked_by` edges
  (issue_relation, sketch 05) resolve; "leaves state X"; "assigned to me"; "SLA at risk"; "initiative health
  changes." These compile to `EventMatcher` predicates over `issue.*` events.

### The UX (detailed in user-flows + wireframes)
- On a **blocked issue** (S1, transition-blocked state): a one-click **"Remind me when unblocked"** affordance
  next to the blocker list → arms a Trigger `{ owner: me, condition: all blocked_by resolved, arms_subject:
  ENG-1421, on_resolve: inbox notification, stale_after: 30d }`. The issue shows a subtle "you'll be notified
  when unblocked" pending state.
- When the last blocker closes → `issue.relation` / `issue.transitioned` events → the bus resolves the Trigger
  → **one** inbox item: "ENG-1421 is now unblocked" (humanised at the backend — NOTIF-1 — with the routable
  ArtifactRef). Fires **once**; then disarmed.
- If 30 days pass with no resolution → `stale_after` fires (the `myelin-flow` timer) → a "still blocked after
  30d — want to escalate?" inbox nudge → Trigger goes stale. No silent forever-armed promises.
- **"Remind me" generalises** beyond unblock: any armable condition gets the same one-click affordance + the
  same pending/resolved/stale lifecycle. This is the "the system assembles context" principle (design-language
  §8b.6) — the user states an *intent* ("tell me when this matters again"); the platform watches.

### Why this is the make-or-break agent-adjacent UX
It's the human-facing half of agent-native: instead of an agent *doing* something, the platform *watches on your
behalf* and re-surfaces precisely when relevant — calm-by-default (design-language P8), zero polling, durable
across restarts/days (the `myelin-flow` timer). It's "My Work that comes to you" rather than a list you check.

## Leaning

- **Part A:** optimistic UI + bus-driven cache over the shared firehose with a **resume-cursor on reconnect**
  (reuse KN-1's substrate, don't invent); issue-body concurrency is single-author CAS, board concurrency is
  server-arbitrated (sketch 06), **no Issues CRDT**. Offline/local-first is a named follow-on, not v1.
- **Part B:** own the Issues-side Trigger UX (armable conditions + the armed/resolved/stale surface); consume the
  bus `arm_trigger` primitive, the `myelin-flow` `stale_after` timer, and the one Notif inbox for `on_resolve`.
  Ship "Remind me when unblocked" as the flagship instance.

## Hands forward

- The reconnect/resync protocol details + the per-view subscription scope bounding — architecture (co-design
  with Chat connection tier / KN-1).
- The full armable-condition catalogue + the Trigger management surface — architecture + wireframes.
- PROVE-IT: reconnect-loses-zero-board-ops drill (rides KN-1's drill) + Trigger-fires-once-after-restart drill.
