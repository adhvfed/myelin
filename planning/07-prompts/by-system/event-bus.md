# Phase 7 — Prompt Ledger: Event Bus + Trigger/Automation Engine (myelin-events)

> Phase: 07-prompts/by-system. The COMPLETE ordered set of implementation prompts that operationalize the
> event-bus shared system's entire Phase-6 roadmap (06-roadmaps/shared/event-bus.md), one clean-context,
> independently-committable prompt per unit of work, each tagged with its master-sequencing band (M0..M6).
> Authored to the template in 00-ledger-overview.md (followed EXACTLY). Frozen architecture (these prompts
> OPERATIONALIZE, they do not redesign): planning/05-refined-shared-systems-architecture/event-bus.md +
> contract-index.md §2/§3/§5.9 + 00-reconciliation-decisions.md. Build order frozen by
> 06-roadmaps/00-master-sequencing.md (M0..M6 bands + the gate invariant). Doctrine carried:
> external-insights/01-process-and-quality-doctrine.md (§1 code-wins-over-docs + name-your-floors; §2
> order-by-non-negotiability + the gate invariant; §3 prove-it-or-it-isn't-real with a quantified drill +
> observability-is-part-of-the-pass; §5 the committed ratchet) and external-insights/04-hard-problems.md
> (§1 erasure-vs-immutability, §2.2 build the resume-cursor transport FIRST, §5.2 column-store seam, §5.3
> reindex-from-source). Identifiers are plain text (no backticks-as-emphasis). Markdown only; no git commits
> by this document. Date: 2026-06-19.
>
> **Stable ids in this file are local (EB-01..EB-14).** The consolidated index (Phase 7-B,
> 01-ledger-index.md) assigns each its global P-NNN ordinal by interleaving all systems in band order; until
> then the EB-NN id is the stable handle DEPENDS-ON edges reference within this file. Each prompt's BAND +
> DEPENDS-ON are the authoritative ordering constraints.

---

## Coverage map (every Bus roadmap milestone → its prompt(s))

The binding guarantee (00-ledger-overview §5): every milestone in 06-roadmaps/shared/event-bus.md §2 maps to
at least one prompt; every floor maps to a floor prompt AND a follow-on prompt.

| Roadmap milestone | Prompts | Notes |
|---|---|---|
| B-M0 (the emit+consume substrate, the silent-data-loss floor) | EB-01, EB-02, EB-03, EB-04, EB-05 | split by the green-gate seam: envelope+taxonomy / outbox+relay / consumer template+dedup / the three lints / schema-evo seam + telemetry wiring |
| B-M1 (tenancy partition + crypto-shred holder seam) | EB-06, EB-07 | partition+residency-pin / holder-registration+crypto-shred (+ the cross-cell bridge FRAME pinned here, built EB-12) |
| B-M2 (reactive layer) | EB-08, EB-09, EB-10, EB-11 | EventMatcher=QueryAst / Signals+Automations+Triggers / firehose resume-cursor + reindex seam / dispatch tier + check-seam carriage |
| B-M3/B-M4 (per-subsystem tokens + check seam live) | EB-13 | the Bus's narrow carriage role (per-subsystem token validation + the live check seam) |
| B-M5 (world-scale hardening + floor follow-ons) | EB-12, EB-14 | cross-cell bridge BUILD (follow-on of the EB-06 frame) / the F6 surge + QPS-order + backups-erasure drills + E2E spine |
| B-M6 (dogfooding) | covered by EB-14 DoD note + the master M6 dogfood prompts | the Bus adds no new engine in M6; its M6 gate is the self-hosting CI graph green + the truth-up pass — exercised by the dogfood prompts, no Bus-specific build |

Floors (00-master-sequencing §5 / roadmap §3) and their follow-on pairs:
- Per-aggregate ordering CORRECTNESS (EB-02, M0) → ordering AT PRODUCTION QPS / BUS-D9 (EB-14, M5).
- Single-region JetStream log (EB-01/EB-02, M0) → column-store/time-series seam, measured-not-predicted (EB-14 names it; promotion is post-M5 and only on measured volume — no dated follow-on prompt is owed until volume is measured).
- Crypto-shred in live stores (EB-07, M1) → reaches backups / BUS-D8 (EB-14, M5).
- Resume-cursor transport (EB-10, M2) → the CRDT slots into it (a Knowledge deliverable, M5; EB-10's D-10 is written to re-run green across the engine_promote boundary — re-confirmed in EB-14).
- Firehose retention window, named-not-numbered (EB-10, M2) → measured+tuned by D-10 (EB-14, M5).
- Single-home-cell propagation + pinned bridge FRAME (EB-06, M1) → cross-cell PII-free bridge LIVE (EB-12, M5).
- Erasure-vs-immutability structural floor (EB-07, M1) → the [OPEN — LEGAL] residual lawful-basis (parallel/legal; not an engineering prompt — the structural floor ships in EB-07, the residual is one ratified statement owned by the GDPR/legal track by reference, X-7).

---

## EB-01 — Freeze the EventEnvelope + the seed taxonomy grammar + token table

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 (the emit + consume substrate) — the EventEnvelope freeze + the taxonomy
  grammar slice. Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.1 + 2.9.
- **DEPENDS-ON.** none (this is a root M0 prompt — the names/units anchor every later contract compiles
  against; it requires only the workspace + glue-crate skeleton, which the substrate M0 prompts lay down,
  and may run as soon as the myelin-events crate skeleton exists).
- **CANON DOCS (read these first, in full, before writing any code).**
  - ../../../VISION.md (always) + external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors,
    code-wins-over-docs — the envelope is the names/units anchor, X-5; getting it wrong calcifies every
    downstream contract).
  - ../../05-refined-shared-systems-architecture/event-bus.md §3.1 (the envelope field list + units — the
    AUTHORITY), §6.1 (the dotted-name grammar), §6.2 (the ArtifactRef subsystem/type token table + the new
    initiative type token), §6.3/§6.4 (the new ci.check.updated / ci.result tokens + the seed event names).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.1 (EventEnvelope — the names/units
    anchor) and 2.9 (taxonomy + token table; new tokens ci.check.updated / ci.result / initiative).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-5 (the two names/units
    anchors are unchanged) and X-3 (the QueryAst freeze the matcher will align to — read for context only).
