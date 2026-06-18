# Phase 3 — Event Bus + Trigger/Automation Engine (`myelin-events`)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §3/§4/§5/§6, [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
> §2/§6, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5.2/§5.3.
> Spine: ADR-04 (delivery model + firehose split), ADR-19 (four primitives), ADR-07 (matcher↔AST),
> ADR-13.2 (envelope), ADR-08.5 (one trigger engine), ADR-16 (backpressure), ADR-11 (cells),
> ADR-12 (GDPR holders). Directives: BUS-1…BUS-6, X-1…X-5. Resolves: **TE-10** (event taxonomy /
> dotted names), **AG-7** (`EventMatcher` predicate language), the **C-3** `ArtifactRef`
> subsystem/type token table drift (consistency-review §C-3).
>
> **This is a FOUNDATIONAL doc.** Refs, Search, Notif, Agent Fabric, OLAP, GDPR/Audit all read the
> contracts in §4 and §5. Those contracts are written to be **stable**; everything else is internal.

---

## 0. Reading map & status

- **§1** — purpose & responsibilities; the four-primitive model (ADR-19).
- **§2** — the durable streaming log: transport choice, partitioning, ordering.
- **§3** — the data model: the common envelope schema, the outbox, the Signal/Trigger/Automation stores.
- **§4** — the algorithms: outbox relay, idempotent consumers, Signal curation, the Trigger state
  machine, the `EventMatcher`/AST, the firehose split, retention/crypto-shred, reindex-from-source.
- **§5** — the contracts/APIs exposed to other shared systems (the glue). **Stable.**
- **§6** — the canonical event taxonomy + the `ArtifactRef` token table (resolves TE-10 + C-3).
- **§7** — scaling/sharding in the cell topology.
- **§8** — failure modes + the drills owed.
- **§9** — cited prior art.
- **§10** — open questions for Phase 4.

**Floors named up front** (VISION §3 / EI-04 §4): single-cell event propagation is built; **cross-cell
fan-out for multi-cell tenants is designed-not-built** (§7.4, floor → P4 control plane). The
column-store/time-series seam for the highest-volume durable streams is **specified-not-built** (BUS-6,
§7.5; promotion trigger = measured volume). The transport selection is **decided (JetStream-class) with
a written escape hatch**, not foreclosed.

---

## 1. Purpose, responsibilities, and the four-primitive model

### 1.1 What `myelin-events` owns

The **canonical, versioned, ordered, durable stream of domain events** — Myelin's nervous system
(EI-02 §3) — and the **trigger/automation engine** that sits on it (ADR-08.5: automations and agents
are *one* engine with different action handlers). Concretely it owns:

1. The **common envelope** (ADR-13.2) and its schema evolution (§3.1).
2. The **transactional outbox + relay** — the *only* sanctioned emit path (BUS-2; EI-02 §4).
3. The **durable streaming log** with per-aggregate ordering, at-least-once delivery, durable pull
   consumers + consumer groups (BUS-1; EI-02 §3).
4. The **firehose split** — high-volume ephemeral streams ride a *separate* transport; the durable bus
   carries only pointer events (ADR-04.5).
5. The **four reactive primitives** (ADR-19) and their engines (§1.2, §4.4–4.7).
6. The **`EventMatcher`** predicate language, sharing the query AST (ADR-07; AG-7; §4.5).
7. The **canonical event taxonomy** and the `ArtifactRef` token table (§6).
8. **Retention + crypto-shred + tombstones** on the log (ADR-04.4; ADR-12; §4.8).
9. **Reindex-from-source re-emit** as a first-class resilience primitive (BUS/REF-4/SEARCH-1; §4.9).

It does **not** own: the durable-workflow engine (ADR-09 — automations *invoke* it; it is a separate
substrate); permission decisions (Id; the bus carries `actor`/`visibility`, Id decides); the typed
relation tables (Refs/subsystems own those, REF-1); the dispatch/agent runtime (Agent Fabric — the bus
delivers to its inbox). The reactive/dispatch tier is **separately reviewed** (D7; §4.7).

### 1.2 The four primitives (ADR-19 / EI-03 §2) — don't collapse them

Each primitive has a **different author, lifetime, store, and failure mode**. This is the single most
load-bearing vocabulary in the doc.

| Primitive | What it is | Author | Lifetime | Cardinality | Store |
|---|---|---|---|---|---|
| **Event** | A fact: "X happened." Every state change. | The producing subsystem (via outbox) | Immutable, append-only, bounded retention | one per state change | The durable log (§2) |
| **Signal** | A **curated, deduplicated, severity-ranked subset** of events actors should react to (errors, alerts, agent proposals, SLA breaches). | A Signal rule (platform/admin-defined) | Derived; lives in a Signal stream + inbox row | ≤ events (dedup collapses) | Signal stream + `signal_inbox` (§3.4) |
| **Automation rule** | A **stateless, per-event reflex the project owns**: "when X, do Y." May *invoke* a durable workflow (ADR-09) for multi-step/HITL; a trivial reflex does not. | A project admin | The rule definition persists; each firing is stateless | fires per matching event | `automation_rule` table (§3.5) |
| **Trigger** | A **stateful promise a person owns**: "wait until condition C, then unblock this task / remind me." A small state machine **armed → resolved / stale / disarmed**, fires **once per arming**. | A person | The arming persists until resolved/stale/disarmed | once per arming | `trigger` table (§3.6) |

**The collision resolution (ADR-19):** the old "Trigger" (a matcher→target *binding* under
`run_as`/`RunBudget`/`DelegationPolicy`/`gates`) is renamed a **subscription/automation binding**.
"Trigger" is reserved for the stateful per-person promise. Throughout this doc, *binding* = the
matcher→handler wiring; *Trigger* = the per-person promise.

**Subscriptions** (the fourth, infra-level wiring — Search indexer, Refs builder, Notif router, OLAP
loader, Audit recorder) are **bindings on the raw Event firehose**; they are the *excepted* consumers
that genuinely need every event (BUS-4). Everything **product/reactive** subscribes to **Signals**, not
the raw firehose — this is the upstream defence against the head-of-line-blocking gotcha (EI-03 §6.1).

> One-liner adopted into UX copy (ADR-19): *a Trigger is a promise the system keeps for you; an
> Automation rule is a reflex the project has.*

---

## 2. The durable streaming log

### 2.1 Transport decision: JetStream-class (BUS-1; default chosen, escape hatch written)

**Decision: NATS JetStream is the reference default transport for the durable bus**, deployed
self-hosted inside each cell. This adopts the doctrine's default-to-beat (BUS-1; EI-02 §3 "a durable
streaming log — a JetStream-class broker — with durable pull consumers and consumer groups, not
fire-and-forget pub/sub") and ADR-04's directional list, narrowed.

**Why JetStream over the same-class alternatives** (this is the *written why* the directives require):

