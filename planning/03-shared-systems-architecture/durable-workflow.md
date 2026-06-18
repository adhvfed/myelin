# Phase 3 — Durable-Workflow Substrate (`myelin-flow`)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md).
> Doctrine (binding): [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> (EI-02) §3/§4/§5/§6/§8/§10, [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
> (EI-03) §5 (approval/cost/loops/storms), §1 (brain/hands strategy boundary), §6 (orchestrator gotchas),
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (EI-04) §1 (erasure),
> §5.2 (event volume).
> Spine: **ADR-09** (durable-execution semantics; *resolves TE-20 build-vs-adopt here*), ADR-08 (Agent
> Fabric; workflow owns budget/gates/state, agent reasoning + tool calls are activities), ADR-11 (cells /
> residency / self-host parity), ADR-12 (GDPR holders / crypto-shred), ADR-13.2 (envelope), ADR-16
> (backpressure), ADR-17 (fail-static), ADR-04/ADR-19 (bus + four primitives — automations *invoke* this
> engine; it does not own the bus). Directives: X-1…X-5, BUS-2/BUS-5, STOR-1/2/3/4, ID-1/2, GD-3, AG-6/AG-8.
> Consumes the foundational P3 contracts: [`00-platform-substrate.md`](./00-platform-substrate.md)
> (`serve(AppSpec)`, the consumer template, the resilient client, `OutboxTx::emit`, `FailStatic`,
> `PersonalDataHolder`), [`identity-and-access.md`](./identity-and-access.md) (`mint_run_token`,
> `delegation`, `check`, `revoke`), [`event-bus.md`](./event-bus.md) (envelope, outbox-only emit, the
> Signal/Trigger/Automation tiers, the firehose split, `arm_trigger`'s `stale_after` timer).
>
> **What this doc decides (the prompt's mandate).** The BUILD-vs-ADOPT call for the durable-execution
> substrate (TE-20) under explicit EU-sovereignty weighting (no US-hosted managed service; self-hostable
> in-cell): **BUILD a thin Postgres-native durable-execution engine (`myelin-flow`), DBOS-class, *not*
> self-hosted Temporal, *not* an off-the-shelf Rust library** (§2 carries the full written why). It then
> details the data model, the replay/journaling algorithm, the durable-timer wheel that powers millions of
> SLA timers (SC-11), the SIGNAL mechanism for HITL waits of days (AG-8), the contracts exposed/consumed,
> cell-topology scaling/sharding, the failure modes + quantified drills, and the open questions for Phase 4.
>
> **Status convention.** *DECIDED* = committed for P4/P5. *FLOOR* = partial answer shipped with a named
> follow-on. *[OPEN → P4/P5/LEGAL]* = handed forward. Every failable property names the drill that proves
> it (Phase 5 executes; this doc enumerates the obligation — PROVE-IT / T-4).
>
> **Floors named up front (VISION §3 / EI-04 §4):** single-cell durable execution is built; **cross-cell
> workflow spanning (a workflow whose activities touch two cells of a multi-cell tenant) is
> designed-not-built** (§7.4, floor → P4 control plane). A **history-archival/compaction tier** for very
> long-lived workflows is **specified-not-built** (§7.5; promotion trigger = measured history growth). The
> engine is **BUILT**, but its determinism guard and the activity-sandbox seam reuse the unified runner
> (ADR-20) — they are not reinvented here.

---

## 0. Reading map

- **§1** — purpose, responsibilities, the one-paragraph thesis, what it is *not*.
- **§2** — the BUILD-vs-ADOPT decision (TE-20) with EU-sovereignty weighting + cited prior art.
- **§3** — the data model / schemas (workflow run, history/journal, timers, signals, activity ledger).
- **§4** — the algorithms (deterministic replay vs journaling, the timer wheel, the signal round-trip,
  the activity execution + retry, idempotency, GDPR erasure on history).
- **§5** — contracts exposed & consumed (the stable glue surface).
- **§6** — the workflow↔agent mapping (ADR-08: workflow owns budget/gates/state; reasoning + tools are
  activities) and the HITL approval-card round-trip (joint with Chat + Agent Fabric).
- **§7** — scaling/sharding in the cell topology (millions of timers, SC-11).
- **§8** — failure modes + the drills owed.
- **§9** — cited prior art.
- **§10** — required changes to foundational systems.
- **§11** — open questions for Phase 4.

---

## 1. Purpose, responsibilities, and the one-paragraph thesis

### 1.1 What `myelin-flow` owns

`myelin-flow` is the **durable-execution substrate**: it runs **deterministic workflow orchestration**
over **non-deterministic, retryable, sandboxed activities**, with **durable timers** and **durable
signals**, so a multi-step, long-running, partially human-gated process **survives crashes, restarts, and
multi-day waits without holding a process or a thread** (ADR-09; EI-03 §5.1 "wire the approve→resume loop
end to end"; `agent-native-design.md §3.2`). Concretely it owns:

1. The **workflow run** lifecycle and its **durable execution history (journal)** — the append-only,
   replay-or-recover-from source of truth for every workflow's state (§3.1–3.2, §4.1).
2. The **durable timer** primitive — `sleep_until` / `sleep_for`, the substrate for **HITL-wait
   deadlines**, **SLA timers at world scale** (SC-11: millions of timers), automation back-off, and the
   bus's stateful-`Trigger` `stale_after` (event-bus §3.6/§4.6 *delegates its durability to us*) (§3.3, §4.2).
3. The **durable signal** primitive — an external, durably-buffered message that unblocks a waiting
   workflow (`wait_for_signal`), the mechanism behind a HITL approval that may arrive **days** later
   (§3.4, §4.3; AG-8).
4. The **activity** primitive — a single unit of non-deterministic work (an agent reasoning `step`, a tool
   `exec`, an HTTP call, a DB write in another subsystem) executed **at-least-once with idempotency**,
   retried on failure, its result journaled (§3.5, §4.4).
5. The **workflow definition registry** — the deterministic functions, versioned, that the engine drives
   (§3.6, §4.6 versioning).
6. The **budget/gate/state ownership** (ADR-08): the workflow is the durable holder of a run's
   `RunBudget`, its HITL `gates`, and its accumulated state; the **reserve/settle cost gate** (D8/CI-2) is
   the workflow's opening and closing act (§6.2).

It is a **shared system in its own right** (ADR-09 §Consequences) deployed **in-cell** (ADR-11),
self-hostable, EU-sovereign. It is `kind`-agnostic about *what* a workflow does: a CI pipeline-of-stages,
an agent run with HITL gates, a multi-subsystem automation (CI-fail → triage → open issue → link → post
chat → propose PR), and a bare SLA timer are all the *same* substrate with different definitions.

### 1.2 What `myelin-flow` is NOT

- **Not the event bus.** Automations *invoke* a workflow (event-bus §3.5 `action.kind = workflow`); the
  bus is the nervous system, this is the *muscle that remembers*. The bus delivers the event that *starts*
  or *signals* a workflow; the workflow's own state changes emit events back **through the outbox** (BUS-2),
  never by a side channel. No circular sync dependency (EI-02 §3): the bus calls `myelin-flow` to start a
  workflow; `myelin-flow` reacts to the bus only by consuming a signal-bearing event, and emits only via
  the outbox.
- **Not the agent runtime.** The Agent Fabric owns `AgentRuntime::step` / `ToolHands::exec` (substrate
  §2.4); here those are **activities**. We never name a model, a prompt, or an SDK (ADR-08.2).
- **Not the authorization engine.** Every activity that has an effect calls Id's `check`/`delegation`
  through the resilient client (ADR-03; AG-5: a denied effect is an ordinary activity error, never a
  privileged path).
- **Not the sandbox.** Activities that run untrusted code (agent tool calls, CI steps) run on the **one
  unified runner** (ADR-20 / CI-1); `myelin-flow` *schedules* the activity and *journals* its result — the
  isolation is the runner's, drilled on a real kernel before any customer code runs (E-9).
- **Not a general task queue.** It is durable *execution* (stateful, replay-recoverable orchestration),
  not fire-and-forget jobs. A stateless per-event reflex stays a stateless automation (event-bus §3.5); it
  does **not** become durable execution (ADR-19 / event-bus §4.7 "a trivial reflex does not become durable
  execution").

### 1.3 One-paragraph thesis

*A Myelin workflow is a deterministic function whose every non-deterministic interaction with the world —
a timer firing, a signal arriving, an activity returning — is **journaled to Postgres in the same
transaction-discipline as the rest of the platform**, so the workflow's entire state is reconstructible by
re-running the function and short-circuiting each already-journaled step. Because the journal lives in the
service's own Postgres and a workflow's terminal effects emit through the very same outbox every other
service uses, durable execution inherits the platform's correctness primitives (transactional outbox,
idempotent consumers, crypto-shred, tenant-partitioning, fail-static) for free instead of bolting on a
second, divergent state machine.* This is the DBOS-class insight (§2) made Myelin-native.

---

## 2. The BUILD-vs-ADOPT decision (TE-20) — DECIDED

ADR-09 committed the **semantics** (Temporal-style: deterministic orchestration + retryable activities +
durable timers + signals) and left **build-vs-adopt vs self-hosted Temporal vs Rust-native library** as a
P3 decision with explicit sovereignty weighting (Temporal-the-cloud-service disallowed). This is that
decision.

### 2.1 The decision

**BUILD a thin, Postgres-native, embedded durable-execution engine — `myelin-flow`, DBOS-class — as a
Rust crate over each service's existing Postgres, *not* self-hosted Temporal, *not* an off-the-shelf Rust
library.** The semantics (ADR-09) are adopted verbatim; the *substrate* is built to sit on the platform
the rest of Phase 3 already mandates.

### 2.2 The candidates, weighed (the written why the directives require)

The dominant axis is **EU-sovereignty + in-cell self-host parity (ADR-11)**: the same artifacts must build
a managed cell and a customer's on-prem cell, and **no US-hosted managed service** is permissible. The
secondary axes are **operational surface** (every additional stateful engine is permanent cost — EI-02 §8),
**outbox/transaction coherence** (durable state and the emitted event must commit together — BUS-2, EI-02
§4), and **fit to the agent/HITL/SLA-timer workload** (long waits, millions of timers, SC-11).

| Candidate | Sovereignty / self-host | Operational surface | Outbox coherence | Verdict |
|---|---|---|---|---|
| **Self-hosted Temporal** (Go server + Cassandra/PG persistence + history/matching/frontend services) | EU-deployable, self-hostable → **satisfies ADR-11's hard constraint** | **Heavy**: a multi-service Go cluster + a dedicated datastore *per cell*, including the smallest self-host cell. A second operational universe beside our PG + JetStream + object store. (EI-02 §8: justify every engine.) | Workflow state lives in **Temporal's** store, our domain state in **ours** → a **dual-write / two-source-of-truth seam** between Temporal history and our outbox; closing it (idempotent activity that writes our DB + emits via our outbox) is exactly the coordination we'd have to build *anyway*. | **Rejected as default.** Sovereignty passes, but it imposes the heaviest per-cell operational footprint precisely where self-host parity hurts most, and it does *not* remove the outbox-coherence work — it adds a second state machine *outside* our transaction. Kept as a **documented escape hatch** (§2.4). |
| **Off-the-shelf Rust library** (e.g. WASM-runtime durable engines; nascent from-scratch projects) | Self-hostable | Lighter, but **immature**: the Rust durable-execution ecosystem in 2025–26 is early (Windmill is a *product* not an embeddable library; WASM-runtime experiments and from-scratch engines exist but are unproven at our scale and would still need our envelope/outbox/tenant/GDPR integration grafted on). | Would still need to be taught our outbox + crypto-shred + tenant-partition discipline. | **Rejected.** We would inherit an external project's roadmap and still write the integration; the integration *is* the hard part. |
| **DBOS-class: BUILD embedded over our own Postgres** | Self-hostable **by construction** — it is a library inside our service, persisting to the **Postgres the cell already runs**. Zero new per-cell engine. **Strongest ADR-11 fit.** | **Lightest**: no new stateful engine, no new datastore. A new "cell" is still PG + JetStream + object store + the unified runner. (EI-02 §8 "smallest set that works"; ADR-11 self-host parity.) | **Native**: every step's journal row commits **in the same Postgres transaction** as the step's own DB writes and its outbox row — the DBOS exactly-once property. Durable state and emitted event are **one commit**, closing the dual-write seam BUS-2 exists to close, *without a second store*. | **CHOSEN.** |

### 2.3 Why DBOS-class wins for Myelin specifically

The decisive prior art is the convergence (2025–26) on two durable-execution implementation families —
**deterministic replay** (Temporal, Restate, Azure Durable Functions: re-run the workflow, short-circuit
journaled steps) and **Postgres-embedded journaling** (DBOS: every step result committed to a Postgres
table, durability and the step's own DB writes in one transaction). Both use "a dedicated and sequenced log
of steps per workflow invocation, compared against the log on recovery" (Vanlightly 2025). Myelin already
*has* the per-service Postgres, the transactional-outbox transaction, and the idempotent-consumer ledger.
The DBOS family **reuses all three**; the Temporal family **duplicates them in a foreign store**. For a
platform whose foundational doctrine is "one DB per service, justify every engine, the outbox is the only
emit path, every store is a residency-pinned PersonalDataHolder," embedding the journal in the service's
own residency-pinned, crypto-shred-capable Postgres is not just lighter — it is the *only* option that
keeps the durable-workflow store inside the same GDPR, tenancy, and restore-consistency envelope as
everything else (ADR-11/12; STOR-4 cross-seam restore). A foreign Temporal/Cassandra store would be a
*separate* PersonalDataHolder with its *own* residency, backup, and crypto-shred story to retrofit — the
exact retrofit-pain EI-02/EI-04 warn against.

We **adopt Temporal's conceptual model wholesale** (history/event-sourcing, deterministic workflow code,
activities as the non-deterministic boundary, signals, durable timers, "if a worker crashes at step 5 of
10 another worker replays to step 6") — the literature is the design (§9). We **reject Temporal's
deployment shape** for in-cell sovereignty/operational reasons. We build the **smallest engine that
delivers those semantics on Postgres**.

### 2.4 The escape hatch (named, not foreclosed)

The engine sits behind a `DurableExecutor` trait (§5.1). If a *measured* per-cell workflow volume or
history depth outgrows the Postgres-embedded tier (the §7.5 promotion trigger), the **escape hatch is a
self-hosted Temporal/Restate-class engine bound behind the same trait** — the *contracts* (§5) are
engine-agnostic, so this is an executor swap, not a workflow rewrite. This mirrors the bus's
JetStream→Kafka escape hatch (event-bus §2.1) and STOR-1's blob-trait philosophy. Self-hosted Temporal
remains ADR-11-legal; Temporal Cloud never is.

### 2.5 The determinism constraint (the one sharp edge we inherit)

Whichever family, **workflow code must be deterministic**: no wall-clock reads, no RNG, no direct I/O, no
unordered map iteration *inside the workflow function* — every such interaction must go through an
**activity, a durable timer, or a signal** so its result is journaled (Temporal/Restate/Resonate
determinism rule; Vanlightly 2025). Myelin enforces this with:

- A **determinism lint** (`flow-determinism`, sibling to the §2.11 substrate lints): a workflow function
  may not call `SystemTime::now`, `rand`, `std::fs`/network, or `HashMap` iteration directly; it must use
  the injected `WfCtx` (`ctx.now()`, `ctx.rand()`, `ctx.sleep`, `ctx.activity`, `ctx.signal`), all of which
  journal. (DBOS/Temporal SDKs intercept these; we make it a compile-time obligation.)
- A **replay-divergence guard** at runtime (§4.1): on replay, if the workflow's next journaled command does
  not match the command the re-run code produces (a non-determinism bug or an unversioned code change), the
  engine **halts the workflow as `nondeterministic` and dead-letters it for inspection** — never silently
  diverges. This is Temporal's "non-determinism error" made a hard stop.

---

## 3. The data model / schemas

All tables live in the **`myelin-flow` service's own Postgres** (EI-02 §8: one DB per service), **tenant +
region first column, RLS-enforced, no cross-tenant query path** (EI-02 §1; ID-3), **per-tenant
envelope-encrypted, crypto-shred-capable, a `PersonalDataHolder`** (ADR-11/12; auto-registered by the
bootstrap harness, substrate §3.4). The engine is **embedded**: these tables sit beside (and commit in the
same transactions as) the workflow's terminal `outbox` writes.

### 3.1 The workflow run

```sql
CREATE TABLE workflow_run (
  tenant         uuid        NOT NULL,
  region         text        NOT NULL,
  run_id         uuid        NOT NULL,             -- ULID-ordered; the durable handle
  wf_type        text        NOT NULL,             -- registered definition name, e.g. 'agent.run', 'ci.pipeline'
  wf_version     int         NOT NULL,             -- the definition version pinned at start (§4.6)
  input          jsonb       NOT NULL,             -- references-not-payloads (IDs/ArtifactRefs, never PII bodies)
  state          wf_state    NOT NULL,             -- running | waiting | completed | failed | nondeterministic | terminated
  cursor         bigint      NOT NULL DEFAULT 0,   -- highest applied history seq (replay short-circuit floor)
  budget         jsonb,                            -- the RunBudget this workflow owns (ADR-08; D8 reserve/settle)
  -- causality (BUS-5): every workflow is caused by something; children derive from it
  correlation_id text        NOT NULL,             -- causal ROOT (carries to every emitted event)
  causation_id   text,                             -- the event that STARTED this workflow
  caused_by      text,                             -- the human session/action (distinct from causation)
  depth          int         NOT NULL,             -- inherited; the loop-cap counter (AG-6)
  partition      smallint    NOT NULL,             -- worker-shard key = hash(run_id) % N (§7.2)
  lease_owner    text,                             -- the worker currently driving this run (§4.7)
  lease_expires  timestamptz,                      -- lease TTL; expiry → another worker may steal (crash recovery)
  created_at     timestamptz NOT NULL DEFAULT now(),
  updated_at     timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant, run_id)
);
CREATE INDEX wf_runnable  ON workflow_run (partition, lease_expires)
  WHERE state IN ('running');                       -- the dispatch scan (leased work only)
CREATE INDEX wf_waiting   ON workflow_run (tenant)  WHERE state = 'waiting';
```

`input`/state are **references-not-payloads** (event-bus §3.1): a workflow about a PR carries the PR's
`ArtifactRef`, never the PR body; personal data stays in the owning subsystem's erasable store, so erasing
a person rarely touches the workflow (§4.8).

### 3.2 The execution history / journal (the source of truth)

Every non-deterministic decision is one **append-only** row. This is Temporal's Event History / DBOS's
step table / Restate's journal (§9), Myelin-native.

```sql
CREATE TABLE wf_history (
  tenant         uuid        NOT NULL,
  run_id         uuid        NOT NULL,
  seq            bigint      NOT NULL,             -- per-run monotonic; the replay order
  kind           hist_kind   NOT NULL,             -- activity_scheduled | activity_completed | activity_failed
                                                   --  | timer_set | timer_fired | signal_waited | signal_received
                                                   --  | wf_started | wf_completed | side_marker (ctx.now/rand)
  command_id     text        NOT NULL,             -- DETERMINISTIC id from (wf_type, seq-position) — replay match key
  result         jsonb,                            -- activity result / signal payload / fired-timer marker (ref-not-PII)
  result_key_ref text,                             -- envelope-encryption key id IF a result must carry inline PII
  occurred_at    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant, run_id, seq),
  UNIQUE (tenant, run_id, command_id)              -- idempotency: a command is journaled at most once
);
```

The `command_id` is **deterministic from the workflow's position** (Temporal/DBOS: the Nth `ctx.activity`
call in a given code path gets a stable id), so on replay the re-run code's Nth command matches the Nth
journaled command **by id** — the §4.1 replay-match. The `UNIQUE(command_id)` makes journaling idempotent:
a crash between "do the activity" and "journal its result" replays safely because the second attempt's
insert is a no-op (`ON CONFLICT DO NOTHING`) returning the journaled result. `result` is
**references-not-payloads**; the rare inline-PII result is **envelope-encrypted** (`result_key_ref`) so
erasure = crypto-shred (§4.8; ADR-12.3).

### 3.3 The durable timer table (powers SC-11: millions of timers)

```sql
CREATE TABLE wf_timer (
  tenant         uuid        NOT NULL,
  region         text        NOT NULL,
  timer_id       uuid        NOT NULL,
  run_id         uuid        NOT NULL,             -- the workflow to wake (or NULL for a bare SLA timer)
  command_id     text        NOT NULL,             -- the wf_history command this timer satisfies
  fire_at        timestamptz NOT NULL,             -- the durable deadline
  bucket         int         NOT NULL,             -- coarse time bucket = epoch_minute(fire_at) — the scan index
  fired          boolean     NOT NULL DEFAULT false,
  partition      smallint    NOT NULL,             -- = run's partition (co-located dispatch)
  PRIMARY KEY (tenant, timer_id)
);
-- The hot dispatch index: only the imminent, unfired bucket is scanned (§4.2). This is what makes
-- "millions of durable timers" an indexed range read, not a table scan.
CREATE INDEX wf_timer_due ON wf_timer (bucket, partition) WHERE NOT fired;
```

The `bucket` (minute granularity) + the **partial index on `NOT fired`** is the world-scale move (§7.3):
the timer-wheel worker scans only the current/overdue buckets of its partition, so a tenant can hold
millions of long-horizon SLA timers (an SLA due in 30 days sits in a far-future bucket, never touched until
its minute) at the cost of one indexed range read per minute per partition. This is the SC-11 substrate the
issue-tracker's SLA timers and the bus's stateful-`Trigger` `stale_after` (event-bus §4.6) both ride.

### 3.4 The durable signal table (powers multi-day HITL waits)

```sql
CREATE TABLE wf_signal (                            -- durably BUFFERED inbound signals
  tenant         uuid        NOT NULL,
  run_id         uuid        NOT NULL,
  signal_name    text        NOT NULL,             -- e.g. 'approval', 'cancel', 'ci.result'
  idem_key       text        NOT NULL,             -- caller-supplied; dedups a re-delivered signal
  payload        jsonb       NOT NULL,             -- the signal body (ref-not-PII; e.g. {approved:true, by:<ArtifactRef>})
  payload_key_ref text,                            -- crypto-shred key if inline PII
  received_at    timestamptz NOT NULL DEFAULT now(),
  consumed_seq   bigint,                           -- the wf_history seq that consumed it (NULL = buffered, unconsumed)
  PRIMARY KEY (tenant, run_id, signal_name, idem_key)
);
CREATE INDEX wf_signal_pending ON wf_signal (tenant, run_id) WHERE consumed_seq IS NULL;
```

A signal can arrive **before** the workflow reaches its `wait_for_signal` (durably buffered) or **after**
it has been waiting for days (the workflow is `state = waiting`, holding **no thread**; the signal arrival
flips it `running` and re-dispatches it). `idem_key` makes signal delivery at-least-once-safe (a re-posted
approval is a no-op). This is the mechanism for "an agent run may pause days on a HITL gate" (ADR-09;
AG-8) — the workflow occupies a row, not a runtime.

### 3.5 The activity ledger (idempotency for at-least-once effects)

Activity results are journaled in `wf_history` (§3.2); the **ledger** is the cross-cut that makes an
activity's *external effect* idempotent when the activity itself wrote to another subsystem or emitted an
event:

```sql
CREATE TABLE wf_activity_attempt (
  tenant       uuid NOT NULL, run_id uuid NOT NULL, command_id text NOT NULL,
  attempt      int  NOT NULL,
  idem_token   text NOT NULL,    -- passed to the activity so ITS downstream write/emit is dedup-keyed (BUS-2 dedup)
  state        text NOT NULL,    -- scheduled | running | succeeded | failed | retrying
  error        text,
  started_at   timestamptz, ended_at timestamptz,
  PRIMARY KEY (tenant, run_id, command_id, attempt)
);
```

The `idem_token` is the bridge to BUS-2: an activity that emits a domain event passes `idem_token` as the
event's dedup id, so an activity retried after a crash that *did* emit produces a broker-deduped event, not
a duplicate (event-bus §4.1 `Nats-Msg-Id`). At-least-once activity + idempotent downstream ≈
effectively-once (EI-02 §4; Helland 2012) — we do **not** chase true exactly-once.

### 3.6 The definition registry

Workflow definitions are **code** (deterministic Rust functions registered at `serve(AppSpec)` boot),
**versioned**:

```sql
CREATE TABLE wf_definition (
  wf_type text NOT NULL, version int NOT NULL,
  code_hash text NOT NULL,       -- content hash of the compiled definition (drift detection)
  status text NOT NULL,          -- active | draining | retired
  registered_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (wf_type, version)
);
```

A running workflow is **pinned to the `wf_version` it started at** (§4.6) so a deploy that changes a
definition cannot introduce a non-determinism divergence into an in-flight run (Temporal versioning; DBOS
workflow versioning). New runs use the new version; old runs replay against the old.

### 3.7 Stateful-component register (X-4)

| Component | Engine | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|
| `workflow_run` + `wf_history` | Postgres (own DB) | `(tenant, region)` + `partition` | one cell's in-flight workflows; recovered by lease-steal replay (§4.7) | per-tenant DEK; per-subject key for inline-PII history rows |
| `wf_timer` | Postgres (own DB) | `(tenant, region)` + `partition`/`bucket` | timers in one partition fire late until recovery; **no loss** (durable rows) | inherits |
| `wf_signal` | Postgres (own DB) | `(tenant, run_id)` | buffered signals delayed until recovery; **no loss** | per-subject key if inline PII |
| `wf_activity_attempt` | Postgres (own DB) | `(tenant, run_id)` | at worst an activity re-attempt (idempotent → safe) | derived |
| Worker lease state | in `workflow_run.lease_*` (PG) | partition | a dead worker's leases expire → stolen; **no loss** | n/a |

Everything else (the dispatch workers, the timer-wheel scanners, the replay engine) is **stateless and
horizontally replaceable** — recoverable by re-leasing runs and replaying history. The control plane holds
**zero in-region personal data** (ADR-11).

---

## 4. The algorithms

### 4.1 Deterministic replay / recovery (the core)

A worker **drives** a workflow by running its deterministic function with a `WfCtx` that **intercepts**
every `ctx.activity`/`ctx.sleep`/`ctx.signal`/`ctx.now`/`ctx.rand` call:

```
drive(run):
  load wf_history[run] ordered by seq          # the journal so far
  replay_pos = 0
  run the workflow function:
    on the Nth ctx.<command> call:
      command_id := deterministic_id(wf_type, position)
      if replay_pos < len(history) and history[replay_pos].command_id == command_id:
          # REPLAY: return the journaled result, do NOT re-execute the side effect
          replay_pos += 1
          return history[replay_pos-1].result
      else if history[replay_pos] exists but command_id mismatches:
          halt run as 'nondeterministic'; dead-letter        # the divergence guard (§2.5)
      else:
          # LIVE: this is new work past the cursor
          execute the command for real (schedule activity / set timer / wait signal),
          journal (activity_scheduled / timer_set / signal_waited) in a PG txn,
          and SUSPEND the workflow (yield the worker) if the command blocks (activity pending / timer / signal)
  on workflow function return → journal wf_completed, emit terminal events via OUTBOX (BUS-2), settle budget
```

The crash-recovery property (Temporal's headline, §9): if a worker dies mid-run, another worker
**re-leases** the run (§4.7) and calls `drive` — replay short-circuits every journaled step (no
re-execution of side effects, because activity *results* are journaled, not re-run) and resumes at the
first un-journaled command, "as if nothing happened." Determinism (§2.5) is what makes replay produce the
*same* command sequence; the divergence guard catches the case where it doesn't.

**Why journaled-result replay, not naive re-execution:** re-running an activity on replay would double its
side effect; journaling the *result* and returning it on replay is the whole point of the model (a step is
done at-most-once-in-effect via the `command_id` unique constraint + the activity `idem_token`). This is
the DBOS "every step result committed to Postgres, returned on recovery" property.

### 4.2 The durable-timer wheel (SC-11: millions of timers)

A **timer-wheel worker per partition** runs a tight loop (the hierarchical-timing-wheel idea — Varghese &
Lauck 1987 — applied at the DB layer via time buckets):

```
every TICK (e.g. 1s, jittered):
  due := SELECT … FROM wf_timer
         WHERE bucket <= epoch_minute(now()) AND NOT fired AND partition = :p
         ORDER BY fire_at FOR UPDATE SKIP LOCKED LIMIT :batch    -- safe across replicas (PG)
  for each due timer, in one txn:
     UPDATE wf_timer SET fired = true
     INSERT wf_history(kind='timer_fired', command_id=timer.command_id, …)   -- journal the fire
     mark run runnable (state running) and notify its partition's dispatcher
```

- **Indexed, not scanned:** the `(bucket, partition) WHERE NOT fired` partial index means a timer due in 30
  days is *never read* until its minute arrives — millions of far-future SLA timers cost nothing until due
  (§7.3). This is the SC-11 mechanism.
- **`FOR UPDATE SKIP LOCKED`** (PostgreSQL ≥ 9.5; same primitive as the outbox relay, event-bus §4.1) makes
  the wheel **safe across replica workers** — no two fire the same timer.
- **At-least-once fire + idempotent journal:** a crash between "fire" and "journal" re-fires; the
  `UNIQUE(run_id, command_id)` on `wf_history` makes the second journal a no-op. A timer is therefore
  **fired effectively-once**.
- **A bare SLA timer** (no `run_id`, or a one-shot workflow) simply emits an `sla.deadline.reached` event
  via the outbox on fire — the issue tracker's SLA-breach Signal rides that (event-bus §3.4).

### 4.3 The signal round-trip (HITL waits of days; AG-8)

```
workflow reaches ctx.wait_for_signal("approval", timeout):
  journal signal_waited(command_id); set timer(command_id, fire_at=now+timeout) for the timeout branch
  CHECK wf_signal for an already-buffered, unconsumed "approval" (arrived early):
     if present → consume it (set consumed_seq), journal signal_received(result=payload), continue driving
     else → SUSPEND: set workflow state='waiting', RELEASE the worker (hold no thread)

external post_signal(run, "approval", payload, idem_key):     # days later, from the approval card
  INSERT wf_signal ON CONFLICT (idem_key) DO NOTHING          # idempotent
  if workflow is 'waiting' on this signal → flip state='running', enqueue for dispatch
  → a worker re-leases, replay short-circuits to the wait, consumes the signal, continues
```

A workflow waiting on a HITL approval is a **row in `wf_signal_pending` + `workflow_run.state='waiting'`**,
consuming **no runtime** — the literal answer to "an agent run may pause for days" (ADR-09; EI-03 §5.1). The
**timeout branch** is a durable timer (§4.2): if the human never approves within the gate's window, the
timer fires, the workflow takes its timeout path (e.g. auto-deny + notify). The approval card UX round-trip
is §6.3.

### 4.4 Activity execution + retry (the non-deterministic boundary)

When `drive` hits a live `ctx.activity(def, input)`:

1. Journal `activity_scheduled(command_id)`; enqueue an **activity task** (bounded queue, §7.6) tagged with
   `idem_token = (run_id, command_id, attempt)`.
2. An **activity worker** runs it through the **resilient client** (substrate §6: timeout / breaker /
   bulkhead / jittered-retry-idempotent-only). For an *agent reasoning* activity it calls
   `AgentRuntime::step`; for a *tool* activity it calls `ToolHands::exec` on the **unified runner** (ADR-20);
   for a *cross-subsystem effect* it calls Id `check`/`delegation` then the target's API.
3. On success → journal `activity_completed(result)` (in a PG txn; if the activity also wrote our DB / emitted
   an event, that commit carries the `idem_token` for dedup — §3.5). Re-dispatch the run.
4. On retryable failure → bounded **full-jitter exponential backoff** (Brooker 2015), `attempt + 1`,
   re-enqueue; after N attempts → journal `activity_failed`, and the workflow's own error-handling path
   runs (compensate / fail / alternate branch). **A non-retryable error** (a `Denied` from Id — AG-5; a
   malformed input) fails immediately, no budget burned on retries.
5. **The reserve/settle cost gate** (D8/CI-2) wraps a *spend-bearing* activity: reserve before step 1,
   settle on step 3; **refuse to start when the wallet is exhausted, never interrupt one in flight** (EI-03
   §5.2). This is what makes a runaway agent self-limiting (§6.2).

### 4.5 Idempotency & the outbox seam (no second emit path)

A workflow's **terminal and intermediate domain events** are emitted **only through the outbox** (BUS-2):
`OutboxTx::emit(draft, cause)` is called *inside the same PG transaction* that journals the producing
command. Because `myelin-flow` is embedded in a service with its own outbox, the workflow's "I opened an
issue" event and the journal row that records the issue-open activity **commit together** — there is no
window where the workflow believes it opened an issue but the event was lost, or vice versa. This is the
single biggest reason the engine is embedded (§2.3). Causality is **derived from the cause** (BUS-5):
events a workflow emits carry the workflow's `correlation_id` (root), `causation_id` = the triggering
command/event, `depth + 1` — so the loop guard (AG-6) reads true provenance.

### 4.6 Workflow versioning (deploy without breaking in-flight runs)

A run is pinned to `wf_version` at start (§3.6). A deploy registers a new version; **in-flight runs keep
replaying against the version they started on** (the registry keeps old versions `draining` until their
runs drain), so a code change can never make a running workflow's replay diverge (§2.5). New runs pick the
`active` version. This is Temporal's versioning discipline; it is the safe-deploy story for long-lived
(multi-day) workflows. **Forward-only** (STOR-2): you never "roll back" a definition under a running
workflow; a fix is a new version, and old runs either drain or are migrated by an explicit, journaled
migration activity.

### 4.7 Lease-based dispatch & crash recovery

Workers claim runnable runs by **lease** (a `lease_owner` + `lease_expires` on `workflow_run`, claimed with
`FOR UPDATE SKIP LOCKED`). A worker heartbeats to extend its lease while driving. **If a worker dies, its
leases expire**, and another worker steals the run and replays (§4.1) — crash recovery with **no lost
progress** (everything past `cursor` is re-derivable; everything at/below is journaled). The lease TTL
bounds recovery latency. This is the standard durable-execution worker model (Temporal matching/history;
DBOS recovery on restart).

### 4.8 GDPR erasure on workflow history (ADR-12; EI-04 §1)

The append-only history is in tension with erasure; resolved with the platform's
**references-not-payloads + crypto-shred + tombstone** triad (event-bus §4.8):

1. **References-not-payloads (primary lever):** `input`, `result`, signal `payload` carry IDs/`ArtifactRef`s;
   the person lives in the owning subsystem's erasable store. Erasing a person **rarely touches the
   workflow** — the workflow's reference to `myelin://…/human/alice` resolves to a tombstone after erasure,
   the workflow's *structure* (audit-critical) survives. *Delete the identity, not the fact* (EI-04 §1).
2. **Crypto-shred (rare inline-PII history/signal row):** when a result/payload *must* carry inline PII
   (`*_key_ref` set), it is **envelope-encrypted with a per-subject key** (ADR-12.3); `erase(subject)` =
   **destroy the key**, rendering the journaled ciphertext (incl. in backups) unrecoverable, without
   rewriting immutable rows.
3. **`PersonalDataHolder`:** `myelin-flow` implements `locate/export/erase` (§5.5): `locate(subject)` finds
   runs/history/signals referencing the subject; `erase` crypto-shreds inline-PII keys + records the
   erasure; `export` returns the subject's workflow involvement (references resolved via owners). The
   bootstrap harness auto-registers every `myelin-flow` store as a holder (substrate §3.4) — "we forgot the
   timer table" is structurally impossible (GD-3).

A **completed** workflow's history is retained per the retention policy (tightest-policy-wins, legal-hold
aware — GD-2) then GC'd; an agent run's *trace* is separately a content-addressed Knowledge document (AG-7),
not this history.

---

## 5. Contracts exposed & consumed

### 5.1 Exposed — the durable-execution API (STABLE; X-5 reconciled)

`myelin-flow` exposes a Rust crate surface (`myelin-flow`) + an internal-RPC wire contract (ADR-02) so a
non-Rust subsystem (e.g. the Chat connection tier, TE-21) consumes the same shapes.

```rust
pub trait DurableExecutor {                          // the engine-agnostic seam (§2.4 escape hatch)
    /// Start a durable workflow. Returns immediately with a durable handle; the run survives crashes.
    fn start(&self, spec: StartSpec, cause: Option<&EventEnvelope>) -> Result<RunId>;
    /// Deliver an external signal (idempotent on idem_key) — the HITL approval, a cancel, a CI result.
    fn signal(&self, run: RunId, name: &str, payload: Json, idem_key: &str) -> Result<()>;
    /// Query a run's coarse status (NOT its internal state — that's the engine's).
    fn describe(&self, run: RunId) -> Result<RunStatus>;
    /// Cancel/terminate (runs the workflow's compensation/cancel path).
    fn cancel(&self, run: RunId, reason: &str) -> Result<()>;
}

pub struct StartSpec {
    pub wf_type: String, pub input: Json,
    pub budget: Option<RunBudget>,                   // the workflow OWNS budget/gates/state (ADR-08)
    pub idem_key: String,                            // dedups a re-issued start (at-least-once trigger)
}

// The surface a workflow DEFINITION is written against (deterministic; the determinism lint guards it):
pub trait WfCtx {
    fn activity<I, O>(&mut self, def: ActivityDef, input: I) -> Result<O>;   // journaled, retried (§4.4)
    fn sleep_until(&mut self, t: Timestamp) -> Result<()>;                   // durable timer (§4.2)
    fn sleep_for(&mut self, d: Seconds) -> Result<()>;
    fn wait_for_signal<P>(&mut self, name: &str, timeout: Option<Seconds>) -> Result<SignalOr Timeout<P>>; // §4.3
    fn now(&mut self) -> Timestamp;                  // journaled side-marker (deterministic on replay)
    fn rand(&mut self) -> u64;                       // journaled side-marker
    fn emit(&mut self, draft: EventDraft) -> Result<EventId>;               // via the OUTBOX (BUS-2), causality derived
}
```

Units (X-5, reconciled against substrate §2.10): timers/timeouts in **seconds**; timestamps **RFC-3339
UTC**; budgets **integer minor-units** (never floats — D8). `RunBudget`, `EventEnvelope`, `EventDraft`,
`ArtifactRef`, `RunId` are the canonical substrate types.

### 5.2 Consumed (the foundational contracts this engine stands on)

| Consumed | From | Use |
|---|---|---|
| `serve(AppSpec)`, the consumer template, `OutboxTx::emit`, `FailStatic`, `PersonalDataHolder`, `ResilientClient` | `00-platform-substrate` | the engine is a normal Myelin service: harness-booted, outbox-only emit, holder-registered, resilient calls |
| `mint_run_token`, `delegation`, `check`, `revoke` | Identity (`identity-and-access`) | per-run agent identity for an activity; effect authorization (AG-5); teardown revoke |
| envelope, outbox-only emit, Signal/Automation/Trigger tiers, firehose `tail` | Event Bus (`event-bus`) | start-on-event; emit-on-state-change; the bus's stateful-`Trigger` `stale_after` **delegates to our timer** |
| `AgentRuntime::step`, `ToolHands::exec`, the unified runner (ADR-20) | Agent Fabric + CI | the *activity* implementations (reasoning + tools) — sandboxed elsewhere, scheduled here |
| KMS / crypto-shred, `BlobStore` | Storage/GDPR | per-subject key for inline-PII history; large activity payloads spill to the blob store by reference |

### 5.3 Who consumes `myelin-flow`

- **Event Bus** — automations with `action.kind = workflow` call `start`; the stateful `Trigger`'s
  `stale_after` is one of our timers (event-bus §3.6/§4.6).
- **Agent Fabric** — every HITL-gated or multi-step agent run *is* a workflow (§6); reasoning + tools are
  activities.
- **CI** — a pipeline is a workflow; each stage/step is an activity on the unified runner; the
  reserve/settle gate is the workflow's bookends.
- **Issue tracker** — SLA timers (SC-11) and the "unblock me when…" Trigger UX (ISS-1) ride our timer/signal.
- **Chat** — the surface for the HITL approval card (§6.3); posts the `approval` signal.

### 5.4 Telemetry contract (X-1 — the Phase-5 drill survival signals)

Exported on the metrics-health port (substrate §10.2): **runnable-run lag** (runs `running` but unleased —
the dispatch backlog), **timer-wheel lag** (oldest overdue unfired timer per partition — the SC-11 health
signal), **activity queue depth + retry rate + dead-letter count**, **replay rate + `nondeterministic`-halt
count** (the §2.5 divergence guard firing), **signal buffer depth + oldest unconsumed wait age** (HITL
backlog), **per-tenant in-flight workflows/activities** (fairness/agent-surge), **causal-depth histogram**
(loop safety), **reserve/settle reject rate** (budget exhaustion). These are the assertions the §8 drills read.

### 5.5 `PersonalDataHolder`

```rust
impl PersonalDataHolder for FlowEngine {
  fn locate(subject) -> runs/history/signals referencing subject (by ArtifactRef) + inline-PII rows;
  fn export(subject) -> the subject's workflow involvement (references resolved via owners);
  fn erase(subject)  -> crypto-shred inline-PII history/signal keys; tombstone references; record receipt; // §4.8
}
```

---

## 6. The workflow↔agent mapping + the HITL approval-card round-trip

### 6.1 Workflow owns budget/gates/state; reasoning + tools are activities (ADR-08; ADR-09)

The ADR-09 mapping, made concrete. An **agent run is a workflow** whose deterministic body is the
**plan-then-apply loop** (ADR-08.3): it owns the `RunBudget`, the HITL `gates`, and the accumulated
conversation cursor *as durable workflow state*. The non-deterministic pieces are **activities**:

```
workflow agent_run(task):
  reserve_budget()                                   # D8 reserve; refuse-start if exhausted (§6.2)
  loop:
    outcome = ctx.activity(AGENT_STEP, conversation)             # AgentRuntime::step — the BRAIN (AG-1), an activity
    match outcome:
      UseTools(calls):
        for call in calls:
          if call.tool in gates and not approved(call):
            decision = ctx.wait_for_signal("approval:"+call.id, timeout=gate.window)  # HITL — may wait DAYS (§4.3)
            if decision.denied or decision.timed_out: record; continue            # withheld tool = ordinary error (AG-8)
          result = ctx.activity(TOOL_EXEC, call)     # ToolHands::exec on the unified runner (ADR-20) — the HANDS (AG-2)
          conversation.append(result)
      Submit(s): break
  settle_budget(); ctx.emit(agent.run.completed)     # via OUTBOX (BUS-2)
```

- **Plan-then-apply survives** (AG-1/AG-3): the brain `step` is a pure activity returning *proposed*
  tool-uses; the workflow applies them through gated activities. Mock and real runtimes are the *same*
  activity (the strategy swap is `runtime_ref`, ADR-08.2) — so the entire HITL/budget/loop story is
  **provable on the mock brain** (EI-03 §1) with zero model spend.
- **The conversation history is workflow state**, journaled implicitly via `AGENT_STEP` activity results —
  so a crashed agent run resumes mid-conversation by replay (§4.1), not by re-reasoning.

### 6.2 Cost & loop safety (EI-03 §5; AG-6)

- **Cost pre-flight is the workflow's first act** (D8/CI-2): `reserve_budget` refuses to *start* a new run
  when the wallet is exhausted, **never interrupts one in flight** (EI-03 §5.2). A spend-bearing activity
  reserves/settles per call. *A runaway agent spends down a wallet and stops — not a surprise bill.*
- **Loop safety is structural** (AG-6): every event the workflow emits derives causality from its cause
  (§4.5), so the **causal-depth ceiling** and the **shared-root-within-a-window tripwire** (read by the
  bus's dispatch tier, event-bus §4.7) see true depth/root — *a human cannot typo into a loop.* The
  workflow's own `depth` (§3.1) is the in-engine ceiling: a workflow that would spawn a child past the
  ceiling refuses.
- **Concurrency caps:** the activity queue and the per-tenant in-flight workflow cap are **bounded; over-cap
  is shed/parked, never forked** (X-3; AG-6) — a mention storm cannot fan out unboundedly.

### 6.3 The HITL approval-card round-trip (joint: Chat + Agent Fabric + this engine)

This is the EI-03 §5.1 warning — "easy to ship the withhold logic and the card but forget the bridge
between them" — designed end-to-end, with `myelin-flow` as the durable bridge:

```
1. Agent workflow hits a gated tool → ctx.wait_for_signal("approval:<call>", timeout=window)
   → emits agent.approval.requested via OUTBOX (payload: tool name, args ArtifactRefs, RISK, a LIVE COST
     ESTIMATE — EI-03 §5.1) → the bus Signal tier routes it → Notif/Chat renders the APPROVAL CARD
     (humanised at the backend — NOTIF-1 — with the pending action + risk + cost).
   The workflow is now state='waiting', holding NO runtime, for up to `window` (which may be DAYS).
2. A human clicks Approve/Deny on the card (Chat). Chat calls Id check(human, approve, run) then
   DurableExecutor::signal(run, "approval:<call>", {approved, by:<ArtifactRef>}, idem_key=card_id).
3. The signal lands in wf_signal (idempotent on card_id — a double-click is one approval). The waiting
   workflow flips to 'running', a worker re-leases + replays to the wait, consumes the signal:
     - approved → the gated TOOL_EXEC activity runs (the step re-runs WITH the tool now allowed — AG-8).
     - denied → the tool is WITHHELD (ordinary error to the loop, no mutation — AG-8); agent continues.
     - timeout (the §4.3 durable timer fired first) → auto-deny path + notify.
```

The **durability is the point**: the approval can arrive **days** after the request, across worker
restarts and deploys, and the workflow resumes exactly where it waited. The card's **live cost estimate**
comes from the reserved budget (§6.2). The approve→resume bridge is `signal` → buffered `wf_signal` →
re-lease → replay → consume — the explicit wiring EI-03 §5.1 says teams forget.

**[OPEN → P4 joint]** the approval-card *data model + visual design* (Chat + DL §11 overlay primitives) and
whether a card can approve a *batch* of gated calls — product/UX-shaped, co-owned with Chat + Agent Fabric.

---

## 7. Scaling / sharding in the cell topology (ADR-11; SC-11)

### 7.1 In-cell, tenant-partitioned (ADR-11.5)

The engine is **cell-local**; a workflow's activities target services in the same cell. Tenant + region is
the first key of every table (EI-02 §1); there is no cross-tenant or cross-cell query path. The control
plane holds zero in-region personal data (ADR-11.4).

### 7.2 Worker sharding by `partition = hash(run_id) % N`

Runs and their timers/activities are partitioned by a hash of `run_id` into N partitions; a worker owns a
set of partitions and scans only its own (`wf_runnable`/`wf_timer_due` indexes are partition-keyed). Adding
workers re-balances partitions — horizontal scale inside a cell. A **hot tenant** scales by more partitions;
a **hot cell** scales by more workers + (if measured) a dedicated read-replica for the dispatch scans (the
ID-4 "measure-before-shard" discipline applied here).

### 7.3 Millions of durable timers (SC-11) — the world-scale case

The headline scale property. SLA timers (issue tracker), HITL-wait timeouts, automation back-offs, and the
bus's `stale_after` Triggers can total **millions of concurrent durable timers** per cell. The design
(§3.3/§4.2):

- **Bucketed partial index:** a timer due far in the future sits in a far-future `bucket` and is **never
  read** until its minute. The wheel scans only `bucket <= now AND NOT fired` for its partition — an indexed
  range read whose cost is proportional to *timers due now*, not *timers outstanding*. A million 30-day SLA
  timers cost ~nothing until they approach their minute.
- **`FOR UPDATE SKIP LOCKED`** spreads the due-timer firing across replica wheel workers with no contention.
- **Measured promotion (BUS-6 analogue):** if a cell's *due-now* rate outgrows the PG-indexed wheel, the
  named follow-on is a dedicated time-series/scheduling tier (or the §2.4 Temporal escape hatch) behind the
  same `WfCtx::sleep` contract — **not added before measured** (§7.5).

### 7.4 Cross-cell workflow spanning (FLOOR — designed-not-built)

A **multi-cell tenant** (SC-2/SC-3) might want a workflow whose activities touch two cells. **Not built in
v1.** The single-cell engine is complete; the seam: the **control plane** carries a **residency-preserving
pointer bridge** (only `run_id` + `wf_type` + `correlation_id`, **never payload/PII**) so a workflow in cell
A can `signal` or `start` a child workflow in cell B, each cell journaling locally and resolving
`ArtifactRef`s per-viewer. Follow-on owner: **P4 control-plane + multi-cell tenancy (SC-2/SC-3)**, jointly
with the bus's identical cross-cell floor (event-bus §7.4). The `DurableExecutor` contract is cell-agnostic
so this extends without a rewrite.

### 7.5 History-archival/compaction tier (SPECIFIED — not built)

A very long-lived workflow (a months-long SLA, a slow multi-day HITL) accumulates history. **Promotion
trigger = measured history growth** (EI-04 §5.2 discipline): until measured, the per-run history in PG +
forward-only retention (§4.8) suffices. The named follow-on: **periodic history compaction** (snapshot the
workflow's continued-as-new state and truncate consumed history — Temporal's "continue-as-new") and/or an
**object-store archival tier** for terminal-run history (`BlobStore`, STOR-1). Specified-not-built; the
schema (`cursor`, content-hashable history) leaves the seam.

### 7.6 Bounded everything (X-3)

The activity queue, the dispatch prefetch, the per-tenant in-flight workflow/activity caps, and the DB pool
are **all bounded; fast-fail/shed on saturation** (EI-02 §5). The **principal-aware shed order** (substrate
§7) applies: a human-initiated workflow's activities ride the protected lane; an agent-storm's ride the
shed-able lane (429 + Retry-After honoured by the resilient client). Per-tenant fairness: one tenant's
workflow surge cannot starve another's (the §8 drill asserts it).

### 7.7 Stateful-component register — see §3.7 (X-4).

---

## 8. Failure modes + the drills owed (PROVE-IT; Phase 5 executes)

Each failable property names the **quantified drill** that proves it (T-2/T-4/T-5). Each emits a green
artifact when it passes; until then the property is **claimed, not proven**.

| # | Property / failure mode | Drill (quantified gate) | Reads (telemetry §5.4) |
|---|---|---|---|
| F-1 | **Crash mid-workflow loses progress** | Kill a worker at activity step 5 of 10 mid-run; assert another worker re-leases, replays, and resumes at step 6 with **zero re-executed side effects, zero lost progress, exactly-once-in-effect**. | runnable lag, replay rate |
| F-2 | **Non-determinism / unversioned-code divergence** | Replay a run against a divergent (buggy / wrong-version) definition; assert the divergence guard **halts the run as `nondeterministic` + dead-letters** — never silently diverges or double-effects. Gate: **0 silent divergence; halt + alert.** | nondeterministic-halt count |
| F-3 | **Durable timer at scale (SC-11)** | Arm **1M+ durable timers** spread over far-future buckets + a burst due in one minute; assert due timers fire **within the tick budget**, far-future timers cost ~nothing, and a worker crash re-fires unfired timers (effectively-once). Gate: **fire-latency within budget at 1M+ outstanding; 0 lost/0 double-fire.** | timer-wheel lag |
| F-4 | **Multi-day HITL signal round-trip** | Start a gated agent workflow; let it wait `state=waiting` across a **worker restart + a deploy**; deliver the `approval` signal hours/days later (double-click to test idempotency); assert it resumes, consumes once, runs/withholds the gated tool correctly. Gate: **resume-after-days works; double-signal = one approval; withheld tool does not mutate (AG-8).** | signal buffer depth, oldest-wait age |
| F-5 | **Outbox coherence (no ghost / no lost)** | Crash a workflow *between* journaling an activity that wrote our DB and emitting its event; assert the journal row and the outbox row committed **together** (one txn) — the event is delivered and never delivered without the state. Gate: **0 ghost, 0 lost.** | outbox depth (shared) |
| F-6 | **Budget exhaustion self-limits a runaway** | Drive an agent loop with a depleting wallet; assert a new spend-bearing activity is **refused at reserve** (run stops), an in-flight one is **never interrupted**. Gate: **no balance → no new spend; in-flight unharmed.** | reserve/settle reject rate |
| F-7 | **Causal-loop tripwire** | Adversarially construct a workflow→event→workflow loop; assert the depth ceiling (§6.2) + the bus's shared-root tripwire + the bounded activity pool stop it (drops/parks over-cap, never forks). Gate: **loop halts ≤ ceiling; breaker trips.** | causal-depth histogram |
| F-8 | **30× agent-surge / fairness** | 30× surge of agent-initiated workflows on one tenant; assert the human-initiated lane holds (interactive workflow latency within budget), the agent lane sheds (429+Retry-After honoured), **other tenants unaffected**. Gate: **human lane holds; cross-tenant unaffected.** | per-tenant in-flight, shed counts |
| F-9 | **Crypto-shred reaches workflow history** | Erase a subject with inline-PII history/signal rows; assert those keys destroyed (ciphertext unrecoverable incl. backups), references tombstoned, structure preserved. Gate: **0 recoverable PII; receipts present.** | holder erase receipts |
| F-10 | **Restore + cross-seam integrity (STOR-4)** | Restore `myelin-flow`'s PG to a consistent point; assert in-flight runs resume correctly and the workflow store ↔ outbox offsets ↔ referenced subsystem rows restore to **one mutually consistent point** (no run pointing at a vanished activity result). Gate: **consistent resume; no orphaned references.** | — |

---

## 9. Cited prior art

- **Durable execution / deterministic replay (the model we adopt).** Temporal architecture &
  documentation — Event History as an append-only event-sourced log per workflow execution; deterministic
  replay reconstructs in-memory state ("a worker crashes at step 5 of 10, another replays to step 6");
  activities as the non-deterministic boundary; signals; durable timers; workflow versioning. Cadence
  (Temporal's predecessor, Uber) — the original "fault-oblivious stateful programming" design. Restate —
  the *journal* framing (equivalent to Temporal's history; SDK intercepts and returns recorded results on
  replay). Azure Durable Functions — replay-based orchestration. Resonate / Vanlightly (2025),
  *Demystifying Determinism in Durable Execution* — where the determinism constraint comes from and why
  workflow code must avoid clocks/RNG/I/O outside steps.
- **Postgres-embedded journaling (the substrate we build).** DBOS — durable execution as an *embedded
  library* over Postgres; every step result committed to a Postgres table **in the same transaction** as
  the step's own DB writes, giving transactional exactly-once and removing the separate orchestration
  server + dedicated cluster — the decisive fit for ADR-11 self-host parity + BUS-2 outbox coherence (§2.3).
- **Outbox / dual-write / idempotency (the coherence we inherit).** Richardson, *Microservices Patterns*
  (2018) ch. 3 (transactional outbox); Kleppmann, *DDIA* (2017) ch. 11 (dual writes, logs, stream
  processing, exactly-once-as-illusion); Helland, *Idempotence Is Not a Medical Condition* (2012) —
  at-least-once + idempotent ≈ effectively-once. `FOR UPDATE SKIP LOCKED` (PostgreSQL ≥ 9.5) — safe
  multi-worker claim for the relay, the timer wheel, and lease dispatch.
- **Durable timers at scale.** Varghese & Lauck, *Hashed and Hierarchical Timing Wheels* (1987) — the
  bucketing idea behind the §4.2 minute-bucket partial index that makes millions of timers (SC-11) an
  indexed range read.
- **Causality / provenance.** Lamport, *Time, Clocks, and the Ordering of Events* (1978) — happened-before,
  behind the nested `causation_id`/`depth` derivation (BUS-5). Dapper / W3C Trace Context — propagating
  causal context across hops.
- **Resilience.** Nygard, *Release It!* (2nd ed.) — Circuit Breaker / Bulkhead / Timeout (the activity
  resilient-client path). Brooker, *Exponential Backoff and Jitter* (AWS, 2015) — full-jitter activity
  retry.
- **Doctrine.** EI-02 §3 (bus), §4 (outbox), §5 (backpressure), §6 (causality), §8 (justify every engine /
  forward-only migrations), §10 (blast-radius / fail-static); EI-03 §1 (brain/hands strategy), §5
  (approval/cost/loops/storms), §6 (orchestrator gotchas); EI-04 §1 (erasure vs immutability), §5.2
  (volume seam).

Sources (web-grounded, June 2026): Temporal architecture & event-history docs
([temporal.io](https://temporal.io/blog/temporal-replaces-state-machines-for-distributed-applications),
[docs.temporal.io](https://docs.temporal.io/workflow-execution/event)); Vanlightly,
[*Demystifying Determinism in Durable Execution*](https://jack-vanlightly.com/blog/2025/11/24/demystifying-determinism-in-durable-execution);
DBOS vs Temporal ([dbos.dev](https://www.dbos.dev/compare/compare-dbos-vs-temporal-dbos),
[tiarebalbi.com](https://www.tiarebalbi.com/en/blog/dbos-vs-temporal-postgres-durable-execution));
Windmill durable-engine ([windmill.dev](https://www.windmill.dev/blog/launch-week-1/fastest-workflow-engine)).

---

## 10. Required changes to foundational systems

The foundational P3 docs anticipated this engine; the required honoured-contracts are mostly *consumption*,
with three small additions to flag:

1. **Event Bus (`event-bus.md`).** Already correct: §3.6/§4.6 say the stateful `Trigger`'s `stale_after`
   timer's durability "is delegated to the durable-workflow engine (ADR-09)." **This doc binds that
   delegation:** the bus's `stale_after` is a `myelin-flow` timer (§4.2). No bus change needed; this is the
   concrete wiring. The bus's `action.kind = workflow` automation (§3.5) calls `DurableExecutor::start` —
   already specified.
2. **Identity (`identity-and-access.md`).** No change. `myelin-flow` consumes `mint_run_token` /
   `delegation` / `check` / `revoke` as specified (§12). An agent activity's per-run token is minted at the
   activity, revoked on workflow completion/teardown — the ID-2 "token life == run life" is honoured by
   tying token TTL to the *activity*, not the (possibly-days-long) *workflow*; the **workflow** holds no
   long-lived privileged token, only re-minting a short-lived one per spend-bearing activity. **This is a
   sharpening to flag:** a multi-day HITL workflow must **re-mint** its agent token when it resumes (the
   pre-wait token has expired) — a small contract note for Id's `mint_run_token` (it must be callable
   mid-workflow on resume). [Carried to §11 / Id P4.]
3. **Platform substrate (`00-platform-substrate.md`).** Add the **`flow-determinism` lint** (§2.5) to the
   §2.11 architecture-lint table — a workflow function that reads a clock/RNG/IO outside `WfCtx` fails to
   compile. This is the sibling of `no-host-exec` / `no-raw-publish`. A one-row addition; flagged for the
   substrate doc's next truth-up (E-1: code wins; the lint ships with this engine regardless).

No change is required to ADR-09 (this *implements* its directional decision and resolves TE-20 within its
stated `[OPEN → P3]`). No store outside `myelin-flow` is read; no second emit path is introduced.

---

## 11. Open questions for Phase 4

1. **HITL approval-card data model + UX** (joint: Chat + Agent Fabric + DL §11). The card's visual design,
   batch-approval semantics, and the live-cost-estimate rendering — product/UX-shaped (§6.3). `[OPEN → P4
   Chat + Agent Fabric]`.
2. **Mid-workflow token re-mint on resume** (§10.2). Id's `mint_run_token` must be callable when a
   multi-day workflow resumes (the pre-wait token expired); the exact re-mint + re-authorization-on-resume
   ergonomics co-design with Id. `[OPEN → P4 Identity + Agent Fabric]`.
3. **Cross-cell workflow spanning** (FLOOR, §7.4) — the control-plane pointer bridge for a multi-cell
   tenant's cross-cell `start`/`signal`, residency proof that no PII crosses, per-cell journaling. `[OPEN →
   P4 control-plane / SC-2/SC-3]` (joint with the bus's identical floor).
4. **History compaction / continue-as-new + archival tier** (§7.5) — the measured-growth promotion trigger,
   the continue-as-new snapshot shape, and the object-store archival of terminal-run history. `[OPEN → P4,
   measured]`.
5. **Per-cell timer-wheel promotion threshold** (§7.3) — the measured due-now rate at which the PG-indexed
   wheel yields to a dedicated scheduling tier (or the §2.4 escape hatch). `[OPEN → P4/P5, measured]`.
6. **Workflow-definition authoring for non-engineers** — whether the no-code automation builder (event-bus
   §10.4) compiles to a *constrained* workflow definition or only to stateless automations; the safe-subset
   of `WfCtx` exposable to tenant-authored automations. `[OPEN → P4 product]`.
7. **CI pipeline as a workflow — the exact stage/step activity granularity** and how the unified-runner job
   spec (`kind = ci`) maps to an activity, co-owned with CI's P4 agent (TE-13/CI-2). `[OPEN → P4 CI]`.

---

## 12. Cross-references

- [`VISION.md`](../../VISION.md) — world-scale, GDPR-by-construction, agent-native, EU-sovereign, Rust-default.
- Doctrine: [`external-insights/03-agent-native-fabric.md`](../../external-insights/03-agent-native-fabric.md)
  §5 (approval/cost/loops/storms — the workflow's safety mandate), §1 (brain/hands), §6 (orchestrator gotchas);
  [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §4/§5/§6/§8/§10;
  [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure), §5.2 (volume).
- Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) — **ADR-09**
  (this resolves its TE-20 build-vs-adopt), ADR-08 (agent fabric), ADR-11 (cells), ADR-12 (GDPR), ADR-16/17,
  ADR-04/ADR-19 (bus).
- Directives: [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md) — X-1…X-5,
  BUS-2/5, ID-1/2, GD-3, AG-6/AG-8, STOR-1/2/3/4, CI-1/2; decision-record D8 (reserve/settle).
- Foundational P3 docs this consumes: [`00-platform-substrate.md`](./00-platform-substrate.md),
  [`identity-and-access.md`](./identity-and-access.md), [`event-bus.md`](./event-bus.md).
- Consumed by (P4): Agent Fabric, CI, Issue tracker (SLA/Trigger UX), Chat (approval card).
