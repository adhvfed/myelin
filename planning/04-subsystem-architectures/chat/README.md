# Chat Subsystem — Phase-4 Design & Architecture (index)

> Phase: `04-subsystem-architectures/chat`. Canonical brief: [`VISION.md`](../../../VISION.md) (never
> contradicted). Chat is subsystem #5 of five — Myelin's **real-time conversation surface** and the **most
> visible surface of the agent-native principle**: live permission-aware unfurls, the HITL approval-card
> bridge, agent presence/streaming, and the one inbox — built almost entirely on the Phase-3 shared chokepoints.
> Date: 2026-06-19.

---

## The two-stage shape

Phase 4 ran in two stages (VISION §3/§5.4: sketch before committing an architecture).

- **Stage 1 — design & sketch** (the explorations + the UX design): [`sketches/`](./sketches/) +
  [`design/`](./design/).
- **Stage 2 — detailed architecture** (the binding write-up): [`architecture/`](./architecture/).

---

## Stage 2 — Architecture (the binding docs) — read in order

| # | Doc | What it owns |
|---|---|---|
| 00 | [`architecture/00-overview.md`](./architecture/00-overview.md) | Role & responsibilities; owns-vs-delegates; the floors named up front; the component map; the build-order law (R1: resume-cursor + outbox-coherence first). |
| 01 | [`architecture/01-tech-and-data-model.md`](./architecture/01-tech-and-data-model.md) | The language/runtime/DB choice + written justification (**the TE-21 connection-tier call: Rust default, BEAM hatch**); the full data model (`Conversation`+kind, `message` log + `MessageStore` trait + tiering, read-state, unfurl cache, membership, ReBAC fragment). |
| 02 | [`architecture/02-internals-and-algorithms.md`](./architecture/02-internals-and-algorithms.md) | The hard-problem algorithms: the connection tier + NATS backplane + **resume-cursor resync** (zero-loss); message-store tiering; the read-state hot path; **cheap per-viewer unfurls**; the **HITL bridge** + Activity-as-view; the erasure cascade; agent presence/streaming/explicit-first dispatch. |
| 03 | [`architecture/03-events-contracts-and-glue.md`](./architecture/03-events-contracts-and-glue.md) | The complete `chat.*` taxonomy (durable vs firehose); every glue contract (`ArtifactRef`/`project`/`replay`, the envelope via the outbox, Id `check`/`list_objects` + the ReBAC fragment, `PersonalDataHolder`, `ToolDef`s, `declare_indexable`, reserve/settle). |
| 04 | [`architecture/04-views-cli-and-api.md`](./architecture/04-views-cli-and-api.md) | The 13 views (S1–S13, ref `design/`); the CLI surface; the API / agent-tool surface. |
| 05 | [`architecture/05-hard-problems.md`](./architecture/05-hard-problems.md) | Each subsystem-specific hard problem resolved, with **cited prior art** + the named floor. |
| 06 | [`architecture/06-shared-system-change-requests.md`](./architecture/06-shared-system-change-requests.md) | The itemized shared-system changes Chat needs for Phase-5 reconciliation (CHG-C1…C12). |
| 07 | [`architecture/07-drills-and-open-questions.md`](./architecture/07-drills-and-open-questions.md) | The quantified drills owed (D-C1…D-C18) + the open questions for Phase 5 (Q-C1…Q-C10). |

## Stage 1 — Design (the UX, produced before the architecture)

| Doc | What it covers |
|---|---|
| [`design/information-architecture.md`](./design/information-architecture.md) | Where Chat sits in the one shell; the conversation-list secondary nav; the 13 primary screens; the SUB-X responsive cases. |
| [`design/user-flows.md`](./design/user-flows.md) | Send/edit (optimistic + honest rollback); the unfurl wedge; the full agent HITL bridge; streaming; the erasure cascade; cross-subsystem flows. |
| [`design/wireframes.md`](./design/wireframes.md) | ASCII wireframes of S2–S13 with empty/loading/error/permission-denied/erased/agent-pending states; the day-one UX primitives applied. |

## Stage 1 — Sketches (the explorations)

[`sketches/00-findings.md`](./sketches/00-findings.md) is the findings summary; `01`–`10` are the per-problem
explorations (connection tier, message store, fanout/read-state, unfurls, erasure, HITL+inbox, agent
presence/streaming, threads/canvas/federation, the wire-contract harness, the taxonomy/glue checklist).

---

## The one-paragraph thesis

Chat is a careful **consumer** of the Phase-3 shared layer (Refs `resolve` for unfurls, Notif `list_inbox` for
the inbox, Workflow `signal` for the HITL bridge, the crypto-shred triad for erasure, the platform hybrid for
fanout) plus **four genuinely-owned hot parts**: the **connection tier** (Rust gateway, NATS-core backplane,
resume-cursor resync as the correctness backbone), the **message store** (Postgres-partitioned hot + object cold,
behind a `MessageStore` trait, Scylla the measured floor), the **read-state hot path** (Valkey + PG,
eventually-consistent, never authoritative in cache), and **cheap per-viewer unfurls** (lazy-on-viewport + a
shared-per-ref projection cache gated by a per-viewer `check`). It **is** the HITL approval-card surface
(`Id.check(approve)` → `DurableExecutor::signal(idem_key=card_id)`), its "Activity/Mentions" is a **view** into
the one Notif inbox (C-9, never a second store), and agent dispatch is **explicit-first** (CHAT-1 — a mention
notifies, it does not auto-spawn a costed run). It invents no auth, reads no other store, and is fully rebuildable
from its own source via `replay`.

## The decisions at a glance

- **Language/runtime/DB:** Rust everywhere (incl. the gateway — the TE-21 call, BEAM hatch written-but-closed);
  Postgres message store behind a `MessageStore` trait (Scylla floor); object-store cold segments; Valkey for
  read-state/presence/unfurl-cache; NATS core for live delivery + presence. EU-deployable / self-hostable
  throughout.
- **Floors named:** Scylla hot tier (R-5); free-text third-party erasure (→ LEGAL); mega-channel home-node
  delivery (R-5); single home-cell; canvas-as-embed; cross-org channels (designed-not-built); the BEAM hatch.
- **Drills owed:** zero-loss-across-reconnect; 30×-agent-surge human-lane-holds; unfurl no-leak + erasure-safe;
  erasure-reaches-every-holder; HITL exactly-once; Search ACL filter — and 12 more (D-C1…D-C18).
