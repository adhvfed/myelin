# Chat — Phase-4 Stage-1 Findings (what I learned, what I commit, what I hand forward)

> Phase: `04-subsystem-architectures/chat`, **Stage 1 of 2 — design & sketch** (VISION §3/§5.4: sketch
> before committing an architecture). This doc summarises the exploration in
> [`01`…`10`](.) and the design sketches in [`../design/`](../design/), states the **committed direction on
> each hard problem**, and hands the **open questions** to my own Stage-2 architecture. Dated 2026-06-19.
>
> **Read order for the architecture stage:** this findings doc → the per-problem sketches it cites → the
> three design docs. The committed directions below are *defaults-to-beat* for Stage 2 (beat one only in
> writing, per VISION §3 / the doctrine specificity contract).

---

## 1. What I learned (the shape of the problem)

1. **Most of Chat's "hard problems" are already solved by the Phase-3 shared layer — Chat's job is to
   *consume the chokepoints correctly*, not re-invent them.** Per-viewer permission-aware unfurls = call
   `refs.resolve` (Refs §4.2 is the non-leaking chokepoint). The inbox = a scoped *view* into Notif (C-9).
   The HITL bridge = `DurableExecutor::signal` (Workflow §6.3). Erasure = the platform crypto-shred +
   references-not-payloads triad (Storage §5.1, Bus §4.8). The fan-out hybrid = Notif §3.5. **The repeated
   discovery: where I reached for a Chat-specific mechanism, the platform already had one, and using it is
   both less work and more correct.** Chat's genuinely-owned hard parts narrow to: the **connection tier**,
   the **message store + tiering**, the **read-state hot path**, and **making per-viewer unfurl resolution
   cheap** at chat density.

2. **The connection-tier divergence (TE-21) is real but disfavoured once the shim cost is costed.** BEAM/
   Phoenix gives PubSub + Presence for free; but the substrate non-negotiables (the protected-human-lane
   shed order, resilient-client + `Retry-After`, the telemetry survival signals, liveness≠readiness) are a
   **substantive owed re-implementation in Elixir** (Sketch 09), and the platform already runs **NATS in
   every cell** (a sovereign, self-hostable pub/sub + presence transport) — so Rust gets the win without
   the second runtime. The Discord precedent (Rust for hot data, Elixir for the gateway) is the one signal
   that keeps the divergence honest-and-open, not foreclosed.

3. **Several "decisions" were already made *against* the flashy default, and that is correct.** Explicit-
   first agent dispatch (CHAT-1 — a mention notifies, it does not auto-spawn a costed run). The one inbox
   (C-9 — no Chat mentions store). Live unfurls, never snapshots (erasure-safe by construction). These are
   the platform protecting Chat from its own most-expensive mistakes; the design honours them.

4. **Chat is the stress-test for the holder spine, and bodies are NOT references-not-payloads** — a body
   *is* the PII. So Chat leans hard on **per-subject-DEK crypto-shred** (GD-4), with a **named floor**:
   free-text third-party names in others' un-erased bodies are not surgically erasable (the chat analogue
   of GD-1's residual). Naming this floor honestly is load-bearing.

---

## 2. Committed direction on each hard problem (the prompt's list)

