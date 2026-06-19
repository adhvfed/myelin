# Chat — 07 · Drills Owed & Open Questions

> See [`00-overview.md`](./00-overview.md) for framing. This doc lists the **quantified drills** Chat owes
> (PROVE-IT: each failable property names its drill + gate) and the **open questions** handed to Phase 6. The
> drills assert against the substrate **telemetry survival signal set** (contract 1.8) — an uncommitted gate is no
> gate. Conformed to the frozen contracts (the firehose `resume` protocol, the per-effect `idem_key`, the frozen
> `SetExpr` filter, the 4-step ladder).

---

## 1. The drills owed (quantified)

| # | Property (the thing that can fail) | Quantified drill (the gate) | Source |
|---|---|---|---|
| **D-C1** | **Zero messages lost across a reconnect** (the connection-tier correctness backbone; the OQ-J pass condition) | Sever the gateway↔firehose mid-publish; assert `resume(stream, scope, last_seq)` from the retention window recovers the gap → **0 lost, 0 duplicate**; when `last_seq` exceeds the window, **`resync_required` → `*.snapshot` (`MessageStore::resync_from`)** recovers it, still 0 lost. | [02 §1.3](./02-internals-and-algorithms.md) |
| **D-C2** | **Per-conversation total order at scale** (contract 2.3, the D-9 drill) | Burst sends + edits to one hot channel from many gateways → **per-conversation total order preserved (ULID `message_id` / `aggregate=conversation_id`); resume gap-free**; out-of-order client ops reconcile to the durable sequence. | [02 §2.2](./02-internals-and-algorithms.md) |
| **D-C3** | **30× agent-surge: the human lane holds** (the connection-storm + agent-mention-storm shed profile, OQ-K) | 30× agent message/connection surge on one tenant → **human connection/read latency stays in budget; the agent lane sheds (429 + `Retry-After` honoured); other tenants unaffected** (per-tenant fairness). | [02 §1.4](./02-internals-and-algorithms.md) |
| **D-C4** | **Deploy reconnect thundering-herd** | Roll the gateway fleet under a connection storm → **bounded reconnect rate; `resume` completes for all; no message loss; readiness gates new connections, liveness does not restart-storm** (contract 1.3). | [02 §1.4](./02-internals-and-algorithms.md) |
| **D-C5** | **Unfurl no-leak** (ADR-03; the 4-step ladder step 1) | Notify/unfurl a confidential artifact to a viewer lacking access → **tombstone rendered, title never present** in the response. | [02 §4.2](./02-internals-and-algorithms.md) |
| **D-C6** | **Unfurl erasure-safe** | Erase a third party rendered in a card → **tombstone on next render, 0 recoverable PII** (no durable snapshot exists; the cache re-resolves live → `erased`). | [02 §4/§6](./02-internals-and-algorithms.md) |
| **D-C7** | **Unfurl live-update** | An artifact's `ci.check.updated`/`*.updated` event → **the shared per-ref cache busts; viewers currently showing the card get a live firehose update** within budget. | [02 §4.4](./02-internals-and-algorithms.md) |
| **D-C8** | **Erasure reaches every Chat holder** (T-5 family) | Erase a person → assert bodies crypto-shred in **hot + cold segments + backups**; mentions render `[erased user]` (pseudonym-map shred); read-state/drafts/unfurl-cache purged; Search (incl. embeddings)/Refs/Notif cascade → **0 recoverable PII** across every Chat-owned + derived store. | [02 §6](./02-internals-and-algorithms.md) |
| **D-C9** | **HITL approve→resume bridge, exactly-once** | Request an approval, kill Chat + Workflow mid-wait, approve days later → **the gated tool runs exactly once; a double-click is one approval (`idem_key=card_id`); deny withholds with no mutation; timeout auto-denies**; resume runs under a freshly-minted token (contract 4.7). | [02 §5](./02-internals-and-algorithms.md) |
| **D-C10** | **Batch/partial approval well-defined** (the frozen per-effect `idem_key`, OQ-F) | A multi-effect card approved 2-of-3 → **the 2 gates resume approved, the 1 withheld, each independent (`idem_key=card_id:<idx>`)**; no effect runs twice; the withheld effect never mutates. | [02 §5.2](./02-internals-and-algorithms.md) |
| **D-C11** | **Search ACL filter** (the `search-requires-acl-filter` lint, contract 6.1) | Search as a non-member → **0 results from channels you're not in**; the lint fails any query path that reaches the index without the frozen `list_objects` `Filter` conjoined over `message.id`. | [03 §7](./03-events-contracts-and-glue.md) |
| **D-C12** | **Read-state cache-loss is benign** | Flush + drop Valkey mid-session → **the PG record is authoritative; a marker is at-worst slightly stale (re-see a few read messages); unread counts recompute correctly**. | [02 §3](./02-internals-and-algorithms.md) |
| **D-C13** | **Outbox co-commit (no dual-write)** (BUS-2 / contract 2.2) | Crash between message persist and event emit → **either both committed or neither**; the message and its `chat.message.created` are atomic; no orphan message, no phantom event. | [01 §3.1](./01-tech-and-data-model.md) |
| **D-C14** | **Idempotent send** | Retry a send (flaky mobile/agent) with the same `client_nonce` → **one message** (`UNIQUE(conv, client_nonce)`). | [01 §3](./01-tech-and-data-model.md) |
| **D-C15** | **Reindex-from-source rebuilds Chat-derived state** (contract 2.6) | Wipe + `replay(scope, since)` → Search/Refs/Notif read-models rebuild from `chat.*.snapshot`; **steady-state and recovery share one path; erased subjects emit tombstones** (no PII resurrected). | [03 §6](./03-events-contracts-and-glue.md) |
| **D-C16** | **Agent presence/streaming is mock-provable** (D6) | Drive the streaming UX against the **mock runtime** (`--use-mock`, contract 8.3) → partials stream on the firehose; final replaces partial; a mid-stream reconnect `resume`s the final, **never a half-message**. | [02 §7.3](./02-internals-and-algorithms.md) |
| **D-C17** | **Explicit-first dispatch holds** (CHAT-1; contract 8.6) | A casual `@agent` mention → **notifies the agent's inbox, does NOT spawn a costed run**; only an explicit action / structured trigger dispatches; reserve/settle gates even the explicit run (no balance → no run). | [02 §7.1](./02-internals-and-algorithms.md) |
| **D-C18** | **`#sub` stability + tombstone ladder** (contract 5.7, X-4) | Edit a message referenced by another artifact → **the `message-<id>` anchor stays stable (live)**; delete it → **the embed degrades to a Tombstone carrying the root** (channel), never dangles. | [03 §2](./03-events-contracts-and-glue.md) |
| **D-C19** | **Frontend switch test** (T-7/T-8) | Drive the real Chat UI in a browser → **a team could move to it without hitting a wall the old tool didn't have**; measured-contrast tokens; latency budgets (optimistic send < ~100ms perceived); flip-popovers tested against the real bottom-pinned composer anchor. | [`../design/wireframes.md`](../design/wireframes.md) |