| Candidate | Verdict | Reasoning |
|---|---|---|
| **NATS JetStream** | **Chosen default** | Raft-replicated streams (Ongaro & Ousterhout, *In Search of an Understandable Consensus Algorithm*, USENIX ATC 2014) give per-stream durability without ZooKeeper/KRaft operational weight; **durable PULL consumers + queue (consumer) groups** are first-class; **subject-based addressing** maps directly onto our dotted taxonomy (§6) with server-side filtering — the literal mechanism for BUS-3's "whitelist the subjects you handle, never `*`"; lightweight single-binary deploy fits **one-cell self-host parity** (ADR-11); per-subject/per-stream **message TTL + `purge` by subject** is the substrate for crypto-shred tombstoning (§4.8). |
| **Kafka / Redpanda** | Same-class, allowed with a written why | Proven at the highest volumes (Kreps et al., *Kafka: a Distributed Messaging System for Log Processing*, NetDB 2011; Wang et al., *Building a Replicated Logging System with Apache Kafka*, VLDB 2015). Partition-as-ordering is exactly our model. **But**: heavier operational surface per cell (broker + coordination), and a JVM/Redpanda-C++ footprint that taxes the "self-host = one cell, same artifacts" goal. **Reserved as the per-cell upgrade** when a cell's measured throughput outgrows JetStream (BUS-6 promotion trigger). The envelope/contracts (§5) are transport-agnostic, so this swap is a relay-target change, not a consumer rewrite. |
| **PG logical-decoding / pure PG-outbox-as-bus** | Acceptable *only* if it provides durable-pull + consumer-group semantics (BUS-1) | Fewest moving parts; tempting at the smallest scale. **But** at git-push QPS / agent-fan-out it becomes the hot table the doctrine warns against, and consumer-group fan-out + replay are bolt-ons, not native. **We use PG for the outbox (the emit *seam*), not as the inter-service bus.** |

The transport sits behind a thin `BusTransport` trait in `myelin-events` (a `put/consume/ack/purge`
surface), so JetStream→Kafka is a one-binding swap, mirroring STOR-1's content-addressed-blob trait
philosophy. **No non-durable fire-and-forget path exists** to regress to (EI-02 §4; BUS-2).

### 2.2 Partitioning: partition key = aggregate; per-aggregate ordering (ADR-04.2)

The **partition key is the aggregate** — one PR, one issue, one CI run, one doc, one channel. All
events for a single aggregate are **totally ordered**; **global order is explicitly NOT promised**
(EI-02 §3; ADR-04.2). Causally-related events are designed to **share an aggregate** rather than
relying on cross-aggregate order (EI-02 §3 "design causally-related events to share a subject").

Concretely, the JetStream **subject** encodes both the routing taxonomy and the ordering key:

```
evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>
   └─ stream filter ─┘   └──────── ordering partition (aggregate) ───────┘
```

- A **stream** is provisioned **per (tenant, subsystem)** (cell-local), partitioned internally by
  `aggregate_id`. This keeps tenant isolation a partition-key property, not a convention (EI-02 §1).
- Ordering is guaranteed **within `<aggregate_type>.<aggregate_id>`**. JetStream preserves publish
  order within a subject; the relay (§4.1) publishes per-aggregate in outbox commit order.
- Consumers that need per-aggregate order use **ordered/partitioned delivery keyed on
  `aggregate_id`** (one in-flight message per aggregate at a time per consumer); consumers that don't
  (Search, OLAP) take unordered higher-throughput delivery.

### 2.3 Per-ref ordering at git-push QPS (the explicit scale case)

Git push is the adversarial ordering case: a force-push or rapid sequence of pushes to **one ref**
must deliver `git.ref.updated` events **in push order**, and a busy monorepo can see hundreds of
ref-updates/sec across refs but bursts on a single hot ref (the default branch).

- **The aggregate for ref events is the ref**, not the repo:
  `evt.<tenant>.git.ref.<repo_id>:<ref_name>.updated`. This gives **per-ref ordering** (the property
  that matters — "what is the current tip of `main`") while letting **different refs of the same repo
  fan out in parallel** (the property that gives throughput). A repo-level aggregate would serialise
  all refs behind the hot branch — the head-of-line trap at the data-model layer.
- The **outbox sequence number** (`outbox.seq`, a per-aggregate monotonic counter, §3.2) is the
  tiebreaker the relay publishes in; consumers dedup on `event_id` and order on `(aggregate, seq)`.
- For a force-push race, the git subsystem's *own* ref-update transaction is the linearisation point
  (it owns the ref lock); the outbox row is written **in that same transaction** (BUS-2), so outbox
  order == ref-update order by construction. The bus never has to re-derive order it wasn't given.

This is the standard "partition by the entity whose order you actually need" lesson from Kafka's
partition-key design (Kreps 2011) applied to the ref.

---

## 3. The data model / schemas

### 3.1 The common envelope (concrete schema — ADR-13.2; the binding contract)

Every event carries this envelope. It is **versioned** (`schema_ver`) and **references-not-payloads**
(EI-02 §3; ADR-04.4): the envelope carries IDs and `ArtifactRef`s; personal data lives in the
producing subsystem's erasable store, which the event points to.

```jsonc
{
  // ── identity & type ──────────────────────────────────────────────
  "event_id":      "01J8...ULID",      // ULID; the IDEMPOTENCY anchor. Time-sortable.
  "type":          "git.pr.opened",    // canonical dotted name (§6 taxonomy)
  "schema_ver":    3,                   // integer; producer's schema version for `payload`

  // ── time (two clocks; EI distinguishes them) ─────────────────────
  "occurred_at":   "2026-06-19T10:14:02.118Z", // when the FACT happened (producer wall clock)
  "recorded_at":   "2026-06-19T10:14:02.140Z", // when the outbox row committed (DB clock)

  // ── tenancy & residency (partition + routing; EI-02 §1, ADR-11) ──
  "tenant":        "acme-eu",          // tenant id — first-class, never derived from URL
  "region":        "eu-central",       // immutable; the cell's region

  // ── actor (one polymorphic Principal incl. on-behalf-of; EI-02 §2)
  "actor": {
    "principal":   "myelin://acme-eu/identity/agent/triage-bot-7", // ArtifactRef of the acting principal
    "kind":        "agent",            // human | agent | service
    "on_behalf_of":"myelin://acme-eu/identity/human/alice",        // null for direct human action
    "session":     "sess_01J8...",     // the originating human session/action (the caused-by root anchor)
    "run":         "run_01J8..."       // present for agent/CI runs; ties to the reserve/settle gate
  },

  // ── subject (what the event is ABOUT) ────────────────────────────
  "subject":       "myelin://acme-eu/git/pr/88",        // the aggregate's ArtifactRef
  "aggregate":     "git/pr/88",                          // the ordering partition key (denormalised)

  // ── causality (NESTED, not flat — BUS-5 / EI-02 §6) ──────────────
  "correlation_id":"01J8...ROOT",      // the causal ROOT (the original human action); carries through
  "causation_id":  "01J8...PARENT",    // the IMMEDIATE parent event_id (parent = cause; null at root)
  "depth":         3,                   // root=0; child = parent.depth + 1 (the loop-cap counter)

  // ── GDPR & visibility routing ────────────────────────────────────
  "contains_personal_data": true,      // routes GDPR handling; drives crypto-shred eligibility
  "data_role":     "tenant-content",   // tenant-content (processor) | platform-operational (controller); ADR-12.5
  "visibility":    "project",          // public | tenant | project | restricted | private — a HINT; Id decides
  "pii_key_ref":   "kms://acme-eu/2026Q2/tenant", // envelope-encryption key id for any inline PII (null if none)

  // ── payload (producer-owned, versioned by schema_ver) ────────────
  "payload": { /* small, references-not-payloads; e.g. {"title_ref":"...","base":"main","head":"feat/x"} */ }
}
```

**Field-level rules** (these are the X-5 reconciliation points other systems depend on):

- `event_id` is a **ULID** (Tonsky/Feigin lexicographic ULID spec): 128-bit, time-sortable, so
  dedup, log scans, and replay cursors all benefit; collision-free under high concurrency. Consumers
  dedup on it (§4.2).