- **DELIVERABLE (what to build + exactly where in the repo).** In the glue crate myelin-events:
  - envelope.rs: the EventEnvelope struct exactly as Bus §3.1 lists — event_id (ULID), type (canonical
    dotted name), schema_ver, occurred_at/recorded_at (RFC-3339 UTC, two clocks), tenant/region, actor{principal,
    kind in {human|agent|service}, on_behalf_of, session, run}, subject (ArtifactRef) + aggregate (ordering
    key), the NESTED causality triad correlation_id/causation_id/depth (root depth=0, child = parent+1),
    contains_personal_data/data_role/visibility (a hint, NEVER an authz decision) / pii_key_ref
    (kms://<tenant>/<dek-epoch>/<class>), and the producer-owned versioned payload. Serde with a stable
    JSON encoding; references-not-payloads (payload is small).
  - taxonomy.rs: the grammar validator for type = <subsystem>.<artifact_type>.<event_name> (lowercase,
    singular, past-tense, tokens match [a-z][a-z0-9_]*, two segments min / three when an artifact type
    clarifies); the canonical ArtifactRef subsystem token set (git/ci/issue/knowledge/chat/identity/refs)
    + the type tokens incl. the new initiative; the seed event-name table (the §6.4 representative names)
    + the new tokens ci.check.updated and ci.result REGISTERED.
  - This freezes the anchor. FLOOR named: per-subsystem dotted-name LIST completion is deferred to EB-13
    (each subsystem owns its full list in M3/M4); EB-01 ships the grammar + seed + the new check-seam tokens
    only.
- **CONTRACTS TO IMPLEMENT.** 2.1 EventEnvelope (owned — the primary contract, the names/units anchor);
  2.9 Event taxonomy + token table (owned — grammar + seed + the three new tokens). Implement to the frozen
  shape in Bus §3.1/§6; a needed shape change is a whole-workspace contract PR, escalated and written down,
  never a local divergence (EI-01 §1).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue drill greens on EB-01 alone
  (it ships data shapes, not a running emit path). The gate is structural: the envelope serialises/deserialises
  round-trip lossless for every field; the taxonomy validator REJECTS a malformed type name (uppercase /
  plural / present-tense / single-segment / unknown subsystem token) and ADMITS every seed name + the three
  new tokens — a red-fixture + green-fixture pair (the same ratchet shape the lints use). This pair IS the
  proof the anchor is well-defined.
- **TESTS (required).** Unit: round-trip serde for the full envelope incl. the nested causality triad and a
  populated pii_key_ref; depth-derivation (child = parent+1) computed from a cause; the taxonomy validator's
  reject-fixture (4+ malformed names) and admit-fixture (every seed name + ci.check.updated + ci.result +
  initiative). CDC: this is a glue-contract carrier; the provider+consumer CDC pair for rows 2.1 and 2.9 is
  the envelope-shape conformance test (a consumer that deserialises an envelope emitted by the canonical
  encoder, and a producer that emits to the canonical shape) — the contract-coverage scanner fails the build
  if 2.1/2.9 lack both. Mutation: myelin-events envelope/taxonomy are mandatory-core; state the mutation-score
  floor for envelope.rs + taxonomy.rs and meet it.
- **DEFINITION OF DONE.** envelope.rs + taxonomy.rs compile in the workspace; 2.1 + 2.9 implemented to the
  frozen shape; the reject/admit fixture pair is green-and-dated; the round-trip + validator unit tests pass;
  the CDC pair exists; the contract-coverage scanner passes for 2.1/2.9; all committed lints green; the floor
  (per-subsystem token list deferred to EB-13) is named in writing; work committed.
- **COMMIT.** Header "P-<NNN> M0: Freeze EventEnvelope + seed taxonomy grammar". Body: contracts 2.1, 2.9
  implemented; the validator reject/admit fixture green (N malformed rejected, all seed + 3 new tokens
  admitted); floor named (per-subsystem token lists → EB-13). Branch first if on default; Co-Authored-By
  trailer.

---

## EB-02 — The transactional outbox + the FOR UPDATE SKIP LOCKED relay (per-aggregate ordering)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the outbox + relay (the Tier-1 silent-data-loss emit floor). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.2 + 2.3.
- **DEPENDS-ON.** EB-01 (the EventEnvelope the outbox row stores + the relay publishes).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (order-by-non-negotiability:
    silent data loss outranks every feature — this IS the floor) + §3 (prove-it-or-it-isn't-real; the
    observability is part of the pass).
  - external-insights/02-platform-substrate.md §4 (the transactional outbox — the only sanctioned emit path).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.2 (partition key = aggregate, per-aggregate
    ordering), §2.3 (per-aggregate ordering at production QPS — the two adversarial cases, the D-9 floor),
    §3.2 (the outbox table schema), §4.1 (the relay: FOR UPDATE SKIP LOCKED, Nats-Msg-Id dedup, DLQ, GC).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.2 (OutboxTx::emit — the ONLY
    sanctioned emit path; same tx; causality correct-by-construction; no publish_now) and 2.3 (outbox table —
    UNIQUE(aggregate, seq); relay FOR UPDATE SKIP LOCKED).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows SUB-D1, BUS-D4 (the outbox no-ghost/no-loss drills this prompt greens).
- **DELIVERABLE.** In myelin-events:
  - outbox.rs: the outbox table schema (event_id UNIQUE, aggregate, seq, subject, envelope, tenant, ...),
    UNIQUE(aggregate, seq) as the source-of-truth ordering invariant; seq allocated per-aggregate INSIDE the
    producing transaction so it reflects true commit order. The OutboxTx::emit(draft, cause) API — written in
    the SAME db transaction as the state change; causality auto-derived from cause (BUS-5). NO publish_now /
    fire-and-forget path exists.
  - relay.rs: the stateless, horizontally-replicable relay — claim a batch (FOR UPDATE SKIP LOCKED, ordered
    aggregate, seq), per-aggregate in seq order transport.put(subject, envelope, dedup_id = event_id) with
    Nats-Msg-Id = event_id for broker-side dedup, mark published_at on ack, retry on failure, dead-letter to
    dlq.<tenant>.<subsystem> after N attempts with a Signal alert, GC published rows after 24h.
  - transport.rs: the BusTransport trait (put/consume/ack/purge) with a JetStream-class reference impl; the
    swap to Kafka/Redpanda is a relay-target change behind this trait, not a consumer rewrite.
  - FLOOR named: per-aggregate ordering CORRECTNESS is shipped here (the UNIQUE(aggregate, seq) construction);
    proving it holds AT PRODUCTION QPS under a hot-ref / hot-channel burst (BUS-D9) is the M5 follow-on EB-14.
    FLOOR named: single-region JetStream log; the column-store/time-series seam (BUS-6) is the post-M5,
    measured-not-predicted follow-on (the BusTransport trait IS the seam — built now, not promoted until
    volume is measured).
- **CONTRACTS TO IMPLEMENT.** 2.2 OutboxTx::emit (owned); 2.3 outbox table + relay (owned). To the frozen
  shape (Bus §3.2/§4.1).
- **GATE / DRILLS.** SUB-D1 → kill the service between commit and publish; the outbox delivers every committed
  event exactly-once-in-effect: 0 ghost, 0 lost; reads outbox-depth drains + the dedup ledger (telemetry, CI).
  BUS-D4 → crash the producer between state-commit and relay-publish; the event is delivered iff the state
  change committed (outbox emit-iff-committed): 0 ghost, 0 lost; reads outbox depth+age (CI). Both thresholds
  are 0 — never weaken to pass; a red gate is a dated "claimed, not proven" scorecard row.
- **TESTS.** Unit: emit-in-same-tx (a rolled-back transaction emits nothing; a committed one emits exactly
  one row); per-aggregate seq monotonicity under concurrent emitters to the same aggregate; the relay's
  SKIP LOCKED claim does not double-publish across two relay workers. Drill scenarios: SUB-D1 and BUS-D4 as
  failure-injection-harness scenarios (kill between commit and publish; assert against outbox-depth + dedup
  telemetry — a drill that survives but emits no signal has FAILED, EI-01 §3). CDC: provider+consumer pair
  for 2.2/2.3. Mutation: outbox.rs + relay.rs are mandatory-core; state and meet the mutation-score floor.
- **DEFINITION OF DONE.** outbox.rs/relay.rs/transport.rs compile; 2.2 + 2.3 to the frozen shape; SUB-D1 and
  BUS-D4 each emit a dated green artifact (0 ghost, 0 lost — PROVEN, not CLAIMED); unit + drill + CDC tests
  pass; coverage scanner + all lints green; the two floors (QPS-order → EB-14, column-store seam → measured
  post-M5) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M0: Transactional outbox + relay (no-ghost/no-loss emit floor)". Body:
  contracts 2.2, 2.3; SUB-D1 green (0 ghost / 0 lost, measured); BUS-D4 green (emit-iff-committed); floors
  named (BUS-D9 QPS-order → EB-14; column-store seam → measured post-M5). Co-Authored-By trailer.

---

## EB-03 — The idempotent-consumer template + the consumer_dedup ledger

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the idempotent-consumer template (the template EVERY consumer in the platform
  is built from). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.4 + 2.5.
- **DEPENDS-ON.** EB-01 (the envelope/event_id the dedup ledger keys on), EB-02 (the transport the template
  binds to).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it: the head-of-line
    stall is forced by a drill) + §7 (abstract-at-the-first-copy — there will be dozens of consumers; this
    template is the one primitive).
  - external-insights/03-agent-native-fabric.md §6.1 (the orchestrator/consumer head-of-line-blocking gotcha,
    the whitelist-not-* defence).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.2 (the shared consumer template — the seven
    EI-03 §6 gotchas: whitelist subjects never *, bind-by-name, idempotent on event_id, ack-after-enqueue,
    term non-retryable junk, bounded prefetch + per-tenant in-flight caps, lag is a survival signal), §3.3
    (the consumer_dedup ledger).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.4 (EventHandler consumer template —
    subjects() whitelist never *, handle → {Done|NonRetryable|Retry}, durable-bind-by-name, ack-after-enqueue,
    dedup ledger, bounded prefetch, lag metric) and 2.5 (consumer_dedup ledger — (consumer, event_id) PK).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows SUB-D2, BUS-D2 (the drills this prompt greens).
- **DELIVERABLE.** In myelin-events:
  - consumer.rs: the EventHandler trait + the consume(ConsumerSpec{durable, subjects /* explicit whitelist,
    NEVER * */, max_ack_pending, per_tenant_inflight}, handler) template. It encodes: (1) whitelist subjects
    via subjects() — never *; (2) bind to a durable consumer by name, never re-assert start policy on
    reconnect; (3) idempotent on event_id (INSERT … ON CONFLICT DO NOTHING against the dedup ledger); (4) ack
    only after the work is durably enqueued (at-least-once to the next stage); (5) term non-retryable junk
    (handle → NonRetryable) so it doesn't burn the redelivery budget; (6) bounded prefetch + bounded handler
    pool + per-tenant in-flight caps; (7) consumer lag (num_pending) exposed as a telemetry survival signal.
  - dedup.rs: the consumer_dedup ledger ((consumer, event_id) PK; presence == "already handled").
  - This is THE template — abstract-at-the-first-copy (EI-01 §7). Every later consumer (Signal engine, Git
    check_status projection, Search indexer, Notif router, the dispatch tier) is built from it, not hand-rolled.
- **CONTRACTS TO IMPLEMENT.** 2.4 EventHandler consumer template (owned); 2.5 consumer_dedup ledger (owned).
- **GATE / DRILLS.** SUB-D2 → drop the broker mid-stream; 0 lost across reconnect (bind-by-name + dedup); a
  slow subject does NOT block others; reads consumer-lag + no-HoL-stall (CI). BUS-D2 → flood a (wrongly)
  *-subscribed consumer with unhandled types; the whitelist-template consumer does NOT stall while the naive
  one does; the lag alarm fires; reads num_pending (CI). Thresholds: 0 lost, lag bounded + alarm fires.
- **TESTS.** Unit: idempotent re-delivery (the same event_id delivered twice produces one effect); the
  whitelist consumer ignores subjects it didn't whitelist (no HoL stall); per-tenant in-flight cap is honoured;
  ack-only-after-enqueue (a crash before enqueue redelivers). Drill scenarios: SUB-D2 and BUS-D2 on the
  failure-injection harness, asserting against consumer-lag telemetry. CDC: provider+consumer pair for
  2.4/2.5. Mutation: consumer.rs + dedup.rs mandatory-core; state and meet the floor.
- **DEFINITION OF DONE.** consumer.rs/dedup.rs compile; 2.4 + 2.5 to the frozen shape; SUB-D2 + BUS-D2 each
  emit a dated green artifact (0 lost across reconnect; whitelist consumer does not stall + lag alarm fires);
  unit + drill + CDC tests pass; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M0: Idempotent-consumer template + dedup ledger". Body: contracts 2.4, 2.5;
  SUB-D2 green (0 lost across reconnect); BUS-D2 green (whitelist consumer no-stall, lag alarm fires).
  Co-Authored-By trailer.

---

## EB-04 — The three Bus architecture lints (no-raw-publish, no-cross-sync-cycle, tenant-predicate-on-streams), each with red+green fixtures

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the Bus's slice of the twelve committed lints (the M0 ratchet). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contract 1.6 (the Bus's three lints).
- **DEPENDS-ON.** EB-02 (no-raw-publish forbids any emit path that isn't OutboxTx::emit — the lint needs the
  sanctioned path to exist), EB-03 (the tenant-predicate-on-streams lint checks the consume/subscribe surface).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §5 (the ratchet — an uncommitted
    gate is no gate; make violations loud, never silently swallowed; no `... || true`).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.2 (the no-raw-publish discipline; the
    whitelist-not-* rule the tenant-predicate slice enforces) + §7.1 (cell-local, no synchronous cross-system
    call in the write path — the no-cross-sync-cycle rule).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 1.6 (the twelve architecture lints —
    the Bus owns no-raw-publish, no-cross-sync-cycle, and the Bus slice of tenant-predicate).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md for the acyclicity rule the
    no-cross-sync-cycle lint enforces (Git never synchronously calls CI; every cross-subsystem dependency is
    an async event/projection).
