# Chat — 05 · Hard Problems Resolved (with cited prior art + named floors)

> See [`00-overview.md`](./00-overview.md) for framing. This doc resolves each **subsystem-specific hard problem**
> the Phase-3 handoff assigned Chat, each with **cited prior art** and a **named floor** where v1 is partial. The
> mechanisms live in [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md); this doc is the
> *justification + the literature*. Stage-1 weighed these in [`../sketches/`](../sketches/); this is the binding
> write-up.

---

## 1. Connection-tier transport + language (TE-21) + the fan-out backplane

**Resolved: Rust gateway (default); NATS-core subject-per-channel backplane; resume-cursor resync from the
durable log as the correctness backbone. The BEAM/Phoenix divergence is written-but-disfavoured. Mega-channel
delivery escalates to a channel-sharded home-node on measured volume.**

**The call, in writing.** The Phase-3 handoff names this "the most likely Rust divergence." I commit **Rust**
because: (1) **no GC pauses + low, predictable memory-per-connection** are the canonical reason the Rust steer
exists, and they are a real edge at millions of long-lived sockets (Phase-2 Chat §3); (2) **one runtime** — the
hot Message/Read-state/Unfurl services are Rust, so the substrate glue (the resilient client, fail-static, the
telemetry survival signals the drills read, the outbox-only emit discipline) is *linked, not re-implemented over
a shim*; getting those subtly wrong in a second runtime is a correctness/availability risk; (3) the platform
**already runs NATS in every cell** (Bus §2.1), giving a sovereign, self-hostable PubSub *and* a presence-suitable
ephemeral channel **without** a second runtime — Rust gets the fan-out win without forfeiting one-runtime
coherence.

**Why the BEAM hatch stays written (not foreclosed; ADR-02 honesty).** Phoenix Channels is best-in-class for
exactly this workload — the BEAM scheduler gives per-connection preemptive scheduling (no head-of-line blocking
from one busy connection, the property Rust must hand-build), supervision trees, and **Phoenix.PubSub +
Phoenix.Presence** essentially for free. If distributed presence-at-scale or tokio scheduler tail-latency proves
intractable in Rust during the build, the escape hatch is real — and **the wire contract makes it a
gateway-process swap, not a platform rewrite** (Sketch 09): the gateway is stateless (sockets + presence + resync
cursors only, no durable store, no outbox), speaks the Rust `EventEnvelope` on the wire, implements
`PersonalDataHolder` over its ephemeral state, and calls the Rust services by RPC for everything correctness-
critical. The shim it would owe (liveness≠readiness, resilient-client + `Retry-After`, the telemetry survival
signals, the protected-human-lane shed order) is bounded but non-trivial — which is *itself* the argument that
the BEAM PubSub/Presence win does not clearly clear the cost.

**Prior art cited.** Discord *"How Discord Scaled Elixir to 5M Concurrent Users"* (2017) + the 2020 Go→Rust
read-states switch (GC pauses the named enemy) — the **exact Rust-hot-data / Elixir-gateway split** is real prior
art, which is why the hatch stays written. The Phoenix "2M connections on one box" benchmark (2015) and the
WhatsApp/Ejabberd Erlang-at-messaging-scale lineage are the BEAM case. Cloudflare's Rust edge proxies (millions
of connections) and the `tokio` work-stealing scheduler design are the Rust case. The resume-cursor backbone is
the EI-04 §2.2 mandate ("a real-time relay *without* resume cursors silently loses the gap on a reconnect")
applied to chat delivery.

**Floors:** (a) **connection-tier language = Rust** — exercise the BEAM hatch only if Rust presence-at-scale/
tail-latency proves intractable; the wire contract makes it a gateway swap. (b) **Mega-channel delivery = NATS
subject fan-out**; the **channel-sharded home-node** (the Phoenix/Discord guild model in Rust + consistent-hash)
is the measured escalation (R-5) on subscriber count exceeding the subject-fan-out budget. (c) **Single home-cell**
for a tenant's chat; multi-region edge + the cross-cell PII-free bridge is the follow-on (SC-2/SC-3).

---

## 2. Message-store substrate + tiering (wide-column candidate)

**Resolved: Postgres-partitioned hot tier (`(tenant, region)` + time sub-partitions) + object-store cold
tiering as item-zero, behind a `MessageStore` trait; ScyllaDB the named measured promotion (R-5).**