- `correlation_id` / `causation_id` / `depth` are **derived from the cause, correct by construction**
  (EI-02 §6): a child event copies `correlation_id` from its cause, sets `causation_id = cause.event_id`,
  `depth = cause.depth + 1`. A human (or agent) **cannot typo into a loop** because the guard reads
  platform metadata, not a commit-message convention (EI-02 §6). The `actor.session` is the distinct
  **caused-by** human-action anchor (BUS-5 keeps it separate from `causation_id`).
- `visibility` is a **hint for fan-out** (which Signals/notifications to consider), **never an authz
  decision** — Id's `check`/`list_objects` is authoritative (ADR-03). Semantics: `public` (cross-tenant
  visible, OSS), `tenant`, `project`, `restricted` (named grantees), `private` (actor only).
- `contains_personal_data` + `pii_key_ref` are the crypto-shred routing pair (§4.8). The **default is
  references-not-payloads**, so `pii_key_ref` is usually null and the event survives erasure untouched
  (the personal data is in the pointed-to store).
- Envelope evolution is **forward-only** (STOR-2): new optional fields only; a removed/renamed field
  is a new `schema_ver` with a registered upcaster (§4.10). Consumers ignore unknown fields.

### 3.2 The outbox table (per producing service; BUS-2)

Each subsystem owns an `outbox` table **in its own database**, written **in the same transaction** as
the state change (EI-02 §4). The relay (§4.1) drains it.

```sql
CREATE TABLE outbox (
  id            uuid    PRIMARY KEY DEFAULT gen_random_uuid(), -- relay claim id
  event_id      text    NOT NULL UNIQUE,        -- ULID; the broker-side DEDUP id (BUS-2)
  aggregate     text    NOT NULL,               -- ordering partition key, e.g. 'git/ref/<repo>:main'
  seq           bigint  NOT NULL,               -- per-aggregate monotonic (ordering tiebreaker)
  subject       text    NOT NULL,               -- destination subject (§2.2)
  envelope      jsonb   NOT NULL,               -- the full envelope (§3.1)
  tenant        text    NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  claimed_at    timestamptz,                    -- set when a relay worker claims it
  published_at  timestamptz,                    -- set on broker ack (then row is GC-eligible)
  attempts      int     NOT NULL DEFAULT 0,
  dead          boolean NOT NULL DEFAULT false, -- moved to dead-letter after bounded retries
  UNIQUE (aggregate, seq)                        -- enforces per-aggregate ordering at the source
);
CREATE INDEX outbox_unclaimed ON outbox (created_at) WHERE published_at IS NULL AND NOT dead;
```

`seq` is allocated per-aggregate (a small `aggregate_seq(aggregate)` counter row, or a sequence keyed
by aggregate) **inside the producing transaction**, so it reflects true commit order. The
`UNIQUE(aggregate, seq)` is the source-of-truth ordering invariant; the relay publishes in `seq` order
per aggregate.

### 3.3 The consumer offset / dedup store

Each durable consumer (consumer group) tracks its position via JetStream's durable consumer
(server-side ack floor) **plus** an idempotency ledger for effect-dedup:

```sql
CREATE TABLE consumer_dedup (
  consumer      text NOT NULL,      -- durable consumer name (bound by name; BUS-3)
  event_id      text NOT NULL,      -- the envelope event_id
  processed_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (consumer, event_id)  -- presence == "already handled"; the idempotency check
);
```

This is bounded (rows older than the log's max-redelivery window are GC'd). At-least-once + this
ledger + deterministic handlers ≈ **effectively-once** (EI-02 §4; ADR-04.1) — we do **not** chase true
exactly-once.

### 3.4 The Signal store (ADR-19; curation/dedup/severity)

```sql
CREATE TABLE signal_rule (              -- defines what events become signals
  id            uuid PRIMARY KEY,
  tenant        text NOT NULL,
  name          text NOT NULL,          -- e.g. 'ci-failure', 'agent-proposal', 'sla-breach'
  matcher       jsonb NOT NULL,         -- an EventMatcher AST (§4.5)
  severity      text NOT NULL,          -- info | notice | warning | error | critical (ranked)
  dedup_key_tpl text,                   -- template over envelope fields → the dedup key (§4.4)
  dedup_window  interval NOT NULL DEFAULT '5 minutes',
  enabled       boolean NOT NULL DEFAULT true
);

CREATE TABLE signal (                   -- the curated, deduped, ranked stream (also an inbox source)
  id            uuid PRIMARY KEY,
  tenant        text NOT NULL,
  rule_id       uuid NOT NULL REFERENCES signal_rule(id),
  dedup_key     text NOT NULL,          -- collapses duplicates within the window
  severity      text NOT NULL,
  first_event   text NOT NULL,          -- event_id that opened this signal
  last_event    text NOT NULL,          -- most recent collapsed event_id
  count         int  NOT NULL DEFAULT 1, -- how many events collapsed into this signal
  subject       text NOT NULL,          -- ArtifactRef
  occurred_at   timestamptz NOT NULL,
  resolved_at   timestamptz,            -- a later event may resolve it (e.g. ci.run.passed clears ci.run.failed)
  UNIQUE (tenant, rule_id, dedup_key, occurred_at)  -- window-scoped
);
```

Signals are themselves published to a **`sig.<tenant>.<severity>.<rule>` subject** so reactive
consumers subscribe to *curated, severity-filtered* streams (BUS-4) — never to `evt.*`.

### 3.5 The Automation-rule store (stateless reflex; ADR-19)

```sql
CREATE TABLE automation_rule (
  id            uuid PRIMARY KEY,
  tenant        text NOT NULL,
  name          text NOT NULL,
  matcher       jsonb NOT NULL,         -- EventMatcher AST; binds to Signals by default, Events if whitelisted
  action        jsonb NOT NULL,         -- {kind: tool|workflow|webhook, ref:..., input_tpl:...}
  run_as        text NOT NULL,          -- the binding principal (ArtifactRef) — was the old "Trigger" run_as
  delegation    jsonb,                  -- DelegationPolicy (Id evaluates the intersection; ADR-08.3)
  budget        jsonb,                  -- RunBudget — reserve/settle gate (D8)
  gates         jsonb,                  -- HITL gate names (ADR-08; AG-8)
  enabled       boolean NOT NULL DEFAULT true,
  created_by    text NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now()
);
```

A trivial reflex (`action.kind = tool|webhook`) fires statelessly per matching event. A multi-step or
HITL action (`action.kind = workflow`) **invokes** the durable-workflow engine (ADR-09) — the
automation rule itself stays stateless; durability lives in the workflow.

### 3.6 The Trigger store (stateful per-person promise; ADR-19)

```sql
CREATE TABLE trigger (
  id            uuid PRIMARY KEY,
  tenant        text NOT NULL,
  owner         text NOT NULL,          -- the PERSON who owns the promise (ArtifactRef)
  condition     jsonb NOT NULL,         -- EventMatcher AST: "until condition C holds"
  arms_subject  text NOT NULL,          -- what gets unblocked/reminded (ArtifactRef)
  state         text NOT NULL DEFAULT 'armed', -- armed | resolved | stale | disarmed
  armed_at      timestamptz NOT NULL DEFAULT now(),
  stale_after   timestamptz,            -- a durable timer (ADR-09) marks it stale
  resolved_by   text,                   -- event_id that resolved it
  resolved_at   timestamptz,
  on_resolve    jsonb NOT NULL          -- {kind: notify|tool|workflow, ...} — fires ONCE per arming
);
CREATE INDEX trigger_armed ON trigger (tenant) WHERE state = 'armed';
```

