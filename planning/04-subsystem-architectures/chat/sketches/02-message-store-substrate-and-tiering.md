# Sketch 02 — The durable message store: substrate + hot/cold tiering

> Exploration note. Weighs the durable per-conversation message log (Phase-2 Chat §9.2, Phase-1 §5.3).
> The shard key must encode residency (ADR-11); the store is the platform's most PII-dense
> `PersonalDataHolder` (Phase-1 §7).

---

## The problem, precisely

The message store is **append-heavy, time-ordered, per-conversation, grows forever**, and must serve:
1. **Cheap appends** at world write-volume (every message, every edit-as-new-version, every tombstone).
2. **Ordered range reads** — "recent N" (open a channel) and "scroll back" (paginate history), plus the
   **resync gap read** ("everything after cursor X") that Sketch 01's correctness backbone depends on.
3. **Per-conversation total order** that is *intrinsic* (k-sortable message id), never wall-clock-derived
   (Phase-1 §2.3 — clock skew at scale).
4. **Residency in the shard key** (ADR-11): a German tenant's messages physically in the EU/its cell.
5. **Crypto-shred-granular erasure** for message bodies + per-subject mention neutralisation (Phase-1
   §7; GD-4 per-subject DEK rule, Storage §5.1).

Every store also inherits the substrate non-negotiables (00 §0.1): `(tenant, region)` first column,
RLS, per-tenant envelope-encryption, holder auto-registration, no cross-tenant query path.

---

## Candidate substrates

### Candidate A — Wide-column (Cassandra/ScyllaDB-class), the Phase-2 directional candidate

- **Shape.** Partition key = `(tenant, conversation_id)`; clustering key = `message_id` (a k-sortable
  ULID/Snowflake, descending for recent-first reads). A conversation is one wide partition; appends are
  to the tail; range reads are a clustering-key slice.
- **For.** This is the canonical chat-log shape (Discord's message store is Cassandra→ScyllaDB;
  Discord eng "How Discord Stores Trillions of Messages", 2023). Linear write scaling, tunable
  consistency, TTL-per-row (useful for retention), and natural time-bucketing of hot partitions. Handles
  the "infinite per-conversation growth" case that melts a single PG table.
- **Against.** A **third stateful engine per cell** (beside PG + NATS + object store) — the exact
  operational tax EI-02 §8 warns against, and it must be made residency-pinned, crypto-shred-capable,
  backup-verified, and self-hostable *per cell including the smallest self-host cell* (ADR-11). ScyllaDB
  is the C++ reimplementation (no JVM, better for the self-host footprint) but is still a new engine.
  Wide-column crypto-shred per-subject is awkward (encryption is per-row-value, doable but bespoke).
  Cassandra's "tombstone" GC interacts badly with high-delete workloads — and GDPR erasure is a delete
  workload (a known Cassandra anti-pattern: tombstone accumulation degrades reads).
- **Hot/cold.** Recent partitions in Scylla; archived conversations tiered to the object store by range.

### Candidate B — Postgres-partitioned (the operational-minimalism path)

- **Shape.** One `message` table **partitioned by `(tenant, region)`** (declarative list/hash
  partitioning) and sub-partitioned by time (monthly range) so old partitions detach to cold storage.
  `PRIMARY KEY (tenant, conversation_id, message_id)`; the per-conversation slice is a BRIN/btree index
  range. The outbox lives in the *same* PG (the cross-seam restore anchor, STOR-4) — appends and the
  `chat.message.created` outbox row **commit in one transaction** (BUS-2), which a separate wide-column
  store *cannot* give without a dual-write (the exact hazard the durable-workflow doc rejected Temporal
  for, Workflow §2.3).
- **For.** **One engine** (PG is already the platform OLTP tier, Storage §3.1); native transactional
  outbox coherence; per-tenant envelope-encryption + per-subject crypto-shred are the *standard* Storage
  patterns (no bespoke work); forward-only online partition management (STOR-2); restore-verification +
  cross-seam consistency (STOR-4) come for free because the message rows and the outbox share the WAL.
  Reindex-from-source (the derived stores) all assume PG already.
