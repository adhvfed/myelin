# EXT-4 — Notification "Why It Fired" + Storm-State UX Projection

> Extension flagged in [`README.md`](./README.md). A **narrow delta** — the server-side dedup/provenance
> is covered; the client-facing projection is the gap. (Lowest-risk extension; mostly a read-model shape.)

## The UX goal that requires it

The calm-by-default inbox (P8) is a lovability cornerstone: one prioritised "what needs *me*" view, with
clear "why am I getting this" provenance, storm-collapsed agent volume, and one read-state truth across
views (design-language §5.8). The completeness-critic (README §9) names the **storm / 30×-agent-surge**
inbox experience as a glossed state. The server already *computes* the provenance and the storm-collapse;
the delta is the **client-facing projection** that renders them as a lovable inbox.

## What the extension is (summary)

A **notification read-model projection** for the client: (1) the **"why it fired" affordance** rendered
from the existing `origin_event` + `reason` (NOTIF-2) — every item answers "why am I getting this" inline;
(2) the **collapsed-storm group projection** — the write-time `dedup_key` collapse rendered as an
expandable group ("47 agent updates on ISSUE-412 — expand"), so a surge reads as one calm item not 47;
(3) the **live unread / storm state** over the firehose so the inbox count and storm groups update live
without polling; (4) the **cross-device read-state echo** — one read-state truth (mark it read in chat,
it's read in the unified inbox) surfaced live across the user's sessions.

## Which architecture doc it touches (and what's already covered)

- **`05-refined/notifications.md`** — already provides: `dedup_key` + `UNIQUE(tenant, recipient,
  dedup_key)` **write-time storm collapse**; `origin_event` + `reason` = the **"why it fired" provenance**
  (NOTIF-2); `mark/snooze/mark_all_read` = **one read-state truth** across all views (contract-index 7.2);
  the five-mechanism storm-control + agent-mention-storm shed budget (C5). **Delta:** the *client-facing
  read-model projection* that renders the why-affordance, the collapsed-storm expandable group, and the
  live unread/storm state.
- **Firehose tier (`event-bus.md`)** — exists; the delta is using it for **live unread/read-state echo**
  to the client.

## Rough size / risk

**Size: S.** **Risk: L** — almost everything load-bearing (dedup, provenance, read-state truth,
storm-budget) is already designed and frozen; this is a read-model/projection shape + a live-echo
subscription on top of existing data. The main care is making the collapsed-storm group expansion
permission-aware and the live echo reconnect-safe.

## Implementation-task framing

"Add a client-facing notification read-model projection rendering the existing `origin_event`+`reason`
'why it fired', the `dedup_key` storm-collapse as an expandable group, and live unread + cross-device
read-state echo over the firehose — surfacing the already-designed dedup/provenance/read-state truth as a
calm, lovable inbox."