- **DELIVERABLE.** In the workspace lint crate (the architecture-lint harness laid down by the substrate M0
  prompts; the lints are committed CI gates, contract 1.6):
  - no-raw-publish: a compile-time lint making a bare broker publish / publish_now a compile error — the only
    sanctioned emit is OutboxTx::emit. Red fixture: a direct transport.put / publish_now in a write path
    (must be rejected). Green fixture: an OutboxTx::emit (must be admitted).
  - no-cross-sync-cycle: a lint rejecting a synchronous cross-subsystem call in a write path (a subsystem
    synchronously calling another to ask "is it green"). Red fixture: a sync cross-subsystem call. Green
    fixture: an async event/projection read.
  - The Bus slice of tenant-predicate: a lint rejecting a stream/consumer/subscribe without a (tenant,
    subsystem) scope. Red fixture: an unscoped subscribe / a subscribe with scope = * . Green fixture: a
    (tenant, subsystem)-scoped stream.
  - Each lint is WIRED INTO CI, loud, never `... || true` (an uncommitted lint is no lint — wire it in, do
    not leave it on disk).
- **CONTRACTS TO IMPLEMENT.** 1.6 (the Bus's three of the twelve lints — owned by the Bus's slice; the lint
  harness is shared substrate).
- **GATE / DRILLS.** The three lints green WITH BOTH FIXTURES, wired into CI loud-never-swallowed: each lint
  rejects its red fixture (proves it forbids) AND admits its green fixture (proves it doesn't over-reject).
  A lint that only rejects (or only admits) is not proven — both fixtures are the pass condition (the same
  ratchet shape master §2 M0 requires). Telemetry: the CI job's lint-pass artifact is the dated green proof.
- **TESTS.** Unit: each lint's red fixture compiles to a lint error; each green fixture compiles clean. The
  CI wiring is the gate — assert the workflow fails (loudly, non-zero exit) when the red fixture is present
  (no `|| true` swallow). CDC: not a runtime contract — the fixture pair IS the test obligation.
- **DEFINITION OF DONE.** The three lints exist in the lint crate, are wired into CI loud-never-swallowed,
  and are green with both fixtures (PROVEN: red rejected, green admitted, dated); the CI-fails-on-red wiring
  test passes; committed. (Note: these three are part of the twelve-lint M0 gate the whole substrate band
  shares; this prompt ships exactly the Bus's three.)
- **COMMIT.** Header "P-<NNN> M0: Bus architecture lints (no-raw-publish / no-cross-sync-cycle / tenant-predicate-on-streams)". Body: contract 1.6 (Bus slice); each lint green with red+green fixtures, wired
  into CI loud-never-swallowed. Co-Authored-By trailer.

---

## EB-05 — The schema-evolution upcaster seam + the Bus survival signals into telemetry (1.8)

- **BAND.** M0.
- **ROADMAP MILESTONE.** B-M0 — the schema-evolution seam (2.8) + wiring the Bus's survival signals into the
  telemetry contract (1.8). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M0", contracts 2.8 + 1.8
  (the Bus's contribution; §4.11).
- **DEPENDS-ON.** EB-01 (the envelope/schema_ver the upcasters operate on), EB-02 (the outbox/relay whose
  depth+age it instruments), EB-03 (the consumer whose lag it instruments).
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (observability is part of the
    pass condition — a drill that emits no signal has failed; the Bus is the largest single contributor to 1.8).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.10 (schema evolution / upcasting:
    expand→migrate→contract, forward-only, (type, from_ver)→to_ver pure fns at consume, an un-upcastable
    schema_ver is term'd to DLQ never silently dropped, no rollback migrations), §4.11 (the telemetry contract
    — the exact survival signals the §8 drills read).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.8 (schema evolution / upcasters —
    forward-only) and 1.8 (telemetry signal set — consumer-lag, outbox-depth, breaker-state, causal-depth,
    per-tenant in-flight: the Phase-5 drill survival signals).
- **DELIVERABLE.** In myelin-events:
  - upcast.rs: the upcaster registry ((type, from_ver) → to_ver pure functions applied at consume); the
    expand→migrate→contract forward-only discipline (new optional fields only, consumers ignore unknowns, no
    rollback migrations); an un-upcastable schema_ver is term'd to the DLQ, never silently dropped. The
    forward-only-migration lint applies to envelope evolution (this prompt wires the Bus's compliance, the lint
    itself is the substrate M0 forward-only-migration lint).
  - telemetry.rs: emit the Bus's survival signals to the metrics port (contract 1.8) — consumer lag
    (num_pending per durable consumer), outbox depth + age, relay publish + dead-letter rate, per-aggregate
    publish latency (recorded_at → broker-ack), dedup hit-rate, per-tenant in-flight, causal-depth histogram +
    shared-root-tripwire counter. These ARE the assertions the §8 drills read — wire them now so every later
    Bus drill has a signal to assert against.
- **CONTRACTS TO IMPLEMENT.** 2.8 schema evolution / upcasters (owned); 1.8 telemetry signal set (the Bus's
  contribution — consumed/emitted, the Bus is the largest single contributor).
- **GATE / DRILLS.** No standalone catalogue drill greens here, but this prompt is the OBSERVABILITY
  precondition: the gate is that the failure-injection harness can inject a producer-kill fault and READ the
  resulting outbox-depth + dedup telemetry assertion (the harness self-test the M0→M1 boundary requires —
  master §2 M0 exit gate, "the harness can inject a fault and read a telemetry assertion"). An un-upcastable
  schema_ver fixture is term'd to the DLQ (asserted: 0 silently dropped).
- **TESTS.** Unit: an upcaster chain (v1→v2→v3) applied at consume produces the current shape; an
  un-upcastable schema_ver lands in the DLQ (not dropped); a consumer ignores an unknown forward-added field.
  Telemetry: assert each survival signal is emitted with the right name/unit to the metrics port (the §4.11
  list); the harness can read an outbox-depth assertion after an injected kill (the self-test). CDC:
  provider+consumer pair for 2.8.
- **DEFINITION OF DONE.** upcast.rs + telemetry.rs compile; 2.8 to the frozen shape; the Bus's 1.8 signals
  are emitted with correct names/units; the harness self-test (inject a kill, read an outbox-depth/dedup
  assertion) emits a dated green artifact; unit + CDC tests pass; coverage scanner + lints green; committed.
  This prompt completes the B-M0 milestone — with EB-01..EB-05 the M0→M1 exit gate (SUB-D1, SUB-D2, BUS-D4,
  BUS-D2, the three lints, the harness self-test) is fully green.
- **COMMIT.** Header "P-<NNN> M0: Schema-evolution upcaster seam + Bus survival signals into telemetry". Body:
  contracts 2.8, 1.8 (Bus contribution); the harness self-test green (inject kill → read outbox-depth/dedup);
  un-upcastable schema_ver → DLQ (0 silently dropped). Co-Authored-By trailer.

---

## EB-06 — Partition streams under (tenant, region) + residency-pin + pin the cross-cell bridge frame

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 (tenancy partition + the crypto-shred holder seam) — the partition + residency
  slice. Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contracts 12.1/12.4 (consumed) + 12.6 (frame
  pinned).
- **DEPENDS-ON.** EB-02 (the streams/outbox being partitioned), EB-03 (the per-tenant in-flight cap that now
  becomes tenant-real). Upstream (must be merged first): the M1 Tenancy contracts 12.1 (the (tenant, region)
  partition key) + 12.4 (residency_verify) — these are owned by the Tenancy system's M1 prompts; EB-06 consumes
  them, so it cannot start until the Tenancy partition key exists.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (the tenant/residency
    partition is a Tier-5 keystone — true by construction, not bolted on) + external-insights/04-hard-problems.md
    §1 (residency: region-pinning, no cross-region query path).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.2 (the subject encodes routing + ordering),
    §7.1–§7.3 (cell-local, per-(tenant, subsystem) streams, the tenant as the blast-radius + fairness unit),
    §7.4 (the cross-cell propagation FLOOR — designed-not-built; the bridge frame now PINNED).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 12.1 (the (tenant, region) partition
    key — consumed), 12.4 (residency_verify — consumed), 12.6 (CrossCellPointer{subject, type, correlation_id,
    home_cell} — the frame pinned here, BUILT in EB-12).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I (the cross-cell frame).
- **DELIVERABLE.** In myelin-events:
  - Partition the streams under the (tenant, region) key: provision per-(tenant, subsystem) streams
    (cell-local), partitioned internally by aggregate_id; the subject encodes both routing and ordering as
    evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.<event_name>. The tenant becomes the
    blast-radius + fairness unit (the EB-03 per-tenant in-flight cap is now tenant-real).
  - Residency-pin: no cross-region stream read path (the residency-pin lint, substrate M0, applies); a write
    where row.region ≠ cell.region is rejected; residency_verify attestation passes for the Bus's streams.
  - PIN the cross-cell bridge frame: the frozen CrossCellPointer{subject: OpaqueSubjectId, type: ArtifactType,
    correlation_id: CorrelationId, home_cell: CellId} type — designed-not-built. The §5 contracts are
    cell-agnostic so it extends without a rewrite. FLOOR named: single-home-cell propagation is v1; the
    cross-cell PII-free bridge BUILD (per-viewer cell-local resolution, the residency proof that no PII
    crosses, multi-cell fan-out) is the M5 follow-on EB-12.
- **CONTRACTS TO IMPLEMENT.** 12.1 (consumed — the partition key the streams are keyed under), 12.4 (consumed
  — residency_verify for the Bus's streams), 12.6 (the CrossCellPointer frame pinned — owned-as-frame, built
  in EB-12).
- **GATE / DRILLS.** The Bus's slice of CP-D3 / STOR-D5 (residency): a write where row.region ≠ cell.region
  is rejected; no cross-region stream read path exists; residency_verify attestation passes for the Bus's
  streams (CI/SCHED). Threshold: 0 cross-region stream reads; the residency-pin lint green on the Bus's
  stream-provisioning code.
- **TESTS.** Unit: a stream provisioned for (tenant, region=eu-west) rejects a read routed from a different
  region; the subject grammar round-trips the routing + ordering key; the per-tenant in-flight cap isolates
  one tenant's surge from another's stream. Residency drill: the Bus's slice of CP-D3 on the harness (an
  out-of-region write is rejected, asserted against the residency telemetry). CDC: the consumer side of
  12.1/12.4 (the Bus calls them); the CrossCellPointer frame's serde round-trip.
- **DEFINITION OF DONE.** Streams partitioned + residency-pinned; 12.1/12.4 consumed correctly; the
  CrossCellPointer frame pinned + serde-round-trips; the residency-pin lint green on the Bus code; the Bus's
  CP-D3 slice emits a dated green artifact (0 cross-region reads); the single-home-cell FLOOR + its M5 EB-12
  follow-on named in writing; committed.
- **COMMIT.** Header "P-<NNN> M1: Partition streams under (tenant, region) + residency-pin + pin cross-cell frame". Body: contracts 12.1/12.4 consumed, 12.6 frame pinned; CP-D3 (Bus slice) green (0 cross-region
  reads); residency-pin lint green; floor named (cross-cell BUILD → EB-12). Co-Authored-By trailer.

---

## EB-07 — Register the Bus as a PersonalDataHolder + wire inline-PII crypto-shred to the KMS hierarchy

- **BAND.** M1.
- **ROADMAP MILESTONE.** B-M1 — the crypto-shred holder seam (the event-log half of erasure-vs-immutability,
  the structural floor). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M1", contract 2.7 (owned) +
  10.1/10.8/11.3/11.4 (consumed).
- **DEPENDS-ON.** EB-01 (the envelope's pii_key_ref + contains_personal_data fields), EB-02 (the outbox the
  *.erased tombstones emit through), EB-06 (the (tenant, region) partition the holder operates within).
  Upstream (must be merged first): the M1 GDPR/Storage contracts 10.1 (PersonalDataHolder trait + harness
  auto-registration), 10.8 (the erasure ledger), 11.3/11.4 (the KMS hierarchy + per-subject DEK) — owned by
  the GDPR/Storage M1 prompts; EB-07 wires to them.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §2 (silent data loss / the
    holder list must be exhaustive before real data) + external-insights/04-hard-problems.md §1
    (erasure-vs-immutability: references-not-payloads + pseudonymous actor.principal + crypto-shred + tombstone
    — the structural answer).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.8 (retention + crypto-shred + tombstones —
    the references-not-payloads + crypto-shred + tombstone triad; this is the Bus's instantiation of the ONE
    platform erasure posture, X-7, by reference NOT restated) + §5.7 (the PersonalDataHolder impl signature:
    locate/erase/export).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.7 (crypto-shred / tombstone on the
    log — owned), 10.1 (PersonalDataHolder — consumed), 10.8 (erasure ledger — consumed), 11.3 (KMS hierarchy
    + KeyOrigin — consumed).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-7 (the ONE platform erasure
    posture — the [OPEN — LEGAL] residual is flagged, not an engineering gate).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    row BUS-D8 (the erasure drill this prompt greens in the live-store leg; the reaches-backups leg is EB-14/M5).
- **DELIVERABLE.** In myelin-events:
  - holder.rs: impl PersonalDataHolder for the EventBus — locate(subject) → inline-PII events + tombstone
    status; erase(subject) → crypto-shred inline-PII keys (destroy the pii_key_ref DEK) + emit *.erased
    tombstones via the outbox; export(subject) → the subject's events (references resolved via owners).
    Auto-registered by the harness so the H1–H18 holder list is EXHAUSTIVE before any real tenant data is
    written.
  - Wire the inline-PII crypto-shred to the M1 KMS hierarchy: the rare inline-PII event is envelope-encrypted
    with pii_key_ref = kms://<tenant>/<dek-epoch>/<class> (per-tenant, optionally per-subject DEK); erasure =
    destroy the key. The references-not-payloads default means MOST events carry contains_personal_data =
    false (erasing the person tombstones the identity, not the fact).
  - Hook the erasure ledger (10.8) for post-restore re-erasure: when Storage restores an older backup, the
    Bus's holder participates in the re-erasure fan-out (the key stays destroyed across a restore).
  - This is the STRUCTURAL FLOOR (the event-log half of erasure). FLOOR named: the reaches-backups leg of
    BUS-D8 is the M5 follow-on EB-14 (re-confirmed with STOR-D4). FLOOR named by reference: the [OPEN — LEGAL]
    residual lawful-basis is the ONE platform posture (10.9, X-7) — handled by the GDPR/legal track, not
    restated here; the structural floor ships regardless.
- **CONTRACTS TO IMPLEMENT.** 2.7 crypto-shred / tombstone on the log (owned); 10.1 PersonalDataHolder
  (consumed — the Bus implements the trait), 10.8 erasure ledger (consumed), 11.3 KMS hierarchy (consumed).
- **GATE / DRILLS.** BUS-D8 (live-store leg) → erase a subject; inline-PII events unrecoverable (the DEK
  destroyed); *.erased tombstones emitted; consumers degrade gracefully; reads erase-receipt + tombstone count
  (SCHED). Threshold: 0 recoverable inline-PII in the live log; tombstones present. The reaches-backups leg is
  EB-14/M5 (re-confirmed with STOR-D4). The Bus's holder participates in the STOR-D1/D2 restore-verify
  cross-seam (the event-log offset is the cross-seam cursor) — must be consistent at the restored point.
- **TESTS.** Unit: erase(subject) destroys the pii_key_ref DEK and renders the inline-PII payload
  unrecoverable; *.erased tombstones are emitted via the outbox; a consumer degrades gracefully on a tombstone;
  export(subject) returns the subject's events with references resolved. Drill scenario: BUS-D8 (live leg) on
  the harness, asserting against erase-receipt + tombstone-count telemetry. CDC: provider side of 2.7 +
  consumer side of 10.1/10.8/11.3.
- **DEFINITION OF DONE.** holder.rs compiles; 2.7 owned + 10.1/10.8/11.3 wired; BUS-D8 (live-store leg) emits
  a dated green artifact (0 recoverable inline-PII live; tombstones present — PROVEN); the holder participates
  correctly in the restore-verify cross-seam; unit + drill + CDC tests pass; coverage scanner + lints green;
  the reaches-backups FLOOR (→ EB-14) + the [OPEN — LEGAL] residual (by reference to X-7) named in writing;
  committed. This completes B-M1.
- **COMMIT.** Header "P-<NNN> M1: Bus PersonalDataHolder + inline-PII crypto-shred to KMS". Body: contract 2.7;
  10.1/10.8/11.3 consumed; BUS-D8 live-store leg green (0 recoverable inline-PII live, tombstones present);
  floors named (reaches-backups → EB-14; [OPEN — LEGAL] residual → X-7 by reference). Co-Authored-By trailer.

---

## EB-08 — The EventMatcher = the frozen myelin-query QueryAst (bounded, permission-aware interpreter)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 (the reactive layer) — the EventMatcher freeze (the prerequisite for
  Signals/Automations/Triggers all sharing one predicate surface). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2", contract 3.4.
- **DEPENDS-ON.** EB-01 (the envelope the matcher evaluates over). Upstream (must be merged first): the M2
  frozen myelin-query QueryAst (13.3, co-owned by Issues/Knowledge) — the EventMatcher IS its predicate core,
  so the freeze must land first; the M1 Identity list_objects SetExpr push-down (4.3) — the matcher composes
  with it for permission-awareness.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts at the plan layer — the matcher, saved views, Search, Notif prefs must not drift; one grammar).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.5 (the EventMatcher = the frozen QueryAst:
    the grammar And/Or/Not/Cmp/In/Has/Text/Ref, ops eq..within; the bounded interpreter — no UDFs/loops/
    recursion, statically cost-bounded, side-effect-free; permission-aware by construction — always composes
    with list_objects(viewer, read, type); one grammar, four compile targets incl. the JetStream subject
    filter + the bounded interpreter).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.4 (EventMatcher = the frozen
    QueryAst), 13.3 (myelin-query primitive frozen byte-identical — consumed; the matcher IS the QueryAst
    predicate core), 4.3 (list_objects SetExpr push-down — consumed; the matcher composes with it).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-3 (the QueryAst freeze) and
    OQ-C.