| # | Hard problem | Committed direction (default-to-beat for Stage 2) | Sketch |
|---|---|---|---|
| 1 | **Connection-tier transport + language (TE-21; BEAM candidate)** | **Rust gateway by default** (no GC pauses at millions of connections; substrate harness + glue crates native). **BEAM/Phoenix kept as the written, open-but-disfavoured divergence**, gated on presence-at-scale/scheduler-tail-latency proving tractable in Rust during build; the divergence cost is the substrate shim (Sketch 09), which is enough work that it doesn't clearly clear the PubSub/Presence win. **Either way the gateway speaks the Rust `EventEnvelope` on the wire + implements `PersonalDataHolder`.** | 01, 09 |
| — | **Fan-out backplane** | **NATS-core subject-per-channel**, reusing the cell's existing NATS (Bus §4.3 firehose seam); **channel-sharded home-node** is the measured escalation for mega-channels. Correctness backbone = **resume-cursor resync from the durable log** (the ephemeral backplane may drop; the cursor recovers — zero-loss-across-reconnect drill). | 01, 04 |
| 2 | **Message-store substrate + tiering (wide-column candidate)** | **Postgres-partitioned hot tier (`(tenant,region)` + time sub-partitions) + object-store cold tiering as item-zero**, behind a `MessageStore` trait. Chosen for **outbox transaction coherence** (message + event in one tx — no dual-write) and because **GDPR (per-subject crypto-shred) is native on PG, bespoke on wide-column**. **Named floor:** the follow-on is **ScyllaDB hot tier on measured volume** (R-5 trigger). The cell bounds the scale; measure before sharding (ADR-10). | 02 |
| 3 | **Write-vs-read fanout boundary** | **The platform hybrid (Notif §3.5):** write-fanout the bounded high-signal set (mentions via structured `mention` nodes, DMs, thread-replies-to-you, HITL-for-you, keyword matches) → Signals → Notif; read-fanout the unbounded ambient set (channel/thread activity, unread counts) via the per-conversation log + lazy unread. The mega-channel never write-amplifies. **Read-state = Valkey hot markers + PG durable record + batched flush**, eventually-consistent, STOR-3-honouring (Valkey never authoritative), firehose-only events. | 03 |
| 4 | **Unfurl live-vs-snapshot + CHEAP per-viewer permission-aware resolution** | **Live per-viewer, never durable snapshots; store only the `artifact_ref` node + a post-time timestamp** (audit "as-of") — which makes erasure free. **Chat calls `refs.resolve` (the non-leaking chokepoint), never re-implements authz.** Cheapness = **lazy-on-viewport + a shared per-`ArtifactRef` projection cache (viewer-independent) gated by a per-viewer `check`/`list_objects` + membership-as-permission class precompute + bus-driven invalidation + resilient-client degradation.** | 04 |
| 5 | **Erasure mechanism specifics** | **Per-subject-DEK crypto-shred for bodies + drafts** (GD-4; bodies *are* PII) + **tombstone the record** (keep the fact, delete the content); **structured `mention(Principal)` neutralisation** via pseudonym-shred (free; ADR-05 payoff). Holder auto-registration enumerates every Chat store; the cascade reaches Search/Refs/Notif via the bus + DSR, never a backdoor. **Named floor:** free-text third-party names in others' un-erased bodies → retention + access-control + documented lawful-basis limit (→ P4 + LEGAL, GD-1 family). | 05 |
| — | **Agent presence/streaming semantics** | **Agent presence is its own fabric-health-derived class** (available/busy/rate-limited/offline), firehose transport, glyph+label not colour, no magic iconography. **Streaming partials ride the firehose; the final message is durable**; built/proven against mocks; verbosity calmed into threads. **Explicit-first dispatch (CHAT-1):** a mention notifies; it does not auto-spawn a costed run. | 07 |
| — | **HITL approval-card surface** | **Chat IS the surface; the bridge is `Id.check(human, approve, run)` → `DurableExecutor::signal(run, name, payload, idem_key=card_id)`** — wired, idempotent (a double-click is one approval), humanised (NOTIF-1), per-viewer-safe (args via `resolve`). The wait/timer/budget/resume are Workflow + Agent Fabric. Card also lands in the unified inbox (C-9) so a gate is never missed. | 06 |
| — | **Threads UX** | **Threads-first with explicit broadcast** (calm-by-default; matters more because agents raise volume). The thread pane hosts agent detail + streaming. | 08 |
| — | **Canvas-vs-Knowledge boundary** | **Canvas = an embedded/pinned Knowledge page (`ArtifactRef`), NOT a Chat-native editor** — Chat references, Knowledge authors. One editor render path, one content model, no duplication. Flagged for joint review; lean firm. | 08 |
| — | **Cross-org / federated channels** | **Deferred for v1; model does NOT foreclose it** — `Conversation.membership` may span tenants; rides the platform's deferred cross-cell PII-free pointer bridge + the explicit-opt-in cross-tenant capability + multi-cell DSR. Named floor. | 08 |

