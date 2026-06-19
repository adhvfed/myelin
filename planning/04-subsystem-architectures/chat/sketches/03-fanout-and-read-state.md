# Sketch 03 — Write-fanout vs read-fanout boundary, and the read-state store

> Exploration note. Resolves Phase-2 Chat §9.3 (the fanout boundary) and Phase-1 §5.2/§5.6. Aligns the
> Chat fanout split with the **platform's already-decided hybrid fanout model** (Notif §3.5).

---

## The problem, precisely

"Fan-out" has **two distinct meanings** in chat that the literature and the platform keep separate, and
conflating them is the classic mistake:

1. **Live delivery fan-out** (the connection tier, Sketch 01): getting a posted message to the *online*
   members' open sockets. This is **read-fanout per channel** — one ordered log per conversation, readers
   cursor in; the backplane just nudges online sockets to pull/receive the new tail.
2. **Attention fan-out** (the inbox/unread side): materialising "you were mentioned" / "you have unreads"
   so a person sees it whether or not they were online. This is where the **write-fanout-vs-read-fanout**
   trade-off bites.

The platform has *already decided* the attention split (Notif §3.5, grounded in the feed-systems
literature — Twitter's @-mention-vs-timeline split, Facebook TAO, Silberstein et al. "Feeding Frontier"
VLDB 2010): **write-fanout the bounded high-signal set; read-fanout the unbounded ambient set.** Chat's
job is to *produce the right signals into that model*, not to invent a second one.

---

## The boundary, resolved (aligned to Notif §3.5)

| Class | Strategy | What it is in chat | Producer |
|---|---|---|---|
| **DIRECT / high-signal (write-fanout)** | materialise a per-recipient `inbox_item` (Notif owns the store) | an `@mention(Principal)` of you, a DM addressed to you, a reply in *your* thread, an HITL approval awaiting you, a keyword-alert match | the structured `mention(Principal)` node (ADR-05) → a `chat.message.mention`/`chat.message.created` event → a Signal → Notif write-fanout (Notif §3.5) |
| **AMBIENT / low-signal (read-fanout)** | one ordered per-conversation log; per-watcher unread computed lazily on read | "channel #general has 40 new messages", "PR-#88 thread got 12 replies you watch", unread counts for channels you're a member of | the per-conversation message log (Sketch 02) + the read-state markers (below); watchers resolved at read via `list_subjects(channel, watch)` (Notif §3.5) |

The two load-bearing rules:

- **The mention is the canonical write-fanout producer**, and it is a **structured `mention(Principal)`
  node**, not parsed free text (ADR-05; Notif §3.5). This is *also* the agent-loop reference gate (AG-6:
  only a structured ref re-triggers, never raw typed text) — so the same node that drives write-fanout
  is the node that makes agent dispatch safe. One mechanism, two payoffs.
- **The unbounded ambient set never write-amplifies.** A 100k-member announcement channel does **zero**
  per-member inbox writes on a post; members see it via the read-fanout log + lazy unread on next open.
  This is the celebrity-fan-out mitigation (Notif §3.5; the feed literature) applied to chat's
  mega-channel case (Sketch 04 handles the *live-delivery* side of mega-channels).

**Where Chat owns the boundary** (not Notif): Chat decides, per event, *which class it is* — i.e. Chat's
event taxonomy (Sketch 10) declares which events carry a `mention(Principal)` node / map to a notify
`reason` (the §8 obligation Notif §8 hands every subsystem). Notif then routes. Chat does **not** own the
inbox store, the priority score, storm-control, or delivery — those are Notif's (C-9; Sketch 06).

---

## The read-state store (the high-write hot path)

Read-state is the churny part: **per-`(user × conversation)` last-read marker + per-thread read state +
unread/mention counts**, written on *every scroll/open* (Phase-1 §2.5/§5.6 — "a bad design here melts
the database"). It is deliberately **separate from the message store** (Sketch 02) and from Notif's inbox.

### Candidate designs

| Candidate | Shape | Verdict |
|---|---|---|
| **PG rows, one per (user, conversation)** | `UPDATE read_state SET last_read=… WHERE user=… AND conv=…` on every read | **Rejected at scale** — a write per scroll melts the OLTP tier (the named failure). Fine only at tiny scale. |
| **Fast KV (Redis/Valkey-class) + batched async flush to PG of record** | last-read markers + unread counters in Valkey (the cache/coordination store the platform already runs); **batched, debounced** writes; PG holds the durable system-of-record, reconstructable | **CHOSEN lean.** Matches Phase-1 §5.6 ("separate fast KV, batched eventually-consistent writes"). **But STOR-3 is law: Valkey is NEVER the source of truth** — so the durable last-read is periodically flushed to PG (or recomputed), and on a cache loss the marker is at-worst slightly stale (you re-see a few read messages — a benign, bounded failure). Unread *counts* are derived (= messages after last_read), so they're recomputable, never authoritative. |
| **CRDT counters per user** | conflict-free unread counters | Over-engineered for v1; last-read-marker + derive-count is simpler and sufficient. Noted, not chosen. |

### The decided posture

- **Valkey for the hot markers + counters; PG for the durable record; batched eventually-consistent
  flush.** Eventually-consistent is *acceptable* for read-state (Phase-1 §5.6) — "delivered/seen on one
  device → eventually seen everywhere" (C-9 read-state truth extended to devices, Notif §9 Q7).
- **Read-state is read-fanout, never write-fanout.** Unread count for channel C = `count(messages in C
  with id > my_last_read[C])` — a bounded range read against the per-conversation log (Sketch 02),
  cached. No per-message-per-member write.
- **Read-state events stay off the durable bus** (Phase-2 Chat §7.1; ADR-04.5): `chat.read_state.updated`
  rides the *firehose* (ephemeral); only a coarse summary (if any) touches the durable bus. The
  read-state store is a `PersonalDataHolder` (last-read markers are personal data) — auto-registered,
  crypto-shred/tombstone on erasure (Sketch 05).
- **Cross-device read-state truth** is the C-9 property (Notif §1.3): marking read in Chat's "Activity"
  view *is the same read-state* as the unified inbox, because the inbox item's read-state is Notif's one
  store — but the *per-channel scroll position* (which message you're looking at) is Chat's read-state.
  The two are linked: opening a channel and scrolling past a mentioned message marks the corresponding
  Notif inbox item read. Sketch 06 details the link.

---

## What this sketch hands forward

- **Attention fan-out = the platform hybrid (Notif §3.5):** write-fanout the bounded direct set (mentions,
  DMs, thread-replies-to-you, HITL-for-you, keyword matches) via structured `mention` nodes → Signals →
  Notif; read-fanout the unbounded ambient set (channel/thread activity, unread counts) via the
  per-conversation log + lazy unread.
- **Live delivery fan-out = read-fanout per channel** over the NATS backplane (Sketch 01); mega-channels
  escalate per Sketch 04.
- **Read-state store = Valkey hot markers + PG durable record + batched flush**, eventually-consistent,
  STOR-3-honouring (Valkey never authoritative), firehose-only events, a `PersonalDataHolder`.
- **Chat owns the per-event class declaration** (which events are direct/ambient/fyi — Sketch 10); Notif
  owns the routing/inbox/priority/delivery.