- **DELIVERABLE.** In myelin-events:
  - matcher.rs: the EventMatcher as the predicate core of the frozen myelin-query QueryAst (And/Or/Not/Cmp/In/
    Has/Text/Ref, ops eq | ne | lt | lte | gt | gte | contains | starts_with | within; Literal = Str | Num |
    Bool | Date | Principal | Ref | Null), serialised as JSON, evaluated by a custom BOUNDED interpreter — NOT
    raw CEL or JSONLogic. The validator REJECTS an AST whose worst-case cost exceeds a budget; evaluation is
    side-effect-free; no UDFs, no loops, no unbounded recursion.
  - The compile path: (a) compile to a JetStream subject filter where possible (cheap server-side prefilter),
    then (b) the bounded interpreter over the envelope for the residual predicate.
  - Permission-aware BY CONSTRUCTION: a matcher always composes with list_objects(viewer, read, type) (the
    frozen SetExpr push-down, 4.3) so no matcher/saved-view/search surface can select artifacts the subject
    can't see. The Has/Ref predicates over projection state express the relational conditions
    (CI's on: pull_request / issue.transitioned; Issues' "all blocked_by resolved").
  - Freeze the grammar BYTE-IDENTICAL with 13.3 — assert the serialisation matches the myelin-query crate's so
    matcher / saved-views / Search / Notif-prefs cannot drift.
- **CONTRACTS TO IMPLEMENT.** 3.4 EventMatcher (owned — = the frozen QueryAst); composes with 4.3
  list_objects (consumed) and shares 13.3 myelin-query (consumed, byte-identical).
