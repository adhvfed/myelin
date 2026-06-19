# Chat — 07 · Drills Owed & Open Questions

> See [`00-overview.md`](./00-overview.md) for framing. This doc lists the **quantified drills** Chat owes
> (PROVE-IT: each failable property names its drill + gate; Phase-5 executes, Chat names) and the **open
> questions** handed to Phase 5. The drills assert against the substrate **telemetry survival signal set**
> (substrate §10.2; X-1) — an uncommitted gate is no gate (E-4).

---

## 1. The drills owed (quantified)

| # | Property (the thing that can fail) | Quantified drill (the gate) | Source |
|---|---|---|---|
| **D-C1** | **Zero messages lost across a reconnect** (the connection-tier correctness backbone) | Sever the gateway↔NATS backplane mid-publish; assert resync from the durable log recovers the gap → **0 lost, 0 duplicate** (idempotency on `message_id` + `client_nonce`). | [02 §1.3](./02-internals-and-algorithms.md) |
| **D-C2** | **Per-conversation total order at scale** | Burst sends + edits to one hot channel from many gateways → **per-conversation total order preserved (ULID); resync gap-free**; out-of-order client ops reconcile to the durable sequence. | [02 §2.2](./02-internals-and-algorithms.md) |
| **D-C3** | **30× agent-surge: the human lane holds** (the connection-storm shed profile, ADR-16) | 30× agent message/connection surge on one tenant → **human connection/read latency stays in budget; the agent lane sheds (429 + `Retry-After` honoured); other tenants unaffected** (per-tenant fairness). | [02 §1.4](./02-internals-and-algorithms.md) |
| **D-C4** | **Deploy reconnect thundering-herd** | Roll the gateway fleet under a connection storm → **bounded reconnect rate; resync completes for all; no message loss; readiness gates new connections, liveness does not restart-storm**. | [02 §1.4](./02-internals-and-algorithms.md) |
| **D-C5** | **Unfurl no-leak** (ADR-03) | Notify/unfurl a confidential artifact to a viewer lacking access → **tombstone rendered, title never present** in the response (inherits Refs/Notif D-N4). | [02 §4.2](./02-internals-and-algorithms.md) |
| **D-C6** | **Unfurl erasure-safe** | Erase a third party rendered in a card → **tombstone on next render, 0 recoverable PII** (no durable snapshot exists; the cache re-resolves live). | [02 §4/§6](./02-internals-and-algorithms.md) |
| **D-C7** | **Unfurl live-update** | An artifact's `*.checks_completed`/`*.updated` event → **the shared per-ref cache busts; viewers currently showing the card get a live update** within budget. | [02 §4.4](./02-internals-and-algorithms.md) |
| **D-C8** | **Erasure reaches every Chat holder** (T-5 family) | Erase a person → assert bodies crypto-shred in **hot + cold segments + backups**; mentions render `[erased user]`; read-state/drafts/unfurl-cache purged; Search/Refs/Notif cascade → **0 recoverable PII** across every Chat-owned + derived store. | [02 §6](./02-internals-and-algorithms.md) |
| **D-C9** | **HITL approve→resume bridge, exactly-once** (Workflow D-N7 analogue) | Request an approval, kill Chat + Workflow mid-wait, approve days later → **the gated tool runs exactly once; a double-click is one approval (`idem_key=card_id`); deny withholds with no mutation; timeout auto-denies**. | [02 §5](./02-internals-and-algorithms.md) |
| **D-C10** | **Batch/partial approval well-defined** | A multi-effect card approved 2-of-3 → **the 2 gates resume approved, the 1 denied, each independent (`idem_key=card_id:<idx>`)**; no effect runs twice. | [02 §5.2](./02-internals-and-algorithms.md) |
| **D-C11** | **Search ACL filter** (the `search-requires-acl-filter` lint, S-3) | Search as a non-member → **0 results from channels you're not in**; the lint fails any query path that reaches the index without a composed `list_objects`. | [03 §7](./03-events-contracts-and-glue.md) |
| **D-C12** | **Read-state cache-loss is benign** (STOR-3) | Flush + drop Valkey mid-session → **the PG record is authoritative; a marker is at-worst slightly stale (re-see a few read messages); unread counts recompute correctly**. | [02 §3](./02-internals-and-algorithms.md) |
| **D-C13** | **Outbox co-commit (no dual-write)** (BUS-2) | Crash between message persist and event emit → **either both committed or neither**; the message and its `chat.message.created` are atomic; no orphan message, no phantom event. | [01 §3.1](./01-tech-and-data-model.md) |
| **D-C14** | **Idempotent send** | Retry a send (flaky mobile/agent) with the same `client_nonce` → **one message** (`UNIQUE(conv, client_nonce)`). | [01 §3](./01-tech-and-data-model.md) |
| **D-C15** | **Reindex-from-source rebuilds Chat-derived state** | Wipe + `replay(scope, since)` → Search/Refs/Notif read-models rebuild from `chat.*.snapshot`; **steady-state and recovery share one path; erased subjects emit tombstones** (no PII resurrected). | [03 §6](./03-events-contracts-and-glue.md) |
| **D-C16** | **Agent presence/streaming is mock-provable** (D6) | Drive the streaming UX against the **mock runtime** (`--use-mock`) → partials stream on the firehose; final replaces partial; a mid-stream reconnect re-fetches the final, **never a half-message**. | [02 §7.3](./02-internals-and-algorithms.md) |
| **D-C17** | **Explicit-first dispatch holds** (CHAT-1; AG-6) | A casual `@agent` mention → **notifies the agent's inbox, does NOT spawn a costed run**; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit run (no balance → no run). | [02 §7.1](./02-internals-and-algorithms.md) |
| **D-C18** | **Frontend switch test** (T-7/T-8) | Drive the real Chat UI in a browser → **a team could move to it without hitting a wall the old tool didn't have**; measured-contrast tokens; latency budgets (optimistic send < ~100ms perceived); flip-popovers tested against the real bottom-pinned composer anchor. | [`../design/wireframes.md`](../design/wireframes.md) |