---

## 3. The glue contracts I commit to implementing (the obligation list)

Per the Phase-3 README §5 + the prompt's standing requirements, Chat will (Sketch 10 has the leaned shapes):

- **`serve(AppSpec)`** (or the cross-language shim if the gateway diverges — Sketch 09); **`OutboxTx::emit`
  as the only emit path** (no fire-and-forget from the gateway); **the three-surface topology**.
- **`project(ref, viewer) → {title,state,icon,render_hint,sub_anchor?}`** for `chat/channel|message|thread`
  (so others can unfurl chat) — per-viewer, pre-permission-checked.
- **`replay(scope, since) → *.snapshot`** (sub-artifact-granular) so Search/Refs/Notif/OLAP
  reindex-from-source; Chat is never read directly.
- **The complete `chat.*` taxonomy** under the Bus §6 grammar (durable vs **firehose** split — presence/
  typing/read-state/`agent.message.partial` are firehose-only); **`declare_indexable(IndexSpec)`** (Search
  always conjoins `list_objects`); **`ToolDef` registrations** (`chat.post/reply/react/create_channel/
  invite/start_dm/archive`, `requires_approval` defaulting on membership/lifecycle mutations).
- **ReBAC namespace fragment** (`channel.read = member + parent_project->read`; `message.view =
  parent_channel->read`) + the **`watcher` relation** (Notif read-fanout).
- **Stable `#sub` scheme** (`#message-<id>`, `#thread-<root>`, stable across edits).
- **`PersonalDataHolder`** over every Chat store; **honour the restriction flag**; **flag hot tables**
  (`message`, read-state) for forward-only migrations; **set per-surface shed budgets** (the connection-
  storm profile).
- **`reserve/settle`** is exercised wherever Chat dispatches spend-bearing agent work — through `EffectApi`
  (which reserves), not a Chat-private path.
- **`PersonalDataHolder` + the Rust `EventEnvelope` on the wire** are implemented **even if the gateway
  diverges to BEAM** (Sketch 09).

---

## 4. Floors named (honesty; VISION §3 / EI-04 §4) — and their follow-ons

| Floor (what ships partial) | Follow-on (named trigger / owner) |
|---|---|
| **Postgres-partitioned message hot tier** | ScyllaDB hot tier on **measured** per-cell volume (R-5) — `MessageStore` trait swap |
| **Free-text third-party mention erasure** (structured mentions are erasable; free-text names are not) | retention + access-control + documented lawful-basis limit → **P4 + LEGAL** (GD-1 family) |
| **Single-home-cell** (a global user's writes route to the tenant's home cell) | multi-region edge + the cross-cell PII-free bridge → **P4 control plane** (SC-2/SC-3) |
| **Cross-org / federated channels** (model permits, not built) | the cross-tenant opt-in capability + multi-cell DSR → **P4 + LEGAL** |
| **Canvas** (embed-a-Knowledge-page, not a Chat editor) | the pin/embed mechanism → joint Chat↔Knowledge review |
| **Connection-tier language = Rust** (BEAM divergence open-but-disfavoured) | exercise the BEAM escape hatch only if Rust presence-at-scale/tail-latency proves intractable in build — the wire contract makes it a gateway swap |
| **Mega-channel live delivery = NATS subject fan-out** | channel-sharded home-node on **measured** subscriber count (R-5) |

---

## 5. PROVE-IT — the drills each failable property owes (Phase-5 executes; I name them)