- **Against.** PG is not *natively* a trillion-row append log; a single mega-channel partition can grow
  unboundedly. Mitigations: time-range sub-partitions with automatic detach-to-cold (the hot/cold tier
  is *mandatory*, not optional, here); `pg_partman`-style partition lifecycle; the hottest mega-channels
  may need their own partition. At Discord's scale PG would not suffice; at Myelin's *realistic per-cell*
  scale (a cell is one region's tenants, ADR-11; not the whole planet in one DB) it plausibly does —
  **but this must be measured, not assumed** (ADR-10 anti-premature-shard).

### Candidate C — A purpose-built log on the object store + a PG index

- Append message frames to per-conversation object-store segments (content-addressed via `BlobStore`,
  STOR-1), with a PG index of `(conversation, message_id → segment+offset)` for range reads.
- **For.** Cheapest infinite growth; residency is the object-store bucket's region.
- **Against.** We'd be **building a log store** (the EI-04 §3 "don't reinvent" warning); range reads
  need a custom reader; crypto-shred over immutable object segments is key-destruction only (fine, but
  per-message granularity inside a segment is awkward). High complexity for v1. **Rejected for v1**, kept
  as the cold-tier mechanism (segments *are* how cold archival works under A or B).

---

## The decision shape: substrate + the mandatory tier

The honest tension is **operational minimalism (one engine, native outbox coherence) vs. proven
infinite-scale chat-log shape (wide-column)**.

**Leaning — start on Postgres-partitioned (Candidate B) with object-store cold tiering as item-zero,
and name wide-column (ScyllaDB) as the measured promotion trigger.** Rationale:

1. **Outbox transaction coherence is decisive.** A chat message's durable persist and its
   `chat.message.created` event *must* commit together (BUS-2; the dual-write hazard is the #1
   silent-data-loss source, substrate D-1). PG gives this natively in one transaction; a separate
   wide-column store forces an outbox-in-PG + message-in-Scylla **dual write** that re-introduces the
   exact seam Workflow §2.3 rejected Temporal for. Keeping the message *and* its outbox row in one PG
   transaction is worth a lot.
2. **GDPR is the dominant correctness axis for chat, not raw throughput.** Per-subject crypto-shred,
   per-subject mention neutralisation, per-channel retention purge across derived stores, and DSR
   export are *standard Storage patterns on PG* and *bespoke on Scylla*. Chat is "the stress test for
   the holder spine" (Phase-2 Chat §8.5) — building that on the engine where the holder patterns are
   native lowers the risk that erasure is subtly wrong (the worst failure class here).
3. **The cell bounds the scale.** A cell is one region's tenants (ADR-11), not the planet; the "single
   PG melts" intuition is calibrated to a global single DB, which we never have. The realistic per-cell
   message volume is plausibly within partitioned-PG + cold-tiering reach — **and the platform mandate
   is measure-before-shard (ADR-10)**, so we do not pre-pay for Scylla.
4. **The tier is not optional.** Whatever the hot substrate, **hot/cold tiering to the object store is
   item-zero** (Phase-1 §5.3 "strongly suggest tiering given infinite growth"): recent messages hot,
   archived conversations/old ranges sealed to content-addressed object segments (STOR-1), still
   range-readable (a cold read is a segment fetch + decrypt), still crypto-shreddable (destroy the
   per-tenant/per-subject DEK).

**Named promotion trigger (R-5):** a *measured* per-cell write/partition volume at which partitioned-PG
serves at degraded append/range latency → promote the hot tier to **ScyllaDB** behind a thin
`MessageStore` trait (`append / range / tombstone / resync_from`), the same escape-hatch-behind-a-trait
philosophy as the Bus (JetStream→Kafka) and Workflow (PG→Temporal). The cold tier and the trait are the
*same* under either hot engine, so the promotion is a hot-tier swap, not a redesign.

**Named floor (honesty):** "partitioned-PG hot tier + object-store cold tier" is the v1 floor; the
follow-on is "ScyllaDB hot tier on measured volume." This is stated as a floor, not as done.

---

## The shard/partition key (residency, decided directionally)

```
partition = (tenant, region)          -- residency in the key (ADR-11; Phase-1 §5.3)
clustering / pk tail = (conversation_id, message_id)   -- per-conversation order
```

- A tenant's messages live in its **cell** (region-pinned); there is no cross-region message query path
  (ADR-11). A global user talking in an EU tenant's channel connects to an edge gateway near them but the
  **write routes home to the tenant's cell** (Sketch 01 / the multi-region tension, Phase-1 §5.9 — a
  named floor; v1 is single-home-cell).
- `message_id` is a **ULID/Snowflake k-sortable id** (Phase-1 §2.3): order is intrinsic, pagination
  cursors are cheap, resync ("after id X") is a clustering-range read. (ULID leaks creation time — fine
  for chat, noted for GDPR minimisation per Phase-1 §2.3.)

---

## Read-state is a *separate* store (decided)

Read-state / unread-counts is **not** in the message store. It is a high-write, eventually-consistent,
per-`(user × conversation)` + per-thread last-read marker (Phase-1 §2.5/§5.6 — "a bad design here melts
the database"). Sketch 03 treats it. The message store stays append-only and read-mostly-by-range; the
churny read-state hot path lives in a fast KV with batched writes.