**The TE-21 build-gate.** D-C3 + D-C4 (presence-at-scale + scheduler tail-latency under storm) are the drills
whose *failure in Rust* would trigger the BEAM escape hatch (bounded by contract 1.7, [05 §1](./05-hard-problems.md)).
They run early against the Rust gateway; if Rust holds, the divergence stays closed.

---

## 2. Open questions for Phase 6

| # | Question | Toward whom | Note |
|---|---|---|---|
| **Q-C1** | The final TE-21 commit confirmation — run D-C3/D-C4 against the Rust gateway early and *confirm* the hatch stays closed (or open it with the frozen 1.7 shim + its drills). | Chat + substrate | The decision is made; the gate is the early presence-at-scale drill. |
| **Q-C2** | The firehose retention-window size for `fan.<tenant>.<channel>` (the measured tunable behind `resume`/`resync_required`) + the channel-sharded-home-node trigger threshold (R-5). | Bus (firehose seam) + Chat | R-C1; measured-not-predicted. |
| **Q-C3** | The read-state batched-flush cadence + the precise `Notif.mark(item, read)` trigger when scrolling past a mention. | Chat + Notif | R-C3; eventually-consistent accepted, the cadence is the tunable. |
| **Q-C4** | The unfurl projection-cache TTL + the membership-class precompute refresh cadence on `member_*` events. | Chat + Refs | R-C4. |
| **Q-C5** | The per-surface shed budget numbers (connection-storm + agent-mention-storm caps + the protected-human-lane reservation), tuned by D-C3/D-C4. | Chat + substrate | R-C2; OQ-K names the floor. |
| **Q-C6** | The free-text-erasure residual lawful basis — ratified **once** as the single platform posture (contract 10.9, `[OPEN — LEGAL]`). | GDPR/DPO/LEGAL | R-C5; Chat carries no separate statement. |
| **Q-C7** | The `MessageStore` Scylla-promotion support in Storage (residency-pinned + crypto-shred per cell) when measured volume triggers it. | Storage | R-C6; measured follow-on. |
| **Q-C8** | Group-DM vs private-channel UX divergence — confirm the `kind` set + the affordance differences over the unified machinery. | Chat (product) | model decided, UX divergence is the open detail. |
| **Q-C9** | The canvas pin/embed mechanism — the joint Chat↔Knowledge review of `conversation.pinned_canvas`. | Chat + Knowledge | R-C7; the lean (embed, not editor) is firm. |
| **Q-C10** | The comment-threading consolidation trigger (OQ-L) — when document-anchored comments need real-time presence, promote onto the Chat threading primitive + the firehose transport. | Chat + Knowledge/Issues | R-C8; named follow-on (gap report E-3). |
| **Q-C11** | Cross-org/federated channels — the cross-tenant capability + multi-cell DSR + residency policy when the frozen cross-cell bridge ships. | control plane + LEGAL | R-C9; designed-not-built. |
| **Q-C12** | Implicit agent auto-dispatch (L-3) — wiring any auto-spawn path waits on counsel ratifying the human-oversight basis. | Chat + Agent Fabric + LEGAL | R-C10; explicit-first is v1. |

---

## 3. The honest one-line state

**Chat's architecture (rewritten against the reconciled layer) commits Rust end-to-end (the TE-21 call, with the
BEAM hatch written-but-closed and now bounded by the frozen cross-language harness shim), a Postgres-partitioned
message store behind a `MessageStore` trait (Scylla the measured floor), a Valkey+PG read-state hot path, live
delivery over the FROZEN firehose resume-cursor protocol (`subscribe/resume/scope`) with the resume-cursor as the
correctness backbone, and cheap per-viewer unfurls that call the Refs chokepoint over the frozen 4-step tombstone
ladder and lower the frozen `SetExpr` to a JOIN — and it earns each by naming the drill that proves it. The two
named platform obligations (the HITL approval-card bridge with the frozen per-effect `idem_key`; the
Activity-inbox-as-view) reuse `DurableExecutor::signal` and `Notif.list_inbox` rather than re-building them. The
honest floors are named: the free-text third-party residual handled BY REFERENCE to the ONE platform posture
(10.9), mega-channel home-node delivery, comment-threading consolidation (OQ-L), cross-org channels, and the
BEAM hatch — each with a trigger and an owner.**
