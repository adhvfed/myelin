# Chat — 01 · Technology, Runtime & Data Model

> See [`00-overview.md`](./00-overview.md) for framing and the document split. This doc commits the
> language/runtime/database choices (with written justification — the TE-21 connection-tier call), and the
> complete data model: the `Conversation` entity, the `message` log + `MessageStore` trait + tiering, the
> read-state store, the unfurl projection cache, membership/drafts/prefs. Schemas are illustrative Postgres/Rust;
> the **shape** is the contract.

---

## 1. The language / runtime / database choice (with written justification)

**Decision: Rust for ALL Chat services — including the connection-tier gateway by default; PostgreSQL-class
OLTP as the message system-of-record (behind a `MessageStore` trait, ScyllaDB the named measured promotion); an
S3-compatible object store for the cold-tier message segments; Valkey for the read-state/presence hot path and
the unfurl cache; NATS core (the cell's existing firehose transport) as the live fan-out + presence backplane.
The BEAM/Elixir+Phoenix divergence for the gateway (TE-21) is kept WRITTEN but DISFAVOURED.** Below is the
written justification each part owes.

### 1.1 The connection tier — the TE-21 call, made in writing: **Rust default, BEAM escape hatch**

The Phase-3 handoff names the real-time connection tier "the most likely Rust divergence (TE-21)." The call,
committed for build/test, is **Rust** — with the **BEAM/Phoenix divergence kept written-and-open but
disfavoured**, gated on one thing: whether distributed presence-at-scale and tokio scheduler tail-latency prove
tractable in Rust during the build. The full weighing is in [05 §1](./05-hard-problems.md); the decision and its
consequences:

| Concern | Choice | Written justification |
|---|---|---|
| Gateway language/runtime | **Rust** (`tokio` + `tokio-tungstenite` WS + `axum`/`tower` HTTP/WS-upgrade/SSE; tasks-as-lightweight-actors) | ADR-02 default. **No GC pauses + low, predictable memory-per-connection** are a real edge at millions of sockets (the canonical reason the Rust steer exists; Phase-2 Chat §3). **One runtime** — the Message/Read-state/Unfurl services are already Rust, so the glue crates (`serve(AppSpec)`, the resilient client, fail-static, the telemetry signal set, the consumer template) are **linked, not wired over a shim** (Sketch 09 — the substrate non-negotiables come for free). |
| Why NOT BEAM/Phoenix (the disfavoured divergence) | **Held open, not chosen** | Phoenix gives PubSub + Presence essentially for free — the single best external fit — but at the cost of (a) a **second runtime** per cell and (b) a **substantive substrate shim** in Elixir: liveness≠readiness, the resilient-client + `Retry-After` honouring, the telemetry survival signals the Phase-5 drills read, and the protected-human-lane shed order (Sketch 09 items 2/5/6/7). Getting those subtly wrong in a second runtime is a correctness/availability risk; the platform **already runs NATS in every cell** (Bus §2.1), which gives a sovereign, self-hostable PubSub *and* a presence-suitable ephemeral channel **without** a new runtime. The honest read: the owed shim is enough work that the free PubSub/Presence win does not clearly clear it. |
| Prior art keeping the divergence honest | Discord: Rust for hot data-services (read-states), Elixir for the real-time gateway/guild fan-out | Discord eng *"How Discord Scaled Elixir to 5M Concurrent Users"* (2017) + the 2020 Go→Rust read-states switch (GC pauses were the named enemy) — this **exact split** is real prior art, which is why the BEAM hatch stays written. The Phoenix "2M connections on one box" benchmark (2015) and the WhatsApp/Ejabberd lineage are the BEAM case. |
| The wire contract either way | **The gateway speaks the Rust `EventEnvelope` on the wire + implements `PersonalDataHolder`** | Mandatory regardless of language (ADR-02 §consequences; the prompt's standing rule). The gateway is **stateless** — live sockets + in-memory presence + resync cursors only, no durable store, no outbox — so the cross-language shim is its *thinnest* possible form (Sketch 09), and the divergence is a **gateway-process swap**, not a subsystem rewrite. |

**EU-deployable / self-hostable: confirmed.** Every component is self-hostable with no US-controlled SaaS:
PostgreSQL, an S3-compatible store (MinIO/Ceph/Garage), NATS (core + JetStream, in-cell), Valkey (the
BSD-licensed Redis fork the platform already runs). A managed realtime SaaS (Ably/Pusher/PubNub) is **rejected
outright** — non-sovereign, not self-hostable (VISION §1).

**Glue-contract implementability across the boundary: confirmed.** In the default (all-Rust) world the glue
contracts are linked types. If the gateway diverges to BEAM, the contracts it must *originate* are small (the
envelope it forwards, the `ArtifactRef` strings it carries, the resync cursors); everything correctness-critical
(persist, emit, authorize, unfurl) is an **RPC call into the Rust services**, which already speak the contracts
(Sketch 09 Part A). The shim is fully specified (Sketch 09 Part B) — which is *why* the divergence is admissible
at all.

### 1.2 The message store — PostgreSQL-partitioned hot tier, behind a `MessageStore` trait

**Decision: a Postgres-partitioned hot tier (partitioned by `(tenant, region)`, time sub-partitioned) +
object-store cold tiering as item-zero, behind a `MessageStore` trait; ScyllaDB is the named measured promotion
(R-5).** The *written why* (full weighing [05 §2](./05-hard-problems.md)):

- **Outbox transaction coherence is decisive.** A message's durable persist and its `chat.message.created` event
  **must** commit together (BUS-2; the dual-write hazard is the #1 silent-data-loss source, substrate D-1). PG
  gives this natively in one transaction; a separate wide-column store forces an outbox-in-PG + message-in-Scylla
  **dual write** — re-introducing exactly the seam Workflow §2.3 rejected Temporal for. The message row and its
  outbox row share the PG WAL.
- **GDPR is chat's dominant correctness axis, not raw throughput.** Per-subject crypto-shred, per-subject
  mention neutralisation, per-channel retention purge, and DSR export are **standard Storage patterns on PG** and
  **bespoke on Scylla** (whose tombstone-GC interacts badly with the delete-heavy erasure workload — a known
  anti-pattern). Chat is "the stress test for the holder spine" — building it where the holder patterns are
  native lowers the risk that erasure is subtly wrong (the worst failure class here).
- **The cell bounds the scale.** A cell is one region's tenants (ADR-11), not the planet; the "single PG melts"
  intuition is calibrated to a global single DB, which Myelin never has. Realistic per-cell volume is plausibly
  within partitioned-PG + cold-tiering reach — **and the mandate is measure-before-shard (ADR-10)**.
- **The trait makes the promotion a swap.** `MessageStore { append, range, tombstone, resync_from }` — the cold
  tier and the trait are identical under either hot engine, so promoting to Scylla is a hot-tier swap, not a
  redesign (the same escape-hatch-behind-a-trait philosophy as Bus JetStream→Kafka, Workflow PG→Temporal).

### 1.3 Read-state, presence, unfurl cache — Valkey; live backplane — NATS core

| Concern | Choice | Justification |
|---|---|---|
| Read-state hot markers + counters | **Valkey** (the cache/coordination store the platform already runs) + PG durable record | Phase-1 §5.6 ("a separate fast KV, batched eventually-consistent writes"). **STOR-3 is law: Valkey is NEVER the source of truth** — the PG record is, reconstructable; a cache loss makes a marker at-worst slightly stale (you re-see a few read messages — benign, bounded). [02 §3](./02-internals-and-algorithms.md). |
| Unfurl shared projection cache | **Valkey** (short-TTL, bus-invalidated) | Viewer-independent projection content cached once per `ArtifactRef`; the per-viewer permission gate is a separate `list_objects`/`check` (Id's fast Leopard pre-filter). [02 §4](./02-internals-and-algorithms.md). |
| Live fan-out + presence/typing backplane | **NATS core** (the cell's existing firehose transport, Bus §4.3) | Subject-per-channel routing is the literal mechanism needed; EU-sovereign, self-hostable, already deployed; non-durable at-most-once is *correct* here (the durable log is the truth; a missed frame is recovered by resync). Reusing NATS avoids a second ephemeral transport (operational minimalism, EI-02 §8). [02 §1/§2](./02-internals-and-algorithms.md). |
| Cold-tier message segments | **S3-compatible object store** (content-addressed, BLAKE3), behind `BlobStore` | STOR-1; an archived conversation range seals to an encrypted segment, still range-readable (a cold read = segment fetch + decrypt), still crypto-shreddable (destroy the per-tenant/per-subject DEK). |
| Message content model | **`myelin-content`** shared crate (Knowledge-led, ADR-05) | Chat **consumes** the block/inline AST; it does **not** re-implement an editor. One editor render path (`render(parse(md)) === md`); the composer compiles the same Rust `myelin-content` core to WASM as Knowledge (DL §8.1 — share the implementation, not the spec). [04](./04-views-cli-and-api.md). |
| HITL durable wait | **`myelin-flow`** (`DurableExecutor`) | Chat does not reinvent durable waits/timers; it posts the approval `signal` ([02 §5](./02-internals-and-algorithms.md)). |

---

## 2. The conversation model — one entity, many kinds (ADR; Sketch 08)

One `Conversation` entity with a `kind` discriminator and a membership strategy — **not** five tables — so DMs,
group-DMs, channels, artifact-linked channels, and announcements share the *same* read/write/fan-out/erasure
machinery (avoid duplicating the hardest logic five times; Phase-2 Chat §1).

```sql
CREATE TYPE conversation_kind AS ENUM (
  'channel_public', 'channel_private', 'dm', 'group_dm', 'artifact_linked', 'announcement'
);

CREATE TABLE conversation (
  tenant          uuid              NOT NULL,
  region          text              NOT NULL,            -- residency-pinned (ADR-11); == cell.region (residency-pin lint, S-1)
  conversation_id uuid              NOT NULL,
  kind            conversation_kind NOT NULL,
  parent_project  uuid,                                  -- the project a channel lives in (channel.read TTU rewrite, §5)
  name            text,                                  -- NULL for dm/group_dm (name == member set; Sketch 08)
  topic           text,                                  -- channel topic; contains_personal_data possible
  linked_ref      text,                                  -- artifact_linked: the ArtifactRef this channel was born from
  pinned_canvas   text,                                  -- a knowledge/page ArtifactRef (the embedded canvas, Sketch 08)
  retention_days  int,                                   -- per-channel auto-delete policy hook (GDPR; engine is GDPR's)
  archived        boolean           NOT NULL DEFAULT false,
  created_by      uuid              NOT NULL,            -- pseudonymous principal_id (erasure-safe)
  created_at      timestamptz       NOT NULL,
  PRIMARY KEY (tenant, conversation_id)
) PARTITION BY LIST (tenant, region);                    -- residency in the partition key
```

- **`kind` adapts presentation, not storage** (design-language §2 "one component, adapt presentation"). A
  group-DM is "a private conversation whose name == its member set, membership-is-the-ACL, no topic"; a private
  channel is "named, topic-scoped, invite-managed." Same storage, same fan-out, two presentations.
- **Artifact-linked channels** carry `linked_ref` and auto-emit `chat.channel.linked` → a `refs.edge.created`
  ("incident #X discussed in channel Y") — a Myelin-specific lever (Phase-1 §2.2).
- **`pinned_canvas`** holds a `knowledge/page` `ArtifactRef` — the canvas is an *embed*, not a Chat editor
  (Sketch 08; [05 §6](./05-hard-problems.md)). Chat owns the *pin/placement*; Knowledge owns the page.
- **Cross-org non-foreclosure (FLOOR):** `membership` (below) is a set of principals that *could* span tenants;
  v1 does not build federation, but the model does not assume single-org-membership-forever (Sketch 08; [05 §7](./05-hard-problems.md)).

### 2.1 Membership (the access-control relation; Sketch 10 §C)

```sql
CREATE TABLE membership (
  tenant          uuid        NOT NULL,
  region          text        NOT NULL,
  conversation_id uuid        NOT NULL,
  principal_id    uuid        NOT NULL,                  -- pseudonymous (human | agent | service)
  role            text        NOT NULL DEFAULT 'member', -- member | admin (manage = invite/archive/settings)
  is_watcher      boolean     NOT NULL DEFAULT true,     -- the Notif read-fanout `watcher` relation (README §5)
  notif_pref      jsonb       NOT NULL DEFAULT '{}',     -- per-channel mute/keyword-alert/DND (delivery is Notif's)
  joined_at       timestamptz NOT NULL,
  PRIMARY KEY (tenant, conversation_id, principal_id)
);
CREATE INDEX membership_by_principal ON membership (tenant, principal_id);  -- "my conversations" (secondary nav)
```

Membership **IS the ACL** for private kinds. A membership write/remove (a) writes the row, (b) emits
`chat.channel.member_added`/`removed` via the outbox, and (c) **projects the ReBAC tuple** to Id via
`write_tuples` (§5 fragment) — all in one transaction so the permission decision can never lag the data. The
`is_watcher` flag is the Notif read-fanout relation Chat owes every watchable type (Notif §8.3).

---

## 3. The message store — the durable per-conversation log (the heart; Sketch 02)

The message store is **append-heavy, time-ordered, per-conversation, grows forever**, and must serve: cheap
appends; ordered range reads ("recent N" + scroll-back + the resync gap read); intrinsic per-conversation total
order; residency in the shard key; crypto-shred-granular erasure of bodies. The hot tier is Postgres-partitioned:

```sql
CREATE TABLE message (
  tenant          uuid        NOT NULL,
  region          text        NOT NULL,                  -- residency in the key (ADR-11; residency-pin lint)
  conversation_id uuid        NOT NULL,
  message_id      ulid        NOT NULL,                  -- K-SORTABLE: intrinsic per-conversation order (Phase-1 §2.3)
  thread_root_id  ulid,                                  -- NULL = top-level; else the thread this reply belongs to
  author          uuid        NOT NULL,                  -- pseudonymous principal_id (human | agent | service)
  author_kind     actor_kind  NOT NULL,                  -- human | agent | service (agent treatment, provenance)
  -- The body IS the PII (not a reference) → per-subject DEK envelope-encryption (GD-4):
  body_inline     bytea       NOT NULL,                  -- markdown-subset STRING (myelin-content), encrypted under pii_key_ref
  body_nodes      bytea       NOT NULL DEFAULT '\x',     -- structured nodes (mention/artifact_ref/embed), encrypted
  pii_key_ref     text        NOT NULL,                  -- kms://<tenant>/<epoch>/subject:<author>  (GD-4; S-8 grammar)
  contains_personal_data boolean NOT NULL DEFAULT true,  -- routes GDPR; almost always true for a chat body
  client_nonce    uuid        NOT NULL,                  -- idempotency nonce (dedup retried sends; Phase-1 §2.3)
  edited_seq      int         NOT NULL DEFAULT 0,        -- edit-as-new-version counter (stable message_id across edits)
  state           msg_state   NOT NULL DEFAULT 'active', -- active | edited | deleted | tombstoned (erased)
  on_behalf_of    uuid,                                  -- agent delegation provenance (envelope actor.on_behalf_of)
  causation_id    ulid,                                  -- the event that caused this (agent posts; provenance popover)
  correlation_id  ulid,                                  -- threads a multi-step agent flow
  created_at      timestamptz NOT NULL,
  PRIMARY KEY (tenant, conversation_id, message_id),
  UNIQUE (tenant, conversation_id, client_nonce)         -- idempotent send: a retried send is a no-op
) PARTITION BY LIST (tenant, region);
-- per-(tenant,region) partitions are time-sub-partitioned (monthly RANGE on message_id's embedded time)
-- so old ranges DETACH to the cold tier; the recent tail stays hot. (pg_partman-style lifecycle.)
CREATE INDEX message_range ON message (tenant, conversation_id, message_id DESC);  -- recent-first + scroll-back
CREATE INDEX message_thread ON message (tenant, conversation_id, thread_root_id, message_id);
```

**Why this shape** (the Sketch 02 resolution):

- **`message_id` is a ULID** (k-sortable; Phase-1 §2.3): per-conversation order is *intrinsic*, never
  wall-clock-derived (clock skew at scale, Phase-1 §2.3); pagination cursors are cheap; the **resync gap read**
  ("everything after cursor X") is a clustering-range read. (ULID leaks creation time — acceptable for chat,
  noted for GDPR minimisation.)
- **Edit = a new version, not a mutation.** `edited_seq` increments; the `message_id` and its `#sub` anchor are
  **stable across edits** so embeds/references don't dangle (Refs §3.5). History is preserved; the latest
  version renders. (Bodies are immutable-append at the storage layer, which is *why* crypto-shred is the erasure
  mechanism — you cannot rewrite an immutable body, you destroy its key.)
- **`body_inline` + `body_nodes` mirror the Knowledge `myelin-content` split** ([Knowledge 01 §2.1](../../knowledge-platform/architecture/01-tech-and-data-model.md)):
  a markdown-subset string for the runs, structured `mention`/`artifact_ref`/`embed` nodes kept **out** of the
  string so reference-extraction is reliable (the `refs.edge.created` producer) and is never a regex-over-prose
  (KN-2/ADR-05). **Both are encrypted under the author's per-subject DEK** because the body *is* the PII.
- **`state = tombstoned`** is the erasure end-state: the *record* survives (conversation structure/order/causality
  intact for others) while the body is crypto-shredded ("delete the content, keep the fact"; Bus §4.8).

### 3.1 The `MessageStore` trait (the hot-engine swap seam; Sketch 02 floor)

```rust
/// The only interface the rest of Chat sees; PG is the v1 impl, Scylla the measured promotion (R-5).
trait MessageStore {
    /// Persist a message AND its chat.message.created outbox row in ONE transaction (BUS-2). Idempotent on client_nonce.
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId>;
    /// Ordered range read: recent-N (open channel) | scroll-back (paginate) | resync-gap ("after cursor X").
    fn range(&self, conv: ConversationId, cursor: RangeCursor, limit: u32) -> Result<Vec<Message>>;
    /// Edit-as-new-version (stable message_id, bumped edited_seq) + the chat.message.edited outbox row.
    fn revise(&self, tx: &mut OutboxTx, msg_id: MessageId, body: ContentAst) -> Result<()>;
    /// Tombstone the record (keep the fact); the body is crypto-shredded separately via the GDPR holder.
    fn tombstone(&self, tx: &mut OutboxTx, msg_id: MessageId, reason: TombstoneReason) -> Result<()>;
    /// The resync correctness backbone: everything in `conv` after `cursor`, gap-free, ordered (Sketch 01).
    fn resync_from(&self, conv: ConversationId, cursor: MessageId) -> Result<Vec<Message>>;
}
```

`append`/`revise`/`tombstone` take the `OutboxTx` so the state change and the event are one transaction — the
trait makes the no-dual-write guarantee structural. **Cold reads** are transparent: `range`/`resync_from` fetch
from the hot PG partition or, for detached ranges, from the cold object segment (fetch + decrypt) behind the
same interface. [02 §2](./02-internals-and-algorithms.md) details the tiering lifecycle and the resync algorithm.

### 3.2 Reactions (lightweight signals; Sketch 10)

```sql
CREATE TABLE reaction (
  tenant uuid NOT NULL, region text NOT NULL,
  conversation_id uuid NOT NULL, message_id ulid NOT NULL,
  principal_id uuid NOT NULL,                            -- who reacted (pseudonymous)
  emoji text NOT NULL,
  created_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, conversation_id, message_id, principal_id, emoji)
);
```

A reaction emits `chat.reaction.added`/`removed`. A ✅ can be an **explicit** approve-action on an agent's
offered action (Sketch 07) — **never** an implicit auto-trigger (CHAT-1; the AG-6 reference gate).

---

## 4. The read-state store (the high-write hot path; Sketch 03)

Read-state is the churny part: per-`(user × conversation)` last-read marker + per-thread read state +
unread/mention counters, written on *every* scroll/open. **Deliberately separate** from the message store and
from Notif's inbox. Valkey holds the hot markers; PG holds the durable record; the flush is batched.

```
# Valkey (hot, ephemeral, never authoritative — STOR-3):
read:<tenant>:<principal>:<conversation>  -> last_read_message_id          (HASH, debounced write)
read:<tenant>:<principal>:thread:<root>   -> last_read_message_id
# unread COUNT is DERIVED, never stored authoritatively: count(message_id > last_read) — a bounded range read, cached.
```

```sql
-- PG: the durable system-of-record (reconstructable; flushed from Valkey on a batched cadence).
CREATE TABLE read_state (
  tenant uuid NOT NULL, region text NOT NULL,
  principal_id uuid NOT NULL,                            -- whose read-state (P's OWN data → erase = delete P's rows)
  conversation_id uuid NOT NULL,
  thread_root_id ulid,                                   -- NULL = channel-level; else per-thread
  last_read_message_id ulid NOT NULL,
  updated_at timestamptz NOT NULL,
  PRIMARY KEY (tenant, principal_id, conversation_id, thread_root_id)
);
```

- **Unread is read-fanout, never write-fanout.** Unread for channel C = `count(messages in C with id >
  my_last_read[C])` — a bounded range read against the per-conversation log, cached. **No per-message-per-member
  write** (the mega-channel never write-amplifies; Sketch 03).
- **Events stay off the durable bus** (ADR-04.5): fine-grained `chat.read_state.*` rides the **firehose**; only
  a coarse `chat.read_state.updated` summary (if any) touches the durable bus (Bus §4.3).
- **Cross-device truth + the Notif link (C-9):** marking read in Chat's "Activity" view marks the *same* Notif
  inbox item read (one store, Notif §1.3); but the per-channel *scroll position* is Chat's read-state. The two
  are **linked at the mention**: scrolling past a mentioned message calls `Notif.mark(item, read)` ([02 §5](./02-internals-and-algorithms.md)).
- `read_state` is a `PersonalDataHolder` — last-read markers are P's own personal data; `erase(P)` deletes P's
  Valkey keys + PG rows.

---

## 5. The ReBAC namespace fragment (Chat's contribution to the one cell schema; Id §5)

Chat declares its fragment to Id (Id §4.9) and **compiles membership to tuples** — no bespoke ACL (ADR-03):

```
definition channel {                                   // = a Conversation of any kind
  relation parent_project: project
  relation member:  user | agent | service             // membership IS the ACL for private kinds
  relation watcher: user | agent                        // the Notif read-fanout relation (README §5 obligation)
  permission read   = member + parent_project->read     // public channels inherit project read
  permission post   = member
  permission manage = member & parent_project->admin    // invite / archive / settings (the consequential mutations)
}
definition message {
  relation parent_channel: channel
  permission view = parent_channel->read
}
```

(verbatim-aligned to Id §5's Chat clause: `channel.read = member + parent_project->read`; `message.view =
parent_channel->read`.) **The per-viewer unfurl is NOT a chat permission** — Chat asks Refs, Refs asks Id
`check(viewer, view, target)` against the *target artifact's* namespace ([02 §4](./02-internals-and-algorithms.md)).
The `watcher` relation is the read-fanout declaration; per-thread watch derives from it.

---

## 6. The unfurl projection cache (the cheap-per-viewer hot path; Sketch 04)

**No durable unfurl snapshot is ever stored** — the message stores only the `artifact_ref` node + the post-time
timestamp (the audit "as-of"), which is what makes erasure free ([02 §6](./02-internals-and-algorithms.md)). The
only unfurl *state* is a short-TTL, bus-invalidated, **viewer-independent** projection cache in Valkey:

```
# Valkey, shared per ArtifactRef (NOT per (ref, viewer)) — the key insight that prevents an N×M blowup:
unfurl:proj:<artifact_ref>  -> { title, state, icon, render_hint, sub_anchor? }   (TTL; busted by *.updated/*.erased)
# the PER-VIEWER decision is a separate Id.check / list_objects (fast, Leopard-prefiltered) — content is
# returned ONLY after the per-viewer check passes (the Refs §4.2 no-leak argument), so one cache entry per ref.
```

The shared cache holds **only** the projection content (which is viewer-independent); the permission gate is a
per-viewer `check`/`list_objects` that runs *before* the cached content is returned. A popular doc embedded in
500 messages resolves its content **once**; the per-viewer cost is a cheap cached `check`. The cache is
bus-invalidated (precise) with TTL as the backstop ([02 §4](./02-internals-and-algorithms.md)).

---

## 7. The stateful-component register + blast-radius (X-4 / SUB-X)

Per X-4, every stateful component named with shard key + blast radius + crypto-shred unit; everything else
stateless and replaceable.

| # | Component | Engine | Holds | Shard key | Blast radius if it dies | Crypto-shred unit |
|---|---|---|---|---|---|---|
| C1 | **Message log + outbox (OLTP)** | Postgres-class (→ Scylla R-5) | bodies (per-subject-DEK), mention/ref nodes, tombstones, `outbox` | `(tenant, region)` + conv + time | one tenant's recent chat; recoverable (outbox drains; reindex rebuilds derived) | **per-subject DEK** for bodies/drafts (GD-4); per-tenant DEK for segments |
| C2 | **Conversation / membership / reaction (OLTP)** | Postgres-class | conversations, membership, reactions, prefs, retention policy | `(tenant, region)` + conv | one tenant's channel metadata | per-tenant DEK; pseudonymous principal ids |
| C3 | **Cold message segments** | S3-compatible object store | sealed archived ranges (content-addressed) | `(tenant, region)` + hash | one tenant's history; OLTP points at it (cross-seam, STOR-4) | per-tenant/per-subject DEK (key destruction reaches cold + backups) |
| C4 | **Read-state** | Valkey (hot) + Postgres (record) | last-read markers + derived counters | `(tenant, region)` + principal | cache loss → slightly-stale markers (benign); PG record authoritative | delete P's keys/rows on erase |
| C5 | **Unfurl projection cache** | Valkey | short-TTL viewer-independent projections | `(tenant, region)` + artifact_ref | derived — re-resolves live; **no durable snapshot** by design | purge entries naming P (re-resolve → tombstone) |
| C6 | **Connection-tier gateway state** | in-memory (Rust; or BEAM) | live sockets, presence, resync cursors | `(tenant, region, conn)` | a node crash → clients reconnect, resync from cursor (**no message loss**) | ephemeral; on erase drop P's sockets/presence/cursors |
| C7 | **Consumer dedup ledgers** | Postgres-class | the consumer template's idempotency (substrate §5) | `(tenant, consumer)` | re-process is idempotent → no loss | inherits C1 |
| C8 | **NATS fan-out subjects** | NATS core | live message/presence frames | subject `fan.<tenant>.<channel>` | a dropped frame → recovered by resync (allowed-to-drop by design) | ephemeral |

The **channel-membership ReBAC tuples are NOT Chat's component** — they live in Id's tuple store; Chat only
*projects* into them (§5). All derived state (C4 counters, C5 cache, the Search index) is rebuildable by
reindex-from-source. C6's whole point is **no message loss on reconnect** — the resume-cursor drill ([07](./07-drills-and-open-questions.md)).

**Hot tables flagged for the `forward-only-migration` lint** (substrate §9): `message`, `read_state`, `reaction`
(the high-write tables) — schema changes use expand→backfill→contract, never a blocking `ALTER`.

Continue to [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) for the algorithms.