The honest tension is **operational minimalism + outbox coherence (one engine) vs. the proven infinite-scale
chat-log shape (wide-column)**. I start on Postgres because: (1) **outbox transaction coherence is decisive** — a
message's persist and its `chat.message.created` event must commit together (BUS-2; the dual-write hazard is the
#1 silent-data-loss source); PG gives this in one transaction, a separate wide-column store forces a dual write
(the exact seam Workflow §2.3 rejected Temporal for). (2) **GDPR is chat's dominant correctness axis** —
per-subject crypto-shred, mention neutralisation, retention purge, DSR export are *standard on PG, bespoke on
Scylla* (whose tombstone-GC degrades reads under the delete-heavy erasure workload — a known Cassandra
anti-pattern). (3) **The cell bounds the scale** — a cell is one region's tenants (ADR-11), not the planet, so
the "single PG melts" intuition (calibrated to a global single DB Myelin never has) likely doesn't apply at
realistic per-cell volume; the mandate is measure-before-shard (ADR-10).

**Prior art cited.** Discord *"How Discord Stores Trillions of Messages"* (2023, Cassandra→ScyllaDB) is the
canonical chat-log-on-wide-column case — kept as the **named promotion**, not the v1 default. The
partitioned-PG + object-segment cold tier is the operational-minimalism path (EI-02 §8 "every additional data
engine is permanent operational cost"); content-addressed cold segments follow the Git object model / Venti
(FAST 2002) / IPFS CID lineage. k-sortable `message_id` (ULID) for intrinsic order follows the Snowflake/ULID
lineage (Phase-1 §2.3). Crypto-shred-for-erasure follows Boneh & Lipton (1996) + NIST SP 800-88r1.

**Floor:** the partitioned-PG hot tier is v1; the follow-on is **ScyllaDB hot tier on measured per-cell
write/partition volume** (R-5) — a `MessageStore` trait swap, since the cold tier + the trait are identical under
either hot engine.

---

## 3. Write-vs-read fanout boundary

**Resolved: the platform hybrid (Notif §3.5) — write-fanout the bounded high-signal set; read-fanout the
unbounded ambient set. The mega-channel never write-amplifies.**

"Fan-out" has two distinct meanings the literature keeps separate, and conflating them is the classic mistake:
**live-delivery fan-out** (getting a post to online sockets — read-fanout per channel, handled by the connection
tier §1) and **attention fan-out** (materialising "you were mentioned" / "you have unreads"). The platform
*already decided* the attention split, so Chat **produces the right signals into it** rather than inventing a
second model: write-fanout the **bounded high-signal set** (mentions via structured `mention` nodes, DMs,
thread-replies-to-you, HITL-for-you, keyword matches) → Signals → Notif; read-fanout the **unbounded ambient set**
(channel/thread activity, unread counts) via the per-conversation log + lazy unread. A 100k-member announcement
does **zero** per-member inbox writes on a post.

**Read-state** is the churny hot path: **Valkey hot markers + PG durable record + batched flush**,
eventually-consistent, **STOR-3-honouring (Valkey never authoritative)**, firehose-only events, a
`PersonalDataHolder`. Unread is **derived** (`count(id > last_read)`), never write-fanned-out
([02 §3](./02-internals-and-algorithms.md)).

**Prior art cited.** The hybrid fan-out is grounded in the feed-systems literature: Silberstein et al. *"Feeding
Frontier"* (VLDB 2010), Twitter's @-mention-vs-timeline split (*Timelines at Scale*), Facebook TAO (ATC 2013).
The Valkey-hot + DB-of-record + batched-flush read-state design is Phase-1 §5.6's recommendation; STOR-3
(cache-never-authoritative) is the platform invariant.

**Floor:** none beyond the platform's (Notif owns the inbox store/priority/storm-control; Chat owns only the
per-event class declaration).

---

## 4. Unfurl live-vs-snapshot + CHEAP per-viewer permission-aware resolution

**Resolved: live per-viewer, never durable snapshots; store only the `artifact_ref` node + a post-time
timestamp (audit "as-of"); Chat calls Refs `resolve` (the non-leaking chokepoint), never re-implements authz.
Cheapness = lazy-on-viewport + a shared per-`ArtifactRef` projection cache (viewer-independent) gated by a
per-viewer `check`/`list_objects` + membership-as-permission class precompute + bus-driven invalidation +
resilient-client degradation.**

