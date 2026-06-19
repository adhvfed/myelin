# Phase 5 — Event Bus + Trigger/Automation Engine (`myelin-events`) — REFINED

> Phase: `05-refined-shared-systems-architecture`. **Supersedes**
> [`../03-shared-systems-architecture/event-bus.md`](../03-shared-systems-architecture/event-bus.md).
> Canonical brief: [`VISION.md`](../../VISION.md). Binding doctrine:
> [`../../external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §3/§4/§5/§6, [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md)
> §1/§5.2/§5.3. Reconciliation spine (binding): [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> + [`contract-index.md`](./contract-index.md). Spine: ADR-04 (delivery + firehose split), ADR-19 (four
> primitives), ADR-07 (matcher↔AST), ADR-13.2 (envelope), ADR-08.5 (one trigger engine), ADR-16
> (backpressure), ADR-11 (cells), ADR-12 (GDPR holders). Date: 2026-06-19.
>
> **This is a FOUNDATIONAL doc.** Refs, Search, Notif, Agent Fabric, OLAP, GDPR/Audit, and all five
> subsystems read the contracts in §5/§6. They are **stable** (ADR-01): changing one is a whole-workspace PR.
>
> **No ADR is reversed.** Phase 5 is confirmation + additive sharpening + freezing the open encodings the
> Phase-4 docs assumed but never pinned. The two names/units anchors (the `EventEnvelope` field list + units
> `00 §2.10`/§3.1; the `ArtifactRef` token table §6.2) are **unchanged** and remain the authority everything
> aligns to (directive X-5).

---

## Changes vs Phase 3 (every change, with its driver)

The Phase-3 Event Bus design is carried forward intact. The deltas Phase 5 applies are:

1. **NEW event tokens registered (contract 2.9).** `ci.check.updated` and `ci.result` (the Git↔CI check
   seam, X-1/OQ-A) and the `issue`-family type token `initiative` (ISS) are added to the §6 taxonomy. The
   grammar is unchanged; these are sanctioned extensions. (§6.3, §6.4.)
2. **NEW `ci.check.updated` / `ci.result` emit shapes pinned to the check seam.** The Bus carries the
   CI-owned `CheckStatus` fact (small, PII-free, references-not-payloads) and the CI-derived `ci.result`
   rollup *signal* the merge-queue durable workflow waits on. The Bus owns the envelope + aggregate-ordering
   guarantees these ride; the `CheckStatus` *shape* and the gate are owned by CI/Git (contract 5.9). (§4.12.)
3. **SHARPEN → NEW firehose resume-cursor protocol (contract 3.5, OQ-J).** The Phase-3 firehose was
   `publish`/`tail` only. Phase 5 freezes a **`subscribe(stream, scope, cursor?)` / `resume(stream, scope,
   last_seq)`** resume-cursor subscription with a per-`(stream, scope)` monotonic `seq`, `(last_seq, now]`
   backfill on reconnect (loses zero ops), `resync_required` fallback to `*.snapshot`, and **bounded-scope
   discipline** (never `*`; `board:`/`doc:`/`channel:`). Co-designed once; ISS/KN/CHAT use it identically.
   (§4.3, §5.5.)
4. **SHARPEN: `EventMatcher` = the frozen `myelin-query` `QueryAst` (contract 3.4, OQ-C).** Phase 3 said the
   matcher *is* the query-AST predicate core; Phase 5 freezes the byte-identical grammar (`And/Or/Not/Cmp/In/
   Has/Text/Ref`, ops `eq..within`). No per-subsystem CEL. The `Has`/`Ref`/`In` predicates over **projection
   state** express CI's `on: pull_request`/`issue.transitioned` filters and Issues' `arm_trigger` relational
   condition ("all `blocked_by` resolved"). One grammar, four compile targets. (§4.5.)
5. **SHARPEN: Trigger `condition` is a `QueryAst` over projection state (contract 3.3).** Same freeze — the
   stateful per-person promise's condition is the same AST, evaluated over projection state. (§4.6.)
6. **CONFIRM + sizing: firehose sized for the heaviest producers.** `ci.log.appended` (heaviest), KN collab
   op-stream + presence, Chat live delivery + presence + agent streaming partials all ride the firehose; the
   durable bus carries only pointer/summary events. (§4.3.)
7. **CONFIRM: per-aggregate ordering at production QPS** — per-**ref** (Git push QPS) and per-**conversation**
   (Chat total order) are restated as the D-9 drill targets; the outbox `UNIQUE(aggregate, seq)` makes outbox
   order == state-change order. (§2.2, §2.3.)
8. **CONFIRM: sub-artifact-granular `replay(scope, since)` → `*.snapshot` (contract 2.6).** CI one-run scope,
   KN page-subtree at block granularity. Unchanged from Phase 3; restated because three subsystems depend on
   sub-artifact granularity. (§4.9.)
9. **CONFIRM: the per-surface shed budgets (OQ-K) the Bus's reactive tier inherits** — the named v1 floors
   (CI-surge / collab op-stream / connection-storm / agent-mention-storm). The Bus owns the discipline; the
   numbers are each subsystem's P4 call. (§4.7, §8 D-7.)
10. **Cross-cell pointer bridge frame pinned (contract 12.6, OQ-I).** The Phase-3 §7.4 floor's bridge frame
    is now the frozen `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution is always
    cell-local. Still designed-not-built (single-home-cell is v1). (§7.4.)

Everything else — the JetStream-class transport decision, the transactional outbox + relay, the idempotent
consumer template, Signal curation, the four primitives, retention/crypto-shred/tombstones, reindex-from-
source, the cell topology — is **unchanged from Phase 3** and cited where it stands.

---

## 0. Reading map & status

- **§1** — purpose & responsibilities; the four-primitive model (ADR-19). *Unchanged.*
- **§2** — the durable streaming log: transport, partitioning, ordering. *Unchanged; QPS targets restated.*
- **§3** — data model: envelope, outbox, Signal/Trigger/Automation stores. *Unchanged.*
- **§4** — algorithms: relay, idempotent consumers, Signal curation, Trigger state machine, the
  `EventMatcher`/AST (**sharpened to the frozen `QueryAst`**), the firehose split (**new resume-cursor
  protocol**), retention/crypto-shred, reindex-from-source, **the check seam emit (new §4.12)**.
- **§5** — contracts exposed to other shared systems. **Stable.** The firehose contract gains
  `subscribe/resume/scope`.
- **§6** — the canonical event taxonomy + `ArtifactRef` token table. **New tokens registered.**
- **§7** — scaling/sharding in the cell topology. *Unchanged; cross-cell frame pinned.*
- **§8** — failure modes + drills. *Unchanged.*
- **§9** — cited prior art. *Unchanged.*
- **§10** — open questions remaining for Phase 6.

**Floors named up front** (VISION §3 / EI-04 §4): single-cell event propagation is built; **cross-cell
fan-out is designed-not-built** (§7.4, frame now pinned, contract 12.6). The column-store/time-series seam for
the highest-volume durable streams is **specified-not-built** (BUS-6, §7.5; promotion = measured volume). The
transport selection is **decided (JetStream-class) with a written escape hatch**, not foreclosed.

---

## 1. Purpose, responsibilities, and the four-primitive model

**Unchanged from Phase 3 §1.** Summarised here; cite Phase-3 §1 for the full text.

### 1.1 What `myelin-events` owns

The **canonical, versioned, ordered, durable stream of domain events** — Myelin's nervous system (EI-02 §3) —
and the **trigger/automation engine** on it (ADR-08.5: automations and agents are one engine, different action
handlers). It owns: the common envelope + schema evolution; the **transactional outbox + relay** (the only
sanctioned emit path, BUS-2); the durable streaming log with per-aggregate ordering + at-least-once delivery +
durable pull consumers (BUS-1); the **firehose split** (now with the resume-cursor protocol, §4.3); the four
reactive primitives (ADR-19); the `EventMatcher` (= the frozen `QueryAst`, ADR-07); the canonical taxonomy +
token table (§6); retention + crypto-shred + tombstones; reindex-from-source re-emit (BUS/REF-4/SEARCH-1).

It does **not** own: the durable-workflow engine (`myelin-flow`, ADR-09 — automations *invoke* it); permission
decisions (Id decides; the bus carries `actor`/`visibility` hints only); the typed relation tables (Refs/
subsystems, REF-1); the dispatch/agent runtime (Agent Fabric — the bus delivers to its inbox); the
`CheckStatus` *shape* and the merge gate (CI/Git own those, contract 5.9 — the bus only carries the events
they ride, §4.12).

### 1.2 The four primitives (ADR-19) — don't collapse them

**Unchanged from Phase 3 §1.2.** Each has a different author, lifetime, store, and failure mode:

| Primitive | What it is | Author | Lifetime | Store |
|---|---|---|---|---|
| **Event** | A fact: "X happened." | The producing subsystem (via outbox) | Immutable, append-only, bounded retention | The durable log (§2) |
| **Signal** | A curated, deduplicated, severity-ranked subset events to react to. | A Signal rule | Derived; Signal stream + inbox row | Signal stream + `signal` (§3.4) |
| **Automation rule** | A stateless, per-event reflex the project owns: "when X, do Y." May invoke a workflow. | A project admin | Definition persists; each firing stateless | `automation_rule` (§3.5) |
| **Trigger** | A stateful promise a person owns: "wait until C, then unblock/remind." Fires once per arming. | A person | Persists until resolved/stale/disarmed | `trigger` (§3.6) |

The collision resolution (the old matcher→target *binding* is a **subscription/automation binding**; "Trigger"
= the per-person promise) is unchanged. **Subscriptions** are the infra-level firehose bindings (Search
indexer, Refs builder, Notif router, OLAP loader, Audit recorder — the excepted full-firehose consumers,
BUS-4). Everything product/reactive subscribes to **Signals**, not the raw firehose — the upstream defence
against the head-of-line-blocking gotcha (EI-03 §6.1).

---

## 2. The durable streaming log

**Unchanged from Phase 3 §2.** Restated tersely; the QPS targets are now Phase-5 drill gates (§8).

### 2.1 Transport: JetStream-class (BUS-1; default chosen, escape hatch written)

**NATS JetStream is the reference default transport for the durable bus**, deployed self-hosted inside each
cell (Phase-3 §2.1). Raft-replicated streams (Ongaro & Ousterhout 2014) give per-stream durability without
ZooKeeper/KRaft weight; durable **pull** consumers + queue groups are first-class; subject-based addressing
maps onto the dotted taxonomy (§6) with server-side filtering — the literal mechanism for "whitelist the
subjects you handle, never `*`" (BUS-3); single-binary deploy fits one-cell self-host parity (ADR-11);
per-subject TTL + `purge` is the crypto-shred substrate (§4.8). Kafka/Redpanda is the **reserved per-cell
upgrade** (BUS-6) when measured throughput outgrows JetStream; PG is used for the **outbox** (the emit seam),
never as the inter-service bus. The transport sits behind a thin `BusTransport` trait (`put/consume/ack/
purge`), so the swap is a relay-target change, not a consumer rewrite. **No non-durable fire-and-forget path
exists** (EI-02 §4; BUS-2).

### 2.2 Partitioning: partition key = aggregate; per-aggregate ordering (ADR-04.2)

**Unchanged.** The partition key is the **aggregate** — one PR, one issue, one CI run, one doc, one channel.
All events for a single aggregate are totally ordered; **global order is explicitly NOT promised** (EI-02 §3).
Causally-related events share an aggregate rather than relying on cross-aggregate order. The JetStream subject
encodes both routing and ordering key:

```
evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>
   └─ stream filter ─┘   └──────── ordering partition (aggregate) ───────┘
```

A stream is provisioned **per (tenant, subsystem)** (cell-local), partitioned internally by `aggregate_id`.
The relay publishes per-aggregate in outbox commit order.

### 2.3 Per-aggregate ordering at production QPS — the two named adversarial cases (D-9)

Two subsystems named per-aggregate ordering at production QPS as load-bearing (change-requests §2); both are
**CONFIRM** and become the D-9 drill (§8):

- **Git push (per-ref).** A force-push or rapid push sequence to **one ref** must deliver `git.ref.updated` in
  push order. The aggregate is **the ref, not the repo**:
  `evt.<tenant>.git.ref.<repo_id>:<ref_name>.updated` — per-ref ordering (what matters: "the current tip of
  `main`") while different refs of the same repo fan out in parallel (throughput). The git subsystem's ref-
  update transaction is the linearisation point; the outbox row is written **in that same transaction**
  (BUS-2), so outbox order == ref-update order by construction.
- **Chat (per-conversation total order).** A busy channel must deliver `chat.message.created` in send order.
  The aggregate is **the conversation/channel**; the same outbox `UNIQUE(aggregate, seq)` invariant gives the
  total order Chat assumes at scale.

This is the "partition by the entity whose order you actually need" lesson (Kreps 2011). The outbox `seq` is
the per-aggregate monotonic tiebreaker; consumers dedup on `event_id` and order on `(aggregate, seq)`.

---

## 3. The data model / schemas

**Unchanged from Phase 3 §3.** The envelope, outbox, dedup, Signal, Automation, and Trigger schemas are
carried forward verbatim; only the cross-references they serve are sharpened (the matcher AST, §4.5; the
Trigger condition, §4.6). The two table-level changes are noted inline.

### 3.1 The common envelope (ADR-13.2; the names/units anchor — UNCHANGED)

Every event carries the versioned, references-not-payloads envelope from Phase-3 §3.1. **It is the names/units
authority and is unchanged** (`00 §2.10`, directive X-5). The fields, restated for the consumers who compile
against it: `event_id` (ULID, the idempotency anchor), `type` (canonical dotted name, §6), `schema_ver`;
`occurred_at`/`recorded_at` (RFC-3339 UTC, two clocks); `tenant`/`region` (the partition + routing key);
`actor{principal, kind∈{human|agent|service}, on_behalf_of, session, run}`; `subject` (the aggregate's
`ArtifactRef`) + `aggregate` (the ordering key); the **nested** causality triad `correlation_id` (causal
root)/`causation_id` (immediate parent)/`depth` (root=0, child = parent+1 — the loop-cap counter, derived from
the cause, correct by construction, BUS-5); the GDPR/visibility routing
`contains_personal_data`/`data_role`/`visibility` (a hint, **never** an authz decision — Id decides)/
`pii_key_ref` (`kms://<tenant>/<dek-epoch>/<class>`); and the producer-owned versioned `payload` (small,
references-not-payloads).

Envelope evolution is **forward-only** (STOR-2): new optional fields only; consumers ignore unknown fields; a
removed/renamed field is a new `schema_ver` with a registered upcaster (§4.10).

### 3.2 The outbox table (per producing service; BUS-2) — UNCHANGED

Each subsystem owns an `outbox` table in its own database, written in the same transaction as the state change
(EI-02 §4); the relay drains it. Schema is Phase-3 §3.2: `(event_id UNIQUE, aggregate, seq, subject, envelope,
tenant, ...)` with `UNIQUE(aggregate, seq)` as the source-of-truth ordering invariant. `seq` is allocated
per-aggregate inside the producing transaction so it reflects true commit order. This is contract 2.3.

### 3.3 Consumer dedup ledger; 3.4 Signal store; 3.5 Automation store; 3.6 Trigger store — UNCHANGED

All four are Phase-3 §3.3–§3.6 verbatim:
- **`consumer_dedup`** `(consumer, event_id)` PK — presence == "already handled" (contract 2.5).
- **`signal_rule` / `signal`** — the curated/deduped/severity-ranked stream + window collapse (contract 3.1).
- **`automation_rule`** — the stateless reflex; `action.kind = workflow` invokes `myelin-flow` (contract 3.2).
- **`trigger`** — the stateful per-person promise; `condition` is a matcher AST (now the frozen `QueryAst`,
  §4.6); `stale_after` is a `myelin-flow` durable timer (contract 3.3).

Two notes: (a) the Trigger `condition` and Automation/Signal `matcher` columns store the frozen `QueryAst`
serialisation (§4.5) — no schema change, the JSON shape is now pinned. (b) No new tables are introduced; the
check seam (§4.12) rides ordinary `outbox` rows.

---

## 4. The algorithms

### 4.1 The outbox relay (FOR UPDATE SKIP LOCKED; BUS-2) — UNCHANGED

The stateless, horizontally-replicable relay (Phase-3 §4.1): claim a batch
(`FOR UPDATE SKIP LOCKED`, ordered `aggregate, seq`), then per aggregate in `seq` order
`transport.put(subject, envelope, dedup_id = event_id)` (JetStream `Nats-Msg-Id = event_id` for broker-side
dedup), mark `published_at` on ack, retry on failure, dead-letter to `dlq.<tenant>.<subsystem>` after N
attempts with a Signal alert, GC published rows after 24h. The textbook outbox/CDC pattern (Richardson 2018;
Kleppmann 2017 ch. 11); a Debezium-class WAL relay is a drop-in alternative behind the same trait if polling
becomes the bottleneck.

### 4.2 At-least-once + idempotent consumers (the shared template; BUS-3) — UNCHANGED

The `myelin-events` crate ships **the only sanctioned consumer template** (Phase-3 §4.2; `no-raw-publish`/
no-raw-subscribe lints). It encodes the four EI-03 §6 gotchas: (1) **whitelist subjects, never `*`** — the
upstream defence against the tens-of-millions-pending head-of-line stall; (2) **bind to a durable consumer by
name; never re-assert start policy on reconnect**; (3) **idempotent on `event_id`** (`INSERT … ON CONFLICT DO
NOTHING`; deterministic handlers); (4) **ack only after the work is durably enqueued** (at-least-once to the
next stage); (5) **`term` non-retryable junk** (don't burn the redelivery budget); (6) **bounded prefetch +
bounded handler pool + per-tenant in-flight caps** (fairness, the agent-surge case); (7) **consumer lag
(`num_pending`) is an exposed telemetry survival signal** (contract 1.8). At-least-once + dedup ledger +
deterministic handlers ≈ **effectively-once** — we do not chase true exactly-once (Helland 2007/2012).

### 4.3 The firehose split + the resume-cursor protocol (ADR-04.5; OQ-J — SHARPENED → NEW protocol)

The durable bus must **not** carry CI log lines, chat presence/typing/read-state, or collab op-streams the
same way — their volume would melt the durable bus and starve control-event ordering (EI-02 §3; ADR-04.5). The
**split is unchanged**; the **subscription protocol over the firehose is new** (OQ-J).

**The split (CONFIRM + sizing for the heaviest producers, change-requests §2).** The durable bus carries only
**"data available/updated" pointer events**; an agent is **never woken per log line** (EI-03 §6.1):

| Stream class | Transport | What rides it (sized for the heaviest producer) | Pointer event on the durable bus |
|---|---|---|---|
| **CI logs** | Append-mostly log/object tier (range-read + tail) | `ci.log.appended` frames — **the heaviest firehose producer** | `ci.log.available` (pointer: "lines N..M ready at `<ref>`") |
| **Chat presence/typing/read-state + live delivery + agent partials** | Ephemeral NATS core (presence) + the resume-cursor tier (live delivery) | presence/typing frames; live message fan-out; agent streaming partials | none (presence) / coarse `chat.read_state.updated` |
| **Collab op-streams (KN)** | The **resume-cursor durable tier** (KN-1, built FIRST) | CRDT/op frames with resume cursors | `knowledge.doc.updated` pointer |

**The resume-cursor subscription protocol (NEW, frozen — OQ-J).** Co-designed **once** for a huge board
(ISS), a hot doc (KN KD-8), and a hot channel (CHAT); all three use it identically. This is the doctrine's
"build the durable resume-cursor transport FIRST, the CRDT slots into it" (EI-04 §2.2, KN-1):

```
subscribe(stream, scope, cursor?) → SubStream   // stream e.g. fan.<tenant>.<channel>; scope BOUNDS the frames
SubStream yields Frame { seq: u64, ... }         // seq is per-(stream, scope) monotonic
resume(stream, scope, last_seq) → backfill (last_seq, now] then live   // the gap is replayed, never lost
```

- **Resume cursor (loses zero ops).** Every frame carries a per-`(stream, scope)` monotonic `seq`. On
  reconnect the client sends `last_seq`; the transport **backfills `(last_seq, now]`** from a bounded firehose
  retention window, then resumes live. A reconnect **loses zero ops** — the T-5 "reconnect-loses-zero-ops"
  drill is the pass condition. If `last_seq` is older than the retention window, the client gets a
  **`resync_required`** signal and falls back to a full `*.snapshot` replay (§4.9, sub-artifact-granular) —
  the cold-rebuild path, **named, not silent**.
- **Per-view scope bounding (the head-of-line + cost discipline).** `scope` is a **bounded selector, never
  `*`**: a board subscribes to `scope = board:<id>` (the issues in *that* board's current filter), a doc to
  `scope = doc:<id>` (its block subtree), a channel to `scope = channel:<id>`. The transport **rejects an
  unbounded/over-broad scope** (the whitelist-not-`*` rule, BUS-3, generalised to the firehose). A huge board
  **paginates its scope** (the visible window + a margin), so a 50k-row board does not stream 50k live frames
  to one client. The client declares the bounded slice it is looking at; the firehose delivers only that
  slice's frames + presence.
- **Backpressure.** Per-connection in-flight frame caps; over-cap sheds in the firehose's own bounded queue
  (EI-02 §5); a slow consumer is dropped to `resync_required` rather than buffering unboundedly. The
  per-surface shed budgets are §4.7 / OQ-K.

The collab transport's resume-cursor + idempotent-apply property and the CRDT are **Knowledge's deliverable**
(KN-1); `myelin-events` provides the **pointer-event seam + the firehose `publish`/`tail`/`subscribe`/`resume`
API** and the protocol above, not the CRDT.

### 4.4 Signal curation / dedup / severity-ranking (ADR-19) — UNCHANGED

A **Signal engine** (an infra consumer on the raw `evt.*` firehose — one of the excepted firehose consumers)
evaluates each event against enabled `signal_rule`s (Phase-3 §4.4): **match** the rule's `EventMatcher`
(§4.5); **severity-rank** (`info<notice<warning<error<critical`); **dedup within window**
(`dedup_key = render(tpl, envelope)`; `ON CONFLICT … count = count+1` — N identical failures collapse to one
Signal with `count=N`, the storm-control primitive Notif relies on); **auto-resolve** (a resolving matcher,
e.g. `ci.run.passed` resolves the matching `ci.run.failed`); **publish** to `sig.<tenant>.<severity>.<rule>`.
This is the upstream defence (BUS-4): product consumers and agents subscribe to curated Signals, so the
raw-firehose stall surface is limited to the handful of infra indexers designed for full volume.

### 4.5 The `EventMatcher` = the frozen `myelin-query` `QueryAst` (ADR-07; OQ-C — SHARPENED → frozen)

**Decision (Phase 3, now frozen byte-identical).** The `EventMatcher` **is** the predicate core of the shared
`myelin-query` `QueryAst` (contract 3.4 / 13.3), serialised as JSON, evaluated by a custom bounded interpreter
— **NOT raw CEL or JSONLogic.** Phase 3 committed the *decision*; Phase 5 freezes the *grammar* so the matcher,
saved views, Search compile, and Notif prefs cannot drift (OQ-C / X-3). The frozen grammar (`00 §X-3`):

```
QueryAst =
  | And([QueryAst]) | Or([QueryAst]) | Not(QueryAst)
  | Cmp { field: FieldPath, op: Op, value: Literal }
  | In  { field: FieldPath, values: [Literal] }
  | Has { field: FieldPath }                   // relation / multi-select membership
  | Text{ query: String, fields: [FieldPath] } // compiles to FT on the search backend
  | Ref { field: FieldPath, target: ArtifactRef }
Op = eq | ne | lt | lte | gt | gte | contains | starts_with | within   // `within` = relative date range
Literal = Str | Num | Bool | Date | Principal | Ref | Null
```

**One grammar, four compile targets** (OLTP, Search FT, the `EventMatcher` interpreter, Notif prefs). For the
matcher specifically it compiles to: (a) a JetStream **subject filter** where possible (cheap server-side
prefilter), then (b) the **bounded interpreter** over the envelope for the residual predicate.

**It expresses the relational/projection-state conditions the subsystems named (CONFIRM/RECONCILE,
change-requests §2)** — *without* a per-subsystem CEL or trigger DSL:
- CI's `on: pull_request` / `issue.transitioned` filters → `Cmp`/`In` over `event.type` and envelope fields.
- Issues' `arm_trigger` over `issue_relation` **projection state** ("all `blocked_by` resolved") → `Has`/`Ref`
  predicates over the projection the Trigger condition reads (§4.6). The relational condition is a membership
  test over projection state, not a join the matcher executes.

**Safety properties (the bar AG-7 set, unchanged).** No user-defined functions, no loops, no unbounded
recursion; every operator is total and **statically cost-bounded** (the validator rejects an AST whose
worst-case cost exceeds a budget); evaluation is side-effect-free. It is **permission-aware by construction**
(ADR-07): a matcher always composes with `list_objects(viewer, read, type)` (now the frozen `SetExpr`
push-down, contract 4.3) so no matcher/saved-view/search surface can select artifacts the subject can't see.
We borrow CEL's safety *discipline* without adopting CEL-the-surface (one parser, one validator, one
DoS-hardening surface — ADR-07).

### 4.6 The Trigger state machine — `condition` is a `QueryAst` over projection state (ADR-19 — SHARPENED)

**Unchanged state machine; the condition encoding is frozen.** `armed → {resolved | stale | disarmed}`, fires
`on_resolve` **once per arming** (Phase-3 §4.6):

- **armed → resolved**: the Trigger-engine consumer matches `condition` (the frozen `QueryAst`, §4.5) against
  an incoming event/projection update; transitions atomically
  (`UPDATE trigger SET state='resolved', resolved_by=:event_id WHERE id=:id AND state='armed'` — the guard
  makes it fire-once under concurrent events); then runs `on_resolve` (notify/tool/workflow) carrying causality
  (the resolving event is the cause). The `condition` may be a relational/projection-state predicate ("all
  `blocked_by` resolved") — the `Has`/`Ref` predicates over the Issues `issue_relation` projection (TE-7,
  contract 5.5) express it; the Trigger reads projection state, not a join.
- **armed → stale**: a `myelin-flow` durable timer set to `stale_after` fires (contract 9.3 — the minute-bucket
  timer wheel; cheap disarm/re-arm of a precomputed `fire_at`, the ISS ask, change-requests §7).
- **armed → disarmed**: the owner cancels.

Re-arming creates a new arming (idempotency is per-arming). The durability of the `stale_after` timer is
delegated to `myelin-flow`, not reinvented (SLA timers — millions of durable timers — share that substrate).

### 4.7 The reactive/dispatch tier (separately-reviewed; D7) — UNCHANGED + per-surface shed budgets (OQ-K)

The tier that consumes Signals and **dispatches agents/automations** is a stateful exception with its own
explicit design (Phase-3 §4.7; the orchestrator EI-03 §6 warns about). Its disciplines, on top of the §4.2
consumer template, are unchanged:

- **Thread causality nested, not flat** (BUS-5): a dispatched action derives
  `causation_id = dispatch_event.event_id`, carries `correlation_id`, `depth = +1`. Flat threading is
  forbidden (it breaks depth-capping).
- **Structural loop guards** (AG-6 / EI-03 §5.3): **self-guard** (drop an event whose `actor.principal` == the
  consumer's agent), **reference gate** (only a structured `artifact_ref` content node can re-trigger, never
  raw typed text — wired to ADR-05's "only `artifact_ref` nodes emit `refs.edge.created`"), **causal-depth
  ceiling** (drop/park when `depth > ceiling`, default 12), and a **shared-causal-root tripwire** (>K events
  on one `correlation_id` in a short window → per-tenant circuit breaker).
- **Bounded dispatch worker pool that drops over-cap** (AG-6): a mention/event storm is bounded; over-cap
  dispatches are dropped (with a Signal), never forked unboundedly. **Explicit-first dispatch (CHAT-1):** a
  mention *notifies*, it does not auto-spawn a costed run; implicit auto-dispatch is L-3 (counsel-gated).
- **Reserve/settle cost gate** (D8 / CI-2): every dispatched run passes the universal reserve/settle gate
  (contract 11.7) before the Agent Fabric/CI runner starts it — no balance → no execution.

**Per-surface shed budgets the tier inherits (OQ-K — CONFIRM, named floors).** ADR-16 (backpressure +
protected human lane + shed order speculative → batch/CI → agent → human-last) stands. The v1 budget *floors*
relevant to the Bus's reactive tier (the concrete numbers are each subsystem's P4 call, asserted by the
drills): the **agent-mention storm** sheds the agent lane with `429 + Retry-After` (the runtime honours it,
ADR-16.3); humans never queue behind agent runs; an unbounded one is the cascade (EI-02 §5). The full table is
`00 §OQ-K`; the Bus owns the discipline (the dispatch worker pool + per-tenant in-flight caps, §4.2), not the
numbers.

The bus *delivers* to the Agent Fabric's `EventInbox` (the Fabric owns the runtime); this tier owns the
*matching, guarding, and rate-limiting* between Signal and inbox.

### 4.8 Retention + crypto-shred + tombstones (ADR-04.4; ADR-12; EI-04 §1) — UNCHANGED

The append-only log vs GDPR erasure tension is resolved by the **references-not-payloads + crypto-shred +
tombstone** triad (Phase-3 §4.8): (1) the envelope carries IDs/`ArtifactRef`s + a **pseudonymous**
`actor.principal`; personal data lives in the producing subsystem's erasable store, so erasing the person
**tombstones the identity, not the fact** (most events: `contains_personal_data = false`). (2) Bounded
retention (JetStream `MaxAge`; default 90 days hot; OLAP/Audit holders are the durable long-term record).
(3) The rare inline-PII event is envelope-encrypted with a `pii_key_ref` (per-tenant, optionally per-subject
DEK); erasure = destroy the key. (4) A `*.erased` tombstone lets live consumers degrade gracefully; the bus is
a `PersonalDataHolder` (contract 2.7).

**This is the bus's instantiation of the ONE platform erasure posture (contract 10.9, X-7).** The free-text/
immutable residual (third-party PII a person typed into another's content) is **handled per the platform
posture in `00-reconciliation §X-7`** — not restated here (the structural floor ships; the residual basis is
`[OPEN — LEGAL]`, flagged for counsel/DPO). The bus solves the *event-log* half; git-history author/email
erasure is the named co-owned deliverable (pseudonymous-by-default + the audited history-rewrite limit).

### 4.9 Reindex-from-source re-emit (BUS/REF-4/SEARCH-1; sub-artifact-granular — CONFIRM) — UNCHANGED

Every derived store (Search, Refs, OLAP, Notif read-models) is reconstructible by asking each owner to re-emit
through the live consumer path — the index **never reads owner DBs**, so steady-state and recovery use one code
path and cannot drift (EI-04 §5.3). The re-emit protocol (Phase-3 §4.9; contract 2.6):

```
reindex(scope) →  for each owning subsystem in scope:
   subsystem.replay(scope, since=<cursor>) → emits *.snapshot via the SAME outbox→bus path
   → the same live consumers (Search/Refs/OLAP/Notif) ingest idempotently (event_id dedup)
```

`*.snapshot` events carry the same envelope and are idempotent on a deterministic `event_id` from
`(aggregate, version)`, so re-running a reindex is safe. **Sub-artifact-granular** (the change-requests §2 ask,
CONFIRM): **CI one-run scope**, **KN page-subtree at block granularity** — so Search re-indexes / Refs
re-derives at sub-artifact granularity (the `#sub` resolution ladder, contract 5.7, degrades over the same
granularity). This is the **only** recovery path (no bespoke "read the index from Postgres" backdoor), the
schema-upcaster backfill path, the new-consumer bootstrap path, **and** the `resync_required` fallback target
for the firehose resume-cursor protocol (§4.3).

### 4.10 Schema evolution / upcasting (forward-only; STOR-2) — UNCHANGED

`schema_ver` is per-producer-per-type; evolution is **expand → migrate → contract** (Phase-3 §4.10). Expand
(optional fields under a new `schema_ver`; consumers ignore unknowns); upcast (`(type, from_ver) → to_ver` pure
functions at consume; an un-upcastable `schema_ver` is `term`'d to DLQ, never silently dropped); contract
(retire the old version only after a reindex-from-source backfill re-emits old aggregates). No rollback
migrations (you can't un-emit). Contract 2.8.

### 4.11 Telemetry contract (the Phase-5 drill survival signals) — UNCHANGED

`myelin-events` emits (contract 1.8): **consumer lag** (`num_pending` per durable consumer — the §4.2
head-of-line signal), **outbox depth + age**, **relay publish + dead-letter rate**, **per-aggregate publish
latency** (`recorded_at`→broker-ack), **dedup hit-rate** (effectively-once health), **per-tenant in-flight**
(fairness/agent-surge), and **causal-depth histogram + shared-root-tripwire counter** (loop-safety). These are
the assertions the §8 drills read.

### 4.12 The Git↔CI check seam — what the Bus carries (NEW; X-1/OQ-A)

The most load-bearing cross-subsystem seam (X-1) is **specified in `00 §X-1` and owned by CI (producer) + Git
(gate)** as contract 5.9. The Bus's role is narrow and additive — it carries two new event flows; it does not
own the `CheckStatus` shape or the merge gate.

1. **`ci.check.updated` (the per-context fact).** CI emits it **via the outbox only** (BUS-2). The envelope
   carries the CI-owned `CheckStatus` struct in `payload` (small, PII-free — references-not-payloads: it
   carries `run`/`details_ref` `ArtifactRef`s, not log bytes). The envelope shape is the canonical one (§3.1)
   with `subject = repo#commit-<oid>/check-<context>` (a `#sub` sub-anchor, contract 5.7) and **`aggregate =
   (repo, commit_oid)`** so all checks for one commit are **per-aggregate ordered** (ADR-04.2). Git's
   `check_status` consumer is built from the §4.2 idempotent template (idempotent on `event_id`) and applies
   CI/Git's `run_attempt` supersession rule (`00 §X-1`) — the Bus guarantees the ordering + at-least-once
   delivery the supersession rule relies on; it does not evaluate the rule.
2. **`ci.result` (the rollup signal the merge queue waits on).** A **CI-derived rollup**, distinct from the
   per-context `ci.check.updated` events: the events drive the always-visible PR-checks UI (via Git's
   projection); the single `ci.result` signal drives the **merge-queue durable workflow's resume** via
   `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` (contract 9.4 — the workflow holds no runtime
   while CI runs for hours). Both are emitted by CI via the outbox; `ci.result` rides the durable bus as the
   signal payload `{ commit_oid, overall: success|failure, contexts: [CheckContext], idem_token }`.

These are the two **new tokens** registered in §6 (`ci.check.updated`, `ci.result`). The Bus owns: their
envelope conformance, per-aggregate ordering on `(repo, commit_oid)`, at-least-once delivery, and the durable
`wait_for_signal` substrate (via `myelin-flow`). It does **not** own: the `CheckStatus` fields, the
`(commit_oid, context)` last-writer-wins / `run_attempt` supersession, the `trust_tier`/fork-endorsement
gating, or the merge gate — all CI/Git (contract 5.9, `00 §X-1`). This is "a shaping, not a new engine."

---

## 5. Contracts / APIs exposed to other systems (the glue — STABLE)

What Refs, Search, Notif, Agent Fabric, OLAP, GDPR, and the subsystems compile against. Field names + units are
the §3.1/§6 anchors; changing them is a breaking, whole-workspace change (ADR-01). **The single change vs
Phase 3 is the firehose contract (5.5), which gains `subscribe/resume/scope`.**

### 5.1 The envelope (§3.1) — the primary contract. UNCHANGED (contract 2.1).
The envelope **is** the contract (ADR-13.2). Non-negotiable fields in §3.1; `type` taxonomy + `subject` token
grammar in §6. The names/units anchor (directive X-5).

### 5.2 Emit (outbox-only; BUS-2) — UNCHANGED (contract 2.2).
```rust
// in the SAME db transaction as the state change — the ONLY emit path
events::outbox(tx).emit(Envelope { type_, subject, payload, actor, /* causality auto-derived from cause */ })?;
```
No `publish_now()` / fire-and-forget. Causality is derived from the triggering event (BUS-5).

### 5.3 Subscribe / consume (the template; BUS-3) — UNCHANGED (contract 2.4).
```rust
events::consume(ConsumerSpec { durable, subjects /* explicit whitelist, never `*` */,
                               max_ack_pending, per_tenant_inflight }, handler)   // §4.2 template
```
Infra indexers may take the firehose; product/reactive consumers take `sig.*` Signals (BUS-4).

### 5.4 Signals / Automations / Triggers (ADR-19) — UNCHANGED surface; matcher/condition = the frozen `QueryAst`.
```rust
events::define_signal_rule(SignalRule { matcher /* QueryAst */, severity, dedup_key_tpl, dedup_window })  // 3.1
events::register_automation(AutomationRule { matcher /* QueryAst */, action, run_as, delegation, budget, gates }) // 3.2
events::arm_trigger(Trigger { owner, condition /* QueryAst over projection state */, arms_subject, on_resolve, stale_after }) // 3.3
events::disarm_trigger(id)
```

### 5.5 Firehose + the resume-cursor protocol (the dedicated transport; §4.3) — SHARPENED → NEW (contract 3.5).
```rust
firehose::publish(stream, frame)                       // CI log frame, presence frame, collab op
firehose::tail(stream, range) -> Stream                // range-read / tail (CI log viewer)
firehose::subscribe(stream, scope, cursor?) -> SubStream   // NEW: per-view, BOUNDED scope (never `*`)
firehose::resume(stream, scope, last_seq) -> SubStream     // NEW: backfill (last_seq, now] then live; loses zero ops
// resync_required signal → fall back to a `*.snapshot` replay (§4.9)
// the durable bus carries the POINTER event (ci.log.available / knowledge.doc.updated) via §5.2 emit
```
`scope` is a bounded selector (`board:<id>`/`doc:<id>`/`channel:<id>`); the transport rejects unbounded scopes
(BUS-3 generalised). ISS/KN/CHAT use this identically (OQ-J).

### 5.6 Reindex-from-source (BUS/SEARCH-1; §4.9) — UNCHANGED (contract 2.6).
```rust
events::reindex(scope)   // asks owners to replay through the live path; scope is sub-artifact-granular
// a subsystem implements: fn replay(scope, since) -> emits *.snapshot via outbox
```

### 5.7 PersonalDataHolder (ADR-12; the bus is a holder) — UNCHANGED (contract 2.7).
```rust
impl PersonalDataHolder for EventBus {
  fn locate(subject) -> inline-PII events + tombstone status;
  fn erase(subject)  -> crypto-shred inline-PII keys + emit *.erased tombstones; // §4.8, per the §X-7 posture
  fn export(subject) -> the subject's events (references resolved via owners);
}
```

### 5.8 Replay / ops — UNCHANGED.
```rust
events::replay(correlation_id)        // re-drive a causal tree for debugging (E-6 investigate-before-build)
events::dlq(stream).inspect|requeue|drop
```

---

## 6. The canonical event taxonomy + `ArtifactRef` token table

### 6.1 Dotted-name scheme (TE-10) — UNCHANGED

`type = <subsystem>.<artifact_type>.<event_name>` — lowercase, dot-separated, **singular** tokens,
**past-tense** verbs (`opened`, not `open`). Two segments minimum, three when an artifact type clarifies.
Tokens match `[a-z][a-z0-9_]*`; lifecycle verbs are consistent
(`created/updated/deleted/merged/closed/reopened/erased`). Subsystem-prefixed is canonical (Phase-3 §6.1).

### 6.2 The `ArtifactRef` subsystem/type token table (the names anchor) — UNCHANGED + one new type token

`ArtifactRef = myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`. The canonical singular subsystem tokens are
unchanged (Phase-3 §6.2): `git`/`ci`/`issue`/`knowledge`/`chat`/`identity`/`refs`; CLI aliases (`repo`/`doc`/
…) are a render-time projection only. Refs is the validator, not a second authority.

**New type token (sanctioned §6.2 extension, change-requests §2):** **`initiative`** — a ranked `issue`-family
type (`myelin://<tenant>/issue/initiative/<id>`). No new subsystem token. (The unified `#sub` sub-anchor
vocabulary `comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-` is **Refs-owned**,
frozen in contract 5.7 / `00 §X-4`; the Bus's `subject` field references it but does not own it.)

### 6.3 New event tokens registered (the X-1 check seam) — NEW

Added to the seed taxonomy (contract 2.9), validated against the §6.1 grammar:

- **`ci.check.updated`** — a `(commit_oid, context)` `CheckStatus` fact; `aggregate = (repo, commit_oid)`
  (§4.12). The producer of the Git merge-gate projection.
- **`ci.result`** — the CI-derived rollup the merge-queue durable workflow waits on as a signal (§4.12,
  contract 9.4).

### 6.4 Representative event names (the seed taxonomy; subsystems complete in 5-B) — UNCHANGED + the new tokens

`git.pr.opened|updated|closed|merged|reopened|marked_ready`, `git.ref.updated`, `git.review.submitted`,
`git.comment.created`; `ci.run.started|passed|failed|cancelled`, **`ci.check.updated`** (new),
**`ci.result`** (new), `ci.log.available` (pointer), `ci.artifact.published`;
`issue.issue.created|updated|transitioned|closed`, `issue.initiative.created|updated` (new type token),
`issue.relation.created`; `knowledge.page.created|updated`, `knowledge.doc.updated` (pointer),
`knowledge.row.updated`; `chat.message.created`, `chat.read_state.updated` (coarse);
`identity.permission.granted|revoked`, `identity.member.added`; `refs.edge.created|removed`; plus the
cross-cutting `*.erased` tombstone and `*.snapshot` reindex events. **Each subsystem owns its full list as a
5-B deliverable**, validated against this grammar.

---

## 7. Scaling / sharding in the cell topology (ADR-11)

**Unchanged from Phase 3 §7**, except the cross-cell bridge frame is now pinned.

### 7.1–7.3 In-cell first; stream sharding; firehose sharded separately — UNCHANGED
The bus is cell-local; heavy cross-system work (Refs build, Search index, OLAP rollup) is async off the bus,
never synchronous in the write path (ADR-11.5). Streams are per (tenant, subsystem), partitioned by
`aggregate_id`; a hot tenant scales by more partitions, a hot cell by adding JetStream nodes (Raft-replicated
streams rebalance). The tenant is the blast-radius + fairness unit (per-tenant in-flight caps, §4.2). The
firehose (CI-log/presence/collab) rides its own transport so it never contends with control-event ordering;
it scales on its own axis (object-tier throughput for logs; ephemeral fan-out for presence; the resume-cursor
tier for collab/live, §4.3).

### 7.4 Cross-cell propagation (FLOOR — designed-not-built; frame now PINNED, contract 12.6 / OQ-I)

A multi-cell tenant needs cross-cell event propagation. **Still a named floor, not built in v1** (single-home-
cell is v1). The Phase-3 design seam is unchanged but the **bridge frame is now frozen** (`00 §OQ-I`):

```
CrossCellPointer { subject: OpaqueSubjectId, type: ArtifactType, correlation_id: CorrelationId, home_cell: CellId }
```

The control plane (PII-free, ADR-11.4) carries **only** this pointer between a tenant's cells — never payload
or PII (residency-preserving, the `control-plane-pii-free` lint). **Resolution is always cell-local:** a viewer
in cell A wanting to render a pointer homed in cell B has **cell B** `resolve(ref, viewer, mode)` *in B*,
permission-checked *in B*, returning only the already-rendered, already-permission-filtered projection (or a
tombstone) — never raw rows, never PII that should stay in B (EI-02 §1; ADR-11). The single-cell path is
complete; the §5 contracts are cell-agnostic so this extends without a rewrite. Follow-on owner: the control
plane / multi-cell tenancy (contract 12.6).

### 7.5 The column-store/time-series seam (BUS-6 — specified-not-built) — UNCHANGED
The highest-volume durable streams keep a seam for a column-store/time-series engine (ClickHouse-class,
aligning with the OLAP read store, ADR-10) — **do not add it before the volume is measured** (BUS-6; EI-04
§5.2). Promotion trigger: a measured per-stream volume the JetStream tier serves at degraded latency. Until
then the 90-day-hot log + OLAP long-term holder suffices.

### 7.6 Stateful-component register + blast radius — UNCHANGED (Phase-3 §7.6)
JetStream streams (Raft-replicated, per-(tenant,subsystem), aggregate-partitioned — one cell degrades, outbox
buffers, no loss); per-service outbox tables (in each subsystem's PG — emit stalls, no loss, relay drains on
recovery); `consumer_dedup` ledgers (re-process is idempotent → safe); Signal/Trigger/Automation stores
(reactive tier degrades, Events unaffected); firehose transport (presence/logs degrade, never affects durable
control events — the split's whole point). Everything else (relay workers, Signal engine, dispatch tier,
consumers) is stateless and replaceable — recoverable by reconnecting to the durable log + reindex-from-source.

---

## 8. Failure modes + the drills owed — UNCHANGED (Phase-3 §8) + the check-seam ordering note

Each property that can fail names the quantified drill (EI-01 P3; T-2). The bus owes (each a Phase-5 scorecard
item, T-4):

| # | Property / failure mode | Drill (quantified gate) | Reads (§4.11) |
|---|---|---|---|
| D-1 | **Zero loss across reconnect** (the headline) | Kill a consumer mid-stream + sever the broker connection during sustained publish; on reconnect (bind-by-name, immutable start policy) assert **0 lost, 0 duplicate effects**. | consumer lag, dedup hit-rate |
| D-2 | **Consumer-lag / head-of-line stall** | Flood a `*`-subscribed (incorrect) consumer with unhandled types; assert the whitelist-template consumer does **not** stall while the naive one does. Gate: **lag bounded, alarm fires**. | `num_pending` |
| D-3 | **Replay correctness** | Replay a `correlation_id` tree; assert deterministic, idempotent re-drive, causality preserved. Gate: **replay == original, exactly once**. | dedup, causal-depth |
| D-4 | **Outbox no-ghost / no-loss** | Crash a producer between state-commit and relay-publish; assert delivered (outbox survived) and **never** delivered without the state change. Gate: **0 ghost, 0 lost**. | outbox depth+age |
| D-5 | **Reindex-from-cold parity** | Wipe a derived store; `reindex(scope)`; assert the rebuild byte-matches live. Gate: **cold == live**. | snapshot dedup |
| D-6 | **Causal-loop tripwire** | Adversarial self-triggering automation; assert depth ceiling + shared-root tripwire trip the per-tenant breaker. Gate: **halts ≤ ceiling; breaker trips**. | depth histogram, tripwire |
| D-7 | **30× agent-surge / fairness** (OQ-K) | 30× agent publish surge on one tenant; assert the human/control lane holds, the agent lane sheds (429+Retry-After honoured), **other tenants unaffected**. Gate: **human-lane latency within budget; cross-tenant unaffected**. | per-tenant in-flight, shed counters |
| D-8 | **Crypto-shred reaches the log** | Erase a subject; assert inline-PII events unrecoverable + `*.erased` tombstones emitted; consumers degrade. Gate: **0 recoverable PII in log/backups; tombstones present**. | holder erase receipts |
| D-9 | **Per-aggregate ordering at production QPS** (§2.3) | Burst force-pushes to one hot ref **and** a burst of sends to one hot channel under load; assert `git.ref.updated` / `chat.message.created` delivered in order per aggregate, parallel across aggregates. Gate: **per-ref + per-conversation order preserved at target QPS**. | per-aggregate publish latency |
| D-10 | **Firehose reconnect loses zero ops** (NEW; OQ-J / T-5) | Drop a `subscribe`d connection mid-stream on a hot board/doc/channel; `resume(last_seq)`; assert `(last_seq, now]` backfilled then live, **zero ops lost**; an out-of-window `last_seq` yields `resync_required` → `*.snapshot` fallback. Gate: **0 ops lost; resync path correct**. | firehose seq gaps, resync count |
| D-11 | **Check-seam ordering + supersession** (NEW; X-1) | Emit interleaved/late-arriving `ci.check.updated` for one `(repo, commit_oid)` across contexts + re-run attempts; assert per-`(repo, commit_oid)` aggregate ordering holds so Git's `run_attempt` supersession is well-defined; a stale lower-attempt re-delivery is droppable. Gate: **aggregate order preserved; supersession deterministic**. | per-aggregate publish latency, dedup |

D-10 and D-11 are the two new drills Phase 5 adds (the firehose resume-cursor protocol and the check-seam
ordering the Git/CI gate relies on). D-1..D-9 are unchanged from Phase 3.

---

## 9. Cited prior art — UNCHANGED (Phase-3 §9)

- **Log-structured / consensus transport.** Ongaro & Ousterhout, *In Search of an Understandable Consensus
  Algorithm (Raft)*, USENIX ATC 2014 (JetStream's replicated streams). Kreps, Narkhede, Rao, *Kafka: a
  Distributed Messaging System for Log Processing*, NetDB 2011; Wang et al., *Building a Replicated Logging
  System with Apache Kafka*, VLDB 2015 (partition-as-ordering). Kreps, *The Log* (2013). NATS JetStream docs
  (durable pull consumers, queue groups, subject filtering, `Nats-Msg-Id` dedup, `MaxAge`/`purge`).
- **Outbox / CDC.** Richardson, *Microservices Patterns* (2018) ch. 3 (transactional outbox + polling
  publisher + log tailing). Debezium (WAL-based relay). Morling, *Reliable Microservices Data Exchange with
  the Outbox Pattern* (2019).
- **Idempotency / effectively-once.** Helland, *Idempotence Is Not a Medical Condition* (ACM Queue 2012) +
  *Life beyond Distributed Transactions* (CIDR 2007). Kleppmann, *Designing Data-Intensive Applications*
  (2017) ch. 11.
- **Causality / provenance.** Lamport, *Time, Clocks, and the Ordering of Events* (CACM 1978) — the basis for
  the nested `causation_id`/`depth` derivation. Sigelman et al., *Dapper* (Google 2010); W3C Trace Context
  (propagating causal context in headers, BUS-5).
- **Predicate / matcher safety.** Google CEL spec — the non-Turing-complete, statically-cost-bounded,
  total-function discipline borrowed for the `EventMatcher` (without adopting CEL-the-surface, ADR-07).
- **Resume-cursor / live transport.** The resumable-subscription pattern (a per-stream monotonic offset + a
  client-supplied resume cursor + a snapshot fallback) is the standard CDC/replication consumer model
  (Kleppmann 2017 ch. 11; Kafka consumer offsets) generalised to a per-view bounded scope — the OQ-J protocol.
- **State machines / durable timers.** Temporal/Cadence durable-execution literature (the `stale_after` timer
  + automation-invoked workflows, ADR-09 / `myelin-flow`).
- **Doctrine.** EI-02 §3 (durable streaming log), §4 (transactional outbox), §5 (backpressure), §6
  (causality); EI-03 §2 (four primitives), §6 (orchestrator gotchas); EI-04 §1 (erasure vs immutability),
  §2.2 (build the resume-cursor transport first), §5.2 (event-volume seam), §5.3 (reindex-from-source).

---

## 10. Open questions remaining for Phase 6

Most Phase-3 §10 open questions are now **closed** by the reconciliation (the matcher↔AST grammar TE-6 →
frozen `QueryAst`, §4.5; the cross-cell bridge frame → pinned, §7.4; the `actor.kind` agent-vs-service question
→ resolved in Id). What remains, honestly, for Phase 6 (roadmaps) and the 5-B subsystem rewrites:

1. **Per-subsystem full event taxonomy completion (5-B).** §6 seeds the grammar + the new check-seam/
   `initiative` tokens; each subsystem (Git/CI/Issues/Knowledge/Chat) owns its **complete** dotted-name list,
   `schema_ver` lineage, and payload shapes, validated against the §6.1 grammar. Not a Bus risk — a per-
   subsystem completion task.
2. **Firehose retention-window sizing for the resume-cursor backfill (§4.3 / D-10).** The `(last_seq, now]`
   backfill is bounded by a retention window; the **window size per stream class** (CI log vs collab op vs
   chat live) is a **measured-not-predicted** tunable — too short forces `resync_required` (expensive
   `*.snapshot`), too long costs storage. Floor: the window must exceed the p99 reconnect gap; tuned by the
   D-10 drill. Named, not numbered, in v1.
3. **Column-store promotion threshold (BUS-6; §7.5).** The measured per-stream volume that promotes a durable
   stream to the ClickHouse-class tier — deferred until volume is *measured* (EI-04 §5.2). A named seam.
4. **Signal-rule authoring UX + the default curated rule set.** Which events are Signals by default, severity
   assignment, and the admin authoring surface (the Zapier-class builder over the frozen `QueryAst`) —
   product/UX-shaped, owned with the design language in 5-B + Phase 6.
5. **The cross-cell pointer-event bridge build (FLOOR; §7.4).** The frame is frozen (contract 12.6); the
   *build* (per-viewer resolution latency, the residency proof that no PII crosses, the multi-cell fan-out) is
   the control-plane / multi-cell tenancy follow-on — designed-not-built, named.

`[OPEN — LEGAL]` carried (the bus's share): the audit-log / event-log retention carve-out (GD-5) and the
inline-PII crypto-shred reach into backups are part of the **one** platform erasure posture (contract 10.9,
`00 §X-7`) — the structural floor ships; counsel/DPO ratifies the residual basis. The bus does not restate the
posture; it instantiates it by reference (§4.8).

---

## 11. Cross-references
- [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) — X-1 (check seam, §4.12), OQ-C
  (`QueryAst`, §4.5), OQ-J (firehose protocol, §4.3), OQ-K (shed budgets, §4.7), OQ-I (cross-cell frame,
  §7.4), X-7 (erasure posture, §4.8).
- [`contract-index.md`](./contract-index.md) — contracts 2.1–2.9 (envelope/outbox/consumer/taxonomy), 3.1–3.6
  (signals/automations/triggers/matcher/**firehose 3.5**), 5.9 (the check seam the Bus carries).
- [`../03-shared-systems-architecture/event-bus.md`](../03-shared-systems-architecture/event-bus.md) — the
  Phase-3 base this refines (superseded).
- Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-04/07/11/12/13/16/19); doctrine EI-02 §3–§6, EI-04 §1/§2.2/§5.
