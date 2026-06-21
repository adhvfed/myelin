# Extension Planning — Technical Extensions a Lovable Product Implies

> Phase: `extension-planning (repo root)`. Captures the concrete
> backend/technical extensions a **lovable** UX implies that the existing architecture does not already
> cover — stated clearly enough to become implementation tasks later. **Cross-checked against the existing
> `planning/` architecture** (Phase 2, 3, and especially the refined Phase-5 shared-systems docs) so we
> flag **genuine deltas, not re-statements**. Where a candidate is already designed, it is listed as
> **"covered by <doc>"** rather than as an extension. Status date: **2026-06-20**.

## The cross-check verdict (read first)

The Myelin architecture is **unusually complete** on the substrate a lovable product needs. The
candidate list the steer named is mostly **already designed**:

| Candidate (from the steer) | Status | Where |
|---|---|---|
| Real-time presence / typing | **COVERED** | Chat firehose path (presence/typing/read-state via ephemeral NATS, never durable bus) — `02-holistic/subsystems/chat.md` §8; `05-refined/event-bus.md` (firehose tier); recon §OQ "firehose presence/typing/read-state subject grammar + TTLs → CONFIRM". |
| Notification dedup / storm-control / "why did this fire" | **COVERED** | `05-refined/notifications.md` — `dedup_key` + `UNIQUE` write-time collapse (C5 storm budgets, agent-mention-storm lane), `origin_event` + `reason` = the "why it fired" provenance (NOTIF-2). |
| Dashboard / analytics aggregations | **COVERED (substrate)** | OLAP read store accepting the subsystem event stream, honouring the restriction flag — `05-refined/00-reconciliation-decisions.md` §8 + §11; `system-overview.md` (OLAP read store). *UX-layer delta flagged below: EXT-2.* |
| Unfurl live-projection / caching | **COVERED** | `05-refined/reference-graph.md` — per-viewer projection + permission check + the bounded invalidatable projection cache (§3.6), the 4-step tombstone/degradation ladder (C-2), content-anchored git line-ranges. |
| Optimistic concurrency / conflict surfacing | **COVERED (engine)** | CAS-floor → CRDT path for collaborative edit (KN-1); per-aggregate ordering on the bus. *UX-surfacing delta flagged below: EXT-3.* |
| Prefetch / perceived-performance support | **PARTIAL → EXT-1** | The bus has *consumer-flow* prefetch (back-pressure), but **client-facing prefetch / context-projection bundling** for the "system assembles + pre-fetches context" UX (§8b.6) is a genuine delta. |
| Event bus, reference graph, shared notifications, agent fabric, audit/DSR | **COVERED** | Phase 3 + Phase 5 shared-systems docs (these are the platform's spine; do not re-state). |

So the genuine extensions are **narrow and mostly at the UX/projection seam**, not the core substrate.
Four are flagged below; each is a *delta* on an existing system, with the doc it touches and a rough
size/risk. The rest is explicitly "covered" so later phases don't re-derive it.

## The flagged extensions

| ID | Extension | UX goal it serves | Touches | Size | Risk | File |
|---|---|---|---|---|---|---|
| **EXT-1** | Client-facing context-projection prefetch / bundling | "The system assembles context; the user never does" — failing check → step → line, the PR context-pane, the next-hop in a notification, **pre-fetched** not just linked (P2/§8b.6). | `reference-graph.md` (projection API), `event-bus.md` (firehose), subsystem projection APIs | M | M | [`perceived-performance.md`](./perceived-performance.md) |
| **EXT-2** | Dashboard/analytics **read-model + query** UX layer over OLAP | The PM/EM/exec reporting surfaces (burndown, cycle-time, CI health, SLA gauges, portfolio rollup) need a *queryable, permission-aware, real-time-ish* read model + a saved-dashboard config object — the OLAP *store* exists; the UX-facing query/config layer is the delta. | OLAP read store (storage §8), `identity-and-access.md` (ACL on aggregates), views component | M | M | [`dashboard-analytics.md`](./dashboard-analytics.md) |
| **EXT-3** | Conflict-surfacing **UX contract** for optimistic/concurrent writes | The honest-rollback + concurrent-edit-conflict states (issue fields, doc blocks) must surface legibly — the *engine* (CAS→CRDT) exists; the **version-token + conflict-payload contract the client renders** is the delta. | subsystem write APIs (issue/doc), `00-platform-substrate.md` (CAS), editor/views components | S–M | M | [`optimistic-concurrency.md`](./optimistic-concurrency.md) |
| **EXT-4** | Notification "why it fired" + storm-state **UX projection** | The provenance (`origin_event`+`reason`) and storm-collapse exist server-side; the delta is the **client-facing projection** that renders "why am I getting this", the collapsed-storm group ("47 agent updates, expand"), and the live unread/storm state — plus the cross-device read-state echo. | `notifications.md` (read API + projection), firehose (live unread) | S | L | [`notification-semantics.md`](./notification-semantics.md) |

**Net:** four genuine deltas, all at the UX/projection seam; **no new core substrate is required** — the
event bus, reference graph, OLAP store, CAS/CRDT engine, notification dedup/provenance, presence firehose,
and agent fabric already exist. Each extension is a thin, decision-ready addition on top of a system that
is already designed.

## How to read each file

Each topic file states, per extension: **the UX goal that requires it**; **what the extension is**
(summary level, not a full design); **which existing architecture doc it touches** (and what it already
covers, so the delta is precise); **rough size/risk**; and a one-line **"implementation task" framing**
so a later phase can pick it up. None of these is code or a finished design — they are decision-ready
task stubs.