The state machine (§4.6) is **armed → {resolved | stale | disarmed}**, firing **once per arming**
(EI-03 §2). The Issue tracker surfaces this as the "unblock/remind me when…" UX (ISS-1).

---

## 4. The algorithms

### 4.1 The outbox relay (FOR UPDATE SKIP LOCKED; BUS-2; EI-02 §4)

The relay is a **stateless, horizontally-replicable** worker (often co-located with the producing
service, or a sidecar). Loop:

```sql
-- claim a batch, safe across replicas (no two relays grab the same row)
WITH claimed AS (
  SELECT id FROM outbox
  WHERE published_at IS NULL AND NOT dead
  ORDER BY aggregate, seq          -- preserve per-aggregate order
  FOR UPDATE SKIP LOCKED
  LIMIT :batch
)
UPDATE outbox o SET claimed_at = now(), attempts = attempts + 1
FROM claimed c WHERE o.id = c.id
RETURNING o.event_id, o.subject, o.envelope, o.aggregate, o.seq;
```

Then, **per aggregate, in `seq` order**:
1. `transport.put(subject, envelope, dedup_id = event_id)` — JetStream's **`Nats-Msg-Id` header set to
   `event_id`** enables **broker-side dedup** (a re-published row within the dedup window is dropped by
   the server, not just the consumer).
2. On broker ack → `UPDATE outbox SET published_at = now()`.
3. On failure → leave `published_at` null; the row is retried next loop (claim lock released). After
   **N attempts** (default 8, exponential backoff) → `dead = true` and copy to the **dead-letter
   stream** `dlq.<tenant>.<subsystem>` with the failure reason; alert via a Signal.
4. A GC job deletes `published_at < now() - interval '24h'` rows (bounded outbox; the log is the
   durable record, not the outbox).

**Properties:** `SKIP LOCKED` makes the relay safe across replicas (PostgreSQL row-locking semantics);
the per-aggregate `seq` ordering is preserved because we claim and publish ordered; `event_id` gives
at-least-once-with-dedup. This is the textbook outbox/CDC pattern (Richardson, *Microservices Patterns*,
2018, ch. 3 "Transactional outbox" / "Polling publisher"; Kleppmann, *Designing Data-Intensive
Applications*, 2017, ch. 11 on logs + change capture). **A logical-decoding (Debezium-class) relay is a
drop-in alternative** behind the same trait if polling becomes the bottleneck (it reads the WAL instead
of polling) — but polling-publisher is the default for operational simplicity and self-host parity.

### 4.2 At-least-once + idempotent consumers (the shared template; BUS-3; EI-03 §6)

The `myelin-events` crate ships **the only sanctioned consumer template**. Every consumer is built from
it; the discipline is mechanical (E-5 lint: no raw broker subscribe outside the helper). The template
encodes the four EI-03 §6 gotchas:

1. **Whitelist subjects — never `*`** (BUS-3 / EI-03 §6.1). A consumer declares the **exact** subjects
   it handles (`sig.acme-eu.error.>` or `evt.acme-eu.git.pr.>`), so unhandled types never accumulate
   behind it. An over-broad subscription head-of-line-blocks everything — the tens-of-millions-pending
   stall (EI-03 §6.1). This is enforced *structurally* by JetStream subject filters.
2. **Bind to a durable consumer by name; never re-declare its start policy on reconnect** (BUS-3 /
   EI-03 §6.2). On reconnect the template **binds** to the existing durable consumer; it never
   re-asserts `DeliverPolicy`/start position (which can wedge the broker into re-delivering all
   events). Start policy is set **once** at creation.
3. **Idempotent on `event_id`** (ADR-04.1): `INSERT INTO consumer_dedup … ON CONFLICT DO NOTHING`;
   if the row already existed, **skip** (already handled). Handlers must be deterministic.
4. **Acknowledge only after the work is enqueued** (EI-03 §6.3): `ack` is sent *after* the effect is
   durably enqueued/committed (at-least-once to the next stage). On crash mid-handle, the message
   redelivers and the dedup ledger makes it a no-op.
5. **Terminate non-retryable messages immediately** (EI-03 §6.3): malformed bytes / unknown
   un-upcastable `schema_ver` are `term`'d (not `nak`'d) so they don't burn the redelivery budget;
   they go to the DLQ for inspection.
6. **Bounded prefetch + bounded handler pool** (X-3 / ADR-16): `MaxAckPending` caps in-flight messages;
   the handler pool is bounded; per-tenant in-flight caps enforce fairness so one tenant's burst can't
   starve another (the agent-surge case, §8).
7. **Consumer-lag is exposed** (BUS-3 / X-1): `num_pending` per consumer is a telemetry survival signal
   (§4.11); a silently growing pending count is the EI-03 §6.1 stall, made visible.

```rust
// illustrative — the ONLY way to consume (no raw subscribe escapes the helper)
events::consume(ConsumerSpec {
    durable: "search-indexer",            // bind by name; immutable start policy
    subjects: vec!["evt.*.*.*.>".into()], // EXPLICIT whitelist — infra indexer is the firehose exception
    max_ack_pending: 5_000,               // bounded prefetch
    per_tenant_inflight: 256,             // fairness cap (ADR-16)
}, |msg| async move {
    if dedup.seen(&msg.event_id).await? { return Ack; }   // idempotent
    match handle(&msg).await {
        Ok(()) =>      { dedup.mark(&msg.event_id).await?; Ack }   // ack AFTER work
        Err(Malformed) =>                                  Term,    // don't retry junk
        Err(Transient) =>                                  Nak,     // redeliver
    }
});
```

### 4.3 The firehose split (ADR-04.5; the dedicated transport)

The durable bus must **not** carry CI log lines, chat presence/typing/read-state, or collab op-streams
the same way — their volume would melt the durable bus and starve control-event ordering (SC-9/SC-10;
EI-02 §3 by implication; ADR-04.5).

| Stream class | Transport | What rides it | Pointer event on the durable bus |
|---|---|---|---|
| **CI logs** | Append-mostly log/object tier (range-read + tail), keyed by run/step | `ci.log.appended` frames | `ci.log.available` (pointer: "lines N..M ready at <ref>") |
| **Chat presence/typing/read-state** | Ephemeral NATS core (non-durable, at-most-once is fine) | presence/typing/read frames | none (ephemeral by nature) or coarse `chat.read_state.updated` summary |
| **Collab op-streams** | The **resume-cursor durable transport** (KN-1 — owned by Knowledge, built FIRST) | CRDT/op frames with resume cursors | `doc.updated` pointer |

**Rule:** the durable bus carries only **"data available/updated" pointer events**; an agent is
**never woken per log line** (EI-03 §6.1; ADR-04.5). The firehose transport is a sibling of the durable
bus inside the cell, exposed via `firehose.publish(stream, frame)` / `firehose.tail(stream, range)`
(§5). The collab transport's resume-cursor + idempotent-apply property is **Knowledge's deliverable**
(KN-1); `myelin-events` provides the *pointer-event seam* and the firehose `tail`/`publish` API, not the
CRDT.

### 4.4 Signal curation / dedup / severity-ranking (ADR-19; resolves BUS-4's open mechanism)

A **Signal engine** (an infra consumer on the raw `evt.*` firehose — one of the excepted firehose
consumers) evaluates each event against enabled `signal_rule`s:

1. **Match** — run the rule's `EventMatcher` (§4.5) over the envelope.
2. **Severity-rank** — stamp the rule's severity (`info`<`notice`<`warning`<`error`<`critical`).
3. **Dedup within window** — compute `dedup_key = render(rule.dedup_key_tpl, envelope)` (e.g.
   `ci-failure:<pipeline>:<branch>`); `INSERT … ON CONFLICT (tenant,rule_id,dedup_key,window)
   DO UPDATE SET count = count+1, last_event = excluded.last_event`. **N identical failures collapse to
   one Signal with `count=N`** — the storm-control primitive Notif relies on (UC-EDGE-4).
4. **Auto-resolve** — a rule may declare a *resolving* matcher (`ci.run.passed` resolves the matching
   `ci.run.failed` Signal): set `resolved_at`, publish a `sig…resolved`.
5. **Publish** to `sig.<tenant>.<severity>.<rule>` for reactive consumers.

This is the upstream defence (BUS-4) against the head-of-line-blocking gotcha: product consumers and
agents subscribe to **curated Signals**, so the raw-firehose stall surface is limited to the handful of
infra indexers (Search/Refs/OLAP/Audit/Signal-engine), which are designed for full volume.

### 4.5 The `EventMatcher` predicate language (AG-7; ADR-07 — sharing the query AST)

**Decision: the `EventMatcher` is the predicate core of the shared query AST (`myelin-query`),
serialised as JSON, evaluated by a custom bounded interpreter — NOT raw CEL or JSONLogic.** This
resolves AG-7's CEL/JSONLogic/custom choice with a written why:

| Option | Verdict | Why |
|---|---|---|
| **CEL** (Google Common Expression Language) | Rejected as the surface, borrowed for design | CEL is well-specified and **non-Turing-complete with documented cost/complexity bounds** (the property we want; cf. the CEL spec's "no unbounded loops, linear-time evaluation" guarantees). But adopting CEL-the-language fragments the platform: saved views, search filters, and matchers would then speak *two* languages (CEL + the query AST), violating ADR-07's "one AST, one parser, one DoS-hardening surface." |
| **JSONLogic** | Rejected | Untyped, no schema awareness, no native `ArtifactRef`/relation/`list_objects` composition; would need so much extension it stops being JSONLogic. |
| **Custom = the query-AST predicate core** | **Chosen** | ADR-07 already mandates one AST for UI saved-views, CLI, API, automations, and agent triggers — *the matcher is one of its compile targets*. We **borrow CEL's safety discipline** (declarative, no unbounded recursion, statically cost-bounded, total functions) and apply it to our AST's predicate subset. One parser, one validator, one renderer-back-to-human, **one DoS-hardening surface** (AG-7; ADR-07 §Consequences). |

**Shape** (the predicate subset of `myelin-query`, the X-5 contract Search & Issues also compile):

```jsonc
{ "and": [
    { "eq":   ["event.type", "ci.run.failed"] },
    { "eq":   ["event.subject.subsystem", "ci"] },
    { "in":   ["event.actor.kind", ["human", "agent"]] },
    { "lt":   ["event.payload.severity_rank", 3] },
    { "ref_in": ["event.subject", "$saved_view:my-pipelines"] }  // composes with list_objects
] }
```

**Safety properties** (the bar AG-7 sets): **no user-defined functions, no loops, no unbounded
recursion**; every operator is total and **statically cost-bounded** (the validator rejects an AST
whose worst-case cost exceeds a budget); evaluation is **side-effect-free**. It is **permission-aware by
construction** (ADR-07): a matcher always composes with `list_objects(viewer, read, type)` so no
matcher/saved-view/search surface can select artifacts the subject can't see. The `EventMatcher`
compiles to: (a) a JetStream subject filter where possible (cheap server-side prefilter), then (b) the
bounded interpreter over the envelope for the residual predicate.

### 4.6 The Trigger state machine (ADR-19; stateful per-person promise)

```
                 condition matches an event
   ┌──────────┐  ──────────────────────────►  ┌───────────┐
   │  armed   │                                │ resolved  │  fires on_resolve ONCE
   └──────────┘                                └───────────┘
        │  stale_after timer (durable, ADR-09) fires
        ▼
   ┌──────────┐        owner disarms
   │  stale   │   ◄──────────────────────────  (from armed, any time) → ┌──────────┐
   └──────────┘                                                          │ disarmed │
                                                                         └──────────┘
```

- **armed → resolved**: the Trigger-engine consumer (a curated Signal/Event consumer) matches
  `condition` against an incoming event; transitions atomically (`UPDATE trigger SET state='resolved',
  resolved_by=:event_id WHERE id=:id AND state='armed'` — the `state='armed'` guard makes it
  **fire-once-per-arming** under concurrent events). Then runs `on_resolve` (notify / tool / workflow),
  carrying causality (the resolving event is the cause).
- **armed → stale**: a **durable timer** (ADR-09) set to `stale_after` fires; the promise expires
  unresolved (e.g. "remind me when the build passes" but it never did within the window).
- **armed → disarmed**: the owner cancels.

Re-arming creates a new arming (idempotency is per-arming). This is a textbook small state machine; the
durability of the `stale_after` timer is delegated to the durable-workflow engine (ADR-09), not
reinvented here (SLA timers — millions of durable timers — share that substrate, SC-11).

### 4.7 The reactive/dispatch tier (separately-reviewed; D7 / EI-03 §6)

Per D7 and EI-03 §6, the tier that consumes Signals and **dispatches agents/automations** is a
**stateful exception** with its own explicit design (it is the orchestrator EI-03 §6 warns about). Its
disciplines, on top of §4.2's consumer template:

- **Thread causality nested, not flat** (BUS-5 / EI-03 §6.4): when a dispatch produces an agent action,
  the new event derives `causation_id = dispatch_event.event_id`, `correlation_id` carried,
  `depth = +1`. Flat threading (everything → root) collapses the "why" chain and breaks depth-capping —
  forbidden.
- **The structural loop guards** (AG-6 / EI-03 §5.3): **self-guard** (skip the agent's own output —
  drop an event whose `actor.principal` == the consumer's agent), **reference gate** (only a
  structured `artifact_ref` node can re-trigger, never raw typed text — wired to ADR-05's "only
  `artifact_ref` nodes emit `ref.created`"), **causal-depth ceiling** (drop/park dispatch when
  `depth > ceiling`, default 12), and a **shared-causal-root tripwire** (if > K events share one
  `correlation_id` within a short window, trip a per-tenant circuit breaker — EI-02 §6).
- **Bounded dispatch worker pool that drops over-cap** (AG-6 / EI-03 §5.4): a mention/event storm is
  bounded; over-cap dispatches are dropped (with a Signal), never forked unboundedly.
- **Reserve/settle cost gate** (D8 / CI-2): every dispatched run passes the universal reserve/settle
  gate (no balance → no execution) before the Agent Fabric/CI runner starts it.

The bus *delivers* to the Agent Fabric's `EventInbox` (the Fabric owns the runtime); this tier owns the
*matching, guarding, and rate-limiting* between Signal and inbox.

### 4.8 Retention + crypto-shred + tombstones (ADR-04.4; ADR-12; EI-04 §1)

The append-only log is in tension with GDPR erasure (EI-04 §1). The bus resolves it with the
**references-not-payloads + crypto-shred + tombstone** triad — the event-log half EI-04 §1 calls
"workable":

1. **References-not-payloads (the primary lever)**: the envelope carries IDs/`ArtifactRef`s + a
   pseudonymous `actor.principal`; the personal data lives in the producing subsystem's erasable store.
   **Erasing the person tombstones the identity, not the fact** (EI-04 §1) — the event's structure and
   causal links survive for audit integrity. Most events need no further action: `contains_personal_data
   = false`.
2. **Bounded retention**: per-stream **message TTL** (JetStream `MaxAge`). Default domain-event
   retention is **90 days hot** on the log; the **OLAP/Audit holders are the durable long-term
   record** (fed off the bus), so the log itself can be short. Replay (§4.9) reconstructs derived stores
   from source, not from an infinitely-retained log.
3. **Crypto-shred (for the rare inline-PII event)**: when an event *must* carry inline PII
   (`contains_personal_data = true`, `pii_key_ref` set), that field is **envelope-encrypted with a
   per-tenant (optionally per-subject) key** (ADR-12.3; co-owned with Storage/KMS, §10.5 of the
   overview). Erasure = **destroy the key**; the ciphertext in the log/backups becomes unrecoverable
   without rewriting the immutable log (EI-04 §1; `gdpr-eu-sovereignty.md §6.5`).
4. **Tombstones**: a `*.erased` tombstone event is published on the aggregate so **live consumers
   degrade gracefully** (Refs renders a tombstone placeholder; Search purges + reindexes). The bus is a
   `PersonalDataHolder`: `locate`/`erase` over its history = crypto-shred the inline-PII keys + emit
   tombstones; it never needs to mutate immutable bytes for the common (references-not-payloads) case.

**Floor named (GD-1):** git-history author/email erasure is NOT solved here — that is the named
co-owned reconciliation deliverable (pseudonymous-commit-by-default + history-rewrite limit). The bus
solves the *event-log* half only; it does not pretend to solve the git-history half (EI-04 §1).

### 4.9 Reindex-from-source re-emit (BUS/REF-4/SEARCH-1; EI-04 §5.3 — first-class)

Every derived store (Search, Refs, OLAP, Notif read-models) is **reconstructible by asking each owner
to re-emit through the live consumer path** — the index **never reads owner DBs**, so steady-state and
recovery use **one code path and cannot drift** (EI-04 §5.3; the doctrine's "reindex-from-source is a
first-class resilience primitive").

The bus exposes a **re-emit protocol**:

```
reindex(scope) →  for each owning subsystem in scope:
   subsystem.replay(scope, since=<cursor>) → emits `*.snapshot` events through the SAME outbox→bus path
   → the same live consumers (Search/Refs/OLAP) ingest them idempotently (event_id dedup)
```

- `*.snapshot` events carry the same envelope and are **idempotent on `event_id`** (a snapshot
  event_id is deterministic from `(aggregate, version)`), so re-running a reindex is safe.
- This is the **only** recovery path (SEARCH-1): there is no bespoke "read the Search index from
  Postgres" backdoor. The **reindex-from-cold parity drill** (§8) asserts a cold rebuild matches the
  live state.
- It doubles as the **schema-upcaster backfill** path (§4.10) and the **new-consumer bootstrap** path
  (a brand-new Refs index is just a reindex from `since=0`).

### 4.10 Schema evolution / upcasting (forward-only; STOR-2)

`schema_ver` is per-producer-per-type. Evolution is **expand→migrate→contract** (STOR-2):

1. **Expand**: producer adds optional fields under a new `schema_ver`; consumers (which ignore unknown
   fields) keep working.
2. **Upcast**: the `myelin-events` crate registers **upcasters** `(type, from_ver) → to_ver` (pure
   functions) applied at consume time, so a consumer always sees the latest shape. A message whose
   `schema_ver` has no registered upcaster path is **`term`'d to DLQ** (§4.2 rule 5), never silently
   dropped.
3. **Contract**: only after a backfill (via reindex-from-source, §4.9) re-emits old aggregates at the
   new version is the old version retired. **No rollback migrations** (you can't un-emit).

### 4.11 Telemetry contract (X-1 — the Phase-5 drill survival signals)

`myelin-events` emits, per the X-1 observability contract: **consumer lag** (`num_pending` per durable
consumer — the §4.2 head-of-line signal), **outbox depth + age** (unpublished rows; a rising age = the
relay is wedged), **relay publish rate + dead-letter rate**, **per-aggregate publish latency**
(`recorded_at`→broker-ack), **dedup hit-rate** (effectively-once health), **per-tenant in-flight**
(fairness/agent-surge), and **causal-depth histogram + shared-root-tripwire counter** (loop-safety).
These are the assertions the §8 drills read.

---

## 5. Contracts / APIs exposed to other systems (the glue — STABLE)

These are what Refs, Search, Notif, Agent Fabric, OLAP, GDPR, and the subsystems compile against.
Field names + units are reconciled here per X-5; changing them is a breaking, whole-workspace change
(ADR-01).

### 5.1 The envelope (§3.1) — the primary contract
The envelope **is** the contract (ADR-13.2). Every producer emits it; every consumer reads it. The
non-negotiable fields are in §3.1; the taxonomy of `type` and the `subject` token grammar are §6.

### 5.2 Emit (outbox-only; BUS-2)
```rust
// in the SAME db transaction as the state change — the ONLY emit path
events::outbox(tx).emit(Envelope { type_, subject, payload, actor, /* causality auto-derived from cause */ })?;
```
There is **no** `publish_now()` / fire-and-forget. `emit` writes the outbox row (§3.2); the relay does
the rest. Causality is **derived from the triggering event** (passed in `ctx`) so it is correct by
construction (BUS-5).

### 5.3 Subscribe / consume (the template; BUS-3)
```rust
events::consume(ConsumerSpec { durable, subjects /* explicit whitelist */, max_ack_pending,
                               per_tenant_inflight }, handler)   // §4.2 template
```
Used by Search (index off `evt.*`), Refs (`ref.created`/`ref.removed`), Notif (Signals), OLAP, Audit.
**Infra indexers may take the firehose; product/reactive consumers take `sig.*` Signals** (BUS-4).

### 5.4 Signals / Automations / Triggers (ADR-19)
```rust
events::define_signal_rule(SignalRule { matcher, severity, dedup_key_tpl, dedup_window })
events::register_automation(AutomationRule { matcher, action, run_as, delegation, budget, gates })
events::arm_trigger(Trigger { owner, condition, arms_subject, on_resolve, stale_after })
events::disarm_trigger(id)
```

### 5.5 Firehose (the dedicated transport; §4.3)
```rust
firehose::publish(stream, frame)          // CI log frame, presence frame, collab op
firehose::tail(stream, range) -> Stream    // range-read / tail (CI log viewer, presence)
// the durable bus carries the POINTER event (ci.log.available / doc.updated) via §5.2 emit
```

### 5.6 Reindex-from-source (BUS/SEARCH-1; §4.9)
```rust
events::reindex(scope)                      // asks owners to replay through the live path
// a subsystem implements: fn replay(scope, since) -> emits *.snapshot via outbox
```

### 5.7 PersonalDataHolder (ADR-12; the bus is a holder)
```rust
impl PersonalDataHolder for EventBus {
  fn locate(subject) -> inline-PII events + tombstone status;
  fn erase(subject)  -> crypto-shred inline-PII keys + emit *.erased tombstones; // §4.8
  fn export(subject) -> the subject's events (references resolved via owners);
}
```

### 5.8 Replay / ops
```rust
events::replay(correlation_id)   // re-drive a causal tree for debugging (E-6 investigate-before-build)
events::dlq(stream).inspect|requeue|drop
```

---

## 6. The canonical event taxonomy + `ArtifactRef` token table (resolves TE-10 + C-3)

### 6.1 Dotted-name scheme (TE-10)

`type = <subsystem>.<artifact_type>.<event_name>` — lowercase, dot-separated, **singular**
artifact/subsystem tokens, **past-tense** event verbs (`opened`, not `open`). Two segments minimum
(`<subsystem>.<event>`), three when an artifact type clarifies (`git.pr.opened`). This canonicalises the
Phase-1 drift (`git.pr.opened` vs `pr.opened`): **subsystem-prefixed is canonical** (technical-
structuring §0 deferred this; resolved here).

Rules: tokens match `[a-z][a-z0-9_]*`; the **subsystem segment is the C-3 canonical token** (§6.2); the
event verb is past tense; lifecycle verbs are consistent across subsystems
(`created/updated/deleted/merged/closed/reopened/erased`).

### 6.2 The `ArtifactRef` subsystem/type token table (resolves C-3)

`ArtifactRef = myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` (ADR-13.1). The **subsystem segment is
platform law and the CLI noun grammar**; C-3 found drift (`issue` vs `issues`, `kb` vs `knowledge`).
**Canonical, singular tokens (decided):**

| Subsystem | **`<subsystem>` token (canonical)** | CLI noun alias | Representative `<type>` tokens | Example `ArtifactRef` |
|---|---|---|---|---|
| Git hosting | **`git`** | `repo` | `repo`, `pr`, `commit`, `branch`, `tag`, `review`, `comment`, `ref` | `myelin://acme-eu/git/pr/88#comment-12` |
| CI/CD | **`ci`** | `ci` | `pipeline`, `run`, `job`, `step`, `artifact` | `myelin://acme-eu/ci/run/4412#step-3` |
| Issue tracker | **`issue`** | `issue` | `issue`, `epic`, `sprint`, `field`, `comment`, `relation` | `myelin://acme-eu/issue/issue/ABC-123` |
| Knowledge | **`knowledge`** | `doc` | `page`, `block`, `database`, `row`, `view` | `myelin://acme-eu/knowledge/block/PAGE-7c2#b9` |
| Chat | **`chat`** | `chat` | `channel`, `message`, `thread` | `myelin://acme-eu/chat/message/M-991` |
| Identity | **`identity`** | `org`/`agent` | `human`, `agent`, `service`, `org`, `team`, `project`, `role` | `myelin://acme-eu/identity/agent/triage-bot-7` |
| Refs | **`refs`** | `refs` | `edge` | `myelin://acme-eu/refs/edge/E-55` |

**Canonical choices that close C-3:** `issue` (singular, not `issues`); `knowledge` for the **ref
segment** with **`doc` as the documented human-facing CLI alias only** (the most visible C-3 mismatch);
`git` (not `repo`) for the ref segment with `repo` as the CLI alias. The **alias map is a separate,
documented render-time projection** (EI-02 §7: display keys are derived, never stored) — refs and events
always use the canonical token.

### 6.3 Representative event names (the seed taxonomy; subsystems extend in P4)

`git.pr.opened|updated|closed|merged|reopened|marked_ready`, `git.ref.updated`, `git.review.submitted`,
`git.comment.created`; `ci.run.started|passed|failed|cancelled`, `ci.log.available` (pointer),
`ci.artifact.published`; `issue.issue.created|updated|transitioned|closed`, `issue.relation.created`;
`knowledge.page.created|updated`, `knowledge.doc.updated` (pointer), `knowledge.row.updated`;
`chat.message.created`, `chat.read_state.updated` (coarse); `identity.permission.granted|revoked`,
`identity.member.added`; `refs.edge.created|removed` (= `ref.created`/`ref.removed`); plus the
cross-cutting `*.erased` tombstone and `*.snapshot` reindex events. Each subsystem owns its full list as
a P4 deliverable; this is the seed + the grammar.

---

## 7. Scaling / sharding in the cell topology (ADR-11)

### 7.1 In-cell first (ADR-11.5)
The bus is **cell-local**; all heavy cross-system work (Refs build, Search index, OLAP rollup) is
**async off the bus**, never synchronous in the write path (ADR-11.5). Per-aggregate (not global)
ordering is what makes the bus horizontally scalable inside a cell (ADR-04.2).

### 7.2 Stream sharding
Streams are provisioned **per (tenant, subsystem)** and partitioned internally by `aggregate_id`
(§2.2). A hot tenant scales by **more partitions within its streams**; a hot **cell** scales by **adding
JetStream nodes** (Raft-replicated streams rebalance). The tenant is the blast-radius and fairness unit
(EI-02 §1): per-tenant in-flight caps (§4.2) ensure one tenant's git-push/agent burst can't starve
another's control events.

### 7.3 The firehose is sharded separately (§4.3)
CI-log/presence/collab volume rides its own transport so it never contends with control-event ordering
(SC-9/SC-10). It scales on its own axis (object-tier throughput for logs; ephemeral fan-out for
presence).

### 7.4 Cross-cell propagation (FLOOR — designed-not-built)
A **multi-cell tenant** (a 10,000-person org spanning cells, SC-2/SC-3) needs cross-cell event
propagation. **This is a named floor, not built in v1.** The design seam: the **control plane**
(personal-data-free, ADR-11.4) carries a **minimal pointer-event bridge** between a tenant's cells
(only `subject` + `type` + `correlation_id`, **never payload or PII** — residency-preserving); each
cell resolves the `ArtifactRef` locally per viewer. Follow-on owner: **P4 / control-plane design + the
multi-cell tenancy resolution (SC-2/SC-3)**. The single-cell path is complete; the contracts (§5) are
cell-agnostic so this extends without a rewrite.

### 7.5 The column-store/time-series seam (BUS-6 — specified-not-built)
The highest-volume **durable** streams (e.g. an audit-grade firehose, or a future high-cardinality
event type) keep a **seam for a column-store/time-series engine** (ClickHouse-class, aligning with the
OLAP read store, ADR-10) — but **do not add it before the volume is measured** (BUS-6; EI-04 §5.2).
**Promotion trigger:** a measured per-stream volume that the JetStream tier serves at degraded latency.
Until then, the 90-day-hot log + OLAP long-term holder (§4.8) suffices.

### 7.6 Stateful-component register + blast-radius note (X-4)
| Stateful component | Shared-state / sharding plan | Blast radius if it fails |
|---|---|---|
| JetStream durable streams | Raft-replicated per stream; per-(tenant,subsystem) streams, aggregate-partitioned | One cell's event delivery degrades; outbox buffers (no loss); consumers resume on recovery |
| Per-service outbox tables | In each subsystem's PG (their shard) | That subsystem's emit stalls; **no loss** (rows persist); relay drains on recovery |
| `consumer_dedup` ledgers | Per consumer, tenant-partitioned PG | A consumer may re-process (idempotent → safe); GC'd within redelivery window |
| Signal / Trigger / Automation stores | PG, tenant-partitioned | Reactive tier degrades; Events unaffected (durable log is the truth) |
| Firehose transport | Ephemeral (presence) / object-tier (logs) | Presence/logs degrade; **never** affects durable control events (the split's whole point) |
Everything else (relay workers, Signal engine, dispatch tier, consumers) is **stateless and
replaceable** — recoverable by reconnecting to the durable log + reindex-from-source (§4.9).

---

## 8. Failure modes + the drills owed (Phase 5 owns mechanics; we enumerate)

Per the PROVE-IT mindset (EI-01 P3; T-2): each property that can fail names the **quantified drill**
that proves it. The bus owes these (each a Phase-5 scorecard item, T-4):

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry, §4.11) |
|---|---|---|---|
| D-1 | **Zero loss across reconnect** (EI-03 §6.2; the headline) | Kill a consumer mid-stream + sever the broker connection during a sustained publish; on reconnect (bind-by-name, immutable start policy) assert **zero events lost, zero duplicated effects** (dedup ledger). Gate: **0 lost, 0 duplicate effects**. | consumer lag, dedup hit-rate |
| D-2 | **Consumer-lag / head-of-line stall** (EI-03 §6.1) | Inject a flood of *unhandled* types at a consumer subscribed (incorrectly) to `*`; assert the whitelist-template consumer (correct) does **not** stall while the naive one does — proving the whitelist defence. Gate: **lag bounded, no silent stall**; lag alarm fires. | `num_pending` per consumer |
| D-3 | **Replay correctness** (E-6) | Replay a `correlation_id` causal tree; assert deterministic re-drive, idempotent (no double effects), causality preserved. Gate: **replay == original effects, exactly once**. | dedup hit-rate, causal-depth |
| D-4 | **Outbox no-ghost / no-loss** (EI-02 §4) | Crash a producer *between* state-commit and relay-publish; assert the event is still delivered (outbox survived) and **never** delivered without the state change. Gate: **0 ghost, 0 lost**. | outbox depth+age |
| D-5 | **Reindex-from-cold parity** (SEARCH-1; EI-04 §5.3) | Wipe a derived store (Search/Refs); `reindex(scope)`; assert the rebuilt store **byte-matches** the live state. Gate: **cold == live**. | snapshot dedup |
| D-6 | **Causal-loop tripwire** (AG-6; EI-03 §5.3) | Adversarially create a self-triggering automation; assert the depth ceiling + shared-root tripwire trip the per-tenant breaker before runaway. Gate: **loop halts ≤ ceiling; breaker trips**. | depth histogram, tripwire counter |
| D-7 | **30× agent-surge / fairness** (ADR-16; EI-02 §5) | 30× agent publish surge on one tenant; assert the **human/control lane holds**, the agent lane sheds (429+Retry-After honoured), **other tenants unaffected**. Gate: **human-lane latency within budget; cross-tenant unaffected**. | per-tenant in-flight, shed counters |
| D-8 | **Crypto-shred reaches the log** (ADR-12; EI-04 §1) | Erase a subject; assert inline-PII events are unrecoverable (key destroyed) and `*.erased` tombstones emitted; live consumers degrade gracefully. Gate: **0 recoverable PII in log/backups; tombstones present**. | holder erase receipts |
| D-9 | **Per-ref ordering at push QPS** (§2.3) | Burst force-pushes to one hot ref under load; assert `git.ref.updated` delivered in push order per ref, parallel across refs. Gate: **per-ref order preserved at target QPS**. | per-aggregate publish latency |

---

## 9. Cited prior art

- **Log-structured / consensus transport.** Diego Ongaro, John Ousterhout, *In Search of an
  Understandable Consensus Algorithm (Raft)*, USENIX ATC 2014 — backs JetStream's replicated streams.
  Jay Kreps, Neha Narkhede, Jun Rao, *Kafka: a Distributed Messaging System for Log Processing*, NetDB
  2011, and Guozhang Wang et al., *Building a Replicated Logging System with Apache Kafka*, VLDB 2015 —
  the partition-as-ordering model. Jay Kreps, *The Log: What every software engineer should know about
  real-time data's unifying abstraction* (2013) — the log-as-source-of-truth thesis behind ADR-04.
  NATS JetStream documentation — durable pull consumers, queue groups, subject filtering, `Nats-Msg-Id`
  dedup, `MaxAge`/`purge` (the crypto-shred substrate).
- **Outbox / CDC.** Chris Richardson, *Microservices Patterns* (Manning, 2018), ch. 3 — "Transactional
  outbox" + "Polling publisher" + "Transaction log tailing." Debezium project (log-tailing CDC) — the
  drop-in WAL-based relay alternative. Gunnar Morling, *Reliable Microservices Data Exchange with the
  Outbox Pattern* (2019).
- **Idempotency / effectively-once.** Pat Helland, *Idempotence Is Not a Medical Condition* (ACM Queue,
  2012) and *Life beyond Distributed Transactions: an Apostate's Opinion* (CIDR 2007) — at-least-once +
  idempotent handlers ≈ effectively-once; entity-keyed dedup. Martin Kleppmann, *Designing
  Data-Intensive Applications* (O'Reilly, 2017), ch. 11 (stream processing, exactly-once-as-illusion,
  logs + change capture).
- **Causality / provenance.** Leslie Lamport, *Time, Clocks, and the Ordering of Events in a
  Distributed System* (CACM, 1978) — happened-before, the basis for the nested `causation_id`/`depth`
  derivation. Distributed-tracing lineage (Sigelman et al., *Dapper*, Google 2010; W3C Trace Context) —
  propagating causal/trace context across hops in headers (BUS-5).
- **Predicate / matcher safety.** Google CEL (Common Expression Language) specification — the
  non-Turing-complete, statically-cost-bounded, total-function discipline we borrow for the
  `EventMatcher` (without adopting CEL-the-surface, per ADR-07).
- **State machines / durable timers.** Temporal / durable-execution literature (Cadence/Temporal design
  docs) — the substrate for the Trigger `stale_after` timer and automation-invoked workflows (ADR-09).
- **Doctrine.** EI-02 §3 (durable streaming log), §4 (transactional outbox), §5 (backpressure), §6
  (causality); EI-03 §2 (four primitives), §6 (orchestrator gotchas); EI-04 §1 (erasure vs
  immutability), §5.2 (event volume seam), §5.3 (reindex-from-source).

---

## 10. Open questions for Phase 4

1. **Per-subsystem full event taxonomy (TE-10 completion).** §6 seeds the grammar + tokens; each
   subsystem (Git/CI/Issues/Knowledge/Chat) owns its **complete** dotted-name list, `schema_ver`
   lineage, and payload shapes as a P4 deliverable, validated against the §6 grammar.
2. **Cross-cell pointer-event bridge (FLOOR; §7.4).** The multi-cell-tenant control-plane bridge —
   exact bridged-field set, residency proof that no PII crosses, per-viewer resolution latency — is
   owned by the **control-plane / multi-cell tenancy** P4 resolution (SC-2/SC-3).
3. **Collab op-stream transport (KN-1).** The resume-cursor durable transport's concrete protocol is
   **Knowledge's** deliverable; the bus provides only the pointer-event seam + firehose `tail`/`publish`.
   The reconnect-loses-zero-ops drill (T-5) lives there; this doc's D-1 covers the **durable bus**.
4. **Signal-rule authoring UX + default rule set.** Which events are Signals by default (the curated
   set), severity assignment, and the admin authoring surface (the Zapier-class builder over the AST) —
   product/UX-shaped, P4 + design language.
5. **CI metering ↔ reserve/settle gate wiring (CI-2/D8).** The dispatch tier (§4.7) calls the universal
   reserve/settle gate; the gate's concrete unit/wallet model is Commercial + CI (P4).
6. **Column-store promotion (BUS-6; §7.5).** The measured-volume threshold and the ClickHouse-class
   target for high-volume durable streams — deferred until volume is *measured* (EI-04 §5.2).
7. **`Agent` vs `Service` principal kind in `actor`/dispatch (AG-1).** The envelope models
   `actor.kind ∈ {human, agent, service}`; whether agent and service are one kind or two is an Id P3/P4
   item that the dispatch loop guards (§4.7) must respect — flagged, not foreclosed here.
8. **Saved-view ↔ EventMatcher AST shared grammar finalisation (TE-6).** §4.5 commits the matcher = the
   query-AST predicate core; the **full** AST grammar (operators, types, `ref_in`/`list_objects`
   composition) is co-finalised with Issues/Search (ADR-07 → P4).