- **GATE / DRILLS.** No standalone catalogue drill (the matcher is greened transitively by BUS-D6 in EB-11
  and the Signal/Trigger drills). The gate is structural: (a) the cost validator REJECTS an over-budget AST
  (a fixture exceeding the bound is rejected — the DoS-hardening property); (b) byte-identical serialisation
  with the myelin-query crate (a cross-crate round-trip fixture); (c) permission-awareness — a matcher over a
  type the viewer can't see returns zero matches after composing with list_objects (0 leak).
- **TESTS.** Unit: every operator total + side-effect-free; the cost validator rejects an over-budget AST;
  byte-identical serde with myelin-query (the no-drift property); a Has/Ref predicate over projection state
  evaluates correctly ("all blocked_by resolved"); the permission compose returns 0 matches for an unviewable
  type. CDC: provider+consumer pair for 3.4; the byte-identical 13.3 conformance test (a Bus-serialised
  QueryAst deserialised by the myelin-query crate, round-trip equal). Mutation: matcher.rs is mandatory-core;
  state and meet the floor.
- **DEFINITION OF DONE.** matcher.rs compiles; 3.4 to the frozen shape, byte-identical with 13.3; the cost
  validator rejects over-budget ASTs (dated); the permission-compose returns 0 leak; unit + CDC tests pass;
  coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M2: EventMatcher = frozen QueryAst (bounded, permission-aware)". Body: contract
  3.4; byte-identical with 13.3 (no drift); cost validator rejects over-budget; permission-compose 0 leak.
  Co-Authored-By trailer.

---