Two properties collide at scale: **per-viewer permission-aware** (viewer B lacking access gets a "no-access"
card, **never the title** — "THE subtlety that separates a real implementation from a demo", Phase-1 §2.4;
ADR-03) and **live, not snapshot** (a snapshot goes stale *and* freezes a third party's PII that may later be
erased — an erasure leak). The platform **already built the chokepoint** that solves both: Refs `resolve(ref,
viewer, mode) → Projection | Tombstone` (Refs §4.2). So Chat's problem narrows to making the per-viewer call
*cheap* at chat density — solved by the four-layer cheapening ([02 §4](./02-internals-and-algorithms.md)): (a)
lazy-on-viewport (resolve only what's on screen — the single biggest cost-killer); (b) split the cache by
viewer-independent projection content (cached **once per ref**) vs. the per-viewer `check` (the fast Leopard
pre-filter) — **one cache entry per ref, never per `(ref, viewer)`**, with no leak (the Refs §4.2 correctness
argument); (c) membership-as-permission class precompute via `list_objects` `Filter` (one class decision, not N);
(d) bus-driven invalidation on `*.updated`/`*.erased` (precise; TTL the backstop).

**Prior art cited.** The non-leaking permission-aware resolution is the Zanzibar `list-objects`/Leopard +
zookie pre-filter (Pang et al., USENIX ATC 2019; ADR-03) — pre-filter, never post-filter (no leak, no N+1). The
live-not-snapshot policy is design-language §5.3. The "store the ref, not the rendered content" decision makes
the audit record itself references-not-payloads (EI-04 §1) and is *why* erasure is free (§5).

**Floor:** mega-channel unfurl *resolution* is already solved by lazy-on-viewport; mega-channel *delivery*
escalates per §1's home-node hatch. Drills: **unfurl-no-leak**, **unfurl-erasure-safe** (inherit Refs/Notif
D-N4/D-N6).

---

## 5. Erasure mechanism specifics

**Resolved: per-subject-DEK crypto-shred for bodies + drafts (GD-4; bodies ARE PII, not references) +
tombstone the record (keep the fact, delete the content); structured `mention(Principal)` neutralisation via
pseudonym-shred (free; the ADR-05 payoff). Free-text third-party names are a named floor.**

Chat is "the most PII-dense holder — free-text bodies *about other people*" and "the stress test for the holder
spine" (Phase-2 Chat §8.5). The crucial honesty: **a chat body is NOT references-not-payloads** — the body *is*
the personal data. So Chat leans hard on **crypto-shred** (per-subject DEK key destruction), the GD-4
"free-text/chat-body/agent-memory = per-subject DEK" rule (Storage §5.1) — chat is the canonical case for it.
Two erasure roles, kept distinct ([02 §6](./02-internals-and-algorithms.md)): **author** (crypto-shred P's DEK →
every body P authored unrecoverable in hot + cold + backups simultaneously, **without** rewriting the immutable
log; tombstone the record) and **mentioned** (the structured `mention(Principal)` points at P's pseudonymous id
→ pseudonym-shred → renders `[erased user]` on next render, free because the node is structured + pseudonymous).
Holder auto-registration enumerates every Chat store so the cascade can't miss one; the cascade reaches
Search/Refs/Notif via the bus + DSR, never a backdoor.