**The TE-21 build-gate.** D-C3 + D-C4 (presence-at-scale + scheduler tail-latency under storm) are the drills
whose *failure in Rust* would trigger the BEAM escape hatch ([05 §1](./05-hard-problems.md)). They are run early
against the Rust gateway; if Rust holds, the divergence stays closed.

---

## 2. Open questions for Phase 5

| # | Question | Toward whom | Note |
|---|---|---|---|
| **Q-C1** | **The final TE-21 commit confirmation.** Architecture commits **Rust**; Phase 5 must run D-C3/D-C4 against the Rust gateway early and *confirm* the hatch stays closed (or open it with the Sketch-09 shim + its drills). | Chat + substrate | The decision is made; the gate is the early presence-at-scale drill. |
| **Q-C2** | **The exact NATS-core subject grammar + TTLs** for `fan.<tenant>.<channel>`, presence, typing, and partial streams; and the channel-sharded-home-node trigger threshold (R-5). | Bus (firehose seam) + Chat | CHG-C3; the home-node promotion is measured, not pre-built. |
| **Q-C3** | **The read-state batched-flush cadence + the Notif inbox-read link** — the exact debounce window, the cross-device truth reconciliation, and the precise `Notif.mark(item, read)` trigger when scrolling past a mention. | Chat + Notif | [02 §3/§5.3](./02-internals-and-algorithms.md); eventually-consistent is acceptable, the cadence is the tunable. |
| **Q-C4** | **The unfurl projection-cache TTL + bus-invalidation key scheme** — the exact TTL backstop, the per-`ArtifactRef` cache key, and the membership-as-permission class precompute refresh on `member_*` events. | Chat + Refs | [02 §4](./02-internals-and-algorithms.md); CHG-C2. |
| **Q-C5** | **The batch/multi-effect approval-card semantics** — confirm the per-effect `idem_key` scheme + the card-anchoring rule (`correlation_id`) jointly with Workflow. | Chat + Workflow | CHG-C4; the lean is decided ([02 §5.2](./02-internals-and-algorithms.md)), the joint confirmation is owed. |
| **Q-C6** | **The free-text-erasure floor's documented lawful-basis limit** — the exact residual statement, co-owned with LEGAL/DPO (GD-1 family). | GDPR/Audit + LEGAL | CHG-C8; the chat residual analogue of git-history. |
| **Q-C7** | **Per-surface shed budgets** — the concrete connection-storm + agent-mention-storm caps + the protected-human-lane reservation size for the gateway. | substrate + Chat | CHG-C9; the D-C3 drill asserts against them. |
| **Q-C8** | **Group-DM vs private-channel UX divergence** — confirm the `kind` set + the affordance differences over the unified machinery. | Chat (product) | [05 §6](./05-hard-problems.md); model is decided, the UX divergence is the open detail. |
| **Q-C9** | **The canvas pin/embed mechanism** — the joint Chat↔Knowledge review of `conversation.pinned_canvas` (the embed render path, the permission inheritance, the erasure ownership). | Chat + Knowledge | [05 §6](./05-hard-problems.md); the lean (embed, not editor) is firm. |
| **Q-C10** | **Cross-org/federated channels** — the cross-tenant capability + multi-cell DSR + residency policy when the cross-cell bridge ships. | control plane + LEGAL | CHG-C10; designed-not-built, model doesn't foreclose ([05 §7](./05-hard-problems.md)). |

---

## 3. The honest one-line state

**Chat's architecture commits Rust end-to-end (the TE-21 call, with the BEAM hatch written-but-closed), a
Postgres-partitioned message store behind a `MessageStore` trait (Scylla the measured floor), a Valkey+PG
read-state hot path, NATS-core live delivery with resume-cursor resync as the correctness backbone, and cheap
per-viewer unfurls that call the Refs chokepoint — and it earns each of those by naming the drill that proves it.
The two named platform obligations (the HITL approval-card bridge; the Activity-inbox-as-view) reuse
`DurableExecutor::signal` and `Notif.list_inbox` rather than re-building them. The honest floors are named:
free-text third-party erasure, mega-channel home-node delivery, cross-org channels, and the BEAM hatch — each with
a trigger and an owner.**