## EB-09 — Signal curation + Automations + Triggers (the four reactive primitives over the matcher)

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — Signal curation (3.1), Automations (3.2), Triggers (3.3). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2".
- **DEPENDS-ON.** EB-03 (the consumer template the Signal engine + Trigger engine are built from), EB-08 (the
  EventMatcher = QueryAst the matcher/condition columns store). Upstream (must be merged first): the M2
  myelin-flow durable timer wheel (9.3) — Trigger stale_after + Automation action.kind=workflow delegate to
  it.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §2 (the four primitives — Event / Signal
    / Automation / Trigger, each a different author/lifetime/store; don't collapse them).
  - ../../05-refined-shared-systems-architecture/event-bus.md §1.2 (the four primitives), §4.4 (Signal
    curation / dedup / severity-ranking — match, severity-rank info<notice<warning<error<critical, dedup
    within window with count=N, auto-resolve, publish to sig.<tenant>.<severity>.<rule>; the upstream defence),
    §4.6 (the Trigger state machine armed → {resolved|stale|disarmed}, fire-once-per-arming via the atomic
    guarded UPDATE; condition = the frozen QueryAst over projection state; stale_after delegated to the
    myelin-flow timer wheel), §3.4/§3.5/§3.6 (the signal/automation/trigger stores).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.1 (define_signal_rule), 3.2
    (register_automation), 3.3 (arm_trigger/disarm_trigger), 9.3 (the durable timer wheel — consumed).
- **DELIVERABLE.** In myelin-events:
  - signals.rs: the Signal engine (an infra consumer on the raw evt.* firehose — one of the excepted
    full-firehose consumers, built from the EB-03 template). define_signal_rule(SignalRule{matcher /* QueryAst
    */, severity, dedup_key_tpl, dedup_window}): match the EventMatcher; severity-rank; dedup within window
    (dedup_key = render(tpl, envelope); ON CONFLICT … count = count+1 — N identical failures collapse to one
    Signal with count=N, the storm-control primitive Notif relies on); auto-resolve (a resolving matcher);
    publish to sig.<tenant>.<severity>.<rule>. The UPSTREAM DEFENCE: product consumers + agents subscribe to
    curated Signals, never the raw firehose.
  - automations.rs: register_automation(AutomationRule{matcher, action, run_as, delegation, budget, gates}) —
    the stateless per-event reflex; action.kind=workflow invokes myelin-flow.
  - triggers.rs: arm_trigger/disarm_trigger(Trigger{owner, condition /* QueryAst over projection state */,
    arms_subject, on_resolve, stale_after}) — the stateful per-person promise; armed → {resolved|stale|
    disarmed}; fire-once-per-arming via the atomic guarded UPDATE (UPDATE trigger SET state='resolved',
    resolved_by=:event_id WHERE id=:id AND state='armed'); armed→stale on a myelin-flow durable timer set to
    stale_after; armed→disarmed on owner cancel; re-arming creates a new arming (idempotency is per-arming).
- **CONTRACTS TO IMPLEMENT.** 3.1 define_signal_rule (owned), 3.2 register_automation (owned), 3.3
  arm_trigger/disarm_trigger (owned); 9.3 the timer wheel (consumed).
- **GATE / DRILLS.** Signal/Trigger correctness is greened transitively by BUS-D3 (replay determinism, EB-10)
  and BUS-D6 (loop-safety, EB-11). The gate for THIS prompt: the dedup-window collapse is correct (N identical
  events → one Signal count=N); the Trigger fires EXACTLY ONCE per arming under concurrent resolving events
  (the atomic guard — a fire-once property test); auto-resolve resolves the matching Signal.
- **TESTS.** Unit: dedup-window collapse (10 identical failures → one Signal count=10); severity-ranking
  ordering; auto-resolve (a ci.run.passed resolves the matching ci.run.failed); the Trigger fire-once-per-arming
  under two concurrent resolving events (only one wins the guarded UPDATE); stale_after delegates to the
  myelin-flow timer (not reinvented); re-arming creates a fresh arming. CDC: provider+consumer pairs for
  3.1/3.2/3.3; the consumer side of 9.3. Mutation: triggers.rs (the fire-once guard) is mandatory-core; state
  and meet the floor.
- **DEFINITION OF DONE.** signals.rs/automations.rs/triggers.rs compile; 3.1/3.2/3.3 to the frozen shape;
  9.3 consumed; the dedup-collapse + fire-once-per-arming + auto-resolve unit tests pass (dated); CDC pairs
  exist; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M2: Signal curation + Automations + Triggers". Body: contracts 3.1/3.2/3.3; 9.3
  consumed; dedup-collapse (count=N) + Trigger fire-once-per-arming + auto-resolve proven. Co-Authored-By
  trailer.

---

## EB-10 — The firehose resume-cursor subscription protocol (built FIRST) + the reindex-from-source seam

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — the firehose transport + resume-cursor protocol (3.5, EI-04 §2.2 "build it
  FIRST") + reindex-from-source (2.6). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M2".
- **DEPENDS-ON.** EB-02 (the durable bus carrying the pointer events), EB-03 (the consumer/dedup the resume
  path leans on). This is the durable real-time transport the KN CAS floor (M3) and CRDT (M5) slot into — it
  MUST be green before any subsystem live surface rides it.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §2.2 (BUILD THE RESUME-CURSOR TRANSPORT FIRST,
    before the CRDT — the CRDT slots into this transport) + §5.3 (reindex-from-source — the only recovery path).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.3 (the firehose split + the resume-cursor
    protocol: subscribe(stream, scope, cursor?)/resume(stream, scope, last_seq); per-(stream, scope) monotonic
    seq; (last_seq, now] backfill on reconnect loses ZERO ops; resync_required → *.snapshot fallback;
    bounded-scope discipline never *, board:/doc:/channel:; per-connection in-flight caps, slow consumer →
    resync_required not unbounded buffering), §4.9 (reindex-from-source: events::reindex(scope) → owner
    replay(scope, since) emits *.snapshot via the SAME outbox→bus path; sub-artifact-granular).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.5 (firehose transport + resume-cursor
    protocol — owned-seam; KN owns the collab CRDT) and 2.6 (reindex-from-source — owned seam + every
    subsystem's replay).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-J (the firehose protocol).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows for D-10 (firehose reconnect loses zero ops — architecture §8 D-10) + BUS-D5 (reindex-from-cold parity).
- **DELIVERABLE.** In myelin-events:
  - firehose.rs: firehose::publish(stream, frame) / tail(stream, range); the NEW subscribe(stream, scope,
    cursor?) → SubStream (frames carry a per-(stream, scope) monotonic seq) and resume(stream, scope,
    last_seq) → backfill (last_seq, now] from a bounded retention window then live (LOSES ZERO OPS); an
    out-of-window last_seq yields resync_required → a *.snapshot replay fallback (the cold-rebuild path, NAMED
    not silent). scope is a BOUNDED selector, NEVER * (board:<id>/doc:<id>/channel:<id>); the transport REJECTS
    an unbounded/over-broad scope (the whitelist-not-* rule generalised); a huge board paginates its scope.
    Per-connection in-flight frame caps; a slow consumer drops to resync_required rather than buffering
    unboundedly.
  - reindex.rs: the events::reindex(scope) seam + the *.snapshot event schema (idempotent on a deterministic
    event_id from (aggregate, version)); sub-artifact-granular (CI one-run, KN page-subtree at block
    granularity). Each owner implements replay(scope, since) → emits *.snapshot via the outbox; the per-owner
    replay lands with each owner in EB-13 (M3/M4) — this prompt ships the SEAM + the *.snapshot schema + a
    small reference consumer to prove cold==live.
  - The Bus provides the pointer-event seam + the firehose subscribe/resume API + the protocol; KN owns the
    CRDT. FLOOR named: the firehose retention window per stream class is named-not-numbered (the window must
    exceed the p99 reconnect gap); it is MEASURED + tuned by D-10 in M5 (EB-14). FLOOR named: D-10 is written
    to re-run green across the KN CAS→CRDT engine_promote boundary (re-confirmed in EB-14).
- **CONTRACTS TO IMPLEMENT.** 3.5 firehose transport + resume-cursor protocol (owned-seam); 2.6
  reindex-from-source (owned-seam + the *.snapshot schema).
- **GATE / DRILLS.** D-10 (firehose) → drop a subscribed connection mid-stream on a hot board/doc/channel;
  resume(last_seq) backfills (last_seq, now] then live, 0 OPS LOST; an out-of-window last_seq yields
  resync_required → *.snapshot fallback; reads firehose seq gaps + resync count (CI). Threshold: 0 ops lost;
  resync path correct. BUS-D5 (reindex) → wipe a derived store, reindex(scope) → the rebuilt store byte-matches
  live (cold == live); reads the reindex-parity hash (SCHED — provable at M2 against a small derived consumer;
  the per-subsystem full reindex lands with each owner in EB-13). The transport REJECTS an over-broad scope
  (a fixture: scope=* is rejected).
- **TESTS.** Unit: per-(stream, scope) monotonic seq; backfill (last_seq, now] then live loses 0 ops; an
  out-of-window last_seq → resync_required; an over-broad scope is rejected; a slow consumer drops to
  resync_required (no unbounded buffering); *.snapshot idempotency on the deterministic event_id. Drill
  scenarios: D-10 (firehose reconnect, 0 ops lost) + BUS-D5 (reindex cold==live on a small derived consumer)
  on the harness, asserting against firehose-seq-gap + resync-count + reindex-parity telemetry. CDC:
  provider+consumer pairs for 3.5/2.6. Mutation: firehose.rs (the resume path) is mandatory-core; state and
  meet the floor.
- **DEFINITION OF DONE.** firehose.rs/reindex.rs compile; 3.5 + 2.6 to the frozen shape; D-10 emits a dated
  green artifact (0 ops lost; resync path correct — PROVEN); BUS-D5 emits a dated green artifact (cold==live
  on a small consumer); the over-broad-scope rejection is proven; unit + drill + CDC tests pass; coverage
  scanner + lints green; the retention-window FLOOR (→ EB-14 measured) + the CRDT-boundary re-run note (→
  EB-14) named in writing; committed.
- **COMMIT.** Header "P-<NNN> M2: Firehose resume-cursor protocol (built first) + reindex-from-source seam".
  Body: contracts 3.5, 2.6; D-10 green (0 ops lost, resync correct); BUS-D5 green (cold==live); over-broad
  scope rejected; floors named (retention window → EB-14; CRDT-boundary re-run → EB-14). Co-Authored-By trailer.

---

## EB-11 — The reactive/dispatch tier (nested causality + loop guards + reserve/settle) + the check-seam carriage

- **BAND.** M2.
- **ROADMAP MILESTONE.** B-M2 — the reactive/dispatch tier (3.6, separately-reviewed D7) + the check-seam
  carriage (2.9/§4.12, the Bus's narrow role in contract 5.9). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M2".
- **DEPENDS-ON.** EB-09 (Signals — the dispatch tier consumes curated Signals), EB-10 (the firehose/reindex
  seam). Upstream (must be merged first): the M2 Agent EventInbox::deliver explicit-first dispatch (8.6) — the
  dispatch tier's delivery target; the M1 reserve/settle cost gate (11.7) — the dispatch cost gate; the M2
  myelin-flow durable signal (9.4) — the merge-queue ci.result wait substrate.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/03-agent-native-fabric.md §5.3 + §6 (the orchestrator gotchas —
    nested causality, structural loop guards, bounded dispatch, explicit-first dispatch) +
    external-insights/01-process-and-quality-doctrine.md §3 (the loop-safety drill forces the runaway and
    observability watches the breaker trip).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.7 (the reactive/dispatch tier: nested
    causality causation_id=dispatch_event.event_id depth=+1 — flat threading FORBIDDEN; structural loop guards
    — self-guard, reference gate only an artifact_ref node re-triggers never raw text, causal-depth ceiling
    default 12, shared-causal-root tripwire → per-tenant breaker; bounded dispatch pool drops over-cap;
    explicit-first dispatch CHAT-1; reserve/settle before any run; the per-surface shed budgets OQ-K, the
    agent-mention storm), §4.12 (the check-seam carriage — what the Bus carries: ci.check.updated
    aggregate=(repo, commit_oid), ci.result the rollup signal; the Bus owns ONLY envelope conformance +
    per-aggregate ordering + at-least-once + the durable wait_for_signal substrate; it does NOT own the
    CheckStatus shape, run_attempt supersession, trust-tier gating, or the merge gate — all CI/Git).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 3.6 (reactive/dispatch tier — owned),
    8.6 (EventInbox::deliver explicit-first — consumed), 11.7 (reserve/settle — consumed), 5.9 (the Git↔CI
    CheckStatus seam — the Bus CARRIES it, CI+Git own it), 9.4 (durable signal — consumed, the ci.result wait),
    2.9 (the ci.check.updated / ci.result tokens, registered in EB-01).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md X-1 (the check seam) + OQ-K
    (shed budgets).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows BUS-D1, BUS-D3, BUS-D6 + D-11 (check-seam ordering, architecture §8 D-11).
- **DELIVERABLE.** In myelin-events:
  - dispatch.rs: the reactive/dispatch tier — consumes Signals, delivers to the Agent EventInbox (8.6).
    Disciplines: NESTED causality (a dispatched action derives causation_id = dispatch_event.event_id,
    correlation_id carried, depth = +1 — flat threading forbidden, it breaks depth-capping); STRUCTURAL loop
    guards (self-guard: drop an event whose actor.principal == the consumer's agent; reference gate: only a
    structured artifact_ref content node re-triggers, never raw text; causal-depth ceiling default 12;
    shared-causal-root tripwire: >K events on one correlation_id in a short window → per-tenant circuit
    breaker); BOUNDED dispatch worker pool that drops over-cap (a mention/event storm is bounded, never forked
    unboundedly, with a Signal); EXPLICIT-FIRST dispatch (CHAT-1: a mention notifies, does not auto-spawn a
    costed run); RESERVE/SETTLE before any run (11.7 — no balance → no execution). The tier inherits the
    per-surface shed budgets (OQ-K) — the agent-mention storm sheds the agent lane with 429 + Retry-After; the
    Bus owns the discipline, the numbers are tuned in M5 (EB-14).
  - check_seam.rs: carry ci.check.updated (envelope subject = repo#commit-<oid>/check-<context>, aggregate =
    (repo, commit_oid) so all checks for one commit are per-aggregate ordered) and ci.result (the rollup signal
    {commit_oid, overall, contexts, idem_token} the merge-queue durable workflow waits on via
    wait_for_signal("ci.result", idem_key=<merge_attempt_id>), contract 9.4). The Bus guarantees per-aggregate
    ordering on (repo, commit_oid) + at-least-once delivery + the durable wait_for_signal substrate; it does
    NOT evaluate run_attempt supersession, trust-tier gating, or the merge gate (CI/Git own those). The
    consumer (Git's check_status projection) lands EB-13/M3; the producer (CI) lands EB-13/M4. "A shaping, not
    a new engine."
- **CONTRACTS TO IMPLEMENT.** 3.6 reactive/dispatch tier (owned); 5.9 check-seam CARRIAGE (the Bus's narrow
  half — envelope + ordering + at-least-once + the wait_for_signal substrate; CI/Git own the rest); 8.6, 11.7,
  9.4 (consumed).
- **GATE / DRILLS.** BUS-D1 → kill a consumer + sever the broker during sustained publish → 0 lost, 0
  duplicate effects on reconnect (CI). BUS-D3 (replay) → replay a correlation_id tree → deterministic,
  idempotent re-drive, causality preserved (replay == original, exactly once) (CI). BUS-D6 (F9) → a
  self-triggering automation → the depth ceiling (12) + the shared-root tripwire trip the per-tenant breaker
  (halts ≤ ceiling; breaker trips) (CI). D-11 (check-seam ordering, X-1) → emit interleaved/late-arriving
  ci.check.updated for one (repo, commit_oid) across contexts + re-run attempts → per-aggregate ordering holds
  so Git's run_attempt supersession is well-defined; a stale lower-attempt re-delivery is droppable (aggregate
  order preserved; supersession deterministic) (CI). All thresholds exact — never weaken. (The M2 hard go/no-go
  AG-D4 is owned by Agent/CI; the Bus's dispatch tier must not deliver to a run executing untrusted code before
  AG-D4 is green — honoured via the reserve/settle + the runner gate, not by the Bus.)
- **TESTS.** Unit: nested causality (a dispatched action's depth = parent+1; flat threading rejected); the
  self-guard drops the agent's own event; the reference gate re-triggers only on an artifact_ref node (raw
  text does not); the depth ceiling parks at 12; the shared-root tripwire trips the per-tenant breaker;
  explicit-first (a mention notifies, 0 auto-spawn); reserve/settle blocks a no-balance run; check-seam
  per-aggregate ordering on (repo, commit_oid). Drill scenarios: BUS-D1, BUS-D3, BUS-D6, D-11 on the harness,
  asserting against per-tenant in-flight + shed-counts + causal-depth histogram + shared-root-tripwire +
  per-aggregate-publish-latency telemetry. CDC: provider+consumer pairs for 3.6 + the Bus's carriage half of
  5.9; consumer side of 8.6/11.7/9.4. Mutation: dispatch.rs (the loop guards) is mandatory-core; state and
  meet the floor.
- **DEFINITION OF DONE.** dispatch.rs/check_seam.rs compile; 3.6 owned + the 5.9 carriage half + 8.6/11.7/9.4
  consumed; BUS-D1, BUS-D3, BUS-D6, D-11 each emit a dated green artifact (0 lost/0 dup; replay==original;
  halts ≤ ceiling + breaker trips; aggregate order preserved + supersession deterministic — all PROVEN); unit
  + drill + CDC tests pass; coverage scanner + lints green; the OQ-K shed-budget numbers FLOOR (→ EB-14 tuned)
  named in writing; committed. This completes B-M2.
- **COMMIT.** Header "P-<NNN> M2: Dispatch tier (causality + loop guards + reserve/settle) + check-seam carriage". Body: contracts 3.6, 5.9 (carriage), 8.6/11.7/9.4 consumed; BUS-D1/BUS-D3/BUS-D6/D-11 green (with
  measured numbers); floor named (OQ-K shed budgets → EB-14). Co-Authored-By trailer.

---

## EB-12 — Build the cross-cell PII-free pointer bridge (the M1-frame floor follow-on)

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 (world-scale hardening + floor follow-ons) — the cross-cell bridge BUILD (the
  follow-on of the EB-06 pinned frame). Roadmap: ../../06-roadmaps/shared/event-bus.md §2 "B-M5", contract
  12.6 live.
- **DEPENDS-ON.** EB-06 (the pinned CrossCellPointer frame). Upstream (must be merged first): the M5
  control-plane multi-cell build (12.6 live) — the cross-cell bridge needs the control plane to carry the
  pointer between a tenant's cells.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/04-hard-problems.md §1 (residency: no PII crosses a cell boundary)
    + external-insights/01-process-and-quality-doctrine.md §1 (name-your-floors: this is the named follow-on
    of the M1 frame).
  - ../../05-refined-shared-systems-architecture/event-bus.md §7.4 (cross-cell propagation — the bridge frame
    CrossCellPointer{subject, type, correlation_id, home_cell}; the control plane carries ONLY this pointer,
    never payload or PII, control-plane-pii-free lint; resolution is ALWAYS cell-local — the home cell renders
    + permission-checks, only the already-filtered projection crosses, or a tombstone).
  - ../../05-refined-shared-systems-architecture/contract-index.md row 12.6 (the cross-cell PII-free pointer
    bridge — built live here).
  - ../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md OQ-I.
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GA-D8 / CP-D7 / CP-D8 (the FLOOR drills, owed when multi-cell ships).
- **DELIVERABLE.** In myelin-events (+ the control-plane integration the Tenancy system owns):
  - The cross-cell event propagation: the control plane carries ONLY CrossCellPointer{subject (opaque), type,
    correlation_id, home_cell} between a tenant's cells — never payload or PII (the control-plane-pii-free lint
    holds). Resolution is ALWAYS cell-local: a viewer in cell A wanting to render a pointer homed in cell B has
    cell B resolve(ref, viewer, mode) in B, permission-checked in B, returning only the already-rendered,
    already-permission-filtered projection (or a tombstone) — never raw rows, never PII that should stay in B.
  - The multi-cell fan-out for the streams the floor follow-ons ride (ISS cross-cell portfolio rollup, KN
    cross-cell collab, CHAT cross-org channels — the Bus carries the pointer events; the subsystems own their
    surfaces).
- **CONTRACTS TO IMPLEMENT.** 12.6 cross-cell PII-free pointer bridge (the Bus's event-propagation half —
  built live; the control plane owns the pointer transport).
- **GATE / DRILLS.** GA-D8 / CP-D7 / CP-D8 (the FLOOR drills, now owed) → multi-cell erasure fan-out per-cell
  receipt set; cell→cell migration 0 loss; the cross-cell ref carries ONLY the PII-free bridge (0 PII crosses)
  (SCHED). Threshold: 0 PII crosses a cell boundary; per-cell receipt set complete; 0 loss on migration. The
  control-plane-pii-free lint green on the bridge code.
- **TESTS.** Unit: the control plane rejects a pointer carrying anything beyond CrossCellPointer (no payload,
  no PII); cell-local resolution returns only the permission-filtered projection from the home cell; a
  tombstone is returned for an erased subject. Drill scenarios: GA-D8 / CP-D7 / CP-D8 on the harness,
  asserting against per-cell receipt + migration-loss + PII-crossing telemetry. CDC: the cross-cell pointer
  conformance test (provider+consumer).
- **DEFINITION OF DONE.** The cross-cell bridge is BUILT and live; 12.6 implemented; GA-D8/CP-D7/CP-D8 each
  emit a dated green artifact (0 PII crosses; per-cell receipts; 0 migration loss — PROVEN); the
  control-plane-pii-free lint green; unit + drill + CDC tests pass; coverage scanner + lints green; committed.
- **COMMIT.** Header "P-<NNN> M5: Build cross-cell PII-free pointer bridge". Body: contract 12.6 live;
  GA-D8/CP-D7/CP-D8 green (0 PII crosses, per-cell receipts, 0 migration loss); control-plane-pii-free lint
  green. Co-Authored-By trailer.

---

## EB-13 — Per-subsystem token validation + the check seam goes live (consumer M3 → producer M4) + per-owner replay

- **BAND.** M3 (the consumer/projection half) → M4 (the producer half). This prompt covers the Bus's narrow
  carriage role across both producer/consumer bands; the cross-band split keeps declaration order (the M4
  producer half DEPENDS-ON the M3 consumer half — see below).
- **ROADMAP MILESTONE.** B-M3/B-M4 (per-subsystem tokens + the check seam goes live). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M3 / B-M4", contracts 2.9 (per-subsystem completion) + 5.9
  (end-to-end) + 2.6 (per-owner replay).
- **DEPENDS-ON.** EB-11 (the check-seam carriage + the dispatch tier), EB-10 (the firehose + reindex seam),
  EB-01 (the taxonomy grammar each subsystem's list validates against). Cross-band: the M4 producer half
  DEPENDS-ON the M3 consumer half within this prompt's own sequencing (Git's check_status projection in M3
  before CI's producer in M4) — the index may split this into two prompts if the bands' gate boundaries
  require it; as written it is one carriage unit with two dated gate legs.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §7 (reconcile cross-component
    contracts at the plan layer — the X-1 check seam is the most load-bearing cross-subsystem contract, split
    producer/consumer across two bands; the Bus's role stays NARROW).
  - ../../05-refined-shared-systems-architecture/event-bus.md §4.12 (the check seam — the Bus's narrow role)
    + §6.1/§6.4 (the grammar each subsystem's dotted-name list validates against) + §4.9 (the per-owner replay
    for reindex).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.9 (each subsystem completes its
    list), 5.9 (the Git↔CI CheckStatus seam end-to-end — CI producer + Git gate; the Bus carries), 2.6
    (per-owner replay).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows GIT-D10 / CI-D8 (the X-1 check seam end-to-end) + D-11 (the Bus's ordering substrate it rests on) +
    GIT-D9 / KN-D7 / KN-D1 / CHAT-D1 / CHAT-D13 (the Bus's per-aggregate ordering under each producer).
- **DELIVERABLE.** In myelin-events (the Bus's carriage; the subsystems own their lists/projection/producer):
  - Validate each subsystem's completed dotted-name event list (Git/KN in M3; CI/Issues/Chat in M4) against
    the §6.1 grammar, with its schema_ver lineage and payload shapes (the Bus owns the grammar + seed; the
    subsystem owns its complete list). Provide the validation harness; each subsystem registers its list.
  - The check seam goes live end-to-end (5.9): in M3 Git ships the check_status CONSUMER/projection (built from
    the EB-03 idempotent template, idempotent on event_id, applying CI/Git's run_attempt supersession over the
    Bus's per-aggregate-ordered ci.check.updated) + awaits CI — the Bus carries it; in M4 CI ships the
    PRODUCER (emits ci.check.updated per (commit_oid, context) + the rollup ci.result signal the merge-queue
    durable workflow waits on). The Bus's role stays narrow: envelope conformance, per-aggregate ordering on
    (repo, commit_oid), at-least-once delivery, the durable wait_for_signal substrate. (Git's projection + the
    supersession rule + CI's producer are owned by the Git/CI subsystem prompts; this Bus prompt provides + drills
    the carriage they ride.)
  - Each subsystem implements replay(scope, since) so reindex-from-source covers it sub-artifact-granular (CI
    one-run, KN page-subtree at block granularity) — the Bus carries the *.snapshot; the per-owner replay is
    each subsystem's. The firehose's heaviest producers come online (ci.log.appended, KN collab op-streams,
    Chat presence/live) over the EB-10 transport; the durable bus carries only the pointer events.
- **CONTRACTS TO IMPLEMENT.** 2.9 (the per-subsystem list validation — the Bus's grammar harness); 5.9
  end-to-end carriage (the Bus's narrow half across both bands); 2.6 per-owner replay (the Bus carries the
  *.snapshot).
- **GATE / DRILLS.** M3→M4 leg: the Bus's per-aggregate ordering holds under Git's push events (GIT-D9 —
  git.ref.updated emitted iff the ref move committed, the Bus carries it) and under KN's block commits (KN-D7
  — block commit ↔ relay-publish outbox emit-iff-committed; KN-D1 — resume(scope=doc, last_seq) loses 0 ops)
  (CI). M4→M5 leg: GIT-D10 / CI-D8 (the X-1 check seam end-to-end — out-of-order/dup ci.check.updated →
  run_attempt supersession; fork self-green neutral; doubly-delivered ci.result → merge-queue wakes EXACTLY
  ONCE; 0 double-merge — the Bus's D-11 ordering guarantee is the substrate this rests on) + CHAT-D1 (sever
  gateway↔firehose → resume 0 lost/0 dup, the Bus's firehose under Chat's load) + CHAT-D13 (message-persist ↔
  event co-commit) (CI). Thresholds: 0 double-merge; 0 lost/0 dup; emit-iff-committed.
- **TESTS.** Unit: the grammar validator admits each subsystem's full list + rejects a malformed addition;
  the check_status projection is idempotent on event_id and per-aggregate ordered; the merge-queue wakes
  exactly once on a doubly-delivered ci.result. Drill scenarios: GIT-D9/KN-D7/KN-D1 (M3 leg) and
  GIT-D10/CI-D8/CHAT-D1/CHAT-D13 (M4 leg) on the harness — the Bus's carriage half asserted against
  per-aggregate-publish-latency + dedup + firehose-seq-gap telemetry. CDC: the carriage half of 5.9 + the
  per-subsystem 2.9 list conformance + 2.6 per-owner replay.
- **DEFINITION OF DONE.** The grammar validation harness is live + each subsystem's list validated; the check
  seam is live end-to-end (Git consumer M3, CI producer M4) with the Bus carrying it; the M3 leg
  (GIT-D9/KN-D7/KN-D1) and the M4 leg (GIT-D10/CI-D8/CHAT-D1/CHAT-D13) each emit a dated green artifact (0
  double-merge; 0 lost/0 dup; emit-iff-committed — PROVEN); unit + drill + CDC tests pass; coverage scanner +
  lints green; committed. (Note: if the index requires the M3 consumer leg and the M4 producer leg as separate
  committable units, this prompt splits at the band boundary — the M4 half DEPENDS-ON the M3 half; the seam is
  never collapsed into one band.)
- **COMMIT.** Header "P-<NNN> M3/M4: Per-subsystem token validation + check seam live + per-owner replay".
  Body: contracts 2.9, 5.9 (end-to-end carriage), 2.6; GIT-D9/KN-D7/KN-D1 (M3 leg) + GIT-D10/CI-D8/CHAT-D1/
  CHAT-D13 (M4 leg) green (with measured numbers); the producer/consumer split kept across bands.
  Co-Authored-By trailer.

---

## EB-14 — World-scale hardening: the 30× surge, per-aggregate order at QPS, crypto-shred to backups, retention-window tuning, the E2E spine

- **BAND.** M5.
- **ROADMAP MILESTONE.** B-M5 — world-scale hardening + the named floor follow-ons that come due here (the
  QPS-order floor, the backups-erasure floor, the retention-window floor, the CRDT-boundary re-run). Roadmap:
  ../../06-roadmaps/shared/event-bus.md §2 "B-M5", drills BUS-D7/BUS-D9/BUS-D8 + D-10-at-scale + the four E2E.
- **DEPENDS-ON.** EB-11 (the dispatch tier whose shed lanes BUS-D7 drills), EB-02 (the per-aggregate ordering
  BUS-D9 proves at QPS), EB-07 (the live-store crypto-shred BUS-D8 extends to backups), EB-10 (the firehose
  whose retention window D-10 tunes), EB-13 (the check seam + all five subsystems live, the spine the E2E ride).
  Upstream (must be merged first): the M5 KN CRDT (the engine_promote boundary D-10 re-runs across) + Storage
  restore-verify at cell scale (STOR-D2/STOR-D4) — the Bus's holder participates in the cross-seam restore.
- **CANON DOCS.**
  - ../../../VISION.md + external-insights/01-process-and-quality-doctrine.md §3 (prove-it at world-scale: the
    30× surge + the QPS-order + the backups-erasure forced by drills, observability watching the human lane
    hold) + external-insights/04-hard-problems.md §5.2 (the column-store seam — measured-not-predicted).
  - ../../05-refined-shared-systems-architecture/event-bus.md §2.3 (per-aggregate ordering at production QPS —
    the D-9 floor), §4.7 (the OQ-K shed budgets the surge tunes), §4.8 (crypto-shred reaching backups), §4.3
    (the retention-window sizing the D-10 drill measures), §7.5 (the column-store/time-series seam BUS-6 —
    promoted ONLY on measured volume).
  - ../../05-refined-shared-systems-architecture/contract-index.md rows 2.3 (per-aggregate ordering at QPS),
    2.7 (crypto-shred to backups).
  - ../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md
    rows BUS-D7 (30× surge), BUS-D9 (per-aggregate order at QPS), BUS-D8 (erasure reaches backups) + the four
    E2E scenarios (E2E-1..E2E-4, the Bus is the spine each rides).
- **DELIVERABLE.** In myelin-events:
  - Tune + prove the OQ-K shed budgets (the named M2 floor): the 30× agent-surge family — the protected
    human/control lane holds while the agent lane sheds (429 + Retry-After honoured) and other tenants are
    unaffected (the per-tenant bulkhead).
  - Prove per-aggregate ordering AT PRODUCTION QPS (the M0 correctness floor's follow-on): burst force-pushes
    to one hot ref + a burst of sends to one hot channel under load → per-ref + per-conversation order
    preserved at target QPS, parallel across aggregates.
  - Extend crypto-shred to backups (the M1 live-store floor's follow-on): inline-PII events unrecoverable in
    backups (the key excluded from backup), not just live DBs — re-confirmed with STOR-D4.
  - Tune the firehose retention window per stream class (the named M2 floor → measured here): so the
    (last_seq, now] backfill exceeds the p99 reconnect gap (D-10 measures it); re-run D-10 GREEN across the KN
    CAS→CRDT engine_promote boundary (the floor's promotion is itself drilled).
  - The column-store/time-series seam (BUS-6): NAMED, not built — promotion to a ClickHouse-class tier behind
    the BusTransport trait happens ONLY if a per-stream volume is MEASURED to outgrow JetStream at degraded
    latency (post-M5, measured-not-predicted). This prompt records the measurement gate, not a build.
  - The Bus as the spine of all four E2E scenarios (it carries every chained mutation): E2E-1 (the PR pane's
    live check-update + the per-ref cache bust), E2E-2 (the flagship — the Signal that wakes the triage agent,
    the durable ci.result wait, the nested-causality run), E2E-3 (reindex-from-cold == live via the *.snapshot
    path), E2E-4 (the bus as a holder in the DSAR fan-out — inline-PII events crypto-shredded). The subsystems
    own the E2E orchestration; the Bus proves its carriage holds under each.
- **CONTRACTS TO IMPLEMENT.** No new contract surface — this prompt HARDENS the owned contracts (2.3 at QPS,
  2.7 to backups, 3.5 retention window, 3.6 shed budgets) and proves the E2E spine. It implements the
  measured numbers the earlier prompts named as floors.
- **GATE / DRILLS.** BUS-D7 (F6) → 30× agent publish surge on one tenant → the human/control lane holds, the
  agent lane sheds (429 + Retry-After), other tenants unaffected; reads shed-counts/lane + per-tenant RED
  (SCHED). BUS-D9 → burst force-pushes to one hot ref + sends to one hot channel under load → per-aggregate
  order preserved at target QPS, parallel across aggregates; reads per-aggregate publish latency (SCHED).
  BUS-D8 (reaches-backups leg) + STOR-D4 → 0 recoverable inline-PII in the log AND backups; tombstones present
  (SCHED). D-10 RE-GREEN across the KN CAS→CRDT engine_promote boundary → the resume-cursor transport survives
  the CRDT promotion (0 ops lost) (CI/SCHED). The four E2E scenarios green (E2E-1..E2E-4) — the Bus is the
  spine each rides; each emits its named green artifact. STOR-D2 at cell scale re-confirmed (the Bus's holder
  consistent at the restored point under world-scale load). All thresholds exact — never weaken; a red gate is
  a dated "claimed, not proven" scorecard row.
- **TESTS.** Drill scenarios (the bulk of this prompt): BUS-D7, BUS-D9, BUS-D8-backups, D-10-across-boundary,
  and the Bus's slice of E2E-1..E2E-4 — all on the failure-injection harness at 30× load with mixed principal
  kinds, asserting against the §4.11 survival signals (shed-counts, per-tenant in-flight, per-aggregate publish
  latency, holder erase receipts, firehose seq gaps). Unit: the retention window > p99 reconnect gap is
  asserted from measured data; the column-store promotion gate is recorded (measured, not predicted). CDC: the
  hardened-contract conformance re-runs.
- **DEFINITION OF DONE.** BUS-D7, BUS-D9, BUS-D8-backups, D-10-across-the-CRDT-boundary, and E2E-1..E2E-4 each
  emit a dated green artifact (human lane holds + agent sheds + cross-tenant 0; QPS-order preserved; 0
  recoverable inline-PII in backups; 0 ops lost across the engine_promote; the four E2E green — all PROVEN);
  STOR-D2 at cell scale re-confirmed; the retention window is measured + tuned; the column-store seam is NAMED
  with its measured promotion gate (not built); coverage scanner + lints green; committed. This completes B-M5;
  the Bus's M6 contribution (the self-hosting CI graph green + the truth-up pass confirming 0 red earlier-band
  Bus gates) is exercised by the master M6 dogfood prompts — the Bus adds no new engine in M6.
- **COMMIT.** Header "P-<NNN> M5: Bus world-scale hardening (surge / QPS-order / backups-erasure / E2E spine)".
  Body: BUS-D7/BUS-D9/BUS-D8-backups/D-10-across-boundary + E2E-1..E2E-4 green (with measured numbers);
  retention window tuned; column-store seam named (measured promotion gate, not built); STOR-D2 cell-scale
  re-confirmed. Co-Authored-By trailer.

---

## Digest (this file)

14 prompts (EB-01..EB-14) operationalize the entire event-bus roadmap, every milestone B-M0..B-M6 covered with
no gap, floors paired to their follow-ons. By band:
- **M0 (5):** EB-01 envelope+taxonomy freeze · EB-02 outbox+relay (SUB-D1/BUS-D4) · EB-03 consumer template+dedup (SUB-D2/BUS-D2) · EB-04 the three Bus lints (red+green fixtures) · EB-05 schema-evo seam + survival signals + the harness self-test. Together they green the M0→M1 exit gate.
- **M1 (2):** EB-06 (tenant,region) partition + residency-pin + cross-cell frame pinned · EB-07 PersonalDataHolder + crypto-shred to KMS (BUS-D8 live leg).
- **M2 (4):** EB-08 EventMatcher = frozen QueryAst · EB-09 Signals+Automations+Triggers · EB-10 firehose resume-cursor (built FIRST, D-10) + reindex seam (BUS-D5) · EB-11 dispatch tier + check-seam carriage (BUS-D1/BUS-D3/BUS-D6/D-11).
- **M3/M4 (1):** EB-13 per-subsystem token validation + the check seam live consumer-then-producer (GIT-D9/KN-D7/KN-D1 → GIT-D10/CI-D8/CHAT-D1/CHAT-D13); splits at the band boundary if the index requires.
- **M5 (2):** EB-12 cross-cell PII-free bridge BUILD (GA-D8/CP-D7/CP-D8) · EB-14 world-scale hardening — 30× surge BUS-D7 / per-aggregate order at QPS BUS-D9 / crypto-shred to backups BUS-D8 / retention-window + CRDT-boundary D-10 / the four E2E spine.
- **M6:** no new Bus engine; the Bus's M6 gate (self-hosting CI graph green + truth-up pass) is exercised by the master dogfood prompts.

Drill coverage: SUB-D1, SUB-D2, BUS-D1..BUS-D9, D-10, D-11 — all 13 owed Bus drills are greened by a named
prompt's GATE/DRILLS field; the permanent STOR-D1/D2 restore-verify cross-seam (the Bus's holder is the
event-log-offset cursor) is participated in by EB-02/EB-07 and re-confirmed at cell scale by EB-14.
