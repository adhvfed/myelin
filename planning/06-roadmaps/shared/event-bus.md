# Phase 6 — Roadmap: Event Bus + Trigger/Automation Engine (`myelin-events`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **event-bus** shared system. Slots into
> the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§2 bands, §3 critical-path/DAG, §4 gate
> invariant, §5 name-your-floors). Frozen architecture (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/event-bus.md`](../../05-refined-shared-systems-architecture/event-bus.md)
> (the refined Bus architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §2/§3 (the contracts the Bus owns) + §1/§5/§9 (the contracts it carries/depends on). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (SUB-D1/D2 + BUS-D1..BUS-D9) + architecture §8 (D-1..D-11, incl. the two new D-10 firehose / D-11
> check-seam drills). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability — silent data loss before any feature; §3 prove-it-or-it-isn't-real + the
> failure-injection harness; §5 the committed ratchet; §1 name-your-floors / code-wins-over-docs) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) (§1
> erasure-vs-immutability on the event log; §2.2 build the resume-cursor transport FIRST; §5.2 the event-volume
> column-store seam; §5.3 reindex-from-source). Spine: ADR-04 (delivery + firehose split), ADR-19 (four
> primitives), ADR-07 (matcher↔AST), ADR-13.2 (envelope), ADR-08.5 (one trigger engine), ADR-16 (backpressure),
> ADR-11 (cells), ADR-12 (GDPR holders). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** The Event Bus is **not a band-local
> system** — it is the **Tier-1 substrate** the master sequencing puts *inside* M0/M1 because "no event is ever
> lost or ghosted" is the silent-data-loss floor every later write rests on (master §1 Tier-1, §2 M0 work).
> Three consequences for this roadmap:
> 1. **The Bus's core ships first, not in a "messaging" band.** The `EventEnvelope` (2.1), the transactional
>    outbox + relay (2.2/2.3), the idempotent-consumer template (2.4/2.5), and three of the twelve lints
>    (`no-raw-publish`, `no-cross-sync-cycle`, plus `tenant-predicate` as it applies to streams) are **M0**.
>    They are the precondition for *every* subsystem honestly emitting anything, so they cannot wait.
> 2. **The reactive layer is M2.** Signals/Automations/Triggers (3.1–3.4), the firehose resume-cursor protocol
>    (3.5 — the thing EI-04 §2.2 says to build *first*, before the CRDT), the dispatch tier (3.6), and the
>    `ci.check.updated`/`ci.result` check-seam carriage (2.9, §4.12) land with the rest of the connective tissue
>    every subsystem projects onto.
> 3. **The hard problems are scheduled, not hand-waved.** The cross-cell pointer bridge (12.6 frame), the
>    column-store/time-series seam for the highest-volume durable streams (BUS-6), and the firehose
>    retention-window sizing are all **named floors with M5/post-M5 follow-ons** (master §5).
>
> The honest progression: **first runnable** = M0 (outbox emits, a consumer dedups, the relay drains — a
> single committed event survives a producer kill). **First useful** = M2 (Signals curate, the firehose resume
> cursor loses zero ops on reconnect, the check seam carries `ci.result`, agents and automations dispatch).
> **Production-hardened** = M5 (the 30× agent-surge family holds the human lane, per-aggregate ordering holds at
> production QPS, crypto-shred reaches backups, the cross-cell bridge + column-store seam are live/measured).

---

## 0. Where the Bus lands in the master bands (the one-paragraph map)

The Bus is the **only shared system whose core is M0, not M2** (master §2: the outbox + idempotent consumer is
the Tier-1 silent-data-loss floor that ships inside M0; the bus's own SUB-D1/SUB-D2/BUS-D4 are the M0→M1 exit
gate). M0 ships the **emit + consume substrate** (envelope, outbox, relay, consumer template, dedup, the
`no-raw-publish` + `no-cross-sync-cycle` lints, the seed taxonomy grammar). M1 adds nothing structurally new to
the Bus but **the Bus's crypto-shred/tombstone holder seam (2.7) is wired into the M1 GDPR/Storage floor**
(per-subject-DEK, the erasure ledger) and per-aggregate ordering is **tenant/region-partitioned** under the M1
tenancy key. M2 is the Bus's second large band — **Signals/Automations/Triggers, the firehose resume-cursor
protocol, the dispatch tier, the `EventMatcher` = frozen `QueryAst`, and the check-seam carriage**. M3/M4
register **per-subsystem event tokens + the live check-seam producer/consumer** but add no Bus engine. M5 is
the Bus's **world-scale hardening + floor follow-ons** (the 30× surge family BUS-D7, per-aggregate-order at QPS
BUS-D9, the cross-cell bridge build, the column-store seam promotion if volume is measured). The Bus
participates in **every M5 E2E scenario** (it is the spine all four ride) and the **M6 dogfood** (the Bus
carries the platform's own events).

**First runnable / first useful / production-hardened:**
- **First runnable (M0):** a committed state-change emits exactly one event through the outbox; a consumer
  dedups it; a producer kill between commit and publish loses nothing (SUB-D1, BUS-D4 green).
- **First useful (M2):** curated Signals drive Notif + agents; the firehose `resume(last_seq)` backfills a
  dropped board/doc/channel connection with zero ops lost (D-10); the check seam carries `ci.result` so a merge
  queue can wait on it; automations + triggers fire once.
- **Production-hardened (M5):** the 30× agent surge sheds the agent lane and holds the human lane across every
  tenant (BUS-D7); per-ref + per-conversation order holds at target QPS (BUS-D9); a subject's inline-PII events
  are unrecoverable in backups (BUS-D8); the cross-cell bridge resolves cell-local with zero PII crossing; the
  column-store seam is promoted only if measured volume demands it.

---

## 1. The contracts the Bus owns / carries / consumes, mapped to the milestone they land in

From contract-index §2 (owned: envelope/outbox/consumer/taxonomy), §3 (owned: signals/automations/triggers/
firehose/matcher/dispatch), §5.9 (carried: the check seam), §1 (consumed: the service shell), §10/§11/§12
(consumed: holder spine, KMS, tenancy). "Lands" = the milestone by which the contract must be implemented or
callable for the Bus's gate to be green.

### 1.1 Owned by the Bus (contract-index §2, §3)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 2.1 | `EventEnvelope` — the canonical versioned envelope; **the names/units anchor** | **M0** | frozen byte-identical in M0 (every later contract aligns to it, X-5). No floor — it is the anchor. |
| 2.2 | `OutboxTx::emit(draft, cause)` — the ONLY sanctioned emit path; same tx; causality correct-by-construction | **M0** | the Tier-1 emit floor. No `publish_now`. |
| 2.3 | `outbox` table — `UNIQUE(aggregate, seq)` ordering; relay `FOR UPDATE SKIP LOCKED` | **M0** | per-**ref**/per-**conversation** ordering *correctness* is M0; *at production QPS* (BUS-D9) is the **M5** scale follow-on. |
| 2.4 | `EventHandler` consumer template — `subjects()` whitelist (never `*`), ack-after-enqueue, dedup, bounded prefetch, lag metric | **M0** | the shared idempotent-consumer template every consumer is built from. |
| 2.5 | `consumer_dedup` ledger — `(consumer, event_id)` PK | **M0** | the effectively-once anchor. |
| 2.6 | Reindex-from-source — `events::reindex(scope)` → owner `replay` emits `*.snapshot`; **sub-artifact-granular** | **M2** seam | the seam + `*.snapshot` schema is M2; per-owner `replay` lands with each owner (M3/M4). The only recovery path. |
| 2.7 | Crypto-shred / tombstone on the log — `*.erased` tombstones; inline-PII envelope-encrypted with `pii_key_ref`; bus is a holder | **M1** holder reg.; **M2** real shred | auto-registered as a `PersonalDataHolder` in M1 (exhaustive holder list before real data); the inline-PII key-destroy + tombstone emit is exercised once events flow (M2); BUS-D8 reaches backups in **M5**. |
| 2.8 | Schema evolution / upcasters — `(type, from_ver) → to_ver` pure fns at consume; forward-only | **M0** seam; **per-type M2+** | the upcaster registry + the forward-only discipline (the `forward-only-migration` lint) is M0; concrete upcasters land as types evolve. |
| 2.9 | Event taxonomy + token table — grammar + seed; **+ new tokens `ci.check.updated`/`ci.result`/`initiative`** | **M0** grammar + seed; **per-subsystem lists M3/M4** | the §6.1 grammar + the seed taxonomy + the new check-seam/`initiative` tokens are registered in M0/M2; each subsystem completes its dotted-name list as it ships (M3/M4). |
| 3.1 | `define_signal_rule(...)` — curated/deduped/ranked Signal subset; `sig.<tenant>.<severity>.<rule>` | **M2** | the upstream defence (consumers take Signals, never `evt.*`). |
| 3.2 | `register_automation(...)` — stateless per-event reflex; may invoke a durable workflow | **M2** | depends on `myelin-flow` (M2) for `action.kind=workflow`. |
| 3.3 | `arm_trigger/disarm_trigger(...)` — stateful per-person promise; `condition` is a `QueryAst` over projection state; `stale_after` is a `myelin-flow` timer | **M2** | depends on `myelin-flow` timer wheel (9.3, M2) + the frozen `QueryAst` (13.3, M2). |
| 3.4 | `EventMatcher` = the frozen `myelin-query` `QueryAst` — bounded interpreter, statically cost-bounded, permission-aware | **M2** | frozen byte-identical with `myelin-query` (13.3) in M2 so matcher/saved-views/Search/Notif-prefs cannot drift. |
| 3.5 | Firehose transport + the resume-cursor subscription protocol — `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`; bounded scope, never `*`; `resync_required` → `*.snapshot` | **M2** | **EI-04 §2.2: built FIRST, before the CRDT.** The CRDT (KN, M3 floor / M5 full) slots into *this* transport. Retention-window sizing is a **named, measured floor** (§10 Q2). |
| 3.6 | Reactive/dispatch tier — Signal→`EventInbox` matching/guarding/rate-limiting; nested causality; loop guards; bounded dispatch; reserve/settle before any run | **M2** | depends on the Agent `EventInbox` (8.6, M2) + reserve/settle (11.7, M1). |

### 1.2 Carried by the Bus for other owners (the check seam — contract-index §5.9)

| # | Contract | Lands | Notes |
|---|---|---|---|
| 5.9 (carriage) | The Git↔CI `CheckStatus` seam — the Bus carries `ci.check.updated` (per-`(commit_oid, context)` fact, `aggregate=(repo, commit_oid)`) + `ci.result` (the rollup signal the merge queue waits on) | **token reg. M0/M2; consumer (Git) M3; producer (CI) M4** | the Bus owns **only** envelope conformance + per-aggregate ordering on `(repo, commit_oid)` + at-least-once delivery + the durable `wait_for_signal` substrate (via `myelin-flow`). It does **not** own the `CheckStatus` fields, the `run_attempt` supersession, the trust-tier gating, or the merge gate (all CI/Git). The seam goes **end-to-end** in M4 (GIT-D10/CI-D8); the Bus's ordering guarantee under it (D-11) is drilled when the producer lands. |

### 1.3 Consumed by the Bus — the upstream dependencies that must exist first

| # | Consumed contract | From | Must be green by | Why the Bus depends on it |
|---|---|---|---|---|
| 1.1/1.2/1.3 | `serve(AppSpec)` + three-surface + liveness≠readiness | **substrate (M0)** | **M0** | the relay + the Signal engine + the dispatch tier all boot from the harness; the bus's metrics port (1.8) is a surface. |
| 1.8 | Telemetry signal set (consumer-lag, outbox-depth, breaker-state, causal-depth, per-tenant in-flight) | **harness (M0)** | **M0** | **every Bus drill asserts against these.** A drill that survives but emits no signal has failed (T-3). The Bus is the **largest single contributor** to 1.8 (§4.11). |
| 1.9 | `ResilientClient` — timeout + breaker + bulkhead + jittered-retry; honours `Retry-After` | **substrate (M0)** | **M0** mechanism; **M2** in dispatch | the dispatch tier's shed (`429 + Retry-After`) and the relay's downstream calls ride it. |
| 1.11 | Protected-human-lane shed order + per-surface shed budgets (OQ-K) | **harness (M0); Bus budget tuned M5** | **M2** mechanism; **M5** numbers | the reactive tier inherits the shed discipline; the **agent-mention-storm** floor is the Bus's named v1 budget. |
| 11.7 | Reserve/settle cost gate | **Agent gate + Commercial wallet (M1)** | **M2** | the dispatch tier reserves before any agent/CI run starts (no balance → no execution). |
| 11.3/11.4 | KMS hierarchy + per-subject DEK (the crypto-shred substrate) | **Storage/GDPR (M1)** | **M1** | the inline-PII `pii_key_ref` envelope-encryption + key-destroy erasure (2.7) needs the per-tenant/per-subject DEK hierarchy. |
| 10.1/10.8 | `PersonalDataHolder` auto-registration + the erasure ledger | **GDPR (M1)** | **M1** | the bus auto-registers as a holder so the H1–H18 list is exhaustive before real data; the erasure ledger drives post-restore re-erasure. |
| 12.1/12.4 | `(tenant, region)` partition key + `residency_verify` | **Tenancy (M1)** | **M1** | streams are per-`(tenant, subsystem)` and region-pinned; the per-tenant in-flight cap is the fairness/blast-radius unit. |
| 12.6 | Cross-cell PII-free pointer bridge (frame) | **control plane (M1 frame; M5 live)** | **M5** | cross-cell event propagation rides the bridge; **designed-not-built until M5** (single-home-cell is v1). |
| 9.3/9.4 | `myelin-flow` durable timer wheel + durable signal | **Workflow (M2)** | **M2** | Trigger `stale_after` (9.3) and the merge-queue `ci.result` wait (9.4) are delegated to `myelin-flow`, not reinvented. |
| 13.3 | `myelin-query` `QueryAst` grammar (frozen byte-identical) | **Issues/Knowledge co-own (M2)** | **M2** | the `EventMatcher` **is** the `QueryAst` predicate core; without the freeze the matcher means something different from saved-views/Search. |
| 8.6 | `EventInbox::deliver` (explicit-first dispatch) | **Agent Fabric (M2)** | **M2** | the dispatch tier delivers matched events to the agent inbox; the bus owns the matching/guarding, the Fabric owns the runtime. |

**The critical upstream dependency, stated plainly.** Unlike every other shared system, the Bus's *core* has
**no upstream blocker** — it **is** the root of the M0 DAG (master §3.2: the outbox + envelope + consumer
template are depended on by everything). What the Bus's *later* bands depend on is: (a) **the M0 telemetry
contract (1.8)** — without it no Bus drill can be a proof; (b) **the M1 KMS + holder spine (11.3/11.4/10.1)** —
without per-subject DEK the crypto-shred half of GDPR is a claim; (c) **the M2 trio `myelin-flow` (9.3/9.4) +
frozen `QueryAst` (13.3) + Agent `EventInbox` (8.6)** — the reactive layer (Signals/Automations/Triggers/
dispatch) cannot ship until all three are frozen/green. The Bus *blocks* far more than it is blocked by: until
SUB-D1/SUB-D2/BUS-D4 are green, **M1 does not start** (master §2 M0 exit gate).

---

## 2. The sequenced milestones (the Bus's slice of each band)

Each milestone states **the work**, the **floor-then-full progression** (each floor named with its scheduled
follow-on), the **upstream dependencies** (what must be green first), and the **quantified gates/drills** that
call it done. Drill thresholds carry the Q32 defaults-to-beat; Phase 6 measures the final numbers (EI-02 §8).

---

### B-M0 — The emit + consume substrate (inside master band M0) — the silent-data-loss floor

**Master band:** M0 (substrate, harness, committed gates). **This is the Bus's largest and most load-bearing
milestone** — it is the Tier-1 silent-data-loss floor (master §1) that *everything* writes through.

**The work:**
- **Freeze the `EventEnvelope` (2.1)** byte-identical — the names/units anchor (X-5): `event_id` (ULID,
  idempotency anchor), `type`/`schema_ver`, `tenant`/`region` (partition + routing), `actor{principal, kind,
  on_behalf_of, session, run}`, `subject` (ArtifactRef) + `aggregate` (ordering key), the **nested** causality
  triad (`correlation_id`/`causation_id`/`depth`), the GDPR/visibility routing
  (`contains_personal_data`/`data_role`/`visibility`/`pii_key_ref`), `occurred_at`/`recorded_at`, and the
  producer-owned `payload` (small, references-not-payloads). Every later contract compiles against this; it is
  frozen first so nothing drifts.
- **The transactional outbox + relay (2.2/2.3).** The `outbox` table per producing service
  (`event_id UNIQUE, aggregate, seq, subject, envelope`, `UNIQUE(aggregate, seq)`), written in the *same
  transaction* as the state change; the stateless relay (`FOR UPDATE SKIP LOCKED`, per-aggregate `seq` order,
  `Nats-Msg-Id = event_id` broker-dedup, DLQ after N attempts, GC published rows). **The only sanctioned emit
  path** — `no-raw-publish` makes a bare `publish_now` a compile error.
- **The idempotent-consumer template (2.4/2.5).** The `EventHandler` with `subjects()` whitelist (**never
  `*`** — the head-of-line-stall defence), bind-by-name (never re-assert start policy on reconnect), idempotent
  on `event_id` (`INSERT … ON CONFLICT DO NOTHING`), ack-after-enqueue, `term` for non-retryable junk, bounded
  prefetch + per-tenant in-flight caps, lag (`num_pending`) exposed to 1.8. This is **the** template every
  consumer in the platform is built from — abstract-at-the-first-copy (EI-01 §7) because there will be dozens.
- **The transport behind the `BusTransport` trait (`put/consume/ack/purge`)** — NATS JetStream-class default;
  the swap to Kafka/Redpanda (BUS-6) is a relay-target change, not a consumer rewrite. **No non-durable
  fire-and-forget path exists.**
- **The Bus's three lints (contract 1.6), each with red + green fixtures** (the M0 ratchet):
  `no-raw-publish` (no event escapes the outbox — red fixture: a direct broker publish; green: an
  `OutboxTx::emit`), `no-cross-sync-cycle` (Git never synchronously calls CI — red: a sync cross-subsystem
  call in a write path; green: an async event/projection), and the Bus's slice of `tenant-predicate` (a
  stream/consumer without a tenant scope — red: an unscoped subscribe; green: a `(tenant, subsystem)` stream).
- **The seed taxonomy grammar (2.9)** — `<subsystem>.<artifact_type>.<event_name>`, lowercase/singular/
  past-tense, the canonical `ArtifactRef` subsystem/type token table (the names anchor), and the new tokens
  `ci.check.updated`/`ci.result`/`initiative` *registered* (the grammar + seed; per-subsystem completion is
  M3/M4).
- **The schema-evolution seam (2.8)** — the upcaster registry `(type, from_ver) → to_ver` + the
  expand→migrate→contract forward-only discipline (the `forward-only-migration` lint applies to envelope
  evolution too: new optional fields only, consumers ignore unknowns, no rollback migrations).
- **Wire the Bus's survival signals into 1.8** — consumer-lag, outbox-depth+age, relay publish+dead-letter
  rate, per-aggregate publish latency, dedup hit-rate, per-tenant in-flight, causal-depth histogram +
  shared-root-tripwire counter. **A Bus drill that emits no signal has failed.**

**Floor-then-full:**
- **Single-region event log (general-purpose DB / JetStream) is the M0 floor; the column-store/time-series
  seam (BUS-6) is the post-M5 follow-on** (master §5). The seam (the `BusTransport` trait + the OLAP long-term
  holder) is built now; the promotion to a ClickHouse-class tier is **measured-not-predicted** — added only
  when a per-stream volume is *measured* to outgrow JetStream (EI-04 §5.2). Named, not numbered, in v1.
- **Per-aggregate ordering correctness is M0; per-aggregate ordering *at production QPS* (BUS-D9) is the M5
  scale follow-on.** The `UNIQUE(aggregate, seq)` invariant makes outbox order == state-change order *by
  construction* now; proving it holds under a burst of force-pushes to one hot ref / sends to one hot channel
  at target QPS is the scheduled M5 drill.

**Upstream dependencies:** none structurally — the Bus is the M0 DAG root. It needs only the **M0 harness
shell (1.1/1.2)**, the **lint framework + contract-coverage scanner (M0 substrate)**, and the **telemetry port
(1.8)** to assert against — all of which are co-built in M0.

**Gate (must be green to satisfy the M0→M1 boundary — master §2/§4):**
- **SUB-D1** (F5) — kill the service between commit and publish → the outbox delivers every committed event
  **exactly-once-in-effect (0 ghost, 0 lost)**. Reads: outbox-depth drains; dedup ledger. CI.
- **SUB-D2** (F5) — drop the broker mid-stream → **0 lost across reconnect** (bind-by-name, dedup); a slow
  subject does not block others. Reads: consumer-lag; no HoL stall. CI.
- **BUS-D4** (F5) — crash the producer between state-commit and relay-publish → the event is delivered (outbox
  survived), **never delivered without the state change** (outbox emit-iff-committed). Reads: outbox
  depth+age. CI.
- **BUS-D2** (F5/HoL) — flood a (wrongly) `*`-subscribed consumer with unhandled types → the whitelist-template
  consumer does **not** stall; the lag alarm fires. CI.
- **The Bus's three lints green** with both fixtures (`no-raw-publish`, `no-cross-sync-cycle`, the Bus slice of
  `tenant-predicate`) — wired into CI, loud, never `|| true`.
- **The harness self-test** — the failure-injection harness can inject a producer-kill fault and read the
  resulting outbox-depth/dedup telemetry assertion (the unit-of-proof self-test, master §1 Tier-0).

---

### B-M1 — Tenancy partition + the crypto-shred holder seam (inside master band M1)

**Master band:** M1 (Identity + storage durability + tenancy). The Bus adds **no new engine** here — it is
*placed under* the M1 partition + durability + GDPR floor.

**The work:**
- **Partition the streams under the `(tenant, region)` key (12.1).** Provision per-`(tenant, subsystem)`
  streams (cell-local), partitioned internally by `aggregate_id`; the tenant becomes the blast-radius +
  fairness unit (the per-tenant in-flight cap from B-M0 is now tenant-real). The subject encodes both routing
  and ordering: `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>`. **Residency-pinned**:
  no cross-region stream read path (the `residency-pin` lint applies).
- **Register the Bus as a `PersonalDataHolder` (2.7 / 10.1).** Auto-registered by the harness so the H1–H18
  holder list is **exhaustive before any real tenant data is written** (master §1 Tier-1). Implement
  `locate(subject)` (inline-PII events + tombstone status), `erase(subject)` (crypto-shred inline-PII keys +
  emit `*.erased` tombstones), `export(subject)` (the subject's events, references resolved via owners).
- **Wire the inline-PII crypto-shred to the M1 KMS hierarchy (11.3/11.4).** The rare inline-PII event is
  envelope-encrypted with `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` (per-tenant, optionally
  per-subject DEK); erasure = destroy the key. The references-not-payloads default means **most** events carry
  `contains_personal_data = false` (erasing the person tombstones the identity, not the fact).
- **Hook the erasure ledger (10.8) for post-restore re-erasure.** When Storage restores an older backup, the
  Bus's holder participates in the re-erasure fan-out (the key stays destroyed across a restore).

**Floor-then-full:**
- **Single-home-cell event propagation is the M1 floor; cross-cell fan-out is the M5 follow-on** (master §5,
  contract 12.6). The cross-cell **bridge frame** (`CrossCellPointer{subject, type, correlation_id,
  home_cell}`) is **pinned** here (the §5 contracts are cell-agnostic so it extends without a rewrite), but the
  *build* — per-viewer cell-local resolution, the residency proof that no PII crosses, the multi-cell fan-out —
  is **designed-not-built** until M5. Named, not silent.
- **The structural crypto-shred floor ships here; the `[OPEN — LEGAL]` residual is flagged.** The bus solves
  the *event-log* half of erasure-vs-immutability (EI-04 §1) — pseudonymous `actor.principal` + crypto-shred +
  `*.erased` tombstone. The free-text/immutable residual (third-party PII a person typed into another's
  content) is handled **per the ONE platform posture (10.9, X-7) by reference, not restated** — the structural
  floor ships regardless; the residual lawful-basis is `[OPEN — LEGAL]` for counsel/DPO (not an engineering
  gate).

**Upstream dependencies (must be green first):**
- **Tenancy `(tenant, region)` partition key + `residency_verify` (12.1/12.4, M1)** — streams cannot be
  region-pinned until the partition key exists.
- **KMS hierarchy + per-subject DEK (11.3/11.4, M1)** — the inline-PII envelope-encryption needs it.
- **`PersonalDataHolder` trait + erasure ledger (10.1/10.8, M1 GDPR structural spine)** — the holder
  registration + the re-erasure path.
- **B-M0 green** (the outbox + consumer template + envelope the holder operates over).

**Gate (the Bus's contribution to the M1→M2 boundary — does not block M2 alone, but must be green for the band):**
- **BUS-D8** (erasure) — erase a subject → inline-PII events unrecoverable (key destroyed); `*.erased`
  tombstones emitted; consumers degrade. Reads: erase-receipt; tombstone count. (SCHED; the *reaches-backups*
  leg is re-confirmed at M5 with STOR-D4.)
- **The Bus's slice of CP-D3 / STOR-D5 (residency)** — a write where `row.region ≠ cell.region` is rejected;
  no cross-region stream read path; `residency_verify` attestation passes for the Bus's streams. CI/SCHED.
- *(The M1 hard go/no-go — STOR-D1/D2 restore-verify, ID-D3 cross-tenant, ID-D1/D2 — are owned by Storage/Id;
  the Bus's holder participates in the restore-verify cross-seam (the event-log offset is the cross-seam
  cursor, 11.5) and must be consistent at the restored point.)*

---

### B-M2 — The reactive layer: Signals, Automations, Triggers, the firehose resume-cursor, the dispatch tier, the check-seam carriage

**Master band:** M2 (the reactive shared layer + the safety drills). **The Bus's second large band** — the
connective tissue every subsystem projects onto.

**The work:**
- **The `EventMatcher` = the frozen `myelin-query` `QueryAst` (3.4).** Freeze the grammar byte-identical with
  13.3 (`And/Or/Not/Cmp/In/Has/Text/Ref`, ops `eq..within`); the bounded interpreter (no UDFs/loops/recursion,
  statically cost-bounded, side-effect-free); **permission-aware by construction** (always composes with
  `list_objects(viewer, read, type)` — the frozen `SetExpr` push-down, 4.3, from M1). One grammar, compiled to
  a JetStream subject filter where possible + the bounded interpreter for the residual predicate. This freeze
  is the *prerequisite* for Signals/Automations/Triggers all sharing one predicate surface.
- **Signal curation (3.1).** The Signal engine (an infra consumer on the raw `evt.*` firehose — one of the
  excepted full-firehose consumers): match the `EventMatcher`, severity-rank
  (`info<notice<warning<error<critical`), **dedup within window** (N identical failures collapse to one Signal
  with `count=N` — the storm-control primitive Notif relies on), auto-resolve, publish to
  `sig.<tenant>.<severity>.<rule>`. **The upstream defence**: product consumers + agents subscribe to curated
  Signals, never the raw firehose.
- **Automations (3.2) + Triggers (3.3).** The stateless per-event reflex (`action.kind=workflow` invokes
  `myelin-flow`); the stateful per-person promise (`armed → {resolved|stale|disarmed}`, fire-once-per-arming
  via the atomic guarded `UPDATE`, `condition` a `QueryAst` over projection state — "all `blocked_by`
  resolved" expressed as `Has`/`Ref` over the Issues `issue_relation` projection; `stale_after` delegated to
  the `myelin-flow` minute-bucket timer wheel, 9.3).
- **The firehose transport + the resume-cursor subscription protocol (3.5) — built FIRST (EI-04 §2.2).**
  `firehose::publish/tail` + the **new** `subscribe(stream, scope, cursor?)` / `resume(stream, scope,
  last_seq)`: per-`(stream, scope)` monotonic `seq`; on reconnect, backfill `(last_seq, now]` from a bounded
  retention window then live (**loses zero ops**); out-of-window `last_seq` → `resync_required` → `*.snapshot`
  fallback (the cold-rebuild path, **named, not silent**). **Bounded scope, never `*`** (`board:`/`doc:`/
  `channel:`); the transport rejects an over-broad scope; a huge board paginates its scope. Per-connection
  in-flight caps; a slow consumer drops to `resync_required` rather than buffering unboundedly. **This is the
  durable real-time transport the CRDT slots into** — the Bus provides the seam + protocol, KN owns the CRDT.
- **The check-seam carriage (2.9 / §4.12, contract 5.9).** Register + carry `ci.check.updated`
  (`aggregate=(repo, commit_oid)`, the CI-owned `CheckStatus` in `payload`, references-not-payloads) and
  `ci.result` (the rollup signal). The Bus guarantees **per-aggregate ordering on `(repo, commit_oid)` +
  at-least-once delivery + the durable `wait_for_signal` substrate** the merge-queue workflow uses — it does
  **not** evaluate the `run_attempt` supersession or the merge gate. The *consumer* (Git's `check_status`
  projection) lands in M3; the *producer* (CI) in M4; the Bus's ordering guarantee under it is D-11.
- **The reactive/dispatch tier (3.6) — separately reviewed (D7).** Consumes Signals, delivers to the Agent
  `EventInbox` (8.6). Disciplines: **nested causality** (a dispatched action derives
  `causation_id = dispatch_event.event_id`, `depth = +1` — flat threading forbidden, it breaks depth-capping);
  **structural loop guards** (self-guard, reference-gate — only a structured `artifact_ref` node re-triggers,
  never raw text — causal-depth ceiling default 12, shared-causal-root tripwire → per-tenant breaker);
  **bounded dispatch pool that drops over-cap** (a mention/event storm is bounded, never forked unboundedly);
  **explicit-first dispatch (CHAT-1)** — a mention *notifies*, does not auto-spawn a costed run; **reserve/
  settle before any run** (11.7 — no balance → no execution).
- **Reindex-from-source (2.6).** The `events::reindex(scope)` seam + the `*.snapshot` event schema
  (sub-artifact-granular — CI one-run, KN page-subtree at block granularity); the per-owner `replay`
  implementations land with each owner (M3/M4). This is **the only recovery path** for derived stores and the
  `resync_required` fallback target for the firehose.

**Floor-then-full:**
- **The firehose retention-window per stream class is a named, measured floor (§10 Q2).** The `(last_seq,
  now]` backfill is bounded by a retention window; the window size (CI log vs collab op vs chat live) is
  **measured-not-predicted** — too short forces expensive `resync_required`, too long costs storage. Floor:
  the window must exceed the p99 reconnect gap; tuned by the D-10 drill in M5. Named, not numbered, in v1.
- **The resume-cursor transport is the floor the CRDT promotes into.** The Bus ships the resume-cursor +
  idempotent-apply *seam*; KN's CAS floor (M3) and then the CRDT (M5) slot into it. **D-10 is written to
  re-run green across the KN CAS→CRDT `engine_promote` boundary** (the floor's promotion is itself drilled,
  master §5).
- **Mock-agent dispatch is the floor; the real `LlmAgentRuntime` is post-M5.** The dispatch tier delivers to
  the Agent `EventInbox` regardless of runtime; the mock→real swap is the Agent Fabric's config swap, not a Bus
  change.

**Upstream dependencies (must be green first):**
- **`myelin-flow` durable timer wheel + durable signal (9.3/9.4, M2)** — Trigger `stale_after` + the
  `ci.result` merge-queue wait are delegated to it.
- **The frozen `myelin-query` `QueryAst` (13.3, M2)** — the `EventMatcher` *is* its predicate core.
- **Identity `list_objects` `SetExpr` push-down (4.3, M1)** — the matcher composes with it for
  permission-awareness.
- **Agent `EventInbox::deliver` + explicit-first dispatch (8.6, M2)** — the dispatch tier's delivery target.
- **Reserve/settle (11.7, M1)** — the dispatch cost gate.
- **B-M0 + B-M1 green** (the outbox, consumer template, holder seam, tenancy partition).

**Gate (the Bus's contribution to the M2→M3 boundary — master §2/§4):**
- **BUS-D1** (F5) — kill a consumer + sever the broker during sustained publish → **0 lost, 0 duplicate
  effects** on reconnect. CI.
- **BUS-D3** (replay) — replay a `correlation_id` tree → deterministic, idempotent re-drive, causality
  preserved (**replay == original**). CI.
- **BUS-D6** (F9) — a self-triggering automation → the depth ceiling + the shared-root tripwire trip the
  per-tenant breaker (**halts ≤ ceiling; breaker trips**). CI.
- **D-10 / BUS firehose** (F5/OQ-J) — drop a `subscribe`d connection mid-stream on a hot board/doc/channel →
  `resume(last_seq)` backfills `(last_seq, now]` then live, **0 ops lost**; an out-of-window `last_seq` yields
  `resync_required` → `*.snapshot` fallback. CI. *(This is the property that makes the M3 KN collab floor + M4
  CHAT/ISS live surfaces safe — it must be green before they ride it.)*
- **D-11 / check-seam ordering** (X-1) — emit interleaved/late-arriving `ci.check.updated` for one `(repo,
  commit_oid)` across contexts + re-run attempts → per-aggregate ordering holds so Git's `run_attempt`
  supersession is well-defined; a stale lower-attempt re-delivery is droppable (**aggregate order preserved;
  supersession deterministic**). CI. *(The Bus's half; the full end-to-end seam is GIT-D10/CI-D8 in M4.)*
- **BUS-D5** (F4) — wipe a derived store, `reindex(scope)` → the rebuilt store byte-matches live
  (**cold == live**). SCHED. *(The seam is provable at M2 against a small derived consumer; the per-subsystem
  full reindex lands with each owner M3/M4.)*

*(The M2 hard go/no-go — AG-D4 the real-kernel sandbox escape — is owned by Agent/CI. The Bus's dispatch tier
must not deliver to a run that would execute untrusted code before AG-D4 is green; the dependency is honoured
by the reserve/settle + the runner gate, not by the Bus.)*

---

### B-M3 / B-M4 — Per-subsystem tokens + the check seam goes live (inside the producer/consumer bands)

**Master bands:** M3 (producers: Git + Knowledge) and M4 (consumers: CI + Issues + Chat). The Bus adds **no
new engine** in these bands — it carries the per-subsystem event flows as each subsystem ships.

**The work:**
- **Each subsystem completes its dotted-name event list (2.9, the 5-B deliverable)** validated against the
  §6.1 grammar, with its `schema_ver` lineage and payload shapes. The Bus owns the grammar + the seed; the
  subsystem owns its complete list. (Git/KN in M3; CI/Issues/Chat in M4.)
- **The check seam goes live (5.9 end-to-end, the X-1 critical-path seam).** In **M3**, Git ships the
  `check_status` **consumer/projection** (built from the Bus's idempotent template, idempotent on `event_id`,
  applying CI/Git's `run_attempt` supersession over the Bus's per-aggregate-ordered `ci.check.updated`) +
  awaits CI. In **M4**, CI ships the **producer** — emits `ci.check.updated` per `(commit_oid, context)` +
  the rollup `ci.result` signal the merge-queue durable workflow waits on. The Bus's role stays narrow:
  envelope conformance, per-aggregate ordering on `(repo, commit_oid)`, at-least-once delivery, the durable
  `wait_for_signal` substrate.
- **Each subsystem implements its `replay(scope, since)` (2.6)** so reindex-from-source covers it
  sub-artifact-granular (CI one-run, KN page-subtree at block granularity).
- **The firehose's heaviest producers come online** — `ci.log.appended` (the heaviest firehose producer,
  CI/M4), KN collab op-streams (M3, the CAS floor rides the resume-cursor transport), Chat presence/live +
  agent partials (M4). The Bus's firehose transport (built in M2) carries them; the durable bus carries only
  the pointer events (`ci.log.available`/`knowledge.doc.updated`/coarse `chat.read_state.updated`).

**Floor-then-full:**
- **The KN CAS floor rides the M2 resume-cursor transport; the CRDT (M5) slots into the same transport.** The
  Bus does not change — D-10 re-runs across the `engine_promote` boundary (master §5).
- **The check seam ships consumer-then-producer across M3→M4 (the named split).** Git ships the gate + the
  projection in M3 (the consumer half), the seam goes live when CI's producer lands in M4 — the most
  load-bearing cross-subsystem contract, deliberately sequenced producer/consumer across two bands.

**Upstream dependencies (must be green first):**
- **B-M2 green** (the check-seam carriage, the firehose transport, the reindex seam all exist).
- **Per band:** Git's `check_status` projection (M3) before CI's producer (M4); each subsystem's `project(ref,
  viewer)` + `IndexSpec` + `replay` as it ships.

**Gate (the Bus's contribution to the M3→M4 and M4→M5 boundaries):**
- **M3→M4:** the Bus's per-aggregate ordering under Git's push events holds (GIT-D9 — `git.ref.updated`
  emitted iff the ref move committed, outbox emit-iff-committed; the Bus carries it) and under KN's block
  commits (KN-D7 — block commit ↔ relay-publish outbox emit-iff-committed; KN-D1 — `resume(scope=doc, last_seq)`
  loses 0 ops, re-run across the CRDT boundary). CI.
- **M4→M5:** **GIT-D10 / CI-D8** (the X-1 check seam end-to-end — out-of-order/dup `ci.check.updated` →
  `run_attempt` supersession; fork self-green neutral; doubly-delivered `ci.result` → merge-queue wakes
  **exactly once**; **0 double-merge**). The Bus's D-11 ordering guarantee is the substrate this rests on. CI.
  Plus **CHAT-D1** (sever gateway↔firehose → `resume` 0 lost/0 dup — the Bus's firehose under Chat's load) and
  **CHAT-D13** (message-persist ↔ event co-commit). CI.

---

### B-M5 — World-scale hardening + the floor follow-ons (inside master band M5)

**Master band:** M5 (world-scale hardening + the floor follow-ons + the cross-subsystem E2E wedge). **The
Bus's production-hardening band.**

**The work — the floor follow-ons (each named in its band; here is its scheduled follow-on):**
- **Cross-cell event propagation goes live (the M1-frame floor → M5 full, contract 12.6).** The cross-cell
  PII-free pointer bridge: the control plane carries **only** `CrossCellPointer{subject, type,
  correlation_id, home_cell}` between a tenant's cells — never payload or PII (`control-plane-pii-free` lint).
  **Resolution is always cell-local** (the home cell renders + permission-checks; only the already-filtered
  projection crosses, or a tombstone). Drills GA-D8 / CP-D7 / CP-D8 (the FLOOR drills, owed when multi-cell
  ships) are now run.
- **The column-store/time-series seam promotion (BUS-6, post-M5 / measured).** If a per-stream volume is
  *measured* to outgrow the JetStream tier at degraded latency (EI-04 §5.2), promote the highest-volume
  durable streams to a ClickHouse-class tier behind the `BusTransport` trait (a relay-target change, not a
  consumer rewrite). **Added only once volume is measured, not before** — named, not scheduled-by-date.
- **The firehose retention-window sizing (the named M2 floor → measured here).** Tune the per-stream-class
  retention window so the `(last_seq, now]` backfill exceeds the p99 reconnect gap (D-10 measures it).

**The work — world-scale hardening (the F6 surge family + the scheduled scale drills):**
- **The 30× agent-surge drills** — prove the protected human/control lane holds while the agent lane sheds
  (`429 + Retry-After` honoured) and other tenants are unaffected (per-tenant bulkhead).
- **Per-aggregate ordering at production QPS (BUS-D9)** — burst force-pushes to one hot ref + a burst of sends
  to one hot channel under load → per-ref + per-conversation order preserved at target QPS.
- **The crypto-shred reaches backups (BUS-D8 at scale, re-confirmed with STOR-D4)** — inline-PII events
  unrecoverable in backups, not just live DBs.

**The work — the Bus is the spine of all four E2E scenarios** (it carries every chained mutation): E2E-1 (the
PR pane's live check-update + the per-ref cache bust), E2E-2 (the flagship — the Signal that wakes the triage
agent, the durable `ci.result` wait, the nested-causality run), E2E-3 (reindex-from-cold == live via the
`*.snapshot` path), E2E-4 (the bus as a holder in the DSAR fan-out — inline-PII events crypto-shredded).

**Floor-then-full:** the cross-cell bridge and the column-store seam are the two floors *promoted* here; both
were named in earlier bands. There is no new floor introduced in M5 — M5 is where the named ones come due.

**Upstream dependencies (must be green first):**
- **B-M4 green** (all five subsystems exist; the deterministic correctness drills are green).
- **The control-plane multi-cell build (12.6 live, M5)** — the cross-cell bridge needs the control plane to
  carry the pointer.
- **Storage restore-verify at cell scale (STOR-D2, M5)** — the Bus's holder participates in the cross-seam
  restore.

**Gate (the Bus's contribution to the M5→M6 boundary — world-scale readiness):**
- **BUS-D7** (F6) — 30× agent publish surge on one tenant → the human/control lane holds, the agent lane sheds,
  **other tenants unaffected**. Reads: shed-counts/lane; per-tenant RED. SCHED. *(Part of the full F6 surge
  family across all owners.)*
- **BUS-D9** (per-ref/per-conversation order) — burst force-pushes to one hot ref + sends to one hot channel
  under load → **per-aggregate order preserved at target QPS**, parallel across aggregates. SCHED.
- **BUS-D8** (erasure, reaches-backups leg) + **STOR-D4** — 0 recoverable inline-PII in the log **and**
  backups; tombstones present. SCHED.
- **D-10 re-green across the KN CAS→CRDT `engine_promote` boundary** — the resume-cursor transport survives the
  CRDT promotion (0 ops lost). CI/SCHED.
- **GA-D8 / CP-D7 / CP-D8** (the FLOOR drills, now owed) — multi-cell erasure fan-out per-cell receipt set;
  cell→cell migration 0 loss; the cross-cell ref carries only the PII-free bridge. SCHED.
- **The four E2E scenarios green** (E2E-1..E2E-4) — the Bus is the spine each rides; each emits its named green
  artifact.

---

### B-M6 — Dogfooding: the Bus carries Myelin's own events (inside master band M6)

**Master band:** M6 (Myelin hosts itself). The Bus carries the platform's own development events — git pushes,
CI runs, issue transitions, chat messages, agent runs — on the same outbox + firehose + check-seam it carries
for tenants.

**The work:**
- The Myelin monorepo's pushes emit `git.ref.updated` through the Bus; the Myelin CI graph emits
  `ci.check.updated`/`ci.result` through the check seam; the team's chat rides the firehose; the
  every-incident-adds-a-drill loop files a Myelin issue **and a reproducing Bus drill** (T-3 — the catalogue
  grows from the platform's own incidents).
- The Bus is exercised by **real load from the builders themselves** — the cheapest, most honest load
  generator (testing-strategy §1).

**Upstream dependencies:** **B-M5 green** (world-scale-ready; the team's data is real tenant data, so the
restore-verify + DSAR fan-out the Bus participates in must be green before the team's events ride it — master
§2 M6 entry).

**Gate (the Bus's contribution to the M6 done-bar):**
- **The Myelin self-hosting CI graph is green** — the Bus carries the platform's own `ci.result` and the merge
  queue wakes on it correctly on Myelin's own commits.
- **No earlier-band Bus gate is red** (the gate invariant — a truth-up pass confirms every PROVEN Bus row rests
  on a dated green artifact, never a doc claim; code-wins-over-docs, EI-01 §1).

---

## 3. The world-scale / hard-problem work, scheduled explicitly (name what ships as a floor)

The doctrine (EI-04 §4, VISION §3): **name the floor and name the follow-on.** The Bus's hard problems, with
ship-band and follow-on-band:

| Hard problem (EI-04 / arch) | Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|---|
| **Erasure vs immutability on the event log** (EI-04 §1) | references-not-payloads + pseudonymous `actor.principal` + per-key crypto-shred + `*.erased` tombstone (the structural floor) | **M1** | the `[OPEN — LEGAL]` residual lawful-basis (10.9, X-7) ratified by counsel/DPO — **one statement, not an engineering gate** | parallel (legal) | the structural floor ships regardless; the residual is flagged |
| **Build the resume-cursor real-time transport FIRST** (EI-04 §2.2) | the firehose `subscribe`/`resume` resume-cursor protocol — the durable transport the CRDT slots into | **M2** | the CRDT (KN, Automerge-/Yjs-class) slots into *this* transport — a KN deliverable, not a Bus change; D-10 re-runs across the boundary | **M5** (KN) | the first true concurrent-edit conflict |
| **Firehose retention-window sizing** (§10 Q2) | a window that exceeds the p99 reconnect gap (named, not numbered) | **M2** | the per-stream-class window **measured + tuned** by D-10 | **M5** | measured reconnect-gap p99 per stream class |
| **Event volume outgrows a general-purpose DB** (EI-04 §5.2) | single-region JetStream-class log + the OLAP long-term holder | **M0** | the column-store/time-series seam (ClickHouse-class) behind the `BusTransport` trait | **post-M5** | a per-stream volume **measured** to outgrow the tier — never before |
| **Cross-cell event propagation** (contract 12.6, OQ-I) | single-home-cell propagation + the pinned `CrossCellPointer` bridge frame (designed-not-built) | **M1** | the cross-cell PII-free pointer bridge live (cell-local resolution); GA-D8/CP-D7/CP-D8 owed | **M5** | cross-cell rollup/collab/cross-org demand |
| **Per-aggregate ordering at production QPS** (§2.3, D-9/BUS-D9) | ordering *correctness* by `UNIQUE(aggregate, seq)` construction | **M0** | ordering proven **at target QPS** under burst force-push / hot-channel load | **M5** | the scheduled scale drill |
| **Crypto-shred reaches backups** (BUS-D8) | key-destroy + tombstone in live stores | **M1** | unrecoverable in **backups** (key excluded from backup), re-confirmed with STOR-D4 | **M5** | the scheduled erasure-reach drill |

**The honest-floor rule binds all of these:** each is tracked in the gap report with its claimed/proven status
and its linked follow-on; the gap being *invisible* is the only failure (EI-04 §4). **D-10 is deliberately
written to re-run green across the KN CAS→CRDT `engine_promote` boundary** so the floor's promotion is itself
drilled.

---

## 4. The Bus's full drill ledger (every owed drill, its band, its gate)

The Bus owes 13 drills (architecture §8 D-1..D-11 + the two SUB-* substrate drills it co-owns). Mapped to the
band whose gate they bound, with the quantified threshold (Q32 default-to-beat) and freq:

| Drill | Band gate | Family | Quantified threshold | Freq |
|---|---|---|---|---|
| **SUB-D1** | M0→M1 | F5 | kill service between commit & publish → exactly-once-in-effect, **0 ghost, 0 lost** | CI |
| **SUB-D2** | M0→M1 | F5/HoL | drop broker mid-stream → **0 lost across reconnect**; slow subject doesn't block others | CI |
| **BUS-D4** | M0→M1 | F5 | crash producer between state-commit & publish → delivered iff committed (**0 ghost, 0 lost**) | CI |
| **BUS-D2** | M0→M1 | F5/HoL | flood a `*`-subscribed consumer → whitelist consumer doesn't stall; lag alarm fires | CI |
| **BUS-D8** | M1 (band) / M5 (backups) | erasure | erase a subject → inline-PII unrecoverable (key destroyed) + `*.erased` tombstones; consumers degrade; **0 recoverable in log/backups** | SCHED |
| **BUS-D1** | M2→M3 | F5 | kill consumer + sever broker during sustained publish → **0 lost, 0 duplicate** on reconnect | CI |
| **BUS-D3** | M2→M3 | replay | replay a `correlation_id` tree → **replay == original, exactly once**, causality preserved | CI |
| **BUS-D6** | M2→M3 | F9 | self-triggering automation → depth ceiling (12) + shared-root tripwire **trip the per-tenant breaker** | CI |
| **D-10 (firehose)** | M2→M3 | F5/OQ-J | drop a `subscribe`d connection → `resume(last_seq)` backfills then live, **0 ops lost**; out-of-window → `resync_required` → `*.snapshot` | CI |
| **D-11 (check seam)** | M2→M3 (Bus half) / M4 (e2e) | X-1 | interleaved/late `ci.check.updated` for one `(repo, commit_oid)` → **aggregate order preserved; supersession deterministic**; stale lower-attempt droppable | CI |
| **BUS-D5** | M2→M3 | F4 | wipe a derived store, `reindex(scope)` → rebuilt **byte-matches live** (cold == live) | SCHED |
| **BUS-D9** | M5→M6 | per-ref/per-conv order | burst force-pushes to one hot ref + sends to one hot channel under load → **per-aggregate order preserved at target QPS**, parallel across aggregates | SCHED |
| **BUS-D7** | M5→M6 | F6 | 30× agent publish surge one tenant → human/control lane holds, agent sheds (429 + Retry-After), **other tenants unaffected** | SCHED |

**The permanent gate the Bus participates in:** the **restore-verify cross-seam** (STOR-D1/D2, 11.5 — the
event-log offset is the cross-seam cursor; the Bus's outbox/streams must be consistent at the restored point).
Re-run on every change touching a store — the silent-data-loss floor ratchets across the whole build (master
§4).

---

## 5. Digest

**The milestones (the Bus's slice of each band):**
- **B-M0 (master M0) — the emit + consume substrate (the silent-data-loss floor):** the frozen
  `EventEnvelope`, the transactional outbox + relay, the idempotent-consumer template + dedup ledger, the
  `BusTransport` trait, the three Bus lints (`no-raw-publish`/`no-cross-sync-cycle`/Bus-slice of
  `tenant-predicate`), the seed taxonomy + new tokens, the schema-evolution seam, the survival signals into
  1.8.
- **B-M1 (master M1) — tenancy partition + the crypto-shred holder seam:** streams partitioned under
  `(tenant, region)` + residency-pinned; the Bus auto-registered as a `PersonalDataHolder`; inline-PII
  crypto-shred wired to the KMS hierarchy; the erasure-ledger post-restore re-erasure hook.
- **B-M2 (master M2) — the reactive layer:** the `EventMatcher` = frozen `QueryAst`; Signal curation;
  Automations + Triggers; **the firehose resume-cursor protocol (built FIRST)**; the check-seam carriage
  (`ci.check.updated`/`ci.result`); the dispatch tier (nested causality + loop guards + bounded pool +
  explicit-first + reserve/settle); reindex-from-source.
- **B-M3/B-M4 (master M3/M4) — per-subsystem tokens + the check seam goes live:** each subsystem completes its
  event list + `replay`; Git ships the `check_status` consumer (M3), CI the producer (M4); the firehose's
  heaviest producers come online. No new Bus engine.
- **B-M5 (master M5) — world-scale hardening + floor follow-ons:** the cross-cell bridge live; the
  column-store seam (if measured); the 30× surge family; per-aggregate order at QPS; crypto-shred reaches
  backups; the Bus as spine of all four E2E scenarios.
- **B-M6 (master M6) — dogfooding:** the Bus carries Myelin's own events; the self-hosting CI graph green.

**The floors + their named follow-ons:**
- **Erasure-vs-immutability structural floor (M1)** → the `[OPEN — LEGAL]` residual lawful-basis (parallel/legal).
- **The firehose resume-cursor transport (M2)** → the CRDT slots into it (M5, KN); D-10 re-runs across the boundary.
- **Firehose retention window — named, not numbered (M2)** → measured + tuned by D-10 (M5).
- **Single-region JetStream log (M0)** → the column-store/time-series seam (post-M5, measured-not-predicted).
- **Single-home-cell propagation + pinned bridge frame (M1)** → cross-cell PII-free bridge live (M5).
- **Per-aggregate ordering correctness (M0)** → proven at production QPS (M5, BUS-D9).
- **Crypto-shred in live stores (M1)** → reaches backups (M5, BUS-D8 + STOR-D4).

**The critical upstream dependencies:**
- The Bus's **core has no upstream blocker — it is the M0 DAG root** (the outbox + envelope + consumer template
  are depended on by everything; until SUB-D1/SUB-D2/BUS-D4 are green, **M1 does not start**).
- Its **later bands depend on:** the M0 telemetry contract (1.8 — no signal, no proof); the M1 KMS + holder
  spine (11.3/11.4/10.1 — the crypto-shred half of GDPR) + the tenancy partition key (12.1); and the M2 trio
  **`myelin-flow` (9.3/9.4) + frozen `QueryAst` (13.3) + Agent `EventInbox` (8.6)** — the reactive layer cannot
  ship until all three are frozen/green.
- The Bus **carries** (does not own) the X-1 check seam: token reg. M0/M2, consumer (Git) M3, producer (CI) M4,
  end-to-end GIT-D10/CI-D8 in M4 — the most load-bearing cross-subsystem contract, split producer/consumer
  across two bands.
