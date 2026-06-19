# Chat — 01 · Technology, Runtime & Data Model

> See [`00-overview.md`](./00-overview.md) for framing, the document split, and the reconciliation deltas. This
> doc carries forward the language/runtime/database choices (with written justification — the TE-21 call) and
> the complete data model, now conformed to the frozen shapes: the **frozen `myelin-content` Chat subset** (X-2),
> the **frozen `#sub` grammar** (X-4), the **frozen cross-language harness shim** (contract 1.7). Schemas are
> illustrative Postgres/Rust; the **shape** is the contract.

---

## 1. The language / runtime / database choice (carried forward + confirmed)

**Decision (unchanged from Phase 4; reconciliation forced no change): Rust for ALL Chat services — including
the connection-tier gateway by default; PostgreSQL-class OLTP as the message system-of-record (behind a
`MessageStore` trait, ScyllaDB the named measured promotion); an S3-compatible object store for the cold-tier
message segments; Valkey for the read-state/presence hot path and the unfurl cache; the cell's NATS-based
firehose tier (now the frozen `subscribe/resume/scope` protocol, contract 3.5) as the live fan-out + presence
backplane. The BEAM/Elixir+Phoenix divergence for the gateway (TE-21) is kept WRITTEN but DISFAVOURED, and is
now bounded by the frozen cross-language harness shim (contract 1.7).** Below is the written justification each
part owes.

### 1.1 The connection tier — the TE-21 call, in writing: **Rust default, BEAM escape hatch (bounded by 1.7)**

The handoff names the real-time connection tier "the most likely Rust divergence (TE-21)." The call, committed
for build/test, is **Rust** — with the **BEAM/Phoenix divergence kept written-and-open but disfavoured**, gated
on whether distributed presence-at-scale and tokio scheduler tail-latency prove tractable in Rust during the
build. The full weighing is in [05 §1](./05-hard-problems.md); the decision and its consequences:

| Concern | Choice | Written justification |
|---|---|---|
| Gateway language/runtime | **Rust** (`tokio` + `tokio-tungstenite` WS + `axum`/`tower` HTTP/WS-upgrade/SSE; tasks-as-lightweight-actors) | ADR-02 default. **No GC pauses + low, predictable memory-per-connection** are a real edge at millions of sockets (the canonical reason the Rust steer exists). **One runtime** — the Message/Read-state/Unfurl services are already Rust, so the glue crates (`serve(AppSpec)`, the resilient client, fail-static, the telemetry signal set, the consumer template, the frozen firehose `subscribe/resume` client) are **linked, not wired over a shim** (contract 1.7 is a no-op in this world — the substrate non-negotiables come for free). |
| Why NOT BEAM/Phoenix (the disfavoured divergence) | **Held open, not chosen** | Phoenix gives PubSub + Presence essentially for free — the single best external fit — but at the cost of (a) a **second runtime** per cell and (b) the **frozen cross-language harness shim** (contract 1.7) in Elixir: three-surface topology, liveness≠readiness, the resilient-client + `Retry-After` honouring, **no fire-and-forget emit** (all emit via the Rust Message Service's outbox), `PersonalDataHolder` over ephemeral state, the shed order, forward-only migrations, and the telemetry survival signals the Phase-5 drills read. Getting those subtly wrong in a second runtime is a correctness/availability risk; the platform **already runs the NATS firehose tier in every cell** (event-bus §4.3), which gives a sovereign, self-hostable PubSub *and* a presence-suitable ephemeral channel **without** a new runtime. The honest read: the owed shim is enough work that the free PubSub/Presence win does not clearly clear it. |
| Prior art keeping the divergence honest | Discord: Rust for hot data-services (read-states), Elixir for the real-time gateway/guild fan-out | Discord eng *"How Discord Scaled Elixir to 5M Concurrent Users"* (2017) + the 2020 Go→Rust read-states switch (GC pauses were the named enemy) — this **exact split** is real prior art, which is why the BEAM hatch stays written. The Phoenix "2M connections on one box" benchmark (2015) and the WhatsApp/Ejabberd lineage are the BEAM case; Cloudflare's Rust edge proxies + `tokio` work-stealing are the Rust case. |
| The wire contract + the frozen shim either way | **The gateway speaks the Rust `EventEnvelope` on the wire, implements `PersonalDataHolder`, and satisfies contract 1.7** | Mandatory regardless of language. The gateway is **stateless** — live sockets + in-memory presence + resume cursors only, no durable store, no outbox — so the cross-language shim is its *thinnest* possible form, and the divergence is a **gateway-process swap**, not a subsystem rewrite. **The frozen 1.7 is what makes the BEAM hatch admissible at all.** |

**EU-deployable / self-hostable: confirmed.** Every component is self-hostable with no US-controlled SaaS:
PostgreSQL, an S3-compatible store (MinIO/Ceph/Garage), NATS (core + JetStream, in-cell), Valkey (the
BSD-licensed Redis fork the platform already runs). A managed realtime SaaS (Ably/Pusher/PubNub) is **rejected
outright** — non-sovereign, not self-hostable (VISION §1).

**Glue-contract implementability across the boundary: confirmed.** In the default (all-Rust) world the glue
contracts are linked types. If the gateway diverges to BEAM, the contracts it must *originate* are small (the
envelope it forwards, the `ArtifactRef` strings it carries, the `subscribe/resume` cursors); everything
correctness-critical (persist, emit, authorize, unfurl) is an **RPC call into the Rust services**, which already
speak the contracts. The frozen shim (contract 1.7) is the precise specification of that boundary — which is
*why* the divergence is admissible.

### 1.2 The message store — PostgreSQL-partitioned hot tier, behind a `MessageStore` trait

**Decision (carried forward): a Postgres-partitioned hot tier (partitioned by `(tenant, region)`, time
sub-partitioned) + object-store cold tiering as item-zero, behind a `MessageStore` trait; ScyllaDB is the named
measured promotion (R-5).** The *written why* (full weighing [05 §2](./05-hard-problems.md)):

- **Outbox transaction coherence is decisive.** A message's durable persist and its `chat.message.created` event
  **must** commit together (BUS-2; the dual-write hazard is the #1 silent-data-loss source). PG gives this
  natively in one transaction; a separate wide-column store forces an outbox-in-PG + message-in-Scylla **dual
  write** — re-introducing exactly the seam the platform rejected. The message row and its outbox row share the
  PG WAL.
- **GDPR is chat's dominant correctness axis, not raw throughput.** Per-subject crypto-shred (contract 11.4),
  per-subject mention neutralisation, per-channel retention purge, and DSR export are **standard Storage patterns
  on PG** and **bespoke on Scylla** (whose tombstone-GC interacts badly with the delete-heavy erasure workload —
  a known anti-pattern). Chat is "the stress test for the holder spine"; building it where the holder patterns are
  native lowers the risk that erasure is subtly wrong (the worst failure class here).
- **The cell bounds the scale.** A cell is one region's tenants (ADR-11), not the planet; the "single PG melts"
  intuition is calibrated to a global single DB, which Myelin never has. Realistic per-cell volume is plausibly
  within partitioned-PG + cold-tiering reach — **and the mandate is measure-before-shard (ADR-10).**
- **The trait makes the promotion a swap.** `MessageStore { append, range, tombstone, resync_from }` — the cold
  tier and the trait are identical under either hot engine, so promoting to Scylla is a hot-tier swap, not a
  redesign (the same escape-hatch-behind-a-trait philosophy as Bus JetStream→Kafka, Workflow PG→Temporal).

### 1.3 Read-state, presence, unfurl cache, content model, live backplane

| Concern | Choice | Justification |
|---|---|---|
| Read-state hot markers + counters | **Valkey** + PG durable record | A separate fast KV, batched eventually-consistent writes. **Valkey is NEVER the source of truth** — the PG record is, reconstructable; a cache loss makes a marker at-worst slightly stale (you re-see a few read messages — benign, bounded). [02 §3](./02-internals-and-algorithms.md). |
| Unfurl shared projection cache | **Valkey** (short-TTL, bus-invalidated) | Viewer-independent projection content cached once per `ArtifactRef`; the per-viewer permission gate is a separate `list_objects`/`check` lowering the **frozen `SetExpr`**. [02 §4](./02-internals-and-algorithms.md). |
| Live fan-out + presence/typing backplane | **The frozen firehose resume-cursor tier** (contract 3.5; NATS-based) | `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`; subject-per-channel routing (`scope = channel:<id>`, bounded never `*`); EU-sovereign, self-hostable, already deployed; the durable log is the truth, so a missed frame is recovered by `resume`. Reusing the one protocol avoids a second ephemeral transport (operational minimalism). [02 §1/§2](./02-internals-and-algorithms.md). |
| Cold-tier message segments | **S3-compatible object store** (content-addressed, BLAKE3), behind `BlobStore` | An archived conversation range seals to an encrypted segment, still range-readable (a cold read = segment fetch + decrypt), still crypto-shreddable (destroy the per-tenant/per-subject DEK, contract 11.4). |
| Message content model | **`myelin-content`** shared crate (Knowledge-led, ADR-05), the **frozen Chat subset** (X-2) | Chat **consumes** the block/inline AST subset (§1.4 below); it does **not** re-implement an editor. One editor render path (`render(parse(md)) === md`); the composer compiles the same Rust `myelin-content` core to **WASM** as Knowledge (contract 13.1 — share the implementation, not the spec). [04](./04-views-cli-and-api.md). |
| HITL durable wait | **`myelin-flow`** (`DurableExecutor`) | Chat does not reinvent durable waits/timers; it posts the approval `signal` with the **frozen per-effect `idem_key`** ([02 §5](./02-internals-and-algorithms.md)). |

### 1.4 The frozen `myelin-content` Chat subset (X-2 / contract 13.1)

Chat **consumes a strict subset** of the canonical `myelin-content` taxonomy — it adds no node type. The frozen
Chat subset (recon §X-2, "Consumed subsets"):

```
Block subset (Chat):
  paragraph, heading(1..3), bullet_list, ordered_list, task_list,
  blockquote, code_block, callout, table, divider, image
  EXCLUDES: db_view, sync_block, toggle   (no in-message databases / transclusion / collapsible toggles)

Inline (the markdown-subset string + the three structured nodes — IDENTICAL across Chat/Issues/Knowledge):
  **bold**, *italic*, `code`, ~~strike~~, [text](url)
  + mention(Principal)        // @alice — renders display name per-viewer (REF-3); the write-fanout producer
  + artifact_ref(ArtifactRef) // a typed reference — the PRODUCER of refs.edge.created (contract 5.4)
  + embed(ArtifactRef)        // an inline unfurl/transclusion request
```

- **The three inline ref nodes are the frozen, identical structured nodes** (contract 13.1). They are stored
  **out** of the markdown string so reference-extraction is reliable (never a regex over prose) — they are the
  producers of `refs.edge.created` uniformly with Issues and Knowledge.
- **No collaborative-edit engine** (X-2): chat messages are small and mostly immutable-after-send, so Chat uses
  the AST with **per-message CAS on edit** (the `edited_seq` counter, §3), not the Knowledge CRDT path.
- **The round-trip invariant** `render(parse(md)) === md` holds over the subset; the composer reuses the same
  WASM-compiled Rust core as Knowledge (no second editor; EI-05 §2).

---

## 2. The conversation model — one entity, many kinds (ADR; Sketch 08)

One `Conversation` entity with a `kind` discriminator and a membership strategy — **not** five tables — so DMs,
group-DMs, channels, artifact-linked channels, and announcements share the *same* read/write/fan-out/erasure
machinery (avoid duplicating the hardest logic five times).

```sql
CREATE TYPE conversation_kind AS ENUM (
  'channel_public', 'channel_private', 'dm', 'group_dm', 'artifact_linked', 'announcement'
);

CREATE TABLE conversation (
  tenant          uuid              NOT NULL,
  region          text              NOT NULL,            -- residency-pinned (ADR-11); == cell.region (residency-pin lint)
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
  ("incident #X discussed in channel Y") via the structured `artifact_ref` node path — a Myelin-specific lever.
- **`pinned_canvas`** holds a `knowledge/page` `ArtifactRef` — the canvas is an *embed*, not a Chat editor
  (Sketch 08; [05 §6](./05-hard-problems.md)). Chat owns the *pin/placement*; Knowledge owns the page.
- **Cross-org non-foreclosure (FLOOR):** `membership` (below) is a set of principals that *could* span tenants;
  v1 does not build federation, but the model does not assume single-org-membership-forever, and cross-org rides
  the **frozen cross-cell pointer bridge** (contract 12.6, OQ-I) when it ships ([05 §7](./05-hard-problems.md)).

### 2.1 Membership (the access-control relation; Sketch 10 §C)

```sql
CREATE TABLE membership (
  tenant          uuid        NOT NULL,
  region          text        NOT NULL,
  conversation_id uuid        NOT NULL,
  principal_id    uuid        NOT NULL,                  -- pseudonymous (human | agent | service)
  role            text        NOT NULL DEFAULT 'member', -- member | admin (manage = invite/archive/settings)
  is_watcher      boolean     NOT NULL DEFAULT true,     -- the Notif read-fanout `watcher` relation (contract 4.9)
  notif_pref      jsonb       NOT NULL DEFAULT '{}',     -- per-channel mute/keyword-alert/DND (delivery is Notif's)
  joined_at       timestamptz NOT NULL,
  PRIMARY KEY (tenant, conversation_id, principal_id)
);
CREATE INDEX membership_by_principal ON membership (tenant, principal_id);  -- "my conversations" (secondary nav)
```

Membership **IS the ACL** for private kinds. A membership write/remove (a) writes the row, (b) emits
`chat.channel.member_added`/`removed` via the outbox, and (c) **projects the ReBAC tuple** to Id via
`write_tuples([Δtuple], precondition) → zookie` (§5 fragment) — **all in one transaction**, and **stamps the
returned zookie** on the conversation (the new-enemy guard, contract 4.6/4.10) so the permission decision can
never lag the data. The `is_watcher` flag is the Notif read-fanout relation Chat owes every watchable type
(contract 4.9), resolved at read-fanout via `list_subjects(channel, watcher)` against the same authz reverse
index that serves `list_objects` (performant at 50k-member density, contract 4.4).

---

## 3. The message store — the durable per-conversation log (the heart; Sketch 02)

The message store is **append-heavy, time-ordered, per-conversation, grows forever**, and must serve: cheap
appends; ordered range reads ("recent N" + scroll-back + the resume gap read); intrinsic per-conversation total
order; residency in the shard key; crypto-shred-granular erasure of bodies. The hot tier is Postgres-partitioned:

```sql
CREATE TABLE message (
  tenant          uuid        NOT NULL,
  region          text        NOT NULL,                  -- residency in the key (ADR-11; residency-pin lint)
  conversation_id uuid        NOT NULL,
  message_id      ulid        NOT NULL,                  -- K-SORTABLE: intrinsic per-conversation order
  thread_root_id  ulid,                                  -- NULL = top-level; else the thread this reply belongs to
  author          uuid        NOT NULL,                  -- pseudonymous principal_id (human | agent | service)
  author_kind     actor_kind  NOT NULL,                  -- human | agent | service (agent treatment, provenance)
  -- The body IS the PII (not a reference) → per-subject DEK envelope-encryption (contract 11.4 / GD-4):
  body_inline     bytea       NOT NULL,                  -- markdown-subset STRING (myelin-content Chat subset), encrypted
  body_nodes      bytea       NOT NULL DEFAULT '\x',     -- structured nodes (mention/artifact_ref/embed), encrypted
  pii_key_ref     text        NOT NULL,                  -- kms://<tenant>/<dek-epoch>/subject:<author>  (frozen units)
  contains_personal_data boolean NOT NULL DEFAULT true,  -- routes GDPR; almost always true for a chat body
  client_nonce    uuid        NOT NULL,                  -- idempotency nonce (dedup retried sends)
  edited_seq      int         NOT NULL DEFAULT 0,        -- per-message CAS-on-edit counter (stable message_id across edits)
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

**Why this shape** (the Sketch 02 resolution, conformed):

- **`message_id` is a ULID** (k-sortable): per-conversation order is *intrinsic*, never wall-clock-derived
  (clock skew at scale); pagination cursors are cheap; the **resume gap read** (`resync_from` = "everything
  after cursor X") is a clustering-range read. (ULID leaks creation time — acceptable for chat, noted for GDPR
  minimisation.) The `message_id` is the stable opaque id behind the frozen `#sub` anchor `message-<message_id>`.
- **Edit = a new version, not a mutation (per-message CAS, X-2).** `edited_seq` increments under CAS; the
  `message_id` and its `#sub` anchor are **stable across edits** so embeds/references don't dangle (contract 5.7).
  History is preserved; the latest version renders. (Bodies are immutable-append at the storage layer, which is
  *why* crypto-shred is the erasure mechanism — you cannot rewrite an immutable body, you destroy its key.)
- **`body_inline` + `body_nodes` mirror the `myelin-content` split** (X-2): a markdown-subset string for the
  runs, the three structured `mention`/`artifact_ref`/`embed` nodes kept **out** of the string so
  reference-extraction is reliable (the `refs.edge.created` producer, contract 5.4) and is never a
  regex-over-prose. **Both are encrypted under the author's per-subject DEK** because the body *is* the PII.
- **`state = tombstoned`** is the erasure end-state: the *record* survives (conversation structure/order/causality
  intact for others) while the body is crypto-shredded ("delete the content, keep the fact"; the 4-step ladder's
  `erased` outcome, contract 5.7).

### 3.1 The `MessageStore` trait (the hot-engine swap seam; Sketch 02 floor)

```rust
/// The only interface the rest of Chat sees; PG is the v1 impl, Scylla the measured promotion (R-5).
trait MessageStore {
    /// Persist a message AND its chat.message.created outbox row in ONE transaction (BUS-2). Idempotent on client_nonce.
    fn append(&self, tx: &mut OutboxTx, msg: NewMessage) -> Result<MessageId>;
    /// Ordered range read: recent-N (open channel) | scroll-back (paginate) | resume-gap ("after cursor X").
    fn range(&self, conv: ConversationId, cursor: RangeCursor, limit: u32) -> Result<Vec<Message>>;
    /// Edit-as-new-version under CAS (stable message_id, bumped edited_seq) + the chat.message.edited outbox row.
    fn revise(&self, tx: &mut OutboxTx, msg_id: MessageId, body: ContentAst, expect_seq: i32) -> Result<()>;
    /// Tombstone the record (keep the fact); the body is crypto-shredded separately via the GDPR holder.
    fn tombstone(&self, tx: &mut OutboxTx, msg_id: MessageId, reason: TombstoneReason) -> Result<()>;
    /// The resume correctness backbone: everything in `conv` after `cursor`, gap-free, ordered (contract 3.5).
    fn resync_from(&self, conv: ConversationId, cursor: MessageId) -> Result<Vec<Message>>;
}
```

`append`/`revise`/`tombstone` take the `OutboxTx` so the state change and the event are one transaction — the
trait makes the no-dual-write guarantee structural. `revise` takes `expect_seq` for the per-message CAS (X-2).
**Cold reads** are transparent: `range`/`resync_from` fetch from the hot PG partition or, for detached ranges,
from the cold object segment (fetch + decrypt) behind the same interface. `resync_from` is what the gateway's
frozen `resume(stream, scope, last_seq)` (contract 3.5) backfills from when the firehose retention window is
exceeded (`resync_required`). [02 §2](./02-internals-and-algorithms.md) details the tiering lifecycle and the
resume algorithm.

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
offered action (Sketch 07) — **never** an implicit auto-trigger (CHAT-1; the explicit-first dispatch gate).

---

## 4. The read-state store (the high-write hot path; Sketch 03)

Read-state is the churny part: per-`(user × conversation)` last-read marker + per-thread read state +
unread/mention counters, written on *every* scroll/open. **Deliberately separate** from the message store and
from Notif's inbox. Valkey holds the hot markers; PG holds the durable record; the flush is batched.

```
# Valkey (hot, ephemeral, never authoritative):
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
  a coarse `chat.read_state.updated` summary touches the durable bus (event-bus §4.3).
- **Cross-device truth + the Notif link (C-9):** marking read in Chat's "Activity" view marks the *same* Notif
  inbox item read (one store, contract 7.1); but the per-channel *scroll position* is Chat's read-state. The two
  are **linked at the mention**: scrolling past a mentioned message calls `Notif.mark(item, read)` ([02 §5](./02-internals-and-algorithms.md)).
- `read_state` is a `PersonalDataHolder` — last-read markers are P's own personal data; `erase(P)` deletes P's
  Valkey keys + PG rows.

---

## 5. The ReBAC namespace fragment (Chat's contribution to the one cell schema; contract 4.9)

Chat declares its **frozen fragment** to Id (contract 4.9: "Chat (`channel.read = member + parent_project->read`)")
and **compiles membership to tuples** — no bespoke ACL (ADR-03):

```
definition channel {                                   // = a Conversation of any kind
  relation parent_project: project
  relation member:  user | agent | service             // membership IS the ACL for private kinds
  relation watcher: user | agent                        // the Notif read-fanout relation (contract 4.9 obligation)
  permission read   = member + parent_project->read     // public channels inherit project read
  permission post   = member
  permission manage = member & parent_project->admin    // invite / archive / settings (the consequential mutations)
}
definition message {
  relation parent_channel: channel
  permission view = parent_channel->read
}
```

(verbatim-aligned to the frozen contract 4.9 Chat clause.) **The per-viewer unfurl is NOT a chat permission** —
Chat asks Refs, Refs asks Id `check(viewer, view, target)` against the *target artifact's* namespace ([02 §4](./02-internals-and-algorithms.md)).
The `watcher` relation is the read-fanout declaration; per-thread watch derives from it. Field/transition ABAC
caveats (contract 4.2 `CaveatContext`) are **not on Chat's hot path** — Chat's row-level visibility is the plain
`channel.read`/`message.view`; the caveat exists for Issues/KN field-hiding, which Chat does not need.

---

## 6. The unfurl projection cache (the cheap-per-viewer hot path; Sketch 04)

**No durable unfurl snapshot is ever stored** — the message stores only the `artifact_ref` node + the post-time
timestamp (the audit "as-of"), which is what makes erasure free ([02 §6](./02-internals-and-algorithms.md)). The
only unfurl *state* is a short-TTL, bus-invalidated, **viewer-independent** projection cache in Valkey:

```
# Valkey, shared per ArtifactRef (NOT per (ref, viewer)) — the key insight that prevents an N×M blowup:
unfurl:proj:<artifact_ref>  -> { title, state, icon, render_hint, sub_anchor? }   (TTL; busted by *.updated/*.erased)
# the PER-VIEWER decision is a separate Id.check / list_objects (lowering the frozen SetExpr); content is
# returned ONLY after the per-viewer check passes (the contract 5.2 no-leak argument), so one cache entry per ref.
```

The shared cache holds **only** the projection content (which is viewer-independent); the permission gate is a
per-viewer `check`/`list_objects` that runs *before* the cached content is returned. A popular doc embedded in
500 messages resolves its content **once**; the per-viewer cost is a cheap cached `check`. The cache is
bus-invalidated (precise) with TTL as the backstop ([02 §4](./02-internals-and-algorithms.md)). A `sub_anchor`
on the projection resolves through the frozen 4-step tombstone ladder (contract 5.7) — a moved/outdated/gone sub
degrades gracefully, never dangles.

---

## 7. The stateful-component register + blast-radius (SUB-X)

Every stateful component named with shard key + blast radius + crypto-shred unit; everything else stateless and
replaceable.

| # | Component | Engine | Holds | Shard key | Blast radius if it dies | Crypto-shred unit |
|---|---|---|---|---|---|---|
| C1 | **Message log + outbox (OLTP)** | Postgres-class (→ Scylla R-5) | bodies (per-subject-DEK), mention/ref nodes, tombstones, `outbox` | `(tenant, region)` + conv + time | one tenant's recent chat; recoverable (outbox drains; reindex rebuilds derived) | **per-subject DEK** for bodies/drafts (contract 11.4); per-tenant DEK for segments |
| C2 | **Conversation / membership / reaction (OLTP)** | Postgres-class | conversations, membership, reactions, prefs, retention policy | `(tenant, region)` + conv | one tenant's channel metadata | per-tenant DEK; pseudonymous principal ids |
| C3 | **Cold message segments** | S3-compatible object store | sealed archived ranges (content-addressed) | `(tenant, region)` + hash | one tenant's history; OLTP points at it (cross-seam) | per-tenant/per-subject DEK (key destruction reaches cold + backups) |
| C4 | **Read-state** | Valkey (hot) + Postgres (record) | last-read markers + derived counters | `(tenant, region)` + principal | cache loss → slightly-stale markers (benign); PG record authoritative | delete P's keys/rows on erase |
| C5 | **Unfurl projection cache** | Valkey | short-TTL viewer-independent projections | `(tenant, region)` + artifact_ref | derived — re-resolves live; **no durable snapshot** by design | purge entries naming P (re-resolve → tombstone) |
| C6 | **Connection-tier gateway state** | in-memory (Rust; or BEAM) | live sockets, presence, resume cursors | `(tenant, region, conn)` | a node crash → clients reconnect, `resume` from cursor (**no message loss**) | ephemeral; on erase drop P's sockets/presence/cursors |
| C7 | **Consumer dedup ledgers** | Postgres-class | the consumer template's idempotency (`consumer_dedup`, contract 2.5) | `(tenant, consumer)` | re-process is idempotent → no loss | inherits C1 |
| C8 | **Firehose subjects (resume-cursor tier)** | NATS-based (contract 3.5) | live message/presence frames per `(stream, scope)` | subject `fan.<tenant>.<channel>` | a dropped frame → recovered by `resume` (allowed-to-drop by design) | ephemeral |

The **channel-membership ReBAC tuples are NOT Chat's component** — they live in Id's tuple store; Chat only
*projects* into them (§5). All derived state (C4 counters, C5 cache, the Search index) is rebuildable by
reindex-from-source. C6's whole point is **no message loss on reconnect** — the resume-cursor drill ([07](./07-drills-and-open-questions.md)).

**Hot tables flagged for the `forward-only-migration` lint** (contract 1.5): `message`, `read_state`, `reaction`
(the high-write tables) — schema changes use expand→backfill→contract, never a blocking `ALTER`.

Continue to [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) for the algorithms.
