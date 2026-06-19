# Chat — 05 · Hard Problems Resolved (with cited prior art + named floors)

> See [`00-overview.md`](./00-overview.md) for framing. This doc resolves each **subsystem-specific hard problem**,
> each with **cited prior art** and a **named floor** where v1 is partial, conformed to the reconciled contracts.
> The mechanisms live in [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md); this doc is the
> *justification + the literature*. The Phase-4 sketches ([`../sketches/`](../sketches/)) are the PRESERVED
> design record.

---

## 1. Connection-tier transport + language (TE-21) + the fan-out backplane

**Resolved: Rust gateway (default); the frozen firehose resume-cursor protocol (`subscribe/resume/scope`,
contract 3.5) as the live tier; resume-cursor resync from the durable log as the correctness backbone. The
BEAM/Phoenix divergence is written-but-disfavoured, bounded by the frozen cross-language harness shim (contract
1.7). Mega-channel delivery escalates to a channel-sharded home-node on measured volume.**

**The call, in writing (carried forward).** I commit **Rust** because: (1) **no GC pauses + low, predictable
memory-per-connection** are the canonical reason the Rust steer exists, a real edge at millions of long-lived
sockets; (2) **one runtime** — the hot Message/Read-state/Unfurl services are Rust, so the substrate glue (the
resilient client, fail-static, the telemetry survival signals the drills read, the outbox-only emit discipline,
the frozen firehose `subscribe/resume` client) is *linked, not re-implemented over a shim*; getting those subtly
wrong in a second runtime is a correctness/availability risk; (3) the platform **already runs the NATS-based
firehose tier in every cell** (event-bus §4.3), giving a sovereign, self-hostable PubSub *and* a
presence-suitable ephemeral channel **without** a second runtime.

**Why the BEAM hatch stays written (not foreclosed; ADR-02 honesty), now bounded by contract 1.7.** Phoenix
Channels is best-in-class for exactly this workload — the BEAM scheduler gives per-connection preemptive
scheduling (no head-of-line blocking from one busy connection, the property Rust must hand-build), supervision
trees, and **Phoenix.PubSub + Phoenix.Presence** essentially for free. If distributed presence-at-scale or tokio
scheduler tail-latency proves intractable in Rust during the build, the escape hatch is real — and **the wire
contract + the frozen cross-language harness shim (contract 1.7) make it a gateway-process swap, not a platform
rewrite**: the gateway is stateless (sockets + presence + resume cursors only, no durable store, no outbox),
speaks the Rust `EventEnvelope` on the wire, implements `PersonalDataHolder` over its ephemeral state, and calls
the Rust services by RPC for everything correctness-critical. The shim it would owe (three-surface,
liveness≠readiness, resilient-client + `Retry-After`, no fire-and-forget emit, the telemetry survival signals,
the protected-human-lane shed order, forward-only migrations) is **now frozen** as contract 1.7 — bounded but
non-trivial, which is *itself* the argument that the BEAM PubSub/Presence win does not clearly clear the cost.