**Prior art cited.** Crypto-shred = Boneh & Lipton (1996) + NIST SP 800-88r1 (key destruction renders ciphertext
unrecoverable). Tombstones = Kleppmann *DDIA* ch.5. The references-not-payloads + pseudonym-indirection triad =
EI-04 §1; Bus §4.8; Storage §5.1. Per-subject vs per-tenant DEK granularity = GD-4 (a per-tenant key would force
erasing P to destroy *everyone's* bodies).

**Floor (the residual, named honestly):** P's name typed into the **free-text body** of someone else's un-erased
message is **not** a structured node and **cannot** be surgically neutralised without content analysis. Covered by
**retention + access-control + a documented lawful-basis limit** — the chat analogue of GD-1's git-history
residual. → **P4 + LEGAL** ([06](./06-shared-system-change-requests.md)). We do not pretend it is perfectly
erasable.

---

## 6. Canvas-vs-Knowledge boundary; threads; group-DM vs private channel

**Resolved: canvas = an embedded/pinned Knowledge page (`ArtifactRef`), NOT a Chat-native editor — Chat
references, Knowledge authors. Threads-first with explicit broadcast. One `Conversation` entity, many `kind`s.**

A "canvas" (a pinned structured summary atop an incident channel, Slack-canvas-like) is a strong fit for Myelin
but **overlaps Knowledge** (Phase-2 Chat §1 non-goal). Building a canvas *inside* Chat would re-implement the
Knowledge block editor + collab + storage — the exact "don't build five editors" anti-pattern (EI-04 §2; KN-1/
KN-4). So a channel's canvas is a **pinned `knowledge/page` embedded via an `ArtifactRef`**
(`conversation.pinned_canvas`, [01 §2](./01-tech-and-data-model.md)): Chat owns the pin/placement; **Knowledge
owns the editor, collab, content, storage, erasure**. One editor render path (`render(parse(md)) === md`), one
content model (`myelin-content`, ADR-05), no duplication — *share the AST, not the editor*
([Knowledge 00](../../knowledge-platform/architecture/00-overview.md)). The boundary is **flagged for the joint
Chat↔Knowledge review** but the lean is firm.

**Threads-first with explicit broadcast** (Phase-1 §2.5 recommendation): a reply goes to its thread by default;
"also send to channel" is an explicit, deliberate broadcast — keeping agent verbosity and incident detail out of
the main timeline (P8; calm-by-default, which matters *more* in Myelin because agents raise volume — Zulip-style
topic threading, competitive-landscape §5). **One `Conversation` entity, many `kind`s** (Phase-2 Chat §1):
group-DM and private channel are distinct `kind`s of one entity (group-DM = "name == member set,
membership-is-the-ACL, no topic"; private channel = "named, topic-scoped, invite-managed"), unified machinery,
two presentations — avoids a second fan-out path.

**Prior art cited.** The "share the content model + editor, not re-build it" rule = ADR-05 + design-language
§5.9; the one-editor-render-path = EI-05 §2. Threads-first = Phase-1 §2.5; Zulip topic threading. One-entity-
many-kinds = the design-language §2 "one component, adapt presentation" principle.

**Floor:** **canvas** is build-via-embed (cheap because it reuses Knowledge) but is not a v1 *hard* commit; the
pin/embed mechanism is flagged for the joint review.

---

## 7. Cross-org / federated channels

**Resolved: deferred for v1; the model does NOT foreclose it.** A cross-org "Slack Connect"-style shared channel
has deep identity, residency, and erasure implications (Phase-1 §2.1/§9.13). The honest constraints if/when built:
**residency** — a channel shared across cells rides the **control-plane PII-free pointer bridge** (the same
cross-cell mechanism Bus §7.4 / Refs §6.5 / Notif §5.4 all defer): only `subject`/`type`/`correlation_id` cross,
never payload/PII; per-viewer resolution always local to the cell holding the artifact. **Identity** — a member
from org B in org A's channel is a cross-tenant principal, and **there is no cross-tenant query path** (ID-3); the
*mechanism* (a narrow explicit-grant userset, never a cross-tenant join) is decided, the *policy* is P4/legal.
**Erasure** — erasing a person in org B who posted in a shared channel reaches org A's cell via the DSR
orchestrator's multi-cell `member_cells` iteration (S-9); tractable *because* the triad is references-not-payloads
+ crypto-shred (§5).

**The non-foreclosure rule:** `Conversation.membership` is a set of principals that *could* span tenants
([01 §2](./01-tech-and-data-model.md)), gated by the cross-tenant capability when it ships. We design the model to
permit it; we don't build it in v1.

**Prior art cited.** The cross-cell PII-free bridge = the platform's deferred floor (Tenancy §10; Bus §7.4); the
multi-cell DSR = GDPR §4.4 + S-9; the explicit cross-tenant capability = the `[OPEN → P4/legal]` Refs §6.4 names
for cross-tenant inbound refs.

**Floor:** cross-org/federated channels are **designed-not-built**; → P4 control plane + LEGAL.

---

## 8. Summary table (each hard problem → resolution → prior art → floor)

| Hard problem | Resolution | Prior art | Floor |
|---|---|---|---|
| Connection tier + language (TE-21) | Rust gateway; NATS-core backplane; resume-cursor resync | Discord Elixir/Rust split; Phoenix 2M; tokio; EI-04 §2.2 | Rust (BEAM hatch); home-node mega-channel; single home-cell |
| Message store + tiering | PG-partitioned hot + object cold; `MessageStore` trait | Discord Scylla; Venti/IPFS CID; ULID; Boneh-Lipton | Scylla hot tier on measured volume (R-5) |
| Write-vs-read fanout | platform hybrid; mega-channel never write-amplifies; Valkey+PG read-state | Feeding Frontier (VLDB 2010); TAO; Phase-1 §5.6 | — |
| Cheap per-viewer unfurls | live; Refs `resolve`; shared-per-ref cache gated by per-viewer `check`; lazy | Zanzibar list-objects/Leopard; design-language §5.3 | mega-channel delivery → home-node |
| Erasure specifics | per-subject DEK crypto-shred + tombstone; structured-mention pseudonym-shred | Boneh-Lipton; NIST 800-88r1; DDIA ch.5; GD-4 | free-text third-party names (→ LEGAL) |
| Canvas / threads / conversation model | embed-a-Knowledge-page; threads-first; one entity many kinds | ADR-05; EI-05 §2; Phase-1 §2.5; Zulip | canvas build-via-embed not a hard commit |
| Cross-org / federated | deferred; model doesn't foreclose | cross-cell bridge; multi-cell DSR; ID-3 | designed-not-built (→ P4 + LEGAL) |

Continue to [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md).