| Property | Quantified drill (gate) |
|---|---|
| **Zero messages lost across a reconnect** (Sketch 01 correctness backbone) | sever the gateway↔backplane mid-publish; assert resync from the durable log recovers the gap → **0 lost, 0 duplicate (idempotency nonce)** |
| **Unfurl no-leak** (Sketch 04; ADR-03) | notify/unfurl a confidential artifact to a viewer lacking access → **tombstone rendered, title never present** (inherits Refs/Notif D-N4) |
| **Unfurl erasure-safe** (Sketch 04/05) | erase a third party in a rendered card → **tombstone on next render, 0 recoverable PII** (no durable snapshot exists) |
| **Erasure reaches every Chat holder** (Sketch 05; T-5 family) | erase a person → assert bodies crypto-shred in hot + cold + **backups**; mentions → `[erased user]`; Search/Refs/Notif cascade → **0 recoverable PII** |
| **30× agent-surge: human lane holds** (ADR-16; the connection-storm profile) | 30× agent message/connection surge on one tenant → **human connection/read latency in budget, agent lane sheds (429+Retry-After honoured), other tenants unaffected** |
| **HITL approve→resume bridge** (Sketch 06; Workflow D-N7 analogue) | request an approval, kill Chat + Workflow mid-wait, approve days later → **the gated tool runs exactly once, a double-click is one approval, deny withholds with no mutation** |
| **Search ACL filter** (Sketch 10) | search as a non-member → **0 results from channels you're not in** (the `search-requires-acl-filter` lint + drill) |
| **Per-conversation order at scale** | burst sends + edits to one hot channel → **per-conversation total order preserved; resync gap-free** |
| **Frontend switch test** (T-7) | drive the real Chat UI in a browser → **a team could move to it without hitting a wall the old tool didn't have**; measured-contrast + latency budgets (T-8) |

---

## 6. Open questions I hand to my own Stage-2 architecture

1. **The TE-21 final call.** Stage 2 must *commit* Rust-or-BEAM in writing (Stage 1 leans Rust-with-open-
   divergence). If Rust: specify the connection-supervision + distributed-presence design on NATS. If BEAM:
   build out the full substrate shim (Sketch 09 items 2/5/6/7) and its drills.
2. **The exact `MessageStore` trait + the partition/sub-partition + cold-tiering mechanism** (Sketch 02):
   detach-to-object-segment lifecycle, the resync-gap range-read shape, the crypto-shred-per-message
   granularity inside an encrypted segment.
3. **The read-state store concrete design** (Sketch 03): Valkey schema, the batched-flush cadence + the
   STOR-3 reconstruction-on-cache-loss path, cross-device read-state truth, the link to Notif item read-state.
4. **The unfurl projection-cache concrete design** (Sketch 04): TTL + bus-invalidation keys, the
   membership-as-permission class precompute, the `list_objects` `Filter` push-down shape Chat composes.
5. **The HITL card data model + batch/multi-effect approval** (Sketch 06, joint with Workflow §6.3): can
   one card approve a plan of N effects; the card-anchoring rule; the `idem_key` per-effect scheme.
6. **Per-surface shed budgets** (substrate §13 Q3): the concrete connection-storm + agent-mention-storm
   caps and the protected-human-lane reservation size for the gateway.
7. **The complete `chat.*` taxonomy + `schema_ver` lineage + payload shapes** (Bus §10.1; Sketch 10) and
   the **default Signal/notify-reason rule set** Chat hands Notif (Notif §9 Q1/Q5).
8. **The free-text-erasure floor's documented lawful-basis limit** (Sketch 05), co-owned with LEGAL/DPO
   (GD-1 family) — the exact residual statement.
9. **Group-DM vs private-channel UX divergence** (Sketch 08) — confirm the `kind` set + the affordance
   differences over the unified machinery.

---

## 7. The one-line summary for the orchestrator

**Chat is mostly a careful *consumer* of the Phase-3 chokepoints (Refs `resolve`, Notif inbox, Workflow
`signal`, the crypto-shred triad, the fan-out hybrid), plus four genuinely-owned hard parts — the
connection tier (Rust default, BEAM open-but-disfavoured), the message store (PG-partitioned + cold-tier
floor → Scylla on measured volume), the read-state hot path (Valkey + PG, STOR-3), and cheap per-viewer
unfurls (lazy + shared-per-ref cache gated by `list_objects`). It IS the HITL approval-card surface and its
"Activity" inbox is a VIEW into the one Notif inbox (C-9). Agent dispatch is explicit-first (CHAT-1).
Erasure leans on per-subject crypto-shred + structured-mention neutralisation, with the free-text-mention
residual named as a floor.**