**Prior art cited.** Discord *"How Discord Scaled Elixir to 5M Concurrent Users"* (2017) + the 2020 Go→Rust
read-states switch (GC pauses the named enemy) — the **exact Rust-hot-data / Elixir-gateway split** is real prior
art, which is why the hatch stays written. The Phoenix "2M connections on one box" benchmark (2015) and the
WhatsApp/Ejabberd Erlang-at-messaging-scale lineage are the BEAM case. Cloudflare's Rust edge proxies (millions
of connections) and the `tokio` work-stealing scheduler are the Rust case. The resume-cursor backbone is the
EI-04 §2.2 mandate ("build the durable resume-cursor transport FIRST; a real-time relay *without* resume cursors
silently loses the gap on a reconnect") — **now the platform's one frozen protocol** (contract 3.5, OQ-J),
co-designed once for ISS boards / KN docs / Chat channels.

**Floors:** (a) **connection-tier language = Rust** — exercise the BEAM hatch (bounded by contract 1.7) only if
Rust presence-at-scale/tail-latency proves intractable; the wire contract makes it a gateway swap. (b)
**Mega-channel delivery = firehose subject fan-out with per-view scope bounding**; the **channel-sharded
home-node** (the Phoenix/Discord guild model in Rust + consistent-hash) is the measured escalation (R-5) on
subscriber count exceeding the subject-fan-out budget. (c) **Single home-cell** for a tenant's chat; multi-region
edge + the frozen cross-cell PII-free bridge (contract 12.6, OQ-I) is the follow-on.

---

## 2. Message-store substrate + tiering (wide-column candidate)

**Resolved (carried forward): Postgres-partitioned hot tier (`(tenant, region)` + time sub-partitions) +
object-store cold tiering as item-zero, behind a `MessageStore` trait; ScyllaDB the named measured promotion
(R-5).**

The honest tension is **operational minimalism + outbox coherence (one engine) vs. the proven infinite-scale
chat-log shape (wide-column)**. I start on Postgres because: (1) **outbox transaction coherence is decisive** — a
message's persist and its `chat.message.created` event must commit together (BUS-2; the dual-write hazard is the
#1 silent-data-loss source); PG gives this in one transaction, a separate wide-column store forces a dual write.
(2) **GDPR is chat's dominant correctness axis** — per-subject crypto-shred (contract 11.4), mention
neutralisation, retention purge, DSR export are *standard on PG, bespoke on Scylla* (whose tombstone-GC degrades
reads under the delete-heavy erasure workload — a known Cassandra anti-pattern). (3) **The cell bounds the
scale** — a cell is one region's tenants (ADR-11), not the planet; the mandate is measure-before-shard (ADR-10).

**Prior art cited.** Discord *"How Discord Stores Trillions of Messages"* (2023, Cassandra→ScyllaDB) is the
canonical chat-log-on-wide-column case — kept as the **named promotion**, not the v1 default. The partitioned-PG
+ object-segment cold tier is the operational-minimalism path (EI-02 §8 "every additional data engine is
permanent operational cost"); content-addressed cold segments follow the Git object model / Venti (FAST 2002) /
IPFS CID lineage. k-sortable `message_id` (ULID) for intrinsic order follows the Snowflake/ULID lineage.
Crypto-shred-for-erasure follows Boneh & Lipton (1996) + NIST SP 800-88r1.

**Floor:** the partitioned-PG hot tier is v1; the follow-on is **ScyllaDB hot tier on measured per-cell
write/partition volume** (R-5) — a `MessageStore` trait swap, since the cold tier + the trait are identical under
either hot engine, residency-pinned + crypto-shred-capable per cell.

---

## 3. Write-vs-read fanout boundary

**Resolved: the platform hybrid (Notif §3.5) — write-fanout the bounded high-signal set; read-fanout the
unbounded ambient set. The mega-channel never write-amplifies.**

"Fan-out" has two distinct meanings the literature keeps separate: **live-delivery fan-out** (getting a post to
online sockets — read-fanout per channel, handled by the connection tier §1) and **attention fan-out**
(materialising "you were mentioned" / "you have unreads"). The platform *already decided* the attention split, so
Chat **produces the right signals into it**: write-fanout the **bounded high-signal set** (mentions via the
frozen structured `mention` node, DMs, thread-replies-to-you, HITL-for-you, keyword matches) → Signals → Notif;
read-fanout the **unbounded ambient set** (channel/thread activity, unread counts) via the per-conversation log +
lazy unread, with watchers resolved by `list_subjects(channel, watcher)` against the authz reverse index (contract
4.4, performant at 50k-member density). A 100k-member announcement does **zero** per-member inbox writes on a post.

**Read-state** is the churny hot path: **Valkey hot markers + PG durable record + batched flush**,
eventually-consistent, **cache-never-authoritative**, firehose-only events, a `PersonalDataHolder`. Unread is
**derived** (`count(id > last_read)`), never write-fanned-out ([02 §3](./02-internals-and-algorithms.md)).

**Prior art cited.** The hybrid fan-out is grounded in the feed-systems literature: Silberstein et al. *"Feeding
Frontier"* (VLDB 2010), Twitter's @-mention-vs-timeline split, Facebook TAO (ATC 2013). The Valkey-hot + DB-of-record
+ batched-flush read-state design + cache-never-authoritative is the platform invariant.

**Floor:** none beyond the platform's (Notif owns the inbox store/priority/storm-control; Chat owns only the
per-event class declaration).

---

## 4. Unfurl live-vs-snapshot + CHEAP per-viewer permission-aware resolution

**Resolved: live per-viewer, never durable snapshots; store only the `artifact_ref` node + a post-time
timestamp (audit "as-of"); Chat calls Refs `resolve` (the non-leaking chokepoint) over the frozen 4-step
tombstone ladder, never re-implements authz. Cheapness = lazy-on-viewport + a shared per-`ArtifactRef` projection
cache (viewer-independent) gated by a per-viewer `check`/`list_objects` (lowering the frozen `SetExpr`) +
membership-as-permission class precompute + bus-driven invalidation + resilient-client degradation.**

Two properties collide at scale: **per-viewer permission-aware** (viewer B lacking access gets a "no-access"
card, **never the title** — "THE subtlety that separates a real implementation from a demo"; ADR-03) and **live,
not snapshot** (a snapshot goes stale *and* freezes a third party's PII that may later be erased — an erasure
leak). The platform **already built the chokepoint** that solves both: Refs `resolve(ref, viewer, mode) →
Projection | Tombstone` (contract 5.2), degrading through the **one frozen 4-step ladder** (contract 5.7). So
Chat's problem narrows to making the per-viewer call *cheap* at chat density — the four-layer cheapening
([02 §4](./02-internals-and-algorithms.md)): (a) lazy-on-viewport (resolve only what's on screen); (b) split the
cache by viewer-independent projection content (cached **once per ref**) vs. the per-viewer `check` — **one cache
entry per ref, never per `(ref, viewer)`**, with no leak; (c) membership-as-permission class precompute via the
frozen `list_objects` `Filter` (the `SetExpr` lowered to a JOIN against the authz reverse index — one class
decision, not N, no N+1); (d) bus-driven invalidation on `*.updated`/`ci.check.updated`/`*.erased` (precise; TTL
the backstop).

**Prior art cited.** The non-leaking permission-aware resolution is the Zanzibar `LookupResources`/Leopard +
zookie pre-filter (Pang et al., USENIX ATC 2019; ADR-03), now realised as the frozen `SetExpr` JOIN target
(contract 4.3, OQ-E) — pre-filter, never post-filter (no leak, no N+1). The live-not-snapshot policy is
design-language §5.3. The "store the ref, not the rendered content" decision makes the audit record itself
references-not-payloads (EI-04 §1) and is *why* erasure is free (§5).

**Floor:** mega-channel unfurl *resolution* is already solved by lazy-on-viewport; mega-channel *delivery*
escalates per §1's home-node hatch. Drills: **unfurl-no-leak**, **unfurl-erasure-safe**.

---

## 5. Erasure mechanism specifics

**Resolved: per-subject-DEK crypto-shred for bodies + drafts (contract 11.4; bodies ARE PII, not references) +
tombstone the record; structured `mention(Principal)` neutralisation via the pseudonym-map shred (free; the
ADR-05 payoff). The free-text third-party residual is handled per the ONE platform posture (contract 10.9, recon
§X-7) BY REFERENCE — not restated here.**

Chat is "the most PII-dense holder — free-text bodies *about other people*" and "the stress test for the holder
spine." The crucial honesty: **a chat body is NOT references-not-payloads** — the body *is* the personal data. So
Chat leans hard on **crypto-shred** (per-subject DEK key destruction), the GD-4 "free-text/chat-body/agent-memory
= per-subject DEK" rule (contract 11.4) — chat is the canonical case. Two erasure roles, kept distinct
([02 §6](./02-internals-and-algorithms.md)): **author** (crypto-shred P's DEK → every body P authored
unrecoverable in hot + cold + backups simultaneously, **without** rewriting the immutable log; tombstone the
record) and **mentioned** (the structured `mention(Principal)` points at P's pseudonymous id → pseudonym-map shred,
contract 4.8 → renders `[erased user]` on next render, free because the node is structured + pseudonymous).
Holder auto-registration (contract 1.4) enumerates every Chat store so the cascade can't miss one; the cascade
reaches Search/Refs/Notif via the bus + DSR (contract 10.4), never a backdoor.

**The residual is the platform's, not Chat's (the X-7 change absorbed).** P's name typed into the **free-text
body** of someone else's un-erased message is encrypted under the **author's** DEK, not the subject's, so the
subject's erasure does not crypto-shred it. This is **exactly** the residual recon §X-7 names *once* across five
subsystems. Per the binding rule ("no subsystem doc restates it"), Chat handles it **by reference to contract
10.9**: best-effort `rectify`/tombstone of the span + the structural guarantee that the residual is never
indexed/agent-readable/in-analytics for a restricted subject (the `restrict` suppression). Chat supplies only the
**structural floor** (per-subject DEK shred + pseudonym-map shred + `restrict`); the lawful-basis statement is
the platform's single `[OPEN — LEGAL]` posture, ratified once by counsel/DPO. We do not pretend free-text
third-party mentions are perfectly erasable, and we do not write a fifth chat-specific residual.

**Prior art cited.** Crypto-shred = Boneh & Lipton (1996) + NIST SP 800-88r1 (key destruction renders ciphertext
unrecoverable). Tombstones = Kleppmann *DDIA* ch.5. The references-not-payloads + pseudonym-indirection triad =
EI-04 §1; the per-subject-DEK granularity = contract 11.4 (a per-tenant key would force erasing P to destroy
*everyone's* bodies).

**Floor:** the free-text third-party residual → handled per the platform posture (contract 10.9, `[OPEN —
LEGAL]`), not a chat-specific floor.

---

## 6. Canvas-vs-Knowledge boundary; threads (+ the OQ-L consolidation); conversation model

**Resolved: canvas = an embedded/pinned Knowledge page (`ArtifactRef`), NOT a Chat-native editor — Chat
references, Knowledge authors. Threads-first with explicit broadcast, over the frozen `#thread-` grammar. One
`Conversation` entity, many `kind`s. The document-anchored-comment-threading consolidation is a NAMED floor
(OQ-L), built over the shared `#sub`/content/refs scheme.**

A "canvas" (a pinned structured summary atop an incident channel) is a strong fit for Myelin but **overlaps
Knowledge** (non-goal). Building a canvas *inside* Chat would re-implement the Knowledge block editor + collab +
storage — the exact "don't build five editors" anti-pattern (EI-04 §2). So a channel's canvas is a **pinned
`knowledge/page` embedded via an `ArtifactRef`** (`conversation.pinned_canvas`, [01 §2](./01-tech-and-data-model.md)):
Chat owns the pin/placement; **Knowledge owns the editor, collab, content, storage, erasure**. One editor render
path (`render(parse(md)) === md`, the WASM core, contract 13.1), one content model (the frozen `myelin-content`,
ADR-05), no duplication — *share the AST, not the editor*.

**Threads-first with explicit broadcast** (the calm-by-default principle): a reply goes to its thread by default;
"also send to channel" is an explicit, deliberate broadcast — keeping agent verbosity and incident detail out of
the main timeline (which matters *more* in Myelin because agents raise volume — Zulip-style topic threading).

**The OQ-L threading consolidation (the named floor, absorbed).** Reconciliation confirms **two threading
implementations in v1, over ONE shared scheme**: Chat owns conversation-threads (real-time, presence, the
connection tier); Knowledge/Issues own document-anchored comment threads (anchored to a block/line/field via
`#sub`). They are **separate stores** because their concurrency/transport profiles differ — but they use the
**same `#thread-`/`#comment-` `#sub` grammar** (contract 5.7, X-4), the **same `myelin-content` AST** (contract
13.1, X-2), and emit the **same `refs.edge.created`** (contract 5.4). So a thread is addressable, referenceable,
and renderable identically regardless of host. **The consolidation follow-on (named, not "someday"):** when
document-anchored comments need real-time multi-party presence, promote them onto the **Chat threading primitive +
the firehose resume-cursor transport** (contract 3.5, OQ-J) — because they already share `#sub` + content + refs,
the promotion swaps the store/transport, not the data model. Tracked in the gap report (E-3) as "KB-native
comments floor → Chat-threading consolidation."

**One `Conversation` entity, many `kind`s:** group-DM and private channel are distinct `kind`s of one entity
(group-DM = "name == member set, membership-is-the-ACL, no topic"; private channel = "named, topic-scoped,
invite-managed"), unified machinery, two presentations — avoids a second fan-out path.

**Prior art cited.** The "share the content model + editor, not re-build it" rule = ADR-05 + the one-editor-render-path
(EI-05 §2). Threads-first = Zulip topic threading. One-entity-many-kinds = the design-language §2 "one component,
adapt presentation" principle. The shared-scheme consolidation is the reconciliation's named floor (OQ-L).

**Floor:** **canvas** is build-via-embed (cheap because it reuses Knowledge) but is not a v1 *hard* commit; the
pin/embed mechanism is flagged for the joint review. **Comment-threading consolidation** is named-not-built (OQ-L).

---

## 7. Cross-org / federated channels

**Resolved: deferred for v1; the model does NOT foreclose it; rides the FROZEN cross-cell pointer bridge (contract
12.6, OQ-I) when it ships.** A cross-org "Slack Connect"-style shared channel has deep identity, residency, and
erasure implications. The honest constraints if/when built:

- **Residency** — a channel shared across cells rides the **frozen `CrossCellPointer{subject, type,
  correlation_id, home_cell}`** (contract 12.6, OQ-I): only the opaque pointer crosses, never payload/PII;
  **per-viewer resolution is always cell-local** — A's gateway asks **cell B** to `resolve(ref, viewer, mode)`
  in B, permission-checked in B against B's tuples, returning only the already-rendered, already-permission-filtered
  projection (or a tombstone), never raw rows.
- **Identity** — a member from org B in org A's channel is a cross-tenant principal, and **there is no
  cross-tenant query path** (EI-02 §1); the *mechanism* (a narrow explicit-grant userset, never a cross-tenant
  join) is decided, the *policy* is P6/legal.
- **Erasure** — erasing a person in org B who posted in a shared channel reaches org A's cell via the DSR
  orchestrator's multi-cell `member_cells` iteration (contract 10.4), tractable *because* the triad is
  references-not-payloads + crypto-shred (§5).

**The non-foreclosure rule:** `Conversation.membership` is a set of principals that *could* span tenants
([01 §2](./01-tech-and-data-model.md)), gated by the cross-tenant capability when it ships.

**Prior art cited.** The cross-cell PII-free bridge = the frozen platform floor (contract 12.6, OQ-I); the
multi-cell DSR = contract 10.4.

**Floor:** cross-org/federated channels are **designed-not-built**; → P6 control plane + LEGAL.

---

## 8. Summary table (each hard problem → resolution → prior art → floor)

| Hard problem | Resolution | Prior art | Floor |
|---|---|---|---|
| Connection tier + language (TE-21) | Rust gateway; frozen firehose resume-cursor protocol (3.5); resync backbone | Discord Elixir/Rust split; Phoenix 2M; tokio; EI-04 §2.2 | Rust (BEAM hatch bounded by 1.7); home-node mega-channel; single home-cell |
| Message store + tiering | PG-partitioned hot + object cold; `MessageStore` trait | Discord Scylla; Venti/IPFS CID; ULID; Boneh-Lipton | Scylla hot tier on measured volume (R-5) |
| Write-vs-read fanout | platform hybrid; mega-channel never write-amplifies; Valkey+PG read-state | Feeding Frontier (VLDB 2010); TAO | — |
| Cheap per-viewer unfurls | live; Refs `resolve` over the 4-step ladder; shared-per-ref cache; frozen `SetExpr` JOIN; lazy | Zanzibar LookupResources/Leopard; design-language §5.3 | mega-channel delivery → home-node |
| Erasure specifics | per-subject DEK crypto-shred + tombstone; pseudonym-map shred; residual per contract 10.9 BY REFERENCE | Boneh-Lipton; NIST 800-88r1; DDIA ch.5; contract 11.4 | free-text third-party residual → platform posture 10.9 (LEGAL) |
| Canvas / threads / conversation model | embed-a-Knowledge-page; threads-first; one entity many kinds; OQ-L shared scheme | ADR-05; EI-05 §2; Zulip | canvas build-via-embed; comment-threading consolidation named (OQ-L) |
| Cross-org / federated | deferred; rides the frozen cross-cell bridge (12.6) | cross-cell bridge OQ-I; multi-cell DSR 10.4 | designed-not-built (→ P6 + LEGAL) |

Continue to [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md).
